//! SDS (Selective Decision/State Snapshot & Decomposition) 로직 (기획 Part 8-3).
//!
//! - SDS-Trigger: 평시 흔들림(데드밴드 안)은 버리고, 세 조건 중 하나일 때만 사건 세션을 엽니다.
//!   1) 정적 임계 돌파 (`x ≥ 경보 한계`), 2) 동적 기울기 돌파 (경보 전이지만 트립까지 `TTF ≤ 30초`), 3) 현장 스캔 투입
//! - SDS-Snapshot: 변곡점 전후 ±30초 시계열을 정수로 동결하고, 게이트별 상태 코드와 축별 확신도를 한 패킷으로 묶습니다.
//! - 블랙박스 봉인: 확정할 때 동결한 시계열 · 상태 전이 · 판단 기록 · 사람의 반응 시간을 SHA-256 으로 봉인해 감사 로그에 남깁니다.
//!   같은 입력은 언제나 같은 해시라서 나중에 원본으로 다시 계산해 바뀌지 않았음을 확인할 수 있습니다.

use std::collections::BTreeMap;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::lattice::SensorRow;
use crate::timeseries::{ttf, Series, Telemetry, BASELINE_SECS, MILLI, SLOPE_SAMPLES};

/// 동적 기울기 트리거의 기준 (트립 도달 예상 시간, 초)
pub const PRE_ALARM_TTF_SECS: f64 = 30.0;
/// 스냅샷이 동결하는 변곡점 앞뒤 길이 (초)
pub const FREEZE_HALF_SECS: i64 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum TriggerKind {
    ThresholdBreach,
    HighSlopePreAlarm,
    FieldScanInjected,
}

impl TriggerKind {
    pub fn code(self) -> &'static str {
        match self {
            TriggerKind::ThresholdBreach => "THRESHOLD_BREACH",
            TriggerKind::HighSlopePreAlarm => "HIGH_SLOPE_PRE_ALARM",
            TriggerKind::FieldScanInjected => "FIELD_SCAN_INJECTED",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TriggerKind::ThresholdBreach => "정적 임계 돌파",
            TriggerKind::HighSlopePreAlarm => "동적 기울기 돌파 (선행 경보)",
            TriggerKind::FieldScanInjected => "현장 스캔 투입",
        }
    }
}

/// 사건을 연 이유
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SdsTrigger {
    pub kind: TriggerKind,
    pub tag: Option<String>,
    pub at_ms: i64,
    pub value: Option<f64>,
    /// 초당 변화율
    pub slope: Option<f64>,
    /// 트립 도달 예상 잔여 시간 (초)
    pub ttf_secs: Option<f64>,
}

/// `at_ms` 에서 이 센서가 사건을 열어야 하는가. 데드밴드 안의 흔들림은 열지 않습니다.
pub fn trigger_at(sensor: &SensorRow, series: &Series, at_ms: i64) -> Option<SdsTrigger> {
    let value = series.value_at(at_ms)?;
    let slope = series.slope_at(at_ms, SLOPE_SAMPLES);
    let remain = slope.and_then(|s| ttf(sensor, value, s));
    let base = |kind| SdsTrigger { kind, tag: Some(sensor.tag.clone()), at_ms, value: Some(value), slope, ttf_secs: remain };
    if sensor.alarm_side(value).is_some() {
        return Some(base(TriggerKind::ThresholdBreach));
    }
    let departed = series.departure(sensor.deadband, series.t0_ms + BASELINE_SECS * 1000, at_ms).is_some();
    (departed && remain.is_some_and(|r| r <= PRE_ALARM_TTF_SECS)).then(|| base(TriggerKind::HighSlopePreAlarm))
}

/// `from_ms` 부터 표본마다 보아 처음으로 사건을 여는 순간
pub fn first_trigger(sensor: &SensorRow, series: &Series, from_ms: i64, until_ms: i64) -> Option<SdsTrigger> {
    let mut t = from_ms.max(series.t0_ms + BASELINE_SECS * 1000);
    while t <= until_ms {
        if let Some(found) = trigger_at(sensor, series, t) {
            return Some(found);
        }
        t += series.period_ms;
    }
    None
}

/// SDS 스냅샷 패킷 (기획안 `SdsSnapshot`)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SdsSnapshot {
    pub sds_id: String,
    pub trigger_kind: String,
    /// 게이트별 압축 상태 코드 (예: `G1_SIGNAL_VALID`, `SDS_CH_A_DRIFT`, `G3_CAUSAL_LAG_MATCH`)
    pub gate_state_vector: Vec<String>,
    /// 직교 분할 질문별 Zoro-1 확신도 (시간축 / 물리축 / 센서축)
    pub decomposed_confidences: Vec<(String, f32)>,
    /// 최소 확신도. 모델이 없거나 어느 축이 반대로 기울면 None
    pub min_confidence: Option<f32>,
    pub center_ms: i64,
}

/// 변곡점 `center_ms` 앞뒤 30초를 ×1000 정수로 동결합니다. 시계열 밖의 시각은 빠집니다.
pub fn freeze(telemetry: &Telemetry, tags: &[&str], center_ms: i64) -> BTreeMap<String, Vec<i64>> {
    let mut out = BTreeMap::new();
    for tag in tags {
        let Some(series) = telemetry.get(tag) else { continue };
        let values: Vec<i64> = (-FREEZE_HALF_SECS..=FREEZE_HALF_SECS)
            .filter_map(|s| series.value_at(center_ms + s * 1000))
            .map(|v| (v * MILLI).round() as i64)
            .collect();
        out.insert(tag.to_string(), values);
    }
    out
}

/// 블랙박스 봉인 해시 (소문자 hex 64자). 순서가 정해진 입력만 씁니다.
pub fn seal(snapshot: &SdsSnapshot, frozen: &BTreeMap<String, Vec<i64>>, decisions: &[String], response_latency_ms: Option<i64>) -> String {
    let mut h = Sha256::new();
    h.update(snapshot.sds_id.as_bytes());
    h.update(snapshot.trigger_kind.as_bytes());
    for code in &snapshot.gate_state_vector {
        h.update(code.as_bytes());
        h.update([0]);
    }
    for (axis, p) in &snapshot.decomposed_confidences {
        h.update(axis.as_bytes());
        h.update(((*p as f64) * 1e6).round().to_le_bytes());
    }
    for (tag, values) in frozen {
        h.update(tag.as_bytes());
        for v in values {
            h.update(v.to_le_bytes());
        }
    }
    for d in decisions {
        h.update(d.as_bytes());
        h.update([0]);
    }
    h.update(response_latency_ms.unwrap_or(-1).to_le_bytes());
    hex::encode(h.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::Lattice;

    fn series(f: impl Fn(f64) -> f64) -> Series {
        Series::new(-60_000, 1000, (0..151).map(|i| f((i - 60) as f64)).collect())
    }

    #[test]
    fn noise_inside_the_deadband_never_opens_a_session() {
        let db = Lattice::embedded().unwrap();
        let s = db.sensor("10HAH10CT001").unwrap();
        let noisy = series(|t| 610.0 + (t * 1.3).sin() * 0.4);
        assert!(first_trigger(s, &noisy, -40_000, 90_000).is_none());
    }

    #[test]
    fn a_threshold_breach_opens_with_its_value() {
        let db = Lattice::embedded().unwrap();
        let s = db.sensor("10HAH10CT001").unwrap();
        // 초당 0.5 도로는 트립(645)까지 늘 30초보다 멀어서 선행 경보 없이 경보 한계(625)에서 열립니다.
        let rising = series(|t| if t < 0.0 { 610.0 } else { 610.0 + 0.5 * t });
        let found = first_trigger(s, &rising, -40_000, 90_000).unwrap();
        assert_eq!(found.kind, TriggerKind::ThresholdBreach);
        assert_eq!(found.at_ms, 30_000);
        let fast = series(|t| if t < 0.0 { 610.0 } else { 610.0 + 3.0 * t });
        assert_eq!(first_trigger(s, &fast, -40_000, 90_000).unwrap().kind, TriggerKind::HighSlopePreAlarm, "초당 3 도면 경보 전에 먼저 알립니다");
    }

    #[test]
    fn a_steep_rise_below_the_alarm_opens_a_pre_alarm() {
        // 기획안 3.1절: 경보선 아래라도 트립까지 30초 안이면 먼저 알립니다 (P7 노내 압력).
        let db = Lattice::embedded().unwrap();
        let s = db.sensor("10HAD10CP101").unwrap();
        let furnace = series(|t| if t < 2.0 { -0.1 } else { -0.1 + 0.06 * (t - 2.0) });
        let found = first_trigger(s, &furnace, -40_000, 90_000).unwrap();
        assert_eq!(found.kind, TriggerKind::HighSlopePreAlarm);
        assert!(found.value.unwrap() < s.alarm_high.unwrap());
        assert!(found.ttf_secs.unwrap() <= PRE_ALARM_TTF_SECS);
        assert_eq!(found.at_ms, 5_000);
    }

    #[test]
    fn the_seal_changes_when_any_frozen_sample_changes() {
        let mut t = Telemetry::default();
        t.series.insert("A".into(), series(|x| x));
        let snap = SdsSnapshot {
            sds_id: "SDS-20261104-0001".into(),
            trigger_kind: "THRESHOLD_BREACH".into(),
            gate_state_vector: vec!["G1_SIGNAL_VALID".into()],
            decomposed_confidences: vec![("시간 선후".into(), 0.988)],
            min_confidence: Some(0.988),
            center_ms: 0,
        };
        let frozen = freeze(&t, &["A", "없음"], 0);
        assert_eq!(frozen["A"].len(), 61);
        assert!(!frozen.contains_key("없음"));
        let a = seal(&snap, &frozen, &["확정".into()], Some(222_000));
        assert_eq!(a, seal(&snap, &frozen, &["확정".into()], Some(222_000)));
        let mut tampered = frozen.clone();
        tampered.get_mut("A").unwrap()[30] += 1;
        assert_ne!(a, seal(&snap, &tampered, &["확정".into()], Some(222_000)));
        assert_ne!(a, seal(&snap, &frozen, &["확정".into()], Some(1)));
    }
}
