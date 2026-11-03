//! 감사 로그 (`tb_audit_log`). 추가만 가능하고, 고치거나 지우는 API 가 없습니다.
//!
//! 기록에는 근무자 실명을 넣지 않습니다. 사건 번호와 판정 코드, 사번 해시만 씁니다.
//! 사건을 확정할 때 남기는 SDS 블랙박스 봉인(`SdsSealed`)은 사고 전후 시계열 · 게이트 상태 전이 · 사람의 반응 시간의 해시입니다 (`sds.rs`).

use chrono::{DateTime, Utc};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Actor {
    FieldTerminal,
    SupervisorConsole,
    ZoroGuardrail,
    System,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AuditCategory {
    /// 사건이 생김 (SDS 트리거)
    IncidentOpened,
    /// 위조 · 모순된 현장 기입을 잠금
    ContradictionBlocked,
    /// 고위험 건에서 작업자 실명 열람 허용
    PrivacyUnmaskGranted,
    /// 저위험 건의 실명 열람 요청 거부
    PrivacyUnmaskDenied,
    /// 근거 없는 고위험 승인 · 자동 확정 시도 등 위험한 조작을 차단
    IllegalActionIntercepted,
    /// 초안의 수치를 원문 값으로 되돌림
    FactLocked,
    /// 현장 단말로 가는 글에서 내부 정보를 차단
    LeakBlocked,
    /// 판정 후보가 정해짐
    DecisionRecorded,
    /// 자동화 모드를 켜거나 끔
    AutomationChanged,
    /// 자동 확정
    AutoFinalized,
    /// 교대감독이 확정
    SupervisorConfirmed,
    /// MOC 변경을 기준치에 오버레이로 반영
    MocApplied,
    /// SDS 블랙박스 봉인
    SdsSealed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuditEvent {
    pub at: DateTime<Utc>,
    pub actor: Actor,
    pub category: AuditCategory,
    pub incident_id: String,
    pub officer_id: Option<String>,
    pub detail: String,
}

impl AuditEvent {
    pub fn new(at: DateTime<Utc>, actor: Actor, category: AuditCategory, incident_id: &str, officer_id: Option<&str>, detail: impl Into<String>) -> Self {
        Self { at, actor, category, incident_id: incident_id.to_string(), officer_id: officer_id.map(str::to_string), detail: detail.into() }
    }
}

#[derive(Debug, Default)]
pub struct AuditLog {
    events: Vec<AuditEvent>,
}

impl AuditLog {
    pub fn append(&mut self, event: AuditEvent) {
        self.events.push(event);
    }

    pub fn events(&self) -> &[AuditEvent] {
        &self.events
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    pub fn of(&self, incident_id: &str) -> Vec<&AuditEvent> {
        self.events.iter().filter(|e| e.incident_id == incident_id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_only_grows() {
        let mut log = AuditLog::default();
        assert!(log.is_empty());
        let now = Utc::now();
        log.append(AuditEvent::new(now, Actor::System, AuditCategory::IncidentOpened, "c1", None, "x"));
        log.append(AuditEvent::new(now, Actor::SupervisorConsole, AuditCategory::SupervisorConfirmed, "c1", Some("OP-201"), "y"));
        assert_eq!(log.len(), 2);
        assert_eq!(log.of("c1").len(), 2);
        assert_eq!(log.events()[1].officer_id.as_deref(), Some("OP-201"));
    }
}
