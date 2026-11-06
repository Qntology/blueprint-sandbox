//! 현장 스캔(계기 점검표 · Red-line 도면 · 마스터 P&ID)의 OCR 글자에서 항목을 뽑습니다 (기획 Part 8 Stage 2).
//!
//! OCR(gocr)은 글자만 읽습니다. 어느 칸이 어떤 항목인지는 서식의 항목 이름과 글자 모양(KKS 12글자, `MOC-연도-번호`, `29.0 → 31.0`)으로 정합니다.
//! 값을 고치는 경우는 하나뿐입니다. 원장에 가까운 값이 하나만 있으면 그 값으로 바꾸고 교정 기록을 남깁니다 (상호작용 ①).
//! 글자 신뢰도가 낮거나 형식이 맞지 않거나 읽지 못한 값은 `uncertain` 에 넣어 사람이 확인하게 합니다.

use chrono::{DateTime, NaiveDateTime, TimeZone, Utc};
use serde::Serialize;

use crate::ai::document::PageReading;
use crate::ai::siglip2::plan::CropKind;
use crate::case::Correction;
use crate::lattice::{edit_distance, KksMatch, Lattice};
use crate::plant::Sector;

/// 값의 글자 신뢰도가 이보다 낮으면 사람이 확인합니다 (zoro-gravity 와 같은 기준)
pub const LOW_CONF: f32 = 0.9;

/// OCR 조각 하나
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageCrop {
    pub title: bool,
    pub bbox: [u32; 4],
    pub text: String,
    /// 글자별 신뢰도. 없으면 글자를 모두 믿지 않습니다.
    pub confs: Vec<f32>,
}

/// 페이지 한 장의 읽기 결과
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PageText {
    pub hash: String,
    pub width: u32,
    pub height: u32,
    pub crops: Vec<PageCrop>,
    /// 서명 · 직인으로 잡힌 상자
    pub signs: Vec<[u32; 4]>,
}

impl PageText {
    pub fn from_reading(hash: &str, size: (u32, u32), r: &PageReading) -> Self {
        let crops = r
            .crops
            .iter()
            .map(|c| PageCrop { title: c.kind == CropKind::Title, bbox: [c.bbox.0, c.bbox.1, c.bbox.2, c.bbox.3], text: c.output.text.clone(), confs: c.output.char_confidence.clone() })
            .collect();
        let signs = r.plan.crops.iter().filter(|c| c.kind == CropKind::Sign).map(|c| [c.bbox.0, c.bbox.1, c.bbox.2, c.bbox.3]).collect();
        Self { hash: hash.to_string(), width: size.0, height: size.1, crops, signs }
    }

    /// 전체 글 (조각 순서대로)
    pub fn text(&self) -> String {
        self.crops.iter().map(|c| strip_tags(&c.text)).collect::<Vec<_>>().join("\n")
    }
}

/// 표 태그(`<td>` 등)를 빼고 칸 사이를 띄웁니다.
pub fn strip_tags(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let mut tag = String::new();
            for d in chars.by_ref() {
                if d == '>' {
                    break;
                }
                tag.push(d);
            }
            let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_string();
            match name.as_str() {
                "tr" | "table" | "tbody" | "thead" => out.push('\n'),
                "td" | "th" => out.push(' '),
                _ => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// 표의 칸 하나. `conf` 는 칸에서 가장 낮은 글자 신뢰도입니다.
#[derive(Debug, Clone)]
struct Cell {
    text: String,
    conf: f32,
}

fn push_cell(cells: &mut Vec<Cell>, chars: &mut Vec<(char, f32)>) {
    let text: String = chars.iter().map(|(c, _)| *c).collect();
    let conf = chars.iter().filter(|(c, _)| c.is_alphanumeric()).map(|(_, f)| *f).fold(1.0f32, f32::min);
    chars.clear();
    let text = text.trim().to_string();
    if text.is_empty() {
        return;
    }
    // `점검자: 이승민` 같은 칸을 항목과 값으로 나눕니다. 시각(14:21:05)은 나누지 않습니다.
    if let Some((label, value)) = text.split_once([':', '：']) {
        if !label.trim().is_empty() && label.chars().count() <= 10 && !label.chars().any(|c| c.is_ascii_digit()) {
            cells.push(Cell { text: label.trim().to_string(), conf });
            if !value.trim().is_empty() {
                cells.push(Cell { text: value.trim().to_string(), conf });
            }
            return;
        }
    }
    cells.push(Cell { text, conf });
}

/// 조각의 글자를 줄과 칸으로 나눕니다. 표 태그가 없으면 한 줄을 한 칸으로 봅니다.
fn rows_of(crop: &PageCrop) -> Vec<Vec<Cell>> {
    let chars: Vec<char> = crop.text.chars().collect();
    let conf = |i: usize| crop.confs.get(i).copied().unwrap_or(0.0);
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut cells = Vec::new();
    let mut cell: Vec<(char, f32)> = Vec::new();
    let mut in_table = false;
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            if let Some(end) = chars[i..].iter().position(|c| *c == '>') {
                let tag: String = chars[i + 1..i + end].iter().collect();
                let name = tag.trim_start_matches('/').split_whitespace().next().unwrap_or("").to_string();
                if matches!(name.as_str(), "table" | "tr" | "td" | "th" | "tbody" | "thead") {
                    in_table = true;
                    if name == "td" || name == "th" {
                        if tag.starts_with('/') {
                            push_cell(&mut cells, &mut cell);
                        }
                    } else {
                        push_cell(&mut cells, &mut cell);
                        if !cells.is_empty() {
                            rows.push(std::mem::take(&mut cells));
                        }
                    }
                    i += end + 1;
                    continue;
                }
            }
        }
        if chars[i] == '\n' && !in_table {
            push_cell(&mut cells, &mut cell);
            if !cells.is_empty() {
                rows.push(std::mem::take(&mut cells));
            }
        } else {
            cell.push((chars[i], conf(i)));
        }
        i += 1;
    }
    push_cell(&mut cells, &mut cell);
    if !cells.is_empty() {
        rows.push(cells);
    }
    rows
}

/// 글자 · 숫자만 남깁니다
fn squeeze(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect()
}

/// 항목 이름이 맞는지 봅니다. 짧은 이름은 그대로 맞아야 합니다.
fn near(cell: &str, label: &str) -> bool {
    let (c, l) = (squeeze(cell), squeeze(label));
    if c == l {
        return true;
    }
    let n = l.chars().count();
    n >= 3 && edit_distance(&c, &l) <= if n >= 5 { 2 } else { 1 }
}

/// 문서 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum DocKind {
    GaugeCard,
    Redline,
    Drawing,
    Unknown,
}

impl DocKind {
    pub fn label(self) -> &'static str {
        match self {
            DocKind::GaugeCard => "현장계기점검표",
            DocKind::Redline => "설비변경 Red-line",
            DocKind::Drawing => "P&ID 도면",
            DocKind::Unknown => "알 수 없는 문서",
        }
    }
}

/// 도면에서 찾은 KKS 하나 (사전 인덱싱 · Red-line 대조용)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct KksHit {
    pub raw: String,
    pub code: String,
    pub corrected: bool,
    /// 조각 상자의 정규화 좌표
    pub bbox: [f32; 4],
    pub conf: f32,
}

/// 한 장에서 뽑은 값
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct Extracted {
    pub kind: Option<DocKind>,
    pub drawing: Option<String>,
    pub drawing_rev: Option<String>,
    pub sector: Option<Sector>,
    pub kks: Option<String>,
    pub tag: Option<String>,
    pub reading: Option<f64>,
    pub unit: Option<String>,
    pub taken_at: Option<DateTime<Utc>>,
    pub moc_no: Option<String>,
    pub old_value: Option<f64>,
    pub new_value: Option<f64>,
    pub worker_name: Option<String>,
    pub worker_id: Option<String>,
    pub signed: bool,
    pub corrections: Vec<Correction>,
    pub uncertain: Vec<String>,
    pub kks_hits: Vec<KksHit>,
}

/// 숫자 하나 (`26.0`, `1,850.0`, `-0.1`)
fn number_in(s: &str) -> Option<f64> {
    let mut out = String::new();
    let mut started = false;
    for c in s.chars() {
        if c.is_ascii_digit() || (c == '.' && started) || (c == '-' && !started) {
            out.push(c);
            started |= c.is_ascii_digit();
        } else if c == ',' && started {
            continue;
        } else if started {
            break;
        } else if c != ' ' {
            out.clear();
        }
    }
    out.trim_end_matches('.').parse().ok()
}

/// ASCII 영숫자 덩어리 (한 칸 띄어 쓴 것은 잇습니다). KKS 후보를 찾을 때 씁니다.
fn ascii_runs(text: &str) -> Vec<String> {
    let mut runs = Vec::new();
    let mut cur = String::new();
    let chars: Vec<char> = text.chars().collect();
    for (i, &c) in chars.iter().enumerate() {
        if c.is_ascii_alphanumeric() || c == '|' {
            cur.push(c);
        } else if c == ' ' && !cur.is_empty() && chars.get(i + 1).is_some_and(|n| n.is_ascii_alphanumeric()) && cur.len() < 12 {
            continue;
        } else if !cur.is_empty() {
            runs.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() {
        runs.push(cur);
    }
    runs
}

/// 글에서 KKS (또는 센서 태그) 후보를 원장과 맞춥니다. 같은 코드는 한 번만 돌려줍니다.
fn find_codes(db: &Lattice, text: &str, tags: bool) -> Vec<KksMatch> {
    let mut out: Vec<KksMatch> = Vec::new();
    for run in ascii_runs(text) {
        let n = run.chars().count();
        let candidates: Vec<String> = if (11..=13).contains(&n) {
            vec![run.clone()]
        } else if n > 13 {
            let c: Vec<char> = run.chars().collect();
            (0..=n - 12).map(|i| c[i..i + 12].iter().collect()).collect()
        } else {
            continue;
        };
        for cand in candidates {
            let m = if tags { db.match_tag(&cand) } else { db.match_kks(&cand) };
            // 센서 태그(`10LAB10CP001`)는 설비 코드(`10LAB10AP001`)와 한 글자 차이라서, 태그에 더 가까운 글자는 설비로 보지 않습니다.
            if !tags && closer_to_a_tag(db, &cand, &m) {
                continue;
            }
            if m.code().is_some() && !out.iter().any(|o| o.code() == m.code()) {
                out.push(m);
                break;
            }
        }
    }
    out
}

fn closer_to_a_tag(db: &Lattice, cand: &str, equipment: &KksMatch) -> bool {
    let repaired = db.repair(cand);
    let tag_distance = match db.match_tag(cand) {
        KksMatch::Exact(_) => 0,
        KksMatch::Corrected { code, .. } => edit_distance(&repaired, &code),
        _ => return false,
    };
    match equipment {
        KksMatch::Exact(_) => false,
        KksMatch::Corrected { code, .. } => tag_distance <= edit_distance(&repaired, code),
        _ => true,
    }
}

fn correction(field: &str, m: &KksMatch) -> Option<Correction> {
    match m {
        KksMatch::Corrected { raw, code, distance } => Some(Correction { field: field.to_string(), raw: raw.clone(), fixed: code.clone(), distance: *distance }),
        _ => None,
    }
}

/// 문서 종류를 정합니다. `prefixes` 는 원장의 변경 관리 번호 머리입니다 (`MOC` · `MOP`).
fn classify(text: &str, prefixes: &[String]) -> DocKind {
    let flat = squeeze(text);
    let has = |label: &str| flat.contains(&squeeze(label)) || best_distance(label, &flat) <= 1;
    if has("현장 계기 점검표") || has("계기 점검표") {
        DocKind::GaugeCard
    } else if has("도면번호") || flat.contains("PID") {
        if moc_runs(text, prefixes).is_empty() {
            DocKind::Drawing
        } else {
            DocKind::Redline
        }
    } else if !moc_runs(text, prefixes).is_empty() && arrow_pair(text).is_some() {
        // 표제란이 없는 조각만 읽었어도(A0 타일 · 바뀐 곳만 읽은 델타) 변경번호와 변경 수치가 함께 있으면 Red-line 입니다.
        DocKind::Redline
    } else {
        DocKind::Unknown
    }
}

/// `want` 와 `text` 의 어떤 부분 문자열 사이의 가장 작은 편집 거리
fn best_distance(want: &str, text: &str) -> usize {
    let want: Vec<char> = squeeze(want).chars().collect();
    let mut col: Vec<usize> = (0..=want.len()).collect();
    let mut best = want.len();
    for c in text.chars() {
        let mut diag = col[0];
        col[0] = 0;
        for (i, &w) in want.iter().enumerate() {
            let next = (diag + (w != c) as usize).min(col[i] + 1).min(col[i + 1] + 1);
            diag = col[i + 1];
            col[i + 1] = next;
        }
        best = best.min(col[want.len()]);
    }
    best
}

/// `MOC-2026-1104` · `MOP-2026-1104` 꼴 (오독 `M0C` · `MDP`, 공백 포함). 머리는 원장에 있는 것만 봅니다.
fn moc_runs(text: &str, prefixes: &[String]) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        let flat: String = line.chars().filter(|c| !c.is_whitespace()).collect();
        for prefix in prefixes {
            let mut up = flat.to_uppercase();
            // 머리의 O 를 0 · D 로 읽은 오독 (`M0C` · `MDP`)
            for wrong in [prefix.replace('O', "0"), prefix.replace('O', "D")] {
                if wrong != *prefix {
                    up = up.replace(&wrong, prefix);
                }
            }
            let mut from = 0;
            while let Some(i) = up[from..].find(prefix.as_str()) {
                let start = from + i;
                // `MOC-연도-번호`: 숫자 여덟 개까지만 봅니다. 숫자 자리의 O · I · L 은 숫자로 읽습니다.
                let mut tail = prefix.clone();
                let mut digits = 0;
                for c in up[start + prefix.len()..].chars() {
                    let d = match c {
                        '0'..='9' => Some(c),
                        'O' => Some('0'),
                        'I' | 'L' => Some('1'),
                        _ => None,
                    };
                    match d {
                        Some(d) if digits < 8 => {
                            tail.push(d);
                            digits += 1;
                        }
                        None if c == '-' => tail.push('-'),
                        _ => break,
                    }
                }
                if digits >= 6 {
                    out.push(tail.trim_end_matches('-').to_string());
                }
                from = start + prefix.len();
            }
        }
    }
    out
}

/// `29.0 MPa → 31.0 MPa` 꼴에서 (이전, 이후). 화살표 바로 앞뒤의 값만 봅니다: 왼쪽은 숫자로 끝나고(뒤에 단위 하나까지),
/// 오른쪽은 숫자로 시작합니다(앞에 `변경` 같은 한글 낱말 하나까지). 도면 점프 상자의 `점프 → PID-101-HAD` 는 값 변경이 아니어서
/// 건너뜁니다. 판독 순서상 그 줄이 메모보다 먼저 오면 옆 상자 글과 붙어 `2 → -101` 로 읽히던 문제입니다.
fn arrow_pair(text: &str) -> Option<(f64, f64)> {
    let numeric = |w: &&str| {
        let mut cs = w.chars();
        match cs.next() {
            Some(c) if c.is_ascii_digit() => true,
            Some('-' | '+') => cs.next().is_some_and(|d| d.is_ascii_digit()),
            _ => false,
        }
    };
    let hangul = |w: &&str| w.chars().all(|c| ('가'..='힣').contains(&c));
    let unit = |w: &&str| w.chars().count() <= 5 && !w.chars().any(|c| c.is_ascii_digit() || ('가'..='힣').contains(&c));
    for line in text.lines() {
        for arrow in ["→", "->", "=>", "➔", "⇒"] {
            let Some((left, right)) = line.split_once(arrow) else { continue };
            let mut ahead = right.split_whitespace();
            let after = match ahead.next() {
                Some(w) if numeric(&w) => number_in(w),
                Some(w) if hangul(&w) => ahead.next().filter(numeric).and_then(number_in),
                _ => None,
            };
            let mut behind = left.split_whitespace().rev();
            let before = match behind.next() {
                Some(w) if numeric(&w) => number_in(w),
                Some(w) if unit(&w) => behind.next().filter(numeric).and_then(number_in),
                _ => None,
            };
            if let (Some(a), Some(b)) = (before, after) {
                return Some((a, b));
            }
        }
    }
    None
}

/// 한국 시각으로 적힌 `2026-11-04 14:21:05`
fn local_time(s: &str) -> Option<DateTime<Utc>> {
    let digits: String = s.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() < 12 {
        return None;
    }
    let text = format!("{}-{}-{} {}:{}:{}", &digits[0..4], &digits[4..6], &digits[6..8], &digits[8..10], &digits[10..12], digits.get(12..14).unwrap_or("00"));
    let naive = NaiveDateTime::parse_from_str(&text, "%Y-%m-%d %H:%M:%S").ok()?;
    crate::plant::kst().from_local_datetime(&naive).single().map(|t| t.with_timezone(&Utc))
}

fn hangul_name(s: &str) -> Option<String> {
    let name: String = s.chars().filter(|c| ('가'..='힣').contains(c)).collect();
    (2..=4).contains(&name.chars().count()).then_some(name)
}

fn worker_id_in(s: &str) -> Option<String> {
    ascii_runs(&s.replace('-', " - ").replace(" - ", "-"))
        .into_iter()
        .chain(s.split_whitespace().map(str::to_string))
        .map(|w| w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-').to_uppercase())
        .find(|w| w.len() >= 3 && w.len() <= 7 && w.chars().next().is_some_and(|c| c.is_ascii_alphabetic()) && w.chars().any(|c| c.is_ascii_digit()))
}

/// 계기 점검표의 항목 (설비 코드 칸은 발전소 `설비 KKS`, 데이터센터 `자산 코드`)
const GAUGE_LABELS: [(&str, &[&str]); 8] = [
    ("taken_at", &["점검 일시"]),
    ("worker_name", &["점검자"]),
    ("worker_id", &["사번"]),
    ("sector", &["관할 섹터", "관할 구역"]),
    ("kks", &["설비 KKS", "자산 코드"]),
    ("tag", &["센서 태그"]),
    ("reading", &["현장 실측치"]),
    ("unit", &["단위"]),
];

/// 설비 코드 칸의 이름 (교정 기록 · 확인 요청에 씁니다): `설비 KKS` · `설비 자산코드`
pub fn code_field(db: &Lattice) -> String {
    format!("설비 {}", db.terms().code)
}

fn gauge_card(db: &Lattice, page: &PageText, out: &mut Extracted) {
    let mut found: Vec<(&str, String, f32)> = Vec::new();
    for crop in &page.crops {
        for row in rows_of(crop) {
            for (i, cell) in row.iter().enumerate() {
                let Some((key, _)) = GAUGE_LABELS.iter().find(|(_, labels)| labels.iter().any(|l| near(&cell.text, l))) else { continue };
                if found.iter().any(|(k, _, _)| k == key) {
                    continue;
                }
                if let Some(value) = row.get(i + 1) {
                    found.push((key, value.text.clone(), value.conf));
                }
            }
        }
    }
    let get = |key: &str| found.iter().find(|(k, _, _)| *k == key).map(|(_, v, c)| (v.clone(), *c));
    let code_label = code_field(db);
    let mut low = |label: &str| out.uncertain.push(format!("{label} 확인"));
    if let Some((v, _)) = get("taken_at") {
        out.taken_at = local_time(&v);
    }
    if let Some((v, c)) = get("kks") {
        let m = db.match_kks(&v);
        out.corrections.extend(correction(&code_label, &m));
        match m.code() {
            Some(code) => out.kks = Some(code.to_string()),
            None => low(&code_label),
        }
        if c < LOW_CONF && !matches!(m, KksMatch::Exact(_) | KksMatch::Corrected { .. }) {
            low(&code_label);
        }
    } else {
        low(&code_label);
    }
    if let Some((v, _)) = get("tag") {
        let m = db.match_tag(&v);
        out.corrections.extend(correction("센서 태그", &m));
        match m.code() {
            Some(code) => out.tag = Some(code.to_string()),
            None => low("센서 태그"),
        }
    }
    match get("reading") {
        Some((v, c)) => {
            out.reading = number_in(&v);
            if out.reading.is_none() || c < LOW_CONF {
                low("현장 실측치");
            }
        }
        None => low("현장 실측치"),
    }
    if let Some((v, _)) = get("unit") {
        out.unit = Some(v.split_whitespace().collect::<String>());
    }
    if let Some((v, _)) = get("sector") {
        out.sector = db.sectors().iter().find(|s| v.contains(&s.building) || near(&v, &format!("{} {}", s.building, s.label))).and_then(|s| Sector::from_key(&s.key));
    }
    let name = get("worker_name").and_then(|(v, _)| hangul_name(&v));
    let id = get("worker_id").and_then(|(v, _)| worker_id_in(&v));
    resolve_worker(db, name, id, out);
}

/// 이름과 사번을 원장과 맞춥니다. 둘이 다른 사람을 가리키면 사람이 확인합니다.
fn resolve_worker(db: &Lattice, name: Option<String>, id: Option<String>, out: &mut Extracted) {
    let by_id = id.as_deref().and_then(|i| db.person(i));
    let by_name = name.as_deref().and_then(|n| db.person_named(n));
    match (by_id, by_name) {
        (Some(a), Some(b)) if a.id != b.id => out.uncertain.push("점검자와 사번이 다른 사람입니다".to_string()),
        (Some(p), _) | (None, Some(p)) => {
            out.worker_id = Some(p.id.clone());
            out.worker_name = Some(p.name.clone());
            return;
        }
        (None, None) => out.uncertain.push("점검자 확인".to_string()),
    }
    out.worker_name = name;
    out.worker_id = id;
}

/// 도면 표제란 (`도면번호: PID-103-LAB (Rev.3)`)
fn title_block(db: &Lattice, text: &str, out: &mut Extracted) {
    let flat: String = text.chars().filter(|c| !c.is_whitespace()).collect::<String>().to_uppercase();
    // 도면 간 점프 상자에도 다른 도면 번호가 적혀 있어서, `도면번호` 바로 뒤의 번호를 먼저 봅니다.
    let titled = flat.find("도면번호").map(|i| flat[i..].chars().skip(4).take(24).collect::<String>());
    let scope = titled.as_deref().unwrap_or(&flat).replace(['-', '_'], "");
    let mut best: Option<(&str, usize)> = None;
    for d in db.drawings() {
        let dist = best_distance(&d.id, &scope);
        if dist <= 2 && best.is_none_or(|(_, b)| dist < b) {
            best = Some((&d.id, dist));
        }
    }
    if let Some((id, _)) = best {
        out.drawing = Some(id.to_string());
        out.sector = db.drawing(id).and_then(|d| Sector::from_key(&d.sector));
    }
    let rev_from = flat.find("도면번호").unwrap_or(0);
    if let Some(i) = flat[rev_from..].find("REV").map(|i| i + rev_from) {
        let tail: String = flat[i + 3..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
        if !tail.is_empty() {
            out.drawing_rev = Some(format!("Rev.{tail}"));
        }
    }
}

fn drawing(db: &Lattice, page: &PageText, out: &mut Extracted) {
    let (w, h) = (page.width.max(1) as f32, page.height.max(1) as f32);
    for crop in &page.crops {
        let text = strip_tags(&crop.text);
        let conf = crop.confs.iter().copied().fold(1.0f32, f32::min);
        for m in find_codes(db, &text, false) {
            let code = m.code().unwrap_or_default().to_string();
            if out.kks_hits.iter().any(|k| k.code == code) {
                continue;
            }
            let raw = match &m {
                KksMatch::Corrected { raw, .. } => raw.clone(),
                _ => code.clone(),
            };
            let b = crop.bbox;
            out.kks_hits.push(KksHit { corrected: matches!(m, KksMatch::Corrected { .. }), raw, code, bbox: [b[0] as f32 / w, b[1] as f32 / h, b[2] as f32 / w, b[3] as f32 / h], conf });
        }
    }
}

fn redline(db: &Lattice, page: &PageText, out: &mut Extracted) {
    let text = page.text();
    let raw = moc_runs(&text, &db.change_prefixes());
    match raw.iter().find_map(|r| db.similar_moc(r).map(|m| (r, m))) {
        Some((_, m)) => {
            // 글자 그대로 없으면 오독을 고친 것입니다. 원문 낱말을 교정 기록에 남깁니다.
            if !text.contains(&m.moc_no) {
                let written = text
                    .split_whitespace()
                    .find(|w| w.to_uppercase().starts_with('M') && w.chars().filter(char::is_ascii_digit).count() >= 4)
                    .unwrap_or(&m.moc_no)
                    .to_string();
                out.corrections.push(Correction { field: format!("{} 번호", db.terms().change), distance: edit_distance(&written, &m.moc_no), raw: written, fixed: m.moc_no.clone() });
            }
            out.moc_no = Some(m.moc_no.clone());
        }
        None => {
            out.moc_no = raw.first().cloned();
            out.uncertain.push("변경번호 확인 (원장에 없음)".to_string());
        }
    }
    match arrow_pair(&text) {
        Some((old, new)) => {
            out.old_value = Some(old);
            out.new_value = Some(new);
            out.reading = Some(new);
        }
        None => out.uncertain.push("변경 수치 확인".to_string()),
    }
    // 수기 메모의 설비 코드: 마스터 도면에도 같은 코드가 있으므로 MOC 원장이 가리키는 설비를 먼저 봅니다.
    let ledger = out.moc_no.as_deref().and_then(|n| db.moc(n));
    let note_codes = find_codes(db, &text, false);
    match ledger {
        Some(m) if note_codes.iter().any(|c| c.code() == Some(m.kks.as_str())) => {
            out.kks = Some(m.kks.clone());
            out.tag = Some(m.tag.clone());
            if let Some(c) = note_codes.iter().find(|c| c.code() == Some(m.kks.as_str())).and_then(|c| correction(&code_field(db), c)) {
                out.corrections.push(c);
            }
        }
        Some(_) => out.uncertain.push("Red-line 의 설비 코드가 변경번호의 설비와 다릅니다".to_string()),
        None => {}
    }
    if let Some(m) = ledger {
        out.unit = Some(m.unit.clone());
    }
    // 서명자: 수기 메모 끝의 이름. 손글씨라 한 글자가 다르게 읽히면, 날짜가 함께 적힌 서명 줄(`2026-11-04 김태현`)에서만
    // 원장에 한 명뿐인 비슷한 이름으로 고치고 교정 기록에 남깁니다.
    let mut name = text.lines().rev().filter_map(hangul_name).find(|n| db.person_named(n).is_some());
    if name.is_none() {
        let dated = |l: &&str| l.split(|c: char| !c.is_ascii_digit() && c != '-').any(|w| w.len() == 10 && w.matches('-').count() == 2);
        if let Some((raw, p)) = text.lines().rev().filter(dated).filter_map(hangul_name).find_map(|n| db.person_one_off(&n).map(|p| (n, p))) {
            out.corrections.push(Correction { field: "서명자 이름".into(), distance: 1, raw, fixed: p.name.clone() });
            name = Some(p.name.clone());
        }
    }
    resolve_worker(db, name, None, out);
}

/// 한 장에서 문서 종류와 항목을 뽑습니다.
pub fn extract(page: &PageText, db: &Lattice) -> Extracted {
    let text = page.text();
    let kind = classify(&text, &db.change_prefixes());
    let mut out = Extracted { kind: Some(kind), signed: !page.signs.is_empty(), ..Extracted::default() };
    match kind {
        DocKind::GaugeCard => gauge_card(db, page, &mut out),
        DocKind::Redline => {
            title_block(db, &text, &mut out);
            drawing(db, page, &mut out);
            redline(db, page, &mut out);
        }
        DocKind::Drawing => {
            title_block(db, &text, &mut out);
            drawing(db, page, &mut out);
        }
        DocKind::Unknown => out.uncertain.push("문서 종류를 알아보지 못했습니다".to_string()),
    }
    if out.sector.is_none() {
        out.sector = out.kks.as_deref().and_then(|k| db.equipment(k)).and_then(|e| Sector::from_key(&e.sector));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    fn crop(text: &str, conf: f32) -> PageCrop {
        PageCrop { title: false, bbox: [100, 200, 900, 400], text: text.to_string(), confs: vec![conf; text.chars().count()] }
    }

    fn page(crops: Vec<PageCrop>, signed: bool) -> PageText {
        PageText { hash: "h".into(), width: 2000, height: 1400, crops, signs: if signed { vec![[10, 10, 50, 50]] } else { vec![] } }
    }

    #[test]
    fn a_gauge_card_is_read_and_its_misread_kks_is_restored() {
        let table = "<table><tr><td>점검 일시</td><td>2026-11-04 14:21:05</td></tr><tr><td>점검자</td><td>이승민</td></tr><tr><td>사번</td><td>E205</td></tr>\
                     <tr><td>관할 섹터</td><td>102동 터빈건물</td></tr><tr><td>설비 KKS</td><td>10MAAl0ST0O1</td></tr><tr><td>센서 태그</td><td>10MAA1OCP001</td></tr>\
                     <tr><td>현장 실측치</td><td>26.0</td></tr><tr><td>단위</td><td>MPa</td></tr></table>";
        let p = page(vec![PageCrop { title: true, ..crop("현장 계기 점검표", 0.99) }, crop(table, 0.97)], true);
        let x = extract(&p, &db());
        assert_eq!(x.kind, Some(DocKind::GaugeCard));
        assert_eq!(x.kks.as_deref(), Some("10MAA10ST001"));
        assert_eq!(x.tag.as_deref(), Some("10MAA10CP001"));
        assert_eq!(x.reading, Some(26.0));
        assert_eq!(x.sector, Some(Sector::Turbine));
        assert_eq!(x.worker_id.as_deref(), Some("E205"));
        assert_eq!(x.taken_at.map(|t| t.to_rfc3339()), Some("2026-11-04T05:21:05+00:00".to_string()));
        assert!(x.signed);
        assert_eq!(x.corrections.len(), 2, "{:?}", x.corrections);
        assert_eq!(x.corrections[0].fixed, "10MAA10ST001");
        assert!(x.uncertain.is_empty(), "{:?}", x.uncertain);
    }

    #[test]
    fn a_low_confidence_reading_goes_to_a_person() {
        let table = "<table><tr><td>현장 실측치</td><td>26.0</td></tr><tr><td>설비 KKS</td><td>10MAA10ST001</td></tr></table>";
        let p = page(vec![crop("현장 계기 점검표", 0.99), crop(table, 0.6)], false);
        let x = extract(&p, &db());
        assert!(x.uncertain.iter().any(|u| u.contains("실측치")), "{:?}", x.uncertain);
        assert!(!x.signed);
    }

    #[test]
    fn a_redline_gives_the_moc_number_old_and_new_values_and_the_signer() {
        let p = page(
            vec![
                PageCrop { title: true, ..crop("도면번호: PID-103-LAB (Rev.3) | 주급수펌프 및 고압급수 제어 계통도", 0.99) },
                crop("KKS: 10LABI0AP001\n주급수펌프 A", 0.95),
                crop("M0C-2026-1104\n10LABI0AP001 토출압력 경보\n29.0 MPa → 31.0 MPa\n2026-11-04 김태현", 0.92),
            ],
            true,
        );
        let x = extract(&p, &db());
        assert_eq!(x.kind, Some(DocKind::Redline));
        assert_eq!(x.drawing.as_deref(), Some("PID-103-LAB"));
        assert_eq!(x.drawing_rev.as_deref(), Some("Rev.3"));
        assert_eq!(x.moc_no.as_deref(), Some("MOC-2026-1104"));
        assert_eq!((x.old_value, x.new_value), (Some(29.0), Some(31.0)));
        assert_eq!(x.kks.as_deref(), Some("10LAB10AP001"));
        assert_eq!(x.tag.as_deref(), Some("10LAB10CP001"));
        assert_eq!(x.worker_id.as_deref(), Some("E102"));
        assert_eq!(x.sector, Some(Sector::Feedwater));
        assert!(x.corrections.iter().any(|c| c.field == "MOC 번호"));
        assert!(x.corrections.iter().any(|c| c.raw == "10LABI0AP001" && c.fixed == "10LAB10AP001"));
    }

    #[test]
    fn a_signer_name_misread_by_one_letter_is_fixed_only_on_the_dated_line() {
        let note = |last: &str| {
            page(
                vec![
                    PageCrop { title: true, ..crop("도면번호: PID-103-LAB (Rev.3)", 0.99) },
                    crop(&format!("MOC-2026-1104\n10LAB10AP001 토출압력 경보\n29.0 MPa → 31.0 MPa\n{last}"), 0.92),
                ],
                true,
            )
        };
        let x = extract(&note("2026-11-04 김래현"), &db());
        assert_eq!(x.worker_id.as_deref(), Some("E102"));
        assert!(x.corrections.iter().any(|c| c.field == "서명자 이름" && c.raw == "김래현" && c.fixed == "김태현"), "{:?}", x.corrections);
        // 날짜가 없는 줄의 비슷한 낱말은 이름으로 고치지 않습니다.
        let x = extract(&note("김래현"), &db());
        assert_eq!(x.worker_id, None);
        assert!(x.uncertain.iter().any(|u| u.contains("점검자")), "{:?}", x.uncertain);
    }

    #[test]
    fn the_title_block_wins_over_jump_boxes_that_name_other_drawings() {
        // 실측: PID-102 를 읽었는데 점프 상자의 `PID-101-HAD → 인입` 이 먼저 나와 101 로 읽혔습니다.
        let p = page(vec![crop("PID-101-HAD → 인입\n101동 최종과열기 (주증기)", 0.95), PageCrop { title: true, ..crop("도면번호: PID-102-MAA (Rev.4) | 주증기 터빈 및 발전기 계통도", 0.99) }], false);
        let x = extract(&p, &db());
        assert_eq!(x.drawing.as_deref(), Some("PID-102-MAA"));
        assert_eq!(x.drawing_rev.as_deref(), Some("Rev.4"));
    }

    #[test]
    fn a_master_drawing_lists_its_kks_with_normalized_boxes() {
        let p = page(
            vec![
                crop("도면번호: PID-103-LAB (Rev.3)", 0.99),
                PageCrop { bbox: [650, 435, 980, 610], ..crop("KKS: 10LAB10AP001\n주급수펌프 A", 0.95) },
                crop("KKS: 10LAB20AA1O2\n주급수 유량조절밸브", 0.95),
            ],
            true,
        );
        let x = extract(&p, &db());
        assert_eq!(x.kind, Some(DocKind::Drawing));
        assert_eq!(x.kks_hits.len(), 2);
        assert_eq!(x.kks_hits[0].bbox, [0.325, 435.0 / 1400.0, 0.49, 610.0 / 1400.0]);
        assert!(x.kks_hits[1].corrected);
    }

    #[test]
    fn small_parsers_handle_plant_text() {
        assert_eq!(number_in("1,850.0 t/h"), Some(1850.0));
        assert_eq!(number_in("-0.10 kPa"), Some(-0.1));
        assert_eq!(number_in("값 없음"), None);
        assert_eq!(arrow_pair("29.0 MPa → 31.0 MPa"), Some((29.0, 31.0)));
        assert_eq!(arrow_pair("기존 29.0 MPa -> 변경 31.0 MPa"), Some((29.0, 31.0)));
        assert_eq!(arrow_pair("29.0MPa →31.0MPa"), Some((29.0, 31.0)));
        assert_eq!(arrow_pair("-0.5 kPa → 0.5 kPa"), Some((-0.5, 0.5)));
        // 점프 상자는 값 변경이 아닙니다. 옆 설비 상자 글과 한 줄로 붙어 먼저 읽혀도 메모의 값을 고릅니다.
        assert_eq!(arrow_pair("점프 → PID-101-HAD"), None);
        assert_eq!(arrow_pair("위치: 103동 2F (EL.+9.0m) CF001 급수 유량 1,850.0 t/h 점프 → PID-101-HAD\n29.0 MPa → 31.0 MPa"), Some((29.0, 31.0)));
        assert_eq!(arrow_pair("103동 2F → PID-101-HAD"), None);
        let moc = ["MOC".to_string()];
        assert_eq!(moc_runs("변경번호 : MOC - 2026 - 1104", &moc), vec!["MOC-2026-1104".to_string()]);
        assert_eq!(moc_runs("MDP-2026-11O4", &["MOP".to_string()]), vec!["MOP-2026-1104".to_string()], "데이터센터 MOP 번호와 그 오독");
        assert!(moc_runs("MOP-2026-1104", &moc).is_empty(), "원장에 없는 머리는 보지 않음");
        assert_eq!(classify("어떤 글", &moc), DocKind::Unknown);
        assert_eq!(classify("MOC-2026-1104\n29.0 MPa → 31.0 MPa", &moc), DocKind::Redline, "표제란 없이 읽은 Red-line 메모");
        assert_eq!(classify("MOC-2026-1104 참조", &moc), DocKind::Unknown, "변경 수치가 없으면 Red-line 으로 보지 않음");
    }
}
