//! 자율 대응 (기획 Part 8-5 5절) · 복합 이슈의 다중 기준 분할 판단 (Part 8-6 3.2절).
//!
//! 대응을 세 등급으로 나눕니다. 정책(`policy.rs`)의 불변식은 그대로입니다: 유리한 처분만 자동, 고위험은 사람.
//! - Tier 1 (무중단 완화): 설비를 멈추지 않고 정상 운전을 되찾는 조치. 여기서는 고착된 조절밸브 옆의 **급수 바이패스 개방**.
//!   `G3_ELIGIBLE_AUTO_BYPASS_MITIGATION` (유리한 처분) 이고, 자동화 모드 ∧ 규칙 검사 통과 ∧ 네 기준 최솟값 ≥ 0.95 일 때만 자동 실행합니다.
//! - Tier 2 (절차 보완): 현장 재점검 · 서명 · MOC 확인. 사람이 확인합니다.
//! - Tier 3 (정지 · 감발): 터빈 · 보일러 정지, 출력 감발. 확신도가 0.99 여도 자동으로 하지 않고 공동 결재를 받습니다.
//!
//! 자동 실행의 명령은 물리엔진(시뮬레이터)에만 들어갑니다. 실제 DCS 로 내보내는 경로는 없습니다.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::decision::{DecisionModel, DecisionTask, Fact};
use crate::lattice::{InterlockCheck, Lattice};
use crate::plant::fmt_value;
use crate::state_machine::analysis::Analysis;
use crate::state_machine::gate1_formal::SignalState;
use crate::state_machine::gate2_consistency::MocOutcome;
use crate::verdict::{judge_by_criteria, Decided, Verdict};

/// 복합 · 다중 이슈를 자동 대응하기 전에 따로 묻는 네 기준 (기획 Part 8-6 `PLANT_MULTI_CRITERIA`)
pub const PLANT_MULTI_CRITERIA: [&str; 4] = [
    "단일 계측기 오류(드리프트·노이즈)가 아닌 실제 물리적 계통 변화 여부",
    "상류 설비(Depth -1~-2)와 하류 설비 간 시계열 전파 시차(Δt) 및 물리량 보존 부합 여부",
    "우회 밸브 개방 또는 예비기 투입을 위한 선행 안전 절차(인터록·LOTO) 충족 여부",
    "설비 변경관리(MOC) 승인 한도 및 운전 기준치 이내 복구 가능 여부",
];

/// 우회 개방 개도: 인터록 조건(이상)에 여유 15 %p, 최소 65 %
const BYPASS_MARGIN: f64 = 15.0;
const BYPASS_MIN: f64 = 65.0;

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Action {
    OpenBypass { kks: String, tag: String, open_pct: f64 },
}

impl Action {
    pub fn label(&self, db: &Lattice) -> String {
        match self {
            Action::OpenBypass { kks, open_pct, .. } => format!("{}({kks}) {}% 개방", db.equipment(kks).map(|e| e.name.as_str()).unwrap_or("바이패스 밸브"), fmt_value(*open_pct)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// 무중단 완화 (자동 가능)
    Tier1,
    /// 절차 보완 (사람 확인)
    Tier2,
    /// 정지 · 감발 (공동 결재)
    Tier3,
}

/// 완화 조치 하나의 제안 · 판단 · 실행 기록
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mitigation {
    pub action: Action,
    pub label: String,
    pub tier: Tier,
    pub verdict: Verdict,
    /// 규칙 검사 (모두 참이어야 자동 실행 후보)
    pub gates: Vec<(String, bool)>,
    pub facts: Vec<(String, String)>,
    /// 네 기준별 확률과 최솟값 (모델이 없거나 이견이면 확신도 없음)
    pub decided: Decided,
    /// 정책 결과: 자동 실행인가, 아니면 교대감독 확인 사유
    pub auto: bool,
    pub reason: String,
    /// 사후 샘플 감사 대상 (자동 실행 건의 10 %, 사건 번호 해시)
    pub post_audit: bool,
    pub executed_at: Option<DateTime<Utc>>,
    pub executed_by: Option<String>,
}

/// 분석 결과로 Tier 1 완화 조치를 세웁니다. 근본 원인이 절차서 인터록의 대상(깊이 0)이고 인터록에 나란한 우회 설비 단계가 있어야 합니다.
pub fn plan(db: &Lattice, a: &Analysis) -> Option<(Action, Vec<(String, bool)>, Vec<Fact>)> {
    let root = a.causal_root()?;
    let draft = a.draft.as_ref()?;
    let il = db.interlock_for(&draft.aop_id)?;
    if il.target != root.kks {
        return None;
    }
    let step = il.steps.iter().find(|s| s.kks != il.target && s.req_depth == 0 && matches!(s.check, InterlockCheck::AtLeast { .. }))?;
    let InterlockCheck::AtLeast { tag, value } = &step.check else { return None };
    let action = Action::OpenBypass { kks: step.kks.clone(), tag: tag.clone(), open_pct: (value + BYPASS_MARGIN).max(BYPASS_MIN) };

    let upstream_ok = a.interlock.as_ref().map(|s| s.steps.iter().filter(|x| x.req_depth < 0).all(|x| x.passed())).unwrap_or(false);
    let signal_ok = a.signal.as_ref().map_or(true, |s| matches!(s.state, SignalState::Valid));
    let moc_ok = !matches!(a.moc.as_ref().map(|m| &m.outcome), Some(MocOutcome::Contradiction { .. }));
    let gates = vec![
        (format!("근본 원인 {} 이 절차서 {} 인터록의 대상 (Depth 0)", root.kks, draft.aop_id), true),
        ("상류 Depth -1 선행 조건 충족 (인터록 Step 1)".to_string(), upstream_ok),
        ("경보 계측 건전성 (동결 · 순간 잡음 없음)".to_string(), signal_ok),
        (format!("변경관리({}) 원장 충돌 없음", db.terms().change), moc_ok),
    ];

    let mut facts = vec![Fact::new("적용 규칙", "설비를 멈추지 않는 우회 개방은 상류 선행 조건 · 계측 건전성 · 원장 충돌 없음이 모두 맞을 때만 자동 대응 후보")];
    if let Some(v) = a.substantive.as_ref().and_then(|s| s.facts.iter().find(|(k, _)| k == "상류 이상 설비")) {
        facts.push(Fact::new("근본 원인", v.1.clone()));
    }
    if let Some(v) = a.substantive.as_ref().and_then(|s| s.facts.iter().find(|(k, _)| k == "경보 계측값")) {
        facts.push(Fact::new("하류 경보", v.1.clone()));
    }
    facts.push(Fact::new("전파 시차", format!("{:.1}초 (배관 허용 {} ~ {}초)", root.lag_secs, fmt_value(root.lag_range.0), fmt_value(root.lag_range.1))));
    if let Some(il) = &a.interlock {
        for s in il.steps.iter().filter(|s| s.req_depth < 0) {
            facts.push(Fact::new(format!("선행 조건 Step {}", s.no), format!("{} · {}", s.text, if s.passed() { "충족" } else { "미충족" })));
        }
    }
    facts.push(Fact::new("완화 조치", format!("{} (Tier 1, 정지 · 감발 없음)", action.label(db))));
    facts.push(Fact::new("계측 건전성", if signal_ok { "동결 · 순간 잡음 없음" } else { "의심" }));
    facts.push(Fact::new("변경관리", if moc_ok { "원장 충돌 없음" } else { "원장과 모순" }));
    Some((action, gates, facts))
}

/// 완화 조치 심사의 제목 (발전소 `급수 바이패스`, 데이터센터 `CDU 우회밸브`)
pub fn review_title(db: &Lattice) -> String {
    format!("{} 자동 완화 조치 심사", db.terms().bypass)
}

/// 네 기준을 하나씩 따로 묻고 최솟값을 확신도로 씁니다 (Part 8-6, 방식 C).
pub fn judge(title: &str, facts: &[Fact], model: Option<&dyn DecisionModel>) -> anyhow::Result<Decided> {
    let task = DecisionTask {
        title: title.to_string(),
        facts: facts.to_vec(),
        options: vec![Verdict::G3EligibleAutoBypassMitigation, Verdict::G3AdverseUpstreamCause, Verdict::G3ViolationHighRisk],
    };
    judge_by_criteria(Verdict::G3EligibleAutoBypassMitigation, &task, (Verdict::G3EligibleAutoBypassMitigation, Verdict::G3AdverseUpstreamCause), &PLANT_MULTI_CRITERIA, model)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioSet;
    use crate::state_machine::analysis::{analyze, Brains};

    #[test]
    fn a_stuck_feedwater_valve_gets_a_bypass_mitigation_plan() {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P2").unwrap();
        let a = analyze(&db, &s.incident(&set, "T-P2"), Brains { model: None, writer: None }, "SDS-T").unwrap();
        let (action, gates, facts) = plan(&db, &a).expect("밸브 고착은 바이패스 완화 조치 후보");
        assert_eq!(action, Action::OpenBypass { kks: "10LAB20AA103".into(), tag: "10LAB20CG103".into(), open_pct: 65.0 });
        assert!(gates.iter().all(|g| g.1), "{gates:?}");
        assert!(facts.iter().any(|f| f.key == "전파 시차"));
        // 모델이 없으면 확신도가 없고, 정책은 자동 실행하지 않습니다
        assert_eq!(review_title(&db), "급수 바이패스 자동 완화 조치 심사");
        let d = judge(&review_title(&db), &facts, None).unwrap();
        assert!(d.judgement.confidence.is_none());
        // 원인이 다른 사건(베어링 단독 고장)에는 우회 조치가 없습니다
        let p4 = set.get("P4").unwrap();
        let a4 = analyze(&db, &p4.incident(&set, "T-P4"), Brains { model: None, writer: None }, "SDS-T").unwrap();
        assert!(plan(&db, &a4).is_none());
    }
}
