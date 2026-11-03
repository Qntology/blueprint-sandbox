//! Gate 1: 형식심사 — 신호 무결성과 현장 문서의 서명 · 작업허가 (기획 Part 8 5절 Stage 3, Part 8-3 3.3절).
//!
//! - 센서 동결(Flatline): 최근 30초 동안 잡음조차 없이 값이 같으면 실제 물리 현상이 아니라 계측기 행(Hang) 고장으로 봅니다.
//! - 불가능한 슬루율: 0.5초 만에 +200 °C 뛰었다 돌아오는 값은 전자기 잡음(EMI)으로 걸러 냅니다.
//! - 현장 문서: 서명 기준(획수 2회 이상, 면적 15% 이상, 직선 아님), Red-line 은 안전작업허가(PTW) · 잠금표지(LOTO) 확인이 있어야 합니다.
//!
//! 이 게이트의 검사는 모두 규칙입니다. 모델 확률로 통과시키지 않습니다.

use serde::Serialize;

use crate::case::{FieldKind, FieldReport, Signature};
use crate::lattice::Lattice;
use crate::timeseries::{Series, Spike, Telemetry};
use crate::verdict::Verdict;

/// 동결로 보는 기간 (초)
pub const FLATLINE_SECS: i64 = 30;
/// 잡음을 찾는 기간 (분석 시각 앞, 초)
pub const SPIKE_WINDOW_SECS: i64 = 60;
pub const SIGN_MIN_STROKES: u32 = 2;
pub const SIGN_MIN_AREA: f32 = 0.15;

pub fn signature_valid(s: &Signature) -> bool {
    s.stroke_count >= SIGN_MIN_STROKES && s.bbox_area_ratio >= SIGN_MIN_AREA && !s.single_line
}

/// 신호 하나의 상태
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum SignalState {
    Valid,
    Flatline { secs: i64, value: f64 },
    SpikeFiltered { spikes: Vec<Spike> },
    NoData,
}

impl SignalState {
    pub fn verdict(&self) -> Verdict {
        match self {
            SignalState::Flatline { .. } => Verdict::G1SensorFlatline,
            SignalState::SpikeFiltered { .. } => Verdict::G1SpikeFiltered,
            _ => Verdict::G1SignalValid,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalReport {
    pub tag: String,
    pub state: SignalState,
    /// 다중 채널 센서의 채널별 상태
    pub channels: Vec<(String, SignalState)>,
}

fn state_of(series: &Series, max_slew: f64, now_ms: i64) -> SignalState {
    if series.is_flat(now_ms, FLATLINE_SECS) {
        let value = series.value_at(now_ms).unwrap_or_default();
        return SignalState::Flatline { secs: FLATLINE_SECS, value };
    }
    let spikes = series.spikes(max_slew, now_ms - SPIKE_WINDOW_SECS * 1000, now_ms);
    if spikes.is_empty() {
        SignalState::Valid
    } else {
        SignalState::SpikeFiltered { spikes }
    }
}

/// 태그의 신호 건전성. 잡음은 걸러 낼 수 있게 위치를 돌려주고, 동결은 그 값을 믿지 않게 합니다.
pub fn signal_health(db: &Lattice, telemetry: &Telemetry, tag: &str, now_ms: i64) -> SignalReport {
    let max_slew = db.sensor(tag).map(|s| s.max_slew).unwrap_or(f64::INFINITY);
    let state = telemetry.get(tag).map(|s| state_of(s, max_slew, now_ms)).unwrap_or(SignalState::NoData);
    let channels = telemetry.channels.get(tag).map(|chs| chs.iter().map(|(c, s)| (c.clone(), state_of(s, max_slew, now_ms))).collect()).unwrap_or_default();
    SignalReport { tag: tag.to_string(), state, channels }
}

/// 현장 문서의 형식심사 결과
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FieldFormal {
    Pass,
    /// 현장 단말에서 서명을 다시 받습니다. 처분이 아닙니다.
    SignatureRetry { reason: String },
    /// 작업허가 확인이 없어 교대감독 확인으로 넘깁니다.
    PermitHold { reason: String },
}

impl FieldFormal {
    pub fn verdict(&self) -> Verdict {
        match self {
            FieldFormal::Pass => Verdict::G1SignValid,
            FieldFormal::SignatureRetry { .. } => Verdict::G1SignInvalidRetry,
            FieldFormal::PermitHold { .. } => Verdict::G1PermitMissingHold,
        }
    }
}

pub fn field_report(report: &FieldReport) -> FieldFormal {
    match &report.signature {
        Some(s) if signature_valid(s) => {}
        Some(_) => return FieldFormal::SignatureRetry { reason: "서명 획수나 면적이 기준에 못 미칩니다. 성함을 다시 적어 주세요".into() },
        None => return FieldFormal::SignatureRetry { reason: "서명란이 비어 있습니다".into() },
    }
    if report.kind == FieldKind::Redline && !report.permit_confirmed {
        return FieldFormal::PermitHold { reason: "설비 변경 작업의 안전작업허가(PTW) · 잠금표지(LOTO) 확인이 없습니다".into() };
    }
    FieldFormal::Pass
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioSet;
    use chrono::Utc;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    #[test]
    fn the_frozen_lube_oil_sensor_is_a_flatline() {
        let set = ScenarioSet::embedded().unwrap();
        let inc = set.get("P6").unwrap().incident(&set, "i");
        let r = signal_health(&db(), &inc.telemetry, "10MAV10CT001", inc.now_ms());
        assert!(matches!(r.state, SignalState::Flatline { value, .. } if value == 56.0), "{:?}", r.state);
        assert_eq!(r.state.verdict(), Verdict::G1SensorFlatline);
    }

    #[test]
    fn the_superheater_spike_is_filtered_and_a_real_rise_is_not() {
        let set = ScenarioSet::embedded().unwrap();
        let p5 = set.get("P5").unwrap().incident(&set, "i");
        let r = signal_health(&db(), &p5.telemetry, "10HAH10CT001", p5.now_ms());
        match &r.state {
            SignalState::SpikeFiltered { spikes } => assert_eq!(spikes.len(), 1),
            other => panic!("{other:?}"),
        }
        let p2 = set.get("P2").unwrap().incident(&set, "i");
        assert_eq!(signal_health(&db(), &p2.telemetry, "10HAH10CT001", p2.now_ms()).state, SignalState::Valid);
    }

    #[test]
    fn the_drifting_channel_is_not_a_spike_or_a_flatline() {
        let set = ScenarioSet::embedded().unwrap();
        let p1 = set.get("P1").unwrap().incident(&set, "i");
        let r = signal_health(&db(), &p1.telemetry, "10MAA10CP001", p1.now_ms());
        assert_eq!(r.channels.len(), 3);
        assert!(r.channels.iter().all(|(_, s)| *s == SignalState::Valid), "{:?}", r.channels);
    }

    #[test]
    fn field_documents_need_a_real_signature_and_redlines_need_a_permit() {
        let mut r = FieldReport::new(FieldKind::Redline, Utc::now());
        assert!(matches!(field_report(&r), FieldFormal::SignatureRetry { .. }));
        r.signature = Some(Signature { stroke_count: 1, bbox_area_ratio: 0.03, single_line: true });
        assert_eq!(field_report(&r).verdict(), Verdict::G1SignInvalidRetry);
        r.signature = Some(Signature::on_paper());
        assert_eq!(field_report(&r).verdict(), Verdict::G1PermitMissingHold);
        r.permit_confirmed = true;
        assert_eq!(field_report(&r), FieldFormal::Pass);
        let mut gauge = FieldReport::new(FieldKind::GaugeCard, Utc::now());
        gauge.signature = Some(Signature::on_paper());
        assert_eq!(field_report(&gauge), FieldFormal::Pass, "계기 점검은 작업허가가 필요 없습니다");
    }
}
