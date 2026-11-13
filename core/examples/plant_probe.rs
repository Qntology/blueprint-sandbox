//! 실제 모델로 합성 발전소 시나리오 전체를 돕니다 (SigLIP2 크롭 · gocr 인식 · Zoro-1 판단 · 글쓰기).
//!
//!   cargo run -p blueprint-core --release --example plant_probe -- [현장 스캔 폴더] [결과 JSON]
//!
//! `BLUEPRINT_DOMAIN=aidc` 면 AI 데이터센터 원장 · 마스터(MEP · SLD) · 시나리오 D1 ~ D7 로 같은 측정을 합니다 (추가 개선안).
//! `BLUEPRINT_LANG=en` 이면 영어 화면처럼 절차서 원인 요약을 영어로 씁니다. `--no-reads` 면 도면 · 스캔을 읽지 않고 시나리오만 돕니다
//! (현장 보고는 시나리오 정답 값).
//!
//! 1. 사전 인덱싱(Phase A): 마스터 P&ID 다섯 장을 읽어 KKS 를 찾고 원장 좌표와 맞춥니다 (찾은 비율 · 상자 겹침).
//! 2. 현장 스캔: `python scripts/gen_pid_drawings.py` 가 만든 점검표 · Red-line 을 읽어 정답과 맞춥니다.
//! 3. 시나리오 P1 ~ P7: 엔진에 실제 판단 · 글쓰기 모델을 꽂아 판정 · 확신도 · SDS 축 · AOP 초안 · 단계 시간을 기록합니다.
//!    현장 보고는 2 에서 읽은 값을 씁니다 (읽지 못하면 시나리오 정답 값).
//!
//! 모델은 앱 데이터 폴더(`%LOCALAPPDATA%\blueprint\models`, zoro-gravity 에서 가져온 것)에 있어야 합니다.

use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::runtime;
use blueprint_core::case::{FieldReport, ReadTimings, Signature};
use blueprint_core::domain::DomainMode;
use blueprint_core::extract::{extract, DocKind, Extracted, PageText};
use blueprint_core::lattice::Lattice;
use blueprint_core::policy::AutomationPolicy;
use blueprint_core::scenario::ScenarioSet;
use blueprint_core::state_machine::{Engine, Outcome};
use chrono::Utc;
use serde_json::{json, Value};

fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: [f32; 4]| (r[2] - r[0]) * (r[3] - r[1]);
    inter / (area(a) + area(b) - inter).max(1e-6)
}

fn read(rt: &runtime::Runtime, db: &Lattice, path: &PathBuf) -> Result<(Extracted, ReadTimings, f64)> {
    let img = image::open(path).with_context(|| format!("이미지를 열 수 없습니다: {}", path.display()))?.to_rgb8();
    let t = Instant::now();
    let pages = rt.reader.read_pages(std::slice::from_ref(&img))?;
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let p = &pages[0];
    let timings = ReadTimings { layout_ms: p.timings.layout_ms, siglip2_ms: p.timings.siglip2_ms, ocr_ms: p.timings.ocr_ms, crops: p.crops.len(), dropped: p.plan.dropped.len(), blank: p.plan.blank.len(), skipped: 0 };
    Ok((extract(&PageText::from_reading("probe", img.dimensions(), p), db), timings, ms))
}

fn outcome_json(o: &Outcome) -> Value {
    match o {
        Outcome::FieldRetry { verdict, reason } => json!({ "outcome": "field_retry", "verdict": verdict.code(), "reason": reason }),
        Outcome::AwaitingField { judgement, reason } => json!({ "outcome": "awaiting_field", "judgement": judgement, "reason": reason }),
        Outcome::SupervisorReview { judgement, reason } => json!({ "outcome": "supervisor_review", "judgement": judgement, "reason": reason }),
        Outcome::AutoFinalized { archive_id, .. } => json!({ "outcome": "auto_finalized", "archive": archive_id }),
        Outcome::Finalized { archive_id } => json!({ "outcome": "finalized", "archive": archive_id }),
        Outcome::Blocked { verdict, reason } => json!({ "outcome": "blocked", "verdict": verdict.code(), "reason": reason }),
    }
}

fn main() -> Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    // `--no-reads`: 마스터 도면 · 현장 스캔을 읽지 않고 시나리오만 돕니다 (현장 보고는 시나리오 정답 값). 영어 초안처럼 판단 · 글쓰기만 볼 때 씁니다.
    let no_reads = all.iter().any(|a| a == "--no-reads");
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    let field_dir = PathBuf::from(args.first().cloned().unwrap_or_else(|| "out/field".into()));
    let out_path = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "out/plant_probe.json".into()));
    let mode = std::env::var("BLUEPRINT_DOMAIN").ok().and_then(|k| DomainMode::from_key(&k)).unwrap_or_default();
    // 화면 언어 (앱의 설정과 같음). 영어면 Zoro-1 이 절차서 원인 요약을 영어로 씁니다.
    if let Some(lang) = std::env::var("BLUEPRINT_LANG").ok().and_then(|l| blueprint_core::i18n::Lang::parse(&l)) {
        blueprint_core::i18n::set(lang);
    }
    let db = Lattice::embedded_for(mode)?;
    let set = ScenarioSet::embedded_for(mode)?;
    println!("[도메인] {} · 설비 {} · 센서 {} · 시나리오 {} · 언어 {}", mode.label(), db.node_count(), db.sensors().len(), set.scenarios.len(), blueprint_core::i18n::current().code());
    let t = Instant::now();
    let rt = runtime::open()?;
    println!("[엔진] {} · 모델 열기 {:.1}s", rt.backend, t.elapsed().as_secs_f64());

    // 1. 사전 인덱싱 (마스터 도면)
    let mut drawings = Vec::new();
    for d in db.drawings().iter().filter(|_| !no_reads) {
        let path = PathBuf::from(format!("core/fixtures/drawings/{}.png", d.id));
        let (x, timings, ms) = read(&rt, &db, &path)?;
        let expected = db.on_drawing(&d.id);
        let found: Vec<&str> = x.kks_hits.iter().map(|h| h.code.as_str()).collect();
        let hit = expected.iter().filter(|e| found.contains(&e.kks.as_str())).count();
        let ious: Vec<f32> = x.kks_hits.iter().filter_map(|h| db.equipment(&h.code).filter(|e| e.drawing == d.id).map(|e| iou(h.bbox, e.bbox))).collect();
        let mean_iou = if ious.is_empty() { 0.0 } else { ious.iter().sum::<f32>() / ious.len() as f32 };
        println!(
            "[도면] {} {:?} {:?} · KKS {hit}/{} · 상자 IoU {:.2} · 교정 {} · {:.1}s (조각 {} · 버림 {})",
            d.id,
            x.drawing,
            x.drawing_rev,
            expected.len(),
            mean_iou,
            x.kks_hits.iter().filter(|h| h.corrected).count(),
            ms / 1e3,
            timings.crops,
            timings.dropped
        );
        drawings.push(json!({ "id": d.id, "read_id": x.drawing, "rev": x.drawing_rev, "expected": expected.len(), "found": hit, "extra": x.kks_hits.len() - hit, "mean_iou": mean_iou, "corrected": x.kks_hits.iter().filter(|h| h.corrected).count(), "ms": ms, "crops": timings.crops, "dropped": timings.dropped, "blank": timings.blank }));
    }

    // 2. 현장 스캔
    let mut scans = Vec::new();
    let mut read_reports: Vec<(String, FieldReport)> = Vec::new();
    for s in &set.scenarios {
        for scan in &s.scans {
            if no_reads {
                read_reports.push((s.id.clone(), s.field_reports(|w| db.person(w).map(|p| p.name.clone())).remove(0)));
                continue;
            }
            let path = field_dir.join(&scan.image);
            if !path.exists() {
                println!("[스캔] {} 없음 (python scripts/gen_pid_drawings.py 로 만드세요)", path.display());
                continue;
            }
            let (x, timings, ms) = read(&rt, &db, &path)?;
            let truth = s.field_reports(|w| db.person(w).map(|p| p.name.clone())).remove(0);
            let checks = json!({
                "kind": x.kind.map(|k| k.label()),
                "kks": x.kks == truth.kks,
                "tag": x.tag == truth.tag,
                "reading": x.reading == truth.reading,
                "moc_no": scan.moc_no.is_none() || x.moc_no == scan.moc_no,
                "old": scan.old.is_none() || x.old_value == scan.old,
                "worker": x.worker_id == truth.worker_id,
                "signed": x.signed,
            });
            println!("[스캔] {} → {:?} kks {:?} tag {:?} 값 {:?} MOC {:?} 기존 {:?} 서명 {} · 교정 {:?} · 확인 {:?} · {:.1}s", scan.image, x.kind, x.kks, x.tag, x.reading, x.moc_no, x.old_value, x.signed, x.corrections, x.uncertain, ms / 1e3);
            scans.push(json!({ "image": scan.image, "checks": checks, "corrections": x.corrections, "uncertain": x.uncertain, "ms": ms }));
            // 읽은 값으로 현장 보고를 만듭니다. 못 읽은 칸은 정답 값으로 채워 엔진 시험을 이어 갑니다.
            let mut r = truth.clone();
            if matches!(x.kind, Some(DocKind::GaugeCard) | Some(DocKind::Redline)) {
                r.kks = x.kks.clone().or(r.kks);
                r.tag = x.tag.clone().or(r.tag);
                r.reading = x.reading.or(r.reading);
                r.moc_no = x.moc_no.clone().or(r.moc_no);
                r.old_value = x.old_value.or(r.old_value);
                r.corrections = x.corrections.clone();
                r.uncertain = x.uncertain.clone();
                if x.signed {
                    r.signature = Some(Signature::on_paper());
                }
            }
            r.timings = Some(timings);
            read_reports.push((s.id.clone(), r));
        }
    }

    // 3. 시나리오
    let mut engine = Engine::new(b"probe", AutomationPolicy::default(), None);
    engine.reset_for(db.terms().clone());
    engine.model = Some(std::sync::Arc::new(rt.model));
    engine.writer = Some(std::sync::Arc::new(rt.writer));
    let mut results = Vec::new();
    for s in &set.scenarios {
        let id = format!("INC-{}", s.id);
        let mut incident = s.incident(&set, &id);
        let reports: Vec<FieldReport> = read_reports.iter().filter(|(sid, _)| *sid == s.id).map(|(_, r)| r.clone()).collect();
        if incident.alarm.is_none() {
            incident.reports = reports.clone();
        }
        let t = Instant::now();
        let first = engine.run(&db, incident, Utc::now())?;
        let mut second = Value::Null;
        if s.alarm.is_some() && !reports.is_empty() {
            let r = reports[0].clone();
            let at = r.taken_at;
            second = outcome_json(&engine.attach_report(&db, &id, r, at)?);
        }
        let secs = t.elapsed().as_secs_f64();
        let Some(session) = engine.store.active(&id) else {
            println!("[{}] {:?} ({secs:.1}s)", s.id, first);
            results.push(json!({ "id": s.id, "first": outcome_json(&first), "secs": secs }));
            continue;
        };
        let a = &session.analysis;
        let decided = a.substantive.as_ref().map(|x| &x.decided);
        let expected = s.expect["verdict"].as_str().unwrap_or("");
        let verdict = session.judgement.as_ref().map(|j| j.verdict.code()).unwrap_or("");
        println!(
            "[{}] 판정 {verdict} (기대 {expected}) · 확신도 {:?} · 축 {:?} · 원인 {:?} · AOP {:?} 모델글 {:?} 문제 {:?} · {secs:.1}s",
            s.id,
            session.judgement.as_ref().and_then(|j| j.confidence),
            decided.map(|d| d.axes.clone()),
            a.causal_root().map(|r| (r.kks.clone(), r.hops, (r.lag_secs * 10.0).round() / 10.0)),
            a.draft.as_ref().map(|d| d.aop_id.clone()),
            a.draft.as_ref().map(|d| d.by_model),
            a.draft.as_ref().map(|d| d.problems.clone()),
        );
        if let Some(d) = &a.draft {
            println!("      초안: {}", d.summary);
        }
        for st in &a.stages {
            println!("      Stage {} {} {:.1}ms {:?} {}", st.stage, st.name, st.ms, st.status, st.summary);
        }
        results.push(json!({
            "id": s.id,
            "expected": expected,
            "verdict": verdict,
            "first": outcome_json(&first),
            "after_report": second,
            "confidence": session.judgement.as_ref().and_then(|j| j.confidence),
            "asked": decided.map(|d| &d.asked),
            "axes": decided.map(|d| &d.axes),
            "root": a.causal_root(),
            "draft": a.draft,
            "stages": a.stages,
            "secs": secs,
        }));
    }
    let report = json!({ "backend": rt.backend, "drawings": drawings, "scans": scans, "scenarios": results, "at": Utc::now().to_rfc3339() });
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;
    println!("[결과] {}", out_path.display());
    blueprint_core::ai::accel::finish_process(&rt.device, 0);
    Ok(())
}
