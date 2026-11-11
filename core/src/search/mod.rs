//! 자연어 검색 (logis-center `ai_search_complex` 의 설계를 발전소 원장에 옮김). 한국어 · 영어 질의를 엔진이 그대로 읽습니다.
//!
//! 주제어실 검색창에 "103동 2층 밸브", "과열기 온도 600도 이상", "급수조절밸브 상류 2홉", "승인 대기 MOC",
//! "valves on floor 2 of building 103", "boiler feed pump" 처럼 씁니다. logis-center 에서 실측으로 얻은 규칙을 그대로 지킵니다 (`docs/search.md`).
//! 1. 넓게 찾고, 조건은 따로 둡니다. 범위(유형)만으로 후보를 모으고 조건은 필터 계획으로 나중에 겁니다.
//! 2. 조건이 정답을 지우면 안 됩니다. 조건마다 원장에 맞는 항목이 하나라도 있는지 대조하고(grounding), 없으면 힌트(가점)로 내립니다.
//!    조건이 하나도 없는 계획은 모두 통과시키고, 조건에 맞는 항목은 전체 표에서 한 번 더 찾아(구제) 뒤에 붙입니다.
//! 3. 질의 전체 경로를 파서와 나란히 돌리고 순위로 합칩니다 (RRF k = 20, 글자 일치 ×2). 점수는 경로마다 척도가 달라 더하지 않습니다.
//! 4. 유형을 먼저 정합니다. 고정 지시어로 정하고 없으면 설비가 기본이며, 유형 제한 경로를 따로 둡니다.
//! 5. 품사 분석은 낱말을 지우는 데가 아니라 꼴을 더하는 데 씁니다 (`analyze`): Stanza 원형으로 조사 · 어미를 뗀 줄기(`급수펌프의` → `급수펌프`),
//!    영어 원형(`valves` → `valve`)을 원문과 함께 맞추고, 기능어는 글자 경로 · NMS 조각에서만 뺍니다.
//!    숫자 · 비교어 · 상태어는 고정 낱말 목록(한국어 · 영어, `lexicon`)으로 코드가 뽑습니다 (모델 아님).
//! 6. 부정어(아닌 · 제외 · 빼고 · 없는 / not · no · except · without)가 붙은 상태어는 상태 필터가 되지 않습니다.
//! 7. 식별자(KKS · 태그 · MOC · 도면 · 사건 번호)는 하이픈째 통째로 두고, KKS 오독은 원장으로 교정합니다 (편집 거리 2, 유일할 때만).
//! 8. 모든 결정에 사유를 남깁니다 (화면의 "왜 찾았나").
//! 9. 임베딩 모델(granite-embedding-97m, 다국어)이 있으면 질의 벡터 · 유형 제한 벡터 경로와, 낱말 창을 원장 개념에 대는 NMS 조각
//!    (`nms`, logis-center Stage-1)을 더합니다. 조각은 조건을 걸지 않고 순위 경로 · 가점 · 관계 질의의 기준 설비로만 씁니다.
//!
//! 판단(생성) 모델은 쓰지 않습니다. 모델이 하나도 없어도 규칙 · 글자 경로만으로 돕니다 (CI · 모델 없는 PC).

pub mod analyze;
pub mod lang;
pub mod lexicon;
pub mod ml;
pub mod models;
pub mod nms;

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

pub use analyze::{Analysis, QueryToken};
pub use models::SearchModels;
pub use nms::{Candidate, Segment, Target};

use crate::lattice::{KksMatch, Lattice};
use crate::plant::fmt_value;
use crate::store::Store;
use crate::timeseries::{classify_with, Baseline, SignalState, Telemetry};

/// RRF 상수와 경로 가중치 (logis-center `search_fusion.rs` 와 같음)
pub const FUSION_K: f32 = 20.0;
pub const LITERAL_WEIGHT: f32 = 2.0;
/// 화면에 내는 최대 개수
pub const LIMIT: usize = 30;
/// 질의 벡터 경로에 넣는 상위 개수 (logis-center 는 200, 이 원장은 100여 건이라 20)
pub const VECTOR_TOP: usize = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DocKind {
    Equipment,
    Sensor,
    Incident,
    Archive,
    Aop,
    Moc,
    Drawing,
}

impl DocKind {
    pub fn label(self) -> &'static str {
        match self {
            DocKind::Equipment => "설비",
            DocKind::Sensor => "계측",
            DocKind::Incident => "사건",
            DocKind::Archive => "보관",
            DocKind::Aop => "절차서",
            DocKind::Moc => "MOC",
            DocKind::Drawing => "도면",
        }
    }
}

/// 유형 지시어. 한글은 부분 일치, 영문은 접두 일치입니다.
const TYPE_WORDS: [(DocKind, &[&str]); 6] = [
    (DocKind::Sensor, &["센서", "계측", "태그", "전송기", "계기"]),
    (DocKind::Incident, &["사건", "알람 목록", "경보 목록", "진행 중", "inc-"]),
    (DocKind::Archive, &["보관", "확정된", "처리 완료", "처리된", "이력", "mcr-", "noc-"]),
    (DocKind::Aop, &["절차서", "aop", "eop", "조치 절차", "대응 절차"]),
    (DocKind::Moc, &["moc", "mop", "변경관리", "레드라인", "red-line", "변경 승인"]),
    (DocKind::Drawing, &["도면", "p&id", "pid-", "mep-", "sld-", "net-", "계통도", "결선도"]),
];

/// 설비 종류 낱말 → 원장 `kind` (뒤 열넷은 AI 데이터센터 원장의 종류)
const KIND_WORDS: [(&str, &[&str]); 33] = [
    ("밸브", &["valve"]),
    ("펌프", &["pump"]),
    ("팬", &["fan"]),
    ("통풍기", &["fan"]),
    ("터빈", &["turbine"]),
    ("베어링", &["bearing"]),
    ("차단기", &["breaker"]),
    ("변압기", &["transformer"]),
    ("과열기", &["superheater"]),
    ("재열기", &["reheater"]),
    ("가열기", &["heater"]),
    ("탈기기", &["deaerator"]),
    ("복수기", &["condenser"]),
    ("냉각기", &["cooler"]),
    ("발전기", &["generator"]),
    ("버너", &["burner"]),
    ("집진기", &["esp"]),
    ("연돌", &["stack"]),
    ("gpu", &["gpu_rack"]),
    ("랙", &["gpu_rack"]),
    ("매니폴드", &["manifold"]),
    ("열교환기", &["exchanger"]),
    ("칠러", &["chiller"]),
    ("냉각탑", &["fan"]),
    ("항온항습기", &["crah"]),
    ("공조기", &["crah"]),
    ("ups", &["ups"]),
    ("배터리", &["battery"]),
    ("pdu", &["pdu"]),
    ("스위치", &["switch"]),
    ("트랜시버", &["transceiver"]),
    ("스토리지", &["storage"]),
    ("탱크", &["tank"]),
];

/// 계측 종류 낱말 (센서 라벨 · 단위로 맞춥니다)
const MEASURE_WORDS: [(&str, &[&str]); 10] = [
    ("온도", &["℃", "°c"]),
    ("압력", &["mpa", "kpa", "bar"]),
    ("유량", &["t/h", "lpm"]),
    ("진동", &["μm", "um", "mm/s"]),
    ("개도", &["%"]),
    ("전류", &["a"]),
    ("수위", &["%"]),
    ("출력", &["mw"]),
    ("전력", &["kw", "mw"]),
    ("농도", &["ppm"]),
];

/// 질적 상태 조건 ("압력 높은", "고착", "비정상"). 지금 계측(물리엔진 · 시나리오 재생)이 있을 때만 판정하고, 없으면 힌트로 내립니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Quality {
    High,
    Low,
    Rising,
    Falling,
    Stuck,
    Frozen,
    Abnormal,
}

impl Quality {
    fn label(self) -> &'static str {
        match self {
            Quality::High => "높음",
            Quality::Low => "낮음",
            Quality::Rising => "상승 중",
            Quality::Falling => "하강 중",
            Quality::Stuck => "고착",
            Quality::Frozen => "동결",
            Quality::Abnormal => "비정상",
        }
    }

    /// 높음 · 낮음은 경보 쪽이거나 정상치에서 여유(데드밴드 ×2 와 정상치 2 % 중 큰 값)보다 벗어났을 때,
    /// 상승 · 하강 · 고착 · 동결은 신호 모양(`timeseries::classify`)으로, 비정상은 그중 하나라도일 때입니다.
    fn holds(self, t: &SignalTag) -> bool {
        let dev = t.value - t.normal;
        match self {
            Quality::High => t.side == Some(true) || dev > t.margin,
            Quality::Low => t.side == Some(false) || -dev > t.margin,
            Quality::Rising => t.state == Some(SignalState::SlewRise),
            Quality::Falling => t.state == Some(SignalState::SlewDrop),
            Quality::Stuck => matches!(t.state, Some(SignalState::StuckLow | SignalState::StuckHigh)),
            Quality::Frozen => t.state == Some(SignalState::Flatline),
            Quality::Abnormal => t.side.is_some() || dev.abs() > t.margin || t.state.is_some_and(|s| s != SignalState::Stable),
        }
    }
}

/// 질적 상태 낱말. 한 글자 낱말(높 · 낮)은 앞머리 일치, 나머지는 낱말 안 어디든 ("온도상승", "고착된").
/// "이상" 은 비교어("600도 이상")라서 "이상한" · "이상징후" 만 비정상입니다.
const QUALITY_WORDS: [(&str, Quality); 21] = [
    ("높", Quality::High),
    ("과다", Quality::High),
    ("낮", Quality::Low),
    ("저하", Quality::Low),
    ("부족", Quality::Low),
    ("상승", Quality::Rising),
    ("오르", Quality::Rising),
    ("올라", Quality::Rising),
    ("증가", Quality::Rising),
    ("하강", Quality::Falling),
    ("하락", Quality::Falling),
    ("떨어", Quality::Falling),
    ("감소", Quality::Falling),
    ("내려", Quality::Falling),
    ("고착", Quality::Stuck),
    ("멈춘", Quality::Stuck),
    ("동결", Quality::Frozen),
    ("비정상", Quality::Abnormal),
    ("이상한", Quality::Abnormal),
    ("이상징후", Quality::Abnormal),
    ("문제", Quality::Abnormal),
];

fn quality_of(w: &str) -> Option<Quality> {
    QUALITY_WORDS.iter().find(|(k, _)| if k.chars().count() == 1 { w.starts_with(k) } else { w.contains(k) }).map(|x| x.1)
}

/// 지금 계측 한 점 (설비 · 센서 문서에 붙음)
#[derive(Debug, Clone, PartialEq)]
struct SignalTag {
    label: String,
    unit: String,
    value: f64,
    normal: f64,
    margin: f64,
    /// 경보 쪽 (`Some(true)` 상한 · `Some(false)` 하한)
    side: Option<bool>,
    state: Option<SignalState>,
}

const NEGATIONS: [&str; 7] = ["아닌", "제외", "빼고", "없는", "말고", "않은", "아니"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Gte,
    Gt,
    Lte,
    Lt,
}

impl Op {
    fn holds(self, v: f64, x: f64) -> bool {
        match self {
            Op::Gte => v >= x,
            Op::Gt => v > x,
            Op::Lte => v <= x,
            Op::Lt => v < x,
        }
    }

    fn text(self) -> &'static str {
        match self {
            Op::Gte => "≥",
            Op::Gt => ">",
            Op::Lte => "≤",
            Op::Lt => "<",
        }
    }
}

/// 비교어 (정확한 낱말 먼저)
const COMPARATORS: [(&str, Op); 9] = [("이상", Op::Gte), ("최소", Op::Gte), ("넘는", Op::Gt), ("초과", Op::Gt), ("이하", Op::Lte), ("이내", Op::Lte), ("최대", Op::Lte), ("미만", Op::Lt), ("밑", Op::Lt)];

/// 검색 대상 한 건
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Doc {
    pub kind: DocKind,
    pub id: String,
    pub title: String,
    pub subtitle: String,
    /// 관련 설비 (누르면 세 뷰가 이 설비로)
    pub kks: Option<String>,
    #[serde(skip)]
    text: String,
    #[serde(skip)]
    sector: Option<String>,
    #[serde(skip)]
    floor: Option<String>,
    #[serde(skip)]
    kinds: Vec<String>,
    /// (라벨, 단위, 값)
    #[serde(skip)]
    measures: Vec<(String, String, Option<f64>)>,
    /// 경보 · 트립 · 정상 (계측값이 있을 때)
    #[serde(skip)]
    level: Option<&'static str>,
    #[serde(skip)]
    status: Option<String>,
    /// 지금 계측의 값 · 모양 (질적 상태 조건용)
    #[serde(skip)]
    signals: Vec<SignalTag>,
    /// 임베딩에 넣는 글 (한국어 이름 · 영어 이름). 지금 계측 값을 넣지 않아 캐시가 그대로 맞습니다.
    #[serde(skip)]
    embed_text: String,
    /// 영어 제목 (en.json). 영어 질의의 n-gram 경로가 봅니다
    #[serde(skip)]
    title_en: String,
    /// 글의 영어 낱말 (영어 질의 낱말은 부분 문자열이 아니라 낱말로 맞춥니다: 'id' 가 'liquid' 에 걸리지 않게)
    #[serde(skip)]
    words: BTreeSet<String>,
}

/// 검색 표 (원장 + 진행 중 사건 · 보관 · 지금 계측)
#[derive(Debug, Clone, Default)]
pub struct Corpus {
    pub docs: Vec<Doc>,
    df: HashMap<String, usize>,
    /// 지금 계측이 있는가 (없으면 질적 상태 조건은 힌트로만)
    pub live: bool,
}

fn level_of(s: &crate::lattice::SensorRow, v: f64) -> &'static str {
    if s.tripped(v) {
        "trip"
    } else if s.alarm_side(v).is_some() {
        "alarm"
    } else {
        "normal"
    }
}

fn worst(a: Option<&'static str>, b: &'static str) -> Option<&'static str> {
    let rank = |x: &str| match x {
        "trip" => 2,
        "alarm" => 1,
        _ => 0,
    };
    match a {
        Some(x) if rank(x) >= rank(b) => Some(x),
        _ => Some(b),
    }
}

impl Corpus {
    /// `live` 가 있으면 계측 조건 · 상태를 그 시각 값으로, 없으면 정상치로 봅니다.
    pub fn build(db: &Lattice, live: Option<(&Telemetry, i64)>, store: Option<&Store>) -> Self {
        let value = |tag: &str, normal: f64| live.and_then(|(t, at)| t.value_at(tag, at)).or(if live.is_none() { Some(normal) } else { None });
        let signal_of = |s: &crate::lattice::SensorRow| -> Option<SignalTag> {
            let (t, at) = live?;
            let series = t.get(&s.tag)?;
            let value = series.value_at(at)?;
            Some(SignalTag {
                label: s.label.clone(),
                unit: s.unit.clone(),
                value,
                normal: s.normal,
                margin: (2.0 * s.deadband).max(0.02 * s.normal.abs()),
                side: s.alarm_side(value),
                // 기준은 원장 정상치입니다. 물리엔진 링버퍼는 60초 창이 미끄러져서 "처음 20초" 가 고장 뒤 값이 될 수 있습니다.
                state: classify_with(s, series, at, Baseline { mean: s.normal, std: 0.0 }, series.t0_ms).map(|r| r.state),
            })
        };
        let names = lexicon::names();
        let en = |ko: &str| names.en(ko).unwrap_or("").to_string();
        let mut docs = Vec::new();
        for e in db.equipment_all() {
            let a = db.address(&e.kks);
            let place = a.as_ref().map(|x| x.place()).unwrap_or_default();
            let sector = db.sector(&e.sector);
            let mut measures = Vec::new();
            let mut level = None;
            let mut text = format!("{} {} {} {} {} {} {}", e.kks, e.name, e.kind, place, e.drawing, sector.map(|s| s.label.as_str()).unwrap_or(""), sector.map(|s| s.building.as_str()).unwrap_or(""));
            for s in db.sensors_of(&e.kks) {
                let v = value(&s.tag, s.normal);
                if let (Some(v), true) = (v, live.is_some()) {
                    level = worst(level, level_of(s, v));
                }
                measures.push((s.label.clone(), s.unit.clone(), v));
                text.push_str(&format!(" {} {}", s.tag, s.label));
            }
            // 영어 이름 · 동 이름도 글자 경로에 (영어 질의가 엔진에 그대로 옵니다)
            text.push_str(&format!(" {}", en(&e.name)));
            let embed_text = format!("{} · {}", e.name, en(&e.name));
            docs.push(Doc {
                kind: DocKind::Equipment,
                id: e.kks.clone(),
                title: e.name.clone(),
                subtitle: format!("{place} · {}", e.drawing),
                kks: Some(e.kks.clone()),
                text,
                sector: Some(e.sector.clone()),
                floor: Some(e.floor.clone()),
                kinds: vec![e.kind.clone()],
                measures,
                level,
                status: None,
                signals: db.sensors_of(&e.kks).into_iter().filter_map(signal_of).collect(),
                embed_text,
                title_en: en(&e.name),
                words: BTreeSet::new(),
            });
        }
        for s in db.sensors() {
            let Some(e) = db.equipment(&s.kks) else { continue };
            let v = value(&s.tag, s.normal);
            let limits = [s.alarm_low.map(|x| format!("경보 {}", fmt_value(x))), s.alarm_high.map(|x| format!("경보 {}", fmt_value(x))), s.trip_high.or(s.trip_low).map(|x| format!("트립 {}", fmt_value(x)))];
            docs.push(Doc {
                kind: DocKind::Sensor,
                id: s.tag.clone(),
                title: format!("{} {}", e.name, s.label),
                subtitle: format!("{} · 정상 {} {} · {}", db.address(&e.kks).map(|a| a.place()).unwrap_or_default(), fmt_value(s.normal), s.unit, limits.iter().flatten().cloned().collect::<Vec<_>>().join(" · ")),
                kks: Some(e.kks.clone()),
                text: format!("{} {} {} {} {} {} {} {} {}", s.tag, s.label, s.unit, e.kks, e.name, e.kind, db.address(&e.kks).map(|a| a.place()).unwrap_or_default(), en(&e.name), en(&s.label)),
                sector: Some(e.sector.clone()),
                floor: Some(e.floor.clone()),
                kinds: vec![e.kind.clone()],
                measures: vec![(s.label.clone(), s.unit.clone(), v)],
                level: v.filter(|_| live.is_some()).map(|v| level_of(s, v)),
                status: None,
                signals: signal_of(s).into_iter().collect(),
                embed_text: format!("{} {} · {} {}", e.name, s.label, en(&e.name), en(&s.label)),
                title_en: format!("{} {}", en(&e.name), en(&s.label)).trim().to_string(),
                words: BTreeSet::new(),
            });
        }
        for a in db.aops() {
            docs.push(Doc {
                kind: DocKind::Aop,
                id: a.id.clone(),
                title: a.title.clone(),
                subtitle: format!("{}{}", a.steps.first().cloned().unwrap_or_default(), if a.high_risk { " · 고위험 (공동 결재)" } else { "" }),
                kks: a.triggers.first().and_then(|t| db.sensor(t)).map(|s| s.kks.clone()),
                text: format!("{} {} {} {} {} {}", a.id, a.title, a.steps.join(" "), a.risk_actions.join(" "), a.triggers.join(" "), en(&a.title)),
                sector: a.sectors.first().cloned(),
                floor: None,
                kinds: Vec::new(),
                measures: Vec::new(),
                level: None,
                status: Some(if a.high_risk { "고위험".into() } else { "일반".into() }),
                signals: Vec::new(),
                embed_text: format!("{} · {} · {}", a.title, en(&a.title), a.steps.first().cloned().unwrap_or_default()),
                title_en: en(&a.title),
                words: BTreeSet::new(),
            });
        }
        for m in db.mocs() {
            let status = match m.status.as_str() {
                "approved" => "승인",
                "pending" => "승인 대기",
                "closed" => "종결",
                other => other,
            };
            docs.push(Doc {
                kind: DocKind::Moc,
                id: m.moc_no.clone(),
                title: format!("{} {} → {} {}", m.param, fmt_value(m.old), fmt_value(m.new), m.unit),
                subtitle: format!("{} · {} · {}", m.tag, status, m.reason),
                kks: Some(m.kks.clone()),
                text: format!("{} {} {} {} {} {} {} {}", m.moc_no, m.kks, m.tag, m.param, m.reason, status, m.status, en(&m.reason)),
                sector: db.equipment(&m.kks).map(|e| e.sector.clone()),
                floor: db.equipment(&m.kks).map(|e| e.floor.clone()),
                kinds: db.equipment(&m.kks).map(|e| vec![e.kind.clone()]).unwrap_or_default(),
                measures: vec![(m.param.clone(), m.unit.clone(), Some(m.new))],
                level: None,
                status: Some(status.to_string()),
                signals: Vec::new(),
                embed_text: format!("{} {} → {} {} · {} · {status}", m.param, fmt_value(m.old), fmt_value(m.new), m.unit, m.reason),
                title_en: String::new(),
                words: BTreeSet::new(),
            });
        }
        for d in db.drawings() {
            let sector = db.sector(&d.sector);
            docs.push(Doc {
                kind: DocKind::Drawing,
                id: d.id.clone(),
                title: d.title.clone(),
                subtitle: format!("{} · {}", d.rev, sector.map(|s| format!("{} {}", s.building, s.label)).unwrap_or_default()),
                kks: db.on_drawing(&d.id).first().map(|e| e.kks.clone()),
                text: format!("{} {} {} {} {}", d.id, d.title, d.rev, sector.map(|s| format!("{} {}", s.building, s.label)).unwrap_or_default(), en(&d.title)),
                sector: Some(d.sector.clone()),
                floor: None,
                kinds: Vec::new(),
                measures: Vec::new(),
                level: None,
                status: None,
                signals: Vec::new(),
                embed_text: format!("{} · {} · {}", d.title, en(&d.title), sector.map(|s| format!("{} {}", s.building, s.label)).unwrap_or_default()),
                title_en: en(&d.title),
                words: BTreeSet::new(),
            });
        }
        if let Some(store) = store {
            for s in store.sessions() {
                let a = &s.analysis;
                let e = db.equipment(&a.route.kks);
                docs.push(Doc {
                    kind: DocKind::Incident,
                    id: s.incident.id.clone(),
                    title: format!("{} {}", e.map(|x| x.name.as_str()).unwrap_or(""), a.candidate.label()),
                    subtitle: s.reason.clone(),
                    kks: Some(a.causal_root().map(|r| r.kks.clone()).unwrap_or_else(|| a.route.kks.clone())),
                    text: format!("{} {} {} {} {} {}", s.incident.id, a.route.kks, e.map(|x| x.name.as_str()).unwrap_or(""), a.candidate.code(), a.candidate.label(), s.reason),
                    sector: e.map(|x| x.sector.clone()),
                    floor: e.map(|x| x.floor.clone()),
                    kinds: e.map(|x| vec![x.kind.clone()]).unwrap_or_default(),
                    measures: Vec::new(),
                    level: None,
                    status: Some("진행 중".into()),
                    signals: Vec::new(),
                    embed_text: format!("{} {} · {}", e.map(|x| x.name.as_str()).unwrap_or(""), a.candidate.label(), s.reason),
                    title_en: e.map(|x| en(&x.name)).unwrap_or_default(),
                    words: BTreeSet::new(),
                });
            }
            for r in store.archive() {
                docs.push(Doc {
                    kind: DocKind::Archive,
                    id: r.archive_id.clone(),
                    title: r.title.clone(),
                    subtitle: format!("{} · {}", r.final_code, crate::plant::local(r.closed_at).format("%m-%d %H:%M")),
                    kks: None,
                    text: format!("{} {} {} {}", r.archive_id, r.incident_id, r.title, r.final_code),
                    sector: None,
                    floor: None,
                    kinds: Vec::new(),
                    measures: Vec::new(),
                    level: None,
                    status: Some("확정".into()),
                    signals: Vec::new(),
                    embed_text: r.title.clone(),
                    title_en: String::new(),
                    words: BTreeSet::new(),
                });
            }
        }
        let mut df = HashMap::new();
        for d in &mut docs {
            d.text = d.text.to_lowercase();
            // 영어 이름이 없는 원장(영어 · 일본어 · 독일어 원장)은 제목 그대로 (영어 원장은 제목이 곧 영어)
            d.title_en = if d.title_en.trim().is_empty() { d.title.to_lowercase() } else { d.title_en.to_lowercase() };
            // 낱말 (악센트를 뗀 꼴). 영어 낱말은 예전처럼 로마자 낱말 그대로, 다른 라틴 문자 낱말(Pumpe · válvula)도 낱말로 맞춥니다.
            d.words = lang::fold(&d.text).split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty() && w.chars().any(|c| c.is_alphabetic())).map(str::to_string).collect();
            let terms: BTreeSet<String> = terms_of(&d.text).into_iter().collect();
            for t in terms {
                *df.entry(t).or_insert(0) += 1;
            }
        }
        Self { docs, df, live: live.is_some() }
    }
}

/// 질의 낱말: 공백으로 나누고 가장자리 문장부호만 떼고, 두 글자 이상. 하이픈 식별자는 통째로, 쉼표 숫자는 숫자만도 더합니다.
pub fn terms_of(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for raw in text.split_whitespace() {
        let w = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '-' && c != '&' && c != '%').to_lowercase();
        if w.chars().count() < 2 {
            continue;
        }
        if w.contains(',') || w.chars().any(|c| c.is_ascii_digit()) && w.contains(',') {
            out.push(w.replace(',', ""));
        }
        out.push(w);
    }
    out.dedup();
    out
}

fn bigrams(text: &str) -> BTreeSet<String> {
    let chars: Vec<char> = text.chars().filter(|c| !c.is_whitespace() && !"·()[],.".contains(*c)).collect();
    chars.windows(2).map(|w| w.iter().collect()).collect()
}

/// 조건 한 개와 그 처리 결과 (화면 · 기록용)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Condition {
    pub kind: &'static str,
    pub text: String,
    /// 원장에 맞는 항목이 있어 필터로 걸렸는가 (아니면 힌트로 내림)
    pub applied: bool,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq)]
enum Cond {
    Sector(String),
    Floor(String),
    Kind(Vec<String>),
    Measure(String),
    Number { measure: Option<String>, op: Op, value: f64 },
    Level(&'static str),
    Status(String),
    /// 질적 상태 (계측 종류가 있으면 그 계측만)
    Signal { measure: Option<String>, want: Quality },
    Near { anchor: String, upstream: Option<bool>, hops: usize },
}

impl Cond {
    fn matches(&self, d: &Doc, db: &Lattice, near: &BTreeMap<String, i32>) -> bool {
        match self {
            Cond::Sector(s) => d.sector.as_deref() == Some(s.as_str()),
            Cond::Floor(f) => d.floor.as_deref() == Some(f.as_str()),
            Cond::Kind(ks) => d.kinds.iter().any(|k| ks.contains(k)),
            Cond::Measure(m) => d.measures.iter().any(|x| measure_fits(m, &x.0, &x.1)),
            Cond::Number { measure, op, value } => d.measures.iter().any(|x| measure.as_deref().is_none_or(|m| measure_fits(m, &x.0, &x.1)) && x.2.is_some_and(|v| op.holds(v, *value))),
            Cond::Level(l) => d.level == Some(*l) || (*l == "alarm" && d.level == Some("trip")),
            Cond::Status(s) => d.status.as_deref().is_some_and(|x| x.contains(s.as_str())),
            Cond::Signal { measure, want } => d.signals.iter().any(|t| measure.as_deref().is_none_or(|m| measure_fits(m, &t.label, &t.unit)) && want.holds(t)),
            Cond::Near { anchor, .. } => d.kks.as_deref().is_some_and(|k| k != anchor && near.contains_key(k)) && db.equipment(d.kks.as_deref().unwrap_or("")).is_some(),
        }
    }

    fn describe(&self, db: &Lattice) -> (&'static str, String) {
        match self {
            Cond::Sector(s) => ("공간", db.sector(s).map(|x| format!("{} {}", x.building, x.label)).unwrap_or_else(|| s.clone())),
            Cond::Floor(f) => ("층", db.floor(f).map(|x| x.label.clone()).unwrap_or_else(|| f.clone())),
            Cond::Kind(k) => ("설비 종류", k.join(", ")),
            Cond::Measure(m) => ("계측", m.clone()),
            Cond::Number { measure, op, value } => ("수치", format!("{} {} {}", measure.as_deref().unwrap_or("값"), op.text(), fmt_value(*value))),
            Cond::Level(l) => ("상태", (*l).to_string()),
            Cond::Status(s) => ("상태", s.clone()),
            Cond::Signal { measure, want } => ("상태", format!("{}{}", measure.as_deref().map(|m| format!("{m} ")).unwrap_or_default(), want.label())),
            Cond::Near { anchor, upstream, hops } => ("연결", format!("{anchor} {} {hops}홉", match upstream { Some(true) => "상류", Some(false) => "하류", None => "주변" })),
        }
    }
}

impl Cond {
    /// 질적 상태 조건이 맞은 계측 (화면의 "왜")
    fn evidence(&self, d: &Doc) -> Option<String> {
        let Cond::Signal { measure, want } = self else { return None };
        let t = d.signals.iter().find(|t| measure.as_deref().is_none_or(|m| measure_fits(m, &t.label, &t.unit)) && want.holds(t))?;
        let shape = t.state.map(|s| format!(", {}", s.label())).unwrap_or_default();
        Some(format!("{} {} {} (정상 {}{shape})", t.label, fmt_value(t.value), t.unit, fmt_value(t.normal)))
    }
}

fn measure_fits(word: &str, label: &str, unit: &str) -> bool {
    if label.contains(word) {
        return true;
    }
    let unit = lang::fold(unit);
    if MEASURE_WORDS.iter().find(|(w, _)| *w == word).is_some_and(|(_, units)| units.contains(&unit.as_str()) && word != "수위" && word != "개도") {
        return true;
    }
    // 원장 라벨이 다른 언어일 때 (영어 · 일본어 · 독일어 원장): 그 계측어의 낱말이 라벨 안에 있는가 ("Überhitzer-Austrittsdampftemperatur" → 온도)
    label_measures(label).contains(&word)
}

/// 라벨 글에 든 계측어 (12개 언어 낱말, 부분 일치 — 라벨은 합성어 · 구). 겹치는 낱말은 긴 것만 봅니다:
/// 독일어 'Füllstand'(수위) 안의 네덜란드어 'stand'(개도), 'Volumenstrom'(유량) 안의 'Strom'(전류)이 따로 잡히지 않게.
fn label_measures(label: &str) -> Vec<&'static str> {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<String, Vec<&'static str>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(v) = cache.lock().ok().and_then(|c| c.get(label).cloned()) {
        return v;
    }
    let f: Vec<char> = lang::fold(label).chars().collect();
    let mut spans: Vec<(usize, usize, &'static str)> = Vec::new();
    let mut find = |w: &str, m: &'static str| {
        let w: Vec<char> = w.chars().collect();
        if w.len() < 2 || w.len() > f.len() {
            return;
        }
        for a in 0..=f.len() - w.len() {
            if f[a..a + w.len()] == w[..] {
                spans.push((a, a + w.len(), m));
            }
        }
    };
    for (en, m) in lexicon::MEASURE_WORDS_EN {
        if en.len() >= 4 {
            find(en, m);
        }
    }
    for l in lang::LANGS {
        if let Some(lex) = ml::lex(l) {
            for (w, m) in lex.measure_words() {
                // 라틴 문자 짧은 낱말(hub · tlak 의 일부 같은 우연)은 빼고, 한자 두 글자 이상은 그대로
                if w.chars().all(|c| c.is_ascii_alphabetic()) && w.len() < 4 {
                    continue;
                }
                find(&w, m);
            }
        }
    }
    spans.sort_by(|a, b| (b.1 - b.0).cmp(&(a.1 - a.0)).then(a.0.cmp(&b.0)));
    let mut kept: Vec<(usize, usize, &'static str)> = Vec::new();
    for s in spans {
        if kept.iter().all(|k| s.1 <= k.0 || s.0 >= k.1 || (s.0 == k.0 && s.1 == k.1)) {
            kept.push(s);
        }
    }
    let mut out: Vec<&'static str> = Vec::new();
    for (_, _, m) in kept {
        if !out.contains(&m) {
            out.push(m);
        }
    }
    if let Ok(mut c) = cache.lock() {
        c.insert(label.to_string(), out.clone());
    }
    out
}

/// 동 번호 (`101동` · `Building 101` · `101号棟` · `Gebäude 101` → 101)
fn building_number(building: &str) -> Option<u32> {
    let digits: String = building.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// 동 번호로 공간(sector) 키를 찾습니다. 원장 언어와 상관없이 번호가 같으면 같은 동입니다.
fn sector_by_number(db: &Lattice, n: u32) -> Option<String> {
    db.sectors().iter().find(|s| building_number(&s.building) == Some(n)).map(|s| s.key.clone())
}

/// 따옴표 안 이름이 원장 설비 이름(또는 en.json 영어 이름)과 같은 설비 (공백 · 대소문자 · 악센트 무시)
fn equipment_by_name(db: &Lattice, name: &str) -> Option<String> {
    let squash = |s: &str| lang::fold(s).chars().filter(|c| c.is_alphanumeric()).collect::<String>();
    let want = squash(name);
    if want.chars().count() < 2 {
        return None;
    }
    let names = lexicon::names();
    db.equipment_all().iter().find(|e| squash(&e.name) == want || names.en(&e.name).is_some_and(|en| squash(en) == want)).map(|e| e.kks.clone())
}

/// 원장 공간 이름(데이터 언어)이 질의에 있는 공간들 (한국어 · 영어 밖 질의). 붙여 쓰는 언어는 부분 일치,
/// 띄어 쓰는 언어는 이름의 내용 낱말(4글자 이상)이 모두 질의 낱말에 있을 때 (굴절 앞머리: Kesselhauses ↔ Kesselhaus).
fn sector_by_label(db: &Lattice, toks: &[QueryToken], glued: bool) -> Vec<String> {
    let joined: String = toks.iter().map(|t| lang::fold(&t.text)).collect::<Vec<_>>().join(if glued { "" } else { " " });
    let forms: Vec<String> = toks.iter().flat_map(|t| t.forms.iter().map(|f| lang::fold(f))).collect();
    let mut out = Vec::new();
    for s in db.sectors() {
        let label = lang::fold(&s.label);
        let hit = if glued {
            let compact: String = label.chars().filter(|c| c.is_alphanumeric()).collect();
            compact.chars().count() >= 2 && joined.replace(|c: char| !c.is_alphanumeric(), "").contains(&compact)
        } else {
            let words: Vec<&str> = label.split(|c: char| !c.is_alphanumeric()).filter(|w| w.chars().count() >= 4).collect();
            !words.is_empty()
                && words.iter().all(|w| {
                    let wc: Vec<char> = w.chars().collect();
                    forms.iter().any(|f| {
                        let fc: Vec<char> = f.chars().collect();
                        fc == wc || (fc.len() >= wc.len() && fc.len() <= wc.len() + 3 && fc[..wc.len()] == wc[..])
                    })
                })
        };
        if hit {
            out.push(s.key.clone());
        }
    }
    out
}

/// 숫자 + 단위 (`600도`, `30MPa`, `1,850`, `50%`)
fn number_unit(w: &str) -> Option<(f64, String)> {
    let mut num = String::new();
    let mut rest = String::new();
    for c in w.chars() {
        if rest.is_empty() && (c.is_ascii_digit() || c == '.' || c == ',' || (c == '-' && num.is_empty())) {
            if c != ',' {
                num.push(c);
            }
        } else {
            rest.push(c);
        }
    }
    let v: f64 = num.parse().ok()?;
    Some((v, rest))
}

fn unit_measure(unit: &str) -> Option<&'static str> {
    let u = unit.to_lowercase();
    if u.starts_with('도') || u.starts_with('℃') || u.starts_with("°c") {
        Some("온도")
    } else if u.starts_with("mpa") || u.starts_with("kpa") || u.starts_with("bar") {
        Some("압력")
    } else if u.starts_with("t/h") || u.starts_with("lpm") {
        Some("유량")
    } else if u.starts_with("μm") || u.starts_with("um") || u.starts_with("mm/s") {
        Some("진동")
    } else if u.starts_with("mw") {
        Some("출력")
    } else if u.starts_with("kw") {
        Some("전력")
    } else if u.starts_with("ppm") {
        Some("농도")
    } else {
        None
    }
}

/// 검색 결과 한 건
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Hit {
    pub doc: Doc,
    pub score: f32,
    /// 왜 찾았나 (맞은 조건 · 낱말 · 경로)
    pub why: Vec<String>,
    /// 조건 계획을 통과했는가 (아니면 전체 순위로만 온 것)
    pub passed: bool,
    /// 전체 표에서 구제한 항목
    pub rescued: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SearchResult {
    pub query: String,
    pub types: Vec<DocKind>,
    pub conditions: Vec<Condition>,
    /// 식별자 교정 · 부정어 · 경로 크기 등 판단 기록
    pub notes: Vec<String>,
    pub hits: Vec<Hit>,
    /// 질의 언어 ("ko" · "en")
    pub lang: &'static str,
    /// 쓴 모델 ("Stanza en · ko · 임베딩 granite-97m", 없으면 "규칙만")
    pub engine: String,
    /// 낱말마다 품사 · 원형 · 줄기 (화면의 "해석")
    pub tokens: Vec<QueryToken>,
    /// NMS 에서 이긴 조각
    pub segments: Vec<Segment>,
}

/// 같은 종류 조건이 여럿이면 어느 하나만 맞아도 됩니다 ("밸브 펌프"). 종류가 다르면 모두 맞아야 합니다.
fn passes_all(conds: &[Cond], d: &Doc, db: &Lattice, near: &BTreeMap<String, i32>) -> bool {
    let mut groups: HashMap<std::mem::Discriminant<Cond>, Vec<&Cond>> = HashMap::new();
    for c in conds {
        groups.entry(std::mem::discriminant(c)).or_default().push(c);
    }
    groups.values().all(|g| g.iter().any(|c| c.matches(d, db, near)))
}

fn rrf(lists: &[(&[usize], f32)]) -> Vec<(usize, f32)> {
    let mut score: BTreeMap<usize, f32> = BTreeMap::new();
    for (list, w) in lists {
        for (rank, i) in list.iter().enumerate() {
            *score.entry(*i).or_default() += w / (FUSION_K + rank as f32 + 1.0);
        }
    }
    let mut out: Vec<(usize, f32)> = score.into_iter().collect();
    out.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// 자연어 질의 하나 (모델 없이: 규칙 · 글자 경로만)
pub fn search(db: &Lattice, corpus: &Corpus, query: &str) -> SearchResult {
    search_with(db, corpus, query, None)
}

/// 숫자가 든 낱말 (동 · 층 · 수치 · 식별자는 규칙이 맡으므로 NMS 창에서 뺍니다)
fn has_digit(w: &str) -> bool {
    w.chars().any(|c| c.is_ascii_digit())
}

/// 관계 · 비교 · 부정 낱말 (규칙이 맡는 닫힌 낱말)
fn closed_word(w: &str) -> bool {
    const KO: [&str; 15] = ["상류", "하류", "주변", "연결", "관련", "이상", "이하", "초과", "미만", "넘는", "이내", "최소", "최대", "밑", "홉"];
    const EN: [&str; 21] = [
        "upstream", "downstream", "around", "near", "nearby", "connected", "neighbor", "neighbour", "adjacent", "hop", "hops", "above", "below", "over", "under", "than", "least", "most", "more", "less",
        "within",
    ];
    KO.iter().any(|k| w.starts_with(k)) || EN.contains(&w) || NEGATIONS.iter().any(|n| w.starts_with(n)) || lexicon::NEGATIONS_EN.contains(&w)
}

/// `toks[i..]` 가 구 `p` 로 시작하는가 (마지막 낱말은 원형 · 단수형도: "heat exchangers")
fn phrase_at(toks: &[QueryToken], i: usize, p: &str) -> bool {
    let parts: Vec<&str> = p.split(' ').collect();
    if i + parts.len() > toks.len() {
        return false;
    }
    parts.iter().enumerate().all(|(k, w)| if k + 1 == parts.len() { toks[i + k].any(|f| f == *w) } else { toks[i + k].text == *w })
}

const ORDINALS_EN: [(&str, u32); 10] = [("first", 1), ("1st", 1), ("second", 2), ("2nd", 2), ("third", 3), ("3rd", 3), ("fourth", 4), ("4th", 4), ("fifth", 5), ("5th", 5)];

fn floor_code(n: u32) -> Option<&'static str> {
    match n {
        1 => Some("1F"),
        2 => Some("2F"),
        3 => Some("3F"),
        4 => Some("4F"),
        5 => Some("5F"),
        _ => None,
    }
}

/// 영어 층 표현: "floor 2", "2nd floor", "second floor", "basement", "outdoor"
fn floor_en(toks: &[QueryToken], i: usize) -> Option<&'static str> {
    let w = toks[i].text.as_str();
    let next = toks.get(i + 1).map(|t| t.text.as_str()).unwrap_or("");
    let num = |s: &str| s.parse::<u32>().ok().or_else(|| ORDINALS_EN.iter().find(|(o, _)| *o == s).map(|x| x.1));
    match w {
        "basement" | "underground" | "b1" | "b1f" => Some("B1F"),
        "outdoor" | "outdoors" | "outside" | "yard" => Some("OUT"),
        "floor" | "fl" => num(next).and_then(floor_code),
        _ if matches!(next, "floor" | "fl" | "level") => num(w).and_then(floor_code),
        _ => None,
    }
}

/// 영어 질의의 머리 명사: 품사가 있으면 첫 명사 덩어리의 끝, 없으면 설비 종류 낱말이 하나라도 있는가
fn head_en(toks: &[QueryToken]) -> (bool, String) {
    let is_kind = |t: &QueryToken| t.any(|f| lexicon::KIND_WORDS_EN.iter().any(|(k, _)| *k == f) || matches!(f, "equipment" | "device" | "asset" | "unit"));
    // 첫 구(첫 전치사 · 관계사 앞)에 설비 종류 낱말이 있으면 그것이 머리입니다 ("pump with high pressure": Stanza 는 pump 를 동사로 봄)
    const BREAK: [&str; 12] = ["with", "that", "which", "where", "whose", "in", "on", "of", "at", "for", "from", "near"];
    // 첫 구의 끝 내용어만 봅니다 ("battery cell temperature" 의 battery 는 수식어, 머리는 temperature)
    let first_phrase = toks.iter().position(|t| BREAK.contains(&t.text.as_str()) || matches!(t.upos.as_deref(), Some("ADP" | "SCONJ"))).unwrap_or(toks.len());
    if let Some(t) = toks[..first_phrase].iter().rev().find(|t| !t.function && !has_digit(&t.text) && !closed_word(&t.text)).filter(|t| is_kind(t)) {
        return (true, t.text.clone());
    }
    if toks.iter().any(|t| t.upos.is_some()) {
        if let Some(start) = toks.iter().position(|t| t.is_noun() && !has_digit(&t.text)) {
            let mut end = start;
            while toks.get(end + 1).is_some_and(|t| t.is_noun() && !has_digit(&t.text)) {
                end += 1;
            }
            return (is_kind(&toks[end]), toks[end].text.clone());
        }
        return (false, String::new());
    }
    (false, String::new())
}

/// 한국어 질의의 머리 명사: 끝 내용어 (명령 동사 "찾아줘" 같은 기능어는 건너뜀)
fn head_ko(toks: &[QueryToken]) -> (bool, String) {
    let Some(t) = toks.iter().rev().find(|t| !t.function && !t.text.is_empty()) else { return (false, String::new()) };
    let trimmed = t.text.trim_end_matches(['을', '를', '이', '가', '은', '는']).to_string();
    let kind = t.any(|f| KIND_WORDS.iter().any(|(k, _)| f.ends_with(k)) || ["설비", "장비", "기기"].contains(&f))
        || KIND_WORDS.iter().any(|(k, _)| trimmed.ends_with(k))
        || ["설비", "장비", "기기"].contains(&trimmed.as_str());
    (kind, trimmed)
}

/// NMS 조각이 이 문서를 가리키는 무게 (개념 경로 · "왜")
fn concept_weight(target: &Target, d: &Doc) -> f32 {
    match target {
        Target::Equipment(k) => {
            if d.kks.as_deref() != Some(k.as_str()) {
                0.0
            } else if d.kind == DocKind::Equipment {
                1.0
            } else {
                0.5
            }
        }
        Target::Sensor(tag, k) => {
            if d.id == *tag {
                1.0
            } else if d.kind == DocKind::Equipment && d.kks.as_deref() == Some(k.as_str()) {
                0.5
            } else {
                0.0
            }
        }
        Target::Kind(ks) => f32::from(u8::from(d.kinds.iter().any(|k| ks.contains(k)))) * 0.5,
        Target::Measure(m) => f32::from(u8::from(d.measures.iter().any(|x| measure_fits(m, &x.0, &x.1)))) * 0.5,
        Target::Sector(s) => f32::from(u8::from(d.sector.as_deref() == Some(s.as_str()))) * 0.3,
        Target::Type(k) => f32::from(u8::from(d.kind == *k)) * 0.3,
        Target::Quality(q) => f32::from(u8::from(d.signals.iter().any(|t| q.holds(t)))) * 0.5,
    }
}

/// NMS 창의 처음 · 끝이 될 수 있는 낱말: 기능어 · 숫자 · 관계 · 비교 · 부정어 · 하이픈 식별자가 아닌 것
fn nms_usable(toks: &[QueryToken]) -> Vec<bool> {
    toks.iter().map(|t| !t.function && !t.text.is_empty() && !has_digit(&t.text) && !closed_word(&t.text) && !t.text.contains('-')).collect()
}

/// NMS 후보 전부 (통과 못 한 것 포함). 문턱을 정할 때 보는 진단용 (`examples/search_eval.rs --nms`).
pub fn nms_candidates(db: &Lattice, query: &str, models: &SearchModels) -> Vec<Candidate> {
    let analysis = analyze::analyze(&lang::normalize_query(query.trim()), |lang| models.stanza_for(lang));
    let sep = if lang::glued(analysis.lang) { "" } else { " " };
    match (models.embedder.as_ref(), models.bank(db)) {
        (Some(e), Some(b)) => nms::ground(&analysis.tokens, &nms_usable(&analysis.tokens), &b, e, sep).map(|x| x.1).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// 자연어 질의 하나. `models` 가 있으면 Stanza 품사 · 원형과 임베딩(질의 벡터 경로 · NMS 조각)을 더합니다.
pub fn search_with(db: &Lattice, corpus: &Corpus, query: &str, models: Option<&SearchModels>) -> SearchResult {
    let normalized = lang::normalize_query(query.trim());
    let q = normalized.to_lowercase();
    let words: Vec<String> = q.split_whitespace().map(str::to_string).collect();
    let analysis = analyze::analyze(&normalized, |lang| models.and_then(|m| m.stanza_for(lang)));
    let toks = &analysis.tokens;
    let en = analysis.lang == "en";
    // 한국어 · 영어 밖 언어: 닫힌 낱말을 그 언어 사전(lexicon_ml.json)으로 (판정이 비긴 언어 사전도 함께)
    let ml_lexes: Vec<&ml::Lex> = if matches!(analysis.lang, "ko" | "en") {
        Vec::new()
    } else {
        std::iter::once(analysis.lang).chain(analysis.also.iter().copied()).filter_map(ml::lex).collect()
    };
    let parsed = (!ml_lexes.is_empty()).then(|| ml::parse(toks, &ml_lexes, db, &analysis.quoted));
    let mut notes = Vec::new();
    if !matches!(analysis.lang, "ko" | "en") || !analysis.also.is_empty() {
        notes.push(format!(
            "질의 언어 {} ({}{})",
            analysis.lang,
            analysis.how,
            if analysis.also.is_empty() { String::new() } else { format!(" · 함께 본 사전 {}", analysis.also.join(" · ")) }
        ));
    }
    if let Some(p) = &parsed {
        notes.extend(p.notes.iter().cloned());
    }
    let quoted_names = lang::quoted_names(&normalized);
    if let Some(tagger) = &analysis.tagger {
        let stems: Vec<String> = toks.iter().filter(|t| t.forms.len() > 1).map(|t| format!("{}→{}", t.text, t.forms[1..].join("/"))).collect();
        let function: Vec<&str> = toks.iter().filter(|t| t.function && !t.text.is_empty()).map(|t| t.text.as_str()).collect();
        // 바꾼 것이 없으면 적지 않습니다 ("103동 2층 밸브" 처럼 조사 · 기능어가 없는 질의)
        if !stems.is_empty() || !function.is_empty() {
            notes.push(format!(
                "품사 분석 {tagger}: 줄기 {} · 기능어 {}",
                if stems.is_empty() { "-".to_string() } else { stems.join(", ") },
                if function.is_empty() { "-".to_string() } else { function.join(", ") }
            ));
        }
    }
    let joined = format!(" {} ", toks.iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" "));
    let has_phrase = |p: &str| joined.contains(&format!(" {p} "));

    // 1) 유형 먼저 (한국어는 부분 일치, 영어는 원형이 같을 때, 다른 언어는 그 언어 사전 · 언어와 상관없는 약어 · 번호 머리)
    let mut types: Vec<DocKind> = if let Some(p) = &parsed {
        let mut t = p.types.clone();
        // 언어와 상관없는 약어 · 번호 머리 (moc · aop · p&id · pid-): 낱말 단위로 ('stroomopwaarts' 안의 'mop' 이 MOC 가 되지 않게)
        for (k, ws) in TYPE_WORDS.iter() {
            let hit = ws.iter().filter(|w| w.is_ascii()).any(|w| toks.iter().any(|t| t.text == *w || (w.ends_with('-') && t.text.starts_with(w))));
            if hit && !t.contains(k) {
                t.push(*k);
            }
        }
        t
    } else {
        TYPE_WORDS.iter().filter(|(_, ws)| ws.iter().any(|w| q.contains(w))).map(|(k, _)| *k).collect()
    };
    for (k, ws) in lexicon::TYPE_WORDS_EN.iter().filter(|_| parsed.is_none()) {
        let hit = ws.iter().any(|w| {
            if w.ends_with('-') {
                toks.iter().any(|t| t.text.starts_with(w))
            } else if w.contains(' ') {
                has_phrase(w)
            } else {
                toks.iter().any(|t| t.any(|f| f == *w))
            }
        });
        if hit && !types.contains(k) {
            types.push(*k);
        }
    }
    if types.is_empty() {
        // 한국어 명사구는 끝 낱말이 머리입니다 ("출구온도 비정상 과열기" 는 과열기를 묻습니다). 영어는 첫 명사 덩어리의 끝입니다
        // ("valves on floor 2" 의 valves). 머리가 설비 종류 낱말이면 설비만 찾습니다.
        let (kind_head, head) = match &parsed {
            Some(p) => p.head.clone().unwrap_or((false, String::new())),
            None if en => head_en(toks),
            None => head_ko(toks),
        };
        if kind_head {
            types = vec![DocKind::Equipment];
            notes.push(format!("유형 지시어 없음 · 머리 낱말 '{head}' → 설비"));
        } else {
            types = vec![DocKind::Equipment, DocKind::Sensor];
            notes.push("유형 지시어 없음 → 설비(기본) · 계측".into());
        }
    }

    // 2) 식별자: KKS · DCIM 코드와 태그는 원장으로 교정. 원장에 그대로 있는 코드(설비든 태그든)를 먼저 봅니다:
    //    태그 교정을 먼저 하면 설비 10LAB10AP001 이 한 글자 다른 태그 10LAB10CP001 로 "교정" 됩니다 (콘솔이 선택 설비 코드를 붙여 묻는 문맥 질의에서 드러남).
    //    DCIM 코드는 글자로 시작하므로(`DC1CDU20AA02`) 첫 글자가 숫자인지가 아니라 12자리 영숫자인지로 봅니다.
    let mut ids: Vec<String> = Vec::new();
    for w in &words {
        let raw = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_uppercase();
        if raw.len() == 12 && raw.chars().all(|c| c.is_ascii_alphanumeric()) && raw.chars().any(|c| c.is_ascii_digit()) {
            match (db.match_tag(&raw), db.match_kks(&raw)) {
                (KksMatch::Exact(c), _) | (_, KksMatch::Exact(c)) => ids.push(c),
                (KksMatch::Corrected { code, raw, distance }, _) | (_, KksMatch::Corrected { code, raw, distance }) => {
                    notes.push(format!("식별자 교정 {raw} → {code} (편집 거리 {distance})"));
                    ids.push(code);
                }
                _ => {}
            }
        } else if raw.contains('-') && raw.len() >= 8 {
            ids.push(raw);
        }
    }

    // 2-1) NMS 조각 (임베딩 모델이 있을 때): 내용어로 시작 · 끝나는 창을 원장 개념에 대고, 겹치는 창은 점수 순으로 하나만 남깁니다.
    let emb = models.and_then(|m| m.embedder.as_ref());
    let bank = models.and_then(|m| m.bank(db));
    let mut usable = nms_usable(toks);
    if let Some(p) = &parsed {
        for (i, closed) in p.closed.iter().enumerate() {
            if *closed {
                usable[i] = false;
            }
        }
    }
    let sep = if lang::glued(analysis.lang) { "" } else { " " };
    let (segments, _candidates) = match (emb, bank.as_deref()) {
        (Some(e), Some(b)) => nms::ground(toks, &usable, b, e, sep).unwrap_or_default(),
        _ => Default::default(),
    };
    for s in &segments {
        notes.push(format!("NMS 조각 '{}' → {} ({} · 코사인 {:.3} · z {:.1})", s.text, s.label, s.target.family(), s.cos, s.z));
    }

    // 3) 조건 (부정어가 붙은 낱말은 조건으로 쓰지 않습니다). 한국어는 부정어가 뒤에("고착 아닌"), 영어는 앞에("not stuck") 옵니다.
    let negated = |i: usize| -> bool {
        if toks[i].hangul || !en {
            words.get(i + 1).is_some_and(|n| NEGATIONS.iter().any(|x| n.starts_with(x))) || NEGATIONS.iter().any(|x| words[i].ends_with(x) && words[i].len() > x.len())
        } else {
            toks[i].text.starts_with("non-")
                || (i.saturating_sub(3)..i).any(|j| lexicon::NEGATIONS_EN.contains(&toks[j].text.as_str()) && toks[j + 1..i].iter().all(|t| t.function))
        }
    };
    let measure_ko = |x: &String| MEASURE_WORDS.iter().find(|(m, _)| x.contains(m)).map(|(m, _)| m.to_string());
    let measure_tok = |t: &QueryToken| -> Option<String> {
        t.forms.iter().find_map(measure_ko).or_else(|| lexicon::MEASURE_WORDS_EN.iter().find(|(e, _)| t.any(|f| f == *e)).map(|(_, m)| m.to_string()))
    };
    let mut conds: Vec<Cond> = parsed.as_ref().map(|p| p.conds.clone()).unwrap_or_default();
    // 원장 공간 이름(데이터 언어)이 질의에 그대로 있으면 그 공간 ("Kesselhauses" ↔ Kesselhaus, ボイラー棟)
    if parsed.is_some() {
        for key in sector_by_label(db, toks, lang::glued(analysis.lang)) {
            if !conds.contains(&Cond::Sector(key.clone())) {
                conds.push(Cond::Sector(key));
            }
        }
    }
    for (i, w) in words.iter().enumerate().filter(|_| parsed.is_none()) {
        let t = &toks[i];
        if t.text.is_empty() {
            continue;
        }
        // 동 번호 (원장 동 이름의 언어와 상관없이 번호로: 103동 ↔ Gebäude 103 · 103号棟)
        if !en {
            let digits: String = w.chars().take_while(char::is_ascii_digit).collect();
            if !digits.is_empty() && w[digits.len()..].starts_with('동') {
                if let Some(key) = digits.parse().ok().and_then(|n| sector_by_number(db, n)) {
                    if !conds.contains(&Cond::Sector(key.clone())) {
                        conds.push(Cond::Sector(key));
                    }
                }
            }
        }
        if negated(i) {
            notes.push(format!("부정어가 붙은 '{w}' 는 조건으로 쓰지 않습니다"));
            continue;
        }
        // 동 조건은 동 번호(103동)나 동 이름 전체(보일러동 · 급수동)일 때만. "주급수" 같은 설비 이름 조각은 글자 경로에 맡깁니다.
        for s in db.sectors() {
            let b = s.building.to_lowercase();
            let label = s.label.replace('·', "").to_lowercase();
            let stem = format!("{}동", s.label.split('·').next().unwrap_or(&s.label).trim_end_matches('동'));
            if t.any(|f| f.starts_with(&b) || f == label || f == stem) || w.starts_with(&b) || *w == label || *w == stem {
                conds.push(Cond::Sector(s.key.clone()));
            }
        }
        if en {
            // 영어 동 · 여러 낱말 종류는 이 낱말에서 시작하는 구로 봅니다 ("building 103", "boiler building", "heat exchanger")
            let names = lexicon::names();
            for s in db.sectors() {
                let num = building_number(&s.building).map(|n| n.to_string()).unwrap_or_default();
                // 영어 이름: en.json (한국어 원장), 없으면 원장 이름 그대로 (영어 원장)
                let label_en = names.en(&s.label).map(str::to_lowercase).unwrap_or_else(|| if s.label.is_ascii() { s.label.to_lowercase() } else { String::new() });
                let at = |p: &str| phrase_at(toks, i, p);
                if (!num.is_empty() && (at(&format!("building {num}")) || at(&format!("bldg {num}")) || at(&format!("{num} building")))) || (!label_en.is_empty() && at(&label_en)) {
                    conds.push(Cond::Sector(s.key.clone()));
                }
            }
            for (k, kinds) in lexicon::KIND_WORDS_EN.iter().filter(|(k, _)| k.contains(' ')) {
                if phrase_at(toks, i, k) {
                    conds.push(Cond::Kind(kinds.iter().map(|x| x.to_string()).collect()));
                }
            }
        }
        let floor = if w.starts_with("지하") || w.starts_with("b1") {
            Some("B1F")
        } else if w.starts_with("옥외") || w.starts_with("야외") {
            Some("OUT")
        } else {
            w.strip_suffix("층").or_else(|| w.strip_suffix('f')).and_then(|n| n.parse::<u32>().ok()).and_then(floor_code).or_else(|| floor_en(toks, i))
        };
        if let Some(f) = floor {
            conds.push(Cond::Floor(f.into()));
        }
        if let Some((_, kinds)) = KIND_WORDS.iter().find(|(k, _)| t.any(|f| f.ends_with(k) || f == *k)) {
            conds.push(Cond::Kind(kinds.iter().map(|x| x.to_string()).collect()));
        } else if let Some((_, kinds)) = lexicon::KIND_WORDS_EN.iter().filter(|(k, _)| !k.contains(' ')).find(|(k, _)| t.any(|f| f == *k)) {
            conds.push(Cond::Kind(kinds.iter().map(|x| x.to_string()).collect()));
        }
        let kind_en = |t: &QueryToken| t.any(|f| lexicon::KIND_WORDS_EN.iter().any(|(k, _)| *k == f));
        let compound_name = KIND_WORDS.iter().any(|(k, _)| w.ends_with(k) && w.as_str() != *k)
            || (en && !kind_en(t) && toks[i + 1..].iter().take_while(|x| !x.function && !has_digit(&x.text)).any(kind_en));
        if !compound_name {
            if let Some(m) = measure_tok(t) {
                conds.push(Cond::Measure(m));
            }
        }
        // 숫자 + 비교어. 한국어는 같은 낱말 안이거나 뒤 ("600도 이상", "30MPa이하"), 영어는 앞이거나 뒤 ("above 600 °C", "600 or more")
        if let Some((value, rest)) = number_unit(w) {
            let next = words.get(i + 1).cloned().unwrap_or_default();
            if en {
                let before: Vec<String> = (1..=3).filter_map(|k| i.checked_sub(k)).map(|s| toks[s..i].iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ")).collect();
                // 뒤 비교어는 단위 낱말 다음에 옵니다 ("1,400 t/h or less")
                let unit_next = rest.is_empty() && (unit_measure(&next).is_some() || lexicon::unit_measure_en(&next).is_some() || next == "%");
                let a0 = (i + 1 + usize::from(unit_next)).min(toks.len());
                let after = toks[a0..(a0 + 2).min(toks.len())].iter().map(|t| t.text.as_str()).collect::<Vec<_>>().join(" ");
                let op = lexicon::COMPARATORS_EN_BEFORE
                    .iter()
                    .chain(lexicon::COMPARATORS_EN_BEFORE_MORE.iter())
                    .find(|(c, _)| before.iter().any(|b| b == c || b.ends_with(&format!(" {c}"))))
                    .or_else(|| lexicon::COMPARATORS_EN_AFTER.iter().find(|(c, _)| after.starts_with(c)))
                    .map(|x| x.1);
                if let Some(op) = op {
                    let unit = if rest.is_empty() { next.clone() } else { rest.clone() };
                    let measure = unit_measure(&unit)
                        .or_else(|| lexicon::unit_measure_en(&unit))
                        .map(str::to_string)
                        .or_else(|| toks[..i].iter().rev().take(4).find_map(measure_tok))
                        .or_else(|| toks[i + 1..].iter().take(3).find_map(measure_tok));
                    conds.push(Cond::Number { measure, op, value });
                }
            } else {
                // "600도 이상" · "30MPa이하" · "1,400 t/h 미만": 비교어는 두 낱말 앞까지, 단위는 붙어 있거나 바로 다음 낱말
                let unit = if rest.is_empty() && unit_measure(&next).is_some() { next.clone() } else { rest.clone() };
                let cmp_text = format!("{rest} {next} {}", words.get(i + 2).cloned().unwrap_or_default());
                if let Some((_, op)) = COMPARATORS.iter().find(|(c, _)| cmp_text.contains(c)) {
                    let measure = unit_measure(&unit).map(str::to_string).or_else(|| words[..i].iter().rev().take(2).find_map(|p| MEASURE_WORDS.iter().find(|(m, _)| p.contains(m)).map(|(m, _)| m.to_string())));
                    conds.push(Cond::Number { measure, op: *op, value });
                }
            }
        }
        let next_text = toks.get(i + 1).map(|t| t.text.as_str()).unwrap_or("");
        if w.starts_with("트립") || t.any(|f| matches!(f, "trip" | "tripped" | "tripping")) {
            conds.push(Cond::Level("trip"));
        } else if ((w.starts_with("경보") || w.starts_with("알람")) && !q.contains("경보 목록") && !q.contains("알람 목록"))
            || (t.any(|f| matches!(f, "alarm" | "alarming")) && !has_phrase("alarm list"))
        {
            conds.push(Cond::Level("alarm"));
        } else if w.starts_with("정상") || t.any(|f| f == "normal") {
            conds.push(Cond::Level("normal"));
        }
        // 질적 상태: 계측 낱말은 같은 낱말 안 → 앞 두 낱말 → 바로 다음 낱말 차례로 찾습니다 ("압력 높은", "출구온도 비정상", "높은 압력", "high pressure")
        let want = quality_of(w).or_else(|| {
            let hit = lexicon::QUALITY_WORDS_EN.iter().find(|(k, _)| t.any(|f| f == *k)).map(|x| x.1);
            // "higher than 600" 은 비교어, "high risk" 는 절차서 상태입니다
            hit.filter(|_| !matches!(next_text, "than" | "risk"))
        });
        if let Some(want) = want {
            let measure = measure_tok(t).or_else(|| toks[..i].iter().rev().take(2).find_map(measure_tok)).or_else(|| toks.get(i + 1).and_then(measure_tok));
            conds.push(Cond::Signal { measure, want });
        }
        let pending = q.contains("대기") || has_phrase("pending") || has_phrase("awaiting");
        if types.contains(&DocKind::Moc) && (w.contains("대기") || t.any(|f| matches!(f, "pending" | "awaiting"))) {
            conds.push(Cond::Status("승인 대기".into()));
        } else if types.contains(&DocKind::Moc) && (w.starts_with("승인") || t.any(|f| matches!(f, "approved" | "approve"))) && !pending {
            conds.push(Cond::Status("승인".into()));
        } else if types.contains(&DocKind::Aop) && (w.starts_with("고위험") || t.text == "high-risk" || (t.text == "high" && next_text == "risk")) {
            conds.push(Cond::Status("고위험".into()));
        }
    }
    // 연결: "상류 · 하류 N홉" · "N hop upstream" 은 기준 설비가 있어야 합니다
    // (식별자, NMS 조각이 가리킨 설비, 이름이 가장 많이 겹치는 설비 차례)
    let word_is = |ws: &[&str]| toks.iter().any(|t| t.any(|f| ws.contains(&f)));
    let (up, down, any_rel, ml_hops) = match parsed.as_ref().and_then(|p| p.relation) {
        Some((dir, hops)) => (dir == Some(true), dir == Some(false), true, hops),
        None if parsed.is_some() => (false, false, false, None),
        None => {
            let up = q.contains("상류") || word_is(&["upstream"]);
            let down = q.contains("하류") || word_is(&["downstream"]);
            let any = up || down || q.contains("주변") || q.contains("연결") || word_is(&["around", "near", "nearby", "connected", "neighbor", "neighbour", "adjacent"]);
            (up, down, any, None)
        }
    };
    let mut near: BTreeMap<String, i32> = BTreeMap::new();
    if any_rel {
        let hops_en = toks.iter().enumerate().find_map(|(i, t)| {
            if matches!(t.text.as_str(), "hop" | "hops") {
                i.checked_sub(1).and_then(|p| toks[p].text.parse::<usize>().ok())
            } else {
                t.text.strip_suffix("-hop").or_else(|| t.text.strip_suffix("-hops")).and_then(|n| n.parse::<usize>().ok())
            }
        });
        let hops = ml_hops
            .or_else(|| words.iter().find_map(|w| w.strip_suffix("홉").and_then(|n| n.parse::<usize>().ok())))
            .or(hops_en)
            .unwrap_or(crate::graph::MAX_HOPS)
            .min(crate::graph::MAX_HOPS);
        let names = lexicon::names();
        let closed_ml = |i: usize| parsed.as_ref().is_some_and(|p| p.closed.get(i).copied().unwrap_or(false));
        // 하이픈 낱말은 조각도 (Hauptspeisewasser-Durchflussregelventil → hauptspeisewasser · durchflussregelventil)
        let content: BTreeSet<String> = toks
            .iter()
            .enumerate()
            .filter(|(i, t)| !t.function && !closed_word(&t.text) && !closed_ml(*i))
            .flat_map(|(_, t)| t.forms.iter().map(|f| lang::fold(f)).collect::<Vec<_>>())
            .flat_map(|f| {
                let mut v = vec![f.clone()];
                if f.contains('-') {
                    v.extend(f.split('-').filter(|p| !p.is_empty()).map(str::to_string));
                }
                v
            })
            .collect();
        let glued = lang::glued(analysis.lang);
        let anchor = ids
            .iter()
            .find_map(|i| db.equipment(i).map(|e| e.kks.clone()).or_else(|| db.sensor(i).map(|s| s.kks.clone())))
            // 따옴표로 묶은 원장 이름 (데이터 언어 이름을 질의 언어 문장에 그대로 넣은 것)
            .or_else(|| quoted_names.iter().find_map(|n| equipment_by_name(db, n)))
            .or_else(|| {
                // NMS 조각이 가리킨 설비 (z 순). 다른 언어 질의는 센서 조각도 그 센서의 설비로 받고("给水流量调节" → 급수 유량 센서 → 그 밸브),
                // 질의에 설비 종류 낱말이 있으면 그 낱말을 품은 조각을 먼저 봅니다 — 연결 기준은 "…밸브 상류" 처럼 종류 낱말로 끝나는 이름이라
                // 'eau alimentaire principale'(→ 주급수펌프 A) 같은 이름 조각보다 'vanne … principale' 가 기준입니다.
                let kks_of = |s: &Segment| match &s.target {
                    Target::Equipment(k) => Some(k.clone()),
                    Target::Sensor(_, k) if parsed.is_some() => Some(k.clone()),
                    _ => None,
                };
                let kind_hits: Vec<(usize, usize)> = parsed.as_ref().map(|p| p.hits.iter().filter(|h| matches!(h.cat, ml::Cat::Kind(_))).map(|h| (h.start, h.end)).collect()).unwrap_or_default();
                let has_kind = |s: &Segment| kind_hits.iter().any(|(a, b)| *a >= s.start && *b <= s.end);
                segments.iter().filter(|s| kks_of(s).is_some()).max_by(|a, b| has_kind(a).cmp(&has_kind(b)).then(a.z.total_cmp(&b.z))).and_then(kks_of)
            })
            .or_else(|| {
                // 원장 이름과 2글자가 가장 많이 겹치는 설비: 한국어 · 일본어 · 중국어 질의만 (영어 질의는 'upstream' 의 up · ps 가 '(UPS)' 에 걸려서 쓰지 않음)
                if en || (parsed.is_some() && !glued) {
                    return None;
                }
                let qg = bigrams(&lang::fold(&q));
                db.equipment_all()
                    .iter()
                    .map(|e| (bigrams(&lang::fold(&e.name)).intersection(&qg).count(), e))
                    .filter(|(n, _)| *n >= 2)
                    .max_by_key(|(n, _)| *n)
                    .map(|(_, e)| e.kks.clone())
            })
            .or_else(|| {
                // 이름과 겹치는 낱말 수 (2개 이상, 같으면 짧은 이름). 영어 이름(en.json)과 원장 이름(데이터 언어) 둘 다
                db.equipment_all()
                    .iter()
                    .filter_map(|e| {
                        let mut best: Option<(usize, usize)> = None;
                        for nm in [names.en(&e.name).map(str::to_string), Some(e.name.clone())].into_iter().flatten() {
                            let name: Vec<String> = lang::fold(&nm).split(|c: char| !c.is_alphanumeric()).filter(|x| !x.is_empty()).map(str::to_string).collect();
                            let hit = name.iter().filter(|x| content.contains(*x) || content.contains(&lexicon::singular_en(x)) || (parsed.is_some() && x.chars().count() >= 4 && content.iter().any(|c| c.starts_with(x.as_str())))).count();
                            if best.is_none_or(|b| hit > b.0) {
                                best = Some((hit, name.len()));
                            }
                        }
                        let (hit, len) = best?;
                        (hit >= 2).then_some((hit, usize::MAX - len, e.kks.clone()))
                    })
                    .max()
                    .map(|x| x.2)
            });
        if let Some(anchor) = anchor {
            let upstream = if up && !down { Some(true) } else if down && !up { Some(false) } else { None };
            for h in crate::graph::hop_depths(db, &anchor, hops) {
                let keep = match upstream {
                    Some(true) => h.depth < 0,
                    Some(false) => h.depth > 0,
                    None => h.depth != 0,
                };
                if keep {
                    near.insert(h.kks, h.depth);
                }
            }
            notes.push(format!("연결 기준 설비 {anchor} ({}홉 안 {}개)", hops, near.len()));
            conds.push(Cond::Near { anchor, upstream, hops });
            // 기준 설비 이름의 낱말은 종류 조건이 아닙니다 ("급수조절밸브 상류" 의 밸브)
            conds.retain(|c| !matches!(c, Cond::Kind(_)));
        }
    }
    conds.dedup();

    // 4) 대상 유형 안에서 조건을 원장과 대조합니다. 조건을 하나씩 더해 가며, 더한 뒤 맞는 항목이 하나도 없으면 그 조건은
    //    힌트(가점)로 내립니다 (정답을 지우지 않게). 사용자가 직접 지목한 연결(상류 · 하류)을 먼저 겁니다.
    let in_scope: Vec<usize> = (0..corpus.docs.len()).filter(|i| types.contains(&corpus.docs[*i].kind)).collect();
    conds.sort_by_key(|c| u8::from(!matches!(c, Cond::Near { .. })));
    let ground = |conds: &[Cond]| -> (Vec<Cond>, Vec<Cond>, Vec<Condition>) {
        let mut hard: Vec<Cond> = Vec::new();
        let mut hints: Vec<Cond> = Vec::new();
        let mut conditions = Vec::new();
        for c in conds {
            let mut trial = hard.clone();
            trial.push(c.clone());
            let satisfiable = in_scope.iter().any(|i| passes_all(&trial, &corpus.docs[*i], db, &near));
            let (kind, text) = c.describe(db);
            let note = if satisfiable {
                "필터".to_string()
            } else if matches!(c, Cond::Signal { .. }) && !corpus.live {
                "지금 계측이 없어 힌트로 내림 (물리엔진 · 시나리오 재생 중에만 판정)".to_string()
            } else {
                "앞 조건과 함께 맞는 항목이 없어 힌트로 내림 (정답을 지우지 않게)".to_string()
            };
            conditions.push(Condition { kind, text, applied: satisfiable, note });
            if satisfiable {
                hard.push(c.clone());
            } else {
                hints.push(c.clone());
            }
        }
        (hard, hints, conditions)
    };
    let (mut hard, mut hints, mut conditions) = ground(&conds);
    // 설비 종류 조건이 모두 힌트로 내려갔으면(앞의 위치 · 상태 조건이 찾는 대상 자체를 밀어냄) 종류를 먼저 대어 다시 겁니다:
    // "보일러동 지하 … 과열기" 를 다른 언어 원장(동 이름이 안 맞아 '보일러동' 이 빠짐)에서 찾으면 '지하' 가 필터가 되어 과열기가 힌트로
    // 밀렸습니다. 종류 조건이 하나라도 걸리면("105동 밸브 유인통풍기": 밸브는 힌트, 통풍기는 필터) 나온 차례 그대로 둡니다.
    if hints.iter().any(|c| matches!(c, Cond::Kind(_))) && !hard.iter().any(|c| matches!(c, Cond::Kind(_))) {
        let mut reordered = conds.clone();
        reordered.sort_by_key(|c| match c {
            Cond::Near { .. } => 0u8,
            Cond::Kind(_) => 1,
            _ => 2,
        });
        let (h2, i2, c2) = ground(&reordered);
        if h2.iter().any(|c| matches!(c, Cond::Kind(_))) {
            notes.push("설비 종류가 앞 조건에 밀려 힌트가 되어, 종류를 먼저 대어 다시 걸었습니다".into());
            (hard, hints, conditions) = (h2, i2, c2);
        }
    }
    let passes = |d: &Doc| -> bool { passes_all(&hard, d, db, &near) };

    // 5) 경로: 파서(조건 통과 + 힌트 수) · 글자 일치(idf) · 2글자 n-gram, 유형 제한 경로, 그리고 (모델이 있으면) 질의 벡터 · 개념 경로
    let n = corpus.docs.len() as f32;
    // 글자 경로 낱말: 기능어는 빼고(영어 'on' 이 'condensate' 에 걸림), 줄기 · 원형을 더합니다 (`급수펌프의` → `급수펌프`, `valves` → `valve`)
    // "설비 · 장비 · equipment" 같은 일반 머리 낱말은 거의 모든 설비 글에 있어 글자 경로를 흐립니다 ("석탄을 가루로 만드는 설비" 가 "탈질 설비" 로)
    const GENERIC: [&str; 8] = ["설비", "장비", "기기", "equipment", "device", "asset", "unit", "system"];
    let mut qterms: Vec<String> = Vec::new();
    // 일본어 · 중국어는 공백 덩어리가 아니라 낱말(문자 묶음 · Stanza 낱말)을 글자 경로 낱말로
    let raw_terms: Vec<String> = if lang::glued(analysis.lang) {
        toks.iter().enumerate().filter(|(i, t)| !t.function && t.text.chars().count() >= 2 && !parsed.as_ref().is_some_and(|p| p.closed[*i])).map(|(_, t)| t.text.clone()).collect()
    } else {
        terms_of(&q)
    };
    for raw in raw_terms {
        let function = toks.iter().enumerate().any(|(i, t)| (t.function || t.any(|f| GENERIC.contains(&f)) || parsed.as_ref().is_some_and(|p| p.closed[i])) && t.text == raw);
        if !function && !GENERIC.contains(&raw.as_str()) && !qterms.contains(&raw) {
            qterms.push(raw);
        }
    }
    for t in toks.iter().filter(|t| !t.function && !t.any(|f| GENERIC.contains(&f))) {
        for f in t.forms.iter().skip(1) {
            if f.chars().count() >= 2 && !qterms.contains(f) {
                qterms.push(f.clone());
            }
        }
    }
    // 호기 표시: 내용어 뒤의 한 글자 (Pod A · Pump B · 펌프 A)는 영어 관사가 아니라 같은 이름의 설비를 가르는 표시입니다
    for i in 1..toks.len() {
        let (prev, cur) = (&toks[i - 1], &toks[i]);
        if cur.text.len() == 1 && cur.text.chars().all(|c| c.is_ascii_alphabetic()) && !prev.text.is_empty() && !has_digit(&prev.text) && !lexicon::FILLER_EN.contains(&prev.text.as_str()) {
            let phrase = format!("{} {}", prev.text, cur.text);
            if !qterms.contains(&phrase) {
                qterms.push(phrase);
            }
        }
    }
    // 영어 낱말(로마자만)은 문서 낱말로 맞추고, 한국어 · 식별자는 부분 문자열로 맞춥니다 (교착어 · 하이픈 번호).
    // 다른 라틴 문자 언어 낱말은 악센트를 뗀 문서 낱말과 같거나, 굴절 앞머리(Pumpen ↔ pumpe)이거나, 합성어 안(독일어 · 네덜란드어)이면 맞음
    let compound = ml_lexes.first().is_some_and(|l| l.compound);
    let latin_ml = parsed.is_some() && !lang::glued(analysis.lang);
    let term_hits = |d: &Doc, t: &str| -> bool {
        if t.chars().all(|c| c.is_ascii_alphabetic()) && !latin_ml {
            d.words.contains(t)
        } else if latin_ml && t.chars().all(char::is_alphabetic) {
            let f = lang::fold(t);
            let n = f.chars().count();
            d.words.contains(&f)
                || d.words.iter().any(|w| {
                    let wn = w.chars().count();
                    (n >= 5 && wn >= n.saturating_sub(2) && wn <= n + 3 && {
                        let common = w.chars().zip(f.chars()).take_while(|(a, b)| a == b).count();
                        common >= 4.max(n.saturating_sub(2)).min(wn)
                    }) || (compound && n >= 5 && wn > n + 2 && w.contains(f.as_str()))
                })
        } else {
            d.text.contains(t)
        }
    };
    let literal_score = |d: &Doc| -> f32 {
        let mut s: f32 = qterms.iter().filter(|t| term_hits(d, t)).map(|t| ((n + 1.0) / (*corpus.df.get(t).unwrap_or(&0) as f32 + 0.5)).ln()).sum();
        s += ids.iter().filter(|id| d.id.eq_ignore_ascii_case(id) || d.text.contains(&id.to_lowercase())).count() as f32 * 10.0;
        s
    };
    let qgrams = bigrams(&lang::fold(&q));
    let gram_score = |d: &Doc| -> f32 {
        // 영어 질의는 영어 제목에 댑니다 (로마자 2글자가 한국어 제목 옆의 식별자에 우연히 겹치지 않게)
        let title = if en { d.title_en.as_str() } else { &lang::fold(&d.title) };
        let g = bigrams(&format!("{} {}", title, d.id.to_lowercase()));
        let inter = g.intersection(&qgrams).count() as f32;
        if inter == 0.0 {
            0.0
        } else {
            2.0 * inter / (g.len() + qgrams.len()) as f32
        }
    };
    let ranked = |pool: &[usize], f: &dyn Fn(&Doc) -> f32| -> Vec<usize> {
        let mut v: Vec<(usize, f32)> = pool.iter().map(|i| (*i, f(&corpus.docs[*i]))).filter(|x| x.1 > 0.0).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|x| x.0).collect()
    };
    let all: Vec<usize> = (0..corpus.docs.len()).collect();
    let parser: Vec<usize> = {
        let mut v: Vec<(usize, f32)> = in_scope
            .iter()
            .filter(|i| !hard.is_empty() && passes(&corpus.docs[**i]))
            .map(|i| {
                let d = &corpus.docs[*i];
                let hint = hints.iter().filter(|c| c.matches(d, db, &near)).count() as f32;
                (*i, 1.0 + hint + literal_score(d) * 0.1 + gram_score(d))
            })
            .collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|x| x.0).collect()
    };
    let literal = ranked(&all, &literal_score);
    let grams = ranked(&all, &gram_score);
    let typed_literal = ranked(&in_scope, &literal_score);
    let typed_grams = ranked(&in_scope, &gram_score);
    // 질의 벡터 경로 (logis-center fuse_whole_query 의 vector · typed_vector): 질의 전체와 문서 글(한국어 · 영어 이름)의 코사인 상위
    let (mut vector, mut typed_vector): (Vec<usize>, Vec<usize>) = (Vec::new(), Vec::new());
    if let Some(e) = emb {
        if let Ok(qv) = e.embed(query.trim()) {
            let mut sims: Vec<(usize, f32)> = corpus.docs.iter().enumerate().filter_map(|(i, d)| e.embed(&d.embed_text).ok().map(|v| (i, crate::ai::embed::cosine(&qv, &v)))).collect();
            sims.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
            vector = sims.iter().take(VECTOR_TOP).map(|x| x.0).collect();
            typed_vector = sims.iter().filter(|x| types.contains(&corpus.docs[x.0].kind)).take(VECTOR_TOP).map(|x| x.0).collect();
        }
    }
    // 개념 경로: NMS 조각이 가리키는 설비 · 센서 · 종류 · 계측 · 공간 (z 가중)
    let concept: Vec<usize> = {
        let mut v: Vec<(usize, f32)> = all.iter().map(|i| (*i, segments.iter().map(|s| concept_weight(&s.target, &corpus.docs[*i]) * s.z).sum::<f32>())).filter(|x| x.1 > 0.0).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|x| x.0).collect()
    };
    let mut sizes = format!("경로 크기 파서 {} · 글자 {} · n-gram {} · 유형 글자 {} · 유형 n-gram {}", parser.len(), literal.len(), grams.len(), typed_literal.len(), typed_grams.len());
    if emb.is_some() {
        sizes.push_str(&format!(" · 벡터 {} · 유형 벡터 {} · 개념 {}", vector.len(), typed_vector.len(), concept.len()));
    }
    notes.push(sizes);
    let fused = rrf(&[
        (&parser, 1.0),
        (&literal, LITERAL_WEIGHT),
        (&grams, 1.0),
        (&typed_literal, 1.0),
        (&typed_grams, 1.0),
        (&vector, 1.0),
        (&typed_vector, 1.0),
        (&concept, 1.0),
    ]);

    // 6) 화면 순서: 조건 계획을 통과한 후보(합친 순서) → 전체 표에서 구제한 항목 → 나머지 (조건이 없으면 모두 통과)
    let mut hits = Vec::new();
    let mut seen = BTreeSet::new();
    let why = |d: &Doc| -> Vec<String> {
        let mut w: Vec<String> = hard
            .iter()
            .chain(hints.iter())
            .filter(|c| c.matches(d, db, &near))
            .map(|c| {
                let (k, t) = c.describe(db);
                c.evidence(d).map_or_else(|| format!("{k} {t}"), |e| format!("{k} {t}: {e}"))
            })
            .collect();
        w.extend(qterms.iter().filter(|t| term_hits(d, t)).map(|t| format!("'{t}'")));
        w.extend(segments.iter().filter(|s| concept_weight(&s.target, d) >= 0.5).map(|s| format!("의미 '{}' ≈ {}", s.text, s.label)));
        if let (Some(k), true) = (d.kks.as_ref(), !near.is_empty()) {
            if let Some(depth) = near.get(k) {
                w.push(format!("Depth {}{depth}", if *depth > 0 { "+" } else { "" }));
            }
        }
        w
    };
    let planned = !hard.is_empty();
    for (i, s) in &fused {
        let d = &corpus.docs[*i];
        let ok = !planned || (types.contains(&d.kind) && passes(d));
        if ok && seen.insert(*i) {
            hits.push(Hit { doc: d.clone(), score: *s, why: why(d), passed: true, rescued: false });
        }
    }
    if planned {
        for i in &in_scope {
            let d = &corpus.docs[*i];
            if passes(d) && seen.insert(*i) {
                hits.push(Hit { doc: d.clone(), score: 0.0, why: why(d), passed: true, rescued: true });
            }
        }
        for (i, s) in &fused {
            if seen.insert(*i) {
                let d = &corpus.docs[*i];
                hits.push(Hit { doc: d.clone(), score: *s, why: why(d), passed: false, rescued: false });
            }
        }
    }
    hits.truncate(LIMIT);
    let engine = models.map(SearchModels::summary).unwrap_or_else(|| "규칙만".into());
    SearchResult { query: query.to_string(), types, conditions, notes, hits, lang: analysis.lang, engine, tokens: analysis.tokens, segments }
}

/// 화면 순위로 채점하는 평가 질의 (logis-center 방식: 필터를 거친 화면 순서로 잼). (질의, 정답 id, 허용 순위)
pub const EVAL_QUERIES: [(&str, &str, usize); 17] = [
    ("103동 2층 밸브", "10LAB20AA102", 2),
    ("103동 2층 밸브", "10LAB20AA103", 2),
    ("급수 바이패스", "10LAB20AA103", 1),
    ("과열기 온도 센서", "10HAH10CT001", 1),
    ("10LAB20AA1O2", "10LAB20AA102", 1),
    ("주급수 유량조절밸브 상류 1홉", "10LAB10AP001", 1),
    ("지하 펌프", "10LCA10AP001", 2),
    ("승인 대기 MOC", "MOC-2026-1021", 1),
    ("MOC-2026-1104", "MOC-2026-1104", 1),
    ("베어링 진동 절차서", "AOP-102-TB01", 1),
    ("경보 한계 600도 이상 센서", "10HAH10CT001", 3),
    ("105동 도면", "PID-105-HLB", 1),
    ("유인통풍기", "10HLB10AN001", 1),
    ("주증기 압력", "10MAA10CP001", 2),
    // 개선안 예시 (지금 계측 없음: 질적 상태는 힌트로)
    ("103동 압력 높은 급수펌프", "10LAB10AP001", 1),
    ("보일러동 지하 출구온도 비정상 과열기", "10HAH10BB001", 1),
    ("10LABI0AA102 고착", "10LAB20AA102", 1),
];

/// 지금 계측이 있을 때의 평가 (시나리오 P2 재생 끝 시각: 밸브 18 % 고착 · 펌프 토출 29.4 MPa · 과열기 638 °C)
/// AI 데이터센터 원장 평가 질의 (계측 없음) · 지금 계측(D2 재생 끝 시각)
pub const EVAL_AIDC_QUERIES: [(&str, &str, usize); 10] = [
    ("103동 2층 밸브", "DC1CDU20AA02", 2),
    ("CDU 우회밸브", "DC1CDU20AA03", 1),
    ("GPU 정션 온도 센서", "DC1GPU10CT01", 2),
    ("Pod A 매니폴드", "DC1MNF10MV01", 1),
    ("DC1CDUI0AP01", "DC1CDU10AP01", 1),
    ("배터리 셀 온도", "DC1BES10CT01", 1),
    ("냉각탑 팬", "DC1CTW10AN01", 2),
    ("열폭주 절차서", "EOP-104-BAT01", 1),
    ("105동 도면", "MEP-105-CHL", 1),
    ("승인 대기 MOP", "MOP-2026-1021", 1),
];
pub const EVAL_AIDC_LIVE_SCENARIO: &str = "D2";
pub const EVAL_AIDC_LIVE_QUERIES: [(&str, &str, usize); 4] = [
    ("고착된 밸브", "DC1CDU20AA02", 1),
    ("온도 높은 GPU 랙", "DC1GPU10RK01", 1),
    ("103동 압력 높은 펌프", "DC1CDU10AP01", 1),
    ("유량 낮은 냉각수", "DC1CDU20AA02", 2),
];

pub const EVAL_LIVE_SCENARIO: &str = "P2";
pub const EVAL_LIVE_QUERIES: [(&str, &str, usize); 7] = [
    ("103동 압력 높은 급수펌프", "10LAB10AP001", 1),
    ("보일러동 지하 출구온도 비정상 과열기", "10HAH10BB001", 1),
    ("10LABI0AA102 고착", "10LAB20AA102", 1),
    ("고착된 밸브", "10LAB20AA102", 1),
    ("유량 낮은 급수", "10LAB20AA102", 1),
    ("온도 높은 센서", "10HAH10CT001", 1),
    ("103동 비정상 설비", "10LAB20AA102", 2),
];

/// 영어 질의 (EVAL_QUERIES 와 같은 정답). 화면이 한국어로 옮기지 않고 엔진이 그대로 읽습니다.
pub const EVAL_QUERIES_EN: [(&str, &str, usize); 17] = [
    ("valves on floor 2 of building 103", "10LAB20AA102", 2),
    ("valves on floor 2 of building 103", "10LAB20AA103", 2),
    ("feedwater bypass", "10LAB20AA103", 1),
    ("superheater temperature sensor", "10HAH10CT001", 1),
    ("10LAB20AA1O2", "10LAB20AA102", 1),
    ("main feedwater flow control valve 1 hop upstream", "10LAB10AP001", 1),
    ("basement pumps", "10LCA10AP001", 2),
    ("pending MOC", "MOC-2026-1021", 1),
    ("MOC-2026-1104", "MOC-2026-1104", 1),
    ("bearing vibration procedure", "AOP-102-TB01", 1),
    ("sensors with alarm limit above 600 degrees", "10HAH10CT001", 3),
    ("building 105 drawings", "PID-105-HLB", 1),
    ("ID fan", "10HLB10AN001", 1),
    ("main steam pressure", "10MAA10CP001", 2),
    ("feedwater pump with high pressure in building 103", "10LAB10AP001", 1),
    ("abnormal outlet temperature superheater in the boiler building basement", "10HAH10BB001", 1),
    ("10LABI0AA102 stuck", "10LAB20AA102", 1),
];

pub const EVAL_LIVE_QUERIES_EN: [(&str, &str, usize); 7] = [
    ("feedwater pump with high pressure in building 103", "10LAB10AP001", 1),
    // 영어 어순으로는 머리 명사가 temperature 라 센서(과열기 출구 증기 온도)가 1위인 것도 맞는 읽기입니다
    ("abnormal superheater outlet temperature in the boiler building basement", "10HAH10BB001", 2),
    ("10LABI0AA102 stuck", "10LAB20AA102", 1),
    ("stuck valves", "10LAB20AA102", 1),
    ("low feedwater flow", "10LAB20AA102", 1),
    ("sensors with high temperature", "10HAH10CT001", 1),
    ("abnormal equipment in building 103", "10LAB20AA102", 2),
];

pub const EVAL_AIDC_QUERIES_EN: [(&str, &str, usize); 10] = [
    ("valves on floor 2 of building 103", "DC1CDU20AA02", 2),
    ("CDU bypass valve", "DC1CDU20AA03", 1),
    ("GPU junction temperature sensor", "DC1GPU10CT01", 2),
    ("Pod A manifold", "DC1MNF10MV01", 1),
    ("DC1CDUI0AP01", "DC1CDU10AP01", 1),
    ("battery cell temperature", "DC1BES10CT01", 1),
    ("cooling tower fan", "DC1CTW10AN01", 2),
    ("thermal runaway procedure", "EOP-104-BAT01", 1),
    ("building 105 drawings", "MEP-105-CHL", 1),
    ("pending MOP", "MOP-2026-1021", 1),
];

pub const EVAL_AIDC_LIVE_QUERIES_EN: [(&str, &str, usize); 4] = [
    ("stuck valves", "DC1CDU20AA02", 1),
    ("GPU racks with high temperature", "DC1GPU10RK01", 1),
    ("pump with high pressure in building 103", "DC1CDU10AP01", 1),
    ("low coolant flow", "DC1CDU20AA02", 2),
];

/// 뜻은 같고 원장 낱말과 글자가 다른 질의 (docs/search.md "하지 않은 것" 의 예: "보일러 물 공급 문제").
/// 규칙 · 글자 경로로는 못 찾고 임베딩 경로 · NMS 조각이 있어야 찾는 몫을 잽니다. 질의는 원장을 보며 직접 만든 것이라 방향만 봅니다.
pub const EVAL_PARAPHRASE_KO: [(&str, &str, usize); 8] = [
    ("보일러에 물을 보내는 펌프", "10LAB10AP001", 3),
    ("연기를 빨아내는 팬", "10HLB10AN001", 3),
    ("석탄을 가루로 만드는 설비", "10HFC10AJ001", 3),
    ("굴뚝", "10HTA10BB001", 3),
    ("증기를 다시 데우는 설비", "10HAJ10BB001", 3),
    ("터빈 축 떨림", "10MAD10CY001", 3),
    ("발전량", "10MKA10CE001", 3),
    ("황산화물 배출", "10HTB10CQ001", 3),
];

pub const EVAL_PARAPHRASE_EN: [(&str, &str, usize); 8] = [
    ("boiler feed pump", "10LAB10AP001", 3),
    ("induced draft fan", "10HLB10AN001", 3),
    ("coal pulverizer", "10HFC10AJ001", 3),
    ("chimney", "10HTA10BB001", 3),
    ("steam reheating", "10HAJ10BB001", 3),
    ("turbine shaft vibration", "10MAD10CY001", 3),
    ("power generated", "10MKA10CE001", 3),
    ("sulfur dioxide emissions", "10HTB10CQ001", 3),
];

pub const EVAL_AIDC_PARAPHRASE: [(&str, &str, usize); 6] = [
    ("배터리 폭발 위험 절차", "EOP-104-BAT01", 3),
    ("불 끄는 가스 용기", "DC1FMS10BB01", 3),
    ("광모듈 온도", "DC1OPT10CT01", 3),
    ("battery fire risk procedure", "EOP-104-BAT01", 3),
    ("fire suppression gas tank", "DC1FMS10BB01", 3),
    ("network optics temperature", "DC1OPT10CT01", 3),
];

/// 시나리오를 끝 시각까지 재생한 지금 계측으로 검색 표를 만듭니다 (평가 · 시험용).
pub fn live_corpus(db: &Lattice, scenario: &str) -> Option<Corpus> {
    let set = crate::scenario::ScenarioSet::embedded_for(db.domain()).ok()?;
    let sc = set.get(scenario)?;
    let telemetry = sc.telemetry(&set);
    let at = sc.time(sc.now_s).timestamp_millis();
    Some(Corpus::build(db, Some((&telemetry, at)), None))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn top(r: &SearchResult, n: usize) -> Vec<String> {
        r.hits.iter().take(n).map(|h| h.doc.id.clone()).collect()
    }

    fn rank(r: &SearchResult, id: &str) -> Option<usize> {
        r.hits.iter().position(|h| h.doc.id == id).map(|p| p + 1)
    }

    use super::EVAL_QUERIES as QUERIES;

    /// 다국어 질의 (모델 없이 규칙만): 그 언어 사전의 닫힌 낱말이 같은 조건이 됩니다. 정답 · 허용 순위는 N:N 평가 질의(core/fixtures/search_ml)와 같습니다.
    #[test]
    fn multilingual_queries_ground_the_same_conditions_without_models() {
        let db = Lattice::embedded().unwrap();
        let still = Corpus::build(&db, None, None);
        let cases: [(&str, &str, &str, usize); 12] = [
            ("103号棟2階のバルブ", "ja", "10LAB20AA102", 2),
            ("103号楼2层的阀门", "zh-hans", "10LAB20AA102", 2),
            ("Pumpen im Keller", "de", "10LCA10AP001", 2),
            ("vannes du bâtiment 103 au 2e étage", "fr", "10LAB20AA102", 2),
            ("válvulas del edificio 103 en la planta 2", "es", "10LAB20AA102", 2),
            ("valvole dell'edificio 103 al secondo piano", "it", "10LAB20AA102", 2),
            ("válvulas do edifício 103 no 2º andar", "pt", "10LAB20AA102", 2),
            ("kleppen in gebouw 103 op de 2e verdieping", "nl", "10LAB20AA102", 2),
            ("ventily v budově 103 ve 2. patře", "cs", "10LAB20AA102", 2),
            ("الصمامات في المبنى 103 الطابق الثاني", "ar", "10LAB20AA102", 2),
            ("MOC en attente d'approbation", "fr", "MOC-2026-1021", 1),
            ("Überhitzer mit abnormaler Austrittstemperatur im Keller des Kesselhauses", "de", "10HAH10BB001", 1),
        ];
        for (q, lang, want, allow) in cases {
            let r = search(&db, &still, q);
            assert_eq!(r.lang, lang, "{q}");
            assert!(rank(&r, want).is_some_and(|k| k <= allow), "{q} → {want} 순위 {:?} · 위 {:?} · 조건 {:?}", rank(&r, want), top(&r, 5), r.conditions);
        }
        // 지금 계측(P2): 질적 상태 낱말이 필터가 됩니다 (고착 · 낮음)
        let live = live_corpus(&db, EVAL_LIVE_SCENARIO).unwrap();
        for q in ["festsitzende Ventile", "vannes bloquées", "固着したバルブ", "卡涩的阀门", "zaseknuté ventily"] {
            let r = search(&db, &live, q);
            assert_eq!(r.hits[0].doc.id, "10LAB20AA102", "{q} · 위 {:?} · 조건 {:?}", top(&r, 5), r.conditions);
        }
        // 부정어: 앞(de nicht · fr pas) · 뒤(ja ない) 모두 조건이 아님
        for q in ["nicht festsitzende Ventile", "vannes qui ne sont pas bloquées", "固着していないバルブ"] {
            let r = search(&db, &live, q);
            assert!(!r.conditions.iter().any(|c| c.text.contains("고착")), "{q}: {:?}", r.conditions);
        }
    }

    /// 다른 언어 원장(독일어): 동은 번호로, 계측 종류는 라벨 낱말(Überhitzer-Austrittsdampftemperatur → 온도)로 맞춥니다.
    #[test]
    fn a_german_ledger_answers_korean_and_english_queries() {
        let db = Lattice::from_json(include_str!("../../fixtures/search_ml/synthetic_db.de.json")).unwrap();
        let still = Corpus::build(&db, None, None);
        let r = search(&db, &still, "103동 2층 밸브");
        assert!(r.conditions.iter().any(|c| c.kind == "공간" && c.applied && c.text.contains("103")), "{:?}", r.conditions);
        assert!(rank(&r, "10LAB20AA102").is_some_and(|k| k <= 2), "{:?}", top(&r, 5));
        let r = search(&db, &still, "superheater temperature sensor");
        assert_eq!(r.hits[0].doc.id, "10HAH10CT001", "{:?} · {:?}", top(&r, 5), r.conditions);
        let r = search(&db, &still, "Temperatursensor Überhitzer");
        assert_eq!(r.hits[0].doc.id, "10HAH10CT001", "{:?} · {:?}", top(&r, 5), r.conditions);
        // 원장 이름을 따옴표로 넣은 질의: 그 설비가 연결 기준 (일본어 문장 속 독일어 이름)
        let r = search(&db, &still, "「Hauptspeisewasserpumpe A」の下流1ホップ");
        assert_eq!(r.lang, "ja");
        assert!(r.conditions.iter().any(|c| c.kind == "연결" && c.text.starts_with("10LAB10AP001")), "{:?}", r.conditions);
        assert!(rank(&r, "10LAB20AA102").is_some_and(|k| k <= 2), "{:?}", top(&r, 5));
    }

    #[test]
    fn qualitative_conditions_wait_for_live_values_and_then_filter() {
        let db = Lattice::embedded().unwrap();
        // 지금 계측이 없으면 질적 상태는 힌트로 내리고 이유를 남깁니다 (정답은 공간 · 종류 · 계측으로 1위).
        let still = Corpus::build(&db, None, None);
        let r = search(&db, &still, "103동 압력 높은 급수펌프");
        let c = r.conditions.iter().find(|c| c.text.contains("높음")).unwrap();
        assert!(!c.applied && c.note.contains("지금 계측이 없어"), "{c:?}");
        assert_eq!(rank(&r, "10LAB10AP001"), Some(1), "{:?}", top(&r, 5));
        // P2 재생 끝: 펌프 토출 29.4 MPa (정상 28.5) → 필터로 걸리고 "왜" 에 값이 나옵니다.
        let live = live_corpus(&db, EVAL_LIVE_SCENARIO).unwrap();
        let r = search(&db, &live, "103동 압력 높은 급수펌프");
        let c = r.conditions.iter().find(|c| c.text.contains("높음")).unwrap();
        assert!(c.applied, "{c:?}");
        assert_eq!(r.hits[0].doc.id, "10LAB10AP001");
        assert!(r.hits[0].why.iter().any(|w| w.contains("29.4") && w.contains("28.5")), "{:?}", r.hits[0].why);
        // 고착된 밸브: 103동 2층 밸브 둘 중 고착된 주 조절밸브만 조건을 통과합니다 (바이패스는 0 % 근처에서 흔들림 = 안정).
        let r = search(&db, &live, "고착된 밸브");
        assert_eq!(r.hits[0].doc.id, "10LAB20AA102", "{:?}", top(&r, 5));
        let passed: Vec<&Hit> = r.hits.iter().filter(|h| h.passed && !h.rescued).collect();
        assert!(passed.iter().all(|h| h.doc.kks.as_deref() == Some("10LAB20AA102")), "{:?}", passed.iter().map(|h| &h.doc.id).collect::<Vec<_>>());
        // 부정어가 붙으면 조건이 아닙니다.
        let r = search(&db, &live, "고착 아닌 밸브");
        assert!(!r.conditions.iter().any(|c| c.text.contains("고착")), "{:?}", r.conditions);
    }

    #[test]
    fn live_queries_put_the_answer_on_top() {
        let db = Lattice::embedded().unwrap();
        let live = live_corpus(&db, EVAL_LIVE_SCENARIO).unwrap();
        for (q, want, within) in EVAL_LIVE_QUERIES {
            let r = search(&db, &live, q);
            let k = rank(&r, want);
            assert!(k.is_some_and(|k| k <= within), "'{q}' → {want} 순위 {k:?} (위: {:?}) 조건 {:?}", top(&r, 5), r.conditions);
        }
    }


    #[test]
    fn plant_queries_put_the_answer_on_top_of_the_screen() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        let mut hit1 = 0;
        let mut mrr = 0.0;
        for (q, want, within) in QUERIES {
            let r = search(&db, &corpus, q);
            let k = rank(&r, want);
            assert!(k.is_some_and(|k| k <= within), "'{q}' → {want} 순위 {k:?} (위: {:?}) 조건 {:?} 기록 {:?}", top(&r, 5), r.conditions, r.notes);
            hit1 += usize::from(k == Some(1));
            mrr += 1.0 / k.unwrap() as f64;
        }
        let mrr = mrr / QUERIES.len() as f64;
        assert!(mrr >= 0.8, "MRR {mrr} (Hit@1 {hit1}/{})", QUERIES.len());
    }

    #[test]
    fn an_unsatisfiable_condition_becomes_a_hint_instead_of_erasing_the_answer() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        // 105동에는 밸브가 없습니다. "밸브" 조건이 105동 설비를 다 지우면 안 됩니다.
        let r = search(&db, &corpus, "105동 밸브 유인통풍기");
        assert!(r.conditions.iter().any(|c| c.kind == "설비 종류" && !c.applied), "{:?}", r.conditions);
        assert_eq!(top(&r, 1), vec!["10HLB10AN001".to_string()]);
    }

    #[test]
    fn an_exact_code_is_not_corrected_into_a_neighbouring_tag() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        // 설비 10LAB10AP001 은 태그 10LAB10CP001 과 한 글자 다릅니다. 원장에 그대로 있는 코드는 교정하지 않고 연결 기준으로 씁니다
        // (콘솔의 선택 설비 문맥 질의 "상류 1홉" → "10LAB10AP001 상류 1홉").
        let r = search(&db, &corpus, "10LAB10AP001 상류 1홉");
        assert!(!r.notes.iter().any(|n| n.contains("식별자 교정")), "{:?}", r.notes);
        assert!(r.notes.iter().any(|n| n.contains("연결 기준 설비 10LAB10AP001")), "{:?}", r.notes);
        assert!(r.hits[0].passed && r.hits[0].doc.kks.as_deref() != Some("10LAB10AP001"), "{:?}", top(&r, 5));
        // 오독은 여전히 교정합니다
        let r = search(&db, &corpus, "10LABI0AA102 상류 1홉");
        assert!(r.notes.iter().any(|n| n.contains("식별자 교정") && n.contains("10LAB20AA102")), "{:?}", r.notes);
    }

    #[test]
    fn a_dcim_asset_code_anchors_a_relational_query() {
        // DCIM 코드는 글자로 시작합니다 (DC1CDU20AA02). 식별자로 읽혀야 "상류 1홉" 의 기준이 됩니다.
        let db = Lattice::embedded_for(crate::domain::DomainMode::AiDataCenter).unwrap();
        let corpus = Corpus::build(&db, None, None);
        let r = search(&db, &corpus, "DC1CDU20AA02 상류 1홉");
        assert!(r.notes.iter().any(|n| n.contains("연결 기준 설비 DC1CDU20AA02")), "{:?}", r.notes);
        assert!(r.conditions.iter().any(|c| c.applied && c.text.contains("DC1CDU20AA02")), "{:?}", r.conditions);
    }

    #[test]
    fn a_negated_state_word_never_becomes_a_filter() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        let r = search(&db, &corpus, "밸브 아닌 103동 설비");
        assert!(!r.conditions.iter().any(|c| c.kind == "설비 종류"), "{:?}", r.conditions);
        assert!(r.notes.iter().any(|n| n.contains("부정어")));
        // 영어는 부정어가 앞에 옵니다
        let live = live_corpus(&db, EVAL_LIVE_SCENARIO).unwrap();
        let r = search(&db, &live, "valves that are not stuck");
        assert!(!r.conditions.iter().any(|c| c.text.contains("고착")), "{:?}", r.conditions);
        assert!(r.conditions.iter().any(|c| c.kind == "설비 종류" && c.applied), "{:?}", r.conditions);
    }

    /// 화면이 한국어로 옮기지 않고 엔진이 영어를 그대로 읽습니다 (모델 없이: 낱말 목록 · 규칙 단수화 · 영어 이름).
    fn assert_set(db: &Lattice, corpus: &Corpus, set: &[(&str, &str, usize)], min_mrr: f64) {
        let mut mrr = 0.0;
        for (q, want, within) in set {
            let r = search(db, corpus, q);
            let k = rank(&r, want);
            assert!(k.is_some_and(|k| k <= *within), "'{q}' → {want} 순위 {k:?} (위: {:?}) 조건 {:?} 기록 {:?}", top(&r, 5), r.conditions, r.notes);
            mrr += 1.0 / k.unwrap() as f64;
        }
        let mrr = mrr / set.len() as f64;
        assert!(mrr >= min_mrr, "MRR {mrr}");
    }

    #[test]
    fn english_queries_work_without_models() {
        let db = Lattice::embedded().unwrap();
        assert_set(&db, &Corpus::build(&db, None, None), &EVAL_QUERIES_EN, 0.9);
        assert_set(&db, &live_corpus(&db, EVAL_LIVE_SCENARIO).unwrap(), &EVAL_LIVE_QUERIES_EN, 0.85);
        let dc = Lattice::embedded_for(crate::domain::DomainMode::AiDataCenter).unwrap();
        assert_set(&dc, &Corpus::build(&dc, None, None), &EVAL_AIDC_QUERIES_EN, 0.85);
        assert_set(&dc, &live_corpus(&dc, EVAL_AIDC_LIVE_SCENARIO).unwrap(), &EVAL_AIDC_LIVE_QUERIES_EN, 0.9);
    }

    #[test]
    fn english_conditions_mirror_the_korean_ones() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        let kinds = |r: &SearchResult| r.conditions.iter().map(|c| format!("{} {}", c.kind, c.text)).collect::<Vec<_>>();
        // 동 · 층 · 종류 (복수형 · 서수)
        let r = search(&db, &corpus, "valves on the 2nd floor of building 103");
        let k = kinds(&r);
        assert!(k.iter().any(|c| c.starts_with("공간") && c.contains("103동")) && k.iter().any(|c| c == "층 2층") && k.iter().any(|c| c == "설비 종류 valve"), "{k:?}");
        // 비교어는 숫자 앞 ("above 600 degrees") 과 뒤 ("600 or more")
        let r = search(&db, &corpus, "sensors above 600 degrees");
        assert!(kinds(&r).iter().any(|c| c.contains("온도 > 600")), "{:?}", kinds(&r));
        let r = search(&db, &corpus, "flow 1,400 t/h or less");
        assert!(kinds(&r).iter().any(|c| c.contains("유량 ≤ 1,400") || c.contains("유량 ≤ 1400")), "{:?}", kinds(&r));
        // "higher than" 은 질적 상태가 아니라 비교어
        let r = search(&db, &corpus, "temperature higher than 600");
        assert!(!kinds(&r).iter().any(|c| c.contains("높음")), "{:?}", kinds(&r));
        // 연결: 영어 이름으로 기준 설비 ('upstream' 의 up · ps 가 UPS 에 걸리지 않게)
        let r = search(&db, &corpus, "main feedwater flow control valve 1 hop upstream");
        assert!(r.notes.iter().any(|n| n.contains("연결 기준 설비 10LAB20AA102")), "{:?}", r.notes);
        assert_eq!(top(&r, 1), vec!["10LAB10AP001".to_string()]);
        // 복합 이름 안의 계측어는 조건이 아님 ("flow control valve" 의 flow)
        assert!(!kinds(&r).iter().any(|c| c.starts_with("계측")), "{:?}", kinds(&r));
        // 호기 표시 (Pod A ≠ Pod B)
        let dc = Lattice::embedded_for(crate::domain::DomainMode::AiDataCenter).unwrap();
        let r = search(&dc, &Corpus::build(&dc, None, None), "Pod A manifold");
        assert_eq!(top(&r, 1), vec!["DC1MNF10MV01".to_string()]);
    }

    #[test]
    fn the_e2e_english_queries_need_no_translation() {
        // scripts/e2e/console.js 의 영어 질의 (예전에는 화면의 i18n/query.ts 가 한국어로 옮겨 보냈습니다)
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        for (q, want) in [("valves on floor 2 of building 103", "10LAB20AA10"), ("main feedwater flow control valve 1 hop upstream", "10LAB10AP001"), ("building 105 valve ID fan", "10HLB10AN001"), ("pending MOC", "MOC-2026-1021")] {
            let r = search(&db, &corpus, q);
            assert!(r.hits[0].doc.id.starts_with(want), "'{q}' → {:?}", top(&r, 3));
            assert_eq!(r.lang, "en");
        }
        // 문맥 질의 (선택 설비 코드를 앞에 붙임)
        let r = search(&db, &corpus, "10LAB10AP001 1 hop upstream");
        assert!(r.notes.iter().any(|n| n.contains("연결 기준 설비 10LAB10AP001")), "{:?}", r.notes);
    }

    #[test]
    fn live_values_drive_level_and_number_conditions() {
        let db = Lattice::embedded().unwrap();
        let set = crate::scenario::ScenarioSet::embedded().unwrap();
        let s = set.get("P2").unwrap();
        let t = s.telemetry(&set);
        let at = s.time(s.now_s).timestamp_millis();
        let corpus = Corpus::build(&db, Some((&t, at)), None);
        // P2 지금: 밸브 개도 18 % (경보), 과열기 627 ℃ (경보)
        let r = search(&db, &corpus, "경보 상태 센서");
        let ids = top(&r, 6);
        assert!(ids.contains(&"10LAB20CG102".to_string()) && ids.contains(&"10HAH10CT001".to_string()), "{ids:?}");
        // 급수 유량 1,119 t/h 만 1,400 미만입니다 (복수 유량 1,500 은 아님)
        let r = search(&db, &corpus, "유량 1,400 t/h 미만");
        assert!(rank(&r, "10LAB20CF001").is_some_and(|k| k <= 2), "{:?} {:?}", top(&r, 4), r.conditions);
        // 조건을 통과한 것은 급수 유량(설비 · 센서)뿐이고, 복수 유량은 통과하지 못한 나머지로만 뒤에 붙습니다.
        let passed: Vec<String> = r.hits.iter().filter(|h| h.passed).map(|h| h.doc.id.clone()).collect();
        assert!(passed.contains(&"10LAB20CF001".to_string()) && !passed.contains(&"10LCA10CF001".to_string()), "{passed:?}");
    }
}
