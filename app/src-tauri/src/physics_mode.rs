//! 물리엔진 · 자율 대응 모드 (기획 Part 8-5). 콘솔 창 전용입니다.
//!
//! 시뮬레이터가 실제 시간으로 100 ms 마다 돌고, 1초마다 SDS 추적기가 링버퍼를 봅니다. 추적기가 사건을 열면 다른 스레드가
//! 엔진(판단 · 글쓰기 모델)을 돌리고 완화 조치를 제안합니다. 모델이 생각하는 동안에도 공정은 계속 움직입니다 (틱을 멈추지 않음).
//! 자동화 모드 ∧ 규칙 통과 ∧ 네 기준 최솟값 ≥ 0.95 이면 바이패스 명령을 시뮬레이터에 넣고, 아니면 교대감독의 실행을 기다립니다.
//! 명령은 시뮬레이터에만 들어갑니다. 계통(태그 · 상수)은 지금 도메인 원장의 `physics` 입니다 (발전소 급수 · 데이터센터 CDU 수냉).

use std::time::Duration;

use blueprint_core::mitigation::{Action, Mitigation};
use blueprint_core::physics::{self, Command, Fault, OpenCall, PlantSim, Tracker, TICK_MS};
use blueprint_core::state_machine::staged;
use blueprint_core::tsdb::LiveRing;
use chrono::Utc;
use serde::Serialize;
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, State, Window};

use super::{console_only, lock, notify_console, Desk};

#[derive(Default)]
pub struct PhysicsState {
    sim: Option<PlantSim>,
    ring: LiveRing,
    tracker: Tracker,
    running: bool,
    started_ms: i64,
    /// 추적기가 연 사건
    incident: Option<String>,
    /// 사건을 연 차례 (고장 코드 · 변곡 시차 · 변화 시작 시차)
    call: Option<OpenCall>,
    /// 엔진이 사건을 분석하는 중
    busy: bool,
    peak_sh: f64,
    mitigation: Option<Mitigation>,
    /// 완화 조치를 낸 시각 (시뮬레이터 시계)
    proposed_ms: Option<i64>,
    note: Option<String>,
    /// 스레드 세대 (다시 시작하면 이전 스레드가 끝납니다)
    generation: u64,
}

impl PhysicsState {
    /// 멈춘 빈 상태 (도메인을 바꿀 때). 세대를 올려 돌던 스레드가 끝나게 합니다.
    pub fn stopped(generation: u64) -> Self {
        Self { generation, ..Self::default() }
    }

    pub fn generation_next(&self) -> u64 {
        self.generation + 1
    }
}

fn apply(ph: &mut PhysicsState, action: &Action, by: &str) {
    let Action::OpenBypass { open_pct, .. } = action;
    if let Some(sim) = ph.sim.as_mut() {
        sim.command(Command::SetBypass { open_pct: *open_pct });
        let at = sim.now_ms();
        let (hot, flow) = (sim.config().tag_hot.clone(), sim.config().tag_flow.clone());
        ph.tracker.mitigating(&[hot.as_str(), flow.as_str()], at, &format!("바이패스 {open_pct}% 개방 ({by})"));
    }
}

fn analyze_call(app: AppHandle, call: OpenCall) {
    std::thread::spawn(move || {
        let desk = app.state::<Desk>();
        let db = desk.db();
        let incident = {
            let ph = lock(&desk.physics);
            physics::incident_from(&db, &call, &ph.ring, "")
        };
        // 1) 글쓰기 없이 분석하고 완화 조치를 먼저 냅니다 (판단 질문만, 수 초). 2) 그다음 AOP 초안을 모델로 다시 씁니다.
        // 엔진 잠금은 준비 · 반영 때만 쥡니다. 모델이 도는 동안에도 콘솔의 조회 · 결정 · 인터록 계측 확인이 멈추지 않습니다.
        let desk: &Desk = &desk;
        let result = staged::open(desk, &db, "INC", Utc::now(), true, |id| {
            let mut incident = incident;
            incident.id = id.to_string();
            incident
        })
        .and_then(|(id, _)| staged::propose_mitigation(desk, &db, &id, Utc::now()).map(|m| (id, m)));
        let id = {
            let mut ph = lock(&desk.physics);
            ph.busy = false;
            ph.proposed_ms = ph.sim.as_ref().map(|s| s.now_ms());
            match result {
                Ok((id, m)) => {
                    ph.incident = Some(id.clone());
                    if let Some(m) = m {
                        if m.auto {
                            apply(&mut ph, &m.action, "Zoro-1 자동");
                        }
                        ph.mitigation = Some(m);
                    } else {
                        ph.note = Some("이 사건에는 우회 완화 조치가 없습니다 (교대감독 결재)".into());
                    }
                    Some(id)
                }
                Err(e) => {
                    ph.note = Some(format!("분석하지 못했습니다: {e:#}"));
                    None
                }
            }
        };
        notify_console(&app);
        let _ = app.emit("physics_update", ());
        if let Some(id) = id {
            // 판정은 다시 묻지 않고 글만 다시 씁니다.
            let done = staged::redraft(desk, &db, &id);
            if let Err(e) = done {
                lock(&desk.physics).note = Some(format!("AOP 초안을 다시 쓰지 못했습니다 (템플릿 유지): {e:#}"));
            }
            notify_console(&app);
        }
    });
}

fn run_loop(app: AppHandle, generation: u64) {
    std::thread::spawn(move || {
        let mut k: u64 = 0;
        loop {
            std::thread::sleep(Duration::from_millis(TICK_MS as u64));
            let desk = app.state::<Desk>();
            let db = desk.db();
            let mut call = None;
            let mut refresh = None;
            {
                let mut ph = lock(&desk.physics);
                if !ph.running || ph.generation != generation {
                    break;
                }
                let Some(sim) = ph.sim.as_mut() else { break };
                let t = sim.now_ms() + TICK_MS;
                let readings = sim.tick();
                let sh = sim.true_value(&sim.config().tag_hot).unwrap_or(0.0);
                for (tag, v) in readings {
                    ph.ring.push(&tag, t, v);
                }
                ph.peak_sh = ph.peak_sh.max(sh);
                k += 1;
                if k % 10 == 0 {
                    let PhysicsState { tracker, ring, busy, incident, call: opened, .. } = &mut *ph;
                    if !*busy {
                        call = tracker.observe(&db, ring);
                        if call.is_some() {
                            *busy = true;
                        }
                        if let Some(c) = &call {
                            *opened = Some(c.clone());
                        }
                    } else {
                        tracker.observe(&db, ring);
                    }
                    if let Some(id) = incident.clone() {
                        let now = t - t.rem_euclid(1000);
                        refresh = Some((id, ring.snapshot(), now));
                    }
                }
            }
            if let Some(call) = call {
                analyze_call(app.clone(), call);
            }
            // 우회가 열리면 인터록 Step 2 를 계측값으로 채웁니다 (엔진이 판단 중이면 다음 초에).
            if let Some((id, telemetry, now)) = refresh {
                if let Ok(mut engine) = desk.engine.try_lock() {
                    if engine.refresh_interlock(&id, &telemetry, now, Utc::now()) {
                        drop(engine);
                        notify_console(&app);
                    }
                }
            }
            if k % 5 == 0 {
                let _ = app.emit("physics_update", ());
            }
        }
    });
}

/// 시뮬레이터를 정상 운전으로 (다시) 시작합니다. 기준 구간을 채우려고 처음 25초는 바로 돌려 둡니다.
#[tauri::command(async)]
pub fn physics_start(window: Window, app: AppHandle, desk: State<'_, Desk>) -> Result<(), String> {
    console_only(&window)?;
    let generation = {
        let mut ph = lock(&desk.physics);
        let start = Utc::now().timestamp_millis();
        let mut sim = PlantSim::new(&desk.db(), start - 25_000, start as u64);
        let mut ring = LiveRing::default();
        sim.run_into(&mut ring, 250);
        let generation = ph.generation + 1;
        let peak_sh = sim.config().t0;
        *ph = PhysicsState { sim: Some(sim), ring, tracker: Tracker::new(), running: true, started_ms: start, peak_sh, generation, ..PhysicsState::default() };
        generation
    };
    run_loop(app, generation);
    Ok(())
}

#[tauri::command(async)]
pub fn physics_stop(window: Window, desk: State<'_, Desk>) -> Result<(), String> {
    console_only(&window)?;
    lock(&desk.physics).running = false;
    Ok(())
}

/// 고장 주입: `valve_stuck` (조절밸브 18 % 고착: 발전소 103동 급수 · 데이터센터 103동 CDU) ·
/// `sensor_drift` (전송기 바이어스: 발전소 주증기 압력 -11.8 MPa · 데이터센터 매니폴드 압력 -1.6 bar)
#[tauri::command(async)]
pub fn physics_inject(window: Window, desk: State<'_, Desk>, kind: String) -> Result<(), String> {
    console_only(&window)?;
    let mut ph = lock(&desk.physics);
    let sim = ph.sim.as_mut().ok_or("먼저 시뮬레이터를 시작하세요")?;
    let fault = match kind.as_str() {
        "valve_stuck" => Fault::ValveStuck { open_pct: 18.0 },
        "sensor_drift" => Fault::SensorBias { tag: sim.config().drift_tag.clone(), bias: sim.config().drift_bias },
        other => return Err(format!("모르는 고장입니다: {other}")),
    };
    sim.inject(fault);
    Ok(())
}

/// 교대감독이 제안된 완화 조치를 실행합니다 (보조 모드 · 확신도 미달일 때).
#[tauri::command(async)]
pub fn physics_execute(window: Window, app: AppHandle, desk: State<'_, Desk>, supervisor_id: String) -> Result<String, String> {
    console_only(&window)?;
    let supervisor = supervisor_id.trim();
    if supervisor.is_empty() {
        return Err("교대감독 사번을 머리에 적어 주세요".into());
    }
    // 명령은 시뮬레이터에 곧바로 넣고, 감사 기록은 엔진이 비면 남깁니다 (엔진이 AOP 초안을 쓰는 중이어도 기다리지 않게).
    let (id, label) = {
        let mut ph = lock(&desk.physics);
        let id = ph.incident.clone().ok_or("추적기가 연 사건이 없습니다")?;
        let m = ph.mitigation.clone().ok_or("제안된 완화 조치가 없습니다")?;
        if m.executed_at.is_some() {
            return Err("이미 실행한 완화 조치입니다".into());
        }
        apply(&mut ph, &m.action, &format!("교대감독 {supervisor}"));
        if let Some(x) = ph.mitigation.as_mut() {
            x.executed_at = Some(Utc::now());
            x.executed_by = Some(format!("교대감독 {supervisor}"));
        }
        (id, m.label)
    };
    let supervisor = supervisor.to_string();
    std::thread::spawn(move || {
        let desk = app.state::<Desk>();
        let recorded = lock(&desk.engine).execute_mitigation(&id, &supervisor, Utc::now());
        if let Err(e) = recorded {
            lock(&desk.physics).note = Some(format!("실행 기록을 남기지 못했습니다: {e:#}"));
        }
        notify_console(&app);
    });
    Ok(format!("{label} 을 실행했습니다 (시뮬레이터)"))
}

/// 물리엔진이 돌고 있으면 지금 계측 (검색 · 상태 조건용)
pub fn live_snapshot(desk: &Desk) -> Option<(blueprint_core::timeseries::Telemetry, i64)> {
    let ph = lock(&desk.physics);
    if !ph.running {
        return None;
    }
    let now = ph.sim.as_ref()?.now_ms();
    Some((ph.ring.snapshot(), now - now.rem_euclid(1000)))
}

#[derive(Debug, Serialize)]
pub struct PhysicsDto {
    /// 화면에 그리는 태그 (원인 밸브 · 우회 밸브 · 유량 · 펌프 토출압 · 하류 온도)와 하류 온도 태그
    shown: Vec<String>,
    hot_tag: String,
    running: bool,
    busy: bool,
    t_secs: f64,
    t0_ms: i64,
    now_ms: i64,
    period_ms: i64,
    series: serde_json::Map<String, Value>,
    latest: Vec<(String, f64)>,
    events: Vec<Value>,
    /// 센서 신호 모양 기록 (SDS 상태 코드)
    signals: Vec<Value>,
    /// 사건을 연 차례
    call: Option<Value>,
    faults: Value,
    commands: Value,
    incident: Option<String>,
    mitigation: Option<Value>,
    /// 완화 조치를 낸 시각 (시작 뒤 초)
    proposed_t: Option<f64>,
    peak_sh: f64,
    note: Option<String>,
}

#[tauri::command(async)]
pub fn physics_status(window: Window, desk: State<'_, Desk>) -> Result<PhysicsDto, String> {
    console_only(&window)?;
    let ph = lock(&desk.physics);
    let now_ms = ph.sim.as_ref().map(|s| s.now_ms()).unwrap_or(0);
    let cfg = ph.sim.as_ref().map(|s| s.config().clone()).unwrap_or_else(|| desk.db().physics().clone());
    let mut series = serde_json::Map::new();
    let mut t0_ms = now_ms;
    for tag in &cfg.shown {
        if let Some(s) = ph.ring.series(tag, 1000) {
            t0_ms = s.t0_ms;
            series.insert(tag.to_string(), json!(s.values));
        }
    }
    let latest = cfg.shown.iter().filter_map(|t| ph.ring.latest(t).map(|(_, v)| (t.to_string(), v))).collect();
    let events = ph
        .tracker
        .events
        .iter()
        .rev()
        .take(30)
        .map(|e| json!({ "t": (e.at_ms - ph.started_ms) as f64 / 1000.0, "tag": e.tag, "from": e.from, "to": e.to, "note": e.note }))
        .collect();
    let signals = ph
        .tracker
        .signal_events
        .iter()
        .rev()
        .take(30)
        .map(|e| json!({ "t": (e.at_ms - ph.started_ms) as f64 / 1000.0, "tag": e.tag, "from": e.from, "to": e.to, "note": e.note }))
        .collect();
    Ok(PhysicsDto {
        shown: cfg.shown.clone(),
        hot_tag: cfg.tag_hot.clone(),
        running: ph.running,
        busy: ph.busy,
        t_secs: (now_ms - ph.started_ms) as f64 / 1000.0,
        t0_ms,
        now_ms: now_ms - now_ms.rem_euclid(1000),
        period_ms: 1000,
        series,
        latest,
        events,
        signals,
        call: ph.call.as_ref().map(|c| json!({ "t": (c.at_ms - ph.started_ms) as f64 / 1000.0, "alarm_tag": c.alarm_tag, "upstream_tag": c.upstream_tag, "lag_secs": c.lag_secs, "onset_lag_secs": c.onset_lag_secs, "fault": c.fault, "trigger": c.trigger })),
        faults: json!(ph.sim.as_ref().map(|s| s.faults.clone()).unwrap_or_default()),
        commands: json!(ph.sim.as_ref().map(|s| s.commands.iter().map(|(t, c)| json!({ "t": (t - ph.started_ms) as f64 / 1000.0, "command": c })).collect::<Vec<_>>()).unwrap_or_default()),
        incident: ph.incident.clone(),
        mitigation: ph.mitigation.as_ref().map(|m| json!(m)),
        proposed_t: ph.proposed_ms.map(|t| (t - ph.started_ms) as f64 / 1000.0),
        peak_sh: ph.peak_sh,
        note: ph.note.clone(),
    })
}
