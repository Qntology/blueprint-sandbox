//! 선행 절차 인터록 (기획 Part 8-4 3절, Part 8-6 절차 이슈 B-3).
//!
//! 절차서(AOP)에 딸린 단계를 깊이 순서(상류 압력 확인 → 우회 개방 → 현장 잠금 서명)대로 끝내야 그 절차서를 확정할 수 있습니다.
//! - 계측 조건(`이하` · `이상`)은 사건을 열 때 DCS 값으로 먼저 봅니다. 맞지 않으면 대기로 두고, 교대감독이 현장 값을 넣거나 스캔으로 채웁니다.
//! - 잠금표지(LOTO) 단계는 현장 단말이 보낸 보고(안전작업허가 확인 + 유효한 서명)나, 교대감독의 무전 확인(사번 날인)으로 채웁니다.
//! - 앞 단계가 끝나지 않았는데 뒤 단계를 채우거나 확정하려 하면 막습니다 (`G3_INTERCEPT_INTERLOCK_ORDER`).
//!
//! 단계마다 대상 설비에서 본 깊이(`req_depth`)가 있고, 원장을 읽을 때 계통 그래프의 깊이와 맞는지 검사합니다 ([`crate::graph::depth_of`]).

use serde::Serialize;

use crate::case::FieldReport;
use crate::lattice::{InterlockCheck, Lattice};
use crate::plant::fmt_value;
use crate::state_machine::gate1_formal::signature_valid;
use crate::timeseries::Telemetry;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StepStatus {
    /// 확인됨
    Passed,
    /// 아직 (조건이 맞지 않거나 증빙이 없음)
    Pending,
}

/// 무엇으로 확인했는가
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// DCS 계측값 (자동)
    Dcs,
    /// 현장 단말 보고 (스캔 · 서명)
    Field,
    /// 교대감독 입력 (현장 무전 · 휴대 계측)
    Supervisor,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StepState {
    pub no: u32,
    pub req_depth: i32,
    pub kks: String,
    pub text: String,
    pub check: InterlockCheck,
    pub status: StepStatus,
    /// 마지막으로 본 값 (계측 조건)
    pub observed: Option<f64>,
    pub unit: Option<String>,
    pub source: Option<Source>,
    /// 화면 · 감사 기록에 남기는 확인 내용
    pub evidence: Option<String>,
}

impl StepState {
    pub fn passed(&self) -> bool {
        self.status == StepStatus::Passed
    }

    /// 조건이 무엇인지 (화면 표시용)
    pub fn requirement(&self) -> String {
        let unit = self.unit.as_deref().unwrap_or("");
        match &self.check {
            InterlockCheck::Below { value, .. } => format!("{} {unit} 이하", fmt_value(*value)),
            InterlockCheck::AtLeast { value, .. } => format!("{} {unit} 이상", fmt_value(*value)),
            InterlockCheck::Loto => "LOTO 서명".to_string(),
        }
    }

    fn holds(&self, v: f64) -> bool {
        match &self.check {
            InterlockCheck::Below { value, .. } => v <= *value,
            InterlockCheck::AtLeast { value, .. } => v >= *value,
            InterlockCheck::Loto => false,
        }
    }
}

/// 사건 하나의 인터록 진행 상태
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct InterlockState {
    pub aop_id: String,
    pub target: String,
    pub fault: String,
    pub steps: Vec<StepState>,
}

impl InterlockState {
    pub fn complete(&self) -> bool {
        self.steps.iter().all(StepState::passed)
    }

    /// 순서상 다음에 끝내야 할 단계
    pub fn next_pending(&self) -> Option<&StepState> {
        self.steps.iter().find(|s| !s.passed())
    }

    pub fn missing(&self) -> Vec<&StepState> {
        self.steps.iter().filter(|s| !s.passed()).collect()
    }
}

/// 교대감독이 단계 하나를 채우는 증빙
#[derive(Debug, Clone, PartialEq)]
pub enum Evidence {
    /// 현장 무전 · 휴대 계측 · 스캔으로 받은 값
    Value { value: f64, how: String },
    /// 잠금표지(LOTO) 서명을 교대감독의 현장 무전 확인으로 갈음 (사번 날인)
    RadioLoto { supervisor: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InterlockError {
    UnknownStep(u32),
    /// 앞 단계가 끝나지 않았습니다
    OutOfOrder { step: u32, pending: u32 },
    /// 받은 값이 조건을 채우지 못합니다
    NotMet { step: u32, reason: String },
    /// 이 단계에 맞지 않는 증빙
    WrongEvidence(u32),
}

impl std::fmt::Display for InterlockError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InterlockError::UnknownStep(n) => write!(f, "인터록 단계 {n} 이 없습니다"),
            InterlockError::OutOfOrder { step, pending } => write!(f, "선행 절차 Step {pending} 을 끝내기 전에 Step {step} 을 확인할 수 없습니다"),
            InterlockError::NotMet { step, reason } => write!(f, "Step {step}: {reason}"),
            InterlockError::WrongEvidence(n) => write!(f, "Step {n} 에 맞지 않는 확인 방법입니다"),
        }
    }
}

/// 절차서에 인터록이 있으면 지금 계측 · 현장 보고로 볼 수 있는 단계를 먼저 확인합니다.
pub fn evaluate(db: &Lattice, aop_id: &str, telemetry: &Telemetry, now_ms: i64, reports: &[FieldReport]) -> Option<InterlockState> {
    let row = db.interlock_for(aop_id)?;
    let mut steps: Vec<StepState> = row
        .steps
        .iter()
        .map(|st| {
            let unit = match &st.check {
                InterlockCheck::Below { tag, .. } | InterlockCheck::AtLeast { tag, .. } => db.sensor(tag).map(|s| s.unit.clone()),
                InterlockCheck::Loto => None,
            };
            StepState { no: st.no, req_depth: st.req_depth, kks: st.kks.clone(), text: st.text.clone(), check: st.check.clone(), status: StepStatus::Pending, observed: None, unit, source: None, evidence: None }
        })
        .collect();
    for step in &mut steps {
        refresh(step, telemetry, now_ms, reports, &row.target);
    }
    Some(InterlockState { aop_id: row.aop_id.clone(), target: row.target.clone(), fault: row.fault.clone(), steps })
}

/// 계측 · 현장 보고가 새로 들어오면 아직 끝나지 않은 단계를 다시 봅니다. 이미 끝난 단계는 그대로 둡니다.
pub fn refresh_all(state: &mut InterlockState, telemetry: &Telemetry, now_ms: i64, reports: &[FieldReport]) {
    let target = state.target.clone();
    for step in state.steps.iter_mut().filter(|s| !s.passed()) {
        refresh(step, telemetry, now_ms, reports, &target);
    }
}

fn refresh(step: &mut StepState, telemetry: &Telemetry, now_ms: i64, reports: &[FieldReport], target: &str) {
    match step.check.clone() {
        InterlockCheck::Below { tag, .. } | InterlockCheck::AtLeast { tag, .. } => {
            let Some(v) = telemetry.value_at(&tag, now_ms) else { return };
            step.observed = Some(v);
            if step.holds(v) {
                step.status = StepStatus::Passed;
                step.source = Some(Source::Dcs);
                step.evidence = Some(format!("DCS {tag} {} {} (조건 {})", fmt_value(v), step.unit.as_deref().unwrap_or(""), step.requirement()));
            }
        }
        InterlockCheck::Loto => {
            // 대상 설비(또는 이 단계의 설비)에 대해 안전작업허가를 확인하고 유효하게 서명한 현장 보고
            let signed = reports.iter().find(|r| {
                r.permit_confirmed && r.signature.as_ref().is_some_and(signature_valid) && r.kks.as_deref().is_some_and(|k| k == step.kks || k == target)
            });
            if let Some(r) = signed {
                step.status = StepStatus::Passed;
                step.source = Some(Source::Field);
                step.evidence = Some(format!("현장 LOTO 점검표 서명 ({})", r.taken_at.format("%H:%M:%S")));
            }
        }
    }
}

/// 교대감독이 단계 하나를 채웁니다. 앞 단계가 끝나지 않았으면 막고(깊이 순서), 값이 조건을 못 채우면 받지 않습니다.
pub fn record(state: &mut InterlockState, no: u32, evidence: Evidence) -> Result<&StepState, InterlockError> {
    let idx = state.steps.iter().position(|s| s.no == no).ok_or(InterlockError::UnknownStep(no))?;
    if let Some(p) = state.steps[..idx].iter().find(|s| !s.passed()) {
        return Err(InterlockError::OutOfOrder { step: no, pending: p.no });
    }
    let step = &mut state.steps[idx];
    match (&step.check, evidence) {
        (InterlockCheck::Below { .. } | InterlockCheck::AtLeast { .. }, Evidence::Value { value, how }) => {
            step.observed = Some(value);
            if !step.holds(value) {
                return Err(InterlockError::NotMet { step: no, reason: format!("{} {} 은 조건({})을 채우지 못합니다", fmt_value(value), step.unit.as_deref().unwrap_or(""), step.requirement()) });
            }
            step.status = StepStatus::Passed;
            step.source = Some(Source::Supervisor);
            step.evidence = Some(format!("{how} {} {}", fmt_value(value), step.unit.as_deref().unwrap_or("")));
        }
        (InterlockCheck::Loto, Evidence::RadioLoto { supervisor }) => {
            step.status = StepStatus::Passed;
            step.source = Some(Source::Supervisor);
            step.evidence = Some(format!("교대감독 현장 무전 확인으로 갈음 (사번 날인 {supervisor})"));
        }
        _ => return Err(InterlockError::WrongEvidence(no)),
    }
    Ok(&state.steps[idx])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::case::{FieldKind, Signature};
    use crate::scenario::ScenarioSet;

    fn p2() -> (Lattice, Telemetry, i64) {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P2").unwrap();
        let t = s.telemetry(&set);
        let now = s.time(s.now_s).timestamp_millis();
        (db, t, now)
    }

    #[test]
    fn feedwater_interlock_checks_upstream_pressure_from_the_dcs() {
        let (db, t, now) = p2();
        let st = evaluate(&db, "AOP-103-FW02", &t, now, &[]).unwrap();
        assert_eq!(st.steps.len(), 3);
        // Step 1: 상류 펌프 토출압력 29.4 MPa ≤ 30.0 → 자동 통과
        assert!(st.steps[0].passed());
        assert_eq!(st.steps[0].source, Some(Source::Dcs));
        assert_eq!(st.steps[0].req_depth, -1);
        // Step 2: 바이패스는 닫혀 있음 (0 %) → 대기
        assert!(!st.steps[1].passed());
        assert!(st.steps[1].observed.unwrap() < 1.0);
        assert_eq!(st.next_pending().unwrap().no, 2);
        assert!(!st.complete());
        assert!(evaluate(&db, "AOP-102-TB01", &t, now, &[]).is_none());
    }

    #[test]
    fn steps_must_be_completed_in_depth_order() {
        let (db, t, now) = p2();
        let mut st = evaluate(&db, "AOP-103-FW02", &t, now, &[]).unwrap();
        // Step 2 를 건너뛰고 LOTO 부터 채우면 막습니다
        assert_eq!(record(&mut st, 3, Evidence::RadioLoto { supervisor: "OP-201".into() }), Err(InterlockError::OutOfOrder { step: 3, pending: 2 }));
        // 개도가 모자라면 받지 않습니다
        assert!(matches!(record(&mut st, 2, Evidence::Value { value: 30.0, how: "현장 무전".into() }), Err(InterlockError::NotMet { step: 2, .. })));
        assert!(record(&mut st, 2, Evidence::Value { value: 65.0, how: "현장 무전".into() }).is_ok());
        assert!(matches!(record(&mut st, 3, Evidence::Value { value: 1.0, how: "x".into() }), Err(InterlockError::WrongEvidence(3))));
        assert!(record(&mut st, 3, Evidence::RadioLoto { supervisor: "OP-201".into() }).is_ok());
        assert!(st.complete());
        assert!(st.steps[2].evidence.as_deref().unwrap().contains("OP-201"));
    }

    #[test]
    fn a_signed_field_report_with_permit_fills_the_loto_step() {
        let (db, t, now) = p2();
        let mut st = evaluate(&db, "AOP-103-FW02", &t, now, &[]).unwrap();
        record(&mut st, 2, Evidence::Value { value: 65.0, how: "현장 무전".into() }).unwrap();
        let mut r = FieldReport::new(FieldKind::Manual, chrono::Utc::now());
        r.kks = Some("10LAB20AA102".into());
        r.signature = Some(Signature::on_paper());
        // 허가 확인이 없으면 채우지 않습니다
        refresh_all(&mut st, &t, now, std::slice::from_ref(&r));
        assert!(!st.complete());
        r.permit_confirmed = true;
        refresh_all(&mut st, &t, now, &[r]);
        assert!(st.complete());
        assert_eq!(st.steps[2].source, Some(Source::Field));
    }

    #[test]
    fn ledger_depths_match_the_graph() {
        let db = Lattice::embedded().unwrap();
        let il = db.interlock_for("AOP-103-FW02").unwrap();
        assert_eq!(crate::graph::depth_of(&db, &il.target, "10LAB10AP001"), Some(-1));
        // 같은 펌프에서 갈라져 같은 수냉벽으로 가는 바이패스는 나란한 우회 (깊이 0)
        assert_eq!(crate::graph::depth_of(&db, &il.target, "10LAB20AA103"), Some(0));
        assert_eq!(crate::graph::depth_of(&db, &il.target, "10HAD10BB001"), Some(1));
        assert_eq!(crate::graph::depth_of(&db, &il.target, "10MAG10AC001"), None);
    }
}
