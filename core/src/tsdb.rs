//! 시계열 저장 · 질의 (기획 Part 8-4 2절). 원장의 불변 원칙(시작 때 한 번 읽고 바꾸지 않음)을 지키려고 둘로 나눕니다.
//!
//! - [`SeedSeries`] (정적 시드, `Lattice` 쪽): 시나리오 · 과거 계측을 `(태그, 시각 ms) → 값 ×1000` 으로 들고 범위 질의를 `O(log N)` 으로 답합니다.
//! - [`LiveRing`] (실시간, `Store` 쪽): DCS(또는 물리엔진)가 보내는 표본을 태그마다 최근 60초만 쌓습니다.
//!   - [`LiveRing::ts_at`]: 현장 게이지 촬영 시각과 같은 시각의 값 (표본 사이 보간)
//!   - [`LiveRing::ts_stats`]: 구간 평균 · 분산(동결 판정) · 기울기 · 최대 · 최소
//!   - [`LiveRing::ts_inflection_lag`]: 상 · 하류 두 센서의 변곡점 시차 `Δt = t_down − t_up`
//!
//! 값은 ×1000 정수로 둡니다 (`RingWindow` 와 같은 규칙: 누적 오차 없이 같은 입력이면 같은 결과).

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;

use crate::lattice::SensorRow;
use crate::timeseries::{ttf, Series, Telemetry, BASELINE_SECS};

/// 실시간 링버퍼가 태그마다 들고 있는 길이
pub const LIVE_WINDOW_MS: i64 = 60_000;

fn to_milli(v: f64) -> i64 {
    (v * 1000.0).round() as i64
}

fn from_milli(v: i64) -> f64 {
    v as f64 / 1000.0
}

fn interpolate(before: Option<(i64, i64)>, after: Option<(i64, i64)>, t_ms: i64) -> Option<f64> {
    match (before, after) {
        (Some((t0, v0)), Some((t1, v1))) if t1 > t0 => {
            let f = (t_ms - t0) as f64 / (t1 - t0) as f64;
            Some(from_milli(v0) + f * (from_milli(v1) - from_milli(v0)))
        }
        (Some((t0, v0)), _) if t0 == t_ms => Some(from_milli(v0)),
        (_, Some((t1, v1))) if t1 == t_ms => Some(from_milli(v1)),
        _ => None,
    }
}

/// 정적 시드 시계열
#[derive(Debug, Clone, Default)]
pub struct SeedSeries {
    map: BTreeMap<(String, i64), i64>,
}

impl SeedSeries {
    pub fn from_telemetry(t: &Telemetry) -> Self {
        let mut s = Self::default();
        for (tag, series) in &t.series {
            for (i, v) in series.values.iter().enumerate() {
                s.insert(tag, series.time_of(i), *v);
            }
        }
        s
    }

    pub fn insert(&mut self, tag: &str, t_ms: i64, value: f64) {
        self.map.insert((tag.to_string(), t_ms), to_milli(value));
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }

    pub fn is_empty(&self) -> bool {
        self.map.is_empty()
    }

    /// `from_ms ..= to_ms` 의 표본 (시각 순)
    pub fn range(&self, tag: &str, from_ms: i64, to_ms: i64) -> Vec<(i64, f64)> {
        if from_ms > to_ms {
            return Vec::new();
        }
        self.map.range((tag.to_string(), from_ms)..=(tag.to_string(), to_ms)).map(|((_, t), v)| (*t, from_milli(*v))).collect()
    }

    /// 그 시각의 값 (앞뒤 표본 사이 보간)
    pub fn at(&self, tag: &str, t_ms: i64) -> Option<f64> {
        let key = (tag.to_string(), t_ms);
        let before = self.map.range(..=key.clone()).next_back().filter(|((g, _), _)| g == tag).map(|((_, t), v)| (*t, *v));
        let after = self.map.range(key..).next().filter(|((g, _), _)| g == tag).map(|((_, t), v)| (*t, *v));
        interpolate(before, after, t_ms)
    }
}

/// 구간 요약
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TsStats {
    pub n: usize,
    pub from_ms: i64,
    pub to_ms: i64,
    pub mean: f64,
    pub variance: f64,
    /// 최소제곱 기울기 (단위/초)
    pub slope_per_sec: f64,
    pub min: f64,
    pub max: f64,
    /// 값이 한 번도 바뀌지 않음 (분산 0, 계측기 동결 의심)
    pub flat: bool,
}

/// 실시간 링버퍼
#[derive(Debug, Clone)]
pub struct LiveRing {
    window_ms: i64,
    buf: BTreeMap<String, VecDeque<(i64, i64)>>,
}

impl Default for LiveRing {
    fn default() -> Self {
        Self::new(LIVE_WINDOW_MS)
    }
}

impl LiveRing {
    pub fn new(window_ms: i64) -> Self {
        Self { window_ms, buf: BTreeMap::new() }
    }

    pub fn clear(&mut self) {
        self.buf.clear();
    }

    /// 표본 하나를 쌓고 창보다 오래된 것은 버립니다. 같은 시각이 다시 오면 값을 바꾸고(재전송), 더 이른 시각은 버립니다.
    pub fn push(&mut self, tag: &str, t_ms: i64, value: f64) {
        let q = self.buf.entry(tag.to_string()).or_default();
        if let Some(last) = q.back_mut() {
            if last.0 == t_ms {
                last.1 = to_milli(value);
                return;
            }
            if last.0 > t_ms {
                return;
            }
        }
        q.push_back((t_ms, to_milli(value)));
        while q.front().is_some_and(|(t, _)| *t < t_ms - self.window_ms) {
            q.pop_front();
        }
    }

    pub fn tags(&self) -> impl Iterator<Item = &str> {
        self.buf.keys().map(String::as_str)
    }

    pub fn len(&self, tag: &str) -> usize {
        self.buf.get(tag).map_or(0, VecDeque::len)
    }

    pub fn latest(&self, tag: &str) -> Option<(i64, f64)> {
        self.buf.get(tag)?.back().map(|(t, v)| (*t, from_milli(*v)))
    }

    /// 그 시각의 값. 버퍼 밖(60초보다 오래됨)이면 None 입니다.
    pub fn ts_at(&self, tag: &str, t_ms: i64) -> Option<f64> {
        let q = self.buf.get(tag)?;
        let i = q.partition_point(|(t, _)| *t <= t_ms);
        let before = i.checked_sub(1).and_then(|k| q.get(k)).copied();
        let after = q.get(i).copied();
        interpolate(before, after, t_ms)
    }

    /// 가장 최근 표본에서 `window_ms` 앞까지의 요약
    pub fn ts_stats(&self, tag: &str, window_ms: i64) -> Option<TsStats> {
        let q = self.buf.get(tag)?;
        let (to_ms, _) = *q.back()?;
        let xs: Vec<(i64, f64)> = q.iter().filter(|(t, _)| *t >= to_ms - window_ms).map(|(t, v)| (*t, from_milli(*v))).collect();
        if xs.len() < 2 {
            return None;
        }
        let n = xs.len() as f64;
        let mean = xs.iter().map(|x| x.1).sum::<f64>() / n;
        let variance = xs.iter().map(|x| (x.1 - mean).powi(2)).sum::<f64>() / n;
        let tm = xs.iter().map(|x| x.0 as f64 / 1000.0).sum::<f64>() / n;
        let sxx = xs.iter().map(|x| (x.0 as f64 / 1000.0 - tm).powi(2)).sum::<f64>();
        let sxy = xs.iter().map(|x| (x.0 as f64 / 1000.0 - tm) * (x.1 - mean)).sum::<f64>();
        let (min, max) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(x.1), hi.max(x.1)));
        Some(TsStats {
            n: xs.len(),
            from_ms: xs[0].0,
            to_ms,
            mean,
            variance,
            slope_per_sec: if sxx > 0.0 { sxy / sxx } else { 0.0 },
            min,
            max,
            flat: max - min < 1e-9,
        })
    }

    /// 지금 값과 기울기로 본 트립(없으면 경보) 한계 도달 예상 시간 (초)
    pub fn ts_ttf(&self, sensor: &SensorRow, window_ms: i64) -> Option<f64> {
        let (_, v) = self.latest(&sensor.tag)?;
        ttf(sensor, v, self.ts_stats(&sensor.tag, window_ms)?.slope_per_sec)
    }

    /// 버퍼를 일정 간격으로 다시 뽑은 계열 (판정 로직은 1초 간격 계열을 씁니다)
    pub fn series(&self, tag: &str, period_ms: i64) -> Option<Series> {
        let q = self.buf.get(tag)?;
        let (t0, _) = *q.front()?;
        let (t1, _) = *q.back()?;
        let start = t0 + (period_ms - t0.rem_euclid(period_ms)) % period_ms;
        let values: Vec<f64> = (0..).map(|k| start + k * period_ms).take_while(|t| *t <= t1).filter_map(|t| self.ts_at(tag, t)).collect();
        (values.len() >= 2).then(|| Series::new(start, period_ms, values))
    }

    /// 변곡점(기준 구간 20초를 벗어난 첫 시각). 버퍼에 기준 구간이 없으면 None 입니다.
    pub fn ts_departure(&self, sensor: &SensorRow) -> Option<f64> {
        let s = self.series(&sensor.tag, 1000)?;
        let from = s.t0_ms + BASELINE_SECS * 1000;
        if from >= s.end_ms() {
            return None;
        }
        s.departure(sensor.deadband, from, s.end_ms()).map(|d| d.at_ms)
    }

    /// 상 · 하류 변곡점 시차 (초). 둘 다 벗어나야 값이 있습니다.
    pub fn ts_inflection_lag(&self, up: &SensorRow, down: &SensorRow) -> Option<f64> {
        Some((self.ts_departure(down)? - self.ts_departure(up)?) / 1000.0)
    }

    /// 지금 버퍼를 1초 간격 계측으로 얼립니다 (SDS 스냅샷 · 사건 열기)
    pub fn snapshot(&self) -> Telemetry {
        let mut t = Telemetry::default();
        for tag in self.buf.keys() {
            if let Some(s) = self.series(tag, 1000) {
                t.series.insert(tag.clone(), s);
            }
        }
        t
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lattice::Lattice;
    use crate::scenario::ScenarioSet;

    fn p2() -> (Lattice, Telemetry, i64) {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P2").unwrap();
        (db, s.telemetry(&set), s.time(0.0).timestamp_millis())
    }

    #[test]
    fn seed_range_queries_answer_from_the_btree() {
        let (_, t, t0) = p2();
        let seed = SeedSeries::from_telemetry(&t);
        assert_eq!(seed.len(), t.series.values().map(|s| s.len()).sum::<usize>());
        let r = seed.range("10LAB20CG102", t0 - 2000, t0 + 2000);
        assert_eq!(r.len(), 5);
        assert!(r.windows(2).all(|w| w[0].0 < w[1].0));
        // 표본 사이 시각은 보간합니다
        let mid = seed.at("10LAB20CG102", t0 + 500).unwrap();
        assert!((mid - t.value_at("10LAB20CG102", t0 + 500).unwrap()).abs() < 1e-3, "{mid}");
        assert!(seed.at("없는태그", t0).is_none());
        assert!(seed.range("10LAB20CG102", t0, t0 - 1).is_empty());
    }

    #[test]
    fn the_live_ring_keeps_sixty_seconds_and_answers_point_queries() {
        let mut ring = LiveRing::default();
        for k in 0..=70 {
            ring.push("X", k * 1000, k as f64);
        }
        assert_eq!(ring.len("X"), 61, "60초 창 (양 끝 포함)");
        assert_eq!(ring.latest("X"), Some((70_000, 70.0)));
        assert_eq!(ring.ts_at("X", 30_500), Some(30.5));
        assert_eq!(ring.ts_at("X", 5_000), None, "창 밖은 답하지 않습니다");
        // 재전송은 값을 바꾸고, 더 이른 시각은 버립니다
        ring.push("X", 70_000, 71.0);
        ring.push("X", 40_000, -1.0);
        assert_eq!(ring.latest("X"), Some((70_000, 71.0)));
        let st = ring.ts_stats("X", 10_000).unwrap();
        assert_eq!(st.n, 11);
        assert!((st.slope_per_sec - 1.0).abs() < 0.1, "{st:?}");
        assert!(!st.flat);
    }

    #[test]
    fn a_frozen_transmitter_has_zero_variance() {
        let mut ring = LiveRing::default();
        for k in 0..40 {
            ring.push("F", k * 1000, 56.0);
        }
        let st = ring.ts_stats("F", 30_000).unwrap();
        assert!(st.flat && st.variance == 0.0);
    }

    #[test]
    fn inflection_lag_between_upstream_valve_and_superheater_matches_p2() {
        let (db, t, t0) = p2();
        let mut ring = LiveRing::default();
        // DCS 가 1초마다 보내는 값을 +20초까지 쌓습니다 (창은 -40 ~ +20 초)
        for tag in ["10LAB20CG102", "10HAH10CT001"] {
            let s = t.get(tag).unwrap();
            for (i, v) in s.values.iter().enumerate() {
                if s.time_of(i) <= t0 + 20_000 {
                    ring.push(tag, s.time_of(i), *v);
                }
            }
        }
        let lag = ring.ts_inflection_lag(db.sensor("10LAB20CG102").unwrap(), db.sensor("10HAH10CT001").unwrap()).unwrap();
        assert!((3.5..5.5).contains(&lag), "밸브 → 과열기 Δt {lag}");
        let snap = ring.snapshot();
        assert_eq!(snap.series.len(), 2);
        assert_eq!(snap.get("10HAH10CT001").unwrap().period_ms, 1000);
        // 과열기는 오르는 중이라 트립(645 ℃) 도달 예상 시간이 있습니다
        assert!(ring.ts_ttf(db.sensor("10HAH10CT001").unwrap(), 5_000).is_some());
    }
}
