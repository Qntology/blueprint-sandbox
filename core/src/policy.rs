//! 운영 모드와 자동화 정책 (기획 Part 8 1절 4행: 결정론적 가드레일, AI 단독 제어 금지).
//!
//! - 기본은 보조 모드(Assist)입니다. Zoro-1 은 근거와 조치 초안을 보여 주고, 확정은 주제어실 교대감독이 합니다.
//! - 자동화 모드(Auto)는 승인자 사번과 근거 규정을 적어야 켤 수 있고, 켜고 끈 기록은 감사 로그에 남습니다.
//! - 계통 이상 · 고위험 · 위조 차단(불리한 판정)과 계측 점검 · 현장 재확인(절차)은 어떤 모드에서도 자동 확정하지 않습니다.
//!   고위험 조치(출력 감발 · 터빈 정지 등)가 든 절차서는 교대감독 공동 결재가 있어야 합니다 (`pipeline::confirm`).
//!   이 규칙에는 설정 스위치가 없습니다.
//! - 자동 확정은 (1) 규칙 검사가 모두 통과하고 (2) Zoro-1 확신도가 기준 이상일 때만 허용합니다 (정상 유지 · 승인된 MOC 반영).
//!   확정된 건의 일정 비율은 사후 샘플 감사 대상으로 표시합니다.
//! - 이 시스템은 설비를 직접 제어하지 않습니다. 확정은 진단과 조치 지시서를 기록하는 일입니다.

use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::audit::{Actor, AuditCategory, AuditEvent};
use crate::verdict::{Judgement, OutcomeClass};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OperatingMode {
    #[default]
    Assist,
    Auto,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AutomationApproval {
    pub approver_id: String,
    pub legal_basis: String,
    pub approved_at: DateTime<Utc>,
}

#[derive(Debug, Error, PartialEq)]
pub enum PolicyError {
    #[error("승인자 사번이 없으면 자동화 모드를 켤 수 없습니다")]
    MissingApprover,
    #[error("근거 규정이 없으면 자동화 모드를 켤 수 없습니다")]
    MissingLegalBasis,
    #[error("신뢰도 기준은 0.5 이상 1.0 이하여야 합니다 (받은 값: {0})")]
    BadThreshold(f32),
    #[error("사후 샘플 비율은 0 이상 1 이하여야 합니다 (받은 값: {0})")]
    BadSampleRate(f32),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Disposition {
    /// 자동 확정. `post_audit` 이면 사후 샘플 감사 대상입니다.
    AutoFinalize { post_audit: bool },
    /// 교대감독이 확인해야 합니다. 사유를 함께 남깁니다.
    PendingSupervisor { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct AutomationPolicy {
    mode: OperatingMode,
    approval: Option<AutomationApproval>,
    min_confidence: f32,
    post_audit_rate: f32,
}

impl Default for AutomationPolicy {
    fn default() -> Self {
        Self { mode: OperatingMode::Assist, approval: None, min_confidence: 0.95, post_audit_rate: 0.10 }
    }
}

impl AutomationPolicy {
    pub fn mode(&self) -> OperatingMode {
        self.mode
    }

    pub fn approval(&self) -> Option<&AutomationApproval> {
        self.approval.as_ref()
    }

    pub fn min_confidence(&self) -> f32 {
        self.min_confidence
    }

    pub fn with_min_confidence(mut self, v: f32) -> Result<Self, PolicyError> {
        if !(0.5..=1.0).contains(&v) {
            return Err(PolicyError::BadThreshold(v));
        }
        self.min_confidence = v;
        Ok(self)
    }

    pub fn with_post_audit_rate(mut self, v: f32) -> Result<Self, PolicyError> {
        if !(0.0..=1.0).contains(&v) {
            return Err(PolicyError::BadSampleRate(v));
        }
        self.post_audit_rate = v;
        Ok(self)
    }

    /// 자동화 모드를 켭니다. 승인자 사번과 근거 규정이 모두 있어야 하며, 켜진 기록을 감사 이벤트로 돌려줍니다.
    pub fn enable_auto(&mut self, approver_id: &str, legal_basis: &str, at: DateTime<Utc>) -> Result<AuditEvent, PolicyError> {
        let approver_id = approver_id.trim();
        let legal_basis = legal_basis.trim();
        if approver_id.is_empty() {
            return Err(PolicyError::MissingApprover);
        }
        if legal_basis.is_empty() {
            return Err(PolicyError::MissingLegalBasis);
        }
        self.mode = OperatingMode::Auto;
        self.approval = Some(AutomationApproval { approver_id: approver_id.to_string(), legal_basis: legal_basis.to_string(), approved_at: at });
        Ok(AuditEvent::new(at, Actor::SupervisorConsole, AuditCategory::AutomationChanged, "-", Some(approver_id), format!("자동화 모드 ON · 근거: {legal_basis}")))
    }

    pub fn disable_auto(&mut self, actor_id: &str, at: DateTime<Utc>) -> AuditEvent {
        self.mode = OperatingMode::Assist;
        self.approval = None;
        AuditEvent::new(at, Actor::SupervisorConsole, AuditCategory::AutomationChanged, "-", Some(actor_id), "자동화 모드 OFF (보조 모드)")
    }

    /// 판정을 자동 확정할 수 있는지 정합니다. `gates_clear` 는 Gate 1~4 의 규칙 검사가 모두 통과했는지입니다.
    pub fn disposition(&self, judgement: &Judgement, gates_clear: bool, high_risk_actions: bool, incident_id: &str) -> Disposition {
        if self.mode == OperatingMode::Assist {
            return pending("보조 모드: 교대감독이 확정합니다");
        }
        match judgement.verdict.class() {
            OutcomeClass::Favorable => {}
            OutcomeClass::Adverse => return pending("계통 이상 · 고위험 판정은 자동 확정하지 않습니다"),
            OutcomeClass::Procedural => return pending("계측 점검 · 현장 재확인은 교대감독이 확인합니다"),
        }
        if high_risk_actions {
            return pending("고위험 조치가 든 절차서는 교대감독 공동 결재가 필요합니다");
        }
        if !gates_clear {
            return pending("규칙 검사에 미확인 항목이 있습니다");
        }
        match judgement.confidence {
            None => pending("Zoro-1 확신도가 없습니다 (모델 미로드이거나 모델이 다른 답을 냈습니다)"),
            Some(p) if p < self.min_confidence => pending(format!("Zoro-1 확신도 {p:.3} 이 기준 {:.2} 미만입니다", self.min_confidence)),
            Some(_) => Disposition::AutoFinalize { post_audit: sampled(incident_id, self.post_audit_rate) },
        }
    }
}

fn pending(reason: impl Into<String>) -> Disposition {
    Disposition::PendingSupervisor { reason: reason.into() }
}

/// 사건 번호 해시로 고르는 결정적 샘플링. 같은 사건은 언제나 같은 결과가 나옵니다.
pub fn sampled(incident_id: &str, rate: f32) -> bool {
    if rate <= 0.0 {
        return false;
    }
    if rate >= 1.0 {
        return true;
    }
    let h = Sha256::digest(incident_id.as_bytes());
    let x = u32::from_be_bytes([h[0], h[1], h[2], h[3]]) as f64 / u32::MAX as f64;
    x < rate as f64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::verdict::Verdict;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 4, 5, 0, 0).unwrap()
    }

    fn auto() -> AutomationPolicy {
        let mut p = AutomationPolicy::default();
        p.enable_auto("OP-201", "발전소 운영 지침 제12조", now()).unwrap();
        p
    }

    fn favorable(conf: Option<f32>) -> Judgement {
        Judgement { verdict: Verdict::G2MocVerified, confidence: conf }
    }

    #[test]
    fn assist_mode_never_auto_finalizes() {
        let p = AutomationPolicy::default();
        assert!(matches!(p.disposition(&favorable(Some(0.999)), true, false, "i-1"), Disposition::PendingSupervisor { .. }));
    }

    #[test]
    fn enabling_auto_requires_approver_and_basis() {
        let mut p = AutomationPolicy::default();
        assert_eq!(p.enable_auto("  ", "지침", now()).unwrap_err(), PolicyError::MissingApprover);
        assert_eq!(p.enable_auto("OP-201", " ", now()).unwrap_err(), PolicyError::MissingLegalBasis);
        assert_eq!(p.mode(), OperatingMode::Assist);
        let ev = p.enable_auto("OP-201", "지침 제12조", now()).unwrap();
        assert_eq!(ev.category, AuditCategory::AutomationChanged);
        assert_eq!(p.mode(), OperatingMode::Auto);
        p.disable_auto("OP-201", now());
        assert_eq!(p.mode(), OperatingMode::Assist);
    }

    #[test]
    fn adverse_procedural_and_high_risk_never_auto() {
        let p = auto();
        let adverse = Judgement { verdict: Verdict::G3AdverseUpstreamCause, confidence: Some(1.0) };
        assert!(matches!(p.disposition(&adverse, true, false, "i"), Disposition::PendingSupervisor { .. }));
        let drift = Judgement { verdict: Verdict::G2SensorDriftIsolated, confidence: Some(0.99) };
        assert!(matches!(p.disposition(&drift, true, false, "i"), Disposition::PendingSupervisor { .. }));
        assert!(matches!(p.disposition(&favorable(Some(0.99)), true, true, "i"), Disposition::PendingSupervisor { .. }));
    }

    #[test]
    fn favorable_needs_clear_gates_and_confidence() {
        let p = auto();
        assert!(matches!(p.disposition(&favorable(None), true, false, "i"), Disposition::PendingSupervisor { .. }));
        assert!(matches!(p.disposition(&favorable(Some(0.90)), true, false, "i"), Disposition::PendingSupervisor { .. }));
        assert!(matches!(p.disposition(&favorable(Some(0.99)), false, false, "i"), Disposition::PendingSupervisor { .. }));
        assert!(matches!(p.disposition(&favorable(Some(0.99)), true, false, "i"), Disposition::AutoFinalize { .. }));
    }

    #[test]
    fn sampling_is_deterministic_and_roughly_at_rate() {
        assert!(!sampled("x", 0.0));
        assert!(sampled("x", 1.0));
        let hits = (0..2000).filter(|i| sampled(&format!("INC-{i}"), 0.10)).count();
        assert!((120..=280).contains(&hits), "hits {hits}");
        assert!(AutomationPolicy::default().with_min_confidence(0.3).is_err());
        assert!(AutomationPolicy::default().with_post_audit_rate(1.5).is_err());
    }
}
