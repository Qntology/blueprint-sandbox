//! 합성 시드 원장 (`Lattice`). 발전소 설비 대장 · 센서 기준치 · 계통 간선 · 도면 간 연결점 · DCS 보팅 · 변경관리(MOC) 원장 ·
//! 근무자 권한 · 비정상 운전절차서(AOP) 목록을 한 파일(`fixtures/synthetic_db.json`)로 미리 넣어 둔 상태로 진단합니다.
//!
//! 발전소 DCS · CMMS 연계가 없어서 시드를 씁니다. 값은 모두 합성이며, 실제 연계가 생기면 이 모듈의 조회 함수 자리가 바뀝니다.
//! 정의는 `docs/synthetic-plant-data.md` 에 있습니다.
//!
//! 도면 ↔ 원장 상호작용 ① (KKS 오독 교정): [`Lattice::match_kks`] 가 OCR 글자를 자리별로 고친 뒤 대장과 편집 거리 2 이내로 맞춥니다.
//! 가까운 코드가 둘 이상이면 고르지 않고 사람에게 넘깁니다.

use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use crate::plant::{repair_kks, Medium, Sector};

const EMBEDDED: &str = include_str!("../fixtures/synthetic_db.json");

/// 편집 거리로 교정할 때 허용하는 최대 글자 수 (기획안: `edit_distance <= 2`)
pub const MAX_KKS_DISTANCE: usize = 2;

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct UnitRow {
    pub code: String,
    pub label: String,
    pub capacity_mw: u32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PlantRow {
    pub name: String,
    pub kind: String,
    pub units: Vec<UnitRow>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SectorRow {
    pub key: String,
    pub code: String,
    pub building: String,
    pub label: String,
    pub kks: String,
    pub drawing: String,
    pub blurb: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct FloorRow {
    pub code: String,
    pub label: String,
    pub elevation_m: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct DrawingRow {
    pub id: String,
    pub sector: String,
    pub title: String,
    pub rev: String,
    pub issued: NaiveDate,
    pub signer: String,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EquipmentRow {
    pub kks: String,
    pub name: String,
    pub sector: String,
    pub floor: String,
    pub drawing: String,
    pub kind: String,
    /// 도면 안의 정규화 좌표 `[x0, y0, x1, y1]` (사전 인덱싱 결과)
    pub bbox: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SensorRow {
    pub tag: String,
    pub kks: String,
    pub label: String,
    pub unit: String,
    pub normal: f64,
    #[serde(default)]
    pub alarm_low: Option<f64>,
    #[serde(default)]
    pub alarm_high: Option<f64>,
    #[serde(default)]
    pub trip_low: Option<f64>,
    #[serde(default)]
    pub trip_high: Option<f64>,
    /// 이 폭 안의 흔들림은 상태 변이로 보지 않습니다 (SDS 데드밴드)
    pub deadband: f64,
    /// 물리적으로 가능한 초당 최대 변화량. 넘었다가 돌아오면 전자기 잡음입니다.
    pub max_slew: f64,
}

impl SensorRow {
    /// 값이 경보 한계를 넘었는가 (`Some(true)` 높음, `Some(false)` 낮음)
    pub fn alarm_side(&self, v: f64) -> Option<bool> {
        if self.alarm_high.is_some_and(|h| v >= h) {
            Some(true)
        } else if self.alarm_low.is_some_and(|l| v <= l) {
            Some(false)
        } else {
            None
        }
    }

    pub fn tripped(&self, v: f64) -> bool {
        self.trip_high.is_some_and(|h| v >= h) || self.trip_low.is_some_and(|l| v <= l)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EdgeRow {
    pub from: String,
    pub to: String,
    pub medium: String,
    pub label: String,
    /// 배관 길이 · 유속 기준 물리적 전파 지연 범위 (초)
    pub lag: [f64; 2],
    /// 이 간선의 흐름을 재는 센서 (프로세스 마이닝의 간선 굵기)
    #[serde(default)]
    pub flow_tag: Option<String>,
}

impl EdgeRow {
    pub fn medium(&self) -> Option<Medium> {
        Medium::from_key(&self.medium)
    }
}

/// 도면 간 점프 연결점 (Off-Page Connector). 출발 도면의 오른쪽 끝과 도착 도면의 왼쪽 입구에 있습니다.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct ConnectorRow {
    pub id: String,
    pub from: String,
    pub to: String,
    pub out_bbox: [f32; 4],
    pub in_bbox: [f32; 4],
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct VotingRow {
    pub tag: String,
    pub logic: String,
    pub channels: Vec<String>,
    /// 채널끼리 이만큼 안이면 같은 값으로 봅니다
    pub tolerance: f64,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct MocRow {
    pub moc_no: String,
    pub kks: String,
    pub tag: String,
    pub param: String,
    pub old: f64,
    pub new: f64,
    pub unit: String,
    pub design_limit: f64,
    pub approved_by: String,
    pub approved_on: Option<NaiveDate>,
    pub status: String,
    pub reason: String,
}

impl MocRow {
    pub fn approved(&self) -> bool {
        self.status == "approved" && self.approved_on.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct PersonRow {
    pub id: String,
    pub name: String,
    pub role: String,
    pub authorized_sectors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct AopRow {
    pub id: String,
    pub title: String,
    pub sectors: Vec<String>,
    pub triggers: Vec<String>,
    pub steps: Vec<String>,
    pub risk_actions: Vec<String>,
    pub high_risk: bool,
}

/// A0 대형 종합 도면 (기획 Part 8-4 1절). 마스터 도면들을 한 장에 앉힌 배치이고, 판독은 격자 타일로 나눠 합니다.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct A0Row {
    pub id: String,
    pub title: String,
    pub rev: String,
    pub size_mm: [u32; 2],
    pub dpi: u32,
    pub pixel_size: [u32; 2],
    /// 마스터 도면 한 장의 픽셀 크기
    pub sheet_px: [u32; 2],
    pub grid: A0Grid,
    pub sheets: Vec<A0Sheet>,
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
pub struct A0Grid {
    pub rows: u32,
    pub cols: u32,
    /// 이웃 타일과 겹치는 비율 (타일 크기 기준)
    pub overlap: f32,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct A0Sheet {
    pub drawing: String,
    pub x: u32,
    pub y: u32,
}

/// 절차서에 딸린 선행 절차 인터록 (기획 Part 8-4 3절). 단계 순서대로 확인해야 그 절차서를 확정할 수 있습니다.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InterlockRow {
    pub aop_id: String,
    /// 깊이 0 이 되는 설비 (절차서가 다루는 고장 설비)
    pub target: String,
    pub fault: String,
    pub steps: Vec<InterlockStepRow>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct InterlockStepRow {
    pub no: u32,
    /// 대상에서 본 홉 깊이 (음수 상류, 0 은 대상 자신이나 나란한 우회 설비)
    pub req_depth: i32,
    pub kks: String,
    pub check: InterlockCheck,
    pub text: String,
}

/// 단계가 확인하는 조건
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InterlockCheck {
    /// 계측값이 이 값 이하
    Below { tag: String, value: f64 },
    /// 계측값이 이 값 이상
    AtLeast { tag: String, value: f64 },
    /// 현장 안전작업허가 · 잠금표지(LOTO) 서명
    Loto,
}

/// 도면 기호 범례 한 줄: 심볼 모양과 그 모양으로 그리는 설비 종류 (`scripts/gen_pid_drawings.py` 와 같은 표)
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct SymbolLegendRow {
    pub class: String,
    pub label: String,
    pub shape: String,
    pub kinds: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct HistoryRow {
    pub date: NaiveDate,
    #[serde(default)]
    pub drawing: Option<String>,
    #[serde(default)]
    pub kks: Option<String>,
    pub kind: String,
    #[serde(default)]
    pub rev: Option<String>,
    #[serde(default)]
    pub moc: Option<String>,
    pub note: String,
}

#[derive(Debug, Clone, Deserialize)]
struct Seed {
    as_of: NaiveDate,
    plant: PlantRow,
    sectors: Vec<SectorRow>,
    floors: Vec<FloorRow>,
    drawings: Vec<DrawingRow>,
    equipment: Vec<EquipmentRow>,
    sensors: Vec<SensorRow>,
    edges: Vec<EdgeRow>,
    connectors: Vec<ConnectorRow>,
    voting: Vec<VotingRow>,
    moc: Vec<MocRow>,
    personnel: Vec<PersonRow>,
    aop: Vec<AopRow>,
    #[serde(default)]
    a0_drawings: Vec<A0Row>,
    #[serde(default)]
    interlocks: Vec<InterlockRow>,
    #[serde(default)]
    symbol_legend: Vec<SymbolLegendRow>,
    history: Vec<HistoryRow>,
}

/// Lv1 ~ Lv5 공간 주소 (상단 브레드크럼)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpatialAddress {
    pub unit_code: String,
    pub unit_label: String,
    pub sector: String,
    pub building: String,
    pub sector_label: String,
    pub floor: String,
    pub floor_label: String,
    pub elevation_m: f64,
    pub drawing: String,
    pub drawing_rev: String,
    pub kks: String,
    pub name: String,
    pub bbox: [f32; 4],
}

impl SpatialAddress {
    /// 층 · 고도 한 줄 (`2F (EL.+9.0m)`, 옥외는 `옥외`)
    pub fn floor_text(&self) -> String {
        if self.floor == "OUT" {
            "옥외".to_string()
        } else {
            format!("{} (EL.{:+.1}m)", self.floor, self.elevation_m)
        }
    }

    /// 사람이 읽는 위치 (`103동 2F (EL.+9.0m)`)
    pub fn place(&self) -> String {
        format!("{} {}", self.building, self.floor_text())
    }

    /// `Lv1 … Lv5` 다섯 칸
    pub fn breadcrumb(&self) -> [String; 5] {
        [
            format!("{} ({})", self.unit_label, self.unit_code),
            format!("{} {}", self.building, self.sector_label),
            self.floor_text(),
            format!("{} ({})", self.drawing, self.drawing_rev),
            self.kks.clone(),
        ]
    }
}

/// KKS · 태그를 대장과 맞춘 결과
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum KksMatch {
    /// 그대로 대장에 있음
    Exact(String),
    /// 고쳐서 하나로 정해짐. `distance` 는 원문과 고친 값의 편집 거리입니다.
    Corrected { raw: String, code: String, distance: usize },
    /// 가까운 코드가 둘 이상이라 고르지 않음
    Ambiguous(Vec<String>),
    Unknown,
}

impl KksMatch {
    pub fn code(&self) -> Option<&str> {
        match self {
            KksMatch::Exact(c) | KksMatch::Corrected { code: c, .. } => Some(c),
            _ => None,
        }
    }
}

/// 두 글의 편집 거리 (글자 단위)
pub fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut col: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.chars().enumerate() {
        let mut diag = col[0];
        col[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let next = (diag + (x != *y) as usize).min(col[j] + 1).min(col[j + 1] + 1);
            diag = col[j + 1];
            col[j + 1] = next;
        }
    }
    col[b.len()]
}

/// 합성 원장. 시작할 때 한 번 읽고 바꾸지 않습니다 (MOC 오버레이는 `store` 가 따로 듭니다).
#[derive(Debug, Clone)]
pub struct Lattice {
    seed: Seed,
    equipment: HashMap<String, usize>,
    sensors: HashMap<String, usize>,
    out_edges: HashMap<String, Vec<usize>>,
    in_edges: HashMap<String, Vec<usize>>,
}

impl Lattice {
    /// 저장소에 들어 있는 시드를 읽습니다.
    pub fn embedded() -> Result<Self> {
        Self::from_json(EMBEDDED)
    }

    pub fn from_json(text: &str) -> Result<Self> {
        let seed: Seed = serde_json::from_str(text).context("합성 원장 시드를 읽을 수 없습니다")?;
        let equipment: HashMap<String, usize> = seed.equipment.iter().enumerate().map(|(i, e)| (e.kks.clone(), i)).collect();
        let sensors: HashMap<String, usize> = seed.sensors.iter().enumerate().map(|(i, s)| (s.tag.clone(), i)).collect();
        let mut out_edges: HashMap<String, Vec<usize>> = HashMap::new();
        let mut in_edges: HashMap<String, Vec<usize>> = HashMap::new();
        for (i, e) in seed.edges.iter().enumerate() {
            out_edges.entry(e.from.clone()).or_default().push(i);
            in_edges.entry(e.to.clone()).or_default().push(i);
        }
        let db = Self { seed, equipment, sensors, out_edges, in_edges };
        db.validate()?;
        Ok(db)
    }

    /// 시드가 자기가 정의한 것만 가리키는지 봅니다. 어긋나면 진단이 엉뚱한 설비를 가리키므로 읽지 않습니다.
    fn validate(&self) -> Result<()> {
        let s = &self.seed;
        for e in &s.equipment {
            if Sector::from_key(&e.sector).is_none() || self.floor(&e.floor).is_none() || self.drawing(&e.drawing).is_none() {
                bail!("설비 {} 의 섹터 · 층 · 도면이 시드에 없습니다", e.kks);
            }
        }
        for x in &s.sensors {
            if self.equipment(&x.kks).is_none() {
                bail!("센서 {} 의 설비 {} 가 대장에 없습니다", x.tag, x.kks);
            }
        }
        for e in &s.edges {
            if self.equipment(&e.from).is_none() || self.equipment(&e.to).is_none() || e.medium().is_none() || e.lag[0] > e.lag[1] {
                bail!("간선 {} → {} 이 올바르지 않습니다", e.from, e.to);
            }
            if e.flow_tag.as_deref().is_some_and(|t| self.sensor(t).is_none()) {
                bail!("간선 {} → {} 의 흐름 센서가 없습니다", e.from, e.to);
            }
        }
        for c in &s.connectors {
            if self.edge_between(&c.from, &c.to).is_none() {
                bail!("연결점 {} 에 해당하는 간선이 없습니다", c.id);
            }
        }
        for m in &s.moc {
            if self.sensor(&m.tag).is_none() || self.person(&m.approved_by).is_none() {
                bail!("MOC {} 의 태그나 승인자가 없습니다", m.moc_no);
            }
        }
        for a0 in &s.a0_drawings {
            for sh in &a0.sheets {
                let fits = sh.x + a0.sheet_px[0] <= a0.pixel_size[0] && sh.y + a0.sheet_px[1] <= a0.pixel_size[1];
                if self.drawing(&sh.drawing).is_none() || !fits {
                    bail!("A0 도면 {} 의 {} 배치가 올바르지 않습니다", a0.id, sh.drawing);
                }
            }
            if a0.grid.rows == 0 || a0.grid.cols == 0 || !(0.0..0.5).contains(&a0.grid.overlap) {
                bail!("A0 도면 {} 의 타일 격자가 올바르지 않습니다", a0.id);
            }
        }
        let mut kinds_seen: Vec<&str> = Vec::new();
        for (i, row) in s.symbol_legend.iter().enumerate() {
            if row.class == "none" || s.symbol_legend[..i].iter().any(|r| r.class == row.class) {
                bail!("기호 범례 {} 가 겹치거나 예약어입니다", row.class);
            }
            for k in &row.kinds {
                if kinds_seen.contains(&k.as_str()) || !s.equipment.iter().any(|e| &e.kind == k) {
                    bail!("기호 범례 {} 의 설비 종류 {k} 가 겹치거나 원장에 없습니다", row.class);
                }
                kinds_seen.push(k);
            }
        }
        for il in &s.interlocks {
            if self.aop(&il.aop_id).is_none() || self.equipment(&il.target).is_none() {
                bail!("인터록 {} 의 절차서나 대상 설비가 없습니다", il.aop_id);
            }
            let mut last = 0;
            for st in &il.steps {
                if st.no <= last {
                    bail!("인터록 {} 의 단계 번호가 차례대로가 아닙니다", il.aop_id);
                }
                last = st.no;
                if let InterlockCheck::Below { tag, .. } | InterlockCheck::AtLeast { tag, .. } = &st.check {
                    if self.sensor(tag).is_none_or(|x| x.kks != st.kks) {
                        bail!("인터록 {} 단계 {} 의 센서 {tag} 가 설비 {} 의 것이 아닙니다", il.aop_id, st.no, st.kks);
                    }
                }
                // 단계가 적어 둔 깊이가 계통 그래프의 깊이와 같아야 합니다 (깊이 순서 강제의 근거).
                if crate::graph::depth_of(self, &il.target, &st.kks) != Some(st.req_depth) {
                    bail!("인터록 {} 단계 {} 의 설비 {} 는 대상에서 깊이 {} 가 아닙니다", il.aop_id, st.no, st.kks, st.req_depth);
                }
            }
        }
        Ok(())
    }

    /// 시드의 기준일
    pub fn as_of(&self) -> NaiveDate {
        self.seed.as_of
    }

    pub fn plant(&self) -> &PlantRow {
        &self.seed.plant
    }

    /// 관계망 노드 수 (설비)
    pub fn node_count(&self) -> usize {
        self.seed.equipment.len()
    }

    pub fn sectors(&self) -> &[SectorRow] {
        &self.seed.sectors
    }

    pub fn sector(&self, key: &str) -> Option<&SectorRow> {
        self.seed.sectors.iter().find(|s| s.key == key)
    }

    pub fn floors(&self) -> &[FloorRow] {
        &self.seed.floors
    }

    pub fn floor(&self, code: &str) -> Option<&FloorRow> {
        self.seed.floors.iter().find(|f| f.code == code)
    }

    pub fn drawings(&self) -> &[DrawingRow] {
        &self.seed.drawings
    }

    pub fn drawing(&self, id: &str) -> Option<&DrawingRow> {
        self.seed.drawings.iter().find(|d| d.id == id)
    }

    pub fn equipment_all(&self) -> &[EquipmentRow] {
        &self.seed.equipment
    }

    pub fn equipment(&self, kks: &str) -> Option<&EquipmentRow> {
        self.equipment.get(kks).map(|i| &self.seed.equipment[*i])
    }

    pub fn on_drawing(&self, drawing: &str) -> Vec<&EquipmentRow> {
        self.seed.equipment.iter().filter(|e| e.drawing == drawing).collect()
    }

    pub fn sensors(&self) -> &[SensorRow] {
        &self.seed.sensors
    }

    pub fn sensor(&self, tag: &str) -> Option<&SensorRow> {
        self.sensors.get(tag).map(|i| &self.seed.sensors[*i])
    }

    pub fn sensors_of(&self, kks: &str) -> Vec<&SensorRow> {
        self.seed.sensors.iter().filter(|s| s.kks == kks).collect()
    }

    pub fn edges(&self) -> &[EdgeRow] {
        &self.seed.edges
    }

    /// 설비에서 나가는 간선 (하류 쪽)
    pub fn out_edges(&self, kks: &str) -> Vec<&EdgeRow> {
        self.out_edges.get(kks).into_iter().flatten().map(|i| &self.seed.edges[*i]).collect()
    }

    /// 설비로 들어오는 간선 (상류 쪽)
    pub fn in_edges(&self, kks: &str) -> Vec<&EdgeRow> {
        self.in_edges.get(kks).into_iter().flatten().map(|i| &self.seed.edges[*i]).collect()
    }

    pub fn edge_between(&self, from: &str, to: &str) -> Option<&EdgeRow> {
        self.out_edges(from).into_iter().find(|e| e.to == to)
    }

    pub fn connectors(&self) -> &[ConnectorRow] {
        &self.seed.connectors
    }

    /// 두 설비를 잇는 도면 간 연결점 (방향 무관)
    pub fn connector_between(&self, a: &str, b: &str) -> Option<&ConnectorRow> {
        self.seed.connectors.iter().find(|c| (c.from == a && c.to == b) || (c.from == b && c.to == a))
    }

    /// 도면 위에 그려진 연결점 (나가는 것과 들어오는 것)
    pub fn connectors_on(&self, drawing: &str) -> Vec<&ConnectorRow> {
        self.seed
            .connectors
            .iter()
            .filter(|c| self.equipment(&c.from).is_some_and(|e| e.drawing == drawing) || self.equipment(&c.to).is_some_and(|e| e.drawing == drawing))
            .collect()
    }

    pub fn voting(&self, tag: &str) -> Option<&VotingRow> {
        self.seed.voting.iter().find(|v| v.tag == tag)
    }

    pub fn mocs(&self) -> &[MocRow] {
        &self.seed.moc
    }

    pub fn moc(&self, moc_no: &str) -> Option<&MocRow> {
        let want = moc_no.trim();
        self.seed.moc.iter().find(|m| m.moc_no == want)
    }

    /// MOC 번호 오독 교정: 하이픈 · 공백을 빼고 편집 거리 1 이내인 번호가 하나뿐이면 돌려줍니다.
    pub fn similar_moc(&self, raw: &str) -> Option<&MocRow> {
        let squeeze = |s: &str| s.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect::<String>();
        let want = squeeze(&raw.replace(['O', 'o'], "0").replace("M0C", "MOC"));
        let mut hits = self.seed.moc.iter().filter(|m| edit_distance(&want, &squeeze(&m.moc_no)) <= 1);
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    pub fn personnel(&self) -> &[PersonRow] {
        &self.seed.personnel
    }

    pub fn person(&self, id: &str) -> Option<&PersonRow> {
        self.seed.personnel.iter().find(|p| p.id == id.trim())
    }

    /// 이름으로 근무자를 찾습니다. 같은 이름이 둘이면 고르지 않습니다.
    pub fn person_named(&self, name: &str) -> Option<&PersonRow> {
        let mut hits = self.seed.personnel.iter().filter(|p| p.name == name.trim());
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    /// 한 글자만 다르게 읽힌 이름(같은 길이, 세 글자 이상)의 근무자. 그런 사람이 정확히 한 명일 때만 고릅니다 (손글씨 오독 교정).
    pub fn person_one_off(&self, name: &str) -> Option<&PersonRow> {
        let want: Vec<char> = name.trim().chars().collect();
        if want.len() < 3 {
            return None;
        }
        let mut hits = self.seed.personnel.iter().filter(|p| {
            let have: Vec<char> = p.name.chars().collect();
            have.len() == want.len() && have.iter().zip(&want).filter(|(a, b)| a != b).count() == 1
        });
        let first = hits.next()?;
        hits.next().is_none().then_some(first)
    }

    /// 근무자가 이 섹터를 관할하는가
    pub fn authorized(&self, person: &str, sector: &str) -> bool {
        self.person(person).is_some_and(|p| p.authorized_sectors.iter().any(|s| s == "*" || s == sector))
    }

    pub fn aops(&self) -> &[AopRow] {
        &self.seed.aop
    }

    pub fn aop(&self, id: &str) -> Option<&AopRow> {
        self.seed.aop.iter().find(|a| a.id == id.trim())
    }

    pub fn a0_drawings(&self) -> &[A0Row] {
        &self.seed.a0_drawings
    }

    pub fn a0(&self, id: &str) -> Option<&A0Row> {
        self.seed.a0_drawings.iter().find(|a| a.id == id)
    }

    pub fn interlocks(&self) -> &[InterlockRow] {
        &self.seed.interlocks
    }

    /// 도면 기호 범례
    pub fn symbol_legend(&self) -> &[SymbolLegendRow] {
        &self.seed.symbol_legend
    }

    /// 설비 종류를 그리는 심볼 (범례에 없으면 None: 미분탄기 · 버너 · 베어링)
    pub fn symbol_for_kind(&self, kind: &str) -> Option<&SymbolLegendRow> {
        self.seed.symbol_legend.iter().find(|r| r.kinds.iter().any(|k| k == kind))
    }

    /// 심볼 이름이 가리키는 설비 종류들
    pub fn kinds_of_symbol(&self, class: &str) -> &[String] {
        self.seed.symbol_legend.iter().find(|r| r.class == class).map_or(&[], |r| r.kinds.as_slice())
    }

    /// 절차서에 딸린 선행 절차 인터록
    pub fn interlock_for(&self, aop_id: &str) -> Option<&InterlockRow> {
        self.seed.interlocks.iter().find(|i| i.aop_id == aop_id.trim())
    }

    /// 경보 · 원인 태그로 표준 절차서를 고릅니다. 앞의 태그가 먼저입니다 (원인 태그를 앞에 둡니다).
    pub fn aop_for(&self, tags: &[&str]) -> Option<&AopRow> {
        tags.iter().find_map(|t| self.seed.aop.iter().find(|a| a.triggers.iter().any(|x| x == t)))
    }

    pub fn history(&self) -> &[HistoryRow] {
        &self.seed.history
    }

    /// 도면의 최신 개정 번호 (대장의 개정 기록 중 가장 늦은 것, 없으면 도면 목록의 값)
    pub fn latest_rev(&self, drawing: &str) -> Option<String> {
        let listed = self.drawing(drawing)?;
        let mut best = (listed.issued, listed.rev.clone());
        for h in self.seed.history.iter().filter(|h| h.kind == "revision" && h.drawing.as_deref() == Some(drawing)) {
            if let Some(rev) = &h.rev {
                if h.date > best.0 {
                    best = (h.date, rev.clone());
                }
            }
        }
        Some(best.1)
    }

    /// 설비의 Lv1 ~ Lv5 주소
    pub fn address(&self, kks: &str) -> Option<SpatialAddress> {
        let e = self.equipment(kks)?;
        let sector = self.sector(&e.sector)?;
        let floor = self.floor(&e.floor)?;
        let drawing = self.drawing(&e.drawing)?;
        let unit = self.seed.plant.units.iter().find(|u| kks.starts_with(&u.code))?;
        Some(SpatialAddress {
            unit_code: unit.code.clone(),
            unit_label: format!("{}({:.1}GW)", unit.label, unit.capacity_mw as f64 / 1000.0),
            sector: sector.key.clone(),
            building: sector.building.clone(),
            sector_label: sector.label.clone(),
            floor: floor.code.clone(),
            floor_label: floor.label.clone(),
            elevation_m: floor.elevation_m,
            drawing: drawing.id.clone(),
            drawing_rev: drawing.rev.clone(),
            kks: e.kks.clone(),
            name: e.name.clone(),
            bbox: e.bbox,
        })
    }

    fn match_code<'a>(raw: &str, codes: impl Iterator<Item = &'a str> + Clone) -> KksMatch {
        let trimmed: String = raw.chars().filter(|c| !c.is_whitespace()).collect();
        if codes.clone().any(|c| c == trimmed) {
            return KksMatch::Exact(trimmed);
        }
        let repaired = repair_kks(raw);
        let mut best: Vec<(&str, usize)> = codes.map(|c| (c, edit_distance(&repaired, c))).filter(|(_, d)| *d <= MAX_KKS_DISTANCE).collect();
        best.sort_by_key(|(_, d)| *d);
        match best.as_slice() {
            [] => KksMatch::Unknown,
            [(code, _)] => KksMatch::Corrected { raw: trimmed.clone(), code: code.to_string(), distance: edit_distance(&trimmed, code) },
            [(code, d0), (_, d1), ..] if d0 < d1 => KksMatch::Corrected { raw: trimmed.clone(), code: code.to_string(), distance: edit_distance(&trimmed, code) },
            many => KksMatch::Ambiguous(many.iter().filter(|(_, d)| *d == many[0].1).map(|(c, _)| c.to_string()).collect()),
        }
    }

    /// 상호작용 ①: 설비 KKS 오독 교정. 그대로 있으면 `Exact`, 고쳐서 하나로 정해지면 `Corrected` 입니다.
    pub fn match_kks(&self, raw: &str) -> KksMatch {
        Self::match_code(raw, self.seed.equipment.iter().map(|e| e.kks.as_str()))
    }

    /// 센서 태그 오독 교정
    pub fn match_tag(&self, raw: &str) -> KksMatch {
        Self::match_code(raw, self.seed.sensors.iter().map(|s| s.tag.as_str()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    #[test]
    fn the_seed_loads_and_refers_only_to_what_it_defines() {
        let db = db();
        assert_eq!(db.sectors().len(), 5);
        assert_eq!(db.drawings().len(), 5);
        assert!(db.node_count() >= 30);
        for e in db.equipment_all() {
            assert!(crate::plant::kks_shape(&e.kks), "{}", e.kks);
            let [x0, y0, x1, y1] = e.bbox;
            assert!(0.0 <= x0 && x0 < x1 && x1 <= 1.0 && 0.0 <= y0 && y0 < y1 && y1 <= 1.0, "{}", e.kks);
        }
        for s in db.sensors() {
            assert!(crate::plant::kks_shape(&s.tag), "{}", s.tag);
            assert_eq!(&s.tag[..7], &s.kks[..7], "센서 태그는 설비 코드와 계통이 같아야 합니다: {}", s.tag);
        }
    }

    #[test]
    fn every_edge_that_leaves_a_drawing_has_a_jump_connector() {
        let db = db();
        for e in db.edges() {
            let (a, b) = (db.equipment(&e.from).unwrap(), db.equipment(&e.to).unwrap());
            if a.drawing != b.drawing {
                assert!(db.connector_between(&e.from, &e.to).is_some(), "{} → {}", e.from, e.to);
            }
        }
        assert!(db.connector_between("10HAD10BB001", "10LAB20AA102").is_some(), "방향과 무관하게 찾습니다");
    }

    #[test]
    fn a_misread_kks_is_restored_when_only_one_code_is_close() {
        let db = db();
        // 기획안 상호작용 ①: `1` 을 `I` 로 읽은 코드
        assert_eq!(
            db.match_kks("10LABI0AP001"),
            KksMatch::Corrected { raw: "10LABI0AP001".into(), code: "10LAB10AP001".into(), distance: 1 }
        );
        assert_eq!(db.match_kks("10LAB10AP001"), KksMatch::Exact("10LAB10AP001".into()));
        assert_eq!(db.match_kks("10MAAl0ST0O1").code(), Some("10MAA10ST001"));
        assert_eq!(db.match_kks("99XYZ99QQ999"), KksMatch::Unknown);
        assert_eq!(db.match_tag("10LAB2OCG1O2").code(), Some("10LAB20CG102"));
    }

    #[test]
    fn two_equally_close_codes_are_not_guessed() {
        let db = db();
        // 10BBA10CB001 과 10BBA20CB001 사이: 가운데 한 글자가 지워져 둘 다 한 글자 차이입니다.
        match db.match_kks("10BBA0CB001") {
            KksMatch::Ambiguous(c) => assert!(c.len() >= 2, "{c:?}"),
            other => panic!("고르면 안 됩니다: {other:?}"),
        }
    }

    #[test]
    fn the_address_has_five_levels() {
        let db = db();
        let a = db.address("10LAB20AA102").unwrap();
        assert_eq!(a.building, "103동");
        assert_eq!(a.floor_text(), "2F (EL.+9.0m)");
        assert_eq!(a.breadcrumb()[0], "1호기(1.2GW) (10)");
        assert_eq!(a.breadcrumb()[3], "PID-103-LAB (Rev.3)");
        assert_eq!(a.breadcrumb()[4], "10LAB20AA102");
        assert_eq!(db.address("10BAT10GT001").unwrap().floor_text(), "옥외");
    }

    #[test]
    fn ledgers_answer_the_scenarios() {
        let db = db();
        let moc = db.moc("MOC-2026-1104").unwrap();
        assert!(moc.approved());
        assert_eq!((moc.old, moc.new), (29.0, 31.0));
        assert!(!db.moc("MOC-2026-1021").unwrap().approved(), "승인 전 MOC");
        assert_eq!(db.similar_moc("M0C-2026-11O4").map(|m| m.moc_no.as_str()), Some("MOC-2026-1104"));
        assert!(db.authorized("E102", "feedwater"));
        assert!(!db.authorized("E102", "turbine"));
        assert!(db.authorized("E301", "turbine"), "* 는 모든 섹터");
        assert_eq!(db.voting("10MAA10CP001").unwrap().channels.len(), 3);
        assert_eq!(db.aop_for(&["10LAB20CG102", "10HAH10CT001"]).unwrap().id, "AOP-103-FW02");
        assert!(db.aop("AOP-102-TB01").unwrap().high_risk);
        assert_eq!(db.latest_rev("PID-103-LAB").as_deref(), Some("Rev.3"));
        assert_eq!(db.person_named("김태현").map(|p| p.id.as_str()), Some("E102"));
    }

    #[test]
    fn sensors_know_their_alarm_side() {
        let db = db();
        let t = db.sensor("10HAH10CT001").unwrap();
        assert_eq!(t.alarm_side(630.0), Some(true));
        assert_eq!(t.alarm_side(610.0), None);
        assert!(!t.tripped(638.0));
        let p = db.sensor("10MAA10CP001").unwrap();
        assert_eq!(p.alarm_side(14.2), Some(false));
        assert!(p.tripped(14.2));
    }
}
