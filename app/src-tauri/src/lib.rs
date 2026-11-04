//! blueprint 앱. 주제어실(MCR) 통합 관제 콘솔과 현장 순시 · 정비 단말 두 창을 띄웁니다 (zoro-gravity 와 같은 구조).
//!
//! 모델 관리는 zoro-gravity 와 같습니다. 다만 모델이 없으면 먼저 zoro-gravity 의 모델 폴더에서 가져오고(하드 링크),
//! 거기에도 없을 때만 Hugging Face 에서 내려받습니다. 진행은 `download_progress` · `download_complete` · `download_error` 이벤트로 알립니다.
//!
//! 현장 단말에는 내부 정보를 보내지 않습니다. 진단 · 결재 명령은 콘솔 창에서만 받고, 이벤트도 콘솔 창으로만 보냅니다.
//! 현장 단말은 스캔(`scan_field_documents`)과 전송(`submit_field_report`)만 부르고, 접수 번호와 안내 글만 받습니다.
//!
//! DCS 연계가 없어서 합성 시나리오를 재생하는 시뮬레이터로 알람을 넣습니다 (`inject_scenario`).
//! 스캐너는 `BLUEPRINT_SCANNER_DIR` 폴더(기본 `%LOCALAPPDATA%\blueprint\scanner`)의 이미지로 대신합니다.

mod physics_mode;

use std::collections::{BTreeSet, HashMap};
use std::hash::{BuildHasher, Hasher};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use blueprint_core::ai::accel;
use blueprint_core::ai::document::DocumentReader;
use blueprint_core::case::{DeviationExcuse, FieldKind, ReadTimings, Signature};
use blueprint_core::extract::{extract, DocKind, Extracted, PageText};
use blueprint_core::interlock::Evidence;
use blueprint_core::issues;
use blueprint_core::audit::AuditCategory;
use blueprint_core::tiling;
use blueprint_core::field::{self, FieldError, FieldForm, FieldState};
use blueprint_core::graph::undirected_distance;
use blueprint_core::lattice::Lattice;
use blueprint_core::mermaid::{self, MermaidOptions};
use blueprint_core::mining;
use blueprint_core::plant::{self, Sector};
use blueprint_core::policy::{AutomationPolicy, OperatingMode};
use blueprint_core::privacy::{mask_id, mask_name, salted_hash};
use blueprint_core::scenario::ScenarioSet;
use blueprint_core::state_machine::gate3_substantive::Justification;
use blueprint_core::state_machine::{Engine, Outcome, SupervisorDecision};
use blueprint_core::store::SessionStatus;
use blueprint_core::timeseries::Telemetry;
use blueprint_models::{app_dir, delete_model as remove_model, download, entry, import_from, import_source, models_dir, status, CATALOG};
use chrono::{DateTime, Duration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::{AppHandle, Emitter, Manager, RunEvent, State, Window};

const CONSOLE: &str = "console";
const FIELD: &str = "field";

/// 마스터 도면 (사전 인덱싱한 P&ID). 도면 뷰가 이 이미지 위에 설비 상자를 얹습니다.
const DRAWINGS: [(&str, &[u8]); 5] = [
    ("PID-101-HAD", include_bytes!("../../../core/fixtures/drawings/PID-101-HAD.png")),
    ("PID-102-MAA", include_bytes!("../../../core/fixtures/drawings/PID-102-MAA.png")),
    ("PID-103-LAB", include_bytes!("../../../core/fixtures/drawings/PID-103-LAB.png")),
    ("PID-104-BBA", include_bytes!("../../../core/fixtures/drawings/PID-104-BBA.png")),
    ("PID-105-HLB", include_bytes!("../../../core/fixtures/drawings/PID-105-HLB.png")),
];

/// 도면 생성기가 그린 배관 꺾은선 (정규화 좌표). 도면 뷰의 흐름 오버레이가 이미지와 같은 선을 따라 그립니다.
const DRAWING_META: [&str; 5] = [
    include_str!("../../../core/fixtures/drawings/PID-101-HAD.json"),
    include_str!("../../../core/fixtures/drawings/PID-102-MAA.json"),
    include_str!("../../../core/fixtures/drawings/PID-103-LAB.json"),
    include_str!("../../../core/fixtures/drawings/PID-104-BBA.json"),
    include_str!("../../../core/fixtures/drawings/PID-105-HLB.json"),
];

/// A0 종합 도면 (Part 8-4): 배치 · 격자 타일 · 설비마다 A0 좌표와 든 타일
fn a0_views(db: &Lattice) -> Value {
    let views: Vec<Value> = db
        .a0_drawings()
        .iter()
        .map(|a0| {
            let grid = tiling::a0_tiles(a0);
            let page = (a0.pixel_size[0], a0.pixel_size[1]);
            let equipment: serde_json::Map<String, Value> = db
                .equipment_all()
                .iter()
                .filter_map(|e| {
                    let g = tiling::a0_bbox(db, a0, &e.kks)?;
                    Some((e.kks.clone(), json!({ "bbox": g, "tile": tiling::tile_of(&grid, g, page).map(|t| t.id.clone()) })))
                })
                .collect();
            json!({ "id": a0.id, "title": a0.title, "rev": a0.rev, "pixel_size": a0.pixel_size, "sheet_px": a0.sheet_px, "grid": a0.grid, "sheets": a0.sheets, "tiles": grid, "equipment": equipment })
        })
        .collect();
    Value::Array(views)
}

fn drawing_pipes() -> Value {
    let mut out = serde_json::Map::new();
    for text in DRAWING_META {
        if let Ok(meta) = serde_json::from_str::<Value>(text) {
            if let (Some(id), Some(pipes)) = (meta["drawing"].as_str(), meta.get("pipes")) {
                out.insert(id.to_string(), pipes.clone());
            }
        }
    }
    Value::Object(out)
}

/// 시뮬레이터가 마지막으로 넣은 시나리오의 계측 (현장 보고 사건에 붙입니다)
struct Live {
    scenario: String,
    telemetry: Telemetry,
    /// 넣은 순간의 시나리오 시각과 벽시계
    sim_at: DateTime<Utc>,
    wall_at: Instant,
    /// 시계열 끝 시각
    end: DateTime<Utc>,
}

impl Live {
    /// 지금의 시나리오 시각 (시계열 끝에서 멈춤)
    fn sim_now(&self) -> DateTime<Utc> {
        (self.sim_at + Duration::milliseconds(self.wall_at.elapsed().as_millis() as i64)).min(self.end)
    }
}

/// 관제 상태. 엔진은 진행 중 사건의 세션을 들고 있습니다.
struct Desk {
    engine: Mutex<Engine>,
    db: Lattice,
    scenarios: ScenarioSet,
    /// GPU 를 쓰는 일은 한 번에 하나입니다.
    gpu: Mutex<()>,
    reader: Mutex<Option<Arc<DocumentReader>>>,
    status: Mutex<String>,
    loaded: Mutex<bool>,
    device: Mutex<Option<candle_core::Device>>,
    live: Mutex<Option<Live>>,
    /// 현장 단말이 결과를 기다리는 접수 번호
    field_case: Mutex<Option<String>>,
    /// 지금 현장 운전원이 넣은 문서에서 읽은 값 (화면을 거치지 않고 접수로 갑니다)
    field_scans: Mutex<Vec<Extracted>>,
    field_timings: Mutex<Option<ReadTimings>>,
    /// 물리엔진 · 자율 대응 모드 (Part 8-5)
    physics: Mutex<physics_mode::PhysicsState>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

fn console_only(window: &Window) -> Result<(), String> {
    if window.label() == CONSOLE {
        Ok(())
    } else {
        Err("주제어실 콘솔에서만 할 수 있습니다".to_string())
    }
}

fn field_only(window: &Window) -> Result<(), String> {
    if window.label() == FIELD {
        Ok(())
    } else {
        Err("현장 단말에서만 할 수 있습니다".to_string())
    }
}

fn random_u32() -> u32 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    hasher.write_u128(nanos);
    hasher.finish() as u32
}

fn engine_ready() -> bool {
    status().iter().all(|s| s.installed || s.entry.repo.is_none())
}

fn local_time(at: DateTime<Utc>) -> String {
    plant::local(at).format("%Y-%m-%d %H:%M:%S").to_string()
}

// ───────────────────────── 모델 관리 ─────────────────────────

#[derive(Debug, Clone, Default, Serialize)]
struct Job {
    running: bool,
    percent: u32,
    message: String,
    error: Option<String>,
}

#[derive(Default)]
struct Jobs(Mutex<HashMap<&'static str, Job>>);

impl Jobs {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<&'static str, Job>> {
        lock(&self.0)
    }
}

#[derive(Debug, Serialize)]
struct ModelRow {
    name: &'static str,
    label: &'static str,
    license: &'static str,
    repo: Option<&'static str>,
    total_bytes: u64,
    bytes_on_disk: u64,
    installed: bool,
    importable: bool,
    job: Job,
}

#[derive(Debug, Serialize)]
struct ModelReport {
    dir: String,
    import_from: Option<String>,
    models: Vec<ModelRow>,
}

fn notify_field(app: &AppHandle) {
    let loaded = *lock(&app.state::<Desk>().loaded);
    let _ = app.emit_to(FIELD, "field_state", json!({ "ready": engine_ready() && loaded }));
}

#[tauri::command]
fn check_model_status(window: Window, jobs: State<'_, Jobs>) -> Result<ModelReport, String> {
    console_only(&window)?;
    let jobs = jobs.lock();
    let models = status()
        .into_iter()
        .map(|s| ModelRow {
            name: s.entry.name,
            label: s.entry.label,
            license: s.entry.license,
            repo: s.entry.repo,
            total_bytes: s.entry.total_bytes(),
            bytes_on_disk: s.bytes_on_disk,
            installed: s.installed,
            importable: s.importable,
            job: jobs.get(s.entry.name).cloned().unwrap_or_default(),
        })
        .collect();
    Ok(ModelReport { dir: models_dir().display().to_string(), import_from: import_source().map(|p| p.display().to_string()), models })
}

/// zoro-gravity 모델 폴더에서 모델 하나를 가져옵니다 (큰 가중치는 하드 링크).
#[tauri::command(async)]
fn import_model(window: Window, app: AppHandle, model_name: String) -> Result<String, String> {
    console_only(&window)?;
    let model = entry(&model_name).ok_or_else(|| format!("모르는 모델입니다: {model_name}"))?;
    let source = import_source().ok_or("가져올 곳을 모릅니다 (BLUEPRINT_IMPORT_FROM)")?;
    let r = import_from(&source, model.name).map_err(|e| format!("{e:#}"))?;
    let _ = app.emit_to(CONSOLE, "download_complete", json!({ "model": model.name }));
    notify_field(&app);
    Ok(format!("링크 {}개 · 복사 {}개 · 그대로 {}개", r.linked.len(), r.copied.len(), r.kept.len()))
}

/// 없는 모델은 먼저 zoro-gravity 에서 가져오고, 그래도 없으면 설정을 열고 내려받으라고 알립니다.
#[tauri::command]
fn ensure_models(window: Window, app: AppHandle) -> Result<(), String> {
    console_only(&window)?;
    if let Some(source) = import_source() {
        for s in status().iter().filter(|s| s.importable) {
            if let Err(e) = import_from(&source, s.entry.name) {
                eprintln!("[모델] {} 가져오기 실패: {e:#}", s.entry.name);
            }
        }
    }
    for s in status().iter().filter(|s| !s.installed && s.entry.repo.is_some()) {
        let alert = json!({ "action": "open_settings", "model": s.entry.name, "message": format!("{} 모델을 내려받습니다", s.entry.label) });
        app.emit_to(CONSOLE, "app_error_alert", alert).map_err(|e| e.to_string())?;
    }
    notify_field(&app);
    Ok(())
}

#[tauri::command]
fn download_model(window: Window, app: AppHandle, jobs: State<'_, Jobs>, model_name: String) -> Result<(), String> {
    console_only(&window)?;
    let model = entry(&model_name).filter(|e| e.repo.is_some()).ok_or_else(|| format!("내려받을 수 없는 모델입니다: {model_name}"))?;
    {
        let mut jobs = jobs.lock();
        let job = jobs.entry(model.name).or_default();
        if job.running {
            return Ok(());
        }
        *job = Job { running: true, ..Job::default() };
    }
    std::thread::spawn(move || {
        let state = app.state::<Jobs>();
        let result = download(model.name, &mut |p| {
            if let Some(job) = state.lock().get_mut(model.name) {
                job.percent = p.percent;
                job.message = p.message.clone();
            }
            let _ = app.emit_to(CONSOLE, "download_progress", json!({ "model": model.name, "percent": p.percent, "message": p.message }));
        });
        let error = result.err().map(|e| format!("{e:#}"));
        if let Some(job) = state.lock().get_mut(model.name) {
            job.running = false;
            job.error = error.clone();
        }
        let _ = match error {
            Some(error) => app.emit_to(CONSOLE, "download_error", json!({ "model": model.name, "error": error })),
            None => app.emit_to(CONSOLE, "download_complete", json!({ "model": model.name })),
        };
        notify_field(&app);
    });
    Ok(())
}

fn delete_models(app: &AppHandle, jobs: &Jobs, names: &[&'static str]) -> Result<(), String> {
    let mut jobs = jobs.lock();
    if let Some(busy) = names.iter().find(|n| jobs.get(**n).is_some_and(|j| j.running)) {
        return Err(format!("{busy} 를 내려받는 중입니다"));
    }
    for name in names {
        remove_model(name).map_err(|e| format!("{e:#}"))?;
        jobs.remove(name);
    }
    notify_field(app);
    Ok(())
}

#[tauri::command]
fn delete_model(window: Window, app: AppHandle, jobs: State<'_, Jobs>, model_name: String) -> Result<(), String> {
    console_only(&window)?;
    let model = entry(&model_name).ok_or_else(|| format!("모르는 모델입니다: {model_name}"))?;
    delete_models(&app, &jobs, &[model.name])
}

#[tauri::command]
fn delete_all_models(window: Window, app: AppHandle, jobs: State<'_, Jobs>) -> Result<(), String> {
    console_only(&window)?;
    let names: Vec<&'static str> = CATALOG.iter().map(|e| e.name).collect();
    delete_models(&app, &jobs, &names)
}

// ───────────────────────── 계통 · 도면 ─────────────────────────

#[derive(Debug, Serialize)]
struct SensorDto {
    tag: String,
    kks: String,
    label: String,
    unit: String,
    normal: f64,
    alarm_low: Option<f64>,
    alarm_high: Option<f64>,
    trip_low: Option<f64>,
    trip_high: Option<f64>,
    /// 기준치에 겹친 MOC 번호
    overlay: Option<String>,
}

#[derive(Debug, Serialize)]
struct EquipmentDto {
    kks: String,
    name: String,
    sector: String,
    floor: String,
    drawing: String,
    kind: String,
    bbox: [f32; 4],
    breadcrumb: Vec<String>,
    place: String,
}

#[derive(Debug, Serialize)]
struct ConnectorDto {
    id: String,
    from: String,
    to: String,
    from_drawing: String,
    to_drawing: String,
    out_bbox: [f32; 4],
    in_bbox: [f32; 4],
    label: String,
}

#[derive(Debug, Serialize)]
struct PlantDto {
    name: String,
    kind: String,
    sectors: Value,
    floors: Value,
    drawings: Value,
    equipment: Vec<EquipmentDto>,
    sensors: Vec<SensorDto>,
    edges: Value,
    connectors: Vec<ConnectorDto>,
    aops: Value,
    pipes: Value,
    a0: Value,
}

/// 계통 전체 (콘솔 창 전용). 센서 기준치에는 반영된 MOC 오버레이를 겹칩니다.
#[tauri::command(async)]
fn plant_topology(window: Window, desk: State<'_, Desk>) -> Result<PlantDto, String> {
    console_only(&window)?;
    let db = &desk.db;
    let engine = lock(&desk.engine);
    let equipment = db
        .equipment_all()
        .iter()
        .map(|e| {
            let a = db.address(&e.kks);
            EquipmentDto {
                kks: e.kks.clone(),
                name: e.name.clone(),
                sector: e.sector.clone(),
                floor: e.floor.clone(),
                drawing: e.drawing.clone(),
                kind: e.kind.clone(),
                bbox: e.bbox,
                breadcrumb: a.as_ref().map(|a| a.breadcrumb().to_vec()).unwrap_or_default(),
                place: a.as_ref().map(|a| a.place()).unwrap_or_default(),
            }
        })
        .collect();
    let sensors = db
        .sensors()
        .iter()
        .map(|s| {
            let eff = engine.store.effective(s);
            let overlay = engine.store.overlays().iter().find(|o| o.tag == s.tag).map(|o| o.moc_no.clone());
            SensorDto { tag: s.tag.clone(), kks: s.kks.clone(), label: s.label.clone(), unit: s.unit.clone(), normal: s.normal, alarm_low: eff.alarm_low, alarm_high: eff.alarm_high, trip_low: eff.trip_low, trip_high: eff.trip_high, overlay }
        })
        .collect();
    let connectors = db
        .connectors()
        .iter()
        .map(|c| ConnectorDto {
            id: c.id.clone(),
            from: c.from.clone(),
            to: c.to.clone(),
            from_drawing: db.equipment(&c.from).map(|e| e.drawing.clone()).unwrap_or_default(),
            to_drawing: db.equipment(&c.to).map(|e| e.drawing.clone()).unwrap_or_default(),
            out_bbox: c.out_bbox,
            in_bbox: c.in_bbox,
            label: db.edge_between(&c.from, &c.to).map(|e| e.label.clone()).unwrap_or_default(),
        })
        .collect();
    Ok(PlantDto {
        name: db.plant().name.clone(),
        kind: db.plant().kind.clone(),
        sectors: json!(db.sectors()),
        floors: json!(db.floors()),
        drawings: json!(db.drawings()),
        equipment,
        sensors,
        edges: json!(db.edges()),
        connectors,
        aops: json!(db.aops()),
        pipes: drawing_pipes(),
        a0: a0_views(db),
    })
}

/// 마스터 도면 이미지 (data URL)
#[tauri::command]
fn drawing_image(window: Window, id: String) -> Result<String, String> {
    console_only(&window)?;
    use base64::Engine as _;
    let (_, bytes) = DRAWINGS.iter().find(|(d, _)| *d == id).ok_or_else(|| format!("도면이 없습니다: {id}"))?;
    Ok(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes)))
}

// ───────────────────────── DCS 시뮬레이터 ─────────────────────────

#[derive(Debug, Serialize)]
struct ScenarioDto {
    id: String,
    title: String,
    story: String,
    has_alarm: bool,
    scans: Vec<String>,
}

#[tauri::command]
fn scenario_list(window: Window, desk: State<'_, Desk>) -> Result<Vec<ScenarioDto>, String> {
    console_only(&window)?;
    Ok(desk
        .scenarios
        .scenarios
        .iter()
        .map(|s| ScenarioDto { id: s.id.clone(), title: s.title.clone(), story: s.story.clone(), has_alarm: s.alarm.is_some(), scans: s.scans.iter().map(|x| x.image.clone()).collect() })
        .collect())
}

fn notify_console(app: &AppHandle) {
    let _ = app.emit_to(CONSOLE, "incident_changed", json!({}));
}

/// 시나리오를 재생해 알람을 넣습니다 (콘솔 창 전용). 알람이 없는 시나리오(Red-line)는 계측만 바꾸고 현장 보고를 기다립니다.
#[tauri::command(async)]
fn inject_scenario(window: Window, app: AppHandle, desk: State<'_, Desk>, id: String) -> Result<Option<String>, String> {
    console_only(&window)?;
    let s = desk.scenarios.get(&id).ok_or_else(|| format!("시나리오가 없습니다: {id}"))?;
    let telemetry = s.telemetry(&desk.scenarios);
    let sim_at = s.time(s.now_s);
    let end = s.time((desk.scenarios.from_s + (s.series.values().next().map_or(0, |v| v.len()) as i64 - 1) * desk.scenarios.period_s) as f64);
    *lock(&desk.live) = Some(Live { scenario: s.id.clone(), telemetry, sim_at, wall_at: Instant::now(), end });
    if s.alarm.is_none() {
        notify_console(&app);
        return Ok(None);
    }
    let result = {
        let _gpu = lock(&desk.gpu);
        let mut engine = lock(&desk.engine);
        let incident_id = engine.next_id("INC", Utc::now());
        let incident = s.incident(&desk.scenarios, &incident_id);
        engine.run(&desk.db, incident, Utc::now()).map(|_| incident_id)
    };
    notify_console(&app);
    result.map(Some).map_err(|e| format!("{e:#}"))
}

// ───────────────────────── 사건 목록 · 상세 ─────────────────────────

#[derive(Debug, Serialize)]
struct IncidentRow {
    id: String,
    title: String,
    building: String,
    kks: String,
    /// "awaiting_field" · "review" · "done"
    state: &'static str,
    verdict: Option<String>,
    verdict_label: Option<String>,
    verdict_class: Option<String>,
    reason: String,
    high_risk: bool,
    opened_at: Option<String>,
    waiting_secs: Option<i64>,
    archive_id: Option<String>,
    final_code: Option<String>,
    auto: bool,
    closed_at: Option<String>,
    scenario: Option<String>,
}

#[tauri::command(async)]
fn incident_list(window: Window, desk: State<'_, Desk>) -> Result<Vec<IncidentRow>, String> {
    console_only(&window)?;
    let engine = lock(&desk.engine);
    let now = Utc::now();
    let mut rows: Vec<IncidentRow> = engine
        .store
        .sessions()
        .map(|s| {
            let a = &s.analysis;
            let j = s.judgement.as_ref();
            IncidentRow {
                id: s.incident.id.clone(),
                title: s.incident.alarm.as_ref().map(|x| x.message.clone()).unwrap_or_else(|| format!("{} 현장 보고", a.route.address.name)),
                building: a.route.address.building.clone(),
                kks: a.route.kks.clone(),
                state: if s.status == SessionStatus::AwaitingField { "awaiting_field" } else { "review" },
                verdict: j.map(|j| j.verdict.code().to_string()),
                verdict_label: j.map(|j| j.verdict.label().to_string()),
                verdict_class: j.map(|j| format!("{:?}", j.verdict.class()).to_lowercase()),
                reason: s.reason.clone(),
                high_risk: a.high_risk(),
                opened_at: Some(local_time(s.opened_at)),
                waiting_secs: Some((now - s.opened_at).num_seconds().max(0)),
                archive_id: None,
                final_code: None,
                auto: false,
                closed_at: None,
                scenario: s.incident.scenario.clone(),
            }
        })
        .collect();
    rows.sort_by(|a, b| b.opened_at.cmp(&a.opened_at));
    for a in engine.store.archive().iter().rev() {
        rows.push(IncidentRow {
            id: a.incident_id.clone(),
            title: a.title.clone(),
            building: String::new(),
            kks: String::new(),
            state: "done",
            verdict: None,
            verdict_label: None,
            verdict_class: None,
            reason: String::new(),
            high_risk: false,
            opened_at: None,
            waiting_secs: None,
            archive_id: Some(a.archive_id.clone()),
            final_code: Some(a.final_code.clone()),
            auto: a.auto,
            closed_at: Some(local_time(a.closed_at)),
            scenario: a.scenario.clone(),
        });
    }
    Ok(rows)
}

#[derive(Debug, Serialize)]
struct ReportDto {
    kind: &'static str,
    kks: Option<String>,
    tag: Option<String>,
    reading: Option<f64>,
    unit: Option<String>,
    taken_at: String,
    moc_no: Option<String>,
    old_value: Option<f64>,
    drawing: Option<String>,
    drawing_rev: Option<String>,
    worker: String,
    worker_id: String,
    signed: bool,
    permit_confirmed: bool,
    excuse: Option<&'static str>,
    corrections: Value,
    uncertain: Vec<String>,
}

#[derive(Debug, Serialize)]
struct TimelineDto {
    t0_ms: i64,
    period_ms: i64,
    now_ms: i64,
    series: HashMap<String, Vec<f64>>,
    channels: HashMap<String, HashMap<String, Vec<f64>>>,
}

#[derive(Debug, Serialize)]
struct DetailDto {
    id: String,
    state: &'static str,
    reason: String,
    notes: Vec<String>,
    field_request: Option<String>,
    judgement: Value,
    alarm: Value,
    received_at: String,
    opened_at: String,
    reports: Vec<ReportDto>,
    analysis: Value,
    timeline: TimelineDto,
    flows: Value,
    /// 단일 · 절차 · 복합 이슈 분해 (Part 8-6)
    issues: Value,
    scenario: Option<String>,
    sim_time: String,
}

fn timeline(t: &Telemetry, now_ms: i64) -> TimelineDto {
    let t0_ms = t.start_ms().unwrap_or(now_ms);
    let period_ms = t.series.values().next().map_or(1000, |s| s.period_ms);
    TimelineDto {
        t0_ms,
        period_ms,
        now_ms,
        series: t.series.iter().map(|(k, s)| (k.clone(), s.values.clone())).collect(),
        channels: t.channels.iter().map(|(k, chs)| (k.clone(), chs.iter().map(|(c, s)| (c.clone(), s.values.clone())).collect())).collect(),
    }
}

/// 사건 상세 (콘솔 창 전용). 근무자 이름과 사번은 가려서 보냅니다.
#[tauri::command(async)]
fn incident_detail(window: Window, desk: State<'_, Desk>, id: String) -> Result<DetailDto, String> {
    console_only(&window)?;
    let engine = lock(&desk.engine);
    let s = engine.store.active(&id).ok_or("진행 중인 사건이 아닙니다 (처리가 끝났으면 보관 목록을 보세요)")?;
    let reports = s
        .incident
        .reports
        .iter()
        .map(|r| ReportDto {
            kind: r.kind.label(),
            kks: r.kks.clone(),
            tag: r.tag.clone(),
            reading: r.reading,
            unit: r.unit.clone(),
            taken_at: local_time(r.taken_at),
            moc_no: r.moc_no.clone(),
            old_value: r.old_value,
            drawing: r.drawing.clone(),
            drawing_rev: r.drawing_rev.clone(),
            worker: r.worker_name.as_deref().map(mask_name).unwrap_or_default(),
            worker_id: r.worker_id.as_deref().map(mask_id).unwrap_or_default(),
            signed: r.signature.is_some(),
            permit_confirmed: r.permit_confirmed,
            excuse: r.excuse.as_ref().map(|e| e.label()),
            corrections: json!(r.corrections),
            uncertain: r.uncertain.clone(),
        })
        .collect();
    let now_ms = s.incident.now_ms();
    let flows = mining::edge_flows(&desk.db, Some(&s.incident.telemetry), now_ms, Some(&s.analysis));
    let intercepted: Vec<String> = engine.store.audit().of(&id).iter().filter(|e| e.category == AuditCategory::IllegalActionIntercepted).map(|e| e.detail.clone()).collect();
    let issues = issues::decompose(&desk.db, &s.analysis, &s.incident.reports, &intercepted);
    Ok(DetailDto {
        id: id.clone(),
        state: if s.status == SessionStatus::AwaitingField { "awaiting_field" } else { "review" },
        reason: s.reason.clone(),
        notes: s.notes.clone(),
        field_request: s.field_request.clone(),
        judgement: json!(s.judgement),
        alarm: json!(s.incident.alarm),
        received_at: local_time(s.incident.received_at),
        opened_at: local_time(s.opened_at),
        reports,
        analysis: json!(s.analysis),
        timeline: timeline(&s.incident.telemetry, now_ms),
        flows: json!(flows),
        issues: json!(issues),
        scenario: s.incident.scenario.clone(),
        sim_time: plant::local(s.incident.received_at).format("%H:%M:%S").to_string(),
    })
}

/// 보관 문서 (콘솔 창 전용)
#[tauri::command(async)]
fn archive_detail(window: Window, desk: State<'_, Desk>, id: String) -> Result<Value, String> {
    console_only(&window)?;
    let engine = lock(&desk.engine);
    let a = engine.store.archive().iter().find(|a| a.incident_id == id).ok_or("보관 문서가 없습니다")?;
    Ok(json!(a))
}

#[derive(Debug, Deserialize)]
struct MermaidInput {
    incident: Option<String>,
    selected: Option<String>,
    mining: bool,
    whole: bool,
}

/// Mermaid 원본 (콘솔 창 전용). 사건이 있으면 그 경로를 굵게, 마이닝을 켜면 흐름 손실과 Δt 를 적습니다.
#[tauri::command(async)]
fn mermaid_source(window: Window, desk: State<'_, Desk>, input: MermaidInput) -> Result<String, String> {
    console_only(&window)?;
    let db = &desk.db;
    let engine = lock(&desk.engine);
    let session = input.incident.as_deref().and_then(|i| engine.store.active(i));
    let analysis = session.map(|s| &s.analysis);
    let flows = match session {
        Some(s) => mining::edge_flows(db, Some(&s.incident.telemetry), s.incident.now_ms(), analysis),
        None => mining::edge_flows(db, None, 0, None),
    };
    let root = analysis.and_then(|a| a.causal_root());
    let path: Vec<String> = root.map(|r| r.path.clone()).unwrap_or_default();
    let center = input.selected.clone().or_else(|| analysis.map(|a| a.route.kks.clone()));
    let scope: Option<BTreeSet<String>> = match (&center, input.whole) {
        (Some(c), false) => {
            let mut set: BTreeSet<String> = undirected_distance(db, c).into_iter().filter(|(_, d)| *d <= 2).map(|(k, _)| k).collect();
            set.extend(path.iter().cloned());
            Some(set)
        }
        _ => None,
    };
    Ok(mermaid::source(
        db,
        &flows,
        &MermaidOptions {
            scope: scope.as_ref(),
            selected: input.selected.as_deref(),
            alarm: analysis.map(|a| a.route.kks.as_str()),
            root: root.map(|r| r.kks.as_str()),
            path: &path,
            mining: input.mining,
        },
    ))
}

#[derive(Debug, Serialize)]
struct MiningDto {
    bottlenecks: Value,
    lanes: Value,
}

#[tauri::command(async)]
fn mining_overview(window: Window, desk: State<'_, Desk>) -> Result<MiningDto, String> {
    console_only(&window)?;
    let engine = lock(&desk.engine);
    let now = Utc::now();
    Ok(MiningDto { bottlenecks: json!(mining::bottlenecks(&desk.db, &engine.store, now)), lanes: json!(mining::lanes(&engine.store, now)) })
}

// ───────────────────────── 교대감독 결정 ─────────────────────────

#[derive(Debug, Deserialize)]
struct DecisionInput {
    id: String,
    supervisor_id: String,
    /// confirm · approve_procedure · approve_false_positive · cancel · recheck · reject
    action: String,
    team_lead_cosign: Option<bool>,
    procedure_ref: Option<String>,
    evidence_scans: Option<u32>,
    note: Option<String>,
}

fn outcome_text(o: &Outcome) -> (bool, String) {
    match o {
        Outcome::Finalized { archive_id } => (true, format!("확정했습니다 ({archive_id})")),
        Outcome::AutoFinalized { archive_id, .. } => (true, format!("자동 확정되었습니다 ({archive_id})")),
        Outcome::Blocked { reason, .. } => (false, format!("차단: {reason}")),
        Outcome::AwaitingField { reason, .. } => (true, reason.clone()),
        Outcome::SupervisorReview { reason, .. } => (true, reason.clone()),
        Outcome::FieldRetry { reason, .. } => (false, reason.clone()),
    }
}

#[derive(Debug, Serialize)]
struct DecisionReply {
    ok: bool,
    message: String,
    finalized: bool,
}

/// 교대감독 결정 (콘솔 창 전용). 가드레일이 막으면 사유를 돌려줍니다.
#[tauri::command(async)]
fn decide_incident(window: Window, app: AppHandle, desk: State<'_, Desk>, input: DecisionInput) -> Result<DecisionReply, String> {
    console_only(&window)?;
    if input.supervisor_id.trim().is_empty() {
        return Err("교대감독 사번을 머리에 적어 주세요".to_string());
    }
    let decision = match input.action.as_str() {
        "confirm" => SupervisorDecision::Confirm,
        "approve_procedure" => SupervisorDecision::ApproveHighRisk(Justification::ApplyProcedure { team_lead_cosign: input.team_lead_cosign.unwrap_or(false), procedure_ref: input.procedure_ref.clone().unwrap_or_default() }),
        "approve_false_positive" => SupervisorDecision::ApproveHighRisk(Justification::ProveFalsePositive { evidence_scans: input.evidence_scans.unwrap_or(0) }),
        "cancel" => SupervisorDecision::ApproveHighRisk(Justification::CancelMistake),
        "recheck" => SupervisorDecision::RequestFieldRecheck { note: input.note.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "현장 계기를 다시 확인해 주세요".into()) },
        "reject" => SupervisorDecision::Reject,
        other => return Err(format!("모르는 결정입니다: {other}")),
    };
    let result = {
        let mut engine = lock(&desk.engine);
        engine.confirm(&input.id, input.supervisor_id.trim(), decision, Utc::now())
    };
    notify_console(&app);
    let outcome = result.map_err(|e| format!("{e:#}"))?;
    let finalized = matches!(outcome, Outcome::Finalized { .. } | Outcome::AutoFinalized { .. });
    if finalized || matches!(outcome, Outcome::AwaitingField { .. }) {
        notify_field_case(&app, &desk, &input.id);
    }
    let (ok, message) = outcome_text(&outcome);
    Ok(DecisionReply { ok, message, finalized })
}

#[derive(Debug, Deserialize)]
struct InterlockInput {
    id: String,
    supervisor_id: String,
    step: u32,
    /// 계측 조건 단계: 현장 무전 · 휴대 계측으로 받은 값
    value: Option<f64>,
    /// LOTO 단계: 교대감독의 현장 무전 확인으로 갈음 (사번 날인)
    #[serde(default)]
    radio: bool,
}

/// 선행 절차 인터록 단계 하나를 채웁니다 (콘솔 창 전용, Part 8-4). 순서를 건너뛰면 가드레일이 막고 감사 기록에 남깁니다.
#[tauri::command(async)]
fn record_interlock(window: Window, app: AppHandle, desk: State<'_, Desk>, input: InterlockInput) -> Result<DecisionReply, String> {
    console_only(&window)?;
    let supervisor = input.supervisor_id.trim();
    if supervisor.is_empty() {
        return Err("교대감독 사번을 머리에 적어 주세요".to_string());
    }
    let evidence = match (input.radio, input.value) {
        (true, _) => Evidence::RadioLoto { supervisor: supervisor.to_string() },
        (false, Some(value)) => Evidence::Value { value, how: "교대감독 입력 (현장 무전)".into() },
        (false, None) => return Err("값을 넣어 주세요".to_string()),
    };
    let result = lock(&desk.engine).record_interlock(&input.id, input.step, evidence, supervisor, Utc::now());
    notify_console(&app);
    let outcome = result.map_err(|e| format!("{e:#}"))?;
    let (ok, message) = outcome_text(&outcome);
    Ok(DecisionReply { ok, message, finalized: false })
}

/// 자연어 검색 (콘솔 창 전용). 원장 · 진행 중 사건 · 보관 문서를 찾고, 계측 조건은 지금 값(물리엔진 · 재생 중 시나리오)으로 봅니다.
#[tauri::command(async)]
fn search(window: Window, desk: State<'_, Desk>, query: String) -> Result<blueprint_core::search::SearchResult, String> {
    console_only(&window)?;
    let live = physics_mode::live_snapshot(&desk).or_else(|| lock(&desk.live).as_ref().map(|l| (l.telemetry.clone(), l.sim_now().timestamp_millis())));
    // 엔진이 사건을 모델로 분석하는 동안(엔진 잠금)은 기다리지 않습니다. 그때는 진행 중 사건 · 보관 문서만 빼고 찾습니다.
    let engine = match desk.engine.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(p)) => Some(p.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    };
    let corpus = blueprint_core::search::Corpus::build(&desk.db, live.as_ref().map(|(t, at)| (t, *at)), engine.as_ref().map(|e| &e.store));
    let mut result = blueprint_core::search::search(&desk.db, &corpus, &query);
    if engine.is_none() {
        result.notes.push("엔진이 사건을 분석 중이라 이번 검색에는 진행 중 사건 · 보관 문서가 빠졌습니다".into());
    }
    Ok(result)
}

/// 작업자 실명 열람 요청 (콘솔 창 전용). 허용 · 거부 모두 감사 기록에 남습니다.
#[tauri::command(async)]
fn request_unmask(window: Window, desk: State<'_, Desk>, id: String, supervisor_id: String) -> Result<Option<Vec<String>>, String> {
    console_only(&window)?;
    let mut engine = lock(&desk.engine);
    engine.request_unmask(&id, supervisor_id.trim(), Utc::now()).map_err(|e| format!("{e:#}"))
}

#[derive(Debug, Deserialize)]
struct AutomationInput {
    on: bool,
    approver_id: String,
    legal_basis: String,
}

#[tauri::command(async)]
fn set_automation(window: Window, app: AppHandle, desk: State<'_, Desk>, input: AutomationInput) -> Result<(), String> {
    console_only(&window)?;
    let mut engine = lock(&desk.engine);
    if input.on {
        engine.enable_automation(&input.approver_id, &input.legal_basis, Utc::now()).map_err(|e| format!("{e:#}"))?;
    } else {
        engine.disable_automation(&input.approver_id, Utc::now());
    }
    drop(engine);
    notify_console(&app);
    Ok(())
}

#[derive(Debug, Serialize)]
struct ConsoleDto {
    automation: bool,
    engine: String,
    pending: usize,
    archived: usize,
    graph_nodes: usize,
    audit_count: usize,
    audit_last: Vec<String>,
    overlays: Value,
    live_scenario: Option<String>,
}

#[tauri::command(async)]
fn console_status(window: Window, desk: State<'_, Desk>) -> Result<ConsoleDto, String> {
    console_only(&window)?;
    let engine = lock(&desk.engine);
    let audit = engine.store.audit().events();
    Ok(ConsoleDto {
        automation: engine.policy.mode() == OperatingMode::Auto,
        engine: lock(&desk.status).clone(),
        pending: engine.store.active_count(),
        archived: engine.store.archive().len(),
        graph_nodes: desk.db.node_count(),
        audit_count: audit.len(),
        audit_last: audit.iter().rev().take(6).map(|e| format!("{} {:?} · {}", plant::local(e.at).format("%H:%M:%S"), e.actor, e.detail)).collect(),
        overlays: json!(engine.store.overlays()),
        live_scenario: lock(&desk.live).as_ref().map(|l| l.scenario.clone()),
    })
}

/// 판정 코드 · 우리말 이름 · 성격 (콘솔 창 전용)
#[tauri::command]
fn verdict_catalog(window: Window) -> Result<Vec<Value>, String> {
    console_only(&window)?;
    Ok(blueprint_core::verdict::ALL.iter().map(|v| json!({ "code": v.code(), "label": v.label(), "class": format!("{:?}", v.class()).to_lowercase() })).collect())
}

// ───────────────────────── 현장 단말 ─────────────────────────

#[tauri::command]
fn field_ready(desk: State<'_, Desk>) -> bool {
    engine_ready() && *lock(&desk.loaded)
}

#[derive(Debug, Serialize)]
struct SectorChoice {
    key: String,
    building: String,
    label: String,
    kks: String,
    blurb: String,
}

#[tauri::command]
fn field_sectors(window: Window, desk: State<'_, Desk>) -> Result<Vec<SectorChoice>, String> {
    field_only(&window)?;
    Ok(desk.db.sectors().iter().map(|s| SectorChoice { key: s.key.clone(), building: s.building.clone(), label: s.label.clone(), kks: s.kks.clone(), blurb: s.blurb.clone() }).collect())
}

#[tauri::command]
fn field_reset(window: Window, desk: State<'_, Desk>) -> Result<(), String> {
    field_only(&window)?;
    *lock(&desk.field_case) = None;
    lock(&desk.field_scans).clear();
    *lock(&desk.field_timings) = None;
    Ok(())
}

fn scanner_dir() -> PathBuf {
    std::env::var_os("BLUEPRINT_SCANNER_DIR").map(PathBuf::from).unwrap_or_else(|| app_dir().join("scanner"))
}

#[derive(Debug, Serialize)]
struct ScanReply {
    docs: Vec<String>,
    kind: Option<&'static str>,
    prefill: Value,
    /// 문서에서 읽은 항목과 원장으로 고친 항목
    scanned: Vec<&'static str>,
    corrected: Vec<&'static str>,
    signed: bool,
    unknown_pages: usize,
}

/// 스캐너 폴더의 이미지를 읽어 칸을 채웁니다 (현장 단말 전용). 읽은 이미지는 바로 지웁니다.
#[tauri::command(async)]
fn scan_field_documents(window: Window, desk: State<'_, Desk>) -> Result<ScanReply, String> {
    field_only(&window)?;
    let reader = lock(&desk.reader).clone().ok_or("단말을 준비하고 있습니다. 잠시만 기다려 주세요.")?;
    let dir = scanner_dir();
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|d| d.filter_map(|e| e.ok()).map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| matches!(x.to_string_lossy().to_lowercase().as_str(), "jpg" | "jpeg" | "png"))).collect())
        .unwrap_or_default();
    files.sort();
    if files.is_empty() {
        return Err(format!("스캐너에 문서가 없습니다. 점검표나 도면을 넣어 주세요. ({})", dir.display()));
    }
    let mut images = Vec::new();
    let mut hashes = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).map_err(|e| e.to_string())?;
        hashes.push(salted_hash(b"scan", &String::from_utf8_lossy(&bytes)));
        images.push(image::load_from_memory(&bytes).map_err(|e| format!("이미지를 읽을 수 없습니다: {e}"))?.to_rgb8());
    }
    let pages = {
        let _gpu = lock(&desk.gpu);
        reader.read_pages(&images).map_err(|e| {
            eprintln!("[스캔] 읽기 실패: {e:#}");
            "문서를 읽지 못했습니다. 주제어실에 알려 주세요.".to_string()
        })?
    };
    let mut timings = ReadTimings::default();
    let found: Vec<Extracted> = pages
        .iter()
        .zip(&hashes)
        .zip(&images)
        .map(|((p, h), img)| {
            timings.layout_ms += p.timings.layout_ms;
            timings.siglip2_ms += p.timings.siglip2_ms;
            timings.ocr_ms += p.timings.ocr_ms;
            timings.crops += p.crops.len();
            timings.dropped += p.plan.dropped.len();
            extract(&PageText::from_reading(h, img.dimensions(), p), &desk.db)
        })
        .collect();
    for f in &files {
        let _ = std::fs::remove_file(f);
    }
    let unknown_pages = found.iter().filter(|x| x.kind == Some(DocKind::Unknown)).count();
    let main = found.iter().find(|x| matches!(x.kind, Some(DocKind::Redline))).or_else(|| found.iter().find(|x| matches!(x.kind, Some(DocKind::GaugeCard)))).cloned();
    let mut scanned = Vec::new();
    let mut corrected = Vec::new();
    let mut prefill = serde_json::Map::new();
    if let Some(x) = &main {
        let mut put = |key: &'static str, v: Option<Value>| {
            if let Some(v) = v {
                prefill.insert(key.to_string(), v);
                scanned.push(key);
            }
        };
        put("sector", x.sector.map(|s| json!(s.key())));
        put("kks", x.kks.clone().map(Value::from));
        put("tag", x.tag.clone().map(Value::from));
        put("reading", x.reading.map(|v| json!(v)));
        put("unit", x.unit.clone().map(Value::from));
        put("moc_no", x.moc_no.clone().map(Value::from));
        put("old_value", x.old_value.map(|v| json!(v)));
        put("worker_name", x.worker_name.clone().map(Value::from));
        put("worker_id", x.worker_id.clone().map(Value::from));
        put("drawing", x.drawing.clone().map(Value::from));
        put("drawing_rev", x.drawing_rev.clone().map(Value::from));
        put("taken_at", x.taken_at.map(|t| json!(plant::local(t).format("%Y-%m-%d %H:%M:%S").to_string())));
        for c in &x.corrections {
            corrected.push(match c.field.as_str() {
                "설비 KKS" => "kks",
                "센서 태그" => "tag",
                _ => "moc_no",
            });
        }
    }
    let docs = found.iter().filter_map(|x| x.kind.map(|k| k.label().to_string())).collect();
    let kind = main.as_ref().and_then(|x| x.kind).map(|k| if k == DocKind::Redline { "redline" } else { "gauge_card" });
    let signed = found.iter().any(|x| x.signed);
    *lock(&desk.field_timings) = Some(timings);
    lock(&desk.field_scans).extend(found);
    Ok(ScanReply { docs, kind, prefill: Value::Object(prefill), scanned, corrected, signed, unknown_pages })
}

#[derive(Debug, Deserialize)]
struct SignatureInput {
    stroke_count: u32,
    bbox_area_ratio: f32,
    single_line: bool,
}

#[derive(Debug, Deserialize)]
struct FieldInput {
    kind: String,
    sector: Option<String>,
    kks: String,
    tag: Option<String>,
    reading: Option<String>,
    unit: Option<String>,
    moc_no: Option<String>,
    old_value: Option<String>,
    worker_name: String,
    worker_id: String,
    signature: Option<SignatureInput>,
    paper_signed: bool,
    permit_confirmed: bool,
    excuse: Option<String>,
    excuse_text: Option<String>,
    edited: Vec<String>,
}

/// 현장 단말이 받는 답. 내부 정보를 넣지 않습니다.
#[derive(Debug, Serialize)]
struct FieldReply {
    /// sent · finished · hold · invalid · sign_again · excuse
    state: &'static str,
    receipt_no: Option<String>,
    notice: Option<String>,
    message: Option<String>,
}

fn number(s: Option<&str>) -> Option<f64> {
    s.map(|x| x.replace(',', "")).and_then(|x| x.trim().parse().ok())
}

/// 현장 보고를 전송합니다 (현장 단말 전용).
#[tauri::command(async)]
fn submit_field_report(window: Window, app: AppHandle, desk: State<'_, Desk>, input: FieldInput) -> Result<FieldReply, String> {
    field_only(&window)?;
    let scans = lock(&desk.field_scans).clone();
    let main = scans.iter().find(|x| matches!(x.kind, Some(DocKind::Redline) | Some(DocKind::GaugeCard)));
    let kind = match input.kind.as_str() {
        "redline" => FieldKind::Redline,
        "manual" => FieldKind::Manual,
        _ => FieldKind::GaugeCard,
    };
    let live = lock(&desk.live).as_ref().map(|l| (l.telemetry.clone(), l.sim_now(), l.end));
    // 실측 시각: 점검표에 적힌 시각이 계측 기간 안이면 그대로, 아니면 시뮬레이터의 지금 시각입니다.
    let sim_now = live.as_ref().map(|l| l.1).unwrap_or_else(Utc::now);
    let scanned_time = main.and_then(|x| x.taken_at).filter(|t| live.as_ref().is_some_and(|l| l.0.start_ms().is_some_and(|s| t.timestamp_millis() >= s) && *t <= l.2));
    let excuse = match input.excuse.as_deref() {
        Some("transmitter_drift") => Some(DeviationExcuse::TransmitterDrift),
        Some("moc_change") => Some(DeviationExcuse::MocChange),
        Some("manual_bypass") => Some(DeviationExcuse::ManualBypass),
        Some("free_text") => Some(DeviationExcuse::FreeText(input.excuse_text.clone().unwrap_or_default())),
        _ => None,
    };
    let signature = if input.paper_signed && main.is_some_and(|x| x.signed) {
        Some(Signature::on_paper())
    } else {
        input.signature.map(|s| Signature { stroke_count: s.stroke_count, bbox_area_ratio: s.bbox_area_ratio, single_line: s.single_line })
    };
    let form = FieldForm {
        kind: Some(kind),
        sector: input.sector.as_deref().and_then(Sector::from_key),
        kks: input.kks.clone(),
        tag: input.tag.clone(),
        reading: number(input.reading.as_deref()),
        unit: input.unit.clone(),
        taken_at: Some(scanned_time.unwrap_or(sim_now)),
        drawing: main.and_then(|x| x.drawing.clone()),
        drawing_rev: main.and_then(|x| x.drawing_rev.clone()),
        moc_no: input.moc_no.clone(),
        old_value: number(input.old_value.as_deref()).or(main.and_then(|x| x.old_value)),
        worker_name: input.worker_name.clone(),
        worker_id: input.worker_id.clone(),
        signature,
        permit_confirmed: input.permit_confirmed,
        excuse,
        corrections: main.map(|x| x.corrections.clone()).unwrap_or_default(),
        uncertain: main.map(|x| x.uncertain.clone()).unwrap_or_default(),
        timings: *lock(&desk.field_timings),
        content_hash: None,
        edited: input.edited.clone(),
    };
    let result = {
        let _gpu = lock(&desk.gpu);
        let mut engine = lock(&desk.engine);
        // 자유 서술 소명은 Zoro-1 이 표준 소명으로 바꿉니다. 바꾸지 못하면 그대로 교대감독에게 갑니다.
        let mut form = form;
        if let (Some(DeviationExcuse::FreeText(text)), Some(writer)) = (form.excuse.clone(), engine.writer.as_deref()) {
            if let Ok(Some(std)) = blueprint_core::drafting::normalize_excuse(writer, &text) {
                form.excuse = Some(std);
            }
        }
        field::submit(&mut engine, &desk.db, &form, live.as_ref().map(|l| &l.0), sim_now)
    };
    notify_console(&app);
    let reply = match result {
        Ok(receipt) => {
            lock(&desk.field_scans).clear();
            *lock(&desk.field_timings) = None;
            match receipt.state {
                FieldState::Sent => {
                    *lock(&desk.field_case) = Some(receipt.receipt_no.clone());
                    FieldReply { state: "sent", receipt_no: Some(receipt.receipt_no), notice: None, message: None }
                }
                FieldState::Finished { notice } => FieldReply { state: "finished", receipt_no: Some(receipt.receipt_no), notice: Some(notice), message: None },
                FieldState::Hold { notice } => FieldReply { state: "hold", receipt_no: Some(receipt.receipt_no), notice: Some(notice), message: None },
            }
        }
        Err(FieldError::Form(m)) => FieldReply { state: "invalid", receipt_no: None, notice: None, message: Some(m) },
        Err(FieldError::Retry(m)) => FieldReply { state: "sign_again", receipt_no: None, notice: None, message: Some(m) },
        Err(FieldError::NeedExcuse { message }) => FieldReply { state: "excuse", receipt_no: None, notice: None, message: Some(message) },
        Err(FieldError::Internal(e)) => {
            eprintln!("[현장] 접수 실패: {e:#}");
            return Err("전송하지 못했습니다. 주제어실에 알려 주세요.".to_string());
        }
    };
    Ok(reply)
}

/// 현장 단말이 기다리는 건이 끝났거나 재점검이 요청되면 알립니다.
fn notify_field_case(app: &AppHandle, desk: &Desk, id: &str) {
    let mut current = lock(&desk.field_case);
    if current.as_deref() != Some(id) {
        return;
    }
    let (notice, done) = {
        let engine = lock(&desk.engine);
        match engine.store.archive().iter().find(|a| a.incident_id == id) {
            Some(a) => (a.field_notice.clone(), true),
            None => match engine.store.active(id).and_then(|s| s.field_request.clone()) {
                Some(note) => (format!("주제어실 재점검 요청 (접수 번호 {id})\n{note}"), false),
                None => return,
            },
        }
    };
    let _ = app.emit_to(FIELD, "field_update", json!({ "receipt_no": id, "notice": notice, "done": done }));
    if done {
        *current = None;
    }
}

// ───────────────────────── 시작 ─────────────────────────

/// 모델을 뒤에서 엽니다. 가중치가 준비될 때까지 기다리고, 열리면 엔진에 꽂습니다.
fn load_models(app: AppHandle) {
    let desk = app.state::<Desk>();
    let gpu_build = cfg!(any(feature = "vulkan", feature = "rocm", feature = "cuda", feature = "metal"));
    if !gpu_build || std::env::var("BLUEPRINT_ENGINE").is_ok_and(|v| v == "rules") {
        *lock(&desk.status) = "규칙만 (모델 없음)".to_string();
        *lock(&desk.loaded) = true;
        notify_field(&app);
        return;
    }
    while !engine_ready() {
        std::thread::sleep(std::time::Duration::from_secs(2));
    }
    match blueprint_core::ai::runtime::open() {
        Ok(rt) => {
            {
                let mut engine = lock(&desk.engine);
                engine.model = Some(Box::new(rt.model));
                engine.writer = Some(Box::new(rt.writer));
            }
            *lock(&desk.reader) = Some(rt.reader);
            *lock(&desk.status) = format!("Zoro-1 · {} (SharedResidency)", rt.backend);
            *lock(&desk.device) = Some(rt.device);
        }
        Err(e) => {
            eprintln!("[엔진] 모델을 열지 못했습니다: {e:#}");
            *lock(&desk.status) = "규칙만 (모델 열기 실패)".to_string();
        }
    }
    *lock(&desk.loaded) = true;
    notify_field(&app);
    notify_console(&app);
}

pub fn run() {
    let salt: Vec<u8> = (0..8).flat_map(|_| random_u32().to_le_bytes()).collect();
    let desk = Desk {
        engine: Mutex::new(Engine::new(&salt, AutomationPolicy::default(), None)),
        db: Lattice::embedded().expect("합성 원장을 읽을 수 없습니다"),
        scenarios: ScenarioSet::embedded().expect("합성 시나리오를 읽을 수 없습니다"),
        gpu: Mutex::new(()),
        reader: Mutex::new(None),
        status: Mutex::new("모델 여는 중".to_string()),
        loaded: Mutex::new(false),
        device: Mutex::new(None),
        live: Mutex::new(None),
        field_case: Mutex::new(None),
        field_scans: Mutex::new(Vec::new()),
        field_timings: Mutex::new(None),
        physics: Mutex::new(physics_mode::PhysicsState::default()),
    };
    tauri::Builder::default()
        .manage(Jobs::default())
        .manage(desk)
        .setup(|app| {
            let handle = app.handle().clone();
            std::thread::spawn(move || load_models(handle));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            check_model_status,
            import_model,
            ensure_models,
            download_model,
            delete_model,
            delete_all_models,
            plant_topology,
            drawing_image,
            scenario_list,
            inject_scenario,
            incident_list,
            incident_detail,
            archive_detail,
            mermaid_source,
            mining_overview,
            decide_incident,
            record_interlock,
            search,
            physics_mode::physics_start,
            physics_mode::physics_stop,
            physics_mode::physics_inject,
            physics_mode::physics_execute,
            physics_mode::physics_status,
            request_unmask,
            set_automation,
            console_status,
            verdict_catalog,
            field_ready,
            field_sectors,
            field_reset,
            scan_field_documents,
            submit_field_report
        ])
        .build(tauri::generate_context!())
        .expect("앱을 시작할 수 없습니다")
        .run(|app, event| {
            // ROCm 프로세스는 DLL 을 내리다 멈출 수 있습니다. GPU 를 비우고 바로 끝냅니다.
            if let RunEvent::Exit = event {
                let desk = app.state::<Desk>();
                let device = lock(&desk.device).clone();
                if let Some(device) = device {
                    accel::finish_process(&device, 0);
                }
            }
        });
}
