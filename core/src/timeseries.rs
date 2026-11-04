//! 시계열 로직 (기획 Part 8-3). 단일 시점의 값보다 변화율 · 변곡 시각 · 전파 시차가 고장 원인을 가릅니다.
//!
//! - [`Series`]: 1초 간격 표본. 시각은 epoch 밀리초입니다. 현장 스캔 시각처럼 표본 사이의 시각은 선형 보간으로 읽습니다 (시점 동기화).
//! - [`Series::departure`]: 기준 구간을 벗어난 첫 시각 (변곡점). 데드밴드와 기준 구간 잡음의 4σ 중 큰 폭을 넘어야 합니다.
//! - [`Series::crossing`]: 한계값을 처음 넘은 시각 (표본 사이 보간)
//! - [`Series::slope_at`]: 최근 표본의 최소제곱 기울기 (단위/초)
//! - [`Series::is_flat`]: 일정 시간 동안 잡음조차 없는 값 고정 (계측기 동결)
//! - [`Series::spikes`]: 물리적으로 불가능한 순간 변화가 다음 표본에서 돌아오면 전자기 잡음입니다
//! - [`RingWindow`]: 기획안의 고정 크기 링버퍼. 값은 ×1000 정수로 보관하고 기울기 · 가속도 · 지수이동평균을 정수로 냅니다.
//! - [`classify`]: 센서 신호 모양 (SDS 상태 코드 `STABLE` · `SLEW_RISE/DROP` · `STUCK_HIGH/LOW` · `FLATLINE` · `NOISY`)과 고장 코드 ([`fault_code`], 예 `VALVE_STUCK_LOW`)
//! - [`Series::cusum`]: 작은 변화도 오래 이어지면 잡는 양쪽 CUSUM 과 변화 시작 추정 (4σ 띠와 같은 오경보 수준에서 1.5σ 변화를 더 빨리)
//! - [`ttf_quadratic`] · [`ttf_conservative`]: 가속 중이면 2차식이 더 이른 트립 도달 시각 (창이 변화 시작 뒤에 놓일 때만)
//! - [`onset_lag`] · [`xcorr_lag`]: 변화 시작 시차, 1차 차분 교차상관 시차 (변곡 시차의 확인용)

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

/// TTF 가 겨누는 한계: 오르면 트립 상한(없으면 경보 상한), 내리면 트립 하한(없으면 경보 하한)
fn ttf_limit(sensor: &SensorRow, rising: bool) -> Option<f64> {
    if rising {
        sensor.trip_high.or(sensor.alarm_high)
    } else {
        sensor.trip_low.or(sensor.alarm_low)
    }
}

/// 지금 기울기로 트립 한계(없으면 경보 한계)에 닿을 때까지 남은 초. 한계 쪽으로 움직이지 않으면 None 입니다.
pub fn ttf(sensor: &SensorRow, value: f64, slope: f64) -> Option<f64> {
    // 데드밴드를 10초에 걸쳐 넘는 정도보다 느리면 추세로 보지 않습니다.
    if slope.abs() < sensor.deadband / 10.0 {
        return None;
    }
    let limit = ttf_limit(sensor, slope > 0.0)?;
    if slope > 0.0 {
        (value < limit).then(|| (limit - value) / slope)
    } else {
        (value > limit).then(|| (value - limit) / -slope)
    }
}

/// 2차 추세를 맞출 때 쓰는 최근 표본 수
pub const QUAD_SAMPLES: usize = 10;

/// 최근 표본에 `v(t) = a + b·t + c·t²` 를 맞춥니다 (`t = 0` 이 마지막 표본, 초 단위). 표본이 다섯보다 적으면 None 입니다.
fn quad_fit(xs: &[f64], dt: f64) -> Option<(f64, f64, f64)> {
    let m = xs.len();
    if m < 5 {
        return None;
    }
    // 정규방정식 (3 × 3)
    let mut s = [0.0f64; 5];
    let mut r = [0.0f64; 3];
    for (i, x) in xs.iter().enumerate() {
        let t = (i as f64 - (m - 1) as f64) * dt;
        let mut p = 1.0;
        for k in 0..5 {
            s[k] += p;
            if k < 3 {
                r[k] += p * x;
            }
            p *= t;
        }
    }
    let a = [[s[0], s[1], s[2]], [s[1], s[2], s[3]], [s[2], s[3], s[4]]];
    let det = |m: [[f64; 3]; 3]| m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1]) - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0]) + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);
    let d = det(a);
    if d.abs() < 1e-12 {
        return None;
    }
    let col = |k: usize| {
        let mut m = a;
        for (row, v) in m.iter_mut().zip(r) {
            row[k] = v;
        }
        det(m) / d
    };
    Some((col(0), col(1), col(2)))
}

/// 2차(가속) TTF: 최근 [`QUAD_SAMPLES`] 표본의 기울기 · 곡률로 한계에 닿는 첫 시각을 풉니다.
/// 한계 쪽으로 움직이며 한계 쪽으로 가속할 때만 답합니다 (감속 중이면 1차식이 더 이르므로 None).
///
/// - 창이 변화 시작(CUSUM) 뒤에 온전히 놓일 때만 씁니다. 평탄에서 기울기로 꺾이는 자리를 창에 넣으면 꺾임을 가속으로 잘못 읽습니다.
/// - 곡률이 창 길이에 걸쳐 데드밴드의 절반도 못 만들면 잡음으로 보고 None 입니다.
pub fn ttf_quadratic(sensor: &SensorRow, series: &Series, at_ms: i64) -> Option<f64> {
    let xs = series.window(at_ms, QUAD_SAMPLES);
    if xs.len() < QUAD_SAMPLES {
        return None;
    }
    let start_ms = at_ms - (xs.len() as i64 - 1) * series.period_ms;
    let onset = series.cusum(sensor.deadband, series.t0_ms + BASELINE_SECS * 1000, at_ms)?.onset_ms;
    if onset > start_ms {
        return None;
    }
    let dt = series.period_ms as f64 / 1000.0;
    let (_, b, c) = quad_fit(xs, dt)?;
    let value = *xs.last()?;
    let span = (xs.len() - 1) as f64 * dt;
    if (c * span * span).abs() < sensor.deadband / 2.0 {
        return None;
    }
    let rising = b > 0.0;
    let limit = ttf_limit(sensor, rising)?;
    // 한계 쪽 거리를 양수로 두고 풉니다: (|c|) τ² + (|b|) τ − 거리 = 0
    let (gap, b, c) = if rising { (limit - value, b, c) } else { (value - limit, -b, -c) };
    if gap <= 0.0 || b < sensor.deadband / 10.0 || c <= 0.0 {
        return None;
    }
    Some((-b + (b * b + 4.0 * c * gap).sqrt()) / (2.0 * c))
}

/// 보수적 TTF: 1차(지금 기울기)와 2차(가속) 중 이른 것. 가속 중인 상승은 1차식이 늦게 잡습니다.
pub fn ttf_conservative(sensor: &SensorRow, series: &Series, at_ms: i64) -> Option<f64> {
    let value = series.value_at(at_ms)?;
    let linear = series.slope_at(at_ms, SLOPE_SAMPLES).and_then(|s| ttf(sensor, value, s));
    match (linear, ttf_quadratic(sensor, series, at_ms)) {
        (Some(a), Some(b)) => Some(a.min(b)),
        (a, b) => a.or(b),
    }
}

/// CUSUM 의 무시 폭 k 와 경보 문턱 h (표준화 단위). h = 12 면 잡음만 있는 120초의 오경보가 4σ 띠와 같은 수준입니다
/// (둘 다 기준 구간 21표본으로 평균 · 표준편차를 추정하는 오차 때문에 약 9 %). 시험 `cusum_catches_small_shifts_...` 참조.
pub const CUSUM_K: f64 = 0.5;
pub const CUSUM_H: f64 = 12.0;

/// 양쪽 CUSUM 경보
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct CusumAlarm {
    /// 누적합이 문턱을 넘은 표본 시각
    pub at_ms: i64,
    /// 변화 시작 추정: 경보 직전, 무시 폭 1σ 누적합이 마지막으로 0 이던 표본의 다음 표본 시각
    pub onset_ms: i64,
    pub direction: Direction,
}

impl Series {
    /// 양쪽 CUSUM (Page): 기준 구간(`from_ms` 앞 20초) 평균 · 표준편차로 표준화한 잔차를 쌓아, 작은 변화도 오래 이어지면 잡습니다.
    /// 표준편차는 데드밴드의 1/4 보다 작게 두지 않습니다 (4σ 띠 = 데드밴드와 같은 기준).
    pub fn cusum(&self, deadband: f64, from_ms: i64, until_ms: i64) -> Option<CusumAlarm> {
        self.cusum_with(deadband, from_ms, until_ms, CUSUM_K, CUSUM_H)
    }

    pub fn cusum_with(&self, deadband: f64, from_ms: i64, until_ms: i64, k: f64, h: f64) -> Option<CusumAlarm> {
        let base = self.baseline(from_ms, BASELINE_SECS)?;
        self.cusum_from(base, deadband, from_ms, until_ms, k, h)
    }

    /// 기준을 밖에서 받는 CUSUM (`from_ms` 다음 표본부터 쌓습니다)
    pub fn cusum_from(&self, base: Baseline, deadband: f64, from_ms: i64, until_ms: i64, k: f64, h: f64) -> Option<CusumAlarm> {
        let sigma = base.std.max(deadband / 4.0).max(1e-12);
        let start = self.index_at(from_ms)? + 1;
        let end = self.index_at(until_ms)?;
        let (mut hi, mut lo) = (0.0f64, 0.0f64);
        // 변화 시작은 무시 폭을 1σ 로 키운 누적합이 마지막으로 0 이던 다음 표본입니다. 경보와 같은 k 로 잡으면
        // 변화 전 잡음에 누적합이 0 위에 머물러 시작이 몇 초 앞당겨집니다 (밸브 계단이 주입 2 ~ 3초 전으로 잡힌 사례).
        let k_onset = k.max(1.0);
        let (mut hi_on, mut lo_on) = (0.0f64, 0.0f64);
        let (mut hi_zero, mut lo_zero) = (start, start);
        for i in start..=end {
            let z = (self.values[i] - base.mean) / sigma;
            hi = (hi + z - k).max(0.0);
            lo = (lo - z - k).max(0.0);
            hi_on = (hi_on + z - k_onset).max(0.0);
            lo_on = (lo_on - z - k_onset).max(0.0);
            if hi_on == 0.0 {
                hi_zero = i + 1;
            }
            if lo_on == 0.0 {
                lo_zero = i + 1;
            }
            if hi > h || lo > h {
                let (zero, direction) = if hi > h { (hi_zero, Direction::Up) } else { (lo_zero, Direction::Down) };
                return Some(CusumAlarm { at_ms: self.time_of(i), onset_ms: self.time_of(zero.min(i)), direction });
            }
        }
        None
    }
}

/// 센서 하나의 지금 모양 (SDS 상태 코드). 고장 코드는 설비 종류와 붙여 씁니다 (`VALVE_STUCK_LOW`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum SignalState {
    Stable,
    /// 기준을 벗어나며 오르는 중
    SlewRise,
    /// 기준을 벗어나며 내리는 중
    SlewDrop,
    /// 기준보다 높은 값에 멈춤
    StuckHigh,
    /// 기준보다 낮은 값에 멈춤 (밸브 고착 · 막힘)
    StuckLow,
    /// 정상 근처에서 잡음조차 없이 값 고정 (계측기 동결)
    Flatline,
    /// 추세 없이 흔들림이 커지거나 순간 튐
    Noisy,
}

impl SignalState {
    pub fn code(self) -> &'static str {
        match self {
            SignalState::Stable => "STABLE",
            SignalState::SlewRise => "SLEW_RISE",
            SignalState::SlewDrop => "SLEW_DROP",
            SignalState::StuckHigh => "STUCK_HIGH",
            SignalState::StuckLow => "STUCK_LOW",
            SignalState::Flatline => "FLATLINE",
            SignalState::Noisy => "NOISY",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            SignalState::Stable => "안정",
            SignalState::SlewRise => "상승 중",
            SignalState::SlewDrop => "하강 중",
            SignalState::StuckHigh => "높은 값에 멈춤",
            SignalState::StuckLow => "낮은 값에 멈춤 (고착)",
            SignalState::Flatline => "값 동결 (계측기)",
            SignalState::Noisy => "잡음 · 순간 튐",
        }
    }
}

/// 설비 종류와 신호 모양을 붙인 고장 코드 (원장 인터록의 `fault` 와 같은 꼴, 예: `VALVE_STUCK_LOW`)
pub fn fault_code(kind: &str, state: SignalState) -> String {
    format!("{}_{}", kind.to_uppercase(), state.code())
}

/// [`classify`] 결과
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SignalReading {
    pub state: SignalState,
    pub value: f64,
    /// 기준 구간(처음 20초) 평균과 판정 폭 (데드밴드 · 4σ 중 큰 값)
    pub baseline: f64,
    pub band: f64,
    /// 최근 5표본 기울기 (단위/초)
    pub slope: f64,
    /// 최근 창에서 직선 추세를 뺀 잔차의 표준편차
    pub jitter: f64,
    /// CUSUM 이 본 변화 시작 (경보가 났을 때)
    pub onset_ms: Option<i64>,
}

/// 두 센서의 변화 시작(CUSUM) 시각 차이 (초, 하류가 늦으면 양수). 기준은 각 계열을 밖에서 받습니다.
/// 문턱(데드밴드 · 4σ)을 넘는 순간으로 재는 변곡 시차는 느린 변화일수록 늦게 잡히는데, 변화 시작은 그 치우침이 작습니다.
/// 계단 · 일정 기울기 어느 응답에도 맞아 교차상관보다 넓게 씁니다.
pub fn onset_lag(up: (&SensorRow, &Series, Baseline), down: (&SensorRow, &Series, Baseline), from_ms: i64, until_ms: i64) -> Option<f64> {
    let onset = |(s, x, b): (&SensorRow, &Series, Baseline)| x.cusum_from(b, s.deadband, from_ms, until_ms, CUSUM_K, CUSUM_H).map(|c| c.onset_ms);
    Some((onset(down)? - onset(up)?) as f64 / 1000.0)
}

/// 모양을 볼 최근 창 (초)
pub const STATE_WINDOW_SECS: i64 = 8;

fn detrended_std(xs: &[f64], dt: f64) -> f64 {
    let n = xs.len() as f64;
    if xs.len() < 3 {
        return 0.0;
    }
    let slope = least_squares(xs, (dt * 1000.0) as i64).unwrap_or(0.0);
    let mean = xs.iter().sum::<f64>() / n;
    let mid = (n - 1.0) / 2.0;
    (xs.iter().enumerate().map(|(i, x)| (x - mean - slope * (i as f64 - mid) * dt).powi(2)).sum::<f64>() / n).sqrt()
}

/// `at_ms` 의 신호 모양. 기준은 시계열 처음 20초입니다 (변곡점과 같은 기준). 기준 구간이 끝나기 전이면 None 입니다.
///
/// 차례: 값 고정(동결 · 고착) → 순간 튐 → 추세(기울기 + 기준 이탈 또는 CUSUM) → 벗어난 채 멈춤(고착) → 흔들림 → 안정
pub fn classify(sensor: &SensorRow, series: &Series, at_ms: i64) -> Option<SignalReading> {
    let base_end = series.t0_ms + BASELINE_SECS * 1000;
    if at_ms <= base_end {
        return None;
    }
    let base = series.baseline(base_end, BASELINE_SECS)?;
    classify_with(sensor, series, at_ms, base, base_end)
}

/// 기준(평균 · 표준편차)을 밖에서 받아 판정합니다. 실시간 링버퍼처럼 창이 미끄러지는 계열은 처음 정상 구간의 기준을 붙잡아 두고 씁니다.
/// CUSUM 은 `from_ms` 다음 표본부터 쌓습니다.
pub fn classify_with(sensor: &SensorRow, series: &Series, at_ms: i64, base: Baseline, from_ms: i64) -> Option<SignalReading> {
    let value = series.value_at(at_ms)?;
    let band = sensor.deadband.max(base.std * 4.0);
    let sigma = base.std.max(sensor.deadband / 4.0);
    let dt = series.period_ms as f64 / 1000.0;
    let n = (STATE_WINDOW_SECS * 1000 / series.period_ms) as usize + 1;
    let win = series.window(at_ms, n);
    let slope = series.slope_at(at_ms, SLOPE_SAMPLES).unwrap_or(0.0);
    let jitter = detrended_std(win, dt);
    let dev = value - base.mean;
    let cusum = series.cusum_from(base, sensor.deadband, from_ms, at_ms, CUSUM_K, CUSUM_H);
    let away = dev.abs() > band;
    let side = |hi, lo| if dev > 0.0 { hi } else { lo };
    let state = if win.len() >= n && series.is_flat(at_ms, STATE_WINDOW_SECS) {
        if away {
            side(SignalState::StuckHigh, SignalState::StuckLow)
        } else {
            SignalState::Flatline
        }
    } else if !series.spikes(sensor.max_slew, at_ms - STATE_WINDOW_SECS * 1000, at_ms).is_empty() {
        SignalState::Noisy
    } else if slope.abs() >= sensor.deadband / 10.0 && slope.abs() * STATE_WINDOW_SECS as f64 > 3.0 * sigma && (away || cusum.is_some()) {
        if slope > 0.0 {
            SignalState::SlewRise
        } else {
            SignalState::SlewDrop
        }
    } else if away {
        side(SignalState::StuckHigh, SignalState::StuckLow)
    } else if jitter > 3.0 * sigma {
        SignalState::Noisy
    } else {
        SignalState::Stable
    };
    Some(SignalReading { state, value, baseline: base.mean, band, slope, jitter, onset_ms: cusum.map(|c| c.onset_ms) })
}

/// 두 계열의 1차 차분 교차상관이 가장 큰 시차 (초, 하류가 늦으면 양수)와 그 상관의 절댓값.
/// 하류가 상류 변화를 지연 뒤 같은 모양(비례)으로 따라갈 때 맞습니다. 하류가 변화를 적분하는 경우
/// (유량 계단 → 온도 일정 기울기)는 차분이 판 모양이라 꼭짓점이 정해지지 않으니 [`onset_lag`] 를 씁니다.
/// 두 계열을 `from_ms` ~ `until_ms` 에서 위쪽 계열의 간격으로 다시 뽑고, 상관은 전체 에너지로 나눕니다 (겹친 구간만으로 나누면 짧은 겹침이 유리해집니다).
pub fn xcorr_lag(up: &Series, down: &Series, from_ms: i64, until_ms: i64, max_lag_secs: f64) -> Option<(f64, f64)> {
    let p = up.period_ms;
    let grid: Vec<i64> = (0..).map(|i| from_ms + i * p).take_while(|t| *t <= until_ms).collect();
    let resample = |s: &Series| grid.iter().map(|t| s.value_at(*t)).collect::<Option<Vec<f64>>>();
    let (a, b) = (resample(up)?, resample(down)?);
    let diff = |xs: &[f64]| xs.windows(2).map(|w| w[1] - w[0]).collect::<Vec<f64>>();
    let (da, db) = (diff(&a), diff(&b));
    let max_lag = (max_lag_secs * 1000.0 / p as f64).round() as isize;
    let energy = (da.iter().map(|x| x * x).sum::<f64>() * db.iter().map(|y| y * y).sum::<f64>()).sqrt();
    if energy <= 0.0 {
        return None;
    }
    let corr = |lag: isize| -> f64 {
        let mut sab = 0.0;
        for i in 0..da.len() as isize {
            let j = i + lag;
            if (0..db.len() as isize).contains(&j) {
                sab += da[i as usize] * db[j as usize];
            }
        }
        (sab / energy).abs()
    };
    let scores: Vec<(isize, f64)> = (-max_lag..=max_lag).map(|l| (l, corr(l))).collect();
    let (best, peak) = scores.iter().copied().max_by(|x, y| x.1.total_cmp(&y.1))?;
    if peak <= 0.0 {
        return None;
    }
    // 이웃 두 점으로 꼭짓점을 보간합니다 (표본 간격보다 고운 시차).
    let at = |l: isize| scores.iter().find(|s| s.0 == l).map(|s| s.1);
    let shift = match (at(best - 1), at(best + 1)) {
        (Some(l), Some(r)) if l + r - 2.0 * peak < 0.0 => (0.5 * (l - r) / (l + r - 2.0 * peak)).clamp(-0.5, 0.5),
        _ => 0.0,
    };
    Some(((best as f64 + shift) * p as f64 / 1000.0, peak))
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

    /// 정해진 의사 잡음 (진폭 a, 시각마다 다름)
    fn wiggle(t: f64, a: f64) -> f64 {
        a * ((t * 1.7).sin() * 0.6 + (t * 0.37 + 1.0).sin() * 0.4)
    }

    fn valve() -> SensorRow {
        SensorRow { tag: "10LAB20CG102".into(), kks: "10LAB20AA102".into(), label: "밸브 개도".into(), unit: "%".into(), normal: 78.0, alarm_low: Some(45.0), alarm_high: None, trip_low: None, trip_high: None, deadband: 0.5, max_slew: 30.0 }
    }

    #[test]
    fn signal_states_follow_the_shape_of_the_recent_window() {
        let s = sensor();
        let state = |f: &dyn Fn(f64) -> f64, at: i64| classify(&s, &series(f), at * 1000).unwrap().state;
        assert_eq!(state(&|t| 610.0 + wiggle(t, 0.1), 30), SignalState::Stable);
        assert_eq!(state(&|t| 610.0 + wiggle(t, 0.1) + if t > 0.0 { 3.6 * t } else { 0.0 }, 5), SignalState::SlewRise);
        assert_eq!(state(&|t| if t < -10.0 { 610.0 + wiggle(t, 0.1) } else { 610.0 }, 30), SignalState::Flatline, "정상 근처 값 고정은 계측기 동결");
        let spike = |t: f64| 610.0 + wiggle(t, 0.1) + if t == 28.0 { 40.0 } else { 0.0 };
        assert_eq!(state(&spike, 30), SignalState::Noisy);
        assert_eq!(state(&spike, 40), SignalState::Stable, "튐이 창을 벗어나면 다시 안정");
        // 밸브 고착: 78 % → 18 % 로 떨어지는 동안은 하강, 그 자리에 머물면 낮은 값에 멈춤
        let v = valve();
        let stuck = series(|t| if t < 0.0 { 78.0 } else { 18.0 } + wiggle(t, 0.05));
        assert_eq!(classify(&v, &stuck, 2_000).unwrap().state, SignalState::SlewDrop);
        let r = classify(&v, &stuck, 12_000).unwrap();
        assert_eq!(r.state, SignalState::StuckLow);
        assert!(r.onset_ms.is_some_and(|t| (0..=1_000).contains(&t)), "CUSUM 변화 시작 {:?}", r.onset_ms);
        assert_eq!(fault_code("valve", r.state), "VALVE_STUCK_LOW", "원장 인터록의 고장 코드와 같은 꼴");
        assert!(classify(&v, &stuck, -50_000).is_none(), "기준 구간 안에서는 판정하지 않습니다");
    }

    /// xorshift64* + Box–Muller (시험용 정규 잡음, 같은 시드면 같은 값)
    struct Rng(u64);

    impl Rng {
        fn uniform(&mut self) -> f64 {
            self.0 ^= self.0 >> 12;
            self.0 ^= self.0 << 25;
            self.0 ^= self.0 >> 27;
            (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
        }

        fn normal(&mut self) -> f64 {
            let (u, v) = (self.uniform().max(1e-12), self.uniform());
            (-2.0 * u.ln()).sqrt() * (2.0 * std::f64::consts::PI * v).cos()
        }
    }

    /// (탐지 비율, 평균 지연 초) — 변화 전 경보는 탐지가 아님
    fn detection(alarms: &[Option<f64>], change_ms: f64) -> (f64, f64) {
        let ok: Vec<f64> = alarms.iter().flatten().filter(|t| **t >= change_ms - 1000.0).map(|t| (t - change_ms).max(0.0) / 1000.0).collect();
        (ok.len() as f64 / alarms.len() as f64, ok.iter().sum::<f64>() / ok.len().max(1) as f64)
    }

    #[test]
    fn cusum_catches_small_shifts_the_4_sigma_band_misses_at_a_similar_false_alarm_rate() {
        // 기준 20초 + 감시 120초, 잡음 σ = 1. 변화는 감시 30초 뒤 계단.
        const N: usize = 400;
        let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
        let mut run = |shift: f64| {
            let (mut band, mut cusum) = (Vec::new(), Vec::new());
            for _ in 0..N {
                let values: Vec<f64> = (0..141).map(|i| rng.normal() + if i >= 50 { shift } else { 0.0 }).collect();
                let s = Series::new(0, 1000, values);
                band.push(s.departure(1e-6, 20_000, 140_000).map(|d| d.at_ms));
                cusum.push(s.cusum(1e-6, 20_000, 140_000).map(|c| c.at_ms as f64));
            }
            (band, cusum)
        };
        let (band0, cusum0) = run(0.0);
        let fp = |xs: &[Option<f64>]| xs.iter().filter(|x| x.is_some()).count() as f64 / N as f64;
        let (fp_band, fp_cusum) = (fp(&band0), fp(&cusum0));
        assert!(fp_band < 0.15 && fp_cusum < 0.15, "잡음만 있을 때 오경보: 띠 {fp_band:.3} · CUSUM {fp_cusum:.3}");
        assert!(fp_cusum <= fp_band + 0.04, "CUSUM 오경보가 띠보다 눈에 띄게 많지 않아야 합니다: {fp_cusum:.3} vs {fp_band:.3}");
        let (band, cusum) = run(1.5);
        let ((rb, db), (rc, dc)) = (detection(&band, 50_000.0), detection(&cusum, 50_000.0));
        assert!(rc >= 0.9 && rb <= 0.7, "1.5σ 계단 탐지: CUSUM {rc:.2} · 띠 {rb:.2}");
        assert!(dc * 2.0 < db, "1.5σ 계단 평균 지연: CUSUM {dc:.1}s · 띠 {db:.1}s");
        let (band, cusum) = run(3.0);
        let ((rb, db), (rc, dc)) = (detection(&band, 50_000.0), detection(&cusum, 50_000.0));
        assert!(rc >= 0.9 && rb >= 0.9, "3σ 계단은 둘 다 잡습니다: CUSUM {rc:.2} · 띠 {rb:.2}");
        assert!(dc < db, "3σ 계단도 CUSUM 이 빠릅니다: {dc:.1}s vs {db:.1}s");
    }

    #[test]
    fn the_quadratic_ttf_is_earlier_while_accelerating_and_silent_while_decelerating() {
        let s = sensor();
        // 가속: 610 + 0.1·t² (t ≥ 0). t = 12 초에 624.4 °C, 645 °C 까지 실제로 6.71 초
        let accel = series(|t| if t > 0.0 { 610.0 + 0.1 * t * t } else { 610.0 });
        let at = 12_000;
        let linear = ttf(&s, accel.value_at(at).unwrap(), accel.slope_at(at, SLOPE_SAMPLES).unwrap()).unwrap();
        let quad = ttf_quadratic(&s, &accel, at).unwrap();
        assert!((quad - 6.71).abs() < 0.05, "2차 TTF {quad:.2}");
        assert!((linear - 10.3).abs() < 0.1, "1차 TTF {linear:.2} (지금 기울기로는 늦게 잡음)");
        assert_eq!(ttf_conservative(&s, &accel, at), Some(quad));
        // 감속: 1차 응답으로 포화 → 2차 TTF 는 답하지 않고 보수적 TTF 는 1차식 그대로
        let decel = series(|t| if t > 0.0 { 610.0 + 40.0 * (1.0 - (-t / 10.0).exp()) } else { 610.0 });
        assert_eq!(ttf_quadratic(&s, &decel, at), None);
        let v = decel.value_at(at).unwrap();
        assert_eq!(ttf_conservative(&s, &decel, at), ttf(&s, v, decel.slope_at(at, SLOPE_SAMPLES).unwrap()));
        // 잡음뿐이면 곡률을 믿지 않습니다.
        assert_eq!(ttf_quadratic(&s, &series(|t| 610.0 + wiggle(t, 0.1)), at), None);
    }

    #[test]
    fn cross_correlation_recovers_the_propagation_delay() {
        // 상류 밸브가 t = 10 초에 떨어지고, 하류가 4 초 늦게 같은 모양으로 변함 (순수 운송 지연)
        let up = series(|t| if t < 10.0 { 78.0 } else { 18.0 } + wiggle(t, 0.05));
        let delayed = series(|t| if t < 14.0 { 610.0 } else { 638.0 } + wiggle(t, 0.05));
        let (lag, rho) = xcorr_lag(&up, &delayed, -20_000, 60_000, 12.0).unwrap();
        assert!((lag - 4.0).abs() < 0.3, "교차상관 시차 {lag:.2}s");
        assert!(rho > 0.9, "상관 {rho:.2}");
        // 하류가 4 초 지연 뒤 1차 응답(τ = 3 초)이면 변화의 무게중심을 맞추므로 지연과 지연 + τ 사이로 나옵니다.
        let lagged = series(|t| 610.0 + wiggle(t, 0.05) + if t >= 14.0 { 28.0 * (1.0 - (-(t - 14.0) / 3.0).exp()) } else { 0.0 });
        let (lag, rho) = xcorr_lag(&up, &lagged, -20_000, 60_000, 12.0).unwrap();
        assert!((4.0..=7.0).contains(&lag), "1차 응답 시차 {lag:.2}s");
        assert!(rho > 0.5, "상관 {rho:.2}");
        let down = lagged;
        // 방향이 거꾸로면 음수
        let (back, _) = xcorr_lag(&down, &up, -20_000, 60_000, 12.0).unwrap();
        assert!(back < 0.0, "{back}");
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
