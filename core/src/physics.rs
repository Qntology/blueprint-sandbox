//! 경량 발전소 물리엔진과 SDS 상태 추적 (기획 Part 8-5 3 · 4절).
//!
//! 급수 계통 한 줄(주급수펌프 → 주 조절밸브 · 바이패스 → 수냉벽 → 과열기)을 집중 정수 모델로 100 ms 틱마다 계산합니다.
//! 고장을 주입하면 각본이 아니라 식으로 연쇄가 생깁니다: 밸브가 고착되면 유량이 줄고, 막힌 물 때문에 펌프 토출압이 오르고,
//! 배관 이류 지연 τ 뒤에 과열기 냉각이 모자라 증기 온도가 오릅니다. 바이패스를 열면 같은 식으로 유량 · 온도가 돌아옵니다.
//!
//! - 유량: `Q* = Qd · min(1.05, (f(u) + k_b · f(b)) / f(u0))`, `f(u) = u^0.35` (빠른 열림 특성), 1차 지연 0.6 s 로 따라감
//! - 펌프 토출압: `P = P0 + 0.9 · (1 − Q/Qd) / 0.4` (유량 40 % 감소에 +0.9 MPa)
//! - 이류 지연: 과열기는 `Q(t − τ)`, τ = 4.2 s
//! - 과열기 열평형: `M · dT/dt = Qd · (T0 − Tin) − Q(t − τ) · (T − Tin)` (정상 610 ℃, 유량 60 % 에서 초기 기울기 약 0.9 ℃/s)
//!
//! 나머지 센서는 정상치에 작은 잡음을 얹고, 센서 바이어스 고장(전송기 드리프트)은 읽은 값에만 더합니다.
//! 이 엔진의 제어 명령은 시뮬레이터 안에서만 효과가 있습니다. 실제 설비는 조작하지 않습니다.

use std::collections::{BTreeMap, VecDeque};

use serde::Serialize;

use crate::lattice::Lattice;
use crate::timeseries::{classify_with, fault_code, onset_lag, Baseline, SignalState, BASELINE_SECS};
use crate::tsdb::LiveRing;

pub const TICK_MS: i64 = 100;

const QD: f64 = 1850.0;
const U0: f64 = 0.78;
const KB: f64 = 0.42;
const P0: f64 = 28.5;
const TAU_Q_S: f64 = 0.6;
/// 펌프 토출압은 유량 변화를 1초 늦게 따라갑니다 (원인 → 결과 순서)
const TAU_P_S: f64 = 1.0;
const ADVECTION_MS: i64 = 4200;
const T0: f64 = 610.0;
const TIN: f64 = 300.0;
const M: f64 = 255_922.0;
/// 바이패스 밸브 행정 속도 (초당 개도)
const BYPASS_STROKE: f64 = 0.20;

pub const VALVE: &str = "10LAB20AA102";
pub const BYPASS: &str = "10LAB20AA103";
const TAG_VALVE: &str = "10LAB20CG102";
const TAG_BYPASS: &str = "10LAB20CG103";
const TAG_FLOW: &str = "10LAB20CF001";
const TAG_PUMP: &str = "10LAB10CP001";
const TAG_SH: &str = "10HAH10CT001";

fn flow_factor(u: f64) -> f64 {
    u.max(0.0).powf(0.35)
}

/// 고장 주입 (Fault Injector)
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Fault {
    /// 밸브 포지셔너 고착 (개도 %)
    ValveStuck { open_pct: f64 },
    /// 전송기 영점 드리프트 (읽은 값에만 더함, 실제 공정은 정상)
    SensorBias { tag: String, bias: f64 },
}

/// 제어 명령. 시뮬레이터에만 들어갑니다.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Command {
    /// 급수 바이패스 밸브 개도 명령 (%)
    SetBypass { open_pct: f64 },
}

/// 급수 계통 시뮬레이터
#[derive(Debug, Clone)]
pub struct PlantSim {
    t_ms: i64,
    u_main: f64,
    stuck: Option<f64>,
    bypass: f64,
    bypass_cmd: f64,
    q: f64,
    p_pump: f64,
    history: VecDeque<(i64, f64)>,
    t_sh: f64,
    bias: BTreeMap<String, f64>,
    normals: Vec<(String, f64, f64)>,
    rng: u64,
    pub faults: Vec<Fault>,
    pub commands: Vec<(i64, Command)>,
}

impl PlantSim {
    /// 정상 운전 상태에서 시작합니다. `start_ms` 는 시뮬레이션 시계의 첫 시각입니다.
    pub fn new(db: &Lattice, start_ms: i64, seed: u64) -> Self {
        let normals = db.sensors().iter().map(|s| (s.tag.clone(), s.normal, s.deadband * 0.12)).collect();
        Self {
            t_ms: start_ms,
            u_main: U0,
            stuck: None,
            bypass: 0.0,
            bypass_cmd: 0.0,
            q: QD,
            p_pump: P0,
            history: VecDeque::from([(start_ms, QD)]),
            t_sh: T0,
            bias: BTreeMap::new(),
            normals,
            rng: seed.max(1),
            faults: Vec::new(),
            commands: Vec::new(),
        }
    }

    pub fn now_ms(&self) -> i64 {
        self.t_ms
    }

    pub fn inject(&mut self, fault: Fault) {
        match &fault {
            Fault::ValveStuck { open_pct } => {
                self.stuck = Some(open_pct / 100.0);
            }
            Fault::SensorBias { tag, bias } => {
                *self.bias.entry(tag.clone()).or_default() += bias;
            }
        }
        self.faults.push(fault);
    }

    pub fn command(&mut self, cmd: Command) {
        match &cmd {
            Command::SetBypass { open_pct } => self.bypass_cmd = (open_pct / 100.0).clamp(0.0, 1.0),
        }
        self.commands.push((self.t_ms, cmd));
    }

    /// 실제 공정값 (센서 잡음 · 바이어스 없이)
    pub fn true_value(&self, tag: &str) -> Option<f64> {
        Some(match tag {
            TAG_VALVE => self.u_main * 100.0,
            TAG_BYPASS => self.bypass * 100.0,
            TAG_FLOW => self.q,
            TAG_PUMP => self.p_pump,
            TAG_SH => self.t_sh,
            _ => self.normals.iter().find(|n| n.0 == tag)?.1,
        })
    }

    fn noise(&mut self, sigma: f64) -> f64 {
        // xorshift64 → 근사 정규(균등 넷 합). 같은 시드면 같은 잡음입니다.
        let mut sum = 0.0;
        for _ in 0..4 {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            sum += (self.rng >> 11) as f64 / (1u64 << 53) as f64;
        }
        (sum - 2.0) * sigma * 1.732
    }

    /// 100 ms 한 틱. 센서가 읽은 값(태그, 값)을 돌려줍니다.
    pub fn tick(&mut self) -> Vec<(String, f64)> {
        let dt = TICK_MS as f64 / 1000.0;
        self.t_ms += TICK_MS;
        // 밸브: 고착이면 그 개도, 아니면 정상 개도. 바이패스는 행정 속도로 명령을 따라갑니다.
        self.u_main = self.stuck.unwrap_or(U0);
        let step = BYPASS_STROKE * dt;
        self.bypass += (self.bypass_cmd - self.bypass).clamp(-step, step);
        // 유량: 목표로 1차 지연
        let target = QD * ((flow_factor(self.u_main) + KB * flow_factor(self.bypass)) / flow_factor(U0)).min(1.05);
        self.q += (target - self.q) * (dt / TAU_Q_S).min(1.0);
        let p_target = P0 + 0.9 * (1.0 - self.q / QD) / 0.4;
        self.p_pump += (p_target - self.p_pump) * (dt / TAU_P_S).min(1.0);
        self.history.push_back((self.t_ms, self.q));
        while self.history.len() > 2 && self.history[1].0 <= self.t_ms - ADVECTION_MS {
            self.history.pop_front();
        }
        // 과열기: τ 전 유량으로 냉각
        let q_late = self.history.iter().take_while(|(t, _)| *t <= self.t_ms - ADVECTION_MS).last().map(|x| x.1).unwrap_or(QD);
        self.t_sh += (QD * (T0 - TIN) - q_late * (self.t_sh - TIN)) / M * dt;

        let tags: Vec<(String, f64)> = self.normals.iter().map(|(t, _, s)| (t.clone(), *s)).collect();
        tags.into_iter()
            .map(|(tag, sigma)| {
                let v = self.true_value(&tag).unwrap_or(0.0) + self.noise(sigma) + self.bias.get(&tag).copied().unwrap_or(0.0);
                (tag, (v * 1000.0).round() / 1000.0)
            })
            .collect()
    }

    /// 틱을 돌리며 링버퍼에 쌓습니다.
    pub fn run_into(&mut self, ring: &mut LiveRing, ticks: usize) {
        for _ in 0..ticks {
            let t = self.t_ms + TICK_MS;
            for (tag, v) in self.tick() {
                ring.push(&tag, t, v);
            }
        }
    }
}

/// SDS 속성 상태 (Part 8-5 4절)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TrackState {
    Stable,
    /// 변곡점 포착 (기준 구간을 벗어남)
    InflectionDetected,
    /// 상 · 하류 변곡이 간선 시차 범위로 이어짐
    CausalChainLocked,
    /// 완화 조치가 들어감
    Mitigating,
    /// 기울기 부호가 바뀌어 정상으로 돌아오는 중
    Recovering,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrackEvent {
    pub at_ms: i64,
    pub tag: String,
    pub from: TrackState,
    pub to: TrackState,
    pub note: String,
}

/// 센서 신호 모양이 바뀐 기록 (SDS 상태 코드: `STABLE → SLEW_DROP → STUCK_LOW`)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SignalEvent {
    pub at_ms: i64,
    pub tag: String,
    pub from: SignalState,
    pub to: SignalState,
    pub note: String,
}

/// 센서마다 SDS 상태를 따라가며, 하류 경보가 상류 변곡과 시차로 이어지면 사건을 열 때를 알려 줍니다.
#[derive(Debug, Clone, Default)]
pub struct Tracker {
    states: BTreeMap<String, (TrackState, Option<f64>)>,
    pub events: Vec<TrackEvent>,
    /// 사건을 연 경보 태그
    pub opened: Option<String>,
    /// 처음 변곡을 본 뒤 하류 경보를 기다리는 한도 (초). 급수 → 과열기 시차 창(최대 12초)과 선행 경보까지의 시간을 덮습니다.
    pub wait_secs: f64,
    /// 센서별 기준: 처음 20초 정상 구간에서 붙잡아 둡니다 (링버퍼 창이 미끄러져도 기준은 그대로)
    baselines: BTreeMap<String, Baseline>,
    signals: BTreeMap<String, SignalState>,
    /// 센서 신호 모양의 바뀐 기록
    pub signal_events: Vec<SignalEvent>,
    /// 회복 판정 히스테리시스: 정상 범위에 들어온 시각
    calm_since: BTreeMap<String, i64>,
    /// 회복 중에서 안정으로 넘기 전에 정상 범위에 머물러야 하는 시간 (초). 경계에서 오락가락하지 않게 합니다.
    pub calm_secs: i64,
}

/// 사건을 열 차례
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct OpenCall {
    pub alarm_tag: String,
    pub upstream_tag: Option<String>,
    pub lag_secs: Option<f64>,
    pub trigger: String,
    pub value: f64,
    pub at_ms: i64,
    /// 원인 설비 종류 + 원인 센서 신호 모양 (예: `VALVE_STUCK_LOW`, 원장 인터록의 `fault` 와 같은 꼴)
    pub fault: Option<String>,
    /// 변화 시작(CUSUM) 시각 차이 (변곡 시차의 확인용, 초)
    pub onset_lag_secs: Option<f64>,
}

impl Tracker {
    pub fn new() -> Self {
        Self { wait_secs: 25.0, calm_secs: 3, ..Self::default() }
    }

    pub fn state(&self, tag: &str) -> TrackState {
        self.states.get(tag).map_or(TrackState::Stable, |s| s.0)
    }

    /// 센서의 지금 신호 모양 (아직 판정 전이면 안정)
    pub fn signal(&self, tag: &str) -> SignalState {
        self.signals.get(tag).copied().unwrap_or(SignalState::Stable)
    }

    /// 센서마다 신호 모양을 다시 판정하고 바뀐 것을 기록합니다.
    fn update_signals(&mut self, db: &Lattice, ring: &LiveRing, now: i64) {
        for s in db.sensors() {
            let Some(series) = ring.series(&s.tag, 1000) else { continue };
            if !self.baselines.contains_key(&s.tag) {
                let end = series.t0_ms + BASELINE_SECS * 1000;
                if series.end_ms() < end {
                    continue;
                }
                let Some(b) = series.baseline(end, BASELINE_SECS) else { continue };
                self.baselines.insert(s.tag.clone(), b);
            }
            let base = self.baselines[&s.tag];
            let Some(r) = classify_with(s, &series, now, base, series.t0_ms) else { continue };
            let from = self.signal(&s.tag);
            if r.state != from {
                self.signals.insert(s.tag.clone(), r.state);
                let note = format!("{} {:.2} {} · 기울기 {:+.2}/s", s.label, r.value, s.unit, r.slope);
                self.signal_events.push(SignalEvent { at_ms: now, tag: s.tag.clone(), from, to: r.state, note });
            }
        }
    }

    fn set(&mut self, tag: &str, to: TrackState, at_ms: i64, note: String) {
        let from = self.state(tag);
        if from == to {
            return;
        }
        let dep = self.states.get(tag).and_then(|s| s.1);
        self.states.insert(tag.to_string(), (to, dep));
        self.events.push(TrackEvent { at_ms, tag: tag.to_string(), from, to, note });
    }

    /// 완화 조치가 들어간 태그를 표시합니다.
    pub fn mitigating(&mut self, tags: &[&str], at_ms: i64, note: &str) {
        for t in tags {
            self.set(t, TrackState::Mitigating, at_ms, note.to_string());
        }
    }

    /// 1초마다 링버퍼를 봅니다. 사건을 열 때면 `OpenCall` 을 돌려줍니다 (한 번만).
    pub fn observe(&mut self, db: &Lattice, ring: &LiveRing) -> Option<OpenCall> {
        let latest = ring.tags().filter_map(|t| ring.latest(t)).map(|x| x.0).max()?;
        // 판정은 1초 간격으로 다시 뽑은 계열로 하고, 그 계열은 정각 초에서 끝납니다. 기준 시각도 정각 초로 내립니다.
        let now = latest - latest.rem_euclid(1000);
        self.update_signals(db, ring, now);
        // 변곡 포착
        for s in db.sensors() {
            if self.state(&s.tag) == TrackState::Stable {
                if let Some(at) = ring.ts_departure(s) {
                    self.states.insert(s.tag.clone(), (TrackState::Stable, Some(at)));
                    let slope = ring.ts_stats(&s.tag, 5_000).map(|x| x.slope_per_sec).unwrap_or(0.0);
                    self.set(&s.tag, TrackState::InflectionDetected, at.round() as i64, format!("{} 기울기 {:+.2}/s", s.label, slope));
                }
            }
            // 회복: 완화 중인데 기울기가 정상 쪽으로 돌아섬
            if matches!(self.state(&s.tag), TrackState::Mitigating | TrackState::CausalChainLocked) {
                if let (Some(st), Some((_, v))) = (ring.ts_stats(&s.tag, 5_000), ring.latest(&s.tag)) {
                    let toward_normal = (v - s.normal).signum() * st.slope_per_sec < 0.0 && st.slope_per_sec.abs() > s.deadband / 20.0;
                    if toward_normal && self.events.iter().any(|e| e.to == TrackState::Mitigating) {
                        self.set(&s.tag, TrackState::Recovering, now, format!("기울기 {:+.2}/s · 정상 쪽으로", st.slope_per_sec));
                    }
                }
            }
            // 회복 → 안정: 정상 범위에 calm_secs 동안 머물러야 넘깁니다 (히스테리시스)
            if self.state(&s.tag) == TrackState::Recovering {
                if let Some((_, v)) = ring.latest(&s.tag) {
                    if (v - s.normal).abs() <= s.deadband * 4.0 {
                        let since = *self.calm_since.entry(s.tag.clone()).or_insert(now);
                        if now - since >= self.calm_secs * 1000 {
                            self.calm_since.remove(&s.tag);
                            self.set(&s.tag, TrackState::Stable, now, format!("정상 범위 {}초 유지", self.calm_secs));
                        }
                    } else {
                        self.calm_since.remove(&s.tag);
                    }
                }
            }
        }
        if self.opened.is_some() {
            return None;
        }
        // 경보 · 기울기 선행 경보가 난 센서
        let triggered: Vec<(String, crate::sds::SdsTrigger)> = db
            .sensors()
            .iter()
            .filter(|s| s.alarm_low.is_some() || s.alarm_high.is_some())
            .filter_map(|s| Some((s.tag.clone(), crate::sds::trigger_at(s, &ring.series(&s.tag, 1000)?, now)?)))
            .collect();
        if triggered.is_empty() {
            return None;
        }
        // 경보 난 센서마다 분석 엔진과 같은 인과 규칙(시차 창 · 물리 방향 · 역류 여파 분리)으로 상류 원인을 찾습니다.
        // 원인이 잡히면 그 경보로 사건을 엽니다 (인과 사슬 잠금). 막힌 밸브 탓에 오른 펌프 압력 같은 역류 여파는 원인이 아닙니다.
        let snapshot = ring.snapshot();
        for (tag, trig) in &triggered {
            let report = crate::causal::trace_root_cause(db, &snapshot, tag, now);
            // 원인은 결과보다 먼저 변해야 합니다 (같은 순간이면 같은 고장의 두 얼굴이지 인과가 아닙니다).
            let Some(root) = report.root.filter(|r| r.lag_secs > 0.0) else { continue };
            // 고장 코드: 원인 설비 종류 + 원인 센서의 지금 모양. 변화 시작(CUSUM) 시차는 변곡 시차의 확인용입니다.
            let fault = db.sensor(&root.tag).and_then(|x| db.equipment(&x.kks)).map(|e| fault_code(&e.kind, self.signal(&root.tag)));
            let part = |t: &str| Some((db.sensor(t)?, snapshot.get(t)?, *self.baselines.get(t)?));
            let onset = match (part(&root.tag), part(tag)) {
                (Some(a), Some(b)) => onset_lag(a, b, a.1.t0_ms.max(b.1.t0_ms), now),
                _ => None,
            };
            let check = onset.map(|x| format!(" · 변화 시작 시차 {x:.1}s")).unwrap_or_default();
            let code = fault.clone().map(|f| format!(" · {f}")).unwrap_or_default();
            self.set(&root.tag, TrackState::CausalChainLocked, now, format!("→ {tag} Δt {:.1}s (허용 {:.1} ~ {:.1}s, {}홉){check}{code}", root.lag_secs, root.lag_range.0, root.lag_range.1, root.hops));
            self.set(tag, TrackState::CausalChainLocked, now, format!("← {} Δt {:.1}s", root.tag, root.lag_secs));
            self.opened = Some(tag.clone());
            return Some(OpenCall { alarm_tag: tag.clone(), upstream_tag: Some(root.tag.clone()), lag_secs: Some(root.lag_secs), trigger: trig.kind.code().to_string(), value: trig.value.unwrap_or_default(), at_ms: now, fault, onset_lag_secs: onset });
        }
        // 이어지는 하류가 기다려도 없으면 처음 경보 난 센서로 엽니다 (단일 이슈).
        let first = self.states.values().filter_map(|s| s.1).fold(f64::INFINITY, f64::min);
        if first.is_finite() && (now as f64 - first) / 1000.0 >= self.wait_secs {
            let (tag, trig) = &triggered[0];
            self.opened = Some(tag.clone());
            let fault = db.sensor(tag).and_then(|x| db.equipment(&x.kks)).map(|e| fault_code(&e.kind, self.signal(tag)));
            return Some(OpenCall { alarm_tag: tag.clone(), upstream_tag: None, lag_secs: None, trigger: trig.kind.code().to_string(), value: trig.value.unwrap_or_default(), at_ms: now, fault, onset_lag_secs: None });
        }
        None
    }
}

/// 추적기가 연 사건: 링버퍼 60초를 1초 간격으로 얼린 계측과, 경보(또는 선행 경보) 한 건
pub fn incident_from(db: &Lattice, call: &OpenCall, ring: &LiveRing, id: &str) -> crate::case::Incident {
    use crate::case::{AlarmEvent, AlarmKind, Incident, IncidentSource};
    use chrono::TimeZone;
    let at = chrono::Utc.timestamp_millis_opt(call.at_ms).single().unwrap_or_else(chrono::Utc::now);
    let sensor = db.sensor(&call.alarm_tag);
    let kind = match (call.trigger.as_str(), sensor.and_then(|s| s.alarm_side(call.value))) {
        ("THRESHOLD_BREACH", Some(true)) => AlarmKind::High,
        ("THRESHOLD_BREACH", Some(false)) => AlarmKind::Low,
        _ => AlarmKind::PreAlarm,
    };
    let label = sensor.map(|s| s.label.clone()).unwrap_or_default();
    let message = match kind {
        AlarmKind::High => format!("{label} 고 경보"),
        AlarmKind::Low => format!("{label} 저 경보"),
        AlarmKind::PreAlarm => format!("{label} 상승 기울기 선행 경보 (트립 도달 30초 이내)"),
    };
    Incident {
        id: id.to_string(),
        source: IncidentSource::Dcs,
        alarm: Some(AlarmEvent { tag: call.alarm_tag.clone(), channel: None, at, value: call.value, kind, message }),
        telemetry: ring.snapshot(),
        reports: Vec::new(),
        received_at: at,
        scenario: Some("PHYS".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(sim: &mut PlantSim, ring: &mut LiveRing, secs: f64) {
        sim.run_into(ring, (secs * 1000.0 / TICK_MS as f64) as usize);
    }

    #[test]
    fn steady_state_stays_at_design_values() {
        let db = Lattice::embedded().unwrap();
        let mut sim = PlantSim::new(&db, 0, 7);
        let mut ring = LiveRing::default();
        run(&mut sim, &mut ring, 30.0);
        assert!((sim.true_value(TAG_FLOW).unwrap() - QD).abs() < 1.0);
        assert!((sim.true_value(TAG_SH).unwrap() - T0).abs() < 0.1);
        assert!((sim.true_value(TAG_PUMP).unwrap() - P0).abs() < 0.01);
        let st = ring.ts_stats(TAG_SH, 10_000).unwrap();
        assert!(st.slope_per_sec.abs() < 0.2 && !st.flat, "{st:?}");
    }

    #[test]
    fn a_stuck_valve_starves_the_superheater_after_the_advection_delay_and_the_bypass_recovers_it() {
        let db = Lattice::embedded().unwrap();
        let mut sim = PlantSim::new(&db, 0, 7);
        let mut ring = LiveRing::default();
        run(&mut sim, &mut ring, 25.0);
        sim.inject(Fault::ValveStuck { open_pct: 18.0 });
        run(&mut sim, &mut ring, 4.0);
        // 유량은 곧바로 약 1,110 t/h, 펌프 토출압은 약 29.4 MPa
        assert!((1050.0..1170.0).contains(&sim.true_value(TAG_FLOW).unwrap()), "{}", sim.true_value(TAG_FLOW).unwrap());
        assert!((29.2..29.6).contains(&sim.true_value(TAG_PUMP).unwrap()));
        // 과열기는 τ(4.2 s) 전에는 그대로
        assert!((sim.true_value(TAG_SH).unwrap() - T0).abs() < 0.5);
        run(&mut sim, &mut ring, 16.0);
        let hot = sim.true_value(TAG_SH).unwrap();
        assert!(hot > 620.0, "냉각이 모자라 온도가 오릅니다: {hot}");
        // 상 · 하류 변곡점 시차는 이류 지연과 같은 쪽
        let lag = ring.ts_inflection_lag(db.sensor(TAG_VALVE).unwrap(), db.sensor(TAG_SH).unwrap()).unwrap();
        assert!((3.0..9.0).contains(&lag), "{lag}");
        // 바이패스 65 % → 유량 회복 → 온도가 꺾여 내려옵니다
        sim.command(Command::SetBypass { open_pct: 65.0 });
        run(&mut sim, &mut ring, 10.0);
        assert!(sim.true_value(TAG_FLOW).unwrap() > 1750.0, "{}", sim.true_value(TAG_FLOW).unwrap());
        let peak = sim.true_value(TAG_SH).unwrap();
        run(&mut sim, &mut ring, 30.0);
        let later = sim.true_value(TAG_SH).unwrap();
        assert!(later < peak && peak < 645.0, "트립 전에 꺾입니다: 고점 {peak} → {later}");
    }

    #[test]
    fn a_sensor_bias_changes_only_the_reading() {
        let db = Lattice::embedded().unwrap();
        let mut sim = PlantSim::new(&db, 0, 3);
        sim.inject(Fault::SensorBias { tag: "10MAA10CP001".into(), bias: -11.8 });
        let readings = sim.tick();
        let v = readings.iter().find(|r| r.0 == "10MAA10CP001").unwrap().1;
        assert!((v - 14.2).abs() < 0.3, "{v}");
        assert!((sim.true_value("10MAA10CP001").unwrap() - 26.0).abs() < 1e-9);
    }

    #[test]
    fn the_tracker_locks_the_valve_to_superheater_chain_and_opens_one_incident() {
        let db = Lattice::embedded().unwrap();
        // 시작 시각이 정각 초가 아니어도 같아야 합니다 (실제 시계)
        let mut sim = PlantSim::new(&db, 1_793_769_600_123, 11);
        let mut ring = LiveRing::default();
        let mut tr = Tracker::new();
        run(&mut sim, &mut ring, 25.0);
        assert_eq!(tr.observe(&db, &ring), None, "정상 운전에서는 열지 않습니다");
        sim.inject(Fault::ValveStuck { open_pct: 18.0 });
        let mut call = None;
        for _ in 0..40 {
            run(&mut sim, &mut ring, 1.0);
            if let Some(c) = tr.observe(&db, &ring) {
                call = Some(c);
                break;
            }
        }
        let call = call.expect("하류 과열기 선행 경보에서 사건을 엽니다");
        assert_eq!(call.alarm_tag, TAG_SH, "{call:?}");
        assert!(call.upstream_tag.is_some() && call.lag_secs.is_some_and(|l| l > 2.0), "{call:?}");
        assert!(tr.events.iter().any(|e| e.tag == TAG_VALVE && e.to == TrackState::InflectionDetected));
        assert!(tr.events.iter().any(|e| e.to == TrackState::CausalChainLocked));
        // SDS 신호 모양: 밸브는 떨어지는 동안 SLEW_DROP, 그 자리에 머물면 STUCK_LOW. 과열기는 SLEW_RISE.
        let valve: Vec<SignalState> = tr.signal_events.iter().filter(|e| e.tag == TAG_VALVE).map(|e| e.to).collect();
        assert_eq!(valve.first(), Some(&SignalState::SlewDrop), "{:?}", tr.signal_events);
        assert_eq!(tr.signal(TAG_VALVE), SignalState::StuckLow, "{:?}", tr.signal_events);
        assert!(tr.signal_events.iter().any(|e| e.tag == TAG_SH && e.to == SignalState::SlewRise), "{:?}", tr.signal_events);
        // 고장 코드는 원장 인터록의 고장 코드와 같아 절차가 바로 이어집니다.
        assert_eq!(call.fault.as_deref(), Some("VALVE_STUCK_LOW"));
        assert!(db.interlocks().iter().any(|i| Some(i.fault.as_str()) == call.fault.as_deref()));
        // 변화 시작 시차는 이류 지연(4.2 초) 근처이고, 문턱으로 잰 변곡 시차보다 짧거나 같습니다 (느린 상승은 문턱을 늦게 넘음).
        let onset = call.onset_lag_secs.expect("변화 시작 시차");
        let lag = call.lag_secs.unwrap();
        assert!((3.0..=6.5).contains(&onset) && onset <= lag + 0.5, "변화 시작 {onset:.1}s · 변곡 {lag:.1}s");
        // 잡음만 있는 센서는 안정에서 움직이지 않습니다.
        assert!(tr.signal_events.iter().all(|e| [TAG_VALVE, TAG_SH, TAG_FLOW, TAG_PUMP].contains(&e.tag.as_str())), "{:?}", tr.signal_events);
        run(&mut sim, &mut ring, 1.0);
        assert_eq!(tr.observe(&db, &ring), None, "사건은 한 번만 엽니다");
    }
}
