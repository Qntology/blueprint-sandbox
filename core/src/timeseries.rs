//! 시계열 로직 (기획 Part 8-3). 단일 시점의 값보다 변화율 · 변곡 시각 · 전파 시차가 고장 원인을 가릅니다.
//!
//! - [`Series`]: 1초 간격 표본. 시각은 epoch 밀리초입니다. 현장 스캔 시각처럼 표본 사이의 시각은 선형 보간으로 읽습니다 (시점 동기화).
//! - [`Series::departure`]: 기준 구간을 벗어난 첫 시각 (변곡점). 데드밴드와 기준 구간 잡음의 4σ 중 큰 폭을 넘어야 합니다.
//! - [`Series::crossing`]: 한계값을 처음 넘은 시각 (표본 사이 보간)
//! - [`Series::slope_at`]: 최근 표본의 최소제곱 기울기 (단위/초)
//! - [`Series::is_flat`]: 일정 시간 동안 잡음조차 없는 값 고정 (계측기 동결)
//! - [`Series::spikes`]: 물리적으로 불가능한 순간 변화가 다음 표본에서 돌아오면 전자기 잡음입니다
//! - [`RingWindow`]: 기획안의 고정 크기 링버퍼. 값은 ×1000 정수로 보관하고 기울기 · 가속도 · 지수이동평균을 정수로 냅니다.

use serde::{Deserialize, Serialize};

use crate::lattice::SensorRow;
use crate::plant::Direction;

/// 시계열 값을 정수로 보관할 때 곱하는 수 (기획안의 `window_milli`)
pub const MILLI: f64 = 1000.0;
/// 기준 구간 길이 (초)
pub const BASELINE_SECS: i64 = 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Series {
    /// 첫 표본의 시각 (epoch 밀리초)
    pub t0_ms: i64,
    pub period_ms: i64,
    pub values: Vec<f64>,
}

/// 기준 구간의 평균과 표준편차
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Baseline {
    pub mean: f64,
    pub std: f64,
}

/// 기준 구간을 벗어난 첫 순간
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Departure {
    /// 보간한 시각 (epoch 밀리초, 소수 포함)
    pub at_ms: f64,
    pub direction: Direction,
    pub baseline: f64,
    /// 판정에 쓴 폭 (데드밴드와 4σ 중 큰 값)
    pub band: f64,
}

/// 순간 잡음 하나
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Spike {
    pub index: usize,
    pub at_ms: i64,
    pub value: f64,
    /// 직전 표본에서 튄 초당 변화량
    pub slew: f64,
}

impl Series {
    pub fn new(t0_ms: i64, period_ms: i64, values: Vec<f64>) -> Self {
        Self { t0_ms, period_ms: period_ms.max(1), values }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn time_of(&self, i: usize) -> i64 {
        self.t0_ms + i as i64 * self.period_ms
    }

    pub fn end_ms(&self) -> i64 {
        self.time_of(self.len().saturating_sub(1))
    }

    /// `t_ms` 이하인 마지막 표본의 번호
    pub fn index_at(&self, t_ms: i64) -> Option<usize> {
        if self.is_empty() || t_ms < self.t0_ms {
            return None;
        }
        Some((((t_ms - self.t0_ms) / self.period_ms) as usize).min(self.len() - 1))
    }

    /// 시각 `t_ms` 의 값. 표본 사이는 선형 보간이고, 시계열 밖이면 None 입니다.
    pub fn value_at(&self, t_ms: i64) -> Option<f64> {
        if self.is_empty() || t_ms < self.t0_ms || t_ms > self.end_ms() {
            return None;
        }
        let pos = (t_ms - self.t0_ms) as f64 / self.period_ms as f64;
        let i = pos.floor() as usize;
        let frac = pos - i as f64;
        let a = self.values[i];
        let b = *self.values.get(i + 1).unwrap_or(&a);
        Some(a + (b - a) * frac)
    }

    /// `until_ms` 까지의 마지막 `n` 표본 (오래된 것부터)
    pub fn window(&self, until_ms: i64, n: usize) -> &[f64] {
        match self.index_at(until_ms) {
            Some(end) => &self.values[(end + 1).saturating_sub(n)..=end],
            None => &[],
        }
    }

    /// `before_ms` 앞 `secs` 초의 평균 · 표준편차. 표본이 셋보다 적으면 None 입니다.
    pub fn baseline(&self, before_ms: i64, secs: i64) -> Option<Baseline> {
        let from = self.index_at(before_ms - secs * 1000).unwrap_or(0);
        let to = self.index_at(before_ms)?;
        let xs = &self.values[from..=to];
        if xs.len() < 3 {
            return None;
        }
        let mean = xs.iter().sum::<f64>() / xs.len() as f64;
        let var = xs.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / xs.len() as f64;
        Some(Baseline { mean, std: var.sqrt() })
    }

    /// `from_ms` ~ `until_ms` 에서 기준 구간(`from_ms` 앞 20초)을 벗어난 첫 시각
    pub fn departure(&self, deadband: f64, from_ms: i64, until_ms: i64) -> Option<Departure> {
        let base = self.baseline(from_ms, BASELINE_SECS)?;
        let band = deadband.max(base.std * 4.0);
        let start = self.index_at(from_ms)?.max(1);
        let end = self.index_at(until_ms)?;
        for i in start..=end {
            let x = self.values[i];
            if (x - base.mean).abs() <= band {
                continue;
            }
            let up = x > base.mean;
            let level = if up { base.mean + band } else { base.mean - band };
            let prev = self.values[i - 1];
            let frac = if (x - prev).abs() > f64::EPSILON { ((level - prev) / (x - prev)).clamp(0.0, 1.0) } else { 1.0 };
            let at_ms = self.time_of(i - 1) as f64 + frac * self.period_ms as f64;
            return Some(Departure { at_ms, direction: if up { Direction::Up } else { Direction::Down }, baseline: base.mean, band });
        }
        None
    }

    /// 한계값을 처음 넘은 시각. `upward` 면 아래에서 위로, 아니면 위에서 아래로 넘는 순간입니다.
    pub fn crossing(&self, limit: f64, upward: bool, from_ms: i64, until_ms: i64) -> Option<f64> {
        let start = self.index_at(from_ms)?.max(1);
        let end = self.index_at(until_ms)?;
        for i in start..=end {
            let (a, b) = (self.values[i - 1], self.values[i]);
            let crossed = if upward { a < limit && b >= limit } else { a > limit && b <= limit };
            if crossed {
                let frac = ((limit - a) / (b - a)).clamp(0.0, 1.0);
                return Some(self.time_of(i - 1) as f64 + frac * self.period_ms as f64);
            }
        }
        None
    }

    /// `at_ms` 까지의 최근 `n` 표본으로 구한 최소제곱 기울기 (단위/초)
    pub fn slope_at(&self, at_ms: i64, n: usize) -> Option<f64> {
        least_squares(self.window(at_ms, n), self.period_ms)
    }

    /// `at_ms` 앞 `secs` 초 동안 값이 한 번도 바뀌지 않았는가 (분산 0, 계측기 동결)
    pub fn is_flat(&self, at_ms: i64, secs: i64) -> bool {
        let n = (secs * 1000 / self.period_ms) as usize + 1;
        let xs = self.window(at_ms, n);
        if xs.len() < n {
            return false;
        }
        let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        hi - lo < 1e-9
    }

    /// 순간 잡음: 직전 표본에서 `max_slew`(초당) 보다 빨리 튀었다가 다음 표본에서 직전 값 근처로 돌아온 표본
    pub fn spikes(&self, max_slew: f64, from_ms: i64, until_ms: i64) -> Vec<Spike> {
        let (Some(start), Some(end)) = (self.index_at(from_ms), self.index_at(until_ms)) else { return Vec::new() };
        let dt = self.period_ms as f64 / 1000.0;
        let mut out = Vec::new();
        for i in start.max(1)..end.min(self.len().saturating_sub(2)) + 1 {
            let (prev, x) = (self.values[i - 1], self.values[i]);
            let Some(&next) = self.values.get(i + 1) else { break };
            let jump = (x - prev).abs() / dt;
            let back = (next - prev).abs() / (2.0 * dt);
            if jump > max_slew && back <= max_slew && (x - next).abs() / dt > max_slew {
                out.push(Spike { index: i, at_ms: self.time_of(i), value: x, slew: jump });
            }
        }
        out
    }

    /// 잡음 표본을 앞뒤 표본의 평균으로 바꾼 시계열
    pub fn despiked(&self, spikes: &[Spike]) -> Series {
        let mut values = self.values.clone();
        for s in spikes {
            if s.index > 0 && s.index + 1 < values.len() {
                values[s.index] = (self.values[s.index - 1] + self.values[s.index + 1]) / 2.0;
            }
        }
        Series { values, ..self.clone() }
    }
}

fn least_squares(xs: &[f64], period_ms: i64) -> Option<f64> {
    if xs.len() < 2 {
        return None;
    }
    let n = xs.len() as f64;
    let dt = period_ms as f64 / 1000.0;
    let t_mean = (n - 1.0) / 2.0 * dt;
    let x_mean = xs.iter().sum::<f64>() / n;
    let (mut num, mut den) = (0.0, 0.0);
    for (i, x) in xs.iter().enumerate() {
        let t = i as f64 * dt - t_mean;
        num += t * (x - x_mean);
        den += t * t;
    }
    (den > 0.0).then(|| num / den)
}

/// 지금 기울기로 트립 한계(없으면 경보 한계)에 닿을 때까지 남은 초. 한계 쪽으로 움직이지 않으면 None 입니다.
pub fn ttf(sensor: &SensorRow, value: f64, slope: f64) -> Option<f64> {
    // 데드밴드를 10초에 걸쳐 넘는 정도보다 느리면 추세로 보지 않습니다.
    if slope.abs() < sensor.deadband / 10.0 {
        return None;
    }
    if slope > 0.0 {
        let limit = sensor.trip_high.or(sensor.alarm_high)?;
        (value < limit).then(|| (limit - value) / slope)
    } else {
        let limit = sensor.trip_low.or(sensor.alarm_low)?;
        (value > limit).then(|| (value - limit) / -slope)
    }
}

/// 고정 크기 링버퍼 (기획안 `RingWindow<T, 60>`). 값은 ×1000 정수입니다.
#[derive(Debug, Clone)]
pub struct RingWindow<const N: usize> {
    buf: [i64; N],
    len: usize,
    head: usize,
}

impl<const N: usize> Default for RingWindow<N> {
    fn default() -> Self {
        Self { buf: [0; N], len: 0, head: 0 }
    }
}

impl<const N: usize> RingWindow<N> {
    pub fn push(&mut self, value: f64) {
        self.buf[self.head] = (value * MILLI).round() as i64;
        self.head = (self.head + 1) % N;
        self.len = (self.len + 1).min(N);
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn is_full(&self) -> bool {
        self.len == N
    }

    /// 오래된 것부터
    pub fn values(&self) -> Vec<i64> {
        let start = (self.head + N - self.len) % N;
        (0..self.len).map(|k| self.buf[(start + k) % N]).collect()
    }

    /// 지수이동평균 (`alpha` 는 0 ~ 1)
    pub fn ewma(&self, alpha: f64) -> Option<i64> {
        let mut it = self.values().into_iter();
        let first = it.next()? as f64;
        Some(it.fold(first, |acc, x| acc + alpha * (x as f64 - acc)).round() as i64)
    }

    /// 마지막 `k` 표본의 기울기 (초당, ×1000 정수). 1Hz 표본입니다.
    pub fn slope_milli(&self, k: usize) -> Option<i64> {
        let v = self.values();
        let xs: Vec<f64> = v[v.len().saturating_sub(k)..].iter().map(|x| *x as f64).collect();
        least_squares(&xs, 1000).map(|s| s.round() as i64)
    }

    /// 가속도: 마지막 `k` 표본의 기울기 - 그 앞 `k` 표본의 기울기 (초당², ×1000 정수)
    pub fn accel_milli(&self, k: usize) -> Option<i64> {
        let v = self.values();
        if v.len() < 2 * k {
            return None;
        }
        let recent: Vec<f64> = v[v.len() - k..].iter().map(|x| *x as f64).collect();
        let before: Vec<f64> = v[v.len() - 2 * k..v.len() - k].iter().map(|x| *x as f64).collect();
        let (a, b) = (least_squares(&recent, 1000)?, least_squares(&before, 1000)?);
        Some(((a - b) / k as f64).round() as i64)
    }
}

/// 센서 하나의 60초 시계열 요약 (기획안 `TimeSeriesMetrics`)
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeSeriesMetrics {
    pub sensor_tag: String,
    /// 변곡점 최초 발생 시각 (epoch ms). 기준 구간을 벗어난 적이 없으면 None
    pub inflection_at_ms: Option<i64>,
    /// 최근 60초 표본 (×1000 정수)
    pub window_milli: Vec<i64>,
    /// 초당 변화율 (×1000)
    pub slope_per_sec_milli: i64,
    /// 변화 가속도 (×1000)
    pub accel_milli: i64,
    /// 지수이동평균 (×1000)
    pub ewma_milli: i64,
    /// 지금 추세로 트립 한계에 닿을 때까지 남은 초
    pub estimated_ttf_secs: Option<u32>,
}

/// 사건 하나의 계측 묶음: 태그별 DCS 대표값과, 다중 채널 센서의 채널별 값
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Telemetry {
    pub series: std::collections::BTreeMap<String, Series>,
    /// 태그 → 채널 이름(`A`, `B`, `C`) → 값
    pub channels: std::collections::BTreeMap<String, std::collections::BTreeMap<String, Series>>,
}

impl Telemetry {
    pub fn get(&self, tag: &str) -> Option<&Series> {
        self.series.get(tag)
    }

    pub fn value_at(&self, tag: &str, t_ms: i64) -> Option<f64> {
        self.get(tag)?.value_at(t_ms)
    }

    /// 잡음을 걸러 낸 시계열로 바꾼 사본
    pub fn with_series(&self, tag: &str, series: Series) -> Telemetry {
        let mut out = self.clone();
        out.series.insert(tag.to_string(), series);
        out
    }

    /// 시계열이 시작하는 시각 (가장 이른 것)
    pub fn start_ms(&self) -> Option<i64> {
        self.series.values().map(|s| s.t0_ms).min()
    }
}

/// 기울기를 잴 때 쓰는 최근 표본 수
pub const SLOPE_SAMPLES: usize = 5;

/// `at_ms` 시각의 60초 요약. 변곡점은 시계열 처음 20초를 기준 구간으로 찾습니다.
pub fn metrics(sensor: &SensorRow, series: &Series, at_ms: i64) -> TimeSeriesMetrics {
    let mut ring = RingWindow::<60>::default();
    for v in series.window(at_ms, 60) {
        ring.push(*v);
    }
    let base_end = series.t0_ms + BASELINE_SECS * 1000;
    let inflection = series.departure(sensor.deadband, base_end, at_ms).map(|d| d.at_ms.round() as i64);
    let slope = series.slope_at(at_ms, SLOPE_SAMPLES).unwrap_or(0.0);
    let value = series.value_at(at_ms).unwrap_or(sensor.normal);
    TimeSeriesMetrics {
        sensor_tag: sensor.tag.clone(),
        inflection_at_ms: inflection,
        window_milli: ring.values(),
        slope_per_sec_milli: (slope * MILLI).round() as i64,
        accel_milli: ring.accel_milli(SLOPE_SAMPLES).unwrap_or(0),
        ewma_milli: ring.ewma(0.3).unwrap_or((value * MILLI) as i64),
        estimated_ttf_secs: ttf(sensor, value, slope).map(|s| s.round().max(0.0) as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sensor() -> SensorRow {
        SensorRow {
            tag: "10HAH10CT001".into(),
            kks: "10HAH10BB001".into(),
            label: "과열기 출구 증기 온도".into(),
            unit: "°C".into(),
            normal: 610.0,
            alarm_low: None,
            alarm_high: Some(625.0),
            trip_low: None,
            trip_high: Some(645.0),
            deadband: 1.0,
            max_slew: 20.0,
        }
    }

    /// t = -60 초부터 1초 간격
    fn series(f: impl Fn(f64) -> f64) -> Series {
        Series::new(-60_000, 1000, (0..151).map(|i| f((i as i64 - 60) as f64)).collect())
    }

    #[test]
    fn values_between_samples_are_interpolated_for_time_alignment() {
        let s = series(|t| t);
        assert_eq!(s.value_at(5_500), Some(5.5));
        assert_eq!(s.value_at(-60_000), Some(-60.0));
        assert_eq!(s.value_at(90_000), Some(90.0));
        assert_eq!(s.value_at(90_001), None);
        assert_eq!(s.window(0, 3), &[-2.0, -1.0, 0.0]);
    }

    #[test]
    fn a_step_departs_between_the_two_samples() {
        let s = series(|t| if t < 1.0 { 78.0 } else { 18.0 });
        let d = s.departure(0.5, -20_000, 30_000).unwrap();
        assert_eq!(d.direction, Direction::Down);
        assert!((0.0..60.0).contains(&d.at_ms), "{}", d.at_ms);
        assert_eq!(d.baseline, 78.0);
        assert!(s.departure(0.5, -20_000, 0).is_none(), "계단 전에는 벗어나지 않습니다");
    }

    #[test]
    fn crossings_and_slopes_follow_the_ramp() {
        // t = 2 초부터 초당 3.6 도 오름
        let s = series(|t| if t < 2.0 { 610.0 } else { 610.0 + 3.6 * (t - 2.0) });
        let at = s.crossing(625.0, true, 0, 30_000).unwrap();
        assert!((at - (2.0 + 15.0 / 3.6) * 1000.0).abs() < 1.0, "{at}");
        assert!((s.slope_at(10_000, 5).unwrap() - 3.6).abs() < 1e-9);
        let remain = ttf(&sensor(), s.value_at(10_000).unwrap(), 3.6).unwrap();
        assert!((remain - (645.0 - 638.8) / 3.6).abs() < 1e-9);
        assert!(ttf(&sensor(), 610.0, 0.01).is_none(), "느린 흔들림은 추세가 아닙니다");
        assert!(ttf(&sensor(), 650.0, 3.0).is_none(), "이미 넘었으면 남은 시간이 없습니다");
    }

    #[test]
    fn a_frozen_value_is_flat_and_noise_is_not() {
        let frozen = series(|t| if t < -40.0 { 45.0 + (t * 0.7).sin() * 0.05 } else { 56.0 });
        assert!(frozen.is_flat(0, 30));
        assert!(!frozen.is_flat(-30_000, 30), "동결 10초 만으로는 30초 구간이 평평하지 않습니다");
        let noisy = series(|t| 45.0 + (t * 0.7).sin() * 0.05);
        assert!(!noisy.is_flat(0, 30));
    }

    #[test]
    fn a_one_sample_jump_that_returns_is_a_spike_but_a_ramp_is_not() {
        let spiky = series(|t| if t == 0.0 { 810.0 } else { 610.0 });
        let found = spiky.spikes(20.0, -10_000, 10_000);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].at_ms, 0);
        assert_eq!(spiky.despiked(&found).value_at(0), Some(610.0));
        let ramp = series(|t| if t < 0.0 { 610.0 } else { 610.0 + 5.0 * t });
        assert!(ramp.spikes(20.0, -10_000, 10_000).is_empty());
        let step = series(|t| if t < 0.0 { 45.0 } else { 56.0 });
        assert!(step.spikes(5.0, -10_000, 10_000).is_empty(), "돌아오지 않는 계단은 잡음이 아닙니다");
    }

    #[test]
    fn the_ring_window_keeps_the_last_n_as_integers() {
        let mut ring = RingWindow::<4>::default();
        for v in [1.0, 2.0, 3.5, 4.0, 5.25] {
            ring.push(v);
        }
        assert!(ring.is_full());
        assert_eq!(ring.values(), vec![2000, 3500, 4000, 5250]);
        let mut linear = RingWindow::<60>::default();
        for i in 0..60 {
            linear.push(i as f64 * 0.5);
        }
        assert_eq!(linear.slope_milli(5), Some(500));
        assert_eq!(linear.accel_milli(5), Some(0));
    }

    #[test]
    fn metrics_report_the_inflection_slope_and_ttf() {
        let s = series(|t| if t < 2.0 { 610.0 } else { (610.0 + 3.6 * (t - 2.0)).min(638.0) });
        let m = metrics(&sensor(), &s, 6_000);
        assert_eq!(m.window_milli.len(), 60);
        assert_eq!(*m.window_milli.last().unwrap(), 624_400);
        assert!((m.inflection_at_ms.unwrap() - 2_278).abs() <= 1, "{:?}", m.inflection_at_ms);
        assert_eq!(m.slope_per_sec_milli, 3600);
        assert_eq!(m.estimated_ttf_secs, Some(6));
    }
}
