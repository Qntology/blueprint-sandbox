//! 저장소 (메모리 구현): 진행 중 사건 세션 · 확정 보관 · 감사 로그 · MOC 오버레이.
//!
//! - `tb_active_session`: 진행 중 사건. 확정되면 근무자 이름 · 자유 서술을 0 으로 덮고 지웁니다 (`Incident` 의 Drop).
//! - `tb_official_archive`: 확정 문서와 SDS 봉인 해시. 보존 기한 동안 남깁니다.
//! - `tb_audit_log`: 추가만 가능한 감사 로그.
//! - MOC 오버레이: 정식 CAD 도면이 나오기 전에 승인된 Red-line 변경을 기준치에 겹쳐 둔 기록 (기획 Part 8 3.1절 3단계).

use std::collections::BTreeMap;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::audit::{AuditEvent, AuditLog};
use crate::case::Incident;
use crate::lattice::SensorRow;
use crate::state_machine::analysis::Analysis;
use crate::state_machine::gate4_5_finalize::ArchiveRow;
use crate::verdict::Judgement;
use crate::tsdb::LiveRing;

/// 세션이 어디서 기다리는지
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SessionStatus {
    /// 현장 게이지 · 재점검을 기다림 (프로세스 마이닝의 HOLD 병목)
    AwaitingField,
    /// 교대감독 확인 · 결재를 기다림
    SupervisorReview,
}

pub struct Session {
    pub incident: Incident,
    pub analysis: Analysis,
    pub judgement: Option<Judgement>,
    pub status: SessionStatus,
    /// 교대감독에게 보이는 대기 사유
    pub reason: String,
    /// 확인할 일 (비어 있지 않으면 자동 확정하지 않습니다)
    pub notes: Vec<String>,
    pub opened_at: DateTime<Utc>,
    /// 현장 재점검 요청 메모
    pub field_request: Option<String>,
    /// 자율 대응 완화 조치 (Part 8-5)
    pub mitigation: Option<crate::mitigation::Mitigation>,
}

/// 기준치에 겹친 MOC 변경
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MocOverlay {
    pub moc_no: String,
    pub tag: String,
    pub param: String,
    pub old: f64,
    pub new: f64,
    pub applied_at: DateTime<Utc>,
    pub incident_id: String,
}

#[derive(Default)]
pub struct Store {
    active: BTreeMap<String, Session>,
    archive: Vec<ArchiveRow>,
    audit: AuditLog,
    overlays: Vec<MocOverlay>,
    /// 실시간 계측 60초 링버퍼 (Part 8-4 시계열 질의: 촬영 시각 값 · 구간 요약 · 변곡점 시차)
    live: LiveRing,
}

impl Store {
    pub fn live(&self) -> &LiveRing {
        &self.live
    }

    pub fn live_mut(&mut self) -> &mut LiveRing {
        &mut self.live
    }

    pub fn open(&mut self, session: Session) -> Result<()> {
        let id = session.incident.id.clone();
        if self.active.contains_key(&id) {
            bail!("이미 진행 중인 사건입니다: {id}");
        }
        self.active.insert(id, session);
        Ok(())
    }

    /// 세션을 꺼냅니다 (현장 보고를 붙여 다시 분석할 때)
    pub fn take(&mut self, id: &str) -> Option<Session> {
        self.active.remove(id)
    }

    pub fn active(&self, id: &str) -> Option<&Session> {
        self.active.get(id)
    }

    pub fn active_mut(&mut self, id: &str) -> Option<&mut Session> {
        self.active.get_mut(id)
    }

    pub fn sessions(&self) -> impl Iterator<Item = &Session> {
        self.active.values()
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.active.contains_key(id) || self.archive.iter().any(|a| a.incident_id == id)
    }

    pub fn record(&mut self, event: AuditEvent) {
        self.audit.append(event);
    }

    pub fn audit(&self) -> &AuditLog {
        &self.audit
    }

    pub fn archive(&self) -> &[ArchiveRow] {
        &self.archive
    }

    pub fn overlays(&self) -> &[MocOverlay] {
        &self.overlays
    }

    /// MOC 오버레이를 겹친 센서 기준치
    pub fn effective(&self, sensor: &SensorRow) -> SensorRow {
        let mut s = sensor.clone();
        for o in self.overlays.iter().filter(|o| o.tag == s.tag) {
            match o.param.as_str() {
                "alarm_high" => s.alarm_high = Some(o.new),
                "alarm_low" => s.alarm_low = Some(o.new),
                "trip_high" => s.trip_high = Some(o.new),
                "trip_low" => s.trip_low = Some(o.new),
                _ => {}
            }
        }
        s
    }

    /// Gate 5: 보관 → 오버레이 → 감사 기록 → 세션 삭제를 한 번에 합니다. 실패할 수 있는 검사를 먼저 합니다.
    pub fn finalize(&mut self, id: &str, row: ArchiveRow, overlay: Option<MocOverlay>, events: Vec<AuditEvent>) -> Result<()> {
        if !self.active.contains_key(id) {
            bail!("진행 중인 사건이 아닙니다: {id}");
        }
        if self.archive.iter().any(|r| r.archive_id == row.archive_id) {
            bail!("중복 문서번호입니다: {}", row.archive_id);
        }
        self.archive.push(row);
        self.overlays.extend(overlay);
        for e in events {
            self.audit.append(e);
        }
        drop(self.active.remove(id));
        Ok(())
    }
}
