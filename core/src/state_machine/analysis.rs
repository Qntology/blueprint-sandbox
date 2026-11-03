//! 사건 하나의 분석: Gate 0 → 4 를 한 번 돌아 화면과 결재에 필요한 것을 모두 만듭니다.
//!
//! 확정(Gate 5)은 여기서 하지 않습니다. 파이프라인(`pipeline::Engine`)이 이 결과로 자동 확정 · 교대감독 대기 · 현장 대기를 정합니다.
//! 현장 보고가 들어오면 같은 사건을 다시 분석합니다 (보고가 늘어난 입력으로 처음부터).
//!
//! 단계 시간은 그 실행에서 잰 값입니다 (Stage 1 · 2 는 문서를 읽을 때 잰 값을 보고가 들고 옵니다).

use std::time::Instant;

use anyhow::Result;
use serde::Serialize;
use serde_json::Value;

use super::gate0_router::{self, Route};
use super::gate1_formal::{self, FieldFormal, SignalReport, SignalState};
use super::gate2_consistency::{self, DriftFinding, FieldCheck, MocCheck, MocOutcome, TruthRow, VoteResult};
use super::gate3_substantive::{self, Substantive};
use super::gate4_5_finalize::{select_aop, template_summary};
use crate::ai::trace;
use crate::case::{AlarmKind, FieldKind, Incident};
use crate::causal::{event_chain, ChainEvent};
use crate::decision::DecisionModel;
use crate::drafting::{draft_aop, AopDraft, TextWriter};
use crate::interlock::{self, InterlockState};
use crate::lattice::Lattice;
use crate::plant::fmt_value;
use crate::sds::{trigger_at, SdsSnapshot, SdsTrigger, TriggerKind};
use crate::stages::{record, StageRecord, StageStatus};
use crate::timeseries::{metrics, TimeSeriesMetrics, BASELINE_SECS};
use crate::verdict::{OutcomeClass, Verdict};

/// 판단 · 글쓰기 모델 (없으면 규칙과 템플릿만)
#[derive(Clone, Copy, Default)]
pub struct Brains<'a> {
    pub model: Option<&'a dyn DecisionModel>,
    pub writer: Option<&'a dyn TextWriter>,
}

/// 분석 뒤 사건이 갈 곳
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Next {
    /// 판정 후보가 정해짐 (자동 확정 또는 교대감독)
    Ready,
    /// 현장 게이지 · 재점검을 기다림
    AwaitingField,
    /// 현장 단말에서 서명을 다시 받아야 함 (사건을 열지 않음)
    FieldRetry,
    /// 위조 · 모순으로 잠금
    Locked,
}

#[derive(Debug, Clone, Serialize)]
pub struct Analysis {
    pub route: Route,
    pub trigger: SdsTrigger,
    pub signal: Option<SignalReport>,
    pub vote: Option<VoteResult>,
    pub field_checks: Vec<FieldCheck>,
    pub drift: Option<DriftFinding>,
    pub moc: Option<MocCheck>,
    /// 3중 진실 대조표
    pub truth: Vec<TruthRow>,
    pub substantive: Option<Substantive>,
    /// 신호 전파 체인
    pub chain: Vec<ChainEvent>,
    pub draft: Option<AopDraft>,
    /// 절차서에 딸린 선행 절차 인터록 (Part 8-4). 다 끝나야 확정할 수 있습니다.
    pub interlock: Option<InterlockState>,
    /// 경보 · 원인 센서의 60초 요약
    pub metrics: Vec<TimeSeriesMetrics>,
    pub stages: Vec<StageRecord>,
    pub sds: SdsSnapshot,
    /// 모델 · 가중치 상주 기록 (단계 상세의 GPU 타임라인)
    pub model_trace: Vec<Value>,
    pub candidate: Verdict,
    pub next: Next,
    /// 교대감독에게 보이는 사유
    pub reason: String,
    /// 확인할 일
    pub notes: Vec<String>,
}

impl Analysis {
    /// 결재에 고위험 조치가 들었는가
    pub fn high_risk(&self) -> bool {
        self.draft.as_ref().is_some_and(|d| d.high_risk)
    }

    pub fn causal_root(&self) -> Option<&crate::causal::RootCause> {
        self.substantive.as_ref()?.causal.as_ref()?.root.as_ref()
    }
}

fn unit_of(db: &Lattice, tag: &str) -> String {
    db.sensor(tag).map(|s| s.unit.clone()).unwrap_or_default()
}

/// 사건을 분석합니다. `sds_id` 는 스냅샷 번호입니다.
pub fn analyze(db: &Lattice, incident: &Incident, brains: Brains<'_>, sds_id: &str) -> Result<Analysis> {
    let now = incident.now_ms();
    let mut codes: Vec<String> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut stages: Vec<StageRecord> = Vec::new();
    let rules = Instant::now();

    // Gate 0: 라우팅 · 핀포인팅
    let route = gate0_router::route(db, incident)?;
    codes.push(route.verdict.code().to_string());

    // SDS 트리거
    let trigger = match &incident.alarm {
        Some(a) => {
            let at = a.at.timestamp_millis();
            let found = db.sensor(&a.tag).zip(incident.telemetry.get(&a.tag)).and_then(|(s, series)| trigger_at(s, series, at));
            let kind = if a.kind == AlarmKind::PreAlarm { TriggerKind::HighSlopePreAlarm } else { TriggerKind::ThresholdBreach };
            SdsTrigger { kind, tag: Some(a.tag.clone()), at_ms: at, value: Some(a.value), slope: found.as_ref().and_then(|f| f.slope), ttf_secs: found.and_then(|f| f.ttf_secs) }
        }
        None => SdsTrigger {
            kind: TriggerKind::FieldScanInjected,
            tag: route.tag.clone(),
            at_ms: incident.reports.first().map(|r| r.taken_at.timestamp_millis()).unwrap_or(now),
            value: incident.reports.first().and_then(|r| r.reading),
            slope: None,
            ttf_secs: None,
        },
    };

    // Stage 1 · 2: 현장 스캔 판독 (보고가 들고 온 시간)
    let read: Vec<_> = incident.reports.iter().filter_map(|r| r.timings).collect();
    let corrections: usize = incident.reports.iter().map(|r| r.corrections.len()).sum();
    if read.is_empty() {
        let why = if incident.reports.is_empty() { "스캔 없음 (신호 알람은 문서 모델을 띄우지 않음)" } else { "현장 수치 직접 입력" };
        stages.push(record(1, 0.0, StageStatus::Skipped, why));
        stages.push(record(2, 0.0, StageStatus::Skipped, why));
    } else {
        let crops: usize = read.iter().map(|t| t.crops).sum();
        let dropped: usize = read.iter().map(|t| t.dropped).sum();
        stages.push(record(1, read.iter().map(|t| t.layout_ms + t.siglip2_ms).sum(), StageStatus::Done, format!("조각 {crops}개 판독 · 안내문 {dropped}개 버림")));
        let summary = if corrections > 0 { format!("대장 대조로 오독 {corrections}건 교정") } else { "판독 완료 · 교정 없음".to_string() };
        stages.push(record(2, read.iter().map(|t| t.ocr_ms).sum(), if corrections > 0 { StageStatus::Corrected } else { StageStatus::Done }, summary));
    }

    // Gate 1: 현장 문서 형식 · 신호 무결성
    for report in &incident.reports {
        match gate1_formal::field_report(report) {
            FieldFormal::Pass => codes.push(Verdict::G1SignValid.code().to_string()),
            FieldFormal::SignatureRetry { reason } => {
                codes.push(Verdict::G1SignInvalidRetry.code().to_string());
                stages.push(record(3, rules.elapsed().as_secs_f64() * 1e3, StageStatus::Blocked, reason.clone()));
                return Ok(placeholder(route, trigger, stages, codes, sds_id, Next::FieldRetry, reason, Verdict::G1SignInvalidRetry));
            }
            FieldFormal::PermitHold { reason } => {
                codes.push(Verdict::G1PermitMissingHold.code().to_string());
                notes.push(reason);
            }
        }
    }
    let mut telemetry = incident.telemetry.clone();
    let signal = route.tag.as_deref().filter(|t| telemetry.get(t).is_some()).map(|t| gate1_formal::signal_health(db, &telemetry, t, now));
    if let Some(s) = &signal {
        codes.push(s.state.verdict().code().to_string());
        if let SignalState::SpikeFiltered { spikes } = &s.state {
            if let Some(series) = telemetry.get(&s.tag) {
                let clean = series.despiked(spikes);
                telemetry = telemetry.with_series(&s.tag, clean);
            }
        }
    }
    let flatline = signal.as_ref().is_some_and(|s| matches!(s.state, SignalState::Flatline { .. }));

    // Gate 2: 보팅 · 현장 실측 · MOC
    let vote = route.tag.as_deref().and_then(|t| gate2_consistency::vote(db, &telemetry, t, now));
    if let Some(v) = &vote {
        codes.push(v.state_code.clone());
    }
    let field_checks: Vec<FieldCheck> = incident.reports.iter().filter_map(|r| gate2_consistency::field_vs_dcs(db, &telemetry, r)).collect();
    let drift = vote.as_ref().map(|v| gate2_consistency::drift(v, field_checks.first()));
    let moc = incident.reports.iter().find(|r| r.kind == FieldKind::Redline).map(|r| gate2_consistency::check_moc(db, r));
    if let Some(m) = &moc {
        codes.push(m.outcome.verdict().code().to_string());
        notes.extend(m.stale_drawing.clone());
    }
    let mut truth = truth_rows(db, incident, &route, vote.as_ref(), &field_checks);
    if let Some(m) = &moc {
        truth.extend(m.rows.iter().cloned());
    }
    for f in field_checks.iter().filter(|f| !f.matches_dcs && vote.is_none()) {
        let unit = unit_of(db, &f.tag);
        notes.push(format!("현장 실측 {} {unit} 가 같은 시각 DCS 값 {} {unit} 와 다릅니다", fmt_value(f.field_value), f.dcs_value.map(fmt_value).unwrap_or_else(|| "없음".into())));
    }
    if let Some(excuse) = incident.reports.iter().find_map(|r| r.excuse.as_ref()) {
        notes.push(format!("현장 소명: {}", excuse.label()));
    }
    let rules_ms = rules.elapsed().as_secs_f64() * 1e3;

    // Gate 3: 판정 + Zoro-1 (모델 사용 기록을 함께 모읍니다)
    let model_started = Instant::now();
    let tag = route.tag.clone().unwrap_or_default();
    let case = Judging { db, telemetry: &telemetry, tag: &tag, now, signal: signal.as_ref(), vote: vote.as_ref(), field: field_checks.first(), drift: drift.as_ref(), moc: moc.as_ref(), flatline };
    let (judged, model_trace) = trace::capture(|| judge(&case, brains.model));
    let (substantive, next, reason) = judged?;
    let model_ms = model_started.elapsed().as_secs_f64() * 1e3;
    let candidate = substantive.decided.judgement.verdict;
    codes.push(candidate.code().to_string());
    if let Some(c) = substantive.causal.as_ref() {
        codes.push(if c.root.is_some() { "G3_CAUSAL_LAG_MATCH".into() } else { "G3_NO_UPSTREAM_CAUSE".into() });
    }
    let confidence = substantive.decided.judgement.confidence;
    let gate_status = match next {
        Next::Locked => StageStatus::Blocked,
        Next::AwaitingField => StageStatus::Waiting,
        _ if candidate.class() == OutcomeClass::Adverse || candidate == Verdict::G2SensorDriftIsolated || flatline => StageStatus::Alert,
        _ => StageStatus::Done,
    };
    let rule_summary = match (&vote, &moc, drift.as_ref()) {
        (_, Some(m), _) => match &m.outcome {
            MocOutcome::Verified => "MOC 원장 일치".to_string(),
            MocOutcome::Contradiction { .. } => "MOC 원장 불일치 · 잠금".to_string(),
            MocOutcome::Hold { .. } => "MOC 승인 · 권한 확인 필요".to_string(),
        },
        (Some(v), _, Some(DriftFinding::Isolated { channel, .. })) => format!("{} 채널 {channel} 편차{}", v.logic, if field_checks.iter().any(|f| f.matches_dcs) { " · 현장 일치" } else { "" }),
        (Some(v), _, _) => format!("{} 보팅 일치", v.logic),
        _ if flatline => "값 고정 (동결) 검출".to_string(),
        _ if signal.as_ref().is_some_and(|s| matches!(s.state, SignalState::SpikeFiltered { .. })) => "순간 잡음 걸러 냄".to_string(),
        _ => "신호 정상 · 원장 대조 완료".to_string(),
    };
    stages.push(record(3, rules_ms, if next == Next::Locked { StageStatus::Blocked } else if notes.is_empty() { StageStatus::Done } else { StageStatus::Alert }, rule_summary));
    let model_summary = match (confidence, brains.model.is_some()) {
        (_, false) => format!("{} (규칙만, 모델 없음)", candidate.label()),
        (Some(p), true) => format!("{} 확신 {p:.2}", candidate.label()),
        (None, true) => format!("{} (모델 이견 · 확신도 없음)", candidate.label()),
    };
    stages.push(record(4, model_ms, gate_status, model_summary));

    // Gate 4: 조치 절차서 초안
    let causal = substantive.causal.as_ref();
    let draft = select_aop(db, candidate, route.tag.as_deref(), causal).map(|aop| {
        let now_value = route.tag.as_deref().and_then(|t| telemetry.value_at(t, now));
        let root_value = causal.and_then(|c| c.root.as_ref()).and_then(|r| telemetry.value_at(&r.tag, now));
        let template = template_summary(db, candidate, route.tag.as_deref(), causal, now_value, root_value);
        let facts: Vec<String> = substantive.facts.iter().filter(|(k, _)| k != "적용 규칙").map(|(k, v)| format!("{k}: {v}")).collect();
        let source: Vec<String> = facts.iter().cloned().chain(std::iter::once(template.clone())).collect();
        draft_aop(brains.writer, aop, &source, &template)
    });
    match &draft {
        Some(d) => {
            let status = if next == Next::AwaitingField { StageStatus::Waiting } else if d.high_risk { StageStatus::Waiting } else { StageStatus::Done };
            let summary = match (d.by_model, d.problems.is_empty()) {
                (true, _) => format!("{} 초안 · 숫자 대조 통과{}", d.aop_id, if d.high_risk { " · 공동결재 대기" } else { "" }),
                (false, true) => format!("{} 템플릿 (글쓰기 모델 없음)", d.aop_id),
                (false, false) => format!("{} 모델 초안 수치 불일치 {}건 → 템플릿", d.aop_id, d.problems.len()),
            };
            stages.push(record(5, d.ms, status, summary));
        }
        None => stages.push(record(5, 0.0, if next == Next::Locked { StageStatus::Blocked } else { StageStatus::Skipped }, if moc.is_some() { "절차서 없음 (변경 반영 심사)" } else { "절차서 없음" })),
    }

    // 시계열 요약 · 전파 체인
    let mut focus: Vec<String> = route.tag.iter().cloned().collect();
    if let Some(root) = causal.and_then(|c| c.root.as_ref()) {
        focus.push(root.tag.clone());
    }
    let metrics: Vec<TimeSeriesMetrics> = focus.iter().filter_map(|t| Some(metrics(db.sensor(t)?, telemetry.get(t)?, now))).collect();
    let start = telemetry.start_ms().unwrap_or(now) + BASELINE_SECS * 1000;
    let chain = event_chain(db, &telemetry, start.max(now - 60_000), now);

    // 선행 절차 인터록: 초안 절차서에 딸린 단계를 지금 계측 · 현장 보고로 먼저 봅니다.
    let interlock = draft.as_ref().and_then(|d| interlock::evaluate(db, &d.aop_id, &telemetry, now, &incident.reports));

    let sds = SdsSnapshot {
        sds_id: sds_id.to_string(),
        trigger_kind: trigger.kind.code().to_string(),
        gate_state_vector: codes,
        decomposed_confidences: substantive.decided.axes.clone(),
        min_confidence: confidence,
        center_ms: causal.and_then(|c| c.alarm_departure_ms).map(|t| t.round() as i64).unwrap_or(trigger.at_ms),
    };
    Ok(Analysis {
        route,
        trigger,
        signal,
        vote,
        field_checks,
        drift,
        moc,
        truth,
        substantive: Some(substantive),
        chain,
        draft,
        interlock,
        metrics,
        stages,
        sds,
        model_trace,
        candidate,
        next,
        reason,
        notes,
    })
}

/// Gate 3 에 넘기는 것
struct Judging<'a> {
    db: &'a Lattice,
    telemetry: &'a crate::timeseries::Telemetry,
    tag: &'a str,
    now: i64,
    signal: Option<&'a SignalReport>,
    vote: Option<&'a VoteResult>,
    field: Option<&'a FieldCheck>,
    drift: Option<&'a DriftFinding>,
    moc: Option<&'a MocCheck>,
    flatline: bool,
}

/// 판정 후보와 다음 갈 곳. MOC → 동결 → 채널 편차 → 계통 원인 순서로 봅니다.
fn judge(c: &Judging<'_>, model: Option<&dyn DecisionModel>) -> Result<(Substantive, Next, String)> {
    let db = c.db;
    if let Some(m) = c.moc {
        let s = gate3_substantive::evaluate_moc(db, m, model)?;
        return Ok(match &m.outcome {
            MocOutcome::Verified => (s, Next::Ready, "Red-line 변경이 승인 원장과 일치합니다".to_string()),
            MocOutcome::Contradiction { reason } => (s, Next::Locked, reason.clone()),
            MocOutcome::Hold { reason } => (s, Next::Ready, reason.clone()),
        });
    }
    let no_signal = SignalReport { tag: c.tag.to_string(), state: SignalState::NoData, channels: Vec::new() };
    let signal = c.signal.unwrap_or(&no_signal);
    if c.flatline {
        let s = gate3_substantive::evaluate_instrument(db, c.tag, Verdict::G1SensorFlatline, signal, c.vote, c.field, None, model)?;
        return Ok((s, Next::Ready, "계측값이 30초 넘게 고정되어 계측기 동결로 봅니다".to_string()));
    }
    match c.drift {
        Some(d @ DriftFinding::Isolated { channel, field_confirmed }) => {
            let s = gate3_substantive::evaluate_instrument(db, c.tag, Verdict::G2SensorDriftIsolated, signal, c.vote, c.field, Some(d), model)?;
            return Ok(if *field_confirmed {
                (s, Next::Ready, format!("채널 {channel} 단독 드리프트를 현장 게이지로 확인했습니다 (트립 보류)"))
            } else {
                (s, Next::AwaitingField, format!("채널 {channel} 만 벗어났습니다. 현장 게이지 실측을 기다립니다"))
            });
        }
        Some(d @ DriftFinding::FieldSidesWithOutlier { channel }) => {
            let s = gate3_substantive::evaluate_instrument(db, c.tag, Verdict::G2FieldRecheckRequired, signal, c.vote, c.field, Some(d), model)?;
            return Ok((s, Next::AwaitingField, format!("현장 게이지가 벗어난 채널 {channel} 과 같습니다. 실제 변화일 수 있어 재점검이 필요합니다")));
        }
        _ => {}
    }
    let s = gate3_substantive::evaluate_alarm(db, c.telemetry, c.tag, c.now, signal, c.vote, model)?;
    let reason = match s.decided.judgement.verdict {
        Verdict::G3AdverseUpstreamCause => {
            let root = s.causal.as_ref().and_then(|x| x.root.as_ref());
            format!("근본 원인 후보: {}", root.and_then(|r| db.address(&r.kks)).map(|a| format!("{} {} {}", a.building, a.floor_text(), a.name)).unwrap_or_default())
        }
        Verdict::G3ViolationHighRisk => "상류 이상 없이 경보 설비만 변했습니다 (단위 설비 고장, 고위험)".to_string(),
        _ => "걸러 낸 계측이 정상 범위입니다".to_string(),
    };
    Ok((s, Next::Ready, reason))
}

/// 서명 재요청처럼 판정 전에 멈춘 분석
#[allow(clippy::too_many_arguments)]
fn placeholder(route: Route, trigger: SdsTrigger, mut stages: Vec<StageRecord>, codes: Vec<String>, sds_id: &str, next: Next, reason: String, candidate: Verdict) -> Analysis {
    for n in (stages.len() as u8 + 1)..=5 {
        stages.push(record(n, 0.0, StageStatus::Skipped, "앞 단계에서 멈춤"));
    }
    let sds = SdsSnapshot { sds_id: sds_id.to_string(), trigger_kind: trigger.kind.code().to_string(), gate_state_vector: codes, decomposed_confidences: Vec::new(), min_confidence: None, center_ms: trigger.at_ms };
    Analysis {
        route,
        trigger,
        signal: None,
        vote: None,
        field_checks: Vec::new(),
        drift: None,
        moc: None,
        truth: Vec::new(),
        substantive: None,
        chain: Vec::new(),
        draft: None,
        interlock: None,
        metrics: Vec::new(),
        stages,
        sds,
        model_trace: Vec::new(),
        candidate,
        next,
        reason,
        notes: Vec::new(),
    }
}

/// 3중 진실 대조표: ① 현장 스캔 ② 원장(보팅) ③ 단일 알람
fn truth_rows(db: &Lattice, incident: &Incident, route: &Route, vote: Option<&VoteResult>, fields: &[FieldCheck]) -> Vec<TruthRow> {
    let mut rows = Vec::new();
    let field_kks = incident.reports.iter().find(|r| r.kind != FieldKind::Redline).and_then(|r| r.kks.clone());
    if incident.alarm.is_some() || field_kks.is_some() {
        rows.push(TruthRow {
            item: "설비 KKS 코드".into(),
            field: field_kks.clone(),
            ledger: Some(route.kks.clone()),
            single: incident.alarm.as_ref().and_then(|a| db.sensor(&a.tag)).map(|s| s.kks.clone()),
            agree: field_kks.as_deref().is_none_or(|k| k == route.kks),
        });
    }
    let Some(tag) = route.tag.as_deref() else { return rows };
    let Some(sensor) = db.sensor(tag) else { return rows };
    let unit = &sensor.unit;
    let field = fields.iter().find(|f| f.tag == tag);
    // 단일 채널 값은 운전원이 보는 그 채널의 표시값입니다 (현장 대조가 있으면 촬영 시각, 없으면 분석 시각).
    let at = field.map(|f| f.taken_at_ms).unwrap_or(incident.now_ms());
    let single = incident.alarm.as_ref().filter(|a| a.tag == tag).map(|a| match &a.channel {
        Some(c) => {
            let shown = incident.telemetry.channels.get(tag).and_then(|chs| chs.get(c)).and_then(|s| s.value_at(at)).unwrap_or(a.value);
            format!("{} {unit} ({c}만)", fmt_value(shown))
        }
        None => format!("{} {unit}", fmt_value(a.value)),
    });
    let ledger = match (vote, field) {
        (Some(v), Some(f)) => f.dcs_value.map(|d| format!("{} {unit} ({} · 스캔 시각)", fmt_value(d), v.logic)),
        (Some(v), None) => Some(format!("{} {unit} ({})", fmt_value(v.voted), v.logic)),
        (None, Some(f)) => f.dcs_value.map(|d| format!("{} {unit} (스캔 시각)", fmt_value(d))),
        (None, None) => incident.telemetry.value_at(tag, incident.now_ms()).map(|d| format!("{} {unit}", fmt_value(d))),
    };
    rows.push(TruthRow {
        item: sensor.label.clone(),
        field: field.map(|f| format!("{} {unit}", fmt_value(f.field_value))),
        ledger,
        single,
        agree: field.is_none_or(|f| f.matches_dcs) && vote.is_none_or(|v| v.outliers.is_empty()),
    });
    rows
}
