//! 파이프라인: 사건을 Gate 0 → 4 로 분석하고, 확정 경로를 정합니다 (Gate 5).
//!
//! 확정 경로는 두 가지뿐입니다.
//! 1. 자동화 정책이 허용한 자동 확정 (Auto 모드 + 유리한 판정 + 규칙 통과 + 고위험 조치 없음 + 확신도 기준 이상)
//! 2. 교대감독의 명시적 결정 (`confirm`). 고위험 조치가 든 절차서는 파트장 공동 결재가 있어야 합니다.
//!
//! 현장 서명 재요청은 판정이 아닙니다. 현장 게이지를 기다리는 사건은 세션으로 남고, 보고가 오면 다시 분석합니다.
//! 이 시스템은 설비를 제어하지 않습니다. 확정은 진단과 조치 지시서를 기록하는 일입니다.

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use super::analysis::{analyze, Analysis, Brains, Next};
use super::gate3_substantive::{approve_high_risk, ApprovalOutcome, Justification};
use super::gate4_5_finalize::{check_field_text, field_notice, retention_until, ArchiveRow};
use crate::audit::{Actor, AuditCategory, AuditEvent};
use crate::case::{FieldReport, Incident};
use crate::decision::DecisionModel;
use crate::drafting::TextWriter;
use crate::interlock::{self, Evidence, InterlockError};
use crate::mitigation::{self, Mitigation};
use crate::lattice::Lattice;
use crate::policy::{AutomationPolicy, Disposition};
use crate::privacy::{self, mask_name};
use crate::sds::{freeze, seal};
use crate::store::{MocOverlay, Session, SessionStatus, Store};
use crate::verdict::{Judgement, OutcomeClass, Verdict};

/// 교대감독이 사건에 내리는 결정
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorDecision {
    /// 진단과 조치 지시서를 확정합니다. 고위험 조치가 들었으면 막힙니다 (`ApproveHighRisk` 를 쓰세요).
    Confirm,
    /// 고위험 조치를 근거와 함께 승인합니다.
    ApproveHighRisk(Justification),
    /// 현장에 재점검을 요청합니다 (사건은 현장 대기로 남습니다).
    RequestFieldRecheck { note: String },
    /// 후보를 받아들이지 않고 닫습니다 (오경보 · 제출 반려).
    Reject,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Outcome {
    /// 현장 단말에서 서명을 다시 받습니다. 사건을 열지 않습니다.
    FieldRetry { verdict: Verdict, reason: String },
    /// 현장 게이지 · 재점검을 기다립니다 (HOLD).
    AwaitingField { judgement: Option<Judgement>, reason: String },
    /// 교대감독 확인 · 결재를 기다립니다.
    SupervisorReview { judgement: Option<Judgement>, reason: String },
    AutoFinalized { archive_id: String, post_audit: bool },
    Finalized { archive_id: String },
    /// 가드레일이 막았습니다. 세션은 그대로 남습니다.
    Blocked { verdict: Verdict, reason: String },
}

struct Closing {
    final_code: String,
    actor: Actor,
    supervisor: Option<String>,
    special_audit_flag: bool,
    post_audit: bool,
    apply_moc: bool,
}

pub struct Engine {
    pub policy: AutomationPolicy,
    /// Zoro-1 판단 (백본 + jev_head). 없으면 규칙만으로 동작하고 자동 확정은 불가능합니다.
    pub model: Option<Box<dyn DecisionModel>>,
    /// Zoro-1 글쓰기 (같은 백본 + 임베딩 표). 없으면 템플릿만 씁니다.
    pub writer: Option<Box<dyn TextWriter>>,
    pub store: Store,
    salt: Vec<u8>,
    seq: u32,
}

impl Engine {
    pub fn new(salt: &[u8], policy: AutomationPolicy, model: Option<Box<dyn DecisionModel>>) -> Self {
        Self { policy, model, writer: None, store: Store::default(), salt: salt.to_vec(), seq: 0 }
    }

    pub fn with_writer(mut self, writer: Box<dyn TextWriter>) -> Self {
        self.writer = Some(writer);
        self
    }

    /// 사건 번호 (`INC-20261104-0001`, 현장 보고는 `FLD-…`)
    pub fn next_id(&mut self, prefix: &str, at: DateTime<Utc>) -> String {
        loop {
            self.seq += 1;
            let id = format!("{prefix}-{}-{:04}", crate::plant::local(at).format("%Y%m%d"), self.seq);
            if !self.store.contains(&id) {
                return id;
            }
        }
    }

    pub fn enable_automation(&mut self, approver_id: &str, basis: &str, now: DateTime<Utc>) -> Result<()> {
        let event = self.policy.enable_auto(approver_id, basis, now)?;
        self.store.record(event);
        Ok(())
    }

    pub fn disable_automation(&mut self, actor_id: &str, now: DateTime<Utc>) {
        let event = self.policy.disable_auto(actor_id, now);
        self.store.record(event);
    }

    fn brains(&self) -> Brains<'_> {
        Brains { model: self.model.as_deref(), writer: self.writer.as_deref() }
    }

    fn sds_id(incident: &Incident) -> String {
        format!("SDS-{}", incident.id.split_once('-').map(|(_, rest)| rest).unwrap_or(&incident.id))
    }

    /// 사건 하나를 Gate 0 부터 돌립니다.
    pub fn run(&mut self, db: &Lattice, incident: Incident, now: DateTime<Utc>) -> Result<Outcome> {
        let analysis = analyze(db, &incident, self.brains(), &Self::sds_id(&incident))?;
        if analysis.next == Next::FieldRetry {
            record(&mut self.store, now, Actor::FieldTerminal, AuditCategory::DecisionRecorded, &incident.id, None, analysis.reason.clone());
            return Ok(Outcome::FieldRetry { verdict: analysis.candidate, reason: analysis.reason });
        }
        record(
            &mut self.store,
            now,
            Actor::System,
            AuditCategory::IncidentOpened,
            &incident.id,
            None,
            format!("{} · {} · {}", analysis.sds.trigger_kind, analysis.route.address.building, analysis.route.kks),
        );
        self.settle(incident, analysis, now, now)
    }

    /// 분석 결과로 세션을 열거나 자동 확정합니다. `opened_at` 은 사건이 처음 열린 시각입니다 (대기 시간 · 반응 시간 계산).
    fn settle(&mut self, incident: Incident, analysis: Analysis, now: DateTime<Utc>, opened_at: DateTime<Utc>) -> Result<Outcome> {
        let id = incident.id.clone();
        let judgement = analysis.substantive.as_ref().map(|s| s.decided.judgement.clone()).unwrap_or_else(|| Judgement::by_rule(analysis.candidate));
        record(&mut self.store, now, Actor::System, AuditCategory::DecisionRecorded, &id, None, format!("판정 후보 {} · {}", judgement.verdict.code(), analysis.reason));
        match analysis.next {
            Next::Locked => {
                record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::ContradictionBlocked, &id, None, analysis.reason.clone());
                let reason = analysis.reason.clone();
                self.open(incident, analysis, Some(judgement), SessionStatus::SupervisorReview, reason.clone(), opened_at)?;
                Ok(Outcome::Blocked { verdict: Verdict::G2RejectContradictionLock, reason })
            }
            Next::AwaitingField => {
                let reason = analysis.reason.clone();
                self.open(incident, analysis, Some(judgement.clone()), SessionStatus::AwaitingField, reason.clone(), opened_at)?;
                Ok(Outcome::AwaitingField { judgement: Some(judgement), reason })
            }
            Next::Ready | Next::FieldRetry => {
                let clear = analysis.notes.is_empty();
                match self.policy.disposition(&judgement, clear, analysis.high_risk(), &id) {
                    Disposition::AutoFinalize { post_audit } => {
                        let final_code = judgement.verdict.code().to_string();
                        let apply_moc = judgement.verdict == Verdict::G2MocVerified;
                        self.open(incident, analysis, Some(judgement), SessionStatus::SupervisorReview, String::new(), opened_at)?;
                        self.finish(&id, Closing { final_code, actor: Actor::ZoroGuardrail, supervisor: None, special_audit_flag: false, post_audit, apply_moc }, now)
                    }
                    Disposition::PendingSupervisor { reason: policy_reason } => {
                        let reason = if analysis.notes.is_empty() { format!("{} · {policy_reason}", analysis.reason) } else { format!("{} · {}", analysis.reason, analysis.notes.join(" · ")) };
                        self.open(incident, analysis, Some(judgement.clone()), SessionStatus::SupervisorReview, reason.clone(), opened_at)?;
                        Ok(Outcome::SupervisorReview { judgement: Some(judgement), reason })
                    }
                }
            }
        }
    }

    fn open(&mut self, incident: Incident, analysis: Analysis, judgement: Option<Judgement>, status: SessionStatus, reason: String, opened_at: DateTime<Utc>) -> Result<()> {
        let notes = analysis.notes.clone();
        self.store.open(Session { incident, analysis, judgement, status, reason, notes, opened_at, field_request: None, mitigation: None })
    }

    /// 진행 중인 사건에 현장 보고를 붙이고 다시 분석합니다. 분석 시각은 보고 시각까지 당깁니다.
    pub fn attach_report(&mut self, db: &Lattice, incident_id: &str, report: FieldReport, now: DateTime<Utc>) -> Result<Outcome> {
        let Some(session) = self.store.take(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        let Session { mut incident, opened_at, analysis: before, .. } = session;
        incident.received_at = incident.received_at.max(report.taken_at);
        incident.reports.push(report);
        let mut analysis = match analyze(db, &incident, self.brains(), &Self::sds_id(&incident)) {
            Ok(a) => a,
            Err(e) => {
                // 다시 분석하지 못해도 사건을 잃으면 안 됩니다. 보고를 뺀 채로 되돌립니다.
                incident.reports.pop();
                let mut analysis = analyze(db, &incident, self.brains(), &Self::sds_id(&incident))?;
                carry_interlock(&before, &mut analysis);
                self.open(incident, analysis, None, SessionStatus::SupervisorReview, format!("현장 보고를 반영하지 못했습니다: {e}"), opened_at)?;
                return Err(e);
            }
        };
        if analysis.next == Next::FieldRetry {
            let reason = analysis.reason.clone();
            incident.reports.pop();
            let mut previous = analyze(db, &incident, self.brains(), &Self::sds_id(&incident))?;
            carry_interlock(&before, &mut previous);
            let status = if previous.next == Next::AwaitingField { SessionStatus::AwaitingField } else { SessionStatus::SupervisorReview };
            let prev_reason = previous.reason.clone();
            self.open(incident, previous, None, status, prev_reason, opened_at)?;
            return Ok(Outcome::FieldRetry { verdict: Verdict::G1SignInvalidRetry, reason });
        }
        // 감사 로그에는 사번 대신 솔트 해시를 남깁니다.
        let worker = incident.reports.last().and_then(|r| r.worker_id.as_deref()).map(|w| privacy::salted_hash(&self.salt, w)[..12].to_string()).unwrap_or_else(|| "없음".into());
        record(&mut self.store, now, Actor::FieldTerminal, AuditCategory::DecisionRecorded, incident_id, None, format!("현장 보고 반영 · 재분석 (작업자 {worker})"));
        carry_interlock(&before, &mut analysis);
        self.settle(incident, analysis, now, opened_at)
    }

    /// 교대감독이 선행 절차 인터록 단계 하나를 채웁니다 (Part 8-4). 앞 단계를 건너뛰면 막고 감사 로그에 남깁니다.
    pub fn record_interlock(&mut self, incident_id: &str, step: u32, evidence: Evidence, supervisor: &str, now: DateTime<Utc>) -> Result<Outcome> {
        let Some(s) = self.store.active_mut(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        let judgement = s.judgement.clone();
        let Some(state) = s.analysis.interlock.as_mut() else { bail!("이 사건의 절차서에는 선행 절차 인터록이 없습니다") };
        match interlock::record(state, step, evidence) {
            Ok(done) => {
                let text = format!("인터록 Step {} 확인 · {}", done.no, done.evidence.clone().unwrap_or_default());
                let left = state.missing().iter().map(|m| format!("Step {}", m.no)).collect::<Vec<_>>();
                record(&mut self.store, now, Actor::SupervisorConsole, AuditCategory::DecisionRecorded, incident_id, Some(supervisor), text);
                let reason = if left.is_empty() { "선행 절차를 모두 확인했습니다. 확정할 수 있습니다".to_string() } else { format!("남은 선행 절차: {}", left.join(", ")) };
                Ok(Outcome::SupervisorReview { judgement, reason })
            }
            Err(e) => {
                let category = if matches!(e, InterlockError::OutOfOrder { .. }) { AuditCategory::IllegalActionIntercepted } else { AuditCategory::DecisionRecorded };
                let actor = if category == AuditCategory::IllegalActionIntercepted { Actor::ZoroGuardrail } else { Actor::SupervisorConsole };
                record(&mut self.store, now, actor, category, incident_id, Some(supervisor), format!("인터록 확인 거부 · {e}"));
                Ok(Outcome::Blocked { verdict: Verdict::G3InterceptInterlockOrder, reason: e.to_string() })
            }
        }
    }

    /// 글쓰기 모델 없이 분석합니다 (AOP 초안은 사실 줄 템플릿). 물리엔진 모드에서 완화 조치를 먼저 내려고 씁니다:
    /// 글쓰기(10 ~ 20초)를 기다리면 공정이 그동안 트립 쪽으로 움직입니다. 모델 초안은 [`Engine::redraft`] 로 뒤에 다시 씁니다.
    pub fn run_quick(&mut self, db: &Lattice, incident: Incident, now: DateTime<Utc>) -> Result<Outcome> {
        let writer = self.writer.take();
        let out = self.run(db, incident, now);
        self.writer = writer;
        out
    }

    /// 진행 중 사건의 AOP 초안을 글쓰기 모델로 다시 씁니다 (인터록 · 완화 조치 기록은 그대로). 글쓰기 모델이 없으면 false.
    pub fn redraft(&mut self, db: &Lattice, incident_id: &str) -> Result<bool> {
        if self.writer.is_none() {
            return Ok(false);
        }
        let Some(s) = self.store.active(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        let fresh = analyze(db, &s.incident, self.brains(), &Self::sds_id(&s.incident))?;
        let Some(s) = self.store.active_mut(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        s.analysis.draft = fresh.draft;
        if let (Some(old), Some(new)) = (s.analysis.stages.iter_mut().find(|x| x.stage == 5), fresh.stages.into_iter().find(|x| x.stage == 5)) {
            *old = new;
        }
        Ok(true)
    }

    /// 자율 대응 (Part 8-5): Tier 1 완화 조치(급수 바이패스 개방)를 세우고 네 기준으로 묻고, 정책이 자동 실행인지 정합니다.
    /// 자동이면 실행 기록까지 남기고 돌려줍니다 (호출한 쪽이 시뮬레이터에 명령을 넣습니다). 사건의 판정 · 결재는 그대로 남습니다.
    pub fn propose_mitigation(&mut self, db: &Lattice, incident_id: &str, now: DateTime<Utc>) -> Result<Option<Mitigation>> {
        let Some(s) = self.store.active(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        let Some((action, gates, facts)) = mitigation::plan(db, &s.analysis) else { return Ok(None) };
        let decided = mitigation::judge(&facts, self.brains().model)?;
        let gates_clear = gates.iter().all(|g| g.1);
        let (auto, reason, post_audit) = match self.policy.disposition(&decided.judgement, gates_clear, false, incident_id) {
            Disposition::AutoFinalize { post_audit } => (true, "자동화 모드 · 규칙 통과 · 네 기준 최솟값이 기준 이상 → Tier 1 자동 실행".to_string(), post_audit),
            Disposition::PendingSupervisor { reason } => (false, reason, false),
        };
        let label = action.label(db);
        let confidence = decided.judgement.confidence.map(|p| format!("{p:.3}")).unwrap_or_else(|| "없음".into());
        let mut m = Mitigation {
            action,
            label: label.clone(),
            tier: mitigation::Tier::Tier1,
            verdict: Verdict::G3EligibleAutoBypassMitigation,
            gates,
            facts: facts.into_iter().map(|f| (f.key, f.value)).collect(),
            decided,
            auto,
            reason: reason.clone(),
            post_audit,
            executed_at: None,
            executed_by: None,
        };
        record(&mut self.store, now, Actor::System, AuditCategory::DecisionRecorded, incident_id, None, format!("완화 조치 제안 · {label} · 최솟값 {confidence} · {reason}"));
        if auto {
            m.executed_at = Some(now);
            m.executed_by = Some("Zoro-1 자동 (Tier 1)".into());
            let audit = if post_audit { " · 사후 샘플 감사 대상" } else { "" };
            record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::AutoFinalized, incident_id, None, format!("완화 조치 자동 실행 · {label}{audit}"));
        }
        if let Some(s) = self.store.active_mut(incident_id) {
            s.mitigation = Some(m.clone());
        }
        Ok(Some(m))
    }

    /// 실시간 계측이 바뀌면 남은 인터록 단계를 다시 봅니다 (예: 자동 우회 개방 뒤 Step 2). 새로 채운 단계가 있으면 감사 기록에 남기고 true.
    pub fn refresh_interlock(&mut self, incident_id: &str, telemetry: &crate::timeseries::Telemetry, now_ms: i64, now: DateTime<Utc>) -> bool {
        let Some(s) = self.store.active_mut(incident_id) else { return false };
        let reports = s.incident.reports.clone();
        let Some(il) = s.analysis.interlock.as_mut() else { return false };
        let before: Vec<u32> = il.steps.iter().filter(|x| x.passed()).map(|x| x.no).collect();
        interlock::refresh_all(il, telemetry, now_ms, &reports);
        let newly: Vec<String> = il.steps.iter().filter(|x| x.passed() && !before.contains(&x.no)).map(|x| format!("Step {} · {}", x.no, x.evidence.clone().unwrap_or_default())).collect();
        if newly.is_empty() {
            return false;
        }
        record(&mut self.store, now, Actor::System, AuditCategory::DecisionRecorded, incident_id, None, format!("인터록 계측 확인 · {}", newly.join(" / ")));
        true
    }

    /// 보조 모드 · 확신도 미달일 때 교대감독이 완화 조치를 실행합니다 (Tier 1 이라 공동 결재는 필요 없음).
    pub fn execute_mitigation(&mut self, incident_id: &str, supervisor: &str, now: DateTime<Utc>) -> Result<Mitigation> {
        let Some(s) = self.store.active_mut(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
        let Some(m) = s.mitigation.as_mut() else { bail!("제안된 완화 조치가 없습니다") };
        if m.executed_at.is_some() {
            bail!("이미 실행한 완화 조치입니다");
        }
        m.executed_at = Some(now);
        m.executed_by = Some(format!("교대감독 {supervisor}"));
        let done = m.clone();
        record(&mut self.store, now, Actor::SupervisorConsole, AuditCategory::DecisionRecorded, incident_id, Some(supervisor), format!("완화 조치 실행 · {}", done.label));
        Ok(done)
    }

    /// 선행 절차가 남았으면 확정을 막습니다 (Part 8-6 절차 이슈 B-3 · 조치 순서 역순 위반).
    fn interlock_gate(&mut self, incident_id: &str, supervisor: &str, now: DateTime<Utc>) -> Option<Outcome> {
        let pending = {
            let state = self.store.active(incident_id)?.analysis.interlock.as_ref()?;
            let p = state.next_pending()?;
            (p.no, p.text.clone())
        };
        record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::IllegalActionIntercepted, incident_id, Some(supervisor), format!("선행 절차 Step {} 미완료 상태에서 확정 시도 → 차단", pending.0));
        Some(Outcome::Blocked { verdict: Verdict::G3InterceptInterlockOrder, reason: format!("선행 절차 Step {} ({}) 를 먼저 끝내야 확정할 수 있습니다", pending.0, pending.1) })
    }

    /// 교대감독 결정. 확정이면 Gate 5 까지 실행합니다.
    pub fn confirm(&mut self, incident_id: &str, supervisor: &str, decision: SupervisorDecision, now: DateTime<Utc>) -> Result<Outcome> {
        let (judgement, status, high_risk, aop_id) = {
            let Some(s) = self.store.active(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
            (s.judgement.clone(), s.status, s.analysis.high_risk(), s.analysis.draft.as_ref().map(|d| d.aop_id.clone()))
        };
        let Some(j) = judgement else {
            return Ok(Outcome::SupervisorReview { judgement: None, reason: "확정할 판정 후보가 없습니다".into() });
        };
        let sup = Some(supervisor.to_string());
        match decision {
            SupervisorDecision::Confirm => {
                if status == SessionStatus::AwaitingField {
                    record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::IllegalActionIntercepted, incident_id, Some(supervisor), "현장 확인 전 확정 시도 → 차단");
                    return Ok(Outcome::Blocked { verdict: Verdict::G2FieldRecheckRequired, reason: "현장 게이지 실측이 아직 없습니다. 현장 확인 뒤에 확정하세요".into() });
                }
                if high_risk {
                    record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::IllegalActionIntercepted, incident_id, Some(supervisor), "고위험 조치 절차서를 공동 결재 없이 확정 시도 → 차단");
                    return Ok(Outcome::Blocked {
                        verdict: Verdict::G3InterceptRubberStamp,
                        reason: "고위험 조치(감발 · 정지 등)가 든 절차서는 파트장 공동 결재와 근거 절차서 번호가 있어야 확정할 수 있습니다".into(),
                    });
                }
                if let Some(blocked) = self.interlock_gate(incident_id, supervisor, now) {
                    return Ok(blocked);
                }
                let apply_moc = j.verdict == Verdict::G2MocVerified;
                self.finish(incident_id, Closing { final_code: j.verdict.code().to_string(), actor: Actor::SupervisorConsole, supervisor: sup, special_audit_flag: false, post_audit: false, apply_moc }, now)
            }
            SupervisorDecision::ApproveHighRisk(justification) => match approve_high_risk(aop_id.as_deref(), Some(&justification)) {
                Ok(ApprovalOutcome::Reverted) => Ok(Outcome::SupervisorReview { judgement: Some(j), reason: "승인을 취소했습니다. 결재 전 상태입니다".into() }),
                Ok(ApprovalOutcome::Approved { special_audit_flag }) => {
                    // 오경보 처리는 절차를 실행하지 않으므로 인터록을 보지 않습니다. 절차 적용(공동 결재)은 선행 절차가 끝나야 합니다.
                    if matches!(justification, Justification::ApplyProcedure { .. }) {
                        if let Some(blocked) = self.interlock_gate(incident_id, supervisor, now) {
                            return Ok(blocked);
                        }
                    }
                    let (code, label) = match &justification {
                        Justification::ProveFalsePositive { .. } => (format!("OVERRIDE_FALSE_POSITIVE:{}", j.verdict.code()), "현장 증빙으로 오경보 처리 (감사실 보고)"),
                        _ => (format!("APPROVED_HIGH_RISK:{}", j.verdict.code()), "고위험 조치 공동 결재"),
                    };
                    record(&mut self.store, now, Actor::SupervisorConsole, AuditCategory::DecisionRecorded, incident_id, Some(supervisor), label);
                    self.finish(incident_id, Closing { final_code: code, actor: Actor::SupervisorConsole, supervisor: sup, special_audit_flag, post_audit: false, apply_moc: false }, now)
                }
                Err(verdict) => {
                    record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::IllegalActionIntercepted, incident_id, Some(supervisor), format!("근거가 맞지 않는 고위험 승인 시도({}) → 차단", j.verdict.code()));
                    Ok(Outcome::Blocked { verdict, reason: "공동 결재 확인과 초안의 절차서 번호가 모두 맞아야 승인할 수 있습니다. 오경보라면 현장 증빙 스캔이 1장 이상 있어야 합니다".into() })
                }
            },
            SupervisorDecision::RequestFieldRecheck { note } => {
                if let Some(s) = self.store.active_mut(incident_id) {
                    s.status = SessionStatus::AwaitingField;
                    s.field_request = Some(note.clone());
                    s.reason = format!("현장 재점검 요청: {note}");
                }
                record(&mut self.store, now, Actor::SupervisorConsole, AuditCategory::DecisionRecorded, incident_id, Some(supervisor), "현장 재점검 요청");
                Ok(Outcome::AwaitingField { judgement: Some(j), reason: format!("현장 재점검 요청: {note}") })
            }
            SupervisorDecision::Reject => {
                // 계통 이상 판정을 사람이 그냥 닫는 것은 오경보 입증과 같아서 근거가 있어야 합니다.
                if j.verdict.class() == OutcomeClass::Adverse && j.verdict != Verdict::G2RejectContradictionLock {
                    record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::IllegalActionIntercepted, incident_id, Some(supervisor), "계통 이상 판정을 근거 없이 반려 시도 → 차단");
                    return Ok(Outcome::Blocked { verdict: Verdict::G3InterceptRubberStamp, reason: "계통 이상 판정을 닫으려면 현장 증빙으로 오경보를 입증하세요".into() });
                }
                self.finish(incident_id, Closing { final_code: format!("SUPERVISOR_REJECT:{}", j.verdict.code()), actor: Actor::SupervisorConsole, supervisor: sup, special_audit_flag: false, post_audit: false, apply_moc: false }, now)
            }
        }
    }

    /// 작업자 실명 열람 (기획안 Stage 3 의 열람 요청). 고위험 판정에서만 허용하고, 허용 · 거부 모두 감사 로그에 남깁니다.
    pub fn request_unmask(&mut self, incident_id: &str, supervisor: &str, now: DateTime<Utc>) -> Result<Option<Vec<String>>> {
        let (risk, names) = {
            let Some(s) = self.store.active(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
            let risk = s.judgement.as_ref().filter(|j| j.verdict.class() != OutcomeClass::Favorable).and_then(|j| j.confidence).unwrap_or(0.0);
            (risk, s.incident.reports.iter().filter_map(|r| r.worker_name.clone()).collect::<Vec<_>>())
        };
        if privacy::unmask_allowed(risk) {
            record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::PrivacyUnmaskGranted, incident_id, Some(supervisor), format!("위험도 {:.1}%: 작업자 실명 열람 허용", risk * 100.0));
            Ok(Some(names))
        } else {
            record(&mut self.store, now, Actor::ZoroGuardrail, AuditCategory::PrivacyUnmaskDenied, incident_id, Some(supervisor), format!("위험도 {:.1}%: 기준 70% 미만이라 열람 거부", risk * 100.0));
            Ok(None)
        }
    }

    /// Gate 4-B (현장 안내 누출 검사) 와 Gate 5 (보관 · 오버레이 · 봉인 · 감사)
    fn finish(&mut self, incident_id: &str, closing: Closing, now: DateTime<Utc>) -> Result<Outcome> {
        let Closing { final_code, actor, supervisor, special_audit_flag, post_audit, apply_moc } = closing;
        let (row, overlay, seal_hash) = {
            let Some(s) = self.store.active(incident_id) else { bail!("진행 중인 사건이 아닙니다: {incident_id}") };
            let a = &s.analysis;
            let has_field = !s.incident.reports.is_empty();
            let notice = if has_field { field_notice(incident_id, &final_code) } else { String::new() };
            // 현장 안내에는 다른 근무자 이름이 들어가면 안 됩니다.
            let others: Vec<String> = Vec::new();
            if let Err((verdict, n)) = check_field_text(&notice, &others) {
                return Ok(Outcome::Blocked { verdict, reason: format!("현장 안내에 내부 정보 {n}건이 섞여 발송을 막았습니다") });
            }
            let confidence = s.judgement.as_ref().and_then(|j| j.confidence).map(|c| format!("{:.1}%", c * 100.0)).unwrap_or_else(|| "없음 (규칙 · 교대감독 판단)".into());
            let mut doc = format!(
                "주제어실 결재 · 사건 {incident_id} · 판정 {final_code} · Zoro-1 확신도 {confidence} · 처리 {actor:?}\n위치: {} · {}\n사유: {}",
                a.route.address.breadcrumb().join(" ▸ "),
                a.route.address.name,
                if s.reason.is_empty() { a.reason.as_str() } else { s.reason.as_str() }
            );
            if let Some(d) = &a.draft {
                doc.push_str(&format!("\n\n{}", d.text()));
                if !d.problems.is_empty() {
                    doc.push_str(&format!("\n(모델 초안의 수치 불일치 {}건으로 템플릿 요약을 썼습니다)", d.problems.len()));
                }
            }
            if let Some(who) = &supervisor {
                doc.push_str(&format!("\n결재: 교대감독 {who}"));
            }
            for r in &s.incident.reports {
                if let Some(name) = &r.worker_name {
                    doc.push_str(&format!("\n현장 보고: {} {} ({})", r.kind.label(), mask_name(name), r.worker_id.as_deref().map(privacy::mask_id).unwrap_or_default()));
                }
            }
            let tags: Vec<&str> = a.metrics.iter().map(|m| m.sensor_tag.as_str()).collect();
            let frozen = freeze(&s.incident.telemetry, &tags, a.sds.center_ms);
            let latency = (now - s.opened_at).num_milliseconds();
            let seal_hash = seal(&a.sds, &frozen, &[final_code.clone(), format!("{actor:?}")], Some(latency));
            let overlay = if apply_moc {
                a.moc.as_ref().and_then(|m| m.ledger.as_ref()).map(|m| MocOverlay { moc_no: m.moc_no.clone(), tag: m.tag.clone(), param: m.param.clone(), old: m.old, new: m.new, applied_at: now, incident_id: incident_id.to_string() })
            } else {
                None
            };
            let archive_id = format!("MCR-{}-{:04}호", crate::plant::local(now).format("%Y"), self.store.archive().len() + 1);
            let row = ArchiveRow {
                archive_id,
                incident_id: incident_id.to_string(),
                title: format!("{} {} · {}", a.route.address.building, a.route.address.name, a.candidate.label()),
                final_code: final_code.clone(),
                mcr_doc: doc,
                field_notice: notice,
                special_audit_flag,
                seal: seal_hash.clone(),
                auto: actor == Actor::ZoroGuardrail,
                closed_at: now,
                retention_until: retention_until(crate::plant::local(now).date_naive()),
                scenario: s.incident.scenario.clone(),
            };
            (row, overlay, seal_hash)
        };
        let archive_id = row.archive_id.clone();
        let mut events = vec![AuditEvent::new(now, Actor::System, AuditCategory::SdsSealed, incident_id, None, format!("SDS 블랙박스 봉인 {}", &seal_hash[..16]))];
        if let Some(o) = &overlay {
            events.push(AuditEvent::new(now, actor, AuditCategory::MocApplied, incident_id, supervisor.as_deref(), format!("{} 반영: {} {} {} → {}", o.moc_no, o.tag, o.param, o.old, o.new)));
        }
        let (category, label) = if actor == Actor::ZoroGuardrail { (AuditCategory::AutoFinalized, "자동화 모드 자동 확정") } else { (AuditCategory::SupervisorConfirmed, "교대감독 확정") };
        events.push(AuditEvent::new(now, actor, category, incident_id, supervisor.as_deref(), format!("{label}: {final_code}")));
        self.store.finalize(incident_id, row, overlay, events)?;
        Ok(if actor == Actor::ZoroGuardrail { Outcome::AutoFinalized { archive_id, post_audit } } else { Outcome::Finalized { archive_id } })
    }
}

fn record(store: &mut Store, at: DateTime<Utc>, actor: Actor, category: AuditCategory, id: &str, officer: Option<&str>, detail: impl Into<String>) {
    store.record(AuditEvent::new(at, actor, category, id, officer, detail));
}

/// 다시 분석해도 교대감독이 이미 확인한 인터록 단계는 그대로 둡니다 (현장 보고가 들어올 때).
fn carry_interlock(before: &Analysis, after: &mut Analysis) {
    let (Some(old), Some(new)) = (before.interlock.as_ref(), after.interlock.as_mut()) else { return };
    if old.aop_id != new.aop_id {
        return;
    }
    for step in new.steps.iter_mut().filter(|s| !s.passed()) {
        if let Some(prev) = old.steps.iter().find(|p| p.no == step.no && p.passed()) {
            *step = prev.clone();
        }
    }
}
