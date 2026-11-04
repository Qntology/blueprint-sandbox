//! 자연어 검색 (logis-center `ai_search_complex` 의 설계를 발전소 원장에 옮김).
//!
//! 주제어실 검색창에 "103동 2층 밸브", "과열기 온도 600도 이상", "급수조절밸브 상류 2홉", "승인 대기 MOC" 처럼 씁니다.
//! logis-center 에서 실측으로 얻은 규칙을 그대로 지킵니다 (`docs/search.md`).
//! 1. 넓게 찾고, 조건은 따로 둡니다. 범위(유형)만으로 후보를 모으고 조건은 필터 계획으로 나중에 겁니다.
//! 2. 조건이 정답을 지우면 안 됩니다. 조건마다 원장에 맞는 항목이 하나라도 있는지 대조하고(grounding), 없으면 힌트(가점)로 내립니다.
//!    조건이 하나도 없는 계획은 모두 통과시키고, 조건에 맞는 항목은 전체 표에서 한 번 더 찾아(구제) 뒤에 붙입니다.
//! 3. 질의 전체 경로를 파서와 나란히 돌리고 순위로 합칩니다 (RRF k = 20, 글자 일치 ×2). 점수는 경로마다 척도가 달라 더하지 않습니다.
//! 4. 유형을 먼저 정합니다. 고정 지시어로 정하고 없으면 설비가 기본이며, 유형 제한 경로를 따로 둡니다.
//! 5. 품사 분석으로 낱말을 지우지 않습니다 (공백 분리). 숫자 · 비교어 · 상태어는 고정 낱말 목록으로 코드가 뽑습니다 (모델 아님).
//! 6. 부정어(아닌 · 제외 · 빼고 · 없는)가 붙은 상태어는 상태 필터가 되지 않습니다.
//! 7. 식별자(KKS · 태그 · MOC · 도면 · 사건 번호)는 하이픈째 통째로 두고, KKS 오독은 원장으로 교정합니다 (편집 거리 2, 유일할 때만).
//! 8. 모든 결정에 사유를 남깁니다 (화면의 "왜 찾았나").
//!
//! 판단 모델은 쓰지 않습니다. logis-center 에서도 모델은 후보가 둘 이상일 때 고르는 데만 썼고, 이 원장 크기(설비 35)에서는 규칙으로 충분합니다.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use serde::Serialize;

use crate::lattice::{KksMatch, Lattice};
use crate::plant::fmt_value;
use crate::store::Store;
use crate::timeseries::{classify_with, Baseline, SignalState, Telemetry};

/// RRF 상수와 경로 가중치 (logis-center `search_fusion.rs` 와 같음)
pub const FUSION_K: f32 = 20.0;
pub const LITERAL_WEIGHT: f32 = 2.0;
/// 화면에 내는 최대 개수
pub const LIMIT: usize = 30;

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
    (DocKind::Archive, &["보관", "확정된", "처리 완료", "처리된", "이력", "mcr-"]),
    (DocKind::Aop, &["절차서", "aop", "조치 절차", "대응 절차"]),
    (DocKind::Moc, &["moc", "변경관리", "레드라인", "red-line", "변경 승인"]),
    (DocKind::Drawing, &["도면", "p&id", "pid-", "계통도"]),
];

/// 설비 종류 낱말 → 원장 `kind`
const KIND_WORDS: [(&str, &[&str]); 18] = [
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
];

/// 계측 종류 낱말 (센서 라벨 · 단위로 맞춥니다)
const MEASURE_WORDS: [(&str, &[&str]); 9] = [
    ("온도", &["℃", "°c"]),
    ("압력", &["mpa", "kpa"]),
    ("유량", &["t/h"]),
    ("진동", &["μm", "um"]),
    ("개도", &["%"]),
    ("전류", &["a"]),
    ("수위", &["%"]),
    ("출력", &["mw"]),
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
                text: format!("{} {} {} {} {} {} {}", s.tag, s.label, s.unit, e.kks, e.name, e.kind, db.address(&e.kks).map(|a| a.place()).unwrap_or_default()),
                sector: Some(e.sector.clone()),
                floor: Some(e.floor.clone()),
                kinds: vec![e.kind.clone()],
                measures: vec![(s.label.clone(), s.unit.clone(), v)],
                level: v.filter(|_| live.is_some()).map(|v| level_of(s, v)),
                status: None,
                signals: signal_of(s).into_iter().collect(),
            });
        }
        for a in db.aops() {
            docs.push(Doc {
                kind: DocKind::Aop,
                id: a.id.clone(),
                title: a.title.clone(),
                subtitle: format!("{}{}", a.steps.first().cloned().unwrap_or_default(), if a.high_risk { " · 고위험 (공동 결재)" } else { "" }),
                kks: a.triggers.first().and_then(|t| db.sensor(t)).map(|s| s.kks.clone()),
                text: format!("{} {} {} {} {}", a.id, a.title, a.steps.join(" "), a.risk_actions.join(" "), a.triggers.join(" ")),
                sector: a.sectors.first().cloned(),
                floor: None,
                kinds: Vec::new(),
                measures: Vec::new(),
                level: None,
                status: Some(if a.high_risk { "고위험".into() } else { "일반".into() }),
                signals: Vec::new(),
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
                text: format!("{} {} {} {} {} {} {}", m.moc_no, m.kks, m.tag, m.param, m.reason, status, m.status),
                sector: db.equipment(&m.kks).map(|e| e.sector.clone()),
                floor: db.equipment(&m.kks).map(|e| e.floor.clone()),
                kinds: db.equipment(&m.kks).map(|e| vec![e.kind.clone()]).unwrap_or_default(),
                measures: vec![(m.param.clone(), m.unit.clone(), Some(m.new))],
                level: None,
                status: Some(status.to_string()),
                signals: Vec::new(),
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
                text: format!("{} {} {} {}", d.id, d.title, d.rev, sector.map(|s| format!("{} {}", s.building, s.label)).unwrap_or_default()),
                sector: Some(d.sector.clone()),
                floor: None,
                kinds: Vec::new(),
                measures: Vec::new(),
                level: None,
                status: None,
                signals: Vec::new(),
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
                });
            }
        }
        let mut df = HashMap::new();
        for d in &mut docs {
            d.text = d.text.to_lowercase();
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
    let unit = unit.to_lowercase();
    MEASURE_WORDS.iter().find(|(w, _)| *w == word).is_some_and(|(_, units)| units.contains(&unit.as_str()) && word != "수위" && word != "개도")
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
    } else if u.starts_with("mpa") || u.starts_with("kpa") {
        Some("압력")
    } else if u.starts_with("t/h") {
        Some("유량")
    } else if u.starts_with("μm") || u.starts_with("um") {
        Some("진동")
    } else if u.starts_with("mw") {
        Some("출력")
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

/// 자연어 질의 하나
pub fn search(db: &Lattice, corpus: &Corpus, query: &str) -> SearchResult {
    let q = query.trim().to_lowercase();
    let words: Vec<String> = q.split_whitespace().map(str::to_string).collect();
    let mut notes = Vec::new();

    // 1) 유형 먼저
    let mut types: Vec<DocKind> = TYPE_WORDS.iter().filter(|(_, ws)| ws.iter().any(|w| q.contains(w))).map(|(k, _)| *k).collect();
    if types.is_empty() {
        // 한국어 명사구는 끝 낱말이 머리입니다 ("출구온도 비정상 과열기" 는 과열기를 묻습니다). 머리가 설비 종류 낱말이면 설비만 찾습니다.
        let head = words.last().map(|w| w.trim_end_matches(['을', '를', '이', '가', '은', '는'])).unwrap_or_default();
        let kind_head = KIND_WORDS.iter().any(|(k, _)| head.ends_with(k)) || ["설비", "장비", "기기"].contains(&head);
        if kind_head {
            types = vec![DocKind::Equipment];
            notes.push(format!("유형 지시어 없음 · 머리 낱말 '{head}' → 설비"));
        } else {
            types = vec![DocKind::Equipment, DocKind::Sensor];
            notes.push("유형 지시어 없음 → 설비(기본) · 계측".into());
        }
    }

    // 2) 식별자: KKS · 태그는 원장으로 교정
    let mut ids: Vec<String> = Vec::new();
    for w in &words {
        let raw = w.trim_matches(|c: char| !c.is_alphanumeric() && c != '-').to_uppercase();
        if raw.len() == 12 && raw.starts_with(|c: char| c.is_ascii_digit()) {
            match db.match_tag(&raw) {
                KksMatch::Exact(c) => ids.push(c),
                KksMatch::Corrected { code, raw, distance } => {
                    notes.push(format!("식별자 교정 {raw} → {code} (편집 거리 {distance})"));
                    ids.push(code);
                }
                _ => match db.match_kks(&raw) {
                    KksMatch::Exact(c) => ids.push(c),
                    KksMatch::Corrected { code, raw, distance } => {
                        notes.push(format!("식별자 교정 {raw} → {code} (편집 거리 {distance})"));
                        ids.push(code);
                    }
                    _ => {}
                },
            }
        } else if raw.contains('-') && raw.len() >= 8 {
            ids.push(raw);
        }
    }

    // 3) 조건 (부정어가 바로 뒤에 붙은 낱말은 조건으로 쓰지 않습니다)
    let negated = |i: usize| words.get(i + 1).is_some_and(|n| NEGATIONS.iter().any(|x| n.starts_with(x))) || NEGATIONS.iter().any(|x| words[i].ends_with(x) && words[i].len() > x.len());
    let mut conds: Vec<Cond> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        if negated(i) {
            notes.push(format!("부정어가 붙은 '{w}' 는 조건으로 쓰지 않습니다"));
            continue;
        }
        // 동 조건은 동 번호(103동)나 동 이름 전체(보일러동 · 급수동)일 때만. "주급수" 같은 설비 이름 조각은 글자 경로에 맡깁니다.
        for s in db.sectors() {
            let b = s.building.to_lowercase();
            let label = s.label.replace('·', "").to_lowercase();
            let stem = format!("{}동", s.label.split('·').next().unwrap_or(&s.label).trim_end_matches('동'));
            if w.starts_with(&b) || *w == label || *w == stem {
                conds.push(Cond::Sector(s.key.clone()));
            }
        }
        let floor = if w.starts_with("지하") || w.starts_with("b1") {
            Some("B1F")
        } else if w.starts_with("옥외") || w.starts_with("야외") {
            Some("OUT")
        } else {
            w.strip_suffix("층").or_else(|| w.strip_suffix('f')).and_then(|n| n.parse::<u32>().ok()).and_then(|n| match n {
                1 => Some("1F"),
                2 => Some("2F"),
                3 => Some("3F"),
                4 => Some("4F"),
                5 => Some("5F"),
                _ => None,
            })
        };
        if let Some(f) = floor {
            conds.push(Cond::Floor(f.into()));
        }
        if let Some((_, kinds)) = KIND_WORDS.iter().find(|(k, _)| w.ends_with(k) || w == k) {
            conds.push(Cond::Kind(kinds.iter().map(|x| x.to_string()).collect()));
        }
        let compound_name = KIND_WORDS.iter().any(|(k, _)| w.ends_with(k) && w.as_str() != *k);
        if let Some((m, _)) = MEASURE_WORDS.iter().find(|(m, _)| w.contains(m)).filter(|_| !compound_name) {
            conds.push(Cond::Measure(m.to_string()));
        }
        // 숫자 + 비교어 (같은 낱말 안이거나 바로 다음 낱말)
        if let Some((value, rest)) = number_unit(w) {
            // "600도 이상" · "30MPa이하" · "1,400 t/h 미만": 비교어는 두 낱말 앞까지, 단위는 붙어 있거나 바로 다음 낱말
            let next = words.get(i + 1).cloned().unwrap_or_default();
            let unit = if rest.is_empty() && unit_measure(&next).is_some() { next.clone() } else { rest.clone() };
            let cmp_text = format!("{rest} {next} {}", words.get(i + 2).cloned().unwrap_or_default());
            if let Some((_, op)) = COMPARATORS.iter().find(|(c, _)| cmp_text.contains(c)) {
                let measure = unit_measure(&unit).map(str::to_string).or_else(|| words[..i].iter().rev().take(2).find_map(|p| MEASURE_WORDS.iter().find(|(m, _)| p.contains(m)).map(|(m, _)| m.to_string())));
                conds.push(Cond::Number { measure, op: *op, value });
            }
        }
        if w.starts_with("트립") {
            conds.push(Cond::Level("trip"));
        } else if (w.starts_with("경보") || w.starts_with("알람")) && !q.contains("경보 목록") && !q.contains("알람 목록") {
            conds.push(Cond::Level("alarm"));
        } else if w.starts_with("정상") {
            conds.push(Cond::Level("normal"));
        }
        // 질적 상태: 계측 낱말은 같은 낱말 안 → 앞 두 낱말 → 바로 다음 낱말 차례로 찾습니다 ("압력 높은", "출구온도 비정상", "높은 압력")
        if let Some(want) = quality_of(w) {
            let measure_in = |x: &String| MEASURE_WORDS.iter().find(|(m, _)| x.contains(m)).map(|(m, _)| m.to_string());
            let measure = measure_in(w).or_else(|| words[..i].iter().rev().take(2).find_map(measure_in)).or_else(|| words.get(i + 1).and_then(measure_in));
            conds.push(Cond::Signal { measure, want });
        }
        if w.contains("대기") && types.contains(&DocKind::Moc) {
            conds.push(Cond::Status("승인 대기".into()));
        } else if w.starts_with("승인") && types.contains(&DocKind::Moc) && !q.contains("대기") {
            conds.push(Cond::Status("승인".into()));
        } else if w.starts_with("고위험") && types.contains(&DocKind::Aop) {
            conds.push(Cond::Status("고위험".into()));
        }
    }
    // 연결: "상류 · 하류 N홉" 은 기준 설비가 있어야 합니다 (식별자, 없으면 이름이 가장 많이 겹치는 설비)
    let up = q.contains("상류");
    let down = q.contains("하류");
    let mut near: BTreeMap<String, i32> = BTreeMap::new();
    if up || down || q.contains("주변") || q.contains("연결") {
        let hops = words.iter().find_map(|w| w.strip_suffix("홉").and_then(|n| n.parse::<usize>().ok())).unwrap_or(crate::graph::MAX_HOPS).min(crate::graph::MAX_HOPS);
        let anchor = ids.iter().find_map(|i| db.equipment(i).map(|e| e.kks.clone()).or_else(|| db.sensor(i).map(|s| s.kks.clone()))).or_else(|| {
            db.equipment_all()
                .iter()
                .map(|e| (bigrams(&e.name.to_lowercase()).intersection(&bigrams(&q)).count(), e))
                .filter(|(n, _)| *n >= 2)
                .max_by_key(|(n, _)| *n)
                .map(|(_, e)| e.kks.clone())
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
            hard.push(c);
        } else {
            hints.push(c);
        }
    }
    let passes = |d: &Doc| -> bool { passes_all(&hard, d, db, &near) };

    // 5) 경로: 파서(조건 통과 + 힌트 수) · 글자 일치(idf) · 2글자 n-gram, 그리고 유형 제한 경로
    let n = corpus.docs.len() as f32;
    let qterms = terms_of(&q);
    let literal_score = |d: &Doc| -> f32 {
        let mut s: f32 = qterms.iter().filter(|t| d.text.contains(t.as_str())).map(|t| ((n + 1.0) / (*corpus.df.get(t).unwrap_or(&0) as f32 + 0.5)).ln()).sum();
        s += ids.iter().filter(|id| d.id.eq_ignore_ascii_case(id) || d.text.contains(&id.to_lowercase())).count() as f32 * 10.0;
        s
    };
    let qgrams = bigrams(&q);
    let gram_score = |d: &Doc| -> f32 {
        let g = bigrams(&format!("{} {}", d.title.to_lowercase(), d.id.to_lowercase()));
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
    notes.push(format!("경로 크기 파서 {} · 글자 {} · n-gram {} · 유형 글자 {} · 유형 n-gram {}", parser.len(), literal.len(), grams.len(), typed_literal.len(), typed_grams.len()));
    let fused = rrf(&[(&parser, 1.0), (&literal, LITERAL_WEIGHT), (&grams, 1.0), (&typed_literal, 1.0), (&typed_grams, 1.0)]);

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
        w.extend(qterms.iter().filter(|t| d.text.contains(t.as_str())).map(|t| format!("'{t}'")));
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
    SearchResult { query: query.to_string(), types, conditions, notes, hits }
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

/// 시나리오를 끝 시각까지 재생한 지금 계측으로 검색 표를 만듭니다 (평가 · 시험용).
pub fn live_corpus(db: &Lattice, scenario: &str) -> Option<Corpus> {
    let set = crate::scenario::ScenarioSet::embedded().ok()?;
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
    fn a_negated_state_word_never_becomes_a_filter() {
        let db = Lattice::embedded().unwrap();
        let corpus = Corpus::build(&db, None, None);
        let r = search(&db, &corpus, "밸브 아닌 103동 설비");
        assert!(!r.conditions.iter().any(|c| c.kind == "설비 종류"), "{:?}", r.conditions);
        assert!(r.notes.iter().any(|n| n.contains("부정어")));
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
