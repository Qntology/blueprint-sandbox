//! SDS 트리거 고도화 비교 (CPU, 모델 없음).
//!
//!   cargo run -p blueprint-core --release --example sds_eval
//!
//! 1. 물리엔진 밸브 고착 (잡음 시드 20개): 과열기 선행 경보가 처음 나는 시각을 이전 규칙(1차 TTF, 튐 막기 없음)과
//!    지금 규칙(1 · 2차 중 이른 TTF, 튐 막기)으로 재고, 그 시각 + 처리 지연(앱 실측: 사건 열림 → 완화 조치 후보 약 12초) 뒤
//!    바이패스를 65 % 열었을 때의 과열기 고점을 잽니다 (트립 645 °C).
//! 2. 시나리오 P1 ~ P7: 경보 한계가 있는 센서마다 첫 트리거(시각 · 종류)가 두 규칙에서 달라지는 곳을 모두 찍습니다.
//! 3. 추적기가 사건을 열 때의 변곡 시차와 변화 시작(CUSUM) 시차 분포 (시작 시각 2가지 × 시드 20개), 고장 코드

use blueprint_core::lattice::{Lattice, SensorRow};
use blueprint_core::physics::{Command, Fault, PlantSim, Tracker, TICK_MS};
use blueprint_core::scenario::ScenarioSet;
use blueprint_core::sds::{trigger_at, TriggerKind, PRE_ALARM_TTF_SECS};
use blueprint_core::timeseries::{ttf, Series, BASELINE_SECS, SLOPE_SAMPLES};
use blueprint_core::tsdb::LiveRing;

const SH: &str = "10HAH10CT001";
/// 사건이 열린 뒤 완화 조치가 나오기까지 (앱 실측 17.3 → 29.0 초)
const PROCESS_SECS: i64 = 12;

/// 이전 규칙 (d975440): 1차 TTF, 한 표본 튐도 그대로 엶
fn old_trigger(sensor: &SensorRow, series: &Series, at: i64) -> Option<TriggerKind> {
    let value = series.value_at(at)?;
    let remain = series.slope_at(at, SLOPE_SAMPLES).and_then(|s| ttf(sensor, value, s));
    if sensor.alarm_side(value).is_some() {
        return Some(TriggerKind::ThresholdBreach);
    }
    let departed = series.departure(sensor.deadband, series.t0_ms + BASELINE_SECS * 1000, at).is_some();
    (departed && remain.is_some_and(|r| r <= PRE_ALARM_TTF_SECS)).then_some(TriggerKind::HighSlopePreAlarm)
}

fn secs(sim: &mut PlantSim, ring: &mut LiveRing, s: i64) {
    sim.run_into(ring, (s * 1000 / TICK_MS) as usize);
}

/// 고착 주입 뒤 매초 두 규칙의 첫 트리거 시각 (주입 기준 초)
fn first_triggers(db: &Lattice, seed: u64) -> (Option<i64>, Option<i64>) {
    let sensor = db.sensor(SH).unwrap();
    let mut sim = PlantSim::new(db, 1_793_769_600_000, seed);
    let mut ring = LiveRing::default();
    secs(&mut sim, &mut ring, 25);
    let inject = sim.now_ms();
    sim.inject(Fault::ValveStuck { open_pct: 18.0 });
    let (mut old, mut new) = (None, None);
    for k in 1..=60 {
        secs(&mut sim, &mut ring, 1);
        let series = ring.series(SH, 1000).unwrap();
        let at = series.end_ms();
        if old.is_none() && old_trigger(sensor, &series, at).is_some() {
            old = Some(k);
        }
        if new.is_none() && trigger_at(sensor, &series, at).is_some() {
            new = Some(k);
        }
        let _ = inject;
        if old.is_some() && new.is_some() {
            break;
        }
    }
    (old, new)
}

/// 주입 뒤 `open_at` 초에 바이패스 65 % 를 명령했을 때 과열기 고점
fn peak_with_bypass(db: &Lattice, seed: u64, open_at: i64) -> f64 {
    let mut sim = PlantSim::new(db, 1_793_769_600_000, seed);
    let mut ring = LiveRing::default();
    secs(&mut sim, &mut ring, 25);
    sim.inject(Fault::ValveStuck { open_pct: 18.0 });
    let mut peak: f64 = 0.0;
    for k in 1..=240 {
        if k == open_at {
            sim.command(Command::SetBypass { open_pct: 65.0 });
        }
        secs(&mut sim, &mut ring, 1);
        peak = peak.max(sim.true_value(SH).unwrap());
    }
    peak
}

fn stats(xs: &[f64]) -> String {
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    let (lo, hi) = xs.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(a, b), x| (a.min(*x), b.max(*x)));
    format!("평균 {mean:.1} (최소 {lo:.1} · 최대 {hi:.1})")
}

fn main() {
    let db = Lattice::embedded().expect("원장");
    println!("## 1. 물리엔진 밸브 고착 (18 %) · 시드 20개 · 처리 지연 {PROCESS_SECS}초");
    let (mut t_old, mut t_new, mut p_old, mut p_new) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for seed in 1..=20u64 {
        let (old, new) = first_triggers(&db, seed);
        let (old, new) = (old.expect("이전 규칙도 엽니다"), new.expect("지금 규칙도 엽니다"));
        let (po, pn) = (peak_with_bypass(&db, seed, old + PROCESS_SECS), peak_with_bypass(&db, seed, new + PROCESS_SECS));
        println!("  시드 {seed:>2}: 선행 경보 {old:>2}s → {new:>2}s · 고점 {po:.1} → {pn:.1} °C");
        t_old.push(old as f64);
        t_new.push(new as f64);
        p_old.push(po);
        p_new.push(pn);
    }
    println!("  선행 경보 (주입 뒤 초) 이전 {} · 지금 {}", stats(&t_old), stats(&t_new));
    println!("  과열기 고점 (°C)       이전 {} · 지금 {}", stats(&p_old), stats(&p_new));

    println!();
    println!("## 2. 시나리오 첫 트리거가 달라지는 곳 (경보 한계가 있는 센서)");
    let set = ScenarioSet::embedded().expect("시나리오");
    let mut same = 0;
    for sc in &set.scenarios {
        let tel = sc.telemetry(&set);
        for s in db.sensors().iter().filter(|s| s.alarm_low.is_some() || s.alarm_high.is_some()) {
            let Some(series) = tel.get(&s.tag) else { continue };
            let from = series.t0_ms + BASELINE_SECS * 1000;
            let first = |f: &dyn Fn(i64) -> Option<TriggerKind>| (0..).map(|i| from + i * series.period_ms).take_while(|t| *t <= series.end_ms()).find_map(|t| f(t).map(|k| (t, k)));
            let old = first(&|t| old_trigger(s, series, t));
            let new = first(&|t| trigger_at(s, series, t).map(|x| x.kind));
            if old == new {
                same += 1;
                continue;
            }
            let show = |x: Option<(i64, TriggerKind)>| x.map_or("없음".to_string(), |(t, k)| format!("{:+.0}s {}", (t - sc.time(0.0).timestamp_millis()) as f64 / 1000.0, k.code()));
            println!("  {} {} {}: 이전 {} → 지금 {}", sc.id, s.tag, s.label, show(old), show(new));
        }
    }
    println!("  (나머지 {same}개 센서 · 시나리오 쌍은 같음)");

    println!();
    println!("## 3. 사건을 열 때의 시차 (밸브 → 과열기, 이류 지연 4.2초)");
    let (mut lags, mut onsets, mut faults) = (Vec::new(), Vec::new(), std::collections::BTreeMap::new());
    for start in [1_793_769_600_000i64, 1_793_769_600_123] {
        for seed in 1..=20u64 {
            let mut sim = PlantSim::new(&db, start, seed);
            let mut ring = LiveRing::default();
            let mut tracker = Tracker::new();
            secs(&mut sim, &mut ring, 25);
            tracker.observe(&db, &ring);
            sim.inject(Fault::ValveStuck { open_pct: 18.0 });
            for _ in 0..40 {
                secs(&mut sim, &mut ring, 1);
                if let Some(call) = tracker.observe(&db, &ring) {
                    lags.push(call.lag_secs.unwrap_or(f64::NAN));
                    onsets.push(call.onset_lag_secs.unwrap_or(f64::NAN));
                    *faults.entry(call.fault.unwrap_or_default()).or_insert(0) += 1;
                    break;
                }
            }
        }
    }
    let below = lags.iter().zip(&onsets).filter(|(l, o)| **o <= **l + 0.5).count();
    println!("  사건 {}건 · 변곡 시차 {} · 변화 시작 시차 {} · 변화 시작 ≤ 변곡 + 0.5초 {below}건", lags.len(), stats(&lags), stats(&onsets));
    println!("  고장 코드 {faults:?}");
}
