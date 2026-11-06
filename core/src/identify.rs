//! KKS 없는 설비 판독 (개선안: 명판 훼손 · 글자 없는 심볼). KKS 를 못 읽은 상자에 남은 단서로 원장의 설비를 고릅니다.
//!
//! | 단서 | 도면에서 읽는 것 | 원장에서 하는 일 |
//! | --- | --- | --- |
//! | 이름 | `주급수 유량조절밸브` | 설비 이름과 2글자 조각 겹침 (Dice) |
//! | 위치 | `위치: 103동 2F` | 동 번호 · 층이 같은 설비 (다르면 감점) |
//! | 계기 태그 | `CG102` | 그 짧은 태그의 센서를 가진 설비 |
//! | 항목 · 단위 | `밸브 개도 78.0 %`, `토출압력 28.5 MPa` | 그 항목의 센서를 가진 설비, 없으면 그 항목을 가진 설비 종류 (원장에서 뽑은 표) |
//! | 좌표 | 조각 · 심볼 상자 | 같은 도면의 원장 상자와 겹침. A0 는 [`a0_to_drawing`] 으로 도면 좌표로 바꾼 뒤 |
//! | 심볼 | SigLIP2 그림 사전의 종류 | 그 심볼로 그리는 설비 종류 (기호 범례). 범례와 어긋나면 감점 |
//!
//! 단서마다 점수를 더해 순위를 매깁니다. 1위 점수가 낮거나 1 · 2위 차이가 작으면 애매하다고 표시하고 사람이 고릅니다.
//! 점수표는 합성 원장에서 정한 손 규칙이라, 실제 도면에서는 다시 재야 합니다.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::extract::strip_tags;
use crate::lattice::{EquipmentRow, Lattice};
use crate::symbols::SymbolHit;

/// 1위가 이 점수보다 낮으면 단서가 모자랍니다.
pub const MIN_SCORE: f32 = 2.5;
/// 1 · 2위 차이가 이보다 작으면 애매합니다.
pub const MIN_GAP: f32 = 1.0;

const W_NAME: f32 = 4.0;
const W_FLOOR: f32 = 1.5;
const W_BUILDING: f32 = 0.5;
const W_TAG: f32 = 5.0;
const W_LABEL: f32 = 3.0;
const W_KIND: f32 = 1.5;
const W_COORD: f32 = 4.0;
const W_SYMBOL: f32 = 2.5;

/// 상자 하나에서 모은 단서. 쓰지 않을 단서는 비워 둡니다 (단서별 정답률을 따로 잴 때).
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Clues {
    /// 표제란에서 읽은 도면 번호. 있으면 그 도면의 설비만 후보입니다.
    pub drawing: Option<String>,
    /// 상자 안에서 읽은 글자 (줄 단위, 순서 무관)
    pub lines: Vec<String>,
    /// 상자 안 조각 · 심볼을 모두 덮는 정규화 좌표
    pub bbox: Option<[f32; 4]>,
    /// 그림 사전이 받아들인 심볼 종류와 유사도
    pub symbol: Option<(String, f32)>,
}

/// 줄 글자에서 뽑은 구조
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TextClues {
    pub names: Vec<String>,
    pub building: Option<String>,
    pub floor: Option<String>,
    pub tags: Vec<String>,
    /// (항목 이름(공백 없음), 단위)
    pub items: Vec<(String, Option<String>)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum ClueKind {
    Name,
    Location,
    Tag,
    Item,
    Coordinate,
    Symbol,
}

impl ClueKind {
    pub fn label(self) -> &'static str {
        match self {
            ClueKind::Name => "이름",
            ClueKind::Location => "위치",
            ClueKind::Tag => "계기 태그",
            ClueKind::Item => "항목 · 단위",
            ClueKind::Coordinate => "좌표",
            ClueKind::Symbol => "심볼",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Evidence {
    pub kind: ClueKind,
    pub score: f32,
    pub detail: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub kks: String,
    pub name: String,
    pub score: f32,
    pub evidence: Vec<Evidence>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Identification {
    /// 1위 (애매해도 채웁니다)
    pub best: Option<String>,
    pub ambiguous: bool,
    pub reason: String,
    /// 점수 순 상위 5개
    pub candidates: Vec<Candidate>,
}

fn squash(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace() && !matches!(c, ':' | '·' | ',' | '(' | ')' | '[' | ']')).collect()
}

fn bigrams(s: &str) -> Vec<(char, char)> {
    let cs: Vec<char> = s.chars().collect();
    cs.windows(2).map(|w| (w[0], w[1])).collect()
}

/// 2글자 조각 겹침 (0 ~ 1)
pub fn dice(a: &str, b: &str) -> f32 {
    let (x, y) = (bigrams(&squash(a)), bigrams(&squash(b)));
    if x.is_empty() || y.is_empty() {
        return 0.0;
    }
    let mut pool = y.clone();
    let mut hit = 0;
    for g in &x {
        if let Some(i) = pool.iter().position(|h| h == g) {
            pool.swap_remove(i);
            hit += 1;
        }
    }
    2.0 * hit as f32 / (x.len() + y.len()) as f32
}

fn hangul(c: char) -> bool {
    ('가'..='힣').contains(&c)
}

/// `CG1O2` → `CG102`. 앞 두 글자는 영문, 뒤 글자는 숫자로 오독을 고칩니다 (KKS 는 숫자 셋, DCIM 은 숫자 둘: `CG02`).
fn short_tag(token: &str) -> Option<String> {
    let t: Vec<char> = token.trim_matches(|c: char| !c.is_ascii_alphanumeric()).to_uppercase().chars().collect();
    if t.len() != 5 && t.len() != 4 {
        return None;
    }
    let letter = |c: char| match c {
        '0' => Some('O'),
        '1' => Some('I'),
        c if c.is_ascii_uppercase() => Some(c),
        _ => None,
    };
    let digit = |c: char| match c {
        'O' | 'D' => Some('0'),
        'I' | 'L' => Some('1'),
        c if c.is_ascii_digit() => Some(c),
        _ => None,
    };
    let mut out = String::new();
    for (i, c) in t.into_iter().enumerate() {
        out.push(if i < 2 { letter(c)? } else { digit(c)? });
    }
    // 센서 태그의 측정 종류 자리는 C 입니다 (예: CP · CT · CG).
    out.starts_with('C').then_some(out)
}

fn unit_of(token: &str) -> Option<String> {
    let t = token.trim_matches(|c: char| c == ':' || c == ',');
    let u = match t {
        "%" => "%",
        "MPa" | "Mpa" | "MPA" => "MPa",
        "°C" | "℃" | "C" | "'C" | "ºC" => "°C",
        "t/h" | "T/H" | "t/hr" => "t/h",
        "μm" | "um" | "µm" => "μm",
        "mm/s" => "mm/s",
        "A" => "A",
        "kV" | "KV" => "kV",
        "MW" => "MW",
        "kPa" | "KPa" => "kPa",
        "bar" | "Bar" | "BAR" => "bar",
        "LPM" | "lpm" | "L/min" => "LPM",
        "kW" | "KW" => "kW",
        "ppm" => "ppm",
        "mg/Nm3" | "mg/Nm³" => "mg/Nm³",
        "rpm" => "rpm",
        "mm" => "mm",
        _ => return None,
    };
    Some(u.to_string())
}

fn is_number(token: &str) -> bool {
    let t = token.trim_matches(|c: char| c == ':' || c == '+');
    !t.is_empty() && t.chars().any(|c| c.is_ascii_digit()) && t.chars().all(|c| c.is_ascii_digit() || matches!(c, '.' | ',' | '-'))
}

/// 줄 글자를 이름 · 위치 · 계기 줄로 나눕니다. KKS 줄은 건너뜁니다 (KKS 를 읽었으면 이 판독이 필요 없습니다).
pub fn parse_lines(lines: &[String]) -> TextClues {
    let mut out = TextClues::default();
    for raw in lines.iter().flat_map(|l| l.lines()) {
        let line = raw.trim();
        if line.is_empty() || line.to_uppercase().starts_with("KKS") || line.starts_with("자산") {
            continue;
        }
        if line.contains("위치") || line.contains("EL.") {
            for tok in line.split_whitespace() {
                let t = tok.trim_matches(|c: char| c == ':' || c == ',');
                if let Some(num) = t.strip_suffix('동').filter(|n| n.len() == 3 && n.chars().all(|c| c.is_ascii_digit())) {
                    out.building = Some(num.to_string());
                }
                let up = t.to_uppercase();
                if matches!(up.as_str(), "B1F" | "B2F" | "1F" | "2F" | "3F" | "4F" | "5F" | "6F") {
                    out.floor = Some(up);
                } else if t == "옥외" {
                    out.floor = Some("OUT".into());
                }
            }
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();
        let tag = tokens.iter().find_map(|t| short_tag(t));
        let unit = tokens.iter().find_map(|t| unit_of(t));
        let has_number = tokens.iter().any(|t| is_number(t));
        if tag.is_some() || (unit.is_some() && has_number) {
            if let Some(t) = &tag {
                out.tags.push(t.clone());
            }
            let words: String = tokens
                .iter()
                .filter(|t| short_tag(t).is_none() && unit_of(t).is_none() && !is_number(t))
                .map(|t| squash(t))
                .collect();
            if words.chars().filter(|c| hangul(*c)).count() >= 2 {
                out.items.push((words, unit));
            }
            continue;
        }
        // 한글 두 글자 이상이거나 영문 약어(`480V MCC`)인 줄은 이름입니다.
        if line.chars().filter(|c| hangul(*c)).count() >= 2 || line.chars().filter(|c| c.is_ascii_uppercase()).count() >= 3 {
            out.names.push(line.to_string());
        }
    }
    out
}

/// 상자 하나의 단서: 그 상자 영역을 다시 읽은 글자(`DocumentReader::read_regions`)와, 그림 사전이 받아들인 심볼 가운데
/// 가운데가 상자 안에 든 것 (유사도가 가장 높은 하나). 좌표 단서는 그 심볼의 위치입니다 (상자 자체가 아님).
/// 도면 판독 조각은 한 행의 상자들을 가로 띠 하나로 읽어서 상자별 글자를 떼어 낼 수 없으므로, 글자는 상자 영역만 다시 읽어 받습니다.
pub fn clues_for_box(text: &str, symbols: &[SymbolHit], page: (u32, u32), drawing: Option<String>, region: [f32; 4]) -> Clues {
    let (w, h) = (page.0.max(1) as f32, page.1.max(1) as f32);
    let lines = strip_tags(text).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_string).collect();
    let inside = |b: [u32; 4]| {
        let (cx, cy) = ((b[0] + b[2]) as f32 / 2.0 / w, (b[1] + b[3]) as f32 / 2.0 / h);
        region[0] <= cx && cx <= region[2] && region[1] <= cy && cy <= region[3]
    };
    let found = symbols.iter().filter(|x| x.matched.class.is_some() && inside(x.bbox)).max_by(|a, b| a.matched.sim.total_cmp(&b.matched.sim));
    Clues {
        drawing,
        lines,
        bbox: found.map(|x| [x.bbox[0] as f32 / w, x.bbox[1] as f32 / h, x.bbox[2] as f32 / w, x.bbox[3] as f32 / h]),
        symbol: found.map(|x| (x.matched.class.clone().unwrap_or_default(), x.matched.sim)),
    }
}

/// A0 종합 도면의 정규화 좌표를 그 칸의 마스터 도면 좌표로 바꿉니다 (상자 가운데가 든 칸).
pub fn a0_to_drawing(db: &Lattice, a0_id: &str, global: [f32; 4]) -> Option<(String, [f32; 4])> {
    let a0 = db.a0(a0_id)?;
    let (w, h) = (a0.pixel_size[0] as f32, a0.pixel_size[1] as f32);
    let (sw, sh) = (a0.sheet_px[0] as f32, a0.sheet_px[1] as f32);
    let (cx, cy) = ((global[0] + global[2]) / 2.0 * w, (global[1] + global[3]) / 2.0 * h);
    let sheet = a0.sheets.iter().find(|s| (s.x as f32..s.x as f32 + sw).contains(&cx) && (s.y as f32..s.y as f32 + sh).contains(&cy))?;
    let local = |v: f32, size: f32, origin: u32, span: f32| ((v * size - origin as f32) / span).clamp(0.0, 1.0);
    Some((
        sheet.drawing.clone(),
        [local(global[0], w, sheet.x, sw), local(global[1], h, sheet.y, sh), local(global[2], w, sheet.x, sw), local(global[3], h, sheet.y, sh)],
    ))
}

fn overlap(region: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = (region[2].min(b[2]) - region[0].max(b[0])).max(0.0);
    let iy = (region[3].min(b[3]) - region[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: [f32; 4]| ((r[2] - r[0]) * (r[3] - r[1])).max(1e-9);
    // 조각은 상자보다 작습니다. 조각이 상자 안에 들면 1 입니다 (IoU 와 포함 비율 중 큰 값).
    (inter / (area(region) + area(b) - inter)).max(inter / area(region))
}

/// 원장 센서 항목 이름(공백 없음) → 그 항목을 가진 설비 종류
fn kinds_by_item(db: &Lattice) -> BTreeMap<String, BTreeSet<String>> {
    let mut m: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for s in db.sensors() {
        if let Some(e) = db.equipment(&s.kks) {
            m.entry(squash(&s.label)).or_default().insert(e.kind.clone());
        }
    }
    m
}

fn item_matches(text: &str, ledger: &str) -> bool {
    let (t, l) = (text.chars().count(), ledger.chars().count());
    t >= 2 && l >= 2 && (text == ledger || ledger.contains(text) || (text.contains(ledger) && l * 2 >= t))
}

fn score(db: &Lattice, e: &EquipmentRow, c: &Clues, t: &TextClues, by_item: &BTreeMap<String, BTreeSet<String>>) -> Candidate {
    let mut ev: Vec<Evidence> = Vec::new();
    let mut add = |kind, score: f32, detail: String| ev.push(Evidence { kind, score, detail });
    // 이름
    if let Some((name, d)) = t.names.iter().map(|n| (n, dice(n, &e.name))).max_by(|a, b| a.1.total_cmp(&b.1)) {
        if d >= 0.5 {
            add(ClueKind::Name, W_NAME * d, format!("'{name}' ≈ '{}' ({d:.2})", e.name));
        }
    }
    // 위치
    if let Some(sector) = db.sector(&e.sector) {
        if let Some(b) = &t.building {
            if sector.building.starts_with(b.as_str()) {
                add(ClueKind::Location, W_BUILDING, format!("{b}동"));
            }
        }
    }
    if let Some(f) = &t.floor {
        if f == &e.floor {
            add(ClueKind::Location, W_FLOOR, format!("층 {f}"));
        } else {
            add(ClueKind::Location, -W_FLOOR, format!("층 {f} ≠ {}", e.floor));
        }
    }
    // 계기 태그 · 항목
    let sensors = db.sensors_of(&e.kks);
    for tag in &t.tags {
        if let Some(s) = sensors.iter().find(|s| s.tag.ends_with(tag.as_str())) {
            add(ClueKind::Tag, W_TAG, format!("{tag} → {}", s.tag));
        }
    }
    for (item, unit) in &t.items {
        let own = sensors.iter().find(|s| item_matches(item, &squash(&s.label)) && unit.as_ref().is_none_or(|u| Some(u) == unit_of(&s.unit).as_ref()));
        if let Some(s) = own {
            add(ClueKind::Item, W_LABEL, format!("'{item}' {} → {} {}", unit.as_deref().unwrap_or(""), s.tag, s.label));
            continue;
        }
        let kinds: BTreeSet<&String> = by_item.iter().filter(|(l, _)| item_matches(item, l)).flat_map(|(_, k)| k).collect();
        if kinds.contains(&e.kind) {
            add(ClueKind::Item, W_KIND / kinds.len() as f32, format!("'{item}' → 종류 {}", kinds.iter().map(|k| k.as_str()).collect::<Vec<_>>().join("·")));
        }
    }
    // 좌표
    if let Some(r) = c.bbox {
        let o = overlap(r, e.bbox);
        if o >= 0.3 {
            add(ClueKind::Coordinate, W_COORD * o, format!("원장 상자와 겹침 {o:.2}"));
        }
    }
    // 심볼
    if let Some((class, sim)) = &c.symbol {
        let kinds = db.kinds_of_symbol(class);
        match db.symbol_for_kind(&e.kind) {
            _ if kinds.contains(&e.kind) => add(ClueKind::Symbol, W_SYMBOL, format!("심볼 {class} ({sim:.2}) → {}", e.kind)),
            Some(own) => add(ClueKind::Symbol, -W_SYMBOL, format!("심볼 {class} ≠ 범례 {}", own.class)),
            None => add(ClueKind::Symbol, -1.0, format!("심볼 {class} · {} 는 범례에 심볼이 없음", e.kind)),
        }
    }
    let score = ev.iter().map(|x| x.score).sum();
    Candidate { kks: e.kks.clone(), name: e.name.clone(), score, evidence: ev }
}

/// 모든 단서
pub const ALL_CLUES: [ClueKind; 6] = [ClueKind::Name, ClueKind::Location, ClueKind::Tag, ClueKind::Item, ClueKind::Coordinate, ClueKind::Symbol];

/// 단서로 원장의 설비 순위를 매깁니다.
pub fn identify(db: &Lattice, c: &Clues) -> Identification {
    identify_only(db, c, &ALL_CLUES)
}

/// 주어진 종류의 단서만 써서 순위를 매깁니다 (단서별 정답률을 잴 때).
pub fn identify_only(db: &Lattice, c: &Clues, kinds: &[ClueKind]) -> Identification {
    let t = parse_lines(&c.lines);
    let by_item = kinds_by_item(db);
    let pool: Vec<&EquipmentRow> = match c.drawing.as_deref().filter(|d| db.drawing(d).is_some()) {
        Some(d) => db.on_drawing(d),
        None => db.equipment_all().iter().collect(),
    };
    let mut ranked: Vec<Candidate> = pool
        .into_iter()
        .map(|e| {
            let mut x = score(db, e, c, &t, &by_item);
            x.evidence.retain(|ev| kinds.contains(&ev.kind));
            x.score = x.evidence.iter().map(|ev| ev.score).sum();
            x
        })
        .filter(|x| !x.evidence.is_empty())
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.kks.cmp(&b.kks)));
    ranked.truncate(5);
    let (best, ambiguous, reason) = match ranked.as_slice() {
        [] => (None, true, "단서가 없습니다".to_string()),
        [a, rest @ ..] => {
            let gap = rest.first().map_or(a.score, |b| a.score - b.score);
            let kinds: BTreeSet<&str> = a.evidence.iter().filter(|e| e.score > 0.0).map(|e| e.kind.label()).collect();
            let why = kinds.into_iter().collect::<Vec<_>>().join(" + ");
            if a.score < MIN_SCORE {
                (Some(a.kks.clone()), true, format!("단서가 약합니다 (점수 {:.1} < {MIN_SCORE}, {why})", a.score))
            } else if gap < MIN_GAP {
                (Some(a.kks.clone()), true, format!("{} 와 {} 가 비슷합니다 (차이 {gap:.1})", a.kks, rest[0].kks))
            } else {
                (Some(a.kks.clone()), false, format!("{why} (점수 {:.1}, 2위와 {gap:.1} 차이)", a.score))
            }
        }
    };
    Identification { best, ambiguous, reason, candidates: ranked }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    fn lines(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn box_text_is_split_into_name_location_tags_and_items() {
        let t = parse_lines(&lines(&["KKS: ▒▒▒", "주급수 유량조절밸브", "위치: 103동 2F (EL.+9.0m)", "CG1O2 밸브 개도 78.0 %", "CF001 급수 유량 1,850.0 t/h"]));
        assert_eq!(t.names, vec!["주급수 유량조절밸브"]);
        assert_eq!(t.building.as_deref(), Some("103"));
        assert_eq!(t.floor.as_deref(), Some("2F"));
        assert_eq!(t.tags, vec!["CG102", "CF001"], "O 를 0 으로 고칩니다");
        assert_eq!(t.items, vec![("밸브개도".to_string(), Some("%".to_string())), ("급수유량".to_string(), Some("t/h".to_string()))]);
    }

    #[test]
    fn the_name_and_location_find_the_valve_without_kks() {
        // 개선안 표 ①: KKS 가 지워져도 "103동 2층 + 조절밸브" 로 원장을 역조회
        let r = identify(&db(), &Clues { lines: lines(&["주급수 유량조절밸브", "위치: 103동 2F (EL.+9.0m)"]), ..Clues::default() });
        assert_eq!(r.best.as_deref(), Some("10LAB20AA102"), "{r:?}");
        assert!(!r.ambiguous, "{}", r.reason);
        assert!(r.candidates[0].evidence.iter().any(|e| e.kind == ClueKind::Name));
    }

    #[test]
    fn items_and_units_point_to_the_kind_when_the_name_is_gone() {
        // 개선안 표 ②: "개도 %" 는 밸브, "토출압력" 은 펌프, "출구증기온도" 는 과열기
        let db = db();
        let kind_of = |xs: &[&str], drawing: Option<&str>| {
            let r = identify(&db, &Clues { drawing: drawing.map(str::to_string), lines: lines(xs), ..Clues::default() });
            let best = r.best.clone().unwrap();
            (db.equipment(&best).unwrap().kind.clone(), r)
        };
        let (k, r) = kind_of(&["개도: 18.0 %"], Some("PID-103-LAB"));
        assert_eq!(k, "valve", "{r:?}");
        let (k, r) = kind_of(&["토출압력: 28.5 MPa"], Some("PID-103-LAB"));
        assert_eq!(k, "pump", "{r:?}");
        let (k, r) = kind_of(&["출구증기온도: 638 C"], None);
        assert_eq!((k.as_str(), r.best.as_deref()), ("superheater", Some("10HAH10BB001")), "{r:?}");
        // 같은 도면의 밸브가 둘이면 개도 하나로는 고르지 못합니다.
        let (_, r) = kind_of(&["개도: 18.0 %"], Some("PID-103-LAB"));
        assert!(r.ambiguous, "{r:?}");
    }

    #[test]
    fn coordinates_alone_find_the_box_and_a0_tiles_map_back_to_the_sheet() {
        let db = db();
        let e = db.equipment("10LAB20AA102").unwrap();
        let [x0, y0, _, _] = e.bbox;
        let inner = [x0 + 0.01, y0 + 0.01, x0 + 0.08, y0 + 0.05];
        let r = identify(&db, &Clues { drawing: Some("PID-103-LAB".into()), bbox: Some(inner), ..Clues::default() });
        assert_eq!(r.best.as_deref(), Some("10LAB20AA102"));
        assert!(!r.ambiguous, "{}", r.reason);
        // 같은 상자를 A0 좌표로 주면 칸을 찾아 도면 좌표로 바꿉니다.
        let a0 = &db.a0_drawings()[0];
        let sheet = a0.sheets.iter().find(|s| s.drawing == "PID-103-LAB").unwrap();
        let g = |v: f32, origin: u32, span: u32, size: u32| (origin as f32 + v * span as f32) / size as f32;
        let global = [g(inner[0], sheet.x, a0.sheet_px[0], a0.pixel_size[0]), g(inner[1], sheet.y, a0.sheet_px[1], a0.pixel_size[1]), g(inner[2], sheet.x, a0.sheet_px[0], a0.pixel_size[0]), g(inner[3], sheet.y, a0.sheet_px[1], a0.pixel_size[1])];
        let (drawing, local) = a0_to_drawing(&db, &a0.id, global).unwrap();
        assert_eq!(drawing, "PID-103-LAB");
        assert!(local.iter().zip(inner).all(|(a, b)| (a - b).abs() < 1e-4), "{local:?}");
    }

    #[test]
    fn box_clues_take_the_reread_text_and_the_best_accepted_symbol_inside_the_box() {
        use crate::symbols::SymbolMatch;
        let hit = |bbox: [u32; 4], class: Option<&str>, sim: f32| SymbolHit { bbox, matched: SymbolMatch { class: class.map(str::to_string), nearest: class.unwrap_or("none").to_string(), sim, margin: 0.1 } };
        let db = db();
        let e = db.equipment("10LAB20AA102").unwrap();
        let (w, h) = (2000u32, 1400u32);
        let px = |v: f32, s: u32| (v * s as f32) as u32;
        let [x0, y0, x1, y1] = e.bbox;
        // 상자 안: 받아들인 밸브 · 버린 후보, 상자 밖: 받아들인 펌프
        let symbols = [
            hit([px(x1, w) - 70, px(y0, h) + 12, px(x1, w) - 12, px(y0, h) + 70], Some("valve"), 0.97),
            hit([px(x0, w) + 20, px(y0, h) + 20, px(x0, w) + 80, px(y0, h) + 80], None, 0.6),
            hit([10, 10, 60, 60], Some("pump"), 0.99),
        ];
        let text = "<table><tr><td>주급수 유량조절밸브</td></tr></table>\n위치: 103동 2F (EL.+9.0m)\n";
        let c = clues_for_box(text, &symbols, (w, h), Some("PID-103-LAB".into()), e.bbox);
        assert_eq!(c.lines, vec!["주급수 유량조절밸브", "위치: 103동 2F (EL.+9.0m)"], "표 태그를 빼고 줄로");
        assert_eq!(c.symbol, Some(("valve".to_string(), 0.97)));
        let b = c.bbox.unwrap();
        assert!(b[0] > x0 && b[2] <= x1 && b[1] >= y0 && b[3] < y1, "좌표 단서는 심볼 위치: {b:?}");
        let r = identify(&db, &c);
        assert_eq!(r.best.as_deref(), Some("10LAB20AA102"));
        assert!(!r.ambiguous, "{}", r.reason);
    }

    #[test]
    fn a_symbol_narrows_the_kind_and_needs_a_second_clue_to_pick_one() {
        let db = db();
        // 103동 도면의 펌프는 셋: 심볼만으로는 애매합니다.
        let pump = Some(("pump".to_string(), 0.9));
        let r = identify(&db, &Clues { drawing: Some("PID-103-LAB".into()), symbol: pump.clone(), ..Clues::default() });
        assert!(r.ambiguous, "{r:?}");
        let pumps: BTreeSet<&str> = r.candidates.iter().filter(|c| c.score > 0.0).map(|c| c.kks.as_str()).collect();
        assert_eq!(pumps, BTreeSet::from(["10LAB10AP001", "10LCA10AP001", "10PAB10AP001"]));
        // 층(1F)을 더하면 주급수펌프 A 하나로 좁혀집니다.
        let r = identify(&db, &Clues { drawing: Some("PID-103-LAB".into()), symbol: pump, lines: lines(&["위치: 103동 1F (EL.+0.0m)"]), ..Clues::default() });
        assert_eq!(r.best.as_deref(), Some("10LAB10AP001"));
        assert!(!r.ambiguous, "{}", r.reason);
        // 심볼이 범례와 어긋나면 감점합니다 (좌표는 밸브 자리인데 그림은 펌프).
        let e = db.equipment("10LAB20AA102").unwrap();
        let r = identify(&db, &Clues { drawing: Some("PID-103-LAB".into()), bbox: Some(e.bbox), symbol: Some(("pump".into(), 0.8)), ..Clues::default() });
        let valve = r.candidates.iter().find(|c| c.kks == "10LAB20AA102").unwrap();
        assert!(valve.evidence.iter().any(|x| x.kind == ClueKind::Symbol && x.score < 0.0), "{valve:?}");
        assert!(r.ambiguous, "단서가 서로 어긋나면 사람이 고릅니다: {r:?}");
    }
}
