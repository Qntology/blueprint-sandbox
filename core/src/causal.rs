//! 인과 시차(Δt) 그래프 탐색 (기획 Part 8-3 3.4절). Gate 3 의 근본 원인 역추적입니다.
//!
//! 하류 알람 설비의 변곡 시각을 `t_alarm` 이라 할 때, 상류 설비의 변곡 시각 `t_up` 이
//! `t_alarm - Δt_max ≤ t_up ≤ t_alarm - Δt_min` 안에 들어와야 진짜 인과 경로로 봅니다 (Δt 범위는 경로 간선 지연의 합).
//! 하류 알람보다 늦게 변한 상류 설비는 원인이 아닙니다. 경로 위의 하류 이웃보다 늦게 변한 상류 설비는 역류 여파로 따로 표시합니다
//! (예: 급수밸브가 닫힌 뒤 펌프 토출 압력이 오르는 배압).
//!
//! 원인은 인과 조건을 만족하고 역류 여파가 아닌 설비 중 가장 먼저 변한 설비입니다. 없으면 단위 설비 단독 고장으로 봅니다.
//! 이 판단은 모두 규칙입니다. 모델은 이 결과를 사실로 받아 확신도만 줍니다.

use serde::Serialize;

use crate::graph::{upstream_paths, MAX_HOPS};
use crate::lattice::Lattice;
use crate::plant::{physics_consistent, Direction, Measure};
use crate::timeseries::{Departure, Telemetry, BASELINE_SECS};

/// 지연 범위 경계에서 봐주는 폭 (초). 1초 표본의 보간 오차입니다.
pub const LAG_TOLERANCE_SECS: f64 = 0.5;
/// 원인 탐색을 알람 변곡 이전 어디까지 할지 (경로 지연 상한에 더하는 여유, 초)
const SEARCH_MARGIN_SECS: f64 = 2.0;

/// 간선 위의 시간 전파 검증 결과 (기획안 `TemporalPropagation`)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TemporalPropagation {
    pub upstream_kks: String,
    pub downstream_kks: String,
    pub upstream_tag: String,
    pub downstream_tag: String,
    /// 관측 시차 Δt = t_down - t_up (초)
    pub observed_lag_secs: f64,
    /// 배관 길이 · 유속 기준 허용 범위 (초)
    pub expected_lag_range: (f64, f64),
    /// 시간 선후와 전파 속도가 맞는가
    pub causal_match: bool,
    /// 경로 위 하류 이웃보다 늦게 변함 (역류 여파)
    pub backflow: bool,
    pub hops: usize,
    pub path: Vec<String>,
    pub connectors: Vec<String>,
    pub up_direction: Direction,
    pub down_direction: Direction,
    /// 변화 방향이 물리적으로 맞는가 (모르면 None)
    pub physics: Option<bool>,
}

/// 근본 원인 설비
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RootCause {
    pub kks: String,
    /// 원인을 보여 준 센서
    pub tag: String,
    pub hops: usize,
    pub path: Vec<String>,
    pub connectors: Vec<String>,
    pub lag_secs: f64,
    pub lag_range: (f64, f64),
    pub direction: Direction,
    pub physics: Option<bool>,
    /// 원인 설비의 변곡 시각 (epoch ms)
    pub at_ms: f64,
}

/// 신호 전파 체인의 한 줄 (프로세스 마이닝의 이벤트 추적)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ChainEvent {
    pub at_ms: f64,
    pub tag: String,
    pub kks: String,
    pub label: String,
    pub direction: Direction,
    /// "departure" (변곡) 또는 "limit" (경보 · 트립 한계 돌파)
    pub kind: &'static str,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CausalReport {
    pub alarm_kks: String,
    pub alarm_tag: String,
    /// 알람 센서의 변곡 시각. 벗어난 적이 없으면 None
    pub alarm_departure_ms: Option<f64>,
    pub alarm_direction: Option<Direction>,
    pub propagation: Vec<TemporalPropagation>,
    pub root: Option<RootCause>,
}

/// 센서의 변곡 (`from_ms` 부터 `until_ms` 까지). 기준 구간은 `from_ms` 앞 20초이고, 시계열 첫 20초보다 앞이면 그 끝으로 당깁니다.
pub fn departure_of(db: &Lattice, telemetry: &Telemetry, tag: &str, from_ms: i64, until_ms: i64) -> Option<Departure> {
    let sensor = db.sensor(tag)?;
    let series = telemetry.get(tag)?;
    let earliest = series.t0_ms + BASELINE_SECS * 1000;
    series.departure(sensor.deadband, from_ms.max(earliest), until_ms)
}

/// 설비의 센서 중 가장 먼저 변한 것
fn first_departure(db: &Lattice, telemetry: &Telemetry, kks: &str, from_ms: i64, until_ms: i64) -> Option<(String, Departure)> {
    db.sensors_of(kks)
        .into_iter()
        .filter_map(|s| departure_of(db, telemetry, &s.tag, from_ms, until_ms).map(|d| (s.tag.clone(), d)))
        .min_by(|a, b| a.1.at_ms.total_cmp(&b.1.at_ms))
}

/// 알람 태그에서 상류로 근본 원인을 찾습니다. `now_ms` 까지의 계측만 씁니다.
pub fn trace_root_cause(db: &Lattice, telemetry: &Telemetry, alarm_tag: &str, now_ms: i64) -> CausalReport {
    let alarm_kks = db.sensor(alarm_tag).map(|s| s.kks.clone()).unwrap_or_default();
    let start = telemetry.start_ms().unwrap_or(now_ms);
    let alarm = departure_of(db, telemetry, alarm_tag, start, now_ms);
    let mut report = CausalReport {
        alarm_kks: alarm_kks.clone(),
        alarm_tag: alarm_tag.to_string(),
        alarm_departure_ms: alarm.map(|d| d.at_ms),
        alarm_direction: alarm.map(|d| d.direction),
        propagation: Vec::new(),
        root: None,
    };
    let Some(alarm) = alarm else { return report };
    let alarm_measure = Measure::of_tag(alarm_tag);

    let paths = upstream_paths(db, &alarm_kks, MAX_HOPS);
    for path in &paths {
        let from = (alarm.at_ms - (path.lag.1 + SEARCH_MARGIN_SECS) * 1000.0).floor() as i64;
        let Some((tag, dep)) = first_departure(db, telemetry, path.source(), from, now_ms) else { continue };
        let lag = (alarm.at_ms - dep.at_ms) / 1000.0;
        let causal_match = lag >= path.lag.0 - LAG_TOLERANCE_SECS && lag <= path.lag.1 + LAG_TOLERANCE_SECS;
        // 경로 위 하류 이웃(알람 설비 제외) 중 이 설비보다 먼저 변한 것이 있으면 역류 여파입니다.
        let backflow = path.nodes[1..path.nodes.len() - 1]
            .iter()
            .filter_map(|n| first_departure(db, telemetry, n, from, now_ms))
            .any(|(_, d)| d.at_ms < dep.at_ms);
        let first_edge = db.edge_between(&path.nodes[0], &path.nodes[1]).and_then(|e| e.medium());
        let physics = first_edge.and_then(|m| physics_consistent(m, (Measure::of_tag(&tag), dep.direction), (alarm_measure, alarm.direction)));
        report.propagation.push(TemporalPropagation {
            upstream_kks: path.source().to_string(),
            downstream_kks: alarm_kks.clone(),
            upstream_tag: tag,
            downstream_tag: alarm_tag.to_string(),
            observed_lag_secs: lag,
            expected_lag_range: path.lag,
            causal_match,
            backflow,
            hops: path.hops(),
            path: path.nodes.clone(),
            connectors: path.connectors.clone(),
            up_direction: dep.direction,
            down_direction: alarm.direction,
            physics,
        });
    }
    report.root = report
        .propagation
        .iter()
        .filter(|p| p.causal_match && !p.backflow && p.physics != Some(false))
        .min_by(|a, b| b.observed_lag_secs.total_cmp(&a.observed_lag_secs))
        .map(|p| RootCause {
            kks: p.upstream_kks.clone(),
            tag: p.upstream_tag.clone(),
            hops: p.hops,
            path: p.path.clone(),
            connectors: p.connectors.clone(),
            lag_secs: p.observed_lag_secs,
            lag_range: p.expected_lag_range,
            direction: p.up_direction,
            physics: p.physics,
            at_ms: alarm.at_ms - p.observed_lag_secs * 1000.0,
        });
    report
}

/// `from_ms` ~ `now_ms` 에 변곡하거나 한계를 넘은 센서를 시간 순으로 모읍니다 (신호 전파 체인).
pub fn event_chain(db: &Lattice, telemetry: &Telemetry, from_ms: i64, now_ms: i64) -> Vec<ChainEvent> {
    let mut out = Vec::new();
    for sensor in db.sensors() {
        let Some(series) = telemetry.get(&sensor.tag) else { continue };
        if let Some(d) = departure_of(db, telemetry, &sensor.tag, from_ms, now_ms) {
            out.push(ChainEvent { at_ms: d.at_ms, tag: sensor.tag.clone(), kks: sensor.kks.clone(), label: sensor.label.clone(), direction: d.direction, kind: "departure" });
        }
        let limits = [(sensor.alarm_high, true), (sensor.trip_high, true), (sensor.alarm_low, false), (sensor.trip_low, false)];
        if let Some((at, up)) = limits.iter().filter_map(|(l, up)| l.and_then(|l| series.crossing(l, *up, from_ms, now_ms)).map(|t| (t, *up))).min_by(|a, b| a.0.total_cmp(&b.0)) {
            out.push(ChainEvent {
                at_ms: at,
                tag: sensor.tag.clone(),
                kks: sensor.kks.clone(),
                label: sensor.label.clone(),
                direction: if up { Direction::Up } else { Direction::Down },
                kind: "limit",
            });
        }
    }
    out.sort_by(|a, b| a.at_ms.total_cmp(&b.at_ms));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::timeseries::Series;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    /// 모든 센서를 정상치로 채우고 몇 개만 바꾼 계측 (t = -60 ~ +90 초)
    fn telemetry(changes: &[(&str, &dyn Fn(f64) -> f64)]) -> Telemetry {
        let db = db();
        let mut t = Telemetry::default();
        for s in db.sensors() {
            let f: Box<dyn Fn(f64) -> f64> = match changes.iter().find(|(tag, _)| *tag == s.tag) {
                Some((_, f)) => Box::new(|x| f(x)),
                None => {
                    let v = s.normal;
                    Box::new(move |_| v)
                }
            };
            t.series.insert(s.tag.clone(), Series::new(-60_000, 1000, (0..151).map(|i| f((i - 60) as f64)).collect()));
        }
        t
    }

    fn ramp(points: &'static [(f64, f64)]) -> impl Fn(f64) -> f64 {
        move |t| {
            if t <= points[0].0 {
                return points[0].1;
            }
            for w in points.windows(2) {
                let ((t0, v0), (t1, v1)) = (w[0], w[1]);
                if t <= t1 {
                    return v0 + (v1 - v0) * (t - t0) / (t1 - t0);
                }
            }
            points[points.len() - 1].1
        }
    }

    #[test]
    fn a_stuck_valve_two_hops_up_is_the_root_and_the_pump_is_only_back_pressure() {
        let valve = ramp(&[(0.0, 78.0), (1.0, 18.0), (90.0, 18.0)]);
        let flow = ramp(&[(0.4, 1850.0), (3.0, 1180.0), (6.0, 1120.0), (90.0, 1118.0)]);
        let pump = ramp(&[(1.6, 28.5), (2.6, 29.15), (5.0, 29.4), (90.0, 29.42)]);
        let temp = ramp(&[(4.2, 610.0), (10.0, 631.0), (20.0, 637.5), (90.0, 638.2)]);
        let t = telemetry(&[("10LAB20CG102", &valve), ("10LAB20CF001", &flow), ("10LAB10CP001", &pump), ("10HAH10CT001", &temp)]);
        let r = trace_root_cause(&db(), &t, "10HAH10CT001", 9_000);
        let root = r.root.expect("근본 원인을 찾아야 합니다");
        assert_eq!(root.kks, "10LAB20AA102");
        assert_eq!(root.tag, "10LAB20CG102", "개도가 유량보다 먼저 변했습니다");
        assert_eq!(root.hops, 2);
        assert_eq!(root.connectors, vec!["OPC-103-101".to_string()]);
        assert!((3.5..5.5).contains(&root.lag_secs), "{}", root.lag_secs);
        assert_eq!(root.physics, Some(true), "급수 감소 → 증기 온도 상승");
        let pump = r.propagation.iter().find(|p| p.upstream_kks == "10LAB10AP001").expect("펌프도 기록합니다");
        assert!(pump.backflow, "밸브보다 늦게 오른 펌프 압력은 역류 여파입니다");
    }

    #[test]
    fn without_any_upstream_change_there_is_no_root() {
        let vib = ramp(&[(0.0, 45.0), (3.0, 95.0), (9.0, 205.0), (14.0, 212.0), (90.0, 213.0)]);
        let t = telemetry(&[("10MAD10CY001", &vib)]);
        let r = trace_root_cause(&db(), &t, "10MAD10CY001", 10_000);
        assert!(r.alarm_departure_ms.is_some());
        assert!(r.root.is_none(), "{:?}", r.root);
        assert!(r.propagation.is_empty());
    }

    #[test]
    fn a_change_too_late_or_too_early_for_the_pipe_is_not_the_cause() {
        // 밸브가 경보보다 30초 먼저 변함: 고압급수 경로 상한 12초를 넘습니다.
        let valve = ramp(&[(-30.0, 78.0), (-29.0, 18.0), (90.0, 18.0)]);
        let temp = ramp(&[(4.2, 610.0), (10.0, 631.0), (90.0, 638.0)]);
        let t = telemetry(&[("10LAB20CG102", &valve), ("10HAH10CT001", &temp)]);
        let r = trace_root_cause(&db(), &t, "10HAH10CT001", 9_000);
        assert!(r.root.is_none(), "{:?}", r.root);
    }

    #[test]
    fn the_fan_stall_explains_the_furnace_pressure() {
        let fan = ramp(&[(0.0, 360.0), (2.0, 300.0), (4.0, 285.0), (90.0, 282.0)]);
        let furnace = ramp(&[(2.0, -0.1), (10.0, 0.38), (30.0, 0.95), (90.0, 1.12)]);
        let t = telemetry(&[("10HLB10CE001", &fan), ("10HAD10CP101", &furnace)]);
        let r = trace_root_cause(&db(), &t, "10HAD10CP101", 6_000);
        let root = r.root.unwrap();
        assert_eq!(root.kks, "10HLB10AN001");
        assert_eq!(root.connectors, vec!["OPC-105-101".to_string()]);
        assert_eq!(root.physics, Some(true), "흡인 약화 → 노내 압력 상승");
    }

    #[test]
    fn the_event_chain_is_in_time_order() {
        let valve = ramp(&[(0.0, 78.0), (1.0, 18.0), (90.0, 18.0)]);
        let temp = ramp(&[(4.2, 610.0), (10.0, 631.0), (90.0, 638.0)]);
        let t = telemetry(&[("10LAB20CG102", &valve), ("10HAH10CT001", &temp)]);
        let chain = event_chain(&db(), &t, -40_000, 20_000);
        let tags: Vec<(&str, &str)> = chain.iter().map(|e| (e.tag.as_str(), e.kind)).collect();
        assert_eq!(tags, vec![("10LAB20CG102", "departure"), ("10LAB20CG102", "limit"), ("10HAH10CT001", "departure"), ("10HAH10CT001", "limit")]);
    }
}
