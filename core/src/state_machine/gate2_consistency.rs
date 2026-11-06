//! Gate 2: 신호 · 원장 정합성 교차 대조 (기획 Part 8 4절 상호작용 ③ ④, Part 8-3 3.3절).
//!
//! 3중 진실 우선순위: ① 현장 스캔 증빙(게이지 · Red-line) > ② DCS 다중 보팅 · MOC 승인 원장 > ③ 단일 센서 신고값.
//!
//! - 상호작용 ③: 단일 센서 알람값, DCS 2-out-of-3 보팅, 현장 게이지 실측을 맞춥니다. 현장 값은 반드시 촬영 시각(`t_scan`)의
//!   DCS 값과 비교합니다 (지금 값과 비교하면 오판합니다).
//! - 상호작용 ④: Red-line 의 변경 번호 · 새 한계 · 서명자의 관할 섹터 권한을 `moc_ledger` 와 맞춥니다.
//!   원장에 없거나 수치가 다르면 즉시 잠급니다 (`ContradictionBlocked`).
//!
//! 수치 비교는 모두 규칙입니다. 모델은 숫자를 계산하지 않습니다.

use serde::Serialize;

use crate::case::{FieldKind, FieldReport};
use crate::lattice::{Lattice, MocRow};
use crate::plant::fmt_value;
use crate::timeseries::Telemetry;
use crate::verdict::Verdict;

/// MOC 수치를 같다고 보는 폭
pub const MOC_TOLERANCE: f64 = 0.05;

/// 다중 채널 보팅 결과
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct VoteResult {
    pub tag: String,
    pub logic: String,
    pub at_ms: i64,
    pub channels: Vec<(String, f64)>,
    /// 가운데 값 (2oo3)
    pub voted: f64,
    pub tolerance: f64,
    /// 가운데 값에서 허용 폭 넘게 벗어난 채널
    pub outliers: Vec<String>,
    /// SDS 상태 코드 (`SDS_VOTING_CONSISTENT`, `SDS_CH_A_DRIFT`, `SDS_ALL_SLEW`)
    pub state_code: String,
}

/// 2oo3 보팅. 채널이 다 없으면 None 입니다.
pub fn vote(db: &Lattice, telemetry: &Telemetry, tag: &str, at_ms: i64) -> Option<VoteResult> {
    let row = db.voting(tag)?;
    let chs = telemetry.channels.get(tag)?;
    let channels: Vec<(String, f64)> = row.channels.iter().filter_map(|c| Some((c.clone(), chs.get(c)?.value_at(at_ms)?))).collect();
    if channels.len() != row.channels.len() {
        return None;
    }
    let mut sorted: Vec<f64> = channels.iter().map(|(_, v)| *v).collect();
    sorted.sort_by(f64::total_cmp);
    let voted = sorted[sorted.len() / 2];
    let outliers: Vec<String> = channels.iter().filter(|(_, v)| (v - voted).abs() > row.tolerance).map(|(c, _)| c.clone()).collect();
    let state_code = match outliers.as_slice() {
        [] => "SDS_VOTING_CONSISTENT".to_string(),
        [one] => format!("SDS_CH_{one}_DRIFT"),
        _ => "SDS_ALL_SLEW".to_string(),
    };
    Some(VoteResult { tag: tag.to_string(), logic: row.logic.clone(), at_ms, channels, voted, tolerance: row.tolerance, outliers, state_code })
}

/// 현장 실측과 같은 시각의 DCS 값 대조
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FieldCheck {
    pub tag: String,
    pub taken_at_ms: i64,
    pub field_value: f64,
    /// 촬영 시각의 DCS 대표값 (보팅이면 가운데 값)
    pub dcs_value: Option<f64>,
    /// 촬영 시각의 채널별 값
    pub channels: Vec<(String, f64)>,
    pub tolerance: f64,
    /// 현장 값이 DCS 대표값과 맞는가
    pub matches_dcs: bool,
    /// 현장 값과 맞는 채널
    pub matching_channels: Vec<String>,
}

/// 같다고 보는 폭: 보팅 허용 폭, 없으면 데드밴드의 5배와 정상치 2% 중 큰 값
pub fn field_tolerance(db: &Lattice, tag: &str) -> f64 {
    if let Some(v) = db.voting(tag) {
        return v.tolerance;
    }
    db.sensor(tag).map(|s| (s.deadband * 5.0).max(s.normal.abs() * 0.02)).unwrap_or(0.0)
}

/// 상호작용 ③ 의 시점 동기화 대조
pub fn field_vs_dcs(db: &Lattice, telemetry: &Telemetry, report: &FieldReport) -> Option<FieldCheck> {
    if report.kind == FieldKind::Redline {
        return None;
    }
    let tag = report.tag.as_deref()?;
    let field_value = report.reading?;
    let at = report.taken_at.timestamp_millis();
    let tolerance = field_tolerance(db, tag);
    let channels: Vec<(String, f64)> = telemetry.channels.get(tag).map(|chs| chs.iter().filter_map(|(c, s)| Some((c.clone(), s.value_at(at)?))).collect()).unwrap_or_default();
    let dcs_value = telemetry.value_at(tag, at);
    let matches_dcs = dcs_value.is_some_and(|d| (d - field_value).abs() <= tolerance);
    let matching_channels = channels.iter().filter(|(_, v)| (v - field_value).abs() <= tolerance).map(|(c, _)| c.clone()).collect();
    Some(FieldCheck { tag: tag.to_string(), taken_at_ms: at, field_value, dcs_value, channels, tolerance, matches_dcs, matching_channels })
}

/// 채널 편차를 보팅과 현장 값으로 가른 결과
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum DriftFinding {
    /// 보팅이 일치하고 현장 값도 맞음
    Consistent,
    /// 한 채널만 벗어남. `field_confirmed` 면 현장 게이지가 나머지 채널과 같아 전송기 단독 드리프트로 확정
    Isolated { channel: String, field_confirmed: bool },
    /// 현장 게이지가 벗어난 채널 쪽과 같음: 실제 공정 변화일 수 있음
    FieldSidesWithOutlier { channel: String },
    /// 여러 채널이 함께 움직임: 실제 공정 변화
    CommonMode,
}

impl DriftFinding {
    pub fn verdict(&self) -> Option<Verdict> {
        match self {
            DriftFinding::Consistent | DriftFinding::CommonMode => None,
            DriftFinding::Isolated { .. } => Some(Verdict::G2SensorDriftIsolated),
            DriftFinding::FieldSidesWithOutlier { .. } => Some(Verdict::G2FieldRecheckRequired),
        }
    }
}

pub fn drift(vote: &VoteResult, field: Option<&FieldCheck>) -> DriftFinding {
    match vote.outliers.as_slice() {
        [] => DriftFinding::Consistent,
        [one] => match field {
            Some(f) if f.matching_channels.contains(one) && !f.matches_dcs => DriftFinding::FieldSidesWithOutlier { channel: one.clone() },
            Some(f) => DriftFinding::Isolated { channel: one.clone(), field_confirmed: f.matches_dcs },
            None => DriftFinding::Isolated { channel: one.clone(), field_confirmed: false },
        },
        _ => DriftFinding::CommonMode,
    }
}

/// 3중 진실 대조표의 한 줄 (Stage 3 상세 화면)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TruthRow {
    pub item: String,
    /// ① 현장 스캔본
    pub field: Option<String>,
    /// ② 시드 원장 (보팅 · MOC 승인)
    pub ledger: Option<String>,
    /// ③ 단일 알람 · 신고값
    pub single: Option<String>,
    pub agree: bool,
}

/// MOC 대조 결과
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum MocOutcome {
    Verified,
    /// 원장에 없거나 수치가 다름: 잠금
    Contradiction { reason: String },
    /// 원장에는 있지만 승인 전이거나 서명자에게 권한이 없음: 교대감독 확인
    Hold { reason: String },
}

impl MocOutcome {
    pub fn verdict(&self) -> Verdict {
        match self {
            MocOutcome::Verified => Verdict::G2MocVerified,
            MocOutcome::Contradiction { .. } => Verdict::G2RejectContradictionLock,
            MocOutcome::Hold { .. } => Verdict::G2FieldRecheckRequired,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MocCheck {
    pub moc_no: Option<String>,
    pub ledger: Option<MocRow>,
    pub rows: Vec<TruthRow>,
    pub outcome: MocOutcome,
    /// 도면 개정 경고 (스캔한 도면이 최신 개정이 아님)
    pub stale_drawing: Option<String>,
}

fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= MOC_TOLERANCE
}

/// 상호작용 ④: Red-line ↔ 변경관리 원장
pub fn check_moc(db: &Lattice, report: &FieldReport) -> MocCheck {
    let moc_no = report.moc_no.clone();
    let ledger = moc_no.as_deref().and_then(|n| db.moc(n)).cloned();
    let stale_drawing = match (report.drawing.as_deref(), report.drawing_rev.as_deref()) {
        (Some(d), Some(rev)) => db.latest_rev(d).filter(|latest| latest != rev).map(|latest| format!("스캔한 도면 {d} {rev} 가 최신 개정 {latest} 이 아닙니다 (구버전 도면)")),
        _ => None,
    };
    let Some(m) = ledger.clone() else {
        let reason = match &moc_no {
            Some(n) => format!("변경관리 원장에 없는 변경번호입니다: {n}"),
            None => "Red-line 에 변경번호가 없습니다".to_string(),
        };
        return MocCheck { moc_no, ledger: None, rows: Vec::new(), outcome: MocOutcome::Contradiction { reason }, stale_drawing };
    };
    let text = |v: Option<f64>| v.map(|v| format!("{} {}", fmt_value(v), m.unit));
    let mut rows = vec![
        TruthRow { item: "변경번호".into(), field: moc_no.clone(), ledger: Some(m.moc_no.clone()), single: None, agree: true },
        TruthRow { item: format!("설비 {}", db.terms().code), field: report.kks.clone(), ledger: Some(m.kks.clone()), single: None, agree: report.kks.as_deref() == Some(m.kks.as_str()) },
        TruthRow { item: "기존 한계".into(), field: text(report.old_value), ledger: text(Some(m.old)), single: None, agree: report.old_value.is_none_or(|v| same(v, m.old)) },
        TruthRow { item: "변경 한계".into(), field: text(report.reading), ledger: text(Some(m.new)), single: None, agree: report.reading.is_some_and(|v| same(v, m.new)) },
    ];
    let signer = report.worker_id.as_deref();
    let sector = db.equipment(&m.kks).map(|e| e.sector.clone()).unwrap_or_default();
    let authorized = signer.is_some_and(|s| db.authorized(s, &sector));
    rows.push(TruthRow {
        item: "서명자 관할 권한".into(),
        field: signer.map(crate::privacy::mask_id),
        ledger: db.sector(&sector).map(|s| format!("{} 관할", s.building)),
        single: None,
        agree: authorized,
    });
    let outcome = if !rows[1].agree {
        MocOutcome::Contradiction { reason: format!("Red-line 의 설비({})가 원장의 설비({})와 다릅니다", report.kks.as_deref().unwrap_or("없음"), m.kks) }
    } else if !rows[3].agree {
        MocOutcome::Contradiction {
            reason: format!("Red-line 의 변경 한계 {} 가 승인 원장의 {} 와 다릅니다", text(report.reading).unwrap_or_else(|| "없음".into()), text(Some(m.new)).unwrap_or_default()),
        }
    } else if !rows[2].agree {
        MocOutcome::Contradiction { reason: "Red-line 의 기존 한계가 원장과 다릅니다".into() }
    } else if report.reading.is_some_and(|v| v > m.design_limit) {
        MocOutcome::Contradiction { reason: format!("변경 한계가 설계 한계 {} {} 를 넘습니다", fmt_value(m.design_limit), m.unit) }
    } else if !m.approved() {
        MocOutcome::Hold { reason: format!("{} 는 아직 승인되지 않은 변경입니다 (상태: {})", m.moc_no, m.status) }
    } else if !authorized {
        MocOutcome::Hold { reason: "서명자에게 이 섹터의 변경 권한이 없습니다".into() }
    } else {
        MocOutcome::Verified
    };
    MocCheck { moc_no, ledger: Some(m), rows, outcome, stale_drawing }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioSet;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    fn names(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn p1_channel_a_drift_is_isolated_and_the_field_gauge_confirms_it() {
        // 기획안 P1: DCS 14.2 MPa (채널 A) vs 보팅 · 현장 게이지 26.0 MPa
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P1").unwrap();
        let inc = s.incident(&set, "i");
        let v = vote(&db(), &inc.telemetry, "10MAA10CP001", inc.now_ms()).unwrap();
        assert_eq!(v.outliers, vec!["A".to_string()]);
        assert_eq!(v.state_code, "SDS_CH_A_DRIFT");
        assert!((v.voted - 26.0).abs() < 0.1);
        assert_eq!(drift(&v, None), DriftFinding::Isolated { channel: "A".into(), field_confirmed: false });

        let report = &s.field_reports(names)[0];
        let f = field_vs_dcs(&db(), &inc.telemetry, report).unwrap();
        assert_eq!(f.taken_at_ms, s.time(65.0).timestamp_millis(), "촬영 시각의 DCS 값과 비교합니다");
        assert!(f.matches_dcs);
        assert_eq!(f.matching_channels, vec!["B".to_string(), "C".to_string()]);
        let a_at_scan = f.channels.iter().find(|(c, _)| c == "A").unwrap().1;
        assert!((a_at_scan - 14.2).abs() < 0.1, "{a_at_scan}");
        assert_eq!(drift(&v, Some(&f)), DriftFinding::Isolated { channel: "A".into(), field_confirmed: true });
    }

    #[test]
    fn a_field_gauge_that_agrees_with_the_odd_channel_is_sent_back_for_a_recheck() {
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P1").unwrap();
        let inc = s.incident(&set, "i");
        let v = vote(&db(), &inc.telemetry, "10MAA10CP001", inc.now_ms()).unwrap();
        let mut report = s.field_reports(names).remove(0);
        report.reading = Some(14.3);
        let f = field_vs_dcs(&db(), &inc.telemetry, &report).unwrap();
        assert_eq!(drift(&v, Some(&f)), DriftFinding::FieldSidesWithOutlier { channel: "A".into() });
        assert_eq!(drift(&v, Some(&f)).verdict(), Some(Verdict::G2FieldRecheckRequired));
    }

    #[test]
    fn p3_redline_matches_the_approved_moc() {
        let set = ScenarioSet::embedded().unwrap();
        let report = &set.get("P3").unwrap().field_reports(names)[0];
        let c = check_moc(&db(), report);
        assert_eq!(c.outcome, MocOutcome::Verified, "{:?}", c.rows);
        assert!(c.rows.iter().all(|r| r.agree));
        assert!(c.stale_drawing.is_none());
    }

    #[test]
    fn p3_forged_value_is_locked() {
        // 기획안 P3: 위조 수치 35.0 MPa 차단
        let set = ScenarioSet::embedded().unwrap();
        let report = &set.get("P3F").unwrap().field_reports(names)[0];
        let c = check_moc(&db(), report);
        assert!(matches!(&c.outcome, MocOutcome::Contradiction { reason } if reason.contains("35.0")), "{:?}", c.outcome);
        assert_eq!(c.outcome.verdict(), Verdict::G2RejectContradictionLock);
    }

    #[test]
    fn unknown_unapproved_and_unauthorized_changes_are_not_verified() {
        let set = ScenarioSet::embedded().unwrap();
        let base = set.get("P3").unwrap().field_reports(names).remove(0);
        let mut unknown = base.clone();
        unknown.moc_no = Some("MOC-2026-9999".into());
        assert!(matches!(check_moc(&db(), &unknown).outcome, MocOutcome::Contradiction { .. }));
        let mut other = base.clone();
        other.worker_id = Some("E118".into());
        assert!(matches!(check_moc(&db(), &other).outcome, MocOutcome::Hold { .. }), "터빈 · 전기 담당은 급수동을 바꿀 수 없습니다");
        let mut stale = base.clone();
        stale.drawing_rev = Some("Rev.2".into());
        assert!(check_moc(&db(), &stale).stale_drawing.is_some());
        let mut pending = base;
        pending.moc_no = Some("MOC-2026-1021".into());
        pending.kks = Some("10MAV10AH001".into());
        pending.old_value = Some(55.0);
        pending.reading = Some(57.0);
        pending.worker_id = Some("E118".into());
        assert!(matches!(check_moc(&db(), &pending).outcome, MocOutcome::Hold { .. }));
    }
}
