//! 현장 순시 · 정비 단말 접수 (기획 Part 8-2 2절, zoro-gravity 키오스크의 5단계 흐름).
//!
//! 현장 단말은 스캔 · 섹터 선택 · 판독값 확인과 LOTO 서명 · 편차 소명 · 전송만 합니다.
//! 돌려주는 것은 접수 번호와 현장에 보여도 되는 안내뿐입니다. 내부 판정 코드 · 확신도 · 원장 수치 · 마인드맵은 보내지 않습니다.
//!
//! - 계기 점검표: 같은 태그로 현장 확인을 기다리는 사건이 있으면 그 사건에 붙입니다. 없으면 현장 보고 사건을 엽니다.
//! - 현장 실측치가 중앙 계측값(알람이 난 채널 값)과 크게 다르고 소명이 없으면 소명을 먼저 받습니다 (내부 수치는 보이지 않음).
//! - Red-line: 새 사건으로 열어 MOC 원장과 대조합니다. 위조 · 모순이면 잠기고, 현장에는 "확인 필요" 만 갑니다.

use std::cell::RefCell;

use chrono::{DateTime, Utc};
use serde::Serialize;
use thiserror::Error;

use crate::case::{Correction, DeviationExcuse, FieldKind, FieldReport, Incident, IncidentSource, ReadTimings, Signature};
use crate::lattice::Lattice;
use crate::plant::Sector;
use crate::state_machine::gate2_consistency::field_tolerance;
use crate::state_machine::gate4_5_finalize::check_field_text;
use crate::state_machine::staged::{self, EngineAccess};
use crate::state_machine::{Engine, Outcome};
use crate::store::SessionStatus;
use crate::timeseries::Telemetry;
use crate::verdict::Verdict;

/// 현장 단말이 보내는 입력
#[derive(Debug, Clone, Default)]
pub struct FieldForm {
    pub kind: Option<FieldKind>,
    pub sector: Option<Sector>,
    pub kks: String,
    pub tag: Option<String>,
    pub reading: Option<f64>,
    pub unit: Option<String>,
    pub taken_at: Option<DateTime<Utc>>,
    pub drawing: Option<String>,
    pub drawing_rev: Option<String>,
    pub moc_no: Option<String>,
    pub old_value: Option<f64>,
    pub worker_name: String,
    pub worker_id: String,
    pub signature: Option<Signature>,
    pub permit_confirmed: bool,
    pub excuse: Option<DeviationExcuse>,
    /// 스캔에서 읽은 교정 기록과 확인할 항목 (서버가 들고 있던 값)
    pub corrections: Vec<Correction>,
    pub uncertain: Vec<String>,
    pub timings: Option<ReadTimings>,
    pub content_hash: Option<String>,
    /// 현장 단말에서 고친 항목
    pub edited: Vec<String>,
}

#[derive(Debug, Error)]
pub enum FieldError {
    #[error("{0}")]
    Form(String),
    /// 서명을 다시 받습니다
    #[error("{0}")]
    Retry(String),
    /// 중앙 계측값과 편차가 있어 소명을 먼저 받습니다
    #[error("{message}")]
    NeedExcuse { message: String },
    #[error("접수하지 못했습니다: {0:#}")]
    Internal(#[from] anyhow::Error),
}

/// 현장 단말에 돌려주는 상태
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum FieldState {
    /// 주제어실이 확인 중
    Sent,
    /// 처리가 끝남 (안내 글)
    Finished { notice: String },
    /// 확인이 필요해 접수가 멈춤 (안내 글)
    Hold { notice: String },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldReceipt {
    pub receipt_no: String,
    pub state: FieldState,
}

pub const EXCUSE_MESSAGE: &str = "중앙 계측값과 현장 실측치 사이에 편차가 있어 현장 상황 확인이 필요합니다";

fn report_of(db: &Lattice, form: &FieldForm, kind: FieldKind, now: DateTime<Utc>) -> Result<FieldReport, FieldError> {
    let kks = db.match_kks(&form.kks).code().map(str::to_string).ok_or_else(|| FieldError::Form("설비 KKS 코드를 대장에서 찾지 못했습니다. 코드를 다시 확인해 주세요".into()))?;
    let tag = match form.tag.as_deref().filter(|t| !t.trim().is_empty()) {
        Some(t) => {
            let tag = db.match_tag(t).code().map(str::to_string).ok_or_else(|| FieldError::Form("센서 태그를 대장에서 찾지 못했습니다".into()))?;
            if db.sensor(&tag).is_some_and(|s| s.kks != kks) {
                return Err(FieldError::Form("센서 태그가 설비 코드와 맞지 않습니다".into()));
            }
            Some(tag)
        }
        // 태그를 적지 않았으면 설비의 첫 센서로 봅니다.
        None => db.sensors_of(&kks).first().map(|s| s.tag.clone()),
    };
    // 사번으로만 찾습니다. 예전에는 모르는 사번이면 이름으로 찾아 원장 사번으로 조용히 바꿔 접수해, 이름만 알면 접수가 됐습니다
    // (UI 단순화 K2). 스캔에서 사번을 잘못 읽었으면 스캔 단계가 원장 사번으로 고치고 단말에 "교정됨" 으로 보입니다 (extract.rs).
    let worker = db.person(&form.worker_id.trim().to_uppercase()).ok_or_else(|| FieldError::Form("점검자 사번을 원장에서 찾지 못했습니다. 사번을 다시 확인해 주세요".into()))?;
    if !form.worker_name.trim().is_empty() && worker.name != form.worker_name.trim() {
        return Err(FieldError::Form("점검자 이름과 사번이 맞지 않습니다".into()));
    }
    if kind != FieldKind::Redline && form.reading.is_none() {
        return Err(FieldError::Form("현장 실측치를 적어 주세요".into()));
    }
    if kind == FieldKind::Redline && form.moc_no.as_deref().is_none_or(|m| m.trim().is_empty()) {
        return Err(FieldError::Form("Red-line 의 변경번호(MOC)를 적어 주세요".into()));
    }
    let mut r = FieldReport::new(kind, form.taken_at.unwrap_or(now));
    r.sector = form.sector.or_else(|| db.equipment(&kks).and_then(|e| Sector::from_key(&e.sector)));
    r.kks = Some(kks);
    r.tag = tag;
    r.reading = form.reading;
    r.unit = form.unit.clone();
    r.drawing = form.drawing.clone();
    r.drawing_rev = form.drawing_rev.clone();
    r.moc_no = form.moc_no.as_deref().map(|m| db.similar_moc(m).map(|x| x.moc_no.clone()).unwrap_or_else(|| m.trim().to_string()));
    r.old_value = form.old_value;
    r.worker_id = Some(worker.id.clone());
    r.worker_name = Some(worker.name.clone());
    r.signature = form.signature;
    r.permit_confirmed = form.permit_confirmed;
    r.excuse = form.excuse.clone();
    r.corrections = form.corrections.clone();
    r.uncertain = form.uncertain.clone();
    r.uncertain.extend(form.edited.iter().map(|e| format!("현장 단말에서 고친 값 확인: {e}")));
    r.timings = form.timings;
    r.content_hash = form.content_hash.clone();
    Ok(r)
}

/// 같은 설비 · 태그로 현장 확인을 기다리는 사건 (없으면 같은 태그의 진행 중 사건)
fn waiting_incident(engine: &Engine, tag: Option<&str>, kks: Option<&str>) -> Option<String> {
    let matches = |s: &&crate::store::Session| s.analysis.route.tag.as_deref() == tag || Some(s.analysis.route.kks.as_str()) == kks;
    let sessions: Vec<_> = engine.store.sessions().filter(|s| s.incident.alarm.is_some()).collect();
    sessions.iter().find(|s| s.status == SessionStatus::AwaitingField && matches(s)).or_else(|| sessions.iter().find(|s| matches(s))).map(|s| s.incident.id.clone())
}

/// 현장 보고를 접수합니다. `live` 는 새 현장 사건에 붙일 지금 계측입니다 (시뮬레이터 값, 없으면 빈 계측).
pub fn submit(engine: &mut Engine, db: &Lattice, form: &FieldForm, live: Option<&Telemetry>, now: DateTime<Utc>) -> Result<FieldReceipt, FieldError> {
    submit_staged(&RefCell::new(engine), db, form, live, now)
}

/// [`submit`] 과 같습니다. 엔진 잠금은 준비 · 반영 때만 쥐고, 판단 · 글쓰기 모델이 도는 동안은 풉니다 ([`staged`]).
pub fn submit_staged(access: &impl EngineAccess, db: &Lattice, form: &FieldForm, live: Option<&Telemetry>, now: DateTime<Utc>) -> Result<FieldReceipt, FieldError> {
    let kind = form.kind.unwrap_or(FieldKind::GaugeCard);
    let report = report_of(db, form, kind, now)?;
    let fresh = |id: &str, report: FieldReport| Incident { id: id.to_string(), source: IncidentSource::Field, alarm: None, telemetry: live.cloned().unwrap_or_default(), reports: vec![report], received_at: now, scenario: None };
    let (receipt_no, outcome) = if kind == FieldKind::Redline {
        staged::open(access, db, "FLD", now, false, |id| fresh(id, report))?
    } else {
        let waiting = access.engine(|engine| -> Result<Option<String>, FieldError> {
            let Some(id) = waiting_incident(engine, report.tag.as_deref(), report.kks.as_deref()) else { return Ok(None) };
            if report.excuse.is_none() {
                let session = engine.store.active(&id).expect("세션");
                // 알람을 낸 채널의 값(운전원이 본 중앙 표시값)과 비교합니다. 내부 값은 안내에 넣지 않습니다.
                let at = report.taken_at.timestamp_millis();
                let reference = session.incident.alarm.as_ref().filter(|a| Some(a.tag.as_str()) == report.tag.as_deref()).map(|a| {
                    a.channel.as_ref().and_then(|c| session.incident.telemetry.channels.get(&a.tag)?.get(c)?.value_at(at)).unwrap_or(a.value)
                });
                let tolerance = report.tag.as_deref().map(|t| field_tolerance(db, t)).unwrap_or(0.0);
                if let (Some(v), Some(r)) = (report.reading, reference) {
                    if (v - r).abs() > tolerance {
                        return Err(FieldError::NeedExcuse { message: EXCUSE_MESSAGE.to_string() });
                    }
                }
            }
            Ok(Some(id))
        })?;
        match waiting {
            Some(id) => {
                let at = report.taken_at;
                let out = staged::attach_report(access, db, &id, report, at.max(now))?;
                (id, out)
            }
            None => staged::open(access, db, "FLD", now, false, |id| fresh(id, report))?,
        }
    };
    let state = match outcome {
        Outcome::FieldRetry { reason, .. } => return Err(FieldError::Retry(reason)),
        Outcome::Blocked { verdict: Verdict::G2RejectContradictionLock, .. } => FieldState::Hold { notice: "제출한 변경 내용을 승인 기록에서 확인하지 못했습니다. 기술책임자에게 변경 승인 여부를 확인해 주세요.".to_string() },
        Outcome::AutoFinalized { .. } | Outcome::Finalized { .. } => {
            let notice = access.engine(|e| e.store.archive().iter().find(|a| a.incident_id == receipt_no).map(|a| a.field_notice.clone()).unwrap_or_default());
            FieldState::Finished { notice }
        }
        _ => FieldState::Sent,
    };
    // 현장으로 가는 글은 마지막으로 한 번 더 검사합니다.
    let shown = match &state {
        FieldState::Finished { notice } | FieldState::Hold { notice } => notice.as_str(),
        FieldState::Sent => "",
    };
    if check_field_text(shown, &[]).is_err() {
        return Err(FieldError::Internal(anyhow::anyhow!("현장 안내에 내부 정보가 섞여 발송을 막았습니다")));
    }
    Ok(FieldReceipt { receipt_no, state })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AutomationPolicy;
    use crate::scenario::ScenarioSet;

    fn setup() -> (Lattice, ScenarioSet, Engine) {
        (Lattice::embedded().unwrap(), ScenarioSet::embedded().unwrap(), Engine::new(b"s", AutomationPolicy::default(), None))
    }

    fn gauge(reading: f64, taken: DateTime<Utc>) -> FieldForm {
        FieldForm {
            kind: Some(FieldKind::GaugeCard),
            kks: "10MAAl0ST001".into(),
            tag: Some("10MAA10CP001".into()),
            reading: Some(reading),
            unit: Some("MPa".into()),
            taken_at: Some(taken),
            worker_name: "이승민".into(),
            worker_id: "E205".into(),
            signature: Some(Signature::on_paper()),
            ..FieldForm::default()
        }
    }

    #[test]
    fn a_gauge_reading_far_from_the_alarm_value_asks_for_an_excuse_then_joins_the_waiting_case() {
        let (db, set, mut e) = setup();
        let s = set.get("P1").unwrap();
        e.run(&db, s.incident(&set, "INC-P1"), s.time(s.now_s)).unwrap();
        let taken = s.time(65.0);
        let err = submit(&mut e, &db, &gauge(26.0, taken), None, taken).unwrap_err();
        match err {
            FieldError::NeedExcuse { message } => {
                assert_eq!(message, EXCUSE_MESSAGE);
                assert!(!message.contains("14.2") && !message.contains("26.0"), "내부 수치를 보이지 않습니다");
            }
            other => panic!("{other:?}"),
        }
        let mut form = gauge(26.0, taken);
        form.excuse = Some(DeviationExcuse::TransmitterDrift);
        let receipt = submit(&mut e, &db, &form, None, taken).unwrap();
        assert_eq!(receipt.receipt_no, "INC-P1", "기다리던 사건에 붙습니다");
        assert_eq!(receipt.state, FieldState::Sent);
        let a = &e.store.active("INC-P1").unwrap().analysis;
        assert!(a.notes.iter().any(|n| n.contains("전송기")), "{:?}", a.notes);
        assert!(a.reason.contains("트립 보류"), "{}", a.reason);
    }

    #[test]
    fn a_forged_redline_gets_only_a_neutral_hold_notice() {
        let (db, set, mut e) = setup();
        let s = set.get("P3F").unwrap();
        let form = FieldForm {
            kind: Some(FieldKind::Redline),
            kks: "10LABI0AP001".into(),
            tag: Some("10LAB10CP001".into()),
            reading: Some(35.0),
            unit: Some("MPa".into()),
            drawing: Some("PID-103-LAB".into()),
            drawing_rev: Some("Rev.3".into()),
            moc_no: Some("M0C-2026-1104".into()),
            old_value: Some(29.0),
            worker_name: "김태현".into(),
            worker_id: "E102".into(),
            signature: Some(Signature::on_paper()),
            permit_confirmed: true,
            ..FieldForm::default()
        };
        let receipt = submit(&mut e, &db, &form, Some(&s.telemetry(&set)), s.time(0.0)).unwrap();
        match &receipt.state {
            FieldState::Hold { notice } => {
                assert!(notice.contains("확인하지 못했습니다"));
                assert!(check_field_text(notice, &[]).is_ok());
            }
            other => panic!("{other:?}"),
        }
        assert!(receipt.receipt_no.starts_with("FLD-"));
    }

    #[test]
    fn unknown_codes_and_mismatched_workers_are_form_errors() {
        let (db, set, mut e) = setup();
        let t = set.get("P1").unwrap().time(65.0);
        let mut bad = gauge(26.0, t);
        bad.kks = "99ZZZ99ZZ999".into();
        assert!(matches!(submit(&mut e, &db, &bad, None, t), Err(FieldError::Form(_))));
        let mut wrong = gauge(26.0, t);
        wrong.worker_name = "김태현".into();
        assert!(matches!(submit(&mut e, &db, &wrong, None, t), Err(FieldError::Form(_))));
        // 모르는 사번은 이름이 원장에 있어도 원장 사번으로 바꿔 받지 않습니다 (K2)
        let mut unknown_id = gauge(26.0, t);
        unknown_id.worker_id = "E999".into();
        assert!(matches!(submit(&mut e, &db, &unknown_id, None, t), Err(FieldError::Form(m)) if m.contains("사번")));
        let mut no_id = gauge(26.0, t);
        no_id.worker_id = String::new();
        assert!(matches!(submit(&mut e, &db, &no_id, None, t), Err(FieldError::Form(_))));
        let mut unsigned = gauge(26.0, t);
        unsigned.signature = None;
        assert!(matches!(submit(&mut e, &db, &unsigned, None, t), Err(FieldError::Retry(_))));
        // 소문자로 친 사번은 같은 사번입니다
        let mut lower = gauge(26.0, t);
        lower.worker_id = "e205".into();
        assert!(!matches!(submit(&mut e, &db, &lower, None, t), Err(FieldError::Form(_))));
    }
}
