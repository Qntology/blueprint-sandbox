//! 판정 선택지(Verdict)와 판정(Judgement).
//!
//! 코드 이름은 기획안(Part 8 5절)의 Gate 코드를 따릅니다. 예: `G3_ADVERSE_UPSTREAM_CAUSE`.
//! 판정은 규칙이 정합니다. 규칙은 계측 · 원장 · 계통 그래프라는 증거에서 나옵니다.
//! Zoro-1 은 그 판정에 확신도를 주거나, 다른 선택지를 고르면 반대 의견으로 남습니다.
//! 자동화 정책은 확신도가 있을 때만 자동 확정을 허용합니다 (policy.rs).
//!
//! SDS 직교 분할 질의(`judge_by_axes`, Part 8-3 3.4절): 복합 징후를 시간축 · 물리축 · 센서축으로 쪼개 따로 묻고
//! 가장 낮은 확률을 확신도로 씁니다. 한 덩어리로 물으면 모델이 시간 선후를 놓칠 수 있기 때문입니다.

use anyhow::{bail, Result};
use serde::{Serialize, Serializer};

use crate::decision::{DecisionModel, DecisionTask, Fact};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Verdict {
    // Gate 0: 5대 섹터 라우팅
    Sec01Boiler,
    Sec02Turbine,
    Sec03Feedwater,
    Sec04Electric,
    Sec05EnvSafety,
    // Gate 1: 형식심사 (신호 건전성 · 서명 · 작업허가)
    G1SignalValid,
    G1SensorFlatline,
    G1SpikeFiltered,
    G1SignValid,
    G1SignInvalidRetry,
    G1PermitMissingHold,
    // Gate 2: 신호 · 원장 정합성
    G2VotingConsistent,
    G2SensorDriftIsolated,
    G2FieldRecheckRequired,
    G2MocVerified,
    G2RejectContradictionLock,
    // Gate 3: 마인드맵 원인 판정
    G3EligibleNormal,
    G3AdverseUpstreamCause,
    G3ViolationHighRisk,
    G3InterceptRubberStamp,
    /// 선행 절차(인터록) 단계를 건너뛰거나 끝내지 않은 채 확정하려 함 (Part 8-4)
    G3InterceptInterlockOrder,
    /// 설비를 멈추지 않는 우회 완화 조치(바이패스 개방)가 적격 (Part 8-5 Tier 1)
    G3EligibleAutoBypassMitigation,
    // Gate 4: 조치 절차서 문서
    G4AopDraftVerified,
    G4BlockInternalLeak,
    G4InterceptDocCorruption,
}

pub const ALL: [Verdict; 25] = [
    Verdict::Sec01Boiler,
    Verdict::Sec02Turbine,
    Verdict::Sec03Feedwater,
    Verdict::Sec04Electric,
    Verdict::Sec05EnvSafety,
    Verdict::G1SignalValid,
    Verdict::G1SensorFlatline,
    Verdict::G1SpikeFiltered,
    Verdict::G1SignValid,
    Verdict::G1SignInvalidRetry,
    Verdict::G1PermitMissingHold,
    Verdict::G2VotingConsistent,
    Verdict::G2SensorDriftIsolated,
    Verdict::G2FieldRecheckRequired,
    Verdict::G2MocVerified,
    Verdict::G2RejectContradictionLock,
    Verdict::G3EligibleNormal,
    Verdict::G3AdverseUpstreamCause,
    Verdict::G3ViolationHighRisk,
    Verdict::G3InterceptRubberStamp,
    Verdict::G3InterceptInterlockOrder,
    Verdict::G3EligibleAutoBypassMitigation,
    Verdict::G4AopDraftVerified,
    Verdict::G4BlockInternalLeak,
    Verdict::G4InterceptDocCorruption,
];

/// 화면 · 기록에는 코드(`G3_ADVERSE_UPSTREAM_CAUSE`)로 나갑니다.
impl Serialize for Verdict {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

/// 자동화 정책이 보는 판정의 성격
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum OutcomeClass {
    /// 정상 유지 · 승인된 변경 반영. 조건을 모두 충족할 때만 자동 확정 후보가 됩니다.
    Favorable,
    /// 계통 이상 · 고위험 · 위조 차단. 어떤 모드에서도 자동 확정하지 않습니다.
    Adverse,
    /// 계측 점검 · 현장 재확인 · 서명 재요청 등 절차. 교대감독이 확인합니다.
    Procedural,
}

impl Verdict {
    pub fn code(self) -> &'static str {
        match self {
            Verdict::Sec01Boiler => "SEC_01_BOILER",
            Verdict::Sec02Turbine => "SEC_02_TURBINE",
            Verdict::Sec03Feedwater => "SEC_03_FEEDWATER",
            Verdict::Sec04Electric => "SEC_04_ELECTRIC",
            Verdict::Sec05EnvSafety => "SEC_05_ENV_SAFETY",
            Verdict::G1SignalValid => "G1_SIGNAL_VALID",
            Verdict::G1SensorFlatline => "G1_SENSOR_FLATLINE",
            Verdict::G1SpikeFiltered => "G1_SPIKE_FILTERED",
            Verdict::G1SignValid => "G1_SIGN_VALID",
            Verdict::G1SignInvalidRetry => "G1_SIGN_INVALID_RETRY",
            Verdict::G1PermitMissingHold => "G1_PERMIT_MISSING_HOLD",
            Verdict::G2VotingConsistent => "G2_VOTING_CONSISTENT",
            Verdict::G2SensorDriftIsolated => "G2_SENSOR_DRIFT_ISOLATED",
            Verdict::G2FieldRecheckRequired => "G2_FIELD_RECHECK_REQUIRED",
            Verdict::G2MocVerified => "G2_MOC_VERIFIED",
            Verdict::G2RejectContradictionLock => "G2_REJECT_CONTRADICTION_LOCK",
            Verdict::G3EligibleNormal => "G3_ELIGIBLE_NORMAL",
            Verdict::G3AdverseUpstreamCause => "G3_ADVERSE_UPSTREAM_CAUSE",
            Verdict::G3ViolationHighRisk => "G3_VIOLATION_HIGH_RISK",
            Verdict::G3InterceptRubberStamp => "G3_INTERCEPT_RUBBER_STAMP",
            Verdict::G3InterceptInterlockOrder => "G3_INTERCEPT_INTERLOCK_ORDER",
            Verdict::G3EligibleAutoBypassMitigation => "G3_ELIGIBLE_AUTO_BYPASS_MITIGATION",
            Verdict::G4AopDraftVerified => "G4_AOP_DRAFT_VERIFIED",
            Verdict::G4BlockInternalLeak => "G4_BLOCK_INTERNAL_LEAK",
            Verdict::G4InterceptDocCorruption => "G4_INTERCEPT_DOC_CORRUPTION",
        }
    }

    pub fn from_code(code: &str) -> Option<Self> {
        ALL.iter().copied().find(|v| v.code() == code)
    }

    /// 주제어실 화면과 글쓰기 모델에 주는 우리말 이름. 모델에 코드를 그대로 주면 글에 코드가 나옵니다.
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Sec01Boiler => "101동 보일러동",
            Verdict::Sec02Turbine => "102동 터빈건물",
            Verdict::Sec03Feedwater => "103동 급수·보조펌프동",
            Verdict::Sec04Electric => "104동 전기·제어동",
            Verdict::Sec05EnvSafety => "105동 환경·통풍설비동",
            Verdict::G1SignalValid => "신호 정상",
            Verdict::G1SensorFlatline => "계측값 고정 (전송기 동결)",
            Verdict::G1SpikeFiltered => "순간 잡음 걸러 냄",
            Verdict::G1SignValid => "서명 유효",
            Verdict::G1SignInvalidRetry => "서명 다시 받음",
            Verdict::G1PermitMissingHold => "작업허가 확인 필요",
            Verdict::G2VotingConsistent => "다중 채널 일치",
            Verdict::G2SensorDriftIsolated => "단일 센서 편차 분리 (트립 보류)",
            Verdict::G2FieldRecheckRequired => "현장 실측 확인 필요",
            Verdict::G2MocVerified => "변경관리 승인 원장과 일치",
            Verdict::G2RejectContradictionLock => "승인 원장과 모순 (잠금)",
            Verdict::G3EligibleNormal => "정상 운전 범위",
            Verdict::G3AdverseUpstreamCause => "상류 설비 파급 원인",
            Verdict::G3ViolationHighRisk => "단위 설비 단독 고장 (고위험)",
            Verdict::G3InterceptRubberStamp => "근거 없는 고위험 승인 차단",
            Verdict::G3InterceptInterlockOrder => "선행 절차(인터록) 미완료 차단",
            Verdict::G3EligibleAutoBypassMitigation => "우회 밸브 완화 조치 적격",
            Verdict::G4AopDraftVerified => "조치 절차서 초안 검증",
            Verdict::G4BlockInternalLeak => "현장 단말 내부 정보 차단",
            Verdict::G4InterceptDocCorruption => "초안 수치 불일치 차단",
        }
    }

    pub fn class(self) -> OutcomeClass {
        match self {
            Verdict::G3EligibleNormal | Verdict::G2MocVerified | Verdict::G3EligibleAutoBypassMitigation => OutcomeClass::Favorable,
            Verdict::G3AdverseUpstreamCause | Verdict::G3ViolationHighRisk | Verdict::G2RejectContradictionLock => OutcomeClass::Adverse,
            _ => OutcomeClass::Procedural,
        }
    }
}

/// 규칙 판정과 (선택적) Zoro-1 의견
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Judgement {
    /// 규칙(증거)이 정한 선택지
    pub verdict: Verdict,
    /// Zoro-1 이 이 선택지에 준 확률. 기준 · 축별로 물었으면 그중 가장 낮은 확률입니다.
    /// 모델이 없거나, 모델이 다른 판단을 냈으면 None. None 이면 자동 확정은 할 수 없습니다.
    pub confidence: Option<f32>,
}

impl Judgement {
    pub fn by_rule(verdict: Verdict) -> Self {
        Self { verdict, confidence: None }
    }
}

/// 모델에 물은 질문 하나와 답 (Stage 4 상세 화면 · SDS 블랙박스에 남깁니다)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Asked {
    pub title: String,
    pub facts: Vec<(String, String)>,
    pub options: Vec<Verdict>,
    pub probabilities: Vec<f32>,
}

/// 판정 결과와 모델에 물은 기록
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Decided {
    pub judgement: Judgement,
    /// 축(기준) 이름과 규칙 쪽 확률
    pub axes: Vec<(String, f32)>,
    pub asked: Vec<Asked>,
}

impl Decided {
    fn rule_only(rule: Verdict, asked: Vec<Asked>) -> Self {
        Self { judgement: Judgement::by_rule(rule), axes: Vec::new(), asked }
    }
}

/// 모델의 답 `(가장 높은 선택지, 합이 1 인 확률)`. 모델이 쓸 수 없는 값을 내면 None 입니다.
fn ask(task: &DecisionTask, model: &dyn DecisionModel, asked: &mut Vec<Asked>) -> Result<Option<(usize, Vec<f32>)>> {
    let probs = model.probabilities(task)?;
    if probs.len() != task.options.len() {
        bail!("모델 확률 {}개가 선택지 {}개와 다릅니다", probs.len(), task.options.len());
    }
    let total: f32 = probs.iter().sum();
    if probs.is_empty() || total.is_nan() || total <= 0.0 || probs.iter().any(|p| !p.is_finite() || *p < 0.0) {
        return Ok(None);
    }
    let normalized: Vec<f32> = probs.iter().map(|p| p / total).collect();
    asked.push(Asked {
        title: task.title.clone(),
        facts: task.facts.iter().map(|f| (f.key.clone(), f.value.clone())).collect(),
        options: task.options.clone(),
        probabilities: normalized.clone(),
    });
    let pick = normalized.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i);
    Ok(pick.map(|i| (i, normalized)))
}

/// 규칙 판정에 모델 의견을 붙입니다. 모델이 `rule` 을 고르면 그 확률이 확신도가 되고, 다른 선택지를 고르면 None 입니다.
pub fn judge(rule: Verdict, task: &DecisionTask, model: Option<&dyn DecisionModel>) -> Result<Judgement> {
    Ok(decide(rule, task, model)?.judgement)
}

/// [`judge`] 와 같고, 물은 기록을 함께 돌려줍니다.
pub fn decide(rule: Verdict, task: &DecisionTask, model: Option<&dyn DecisionModel>) -> Result<Decided> {
    let mut asked = Vec::new();
    let Some(model) = model else { return Ok(Decided::rule_only(rule, asked)) };
    let Some((pick, probs)) = ask(task, model, &mut asked)? else { return Ok(Decided::rule_only(rule, asked)) };
    let confidence = (task.options[pick] == rule).then(|| probs[pick]);
    Ok(Decided { judgement: Judgement { verdict: rule, confidence }, axes: Vec::new(), asked })
}

/// 유리한 판정의 확신도를 기준별로 따로 물어 정합니다 (zoro-gravity 와 같은 방식).
///
/// 전체 질문에서 모델도 유리한 쪽을 골랐는지 보고, 기준마다 `(유리, 불리)` 두 선택지로 다시 물어 가장 낮은 확률을 확신도로 씁니다.
/// 어느 기준에서든 불리한 쪽이 앞서면 확신도는 None 입니다. 불리하거나 절차인 판정은 [`decide`] 와 같습니다.
pub fn judge_by_criteria(rule: Verdict, task: &DecisionTask, pair: (Verdict, Verdict), criteria: &[&str], model: Option<&dyn DecisionModel>) -> Result<Decided> {
    let mut asked = Vec::new();
    let Some(model) = model else { return Ok(Decided::rule_only(rule, asked)) };
    if rule.class() != OutcomeClass::Favorable {
        return decide(rule, task, Some(model));
    }
    let Some((pick, _)) = ask(task, model, &mut asked)? else { return Ok(Decided::rule_only(rule, asked)) };
    if task.options[pick].class() != OutcomeClass::Favorable {
        return Ok(Decided::rule_only(rule, asked));
    }
    let mut axes = Vec::new();
    for criterion in criteria {
        let question = DecisionTask { title: format!("{} 중 {criterion} 만 판단", task.title), facts: task.facts.clone(), options: vec![pair.0, pair.1] };
        let Some((pick, probs)) = ask(&question, model, &mut asked)? else { return Ok(Decided::rule_only(rule, asked)) };
        axes.push((criterion.to_string(), probs[0]));
        if pick != 0 {
            return Ok(Decided { judgement: Judgement::by_rule(rule), axes, asked });
        }
    }
    let lowest = axes.iter().map(|(_, p)| *p).fold(1.0f32, f32::min);
    Ok(Decided { judgement: Judgement { verdict: rule, confidence: Some(lowest) }, axes, asked })
}

/// SDS 직교 분할 질의의 축 하나: 이름과 그 축에서만 보여 줄 사실
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Axis {
    pub name: String,
    pub facts: Vec<Fact>,
}

/// SDS 직교 분할 질의. 전체 질문 뒤에 축마다 `(규칙, 반대)` 두 선택지로 그 축의 사실만 보여 주고 묻습니다.
///
/// 판정은 규칙이 정하고, 축 확률은 모두 기록합니다. 전체 질문이나 어느 축에서든 반대 쪽이 앞서면 확신도는 None 입니다.
/// 불리한 판정(원인 지목 · 고위험)도 축을 묻습니다. 자동 확정은 하지 않지만 교대감독이 볼 근거와 블랙박스 기록이 됩니다.
pub fn judge_by_axes(rule: Verdict, task: &DecisionTask, counter: Verdict, axes: &[Axis], model: Option<&dyn DecisionModel>) -> Result<Decided> {
    let mut asked = Vec::new();
    let Some(model) = model else { return Ok(Decided::rule_only(rule, asked)) };
    let Some((pick, _)) = ask(task, model, &mut asked)? else { return Ok(Decided::rule_only(rule, asked)) };
    let mut agree = task.options[pick] == rule;
    let mut probs = Vec::new();
    for axis in axes {
        let question = DecisionTask { title: format!("{} 중 {} 만 판단", task.title, axis.name), facts: axis.facts.clone(), options: vec![rule, counter] };
        let Some((pick, p)) = ask(&question, model, &mut asked)? else { return Ok(Decided::rule_only(rule, asked)) };
        probs.push((axis.name.clone(), p[0]));
        agree &= pick == 0;
    }
    let lowest = probs.iter().map(|(_, p)| *p).fold(1.0f32, f32::min);
    Ok(Decided { judgement: Judgement { verdict: rule, confidence: agree.then_some(lowest) }, axes: probs, asked })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 제목에 따라 정해 둔 규칙 쪽 확률을 답하는 대역. 질문을 세어 둡니다.
    struct Scripted {
        whole: Vec<f32>,
        by_title: Vec<(&'static str, f32)>,
        asked: std::sync::Mutex<Vec<String>>,
    }

    impl DecisionModel for Scripted {
        fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
            self.asked.lock().unwrap().push(task.title.clone());
            for (name, p) in &self.by_title {
                if task.title.contains(name) {
                    return Ok(vec![*p, 1.0 - *p]);
                }
            }
            Ok(self.whole.clone())
        }
    }

    fn scripted(whole: Vec<f32>, by_title: Vec<(&'static str, f32)>) -> Scripted {
        Scripted { whole, by_title, asked: std::sync::Mutex::new(Vec::new()) }
    }

    fn task() -> DecisionTask {
        DecisionTask {
            title: "발전소 계통 이상 근본 원인 판정".into(),
            facts: vec![Fact::new("상류 이상 설비", "103동 급수조절밸브 개도 18.0%")],
            options: vec![Verdict::G3AdverseUpstreamCause, Verdict::G3ViolationHighRisk, Verdict::G3EligibleNormal],
        }
    }

    fn axes() -> Vec<Axis> {
        vec![
            Axis { name: "시간 선후".into(), facts: vec![Fact::new("시차", "4.2초")] },
            Axis { name: "물리 방향".into(), facts: vec![Fact::new("방향", "유량 하락 → 온도 상승")] },
            Axis { name: "센서 건전성".into(), facts: vec![Fact::new("보팅", "정상")] },
        ]
    }

    #[test]
    fn codes_round_trip_and_have_korean_labels() {
        for v in ALL {
            assert_eq!(Verdict::from_code(v.code()), Some(v));
            assert!(!v.label().is_empty() && !v.label().contains('_'), "{:?}", v);
        }
        assert_eq!(Verdict::from_code("NOPE"), None);
    }

    #[test]
    fn verdicts_serialize_as_their_codes() {
        assert_eq!(serde_json::to_string(&Verdict::Sec01Boiler).unwrap(), "\"SEC_01_BOILER\"");
        assert_eq!(serde_json::to_value(Judgement::by_rule(Verdict::G3EligibleNormal)).unwrap()["verdict"], "G3_ELIGIBLE_NORMAL");
    }

    #[test]
    fn classes_match_the_policy_split() {
        assert_eq!(Verdict::G3EligibleNormal.class(), OutcomeClass::Favorable);
        assert_eq!(Verdict::G2MocVerified.class(), OutcomeClass::Favorable);
        assert_eq!(Verdict::G3AdverseUpstreamCause.class(), OutcomeClass::Adverse);
        assert_eq!(Verdict::G2RejectContradictionLock.class(), OutcomeClass::Adverse);
        assert_eq!(Verdict::G2SensorDriftIsolated.class(), OutcomeClass::Procedural);
        assert_eq!(Verdict::G1SensorFlatline.class(), OutcomeClass::Procedural);
    }

    #[test]
    fn the_lowest_axis_sets_the_confidence_and_every_axis_is_recorded() -> Result<()> {
        let m = scripted(vec![0.974, 0.019, 0.007], vec![("시간 선후", 0.988), ("물리 방향", 0.974), ("센서 건전성", 0.981)]);
        let d = judge_by_axes(Verdict::G3AdverseUpstreamCause, &task(), Verdict::G3ViolationHighRisk, &axes(), Some(&m))?;
        assert_eq!(d.judgement.verdict, Verdict::G3AdverseUpstreamCause);
        assert!((d.judgement.confidence.unwrap() - 0.974).abs() < 1e-6);
        assert_eq!(d.axes.len(), 3);
        assert_eq!(d.asked.len(), 4, "전체 질문 + 축 셋");
        assert_eq!(d.asked[1].facts, vec![("시차".to_string(), "4.2초".to_string())], "축 질문에는 그 축의 사실만 보입니다");
        Ok(())
    }

    #[test]
    fn an_axis_that_leans_the_other_way_removes_the_confidence_but_keeps_the_rule() -> Result<()> {
        let m = scripted(vec![0.9, 0.05, 0.05], vec![("시간 선후", 0.99), ("물리 방향", 0.3), ("센서 건전성", 0.99)]);
        let d = judge_by_axes(Verdict::G3AdverseUpstreamCause, &task(), Verdict::G3ViolationHighRisk, &axes(), Some(&m))?;
        assert_eq!(d.judgement, Judgement::by_rule(Verdict::G3AdverseUpstreamCause));
        assert_eq!(d.axes.len(), 3, "기울어진 축도 기록합니다");
        Ok(())
    }

    #[test]
    fn favorable_criteria_follow_zoro_gravity() -> Result<()> {
        let task = DecisionTask {
            title: "Red-line 변경 반영 심사".into(),
            facts: vec![Fact::new("원장", "일치")],
            options: vec![Verdict::G2MocVerified, Verdict::G2RejectContradictionLock],
        };
        let pair = (Verdict::G2MocVerified, Verdict::G2RejectContradictionLock);
        let m = scripted(vec![0.97, 0.03], vec![("원장 일치", 0.982), ("서명자 권한", 0.964)]);
        let d = judge_by_criteria(Verdict::G2MocVerified, &task, pair, &["원장 일치 여부", "서명자 권한 여부"], Some(&m))?;
        assert!((d.judgement.confidence.unwrap() - 0.964).abs() < 1e-6);
        let leaning = scripted(vec![0.2, 0.8], vec![]);
        let d = judge_by_criteria(Verdict::G2MocVerified, &task, pair, &["원장 일치 여부"], Some(&leaning))?;
        assert_eq!(d.judgement.confidence, None);
        assert_eq!(d.asked.len(), 1, "전체 질문이 기울면 기준을 묻지 않습니다");
        Ok(())
    }

    #[test]
    fn without_a_model_only_the_rule_remains() -> Result<()> {
        let d = judge_by_axes(Verdict::G3ViolationHighRisk, &task(), Verdict::G3AdverseUpstreamCause, &axes(), None)?;
        assert_eq!(d.judgement, Judgement::by_rule(Verdict::G3ViolationHighRisk));
        assert!(d.asked.is_empty());
        assert_eq!(judge(Verdict::G3EligibleNormal, &task(), None)?, Judgement::by_rule(Verdict::G3EligibleNormal));
        Ok(())
    }

    #[test]
    fn a_model_with_the_wrong_number_of_probabilities_is_an_error() {
        struct Wrong;
        impl DecisionModel for Wrong {
            fn probabilities(&self, _task: &DecisionTask) -> Result<Vec<f32>> {
                Ok(vec![1.0])
            }
        }
        assert!(judge(Verdict::G3EligibleNormal, &task(), Some(&Wrong)).is_err());
    }
}
