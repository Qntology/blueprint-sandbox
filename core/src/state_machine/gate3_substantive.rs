//! Gate 3: 마인드맵 상 · 하류 역추적 + Zoro-1 판단 (기획 Part 8 5절 Stage 4, Part 8-3 3.4절).
//!
//! 판정은 규칙이 정합니다: 인과 시차 탐색(`causal`)이 상류 원인을 찾으면 상류 파급 원인, 상류 이상 없이 알람 설비만 변하면
//! 단위 설비 단독 고장(고위험), 잡음을 걸러 낸 뒤 변화가 없으면 정상 운전 범위입니다.
//! Zoro-1 은 정제된 사실(설비 코드 · 계측값 · 규칙 결과, 근무자 정보 없음)만 읽고 선택지 확률을 냅니다 (0-Generation 포인터).
//!
//! SDS 직교 분할: 원인 판정은 ① 시간 선후 ② 열역학 · 유체 방향 ③ 센서 건전성 세 축으로 따로 묻고 최솟값을 확신도로 씁니다.
//! 정상 유지 판정은 기준별 분할 질문의 최솟값이 0.95 이상일 때만 자동 확정 후보가 됩니다 (`policy`).
//! 모델 확률이 높다는 것만으로 판정이 바뀌지 않습니다.

use anyhow::Result;
use serde::Serialize;

use super::gate1_formal::{SignalReport, SignalState};
use super::gate2_consistency::{DriftFinding, FieldCheck, MocCheck, VoteResult};
use crate::causal::{trace_root_cause, CausalReport};
use crate::decision::{DecisionModel, DecisionTask, Fact};
use crate::lattice::{Lattice, SensorRow};
use crate::plant::{fmt_value, Direction};
use crate::timeseries::{ttf, Telemetry, SLOPE_SAMPLES};
use crate::verdict::{decide, judge_by_axes, judge_by_criteria, Axis, Decided, Verdict};

/// 원인 판정의 선택지 (기획안 Stage 4 의 세 막대)
pub const OPTIONS: [Verdict; 3] = [Verdict::G3AdverseUpstreamCause, Verdict::G3ViolationHighRisk, Verdict::G3EligibleNormal];
/// 정상 유지를 자동 확정하기 전에 따로 묻는 기준
pub const NORMAL_CRITERIA: [&str; 3] = ["순간 잡음 여부", "경보 한계 이내 여부", "상류 설비 정상 여부"];
/// MOC 반영을 자동 확정하기 전에 따로 묻는 기준
pub const MOC_CRITERIA: [&str; 3] = ["승인 원장 일치 여부", "서명자 권한 여부", "설계 한계 이내 여부"];
/// SDS 직교 분할 축 이름
pub const AXIS_TIME: &str = "시간 선후 및 전파 시차";
pub const AXIS_PHYSICS: &str = "열역학 · 유체 보존 방향성";
pub const AXIS_SENSOR: &str = "센서 건전성";

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Substantive {
    pub decided: Decided,
    pub causal: Option<CausalReport>,
    /// 모델에 준 사실 (화면 · 블랙박스용)
    pub facts: Vec<(String, String)>,
    /// 지금 추세로 트립까지 남은 초
    pub ttf_secs: Option<f64>,
}

fn place(db: &Lattice, kks: &str) -> String {
    match db.address(kks) {
        Some(a) => format!("{} {} {}", a.building, a.floor_text(), a.name),
        None => kks.to_string(),
    }
}

/// 경보 · 보호 정지 한계 (보호 정지 이름은 도메인 용어: 발전소 트립 · 데이터센터 셧다운)
fn limits(db: &Lattice, s: &SensorRow) -> String {
    let mut parts = Vec::new();
    if let Some(v) = s.alarm_high.or(s.alarm_low) {
        parts.push(format!("경보 한계 {} {}", fmt_value(v), s.unit));
    }
    if let Some(v) = s.trip_high.or(s.trip_low) {
        parts.push(format!("{} 한계 {} {}", db.terms().trip, fmt_value(v), s.unit));
    }
    parts.join(", ")
}

fn change(s: &SensorRow, now: f64) -> String {
    let pct = if s.normal.abs() > f64::EPSILON { (now - s.normal) / s.normal.abs() * 100.0 } else { 0.0 };
    format!("{} {} {} (정상 {} {}, {:+.1}%)", s.label, fmt_value(now), s.unit, fmt_value(s.normal), s.unit, pct)
}

fn sensor_health_text(db: &Lattice, signal: &SignalReport, vote: Option<&VoteResult>, field: Option<&FieldCheck>) -> String {
    let mut parts = vec![match &signal.state {
        SignalState::Valid => "동결 · 순간 잡음 없음".to_string(),
        SignalState::SpikeFiltered { spikes } => format!("순간 잡음 {}건을 걸러 냄", spikes.len()),
        SignalState::Flatline { secs, .. } => format!("{secs}초 동안 값 고정"),
        SignalState::NoData => "계측 없음".to_string(),
    }];
    if let Some(v) = vote {
        parts.push(if v.outliers.is_empty() { format!("{} 보팅 채널 일치", v.logic) } else { format!("{} 보팅에서 채널 {} 편차", v.logic, v.outliers.join(", ")) });
    }
    if let Some(f) = field {
        let dcs = &db.terms().telemetry;
        parts.push(if f.matches_dcs { format!("현장 게이지가 {dcs} 와 일치") } else { format!("현장 게이지가 {dcs} 와 다름") });
    }
    parts.join(", ")
}

/// DCS 알람의 원인 판정. `telemetry` 는 잡음을 걸러 낸 계측입니다.
pub fn evaluate_alarm(
    db: &Lattice,
    telemetry: &Telemetry,
    alarm_tag: &str,
    now_ms: i64,
    signal: &SignalReport,
    vote: Option<&VoteResult>,
    model: Option<&dyn DecisionModel>,
) -> Result<Substantive> {
    let Some(sensor) = db.sensor(alarm_tag) else { anyhow::bail!("원장에 없는 센서입니다: {alarm_tag}") };
    let causal = trace_root_cause(db, telemetry, alarm_tag, now_ms);
    let series = telemetry.get(alarm_tag);
    let now_value = series.and_then(|s| s.value_at(now_ms)).unwrap_or(sensor.normal);
    let slope = series.and_then(|s| s.slope_at(now_ms, SLOPE_SAMPLES)).unwrap_or(0.0);
    let ttf_secs = ttf(sensor, now_value, slope);
    let health = sensor_health_text(db, signal, vote, None);
    let trip = &db.terms().trip;

    let rule = if causal.root.is_some() {
        Verdict::G3AdverseUpstreamCause
    } else if causal.alarm_departure_ms.is_some() {
        Verdict::G3ViolationHighRisk
    } else {
        Verdict::G3EligibleNormal
    };
    let mut facts = vec![
        Fact::new("적용 규칙", "상류 설비의 변곡이 배관 지연 범위 안에서 하류 경보보다 앞서면 상류 파급 원인, 상류 이상 없이 경보 설비만 변하면 단위 설비 단독 고장"),
        Fact::new("경보 위치", place(db, &sensor.kks)),
        Fact::new("경보 계측값", format!("{} {} {} ({})", sensor.label, fmt_value(now_value), sensor.unit, limits(db, sensor))),
    ];
    if let Some(t) = ttf_secs {
        facts.push(Fact::new(format!("{trip} 도달 예상"), format!("{t:.0}초 (초당 {:+.2} {})", slope, sensor.unit)));
    }
    let mut axes = Vec::new();
    match &causal.root {
        Some(root) => {
            let up = db.sensor(&root.tag);
            let up_now = telemetry.value_at(&root.tag, now_ms);
            facts.push(Fact::new("상류 이상 설비", match (up, up_now) {
                (Some(s), Some(v)) => format!("{} {}", place(db, &root.kks), change(s, v)),
                _ => place(db, &root.kks),
            }));
            // 원인 설비를 지나는 흐름 센서 (급수 유량 등)
            if let Some(flow) = db.out_edges(&root.kks).into_iter().find_map(|e| e.flow_tag.clone()).filter(|t| *t != root.tag) {
                if let (Some(s), Some(v)) = (db.sensor(&flow), telemetry.value_at(&flow, now_ms)) {
                    facts.push(Fact::new("상류 흐름", change(s, v)));
                }
            }
            let route: Vec<String> = root.path.iter().map(|k| place(db, k)).collect();
            let jumps = if root.connectors.is_empty() { String::new() } else { format!(", 도면 점프 {}", root.connectors.join(" · ")) };
            facts.push(Fact::new("인과 경로", format!("{} ({}홉{jumps})", route.join(" → "), root.hops)));
            facts.push(Fact::new("전파 시차", format!("{:.1}초 (허용 {:.1} ~ {:.1}초)", root.lag_secs, root.lag_range.0, root.lag_range.1)));
            facts.push(Fact::new("센서 건전성", health.clone()));
            let dir = |d: Direction| d.arrow();
            axes.push(Axis {
                name: AXIS_TIME.into(),
                facts: vec![
                    Fact::new("상류 변곡", format!("{} {}", place(db, &root.kks), dir(root.direction))),
                    Fact::new("하류 변곡", format!("{} {}", place(db, &sensor.kks), causal.alarm_direction.map(dir).unwrap_or("변화 없음"))),
                    Fact::new("관측 시차", format!("상류가 {:.1}초 먼저 변함", root.lag_secs)),
                    Fact::new("배관 허용 시차", format!("{:.1} ~ {:.1}초", root.lag_range.0, root.lag_range.1)),
                ],
            });
            axes.push(Axis {
                name: AXIS_PHYSICS.into(),
                facts: vec![
                    Fact::new("상류 변화", match (up, up_now) { (Some(s), Some(v)) => change(s, v), _ => dir(root.direction).to_string() }),
                    Fact::new("하류 변화", change(sensor, now_value)),
                    Fact::new("방향 판정", match root.physics { Some(true) => "물리적으로 맞음", Some(false) => "물리적으로 맞지 않음", None => "판정 표 없음" }),
                ],
            });
        }
        None => {
            facts.push(Fact::new("상류 이상 설비", "없음 (상류 3홉 안 변곡 없음)"));
            facts.push(Fact::new("센서 건전성", health.clone()));
            axes.push(Axis {
                name: "상류 설비 건전성".into(),
                facts: vec![Fact::new("상류 3홉 변곡", "없음"), Fact::new("역류 여파", format!("{}건", causal.propagation.iter().filter(|p| p.backflow).count()))],
            });
            axes.push(Axis {
                name: "경보 계측값 추세".into(),
                facts: vec![
                    Fact::new("현재값", change(sensor, now_value)),
                    Fact::new("한계", limits(db, sensor)),
                    Fact::new("추세", ttf_secs.map(|t| format!("{trip}까지 {t:.0}초")).unwrap_or_else(|| format!("{trip} 방향 추세 없음"))),
                ],
            });
        }
    }
    axes.push(Axis { name: AXIS_SENSOR.into(), facts: vec![Fact::new("신호 상태", health)] });

    let task = DecisionTask { title: db.terms().decision_title(), facts: facts.clone(), options: OPTIONS.to_vec() };
    let decided = match rule {
        Verdict::G3EligibleNormal => judge_by_criteria(rule, &task, (Verdict::G3EligibleNormal, Verdict::G3ViolationHighRisk), &NORMAL_CRITERIA, model)?,
        Verdict::G3AdverseUpstreamCause => judge_by_axes(rule, &task, Verdict::G3ViolationHighRisk, &axes, model)?,
        _ => judge_by_axes(rule, &task, Verdict::G3AdverseUpstreamCause, &axes, model)?,
    };
    Ok(Substantive { decided, causal: Some(causal), facts: facts.into_iter().map(|f| (f.key, f.value)).collect(), ttf_secs })
}

/// 계측 편차 · 동결 판정 (P1 · P6). 규칙이 정한 절차 판정에 모델 의견만 붙입니다.
pub fn evaluate_instrument(
    db: &Lattice,
    tag: &str,
    rule: Verdict,
    signal: &SignalReport,
    vote: Option<&VoteResult>,
    field: Option<&FieldCheck>,
    finding: Option<&DriftFinding>,
    model: Option<&dyn DecisionModel>,
) -> Result<Substantive> {
    let sensor = db.sensor(tag);
    let mut facts = vec![
        Fact::new("적용 규칙", "다중 채널 중 하나만 벗어나고 현장 게이지가 나머지와 같으면 전송기 단독 이상, 값이 잡음 없이 고정되면 계측기 동결"),
        Fact::new("계측 위치", sensor.map(|s| format!("{} {}", place(db, &s.kks), s.label)).unwrap_or_else(|| tag.to_string())),
    ];
    if let Some(v) = vote {
        let unit = sensor.map(|s| s.unit.as_str()).unwrap_or("");
        let chs: Vec<String> = v.channels.iter().map(|(c, x)| format!("{c} {} {unit}", fmt_value(*x))).collect();
        facts.push(Fact::new("채널 값", chs.join(", ")));
        facts.push(Fact::new("보팅 대표값", format!("{} {unit}", fmt_value(v.voted))));
    }
    if let Some(f) = field {
        let unit = sensor.map(|s| s.unit.as_str()).unwrap_or("");
        facts.push(Fact::new("현장 게이지", format!("{} {unit} (같은 시각 {} 대표값 {} {unit})", fmt_value(f.field_value), db.terms().telemetry, f.dcs_value.map(fmt_value).unwrap_or_else(|| "없음".into()))));
    }
    if let Some(DriftFinding::Isolated { channel, field_confirmed }) = finding {
        facts.push(Fact::new("채널 판정", format!("채널 {channel} 단독 편차{}", if *field_confirmed { ", 현장 게이지가 나머지 채널과 일치" } else { ", 현장 확인 전" })));
    }
    facts.push(Fact::new("센서 건전성", sensor_health_text(db, signal, vote, field)));
    let task = DecisionTask { title: "계측 이상과 실제 공정 변화 판정".into(), facts: facts.clone(), options: vec![rule, Verdict::G3ViolationHighRisk] };
    let decided = decide(rule, &task, model)?;
    Ok(Substantive { decided, causal: None, facts: facts.into_iter().map(|f| (f.key, f.value)).collect(), ttf_secs: None })
}

/// Red-line 변경 반영 판정 (P3). 원장 대조가 통과한 변경만 유리한 판정이 됩니다.
pub fn evaluate_moc(db: &Lattice, check: &MocCheck, model: Option<&dyn DecisionModel>) -> Result<Substantive> {
    let rule = check.outcome.verdict();
    let mut facts = vec![Fact::new("적용 규칙", "Red-line 의 변경번호 · 설비 · 새 한계가 승인 원장과 같고, 서명자에게 관할 권한이 있고, 설계 한계 안이어야 반영")];
    if let Some(m) = &check.ledger {
        facts.push(Fact::new("변경 대상", format!("{} {}", place(db, &m.kks), db.sensor(&m.tag).map(|s| s.label.clone()).unwrap_or_default())));
        facts.push(Fact::new("원장 변경 내용", format!("{} {} → {} {} (설계 한계 {} {})", fmt_value(m.old), m.unit, fmt_value(m.new), m.unit, fmt_value(m.design_limit), m.unit)));
        facts.push(Fact::new("원장 상태", if m.approved() { "승인" } else { "승인 전" }));
    }
    for row in &check.rows {
        facts.push(Fact::new(format!("대조: {}", row.item), if row.agree { "일치" } else { "불일치" }));
    }
    let task = DecisionTask { title: "Red-line 변경 반영 심사".into(), facts: facts.clone(), options: vec![Verdict::G2MocVerified, Verdict::G2RejectContradictionLock] };
    let decided = judge_by_criteria(rule, &task, (Verdict::G2MocVerified, Verdict::G2RejectContradictionLock), &MOC_CRITERIA, model)?;
    Ok(Substantive { decided, causal: None, facts: facts.into_iter().map(|f| (f.key, f.value)).collect(), ttf_secs: None })
}

/// 고위험 조치를 승인할 때 내미는 근거 (기획안 Stage 5 의 결재 영역)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum Justification {
    /// 잘못 누름: 결재 전 상태로 되돌립니다.
    CancelMistake,
    /// 현장 증빙으로 오경보를 입증: 증빙 스캔이 1장 이상 있어야 하고, 특별 감사 표시가 붙습니다.
    ProveFalsePositive { evidence_scans: u32 },
    /// 절차서에 따른 고위험 조치: 교대감독 파트장 공동 결재와 근거 절차서 번호가 있어야 합니다.
    ApplyProcedure { team_lead_cosign: bool, procedure_ref: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApprovalOutcome {
    Reverted,
    Approved { special_audit_flag: bool },
}

/// 고위험 조치 승인 시도를 검사합니다. `aop_id` 는 초안의 절차서 번호이고, 근거 번호가 이와 같아야 합니다.
pub fn approve_high_risk(aop_id: Option<&str>, justification: Option<&Justification>) -> Result<ApprovalOutcome, Verdict> {
    match justification {
        None => Err(Verdict::G3InterceptRubberStamp),
        Some(Justification::CancelMistake) => Ok(ApprovalOutcome::Reverted),
        Some(Justification::ProveFalsePositive { evidence_scans }) if *evidence_scans > 0 => Ok(ApprovalOutcome::Approved { special_audit_flag: true }),
        Some(Justification::ProveFalsePositive { .. }) => Err(Verdict::G3InterceptRubberStamp),
        Some(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref }) if aop_id.is_some_and(|a| a == procedure_ref.trim()) => Ok(ApprovalOutcome::Approved { special_audit_flag: false }),
        Some(Justification::ApplyProcedure { .. }) => Err(Verdict::G3InterceptRubberStamp),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioSet;
    use crate::state_machine::gate1_formal::signal_health;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    /// 모델이 받은 질문을 기록하는 대역. 첫 선택지에 확률을 몰아 줍니다.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<DecisionTask>>);

    impl DecisionModel for Recorder {
        fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
            self.0.lock().unwrap().push(task.clone());
            let n = task.options.len();
            Ok((0..n).map(|i| if i == 0 { 0.97 } else { 0.03 / (n - 1) as f32 }).collect())
        }
    }

    #[test]
    fn p2_names_the_stuck_valve_and_asks_three_axes() -> Result<()> {
        let set = ScenarioSet::embedded().unwrap();
        let inc = set.get("P2").unwrap().incident(&set, "i");
        let signal = signal_health(&db(), &inc.telemetry, "10HAH10CT001", inc.now_ms());
        let rec = Recorder::default();
        let s = evaluate_alarm(&db(), &inc.telemetry, "10HAH10CT001", inc.now_ms(), &signal, None, Some(&rec))?;
        assert_eq!(s.decided.judgement.verdict, Verdict::G3AdverseUpstreamCause);
        assert!((s.decided.judgement.confidence.unwrap() - 0.97).abs() < 1e-5);
        let names: Vec<&str> = s.decided.axes.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, vec![AXIS_TIME, AXIS_PHYSICS, AXIS_SENSOR]);
        let root = s.causal.as_ref().unwrap().root.as_ref().unwrap();
        assert_eq!((root.kks.as_str(), root.hops), ("10LAB20AA102", 2));
        let facts = &s.facts;
        assert!(facts.iter().any(|(k, v)| k == "상류 흐름" && v.contains("1,1")), "{facts:?}");
        assert!(facts.iter().any(|(k, v)| k == "인과 경로" && v.contains("OPC-103-101")), "{facts:?}");
        Ok(())
    }

    #[test]
    fn p4_without_an_upstream_cause_is_a_high_risk_unit_fault() -> Result<()> {
        let set = ScenarioSet::embedded().unwrap();
        let inc = set.get("P4").unwrap().incident(&set, "i");
        let signal = signal_health(&db(), &inc.telemetry, "10MAD10CY001", inc.now_ms());
        let s = evaluate_alarm(&db(), &inc.telemetry, "10MAD10CY001", inc.now_ms(), &signal, None, None)?;
        assert_eq!(s.decided.judgement.verdict, Verdict::G3ViolationHighRisk);
        assert!(s.causal.unwrap().root.is_none());
        Ok(())
    }

    #[test]
    fn the_model_never_sees_worker_names_or_ids() -> Result<()> {
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P3").unwrap();
        let report = &s.field_reports(|_| Some("김태현".into()))[0];
        let check = crate::state_machine::gate2_consistency::check_moc(&db(), report);
        let rec = Recorder::default();
        evaluate_moc(&db(), &check, Some(&rec))?;
        let asked = format!("{:?}", rec.0.lock().unwrap());
        for secret in ["김태현", "E102"] {
            assert!(!asked.contains(secret), "모델에 보이면 안 되는 값: {secret}");
        }
        assert!(asked.contains("31.0"));
        Ok(())
    }

    #[test]
    fn rubber_stamping_a_high_risk_procedure_is_blocked() {
        assert_eq!(approve_high_risk(Some("AOP-103-FW02"), None), Err(Verdict::G3InterceptRubberStamp));
        assert_eq!(approve_high_risk(Some("AOP-103-FW02"), Some(&Justification::CancelMistake)), Ok(ApprovalOutcome::Reverted));
        let cosign = |ok: bool, r: &str| Justification::ApplyProcedure { team_lead_cosign: ok, procedure_ref: r.into() };
        assert_eq!(approve_high_risk(Some("AOP-103-FW02"), Some(&cosign(true, "AOP-103-FW02"))), Ok(ApprovalOutcome::Approved { special_audit_flag: false }));
        assert_eq!(approve_high_risk(Some("AOP-103-FW02"), Some(&cosign(false, "AOP-103-FW02"))), Err(Verdict::G3InterceptRubberStamp));
        assert_eq!(approve_high_risk(Some("AOP-103-FW02"), Some(&cosign(true, "AOP-102-TB01"))), Err(Verdict::G3InterceptRubberStamp), "다른 절차서 번호");
        assert_eq!(approve_high_risk(None, Some(&Justification::ProveFalsePositive { evidence_scans: 1 })), Ok(ApprovalOutcome::Approved { special_audit_flag: true }));
        assert_eq!(approve_high_risk(None, Some(&Justification::ProveFalsePositive { evidence_scans: 0 })), Err(Verdict::G3InterceptRubberStamp));
    }
}
