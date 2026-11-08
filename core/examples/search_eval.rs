//! 자연어 검색 평가 (logis-center 방식: 조건 필터를 거친 화면 순서로 채점). 한국어 · 영어.
//!
//!   cargo run --release -p blueprint-core --example search_eval [-- --verbose]
//!
//! 질의 묶음마다 세 가지로 잽니다: 규칙만(모델 없음) · +Stanza(품사 · 원형) · +Stanza+임베딩(질의 벡터 · NMS 조각).
//! 모델은 모델 폴더의 `stanza` · `embed` 를 씁니다 (`blueprint-models import embed` · `import stanza <변환본>`). 없으면 그 모드를 건너뜁니다.
//! 발전소 · AI 데이터센터 원장, 지금 계측 없음(질적 상태는 힌트)과 시나리오 재생 끝 시각(질적 상태가 필터)을 모두 잽니다.

use std::time::Instant;

use blueprint_core::domain::DomainMode;
use blueprint_core::lattice::Lattice;
use blueprint_core::search::*;

struct Score {
    mrr: f64,
    hit1: usize,
    within: usize,
    n: usize,
    ms: f64,
}

fn run(db: &Lattice, corpus: &Corpus, queries: &[(&str, &str, usize)], models: Option<&SearchModels>, verbose: bool) -> Score {
    let (mut mrr, mut hit1, mut within) = (0.0, 0, 0);
    let mut ms = 0.0;
    for (q, want, allowed) in queries {
        let t = Instant::now();
        let r = search_with(db, corpus, q, models);
        ms += t.elapsed().as_secs_f64() * 1000.0;
        let rank = r.hits.iter().position(|h| h.doc.id == *want).map(|p| p + 1);
        mrr += rank.map_or(0.0, |k| 1.0 / k as f64);
        hit1 += usize::from(rank == Some(1));
        within += usize::from(rank.is_some_and(|k| k <= *allowed));
        if verbose || rank.is_none_or(|k| k > *allowed) {
            let conds: Vec<String> = r.conditions.iter().map(|c| format!("{} {}{}", c.kind, c.text, if c.applied { "" } else { "(힌트)" })).collect();
            let top: Vec<String> = r.hits.iter().take(3).map(|h| h.doc.id.clone()).collect();
            let segs: Vec<String> = r.segments.iter().map(|s| format!("'{}'→{}({:.1})", s.text, s.label, s.z)).collect();
            println!("    {} {q} → {want} 순위 {} · 위 {top:?} · 조건 {} · 조각 {}", if rank.is_some_and(|k| k <= *allowed) { "·" } else { "✗" }, rank.map_or("-".into(), |k| k.to_string()), conds.join(", "), segs.join(" "));
        }
    }
    let n = queries.len();
    Score { mrr: mrr / n as f64, hit1, within, n, ms: ms / n as f64 }
}

fn main() {
    let verbose = std::env::args().any(|a| a == "--verbose");
    if std::env::args().any(|a| a == "--nms") {
        // 문턱 진단: 질의마다 NMS 후보(창 · 최고 개념 · 코사인 · z · 2위와의 차이 · 통과)
        let m = SearchModels::load_default();
        let plant = Lattice::embedded().expect("원장");
        for (q, want, _) in EVAL_QUERIES.iter().chain(EVAL_QUERIES_EN.iter()).chain(EVAL_PARAPHRASE_KO.iter()).chain(EVAL_PARAPHRASE_EN.iter()) {
            println!("{q} (정답 {want})");
            for c in nms_candidates(&plant, q, &m) {
                println!("   {} '{}' → {} cos {:.3} z {:.2} margin {:.3}", if c.passed { "✓" } else { " " }, c.text, c.label, c.cos, c.z, c.margin);
            }
        }
        return;
    }
    let t = Instant::now();
    let full = SearchModels::load_default();
    println!("모델: {} ({:.1} s) {:?}", full.summary(), t.elapsed().as_secs_f32(), full.notes);
    let stanza_only = SearchModels::load(Some(&blueprint_core::ai::model_store::model_dir("stanza")), None);
    let mut modes: Vec<(&str, Option<&SearchModels>)> = vec![("규칙만", None)];
    if !stanza_only.stanza.is_empty() {
        modes.push(("+Stanza", Some(&stanza_only)));
    }
    if full.embedder.is_some() {
        modes.push(("+Stanza+임베딩", Some(&full)));
    }

    let plant = Lattice::embedded().expect("원장");
    let dc = Lattice::embedded_for(DomainMode::AiDataCenter).expect("데이터센터 원장");
    let plant_still = Corpus::build(&plant, None, None);
    let plant_live = live_corpus(&plant, EVAL_LIVE_SCENARIO).expect("시나리오");
    let dc_still = Corpus::build(&dc, None, None);
    let dc_live = live_corpus(&dc, EVAL_AIDC_LIVE_SCENARIO).expect("시나리오");
    // 처음 한 번 검색 표 · 개념 뱅크를 임베딩합니다 (그 뒤는 캐시). 질의 시간에 넣지 않으려고 미리 돌립니다.
    if let Some(m) = modes.last().and_then(|m| m.1) {
        let t = Instant::now();
        for (db, corpus) in [(&plant, &plant_still), (&dc, &dc_still)] {
            let _ = search_with(db, corpus, "준비", Some(m));
        }
        println!("검색 표 · 개념 뱅크 임베딩 {:.1} s", t.elapsed().as_secs_f32());
    }
    let sets: Vec<(&str, &Lattice, &Corpus, &[(&str, &str, usize)])> = vec![
        ("발전소 · 한국어", &plant, &plant_still, &EVAL_QUERIES),
        ("발전소 · 영어", &plant, &plant_still, &EVAL_QUERIES_EN),
        ("발전소 지금 계측 · 한국어", &plant, &plant_live, &EVAL_LIVE_QUERIES),
        ("발전소 지금 계측 · 영어", &plant, &plant_live, &EVAL_LIVE_QUERIES_EN),
        ("데이터센터 · 한국어", &dc, &dc_still, &EVAL_AIDC_QUERIES),
        ("데이터센터 · 영어", &dc, &dc_still, &EVAL_AIDC_QUERIES_EN),
        ("데이터센터 지금 계측 · 한국어", &dc, &dc_live, &EVAL_AIDC_LIVE_QUERIES),
        ("데이터센터 지금 계측 · 영어", &dc, &dc_live, &EVAL_AIDC_LIVE_QUERIES_EN),
        ("뜻풀이 · 한국어", &plant, &plant_still, &EVAL_PARAPHRASE_KO),
        ("뜻풀이 · 영어", &plant, &plant_still, &EVAL_PARAPHRASE_EN),
        ("뜻풀이 · 데이터센터", &dc, &dc_still, &EVAL_AIDC_PARAPHRASE),
    ];
    println!();
    print!("| 묶음 | 질의 |");
    for (name, _) in &modes {
        print!(" {name} MRR · Hit@1 · 허용 |");
    }
    println!();
    print!("| --- | --- |");
    for _ in &modes {
        print!(" --- |");
    }
    println!();
    let mut rows = Vec::new();
    for (name, db, corpus, queries) in &sets {
        let mut row = format!("| {name} | {} |", queries.len());
        for (mode, m) in &modes {
            if verbose {
                println!("  [{name} · {mode}]");
            }
            let s = run(db, corpus, queries, *m, verbose);
            row.push_str(&format!(" {:.3} · {}/{} · {}/{} ({:.0} ms) |", s.mrr, s.hit1, s.n, s.within, s.n, s.ms));
            if !verbose && s.within < s.n {
                println!("  ↑ [{name} · {mode}] 허용 순위 밖 {}건", s.n - s.within);
            }
        }
        rows.push(row);
    }
    println!();
    for r in rows {
        println!("{r}");
    }
}
