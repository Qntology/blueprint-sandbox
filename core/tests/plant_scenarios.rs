//! 기획안 P11 ~ P13 검증 시나리오의 엔드투엔드 시험 (합성 시드 원장 + 합성 시계열).
//!
//! 판단 · 글쓰기 모델 자리에는 정해 둔 답을 내는 대역을 꽂습니다. 실제 모델 점검은 `real_models.rs` (`--ignored`) 입니다.
//!
//! - P1 센서 드리프트: DCS 채널 A 14.2 MPa vs 보팅 · 현장 게이지 26.0 MPa → 트립 보류 (현장 확인 전에는 확정 불가)
//! - P2 상류 밸브 고착: 101동 과열 경보 → 도면 점프로 2홉 상류 103동 급수밸브(개도 18%) 지목 → 고위험 공동 결재
//! - P3 Red-line MOC: 승인 원장 일치 건 통과 · 오버레이 반영, 위조 수치(35.0 MPa) 잠금
//! - P4 베어링 고위험: 교대감독 공동 결재 강제, 근거 없는 승인 · 반려 차단
//! - P5 · P6 · P7: 순간 잡음 · 계측 동결 · 기울기 선행 경보 (Part 8-3)

use std::sync::Mutex;

use anyhow::Result;
use chrono::{DateTime, Utc};
use blueprint_core::audit::AuditCategory;
use blueprint_core::case::{FieldReport, Signature};
use blueprint_core::decision::{DecisionModel, DecisionTask};
use blueprint_core::drafting::TextWriter;
use blueprint_core::interlock::Evidence;
use blueprint_core::lattice::Lattice;
use blueprint_core::policy::AutomationPolicy;
use blueprint_core::scenario::{Scenario, ScenarioSet};
use blueprint_core::state_machine::gate3_substantive::Justification;
use blueprint_core::state_machine::{Engine, Outcome, SupervisorDecision};
use blueprint_core::stages::StageStatus;
use blueprint_core::verdict::Verdict;

/// 기대한 판정에 0.97 을 주는 대역. 질문을 모두 기록합니다.
struct Oracle {
    want: Verdict,
    p: f32,
    asked: Mutex<Vec<DecisionTask>>,
}

impl Oracle {
    fn new(want: Verdict) -> Self {
        Self { want, p: 0.97, asked: Mutex::new(Vec::new()) }
    }
}

impl DecisionModel for Oracle {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.asked.lock().unwrap().push(task.clone());
        let n = task.options.len();
        let pick = task.options.iter().position(|v| *v == self.want).unwrap_or(0);
        Ok((0..n).map(|i| if i == pick { self.p } else { (1.0 - self.p) / (n - 1) as f32 }).collect())
    }
}

/// 공유하는 대역 (엔진이 Box 로 가져가도 시험이 기록을 볼 수 있게)
struct Shared(std::sync::Arc<Oracle>);

impl DecisionModel for Shared {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.0.probabilities(task)
    }
}

/// 사실 줄을 그대로 옮겨 쓰는 글쓰기 대역 (숫자 대조를 통과합니다)
struct Copycat;

impl TextWriter for Copycat {
    fn complete(&self, _system: &str, user: &str, _max: usize) -> Result<String> {
        let line = user.lines().find(|l| l.starts_with("상류 이상 설비") || l.starts_with("경보 계측값") || l.starts_with("계측 위치")).unwrap_or("원인 진단");
        Ok(format!("{} 때문입니다.", line.split_once(": ").map(|(_, v)| v).unwrap_or(line)))
    }
}

/// 없는 숫자를 지어내는 글쓰기 대역
struct Liar;

impl TextWriter for Liar {
    fn complete(&self, _system: &str, _user: &str, _max: usize) -> Result<String> {
        Ok("급수조절밸브 개도가 15.0 %로 떨어져 과열기 온도가 700.0 °C 가 되었습니다.".into())
    }
}

struct Fixture {
    db: Lattice,
    set: ScenarioSet,
}

fn fixture() -> Fixture {
    Fixture { db: Lattice::embedded().unwrap(), set: ScenarioSet::embedded().unwrap() }
}

impl Fixture {
    fn scenario(&self, id: &str) -> &Scenario {
        self.set.get(id).unwrap()
    }

    fn reports(&self, id: &str) -> Vec<FieldReport> {
        self.scenario(id).field_reports(|w| self.db.person(w).map(|p| p.name.clone()))
    }

    fn now(&self, id: &str) -> DateTime<Utc> {
        let s = self.scenario(id);
        s.time(s.now_s)
    }

    /// 시나리오 사건을 돌립니다. 현장 보고만 있는 시나리오는 보고를 붙여서 엽니다.
    fn run(&self, engine: &mut Engine, id: &str) -> (String, Outcome) {
        let s = self.scenario(id);
        let mut incident = s.incident(&self.set, &format!("INC-{id}"));
        if incident.alarm.is_none() {
            incident.reports = self.reports(id);
        }
        let out = engine.run(&self.db, incident, self.now(id)).unwrap();
        (format!("INC-{id}"), out)
    }
}

fn engine(want: Option<Verdict>) -> (Engine, Option<std::sync::Arc<Oracle>>) {
    match want {
        Some(v) => {
            let oracle = std::sync::Arc::new(Oracle::new(v));
            (Engine::new(b"salt", AutomationPolicy::default(), Some(Box::new(Shared(oracle.clone())))).with_writer(Box::new(Copycat)), Some(oracle))
        }
        None => (Engine::new(b"salt", AutomationPolicy::default(), None), None),
    }
}

fn verdict_of(out: &Outcome) -> Option<Verdict> {
    match out {
        Outcome::AwaitingField { judgement, .. } | Outcome::SupervisorReview { judgement, .. } => judgement.as_ref().map(|j| j.verdict),
        Outcome::Blocked { verdict, .. } | Outcome::FieldRetry { verdict, .. } => Some(*verdict),
        _ => None,
    }
}

#[test]
fn p1_sensor_drift_waits_for_the_field_gauge_then_holds_the_trip() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G2SensorDriftIsolated));
    let (id, out) = f.run(&mut e, "P1");
    assert!(matches!(&out, Outcome::AwaitingField { reason, .. } if reason.contains("채널 A")), "{out:?}");
    assert_eq!(verdict_of(&out), Some(Verdict::G2SensorDriftIsolated));

    // 현장 확인 전 확정은 막힙니다.
    let blocked = e.confirm(&id, "OP-201", SupervisorDecision::Confirm, f.now("P1")).unwrap();
    assert!(matches!(blocked, Outcome::Blocked { verdict: Verdict::G2FieldRecheckRequired, .. }), "{blocked:?}");

    // 현장 운전원이 14:21:05 에 찍은 게이지 26.0 MPa
    let report = f.reports("P1").remove(0);
    let taken = report.taken_at;
    let out = e.attach_report(&f.db, &id, report, taken).unwrap();
    assert!(matches!(&out, Outcome::SupervisorReview { reason, .. } if reason.contains("트립 보류")), "{out:?}");
    let s = e.store.active(&id).unwrap();
    let a = &s.analysis;
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "AOP-102-IC01");
    assert!(!a.high_risk(), "채널 우회는 고위험 조치가 아닙니다");
    let pressure = a.truth.iter().find(|r| r.item == "주증기 압력").expect("3중 대조표");
    assert_eq!(pressure.field.as_deref(), Some("26.0 MPa"));
    assert!(pressure.ledger.as_deref().unwrap().starts_with("26."), "{:?}", pressure.ledger);
    let single = pressure.single.as_deref().unwrap();
    assert!(single.starts_with("14.") && single.ends_with("MPa (A만)"), "운전원이 보던 채널 A 표시값 14.2 MPa: {single}");
    assert!(a.sds.gate_state_vector.iter().any(|c| c == "SDS_CH_A_DRIFT"), "{:?}", a.sds.gate_state_vector);
    assert_eq!(a.field_checks[0].taken_at_ms, taken.timestamp_millis(), "촬영 시각의 DCS 값과 비교합니다");

    let done = e.confirm(&id, "OP-201", SupervisorDecision::Confirm, taken).unwrap();
    assert!(matches!(done, Outcome::Finalized { .. }), "{done:?}");
    let row = &e.store.archive()[0];
    assert_eq!(row.final_code, "G2_SENSOR_DRIFT_ISOLATED");
    assert_eq!(row.seal.len(), 64);
    assert!(row.field_notice.contains("주제어실"));
    assert!(e.store.audit().of(&id).iter().any(|ev| ev.category == AuditCategory::SdsSealed));
    assert!(row.mcr_doc.contains("이○민"), "현장 보고자는 가린 이름으로만 남습니다: {}", row.mcr_doc);
    assert!(!row.mcr_doc.contains("이승민"));
}

#[test]
fn p2_names_the_stuck_valve_two_hops_up_and_forces_a_cosign() {
    let f = fixture();
    let (mut e, oracle) = engine(Some(Verdict::G3AdverseUpstreamCause));
    let (id, out) = f.run(&mut e, "P2");
    assert_eq!(verdict_of(&out), Some(Verdict::G3AdverseUpstreamCause), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    let root = a.causal_root().unwrap();
    assert_eq!((root.kks.as_str(), root.hops), ("10LAB20AA102", 2));
    assert_eq!(root.connectors, vec!["OPC-103-101".to_string()]);
    assert!((3.5..5.5).contains(&root.lag_secs), "{}", root.lag_secs);
    let d = a.draft.as_ref().unwrap();
    assert_eq!(d.aop_id, "AOP-103-FW02");
    assert!(d.high_risk && d.by_model && d.problems.is_empty(), "{d:?}");
    assert!(d.numbers.iter().all(|(_, ok)| *ok), "초안 수치는 사실과 100% 일치: {:?}", d.numbers);
    assert_eq!(a.sds.decomposed_confidences.len(), 3, "SDS 세 축");
    assert!((a.sds.min_confidence.unwrap() - 0.97).abs() < 1e-5);
    assert_eq!(a.stages.len(), 5);
    assert_eq!(a.stages[0].status, StageStatus::Skipped, "스캔 없는 신호 알람은 문서 모델을 띄우지 않습니다");
    assert_eq!(a.stages[3].status, StageStatus::Alert);
    assert!(a.chain.first().is_some_and(|c| c.tag == "10LAB20CG102"), "전파 체인은 밸브 개도 급락에서 시작합니다: {:?}", a.chain.first());
    let asked = format!("{:?}", oracle.unwrap().asked.lock().unwrap());
    assert!(asked.contains("10LAB20AA102") || asked.contains("주급수 유량조절밸브"));

    let stamp = e.confirm(&id, "OP-201", SupervisorDecision::Confirm, f.now("P2")).unwrap();
    assert!(matches!(stamp, Outcome::Blocked { verdict: Verdict::G3InterceptRubberStamp, .. }), "{stamp:?}");
    let wrong = e.confirm(&id, "OP-201", SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref: "AOP-102-TB01".into() }), f.now("P2")).unwrap();
    assert!(matches!(wrong, Outcome::Blocked { .. }), "다른 절차서 번호는 막힙니다");
    let cosign = || SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref: "AOP-103-FW02".into() });

    // 선행 절차 인터록 (Part 8-4): Step 1 (상류 펌프 토출압력 29.4 ≤ 30.0) 은 DCS 값으로 이미 통과, Step 2 (바이패스 개방) 가 남았습니다.
    let il = e.store.active(&id).unwrap().analysis.interlock.clone().unwrap();
    assert!(il.steps[0].passed() && !il.steps[1].passed() && !il.steps[2].passed(), "{il:?}");
    let early = e.confirm(&id, "OP-201", cosign(), f.now("P2")).unwrap();
    assert!(matches!(&early, Outcome::Blocked { verdict: Verdict::G3InterceptInterlockOrder, reason } if reason.contains("Step 2")), "공동 결재가 맞아도 선행 절차가 남으면 막힙니다: {early:?}");
    // 순서를 건너뛰면 막고, 개도가 모자라면 받지 않습니다.
    let skip = e.record_interlock(&id, 3, Evidence::RadioLoto { supervisor: "OP-201".into() }, "OP-201", f.now("P2")).unwrap();
    assert!(matches!(skip, Outcome::Blocked { verdict: Verdict::G3InterceptInterlockOrder, .. }), "{skip:?}");
    let short = e.record_interlock(&id, 2, Evidence::Value { value: 30.0, how: "현장 무전".into() }, "OP-201", f.now("P2")).unwrap();
    assert!(matches!(short, Outcome::Blocked { .. }), "{short:?}");
    let opened = e.record_interlock(&id, 2, Evidence::Value { value: 65.0, how: "현장 무전".into() }, "OP-201", f.now("P2")).unwrap();
    assert!(matches!(&opened, Outcome::SupervisorReview { reason, .. } if reason.contains("Step 3")), "{opened:?}");
    let loto = e.record_interlock(&id, 3, Evidence::RadioLoto { supervisor: "OP-201".into() }, "OP-201", f.now("P2")).unwrap();
    assert!(matches!(&loto, Outcome::SupervisorReview { reason, .. } if reason.contains("모두")), "{loto:?}");

    let ok = e.confirm(&id, "OP-201", cosign(), f.now("P2")).unwrap();
    assert!(matches!(ok, Outcome::Finalized { .. }), "{ok:?}");
    assert_eq!(e.store.archive()[0].final_code, "APPROVED_HIGH_RISK:G3_ADVERSE_UPSTREAM_CAUSE");
    // 무입증 확정 · 다른 절차서 번호 · 선행 절차 미완료 확정 · 순서 건너뛰기, 네 번 막았습니다.
    let intercepted = e.store.audit().of(&id).iter().filter(|ev| ev.category == AuditCategory::IllegalActionIntercepted).count();
    assert_eq!(intercepted, 4);
}

#[test]
fn p3_an_approved_redline_is_applied_as_an_overlay() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G2MocVerified));
    let (id, out) = f.run(&mut e, "P3");
    assert_eq!(verdict_of(&out), Some(Verdict::G2MocVerified), "{out:?}");
    assert!(matches!(&out, Outcome::SupervisorReview { .. }), "보조 모드에서는 교대감독이 확정합니다");
    let done = e.confirm(&id, "OP-201", SupervisorDecision::Confirm, f.now("P3")).unwrap();
    assert!(matches!(done, Outcome::Finalized { .. }), "{done:?}");
    let o = &e.store.overlays()[0];
    assert_eq!((o.moc_no.as_str(), o.tag.as_str(), o.new), ("MOC-2026-1104", "10LAB10CP001", 31.0));
    let effective = e.store.effective(f.db.sensor("10LAB10CP001").unwrap());
    assert_eq!(effective.alarm_high, Some(31.0));
    assert!(e.store.archive()[0].field_notice.contains("반영되었습니다"));
}

#[test]
fn p3_auto_mode_finalizes_an_approved_redline_on_its_own() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G2MocVerified));
    e.enable_automation("OP-201", "발전소 운영 지침 제12조 (MOC 오버레이)", f.now("P3")).unwrap();
    let (_, out) = f.run(&mut e, "P3");
    assert!(matches!(out, Outcome::AutoFinalized { .. }), "{out:?}");
    assert_eq!(e.store.overlays().len(), 1);
    assert!(e.store.archive()[0].auto);
}

#[test]
fn p3_a_forged_redline_is_locked_and_only_the_rejection_can_be_confirmed() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G2MocVerified));
    e.enable_automation("OP-201", "지침", f.now("P3F")).unwrap();
    let (id, out) = f.run(&mut e, "P3F");
    assert!(matches!(&out, Outcome::Blocked { verdict: Verdict::G2RejectContradictionLock, reason } if reason.contains("35.0")), "{out:?}");
    assert!(e.store.audit().of(&id).iter().any(|ev| ev.category == AuditCategory::ContradictionBlocked));
    assert!(e.store.overlays().is_empty(), "위조 수치는 반영하지 않습니다");
    let done = e.confirm(&id, "OP-201", SupervisorDecision::Confirm, f.now("P3F")).unwrap();
    assert!(matches!(done, Outcome::Finalized { .. }));
    assert!(e.store.overlays().is_empty());
    assert!(e.store.archive()[0].field_notice.contains("확인하지 못했습니다"));
}

#[test]
fn p4_a_high_risk_unit_fault_cannot_be_closed_or_stamped_without_grounds() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G3ViolationHighRisk));
    let (id, out) = f.run(&mut e, "P4");
    assert_eq!(verdict_of(&out), Some(Verdict::G3ViolationHighRisk), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert!(a.causal_root().is_none());
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "AOP-102-TB01");
    assert!(a.high_risk());
    let now = f.now("P4");
    assert!(matches!(e.confirm(&id, "OP-201", SupervisorDecision::Reject, now).unwrap(), Outcome::Blocked { .. }));
    assert!(matches!(e.confirm(&id, "OP-201", SupervisorDecision::ApproveHighRisk(Justification::ProveFalsePositive { evidence_scans: 0 }), now).unwrap(), Outcome::Blocked { .. }));
    assert!(matches!(e.confirm(&id, "OP-201", SupervisorDecision::ApproveHighRisk(Justification::CancelMistake), now).unwrap(), Outcome::SupervisorReview { .. }));
    let ok = e.confirm(&id, "OP-201", SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref: "AOP-102-TB01".into() }), now).unwrap();
    assert!(matches!(ok, Outcome::Finalized { .. }), "{ok:?}");
}

#[test]
fn p5_a_spike_is_filtered_and_auto_mode_can_close_it() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G3EligibleNormal));
    let (id, out) = f.run(&mut e, "P5");
    assert_eq!(verdict_of(&out), Some(Verdict::G3EligibleNormal), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert!(a.sds.gate_state_vector.iter().any(|c| c == "G1_SPIKE_FILTERED"));
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "AOP-COM-EMI01");

    let (mut auto, _) = engine(Some(Verdict::G3EligibleNormal));
    auto.enable_automation("OP-201", "지침", f.now("P5")).unwrap();
    let (_, out) = f.run(&mut auto, "P5");
    assert!(matches!(out, Outcome::AutoFinalized { .. }), "{out:?}");
}

#[test]
fn p6_a_frozen_sensor_goes_to_instrument_maintenance() {
    let f = fixture();
    let (mut e, _) = engine(None);
    let (id, out) = f.run(&mut e, "P6");
    assert_eq!(verdict_of(&out), Some(Verdict::G1SensorFlatline), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "AOP-COM-IC02");
    assert!(!a.draft.as_ref().unwrap().by_model, "글쓰기 모델이 없으면 템플릿입니다");
    assert!(a.stages[3].summary().contains("규칙만"));
}

#[test]
fn p7_a_steep_furnace_pressure_rise_is_traced_to_the_fan_before_the_alarm() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G3AdverseUpstreamCause));
    let (id, out) = f.run(&mut e, "P7");
    assert_eq!(verdict_of(&out), Some(Verdict::G3AdverseUpstreamCause), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert_eq!(a.sds.trigger_kind, "HIGH_SLOPE_PRE_ALARM");
    assert!(a.trigger.ttf_secs.is_some_and(|t| t <= 30.0), "{:?}", a.trigger);
    assert_eq!(a.causal_root().unwrap().kks, "10HLB10AN001");
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "AOP-105-DR01");
}

#[test]
fn a_bad_signature_sends_the_field_terminal_back_without_opening_a_case() {
    let f = fixture();
    let (mut e, _) = engine(None);
    let s = f.scenario("P3");
    let mut incident = s.incident(&f.set, "INC-SIGN");
    let mut report = f.reports("P3").remove(0);
    report.signature = Some(Signature { stroke_count: 1, bbox_area_ratio: 0.03, single_line: true });
    incident.reports = vec![report];
    let out = e.run(&f.db, incident, f.now("P3")).unwrap();
    assert!(matches!(out, Outcome::FieldRetry { verdict: Verdict::G1SignInvalidRetry, .. }), "{out:?}");
    assert_eq!(e.store.active_count(), 0);
}

#[test]
fn an_invented_number_in_the_aop_falls_back_to_the_template() {
    let f = fixture();
    let oracle = std::sync::Arc::new(Oracle::new(Verdict::G3AdverseUpstreamCause));
    let mut e = Engine::new(b"salt", AutomationPolicy::default(), Some(Box::new(Shared(oracle)))).with_writer(Box::new(Liar));
    let (id, _) = f.run(&mut e, "P2");
    let a = &e.store.active(&id).unwrap().analysis;
    let d = a.draft.as_ref().unwrap();
    assert!(!d.by_model);
    assert!(d.problems.iter().any(|p| p.contains("700.0")), "{:?}", d.problems);
    assert!(d.summary.contains("10LAB20AA102"), "템플릿은 사실 줄로 만듭니다: {}", d.summary);
    assert!(a.stages[4].summary().contains("템플릿"));
}

#[test]
fn the_model_never_sees_worker_names_or_ids() {
    let f = fixture();
    let (mut e, oracle) = engine(Some(Verdict::G2SensorDriftIsolated));
    let (id, _) = f.run(&mut e, "P1");
    let report = f.reports("P1").remove(0);
    let at = report.taken_at;
    e.attach_report(&f.db, &id, report, at).unwrap();
    let (_, _) = f.run(&mut e, "P3");
    let asked = format!("{:?}", oracle.unwrap().asked.lock().unwrap());
    for secret in ["이승민", "김태현", "E205", "E102"] {
        assert!(!asked.contains(secret), "모델에 보이면 안 되는 값: {secret}");
    }
}

#[test]
fn unmasking_a_worker_needs_a_high_risk_case_and_is_always_logged() {
    let f = fixture();
    let (mut e, _) = engine(Some(Verdict::G2MocVerified));
    let (id, _) = f.run(&mut e, "P3");
    assert_eq!(e.request_unmask(&id, "OP-201", f.now("P3")).unwrap(), None, "정상 변경 건은 실명 열람 거부");
    let cats: Vec<AuditCategory> = e.store.audit().of(&id).iter().map(|ev| ev.category).collect();
    assert!(cats.contains(&AuditCategory::PrivacyUnmaskDenied));
}

/// Part 8-5 폐루프: 물리엔진 고장 → SDS 추적 → 사건 → 네 기준 판단 → Tier 1 자동 우회 개방 → 온도 회복.
/// 판단 대역은 완화 조치에 0.97 을 줍니다 (실제 모델 값은 docs/verification.md).
#[test]
fn closed_loop_auto_bypass_recovers_the_superheater_before_trip() {
    use blueprint_core::physics::{incident_from, Command, Fault, PlantSim, Tracker, TrackState};
    use blueprint_core::tsdb::LiveRing;

    let db = Lattice::embedded().unwrap();
    let (mut e, _) = engine(Some(Verdict::G3EligibleAutoBypassMitigation));
    let t0 = Utc::now();
    e.enable_automation("OP-201", "발전처 자동화 운영 지침 4.2 (Tier 1 우회 완화)", t0).unwrap();
    let mut sim = PlantSim::new(&db, t0.timestamp_millis(), 5);
    let mut ring = LiveRing::default();
    let mut tracker = Tracker::new();
    sim.run_into(&mut ring, 250);
    sim.inject(Fault::ValveStuck { open_pct: 18.0 });
    let mut opened = None;
    for _ in 0..40 {
        sim.run_into(&mut ring, 10);
        if let Some(call) = tracker.observe(&db, &ring) {
            opened = Some(call);
            break;
        }
    }
    let call = opened.expect("과열기 선행 경보에서 사건을 엽니다");
    let incident = incident_from(&db, &call, &ring, "INC-PHYS-1");
    let out = e.run(&db, incident, Utc::now()).unwrap();
    assert_eq!(verdict_of(&out), Some(Verdict::G3AdverseUpstreamCause), "{out:?}");
    let m = e.propose_mitigation(&db, "INC-PHYS-1", Utc::now()).unwrap().expect("바이패스 완화 조치");
    assert!(m.auto && m.executed_at.is_some(), "자동화 모드 · 규칙 통과 · 최솟값 0.97 → 자동 실행: {m:?}");
    assert_eq!(m.decided.axes.len(), 4, "네 기준을 따로 묻습니다");
    assert!(e.store.audit().of("INC-PHYS-1").iter().any(|ev| ev.category == AuditCategory::AutoFinalized));
    // 사건의 판정(상류 원인 · 고위험 절차서)은 자동 확정되지 않고 교대감독에게 남습니다.
    assert!(e.store.active("INC-PHYS-1").is_some());

    let blueprint_core::mitigation::Action::OpenBypass { open_pct, .. } = m.action;
    sim.command(Command::SetBypass { open_pct });
    tracker.mitigating(&["10HAH10CT001"], sim.now_ms(), "바이패스 자동 개방");
    let mut peak: f64 = 0.0;
    for _ in 0..90 {
        sim.run_into(&mut ring, 10);
        tracker.observe(&db, &ring);
        peak = peak.max(sim.true_value("10HAH10CT001").unwrap());
    }
    let last = sim.true_value("10HAH10CT001").unwrap();
    assert!(peak < 645.0, "트립 한계 전에 꺾입니다: 고점 {peak}");
    assert!(last < peak - 3.0, "내려오고 있습니다 (열 시정수 약 140초): {peak} → {last}");
    assert!(tracker.events.iter().any(|ev| ev.to == TrackState::Recovering), "{:?}", tracker.events);
    // 우회가 열렸으니 인터록 Step 2 (바이패스 ≥ 50 %) 는 이제 계측값으로 채워집니다.
    let mut il = e.store.active("INC-PHYS-1").unwrap().analysis.interlock.clone().unwrap();
    // 1초 간격 계열은 정각 초에서 끝나므로 그 시각으로 묻습니다.
    let now = sim.now_ms() - sim.now_ms().rem_euclid(1000);
    blueprint_core::interlock::refresh_all(&mut il, &ring.snapshot(), now, &[]);
    assert!(il.steps[1].passed(), "{:?}", il.steps[1]);
}
