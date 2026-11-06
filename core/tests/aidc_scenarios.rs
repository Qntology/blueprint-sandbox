//! AI 데이터센터 모드 시나리오 D1 ~ D7 · D3F 의 엔드투엔드 시험 (추가 개선안 Part 2, `docs/aidc-control-plan.md`).
//!
//! 발전소 시나리오(`plant_scenarios.rs`)와 같은 엔진 · 같은 규칙을 원장 시드(`aidc_db.json`)와 시나리오(`aidc_scenarios.json`)만 바꿔 돌립니다.
//! 판단 · 글쓰기 모델 자리에는 정해 둔 답을 내는 대역을 꽂습니다.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use blueprint_core::audit::AuditCategory;
use blueprint_core::case::FieldReport;
use blueprint_core::decision::{DecisionModel, DecisionTask};
use blueprint_core::domain::DomainMode;
use blueprint_core::drafting::TextWriter;
use blueprint_core::interlock::Evidence;
use blueprint_core::lattice::Lattice;
use blueprint_core::policy::AutomationPolicy;
use blueprint_core::scenario::{Scenario, ScenarioSet};
use blueprint_core::state_machine::gate3_substantive::Justification;
use blueprint_core::state_machine::{Engine, Outcome, SupervisorDecision};
use blueprint_core::verdict::Verdict;
use chrono::{DateTime, Utc};

struct Oracle {
    want: Verdict,
    asked: Mutex<Vec<DecisionTask>>,
}

impl DecisionModel for Oracle {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.asked.lock().unwrap().push(task.clone());
        let n = task.options.len();
        let pick = task.options.iter().position(|v| *v == self.want).unwrap_or(0);
        Ok((0..n).map(|i| if i == pick { 0.97 } else { 0.03 / (n - 1) as f32 }).collect())
    }
}

struct Shared(Arc<Oracle>);

impl DecisionModel for Shared {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.0.probabilities(task)
    }
}

/// 사실 줄을 옮겨 쓰는 글쓰기 대역. 받은 지시문(도메인 용어)을 기록합니다.
#[derive(Default)]
struct Copycat {
    systems: Mutex<Vec<String>>,
}

impl TextWriter for Copycat {
    fn complete(&self, system: &str, user: &str, _max: usize) -> Result<String> {
        self.systems.lock().unwrap().push(system.to_string());
        let line = user.lines().find(|l| l.starts_with("상류 이상 설비") || l.starts_with("경보 계측값") || l.starts_with("계측 위치")).unwrap_or("원인 진단");
        Ok(format!("{} 때문입니다.", line.split_once(": ").map(|(_, v)| v).unwrap_or(line)))
    }
}

struct SharedWriter(Arc<Copycat>);

impl TextWriter for SharedWriter {
    fn complete(&self, system: &str, user: &str, max: usize) -> Result<String> {
        self.0.complete(system, user, max)
    }
}

struct Fixture {
    db: Lattice,
    set: ScenarioSet,
}

fn fixture() -> Fixture {
    Fixture { db: Lattice::embedded_for(DomainMode::AiDataCenter).unwrap(), set: ScenarioSet::embedded_for(DomainMode::AiDataCenter).unwrap() }
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

fn engine(want: Verdict) -> (Engine, Arc<Oracle>, Arc<Copycat>) {
    let oracle = Arc::new(Oracle { want, asked: Mutex::new(Vec::new()) });
    let writer = Arc::new(Copycat::default());
    let db = Lattice::embedded_for(DomainMode::AiDataCenter).unwrap();
    let mut e = Engine::new(b"salt", AutomationPolicy::default(), Some(Box::new(Shared(oracle.clone())))).with_writer(Box::new(SharedWriter(writer.clone())));
    e.reset_for(db.terms().clone());
    (e, oracle, writer)
}

fn verdict_of(out: &Outcome) -> Option<Verdict> {
    match out {
        Outcome::AwaitingField { judgement, .. } | Outcome::SupervisorReview { judgement, .. } => judgement.as_ref().map(|j| j.verdict),
        Outcome::Blocked { verdict, .. } | Outcome::FieldRetry { verdict, .. } => Some(*verdict),
        _ => None,
    }
}

/// 시나리오마다 적어 둔 기대(`expect`)를 엔진 결과와 맞춥니다: 판정 · 근본 원인 · 홉 · 도면 점프 · 절차서 · 트리거 · Gate 1 · 채널.
#[test]
fn every_data_center_scenario_meets_its_expectation() {
    let f = fixture();
    for s in &f.set.scenarios {
        let want = Verdict::from_code(s.expect["verdict"].as_str().unwrap()).unwrap();
        let (mut e, _, _) = engine(want);
        let (id, out) = f.run(&mut e, &s.id);
        assert_eq!(verdict_of(&out), Some(want), "{}: {out:?}", s.id);
        let Some(session) = e.store.active(&id) else { continue };
        let a = &session.analysis;
        if let Some(root) = s.expect.get("root_cause") {
            assert_eq!(a.causal_root().map(|r| r.kks.as_str()), root.as_str(), "{} 근본 원인", s.id);
        }
        if let Some(hops) = s.expect.get("hops").and_then(|h| h.as_u64()) {
            assert_eq!(a.causal_root().unwrap().hops as u64, hops, "{}", s.id);
        }
        if let Some(c) = s.expect.get("connector").and_then(|c| c.as_str()) {
            assert!(a.causal_root().unwrap().connectors.iter().any(|x| x == c), "{}: {:?}", s.id, a.causal_root());
        }
        if let Some(aop) = s.expect.get("aop").and_then(|x| x.as_str()) {
            assert_eq!(a.draft.as_ref().map(|d| d.aop_id.as_str()), Some(aop), "{} 절차서", s.id);
        }
        if let Some(t) = s.expect.get("trigger").and_then(|x| x.as_str()) {
            assert_eq!(a.sds.trigger_kind, t, "{}", s.id);
        }
        if let Some(g) = s.expect.get("gate1").and_then(|x| x.as_str()) {
            assert!(a.sds.gate_state_vector.iter().any(|c| c == g), "{}: {:?}", s.id, a.sds.gate_state_vector);
        }
        if let Some(true) = s.expect.get("high_risk").and_then(|x| x.as_bool()) {
            assert!(a.high_risk(), "{}", s.id);
        }
        if let Some(true) = s.expect.get("awaits_field").and_then(|x| x.as_bool()) {
            assert!(matches!(out, Outcome::AwaitingField { .. }), "{}: {out:?}", s.id);
        }
    }
}

#[test]
fn d1_manifold_drift_holds_the_epo_until_the_field_gauge_then_closes_as_noc() {
    let f = fixture();
    let (mut e, _, _) = engine(Verdict::G2SensorDriftIsolated);
    let (id, out) = f.run(&mut e, "D1");
    assert!(matches!(&out, Outcome::AwaitingField { reason, .. } if reason.contains("채널 A")), "{out:?}");
    let blocked = e.confirm(&id, "NOC-201", SupervisorDecision::Confirm, f.now("D1")).unwrap();
    assert!(matches!(blocked, Outcome::Blocked { verdict: Verdict::G2FieldRecheckRequired, .. }), "{blocked:?}");

    // 현장 엔지니어가 15:21:05 에 찍은 매니폴드 압력계 2.4 bar
    let report = f.reports("D1").remove(0);
    let taken = report.taken_at;
    let out = e.attach_report(&f.db, &id, report, taken).unwrap();
    assert!(matches!(&out, Outcome::SupervisorReview { .. }), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert_eq!(a.draft.as_ref().unwrap().aop_id, "EOP-101-IC01");
    assert!(!a.high_risk());
    let pressure = a.truth.iter().find(|r| r.item == "매니폴드 공급 압력").expect("3중 대조표");
    assert_eq!(pressure.field.as_deref(), Some("2.4 bar"));
    assert!(pressure.single.as_deref().unwrap().ends_with("bar (A만)"), "{:?}", pressure.single);
    assert!(a.truth.iter().any(|r| r.item == "설비 자산코드"), "{:?}", a.truth.iter().map(|r| &r.item).collect::<Vec<_>>());
    assert!(a.sds.gate_state_vector.iter().any(|c| c == "SDS_CH_A_DRIFT"), "{:?}", a.sds.gate_state_vector);

    let done = e.confirm(&id, "NOC-201", SupervisorDecision::Confirm, taken).unwrap();
    assert!(matches!(done, Outcome::Finalized { .. }), "{done:?}");
    let row = &e.store.archive()[0];
    assert!(row.archive_id.starts_with("NOC-2026-"), "{}", row.archive_id);
    assert!(row.mcr_doc.starts_with("NOC 관제실 결재"), "{}", row.mcr_doc);
    assert!(row.field_notice.contains("NOC 관제실"), "{}", row.field_notice);
    assert!(row.mcr_doc.contains("이○민") && !row.mcr_doc.contains("이승민"));
}

#[test]
fn d2_names_the_stuck_cdu_valve_two_hops_up_and_forces_a_cosign_with_interlocks() {
    let f = fixture();
    let (mut e, oracle, writer) = engine(Verdict::G3AdverseUpstreamCause);
    let (id, out) = f.run(&mut e, "D2");
    assert_eq!(verdict_of(&out), Some(Verdict::G3AdverseUpstreamCause), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    let root = a.causal_root().unwrap();
    assert_eq!((root.kks.as_str(), root.hops), ("DC1CDU20AA02", 2));
    assert!((3.5..5.5).contains(&root.lag_secs), "{}", root.lag_secs);
    assert_eq!(root.physics, Some(true), "냉각수 감소 → GPU 온도 상승은 물리적으로 맞습니다");
    // 펌프 토출압 상승은 원인이 아니라 역류 여파입니다.
    let pump = a.substantive.as_ref().unwrap().causal.as_ref().unwrap().propagation.iter().find(|p| p.upstream_kks == "DC1CDU10AP01").unwrap();
    assert!(pump.backflow, "{pump:?}");
    let d = a.draft.as_ref().unwrap();
    assert_eq!(d.aop_id, "EOP-103-CDU02");
    assert!(d.high_risk && d.by_model && d.problems.is_empty(), "{d:?}");
    assert!(a.chain.first().is_some_and(|c| c.tag == "DC1CDU20CG02"), "{:?}", a.chain.first());
    // 판단 질문 · 글쓰기 지시문이 데이터센터 용어입니다.
    let asked = oracle.asked.lock().unwrap();
    assert!(asked.iter().any(|t| t.title == "AI 데이터센터 계통 이상 근본 원인 및 처분 판정"), "{:?}", asked.iter().map(|t| &t.title).collect::<Vec<_>>());
    assert!(asked.iter().flat_map(|t| &t.facts).any(|x| x.key == "셧다운 도달 예상" || x.value.contains("셧다운")));
    drop(asked);
    assert!(writer.systems.lock().unwrap().iter().all(|s| s.starts_with("AI 데이터센터 NOC 관제실의 비상 운영절차서(EOP)")));

    let cosign = || SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref: "EOP-103-CDU02".into() });
    let stamp = e.confirm(&id, "NOC-201", SupervisorDecision::Confirm, f.now("D2")).unwrap();
    assert!(matches!(stamp, Outcome::Blocked { verdict: Verdict::G3InterceptRubberStamp, .. }), "{stamp:?}");
    // 인터록 Step 1 (펌프 토출압 3.4 ≤ 3.8 bar) 은 BMC 값으로 통과, Step 2 (우회밸브 ≥ 50 %) 가 남았습니다.
    let il = e.store.active(&id).unwrap().analysis.interlock.clone().unwrap();
    assert!(il.steps[0].passed() && !il.steps[1].passed(), "{il:?}");
    assert!(il.steps[0].evidence.as_deref().unwrap().starts_with("BMC·DCIM DC1CDU10CP01"), "{:?}", il.steps[0].evidence);
    let early = e.confirm(&id, "NOC-201", cosign(), f.now("D2")).unwrap();
    assert!(matches!(early, Outcome::Blocked { verdict: Verdict::G3InterceptInterlockOrder, .. }), "{early:?}");
    e.record_interlock(&id, 2, Evidence::Value { value: 65.0, how: "현장 무전".into() }, "NOC-201", f.now("D2")).unwrap();
    e.record_interlock(&id, 3, Evidence::RadioLoto { supervisor: "NOC-201".into() }, "NOC-201", f.now("D2")).unwrap();
    let ok = e.confirm(&id, "NOC-201", cosign(), f.now("D2")).unwrap();
    assert!(matches!(ok, Outcome::Finalized { .. }), "{ok:?}");
    assert_eq!(e.store.archive()[0].final_code, "APPROVED_HIGH_RISK:G3_ADVERSE_UPSTREAM_CAUSE");
}

#[test]
fn d3_an_approved_mop_redline_is_applied_and_the_forged_one_is_locked() {
    let f = fixture();
    let (mut e, _, _) = engine(Verdict::G2MocVerified);
    let (id, out) = f.run(&mut e, "D3");
    assert_eq!(verdict_of(&out), Some(Verdict::G2MocVerified), "{out:?}");
    e.confirm(&id, "NOC-201", SupervisorDecision::Confirm, f.now("D3")).unwrap();
    let o = &e.store.overlays()[0];
    assert_eq!((o.moc_no.as_str(), o.tag.as_str(), o.new), ("MOP-2026-1104", "DC1CDU10CP01", 3.5));
    assert_eq!(e.store.effective(f.db.sensor("DC1CDU10CP01").unwrap()).alarm_high, Some(3.5));

    let (mut e, _, _) = engine(Verdict::G2MocVerified);
    let (id, out) = f.run(&mut e, "D3F");
    assert!(matches!(&out, Outcome::Blocked { verdict: Verdict::G2RejectContradictionLock, reason } if reason.contains("4.5")), "{out:?}");
    assert!(e.store.audit().of(&id).iter().any(|ev| ev.category == AuditCategory::ContradictionBlocked));
    assert!(e.store.overlays().is_empty(), "위조 수치는 반영하지 않습니다");
}

#[test]
fn d4_a_battery_thermal_runaway_precursor_needs_the_two_person_rule() {
    let f = fixture();
    let (mut e, _, _) = engine(Verdict::G3ViolationHighRisk);
    let (id, out) = f.run(&mut e, "D4");
    assert_eq!(verdict_of(&out), Some(Verdict::G3ViolationHighRisk), "{out:?}");
    let a = &e.store.active(&id).unwrap().analysis;
    assert!(a.causal_root().is_none(), "배터리실 공조 등 상류에는 이상이 없습니다");
    let d = a.draft.as_ref().unwrap();
    assert!(d.risk_actions.iter().any(|r| r.contains("EPO")), "{:?}", d.risk_actions);
    let now = f.now("D4");
    assert!(matches!(e.confirm(&id, "NOC-201", SupervisorDecision::Confirm, now).unwrap(), Outcome::Blocked { verdict: Verdict::G3InterceptRubberStamp, .. }));
    assert!(matches!(e.confirm(&id, "NOC-201", SupervisorDecision::Reject, now).unwrap(), Outcome::Blocked { .. }));
    let ok = e.confirm(&id, "NOC-201", SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: true, procedure_ref: "EOP-104-BAT01".into() }), now).unwrap();
    assert!(matches!(ok, Outcome::Finalized { .. }), "{ok:?}");
}

#[test]
fn d7_a_steep_chilled_water_rise_is_traced_to_the_cooling_tower_fan_before_the_alarm() {
    let f = fixture();
    let (mut e, _, _) = engine(Verdict::G3AdverseUpstreamCause);
    let (id, _) = f.run(&mut e, "D7");
    let a = &e.store.active(&id).unwrap().analysis;
    assert_eq!(a.sds.trigger_kind, "HIGH_SLOPE_PRE_ALARM");
    assert!(a.trigger.ttf_secs.is_some_and(|t| t <= 30.0), "{:?}", a.trigger);
    let root = a.causal_root().unwrap();
    assert_eq!(root.kks, "DC1CTW10AN01");
    assert_eq!(root.physics, Some(true), "팬 전류 하락 → 냉수 온도 상승");
}

/// 도메인을 바꿔도 사건 번호는 이어 셉니다. 되돌리면 같은 날 앞 도메인의 사건과 번호가 겹쳐, 번호로 기억해 둔 콘솔 상태
/// (확정 뒤 보관 문서를 한 번만 여는 것)가 새 사건에 걸렸습니다 (앱 E2E D2 에서 보관 문서가 열리지 않던 원인).
#[test]
fn incident_numbers_keep_counting_across_a_domain_switch() {
    let (mut e, _, _) = engine(Verdict::G3EligibleNormal);
    let at = DateTime::parse_from_rfc3339("2026-11-06T09:00:00Z").unwrap().with_timezone(&Utc);
    assert_eq!(e.next_id("INC", at), "INC-20261106-0001");
    e.reset_for(Lattice::embedded().unwrap().terms().clone());
    assert_eq!(e.next_id("INC", at), "INC-20261106-0002");
    assert!(e.store.archive().is_empty(), "보관 · 진행 사건은 비웁니다");
}

/// 추가 개선안 2.2절의 폐루프: CDU 밸브 고착 → SDS 추적 → 사건 → 네 기준 → Tier 1 CDU 우회밸브 자동 개방 → GPU 온도 회복 (셧다운 전).
#[test]
fn closed_loop_auto_bypass_recovers_the_gpu_before_shutdown() {
    use blueprint_core::physics::{incident_from, Command, Fault, PlantSim, Tracker};
    use blueprint_core::tsdb::LiveRing;

    let db = Lattice::embedded_for(DomainMode::AiDataCenter).unwrap();
    let hot = db.physics().tag_hot.clone();
    let (mut e, _, _) = engine(Verdict::G3EligibleAutoBypassMitigation);
    let t0 = Utc::now();
    e.enable_automation("NOC-201", "데이터센터 자율 운영 지침 4.2 (Tier 1 우회 완화)", t0).unwrap();
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
    let call = opened.expect("GPU 정션 온도 선행 경보에서 사건을 엽니다");
    let incident = incident_from(&db, &call, &ring, "INC-PHYS-D");
    let out = e.run(&db, incident, Utc::now()).unwrap();
    assert_eq!(verdict_of(&out), Some(Verdict::G3AdverseUpstreamCause), "{out:?}");
    let m = e.propose_mitigation(&db, "INC-PHYS-D", Utc::now()).unwrap().expect("CDU 우회밸브 완화 조치");
    assert!(m.auto && m.executed_at.is_some(), "{m:?}");
    let blueprint_core::mitigation::Action::OpenBypass { kks, open_pct, .. } = m.action.clone();
    assert_eq!(kks, "DC1CDU20AA03");
    sim.command(Command::SetBypass { open_pct });
    let mut peak: f64 = 0.0;
    for _ in 0..60 {
        sim.run_into(&mut ring, 10);
        peak = peak.max(sim.true_value(&hot).unwrap());
    }
    let last = sim.true_value(&hot).unwrap();
    assert!(peak < 95.0, "셧다운 한계 전에 꺾입니다: 고점 {peak:.1}");
    assert!(last < peak - 5.0, "내려오고 있습니다: {peak:.1} → {last:.1}");
}
