//! 다국어 질의의 닫힌 낱말 (한국어 · 영어 밖 10개 언어: de fr es it pt nl cs ar ja zh-hans).
//!
//! 한국어 · 영어는 `mod.rs` · `lexicon.rs` 의 규칙 그대로 두고, 나머지 언어는 `lexicon_ml.json` 의 낱말로 같은 조건(공간 · 층 · 설비 종류 ·
//! 계측 · 수치 · 상태 · 질적 상태 · 연결 · 부정어)을 만듭니다. 조건 값은 원장 코드(kind · 계측어 · 상태)라 데이터 언어와 상관없습니다.
//!
//! 낱말 맞추기 (logis-center 의 굴절 · 붙여 쓰기 규칙을 옮김: `search_relation::word_matches` · `arabic_bare_forms` ·
//! `event_type_lemma_key` 의 합성어 꼬리):
//! - 라틴 문자: 같음, 또는 앞머리가 max(4, 사전 낱말 길이 − 2) 글자 이상 같고 길이 차이가 4 이하 (Pumpen↔Pumpe, čerpadla↔čerpadlo,
//!   válvulas↔válvula). 독일어 · 네덜란드어는 합성어 안(Speisewasser**pumpe**, Lager**schwingung**)도 봅니다 — 앞 3글자 이상 뒤나 맨 앞.
//! - 아랍 문자: 접어(و ف + ب ل ك + ال لل)를 뗀 꼴로 같음 · 앞머리 (المضخات → مضخات ↔ مضخة).
//! - 일본어 · 중국어: 질의 글 안의 부분 일치. 한 글자 낱말(泵 · 阀 · 高 · 棟 · 階 …)은 앞뒤 글자를 봅니다 (高压 의 高 는 '높음' 이 아님).
//! - 여러 사전 낱말이 겹치면 긴 것이 이깁니다 (Ventilator ⊃ Ventil, 不正常 ⊃ 不, mehr als ⊃ mehr) — 사전 낱말에 대한 작은 NMS 입니다.
//!   겹치지 않으면 한 낱말에서 둘 이상 나옵니다 (Temperatur·sensor → 계측 + 센서, Lager·schwingung → 베어링 + 진동).

use std::collections::HashMap;
use std::sync::OnceLock;

use serde::Deserialize;

use super::analyze::QueryToken;
use super::lang::fold;
use super::{Cond, DocKind, Op, Quality, MEASURE_WORDS};
use crate::lattice::Lattice;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Head {
    /// 첫 구(첫 기능어 앞)의 끝 내용어 (en · de · nl: "Ventile in Gebäude 103", "festsitzende Ventile")
    FirstPhraseEnd,
    /// 첫 명사 (fr · es · it · pt · cs · ar: "vannes du bâtiment 103", "zaseknuté ventily" 의 ventily)
    FirstNoun,
    /// 끝 내용어 (ja · zh-hans: "103号棟2階のバルブ")
    LastContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Writing {
    Latin,
    Arabic,
    Glued,
}

/// 사전 낱말의 쓰임
#[derive(Debug, Clone, PartialEq)]
pub enum Cat {
    Type(DocKind),
    Generic,
    Kind(String),
    Measure(&'static str),
    Quality(Quality),
    Level(&'static str),
    Pending,
    Approved,
    HighRisk,
    Up,
    Down,
    Near,
    Hop,
    CmpBefore(Op),
    CmpAfter(Op),
    NegBefore,
    NegAfter,
    Building { before: bool },
    Floor { before: bool },
    Basement,
    Outdoor,
    Ground,
    Ordinal(u32),
    Unit(&'static str),
}

#[derive(Debug, Clone)]
struct Entry {
    words: Vec<String>,
    /// 붙여 쓰는 언어용 (공백 없이)
    joined: String,
    cat: Cat,
}

/// 한 언어의 사전
#[derive(Debug)]
pub struct Lex {
    pub code: &'static str,
    pub writing: Writing,
    pub head: Head,
    pub compound: bool,
    function: Vec<String>,
    command: Vec<String>,
    domain: Vec<String>,
    neg_prefix: Vec<String>,
    entries: Vec<Entry>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawNeg {
    before: Vec<String>,
    after: Vec<String>,
    prefix: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawCmp {
    before: Vec<(String, String)>,
    after: Vec<(String, String)>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawSides {
    before: Vec<String>,
    after: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawFloor {
    before: Vec<String>,
    after: Vec<String>,
    ordinals: HashMap<String, u32>,
    ground: Vec<String>,
    basement: Vec<String>,
    outdoor: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawLang {
    script: String,
    head: String,
    compound: bool,
    function: Vec<String>,
    command: Vec<String>,
    domain: Vec<String>,
    negation: RawNeg,
    comparators: RawCmp,
    building: RawSides,
    floor: RawFloor,
    types: HashMap<String, Vec<String>>,
    kinds: HashMap<String, Vec<String>>,
    measures: HashMap<String, Vec<String>>,
    qualities: HashMap<String, Vec<String>>,
    levels: HashMap<String, Vec<String>>,
    status: HashMap<String, Vec<String>>,
    relations: HashMap<String, Vec<String>>,
    units: HashMap<String, Vec<String>>,
    generic: Vec<String>,
}

#[derive(Deserialize)]
struct RawLex {
    langs: HashMap<String, RawLang>,
}

fn op_of(s: &str) -> Option<Op> {
    Some(match s {
        "gte" => Op::Gte,
        "gt" => Op::Gt,
        "lte" => Op::Lte,
        "lt" => Op::Lt,
        _ => return None,
    })
}

fn doc_kind(s: &str) -> Option<DocKind> {
    Some(match s {
        "sensor" => DocKind::Sensor,
        "incident" => DocKind::Incident,
        "archive" => DocKind::Archive,
        "aop" => DocKind::Aop,
        "moc" => DocKind::Moc,
        "drawing" => DocKind::Drawing,
        "equipment" => DocKind::Equipment,
        _ => return None,
    })
}

fn quality(s: &str) -> Option<Quality> {
    Some(match s {
        "high" => Quality::High,
        "low" => Quality::Low,
        "rising" => Quality::Rising,
        "falling" => Quality::Falling,
        "stuck" => Quality::Stuck,
        "frozen" => Quality::Frozen,
        "abnormal" => Quality::Abnormal,
        _ => return None,
    })
}

fn measure_code(s: &str) -> Option<&'static str> {
    MEASURE_WORDS.iter().map(|(m, _)| *m).find(|m| *m == s)
}

impl Lex {
    fn build(code: &'static str, raw: RawLang) -> Self {
        let writing = match raw.script.as_str() {
            "glued" => Writing::Glued,
            "arabic" => Writing::Arabic,
            _ => Writing::Latin,
        };
        let head = match raw.head.as_str() {
            "first_noun" => Head::FirstNoun,
            "last_content" => Head::LastContent,
            _ => Head::FirstPhraseEnd,
        };
        let mut entries = Vec::new();
        let mut add = |p: &str, cat: Cat| {
            let f = fold(p.trim());
            if f.is_empty() {
                return;
            }
            let words: Vec<String> = f.split_whitespace().map(str::to_string).collect();
            let joined = words.concat();
            entries.push(Entry { words, joined, cat });
        };
        for (k, ws) in &raw.types {
            if let Some(d) = doc_kind(k) {
                for w in ws {
                    add(w, if d == DocKind::Equipment { Cat::Generic } else { Cat::Type(d) });
                }
            }
        }
        for w in &raw.generic {
            add(w, Cat::Generic);
        }
        for (k, ws) in &raw.kinds {
            for w in ws {
                add(w, Cat::Kind(k.clone()));
            }
        }
        for (k, ws) in &raw.measures {
            if let Some(m) = measure_code(k) {
                for w in ws {
                    add(w, Cat::Measure(m));
                }
            }
        }
        for (k, ws) in &raw.qualities {
            if let Some(q) = quality(k) {
                for w in ws {
                    add(w, Cat::Quality(q));
                }
            }
        }
        for (k, ws) in &raw.levels {
            let l: &'static str = match k.as_str() {
                "trip" => "trip",
                "alarm" => "alarm",
                _ => "normal",
            };
            for w in ws {
                add(w, Cat::Level(l));
            }
        }
        for (k, ws) in &raw.status {
            let c = match k.as_str() {
                "pending" => Cat::Pending,
                "approved" => Cat::Approved,
                _ => Cat::HighRisk,
            };
            for w in ws {
                add(w, c.clone());
            }
        }
        for (k, ws) in &raw.relations {
            let c = match k.as_str() {
                "upstream" => Cat::Up,
                "downstream" => Cat::Down,
                "near" => Cat::Near,
                _ => Cat::Hop,
            };
            for w in ws {
                add(w, c.clone());
            }
        }
        for (w, o) in &raw.comparators.before {
            if let Some(op) = op_of(o) {
                add(w, Cat::CmpBefore(op));
            }
        }
        for (w, o) in &raw.comparators.after {
            if let Some(op) = op_of(o) {
                add(w, Cat::CmpAfter(op));
            }
        }
        for w in &raw.negation.before {
            add(w, Cat::NegBefore);
        }
        for w in &raw.negation.after {
            add(w, Cat::NegAfter);
        }
        for w in &raw.building.before {
            add(w, Cat::Building { before: true });
        }
        for w in &raw.building.after {
            add(w, Cat::Building { before: false });
        }
        for w in &raw.floor.before {
            add(w, Cat::Floor { before: true });
        }
        for w in &raw.floor.after {
            add(w, Cat::Floor { before: false });
        }
        for (w, n) in &raw.floor.ordinals {
            add(w, Cat::Ordinal(*n));
        }
        for w in &raw.floor.ground {
            add(w, Cat::Ground);
        }
        for w in &raw.floor.basement {
            add(w, Cat::Basement);
        }
        for w in &raw.floor.outdoor {
            add(w, Cat::Outdoor);
        }
        for (k, ws) in &raw.units {
            if let Some(m) = measure_code(k) {
                for w in ws {
                    add(w, Cat::Unit(m));
                }
            }
        }
        let fold_all = |v: &[String]| v.iter().map(|x| fold(x)).collect::<Vec<_>>();
        Self {
            code,
            writing,
            head,
            compound: raw.compound,
            function: fold_all(&raw.function),
            command: fold_all(&raw.command),
            domain: fold_all(&raw.domain),
            neg_prefix: fold_all(&raw.negation.prefix),
            entries,
        }
    }

    /// 언어 판정 투표에 쓰는 낱말 (기능어 · 도메인 · 닫힌 낱말 전부)
    pub fn vocabulary(&self) -> Vec<String> {
        let mut v: Vec<String> = self.function.iter().chain(self.command.iter()).chain(self.domain.iter()).cloned().collect();
        v.extend(self.entries.iter().map(|e| e.words.join(" ")));
        if self.code == "en" {
            let l = &super::lexicon::KIND_WORDS_EN;
            v.extend(l.iter().map(|(w, _)| w.to_string()));
            v.extend(super::lexicon::MEASURE_WORDS_EN.iter().map(|(w, _)| w.to_string()));
            v.extend(super::lexicon::QUALITY_WORDS_EN.iter().map(|(w, _)| w.to_string()));
            v.extend(super::lexicon::FILLER_EN.iter().map(|w| w.to_string()));
            for (_, ws) in super::lexicon::TYPE_WORDS_EN.iter() {
                v.extend(ws.iter().filter(|w| !w.ends_with('-')).map(|w| w.to_string()));
            }
        }
        v
    }

    /// 계측어 낱말 (접은 꼴, 여러 낱말은 공백으로) — 원장 라벨의 계측 종류를 정할 때 (`mod.rs` label_measures)
    pub fn measure_words(&self) -> Vec<(String, &'static str)> {
        self.entries
            .iter()
            .filter_map(|e| if let Cat::Measure(m) = e.cat { Some((if self.writing == Writing::Glued { e.joined.clone() } else { e.words.join(" ") }, m)) } else { None })
            .collect()
    }

    /// 기능어 · 명령 동사 (Stanza 품사가 없을 때)
    pub fn is_function(&self, form: &str) -> bool {
        self.function.iter().any(|w| w == form) || self.command.iter().any(|w| w == form)
    }

    pub fn is_command(&self, form: &str) -> bool {
        self.command.iter().any(|w| w == form)
    }
}

/// 사전 (처음 부를 때 한 번 읽음)
pub fn lex(lang: &str) -> Option<&'static Lex> {
    static ALL: OnceLock<HashMap<&'static str, Lex>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let raw: RawLex = serde_json::from_str(include_str!("lexicon_ml.json")).expect("lexicon_ml.json 형식");
        let mut out = HashMap::new();
        for (k, v) in raw.langs {
            if let Some(code) = super::lang::code(&k) {
                out.insert(code, Lex::build(code, v));
            }
        }
        out
    });
    all.get(lang)
}

/// 아랍어 접어를 뗀 꼴들: و/ف, 그 뒤 ب/ل/ك, 그 뒤 ال/لل (두 글자 이상 남을 때만)
pub fn arabic_bare(w: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur: Vec<char> = w.chars().collect();
    let push = |c: &[char], out: &mut Vec<String>| {
        if c.len() >= 2 {
            let s: String = c.iter().collect();
            if !out.contains(&s) {
                out.push(s);
            }
        }
    };
    if cur.len() > 3 && matches!(cur[0], 'و' | 'ف') {
        cur.remove(0);
        push(&cur, &mut out);
    }
    if cur.len() > 3 && matches!(cur[0], 'ب' | 'ل' | 'ك') {
        let mut c2 = cur.clone();
        c2.remove(0);
        push(&c2, &mut out);
        if c2.len() > 3 && c2[0] == 'ا' && c2[1] == 'ل' {
            push(&c2[2..], &mut out);
        }
        if cur[0] == 'ل' && cur.len() > 3 && cur[1] == 'ل' {
            push(&cur[2..], &mut out);
        }
    }
    if cur.len() > 3 && cur[0] == 'ا' && cur[1] == 'ل' {
        push(&cur[2..], &mut out);
    }
    out
}

/// 앞머리가 같은 글자 수
fn common_prefix(a: &[char], b: &[char]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// 굴절 앞머리 일치: 사전 낱말 4글자 이상(아랍어 3), 앞머리 max(4, n − 2) (아랍어 max(3, n − 1)) 이상, 길이 차이 4 이하
/// 굴절 꼴인가: 사전 낱말의 끝 한 글자(8글자 이상은 두 글자)까지만 다르고, 질의 낱말 쪽 남은 꼬리는 세 글자 이하 (어미 · 복수 · 격).
/// válvula·s, čerpadl·a ↔ čerpadl·o, budov·ě ↔ budov·a, teplot·ou ↔ teplot·a, abnormal·er. 꼬리가 길면 다른 낱말입니다:
/// 'ventil·átor'(팬) 은 'ventil'(밸브)이 아니고, 'plan·ta'(층) 는 'plan·o'(도면)가 아님 (처음 규칙은 앞머리만 봐서 둘 다 맞았음).
fn inflected(form: &[char], entry: &[char], arabic: bool) -> bool {
    let n = entry.len();
    let min_len = if arabic { 3 } else { 4 };
    if n < min_len {
        return false;
    }
    let common = common_prefix(form, entry);
    let entry_rest = n - common;
    let form_rest = form.len().saturating_sub(common);
    let allowed = if arabic || n >= 8 { 2 } else { 1 };
    // 질의 낱말이 사전 낱말의 앞부분일 뿐이면(어미 없음) 한 글자 모자란 것까지만: خار 는 خارجي(옥외)가 아님, منخفض 는 منخفضة
    let allowed = if form_rest == 0 { allowed.min(1) } else { allowed };
    common >= min_len && entry_rest <= allowed && form_rest <= 3
}

/// 낱말 맞춘 자리 하나
#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    /// 질의 낱말 자리 [start, end)
    pub start: usize,
    pub end: usize,
    /// 이어 붙인 질의 글(낱말 사이 공백 하나, 붙여 쓰는 언어는 공백 없음)의 글자 자리 [c0, c1)
    pub c0: usize,
    pub c1: usize,
    pub cat: Cat,
    /// 맞은 글자 수 (긴 것이 이김)
    pub score: usize,
    /// 사전 낱말 (기록용)
    pub entry: String,
    /// 어느 사전에서 나왔나 (0 = 질의 언어, 1.. = 판정이 비긴 언어)
    pub src: usize,
    /// 글자 그대로 맞았나 (굴절 앞머리 · 원형으로 맞은 것은 false)
    pub exact: bool,
}

/// 비교에 쓰는 낱말 꼴 (접은 원문 · 원형 · 아랍어 접어 뗀 꼴)
fn forms_of(t: &QueryToken, writing: Writing) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for f in &t.forms {
        let f = fold(f);
        if !f.is_empty() && !out.contains(&f) {
            out.push(f);
        }
    }
    // 접어는 원문 꼴에서만 뗍니다. 원형은 이미 맨 꼴이라 다시 떼면 낱말 첫 글자가 잘립니다 (البخار 의 원형 بخار → خار → خارجي '옥외')
    if writing == Writing::Arabic {
        if let Some(surface) = out.first().cloned() {
            for b in arabic_bare(&surface) {
                if !out.contains(&b) {
                    out.push(b);
                }
            }
        }
    }
    out
}

/// 낱말 하나 안에서 사전 낱말이 맞은 글자 자리들 (원문 꼴 기준). 원형 · 접어 뗀 꼴로 맞으면 낱말 전체.
fn spans_in(t: &QueryToken, forms: &[String], e: &Entry, lex: &Lex) -> Vec<(usize, usize, bool)> {
    let w: Vec<char> = e.words[0].chars().collect();
    let text: Vec<char> = forms.first().map(|f| f.chars().collect()).unwrap_or_default();
    let arabic = lex.writing == Writing::Arabic;
    let mut out = Vec::new();
    for f in forms {
        let fc: Vec<char> = f.chars().collect();
        if fc == w || inflected(&fc, &w, arabic) {
            out.push((0, text.len(), fc == w));
            break;
        }
    }
    // 합성어 안 (독일어 · 네덜란드어): 맨 앞이거나 앞 3글자 이상 뒤 (über·hitzer 의 'erhitzer' 는 2글자 뒤라 아님), 하이픈 뒤도
    if out.is_empty() && lex.compound && w.len() >= 4 && text.len() >= w.len() + 2 {
        let mut p = 0;
        while p + w.len() <= text.len() {
            if text[p..p + w.len()] == w[..] && (p == 0 || p >= 3 || text[p - 1] == '-') {
                out.push((p, p + w.len(), true));
                p += w.len();
            } else {
                p += 1;
            }
        }
    }
    // 하이픈으로 이은 낱말의 조각 (Speisewasser-Bypass, émetteur-récepteur 는 사전 낱말 그대로)
    if out.is_empty() && text.contains(&'-') && !e.words[0].contains('-') {
        let mut at = 0;
        for part in text.split(|c| *c == '-') {
            if part == &w[..] || inflected(part, &w, arabic) {
                out.push((at, at + part.len(), part == &w[..]));
            }
            at += part.len() + 1;
        }
    }
    let _ = t;
    out
}

fn word_ok(forms: &[String], w: &str, last: bool, arabic: bool) -> bool {
    let wc: Vec<char> = w.chars().collect();
    forms.iter().any(|f| f == w || (last && inflected(&f.chars().collect::<Vec<_>>(), &wc, arabic)))
}

/// 띄어 쓰는 언어 (라틴 · 아랍): 낱말 단위로 맞춥니다
fn match_spaced(toks: &[QueryToken], lexes: &[&Lex], starts: &[usize]) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (src, lex) in lexes.iter().enumerate() {
        let forms: Vec<Vec<String>> = toks.iter().map(|t| forms_of(t, lex.writing)).collect();
        let arabic = lex.writing == Writing::Arabic;
        for e in &lex.entries {
            let k = e.words.len();
            for i in 0..toks.len() {
                if forms[i].is_empty() || i + k > toks.len() {
                    continue;
                }
                if k == 1 {
                    for (a, b, exact) in spans_in(&toks[i], &forms[i], e, lex) {
                        hits.push(Hit { start: i, end: i + 1, c0: starts[i] + a, c1: starts[i] + b, cat: e.cat.clone(), score: b - a, entry: e.words[0].clone(), src, exact });
                    }
                } else if (0..k).all(|j| word_ok(&forms[i + j], &e.words[j], j + 1 == k, arabic)) {
                    let c1 = starts[i + k - 1] + forms[i + k - 1].first().map(|f| f.chars().count()).unwrap_or(0);
                    let exact = (0..k).all(|j| forms[i + j].iter().any(|f| *f == e.words[j]));
                    hits.push(Hit { start: i, end: i + k, c0: starts[i], c1, cat: e.cat.clone(), score: c1 - starts[i], entry: e.words.join(" "), src, exact });
                }
            }
        }
    }
    hits
}

/// 붙여 쓰는 언어 (일본어 · 중국어): 이어 붙인 글 안의 부분 일치
fn match_glued(toks: &[QueryToken], lexes: &[&Lex], starts: &[usize], text: &[char]) -> Vec<Hit> {
    let mut hits = Vec::new();
    let tok_of = |c: usize| -> usize { starts.iter().rposition(|s| *s <= c).unwrap_or(0) };
    let digit = |i: Option<usize>| i.and_then(|i| text.get(i)).is_some_and(|c| c.is_ascii_digit());
    for (src, lex) in lexes.iter().enumerate() {
        for e in &lex.entries {
            let w: Vec<char> = e.joined.chars().collect();
            if w.is_empty() || w.len() > text.len() {
                continue;
            }
            let ascii = w.iter().all(|c| c.is_ascii_alphanumeric());
            for a in 0..=text.len() - w.len() {
                if text[a..a + w.len()] != w[..] {
                    continue;
                }
                let b = a + w.len();
                let before = a.checked_sub(1).map(|i| text[i]);
                let after = text.get(b).copied();
                // 로마자 낱말(gpu · moc · ups)은 로마자 낱말 경계에서만
                if ascii && (before.is_some_and(|c| c.is_ascii_alphanumeric()) || after.is_some_and(|c| c.is_ascii_alphanumeric())) {
                    continue;
                }
                if w.len() == 1 {
                    let ok = match &e.cat {
                        // 번호 뒤에만: 103号棟 · 2階 · 2层 · 1跳
                        Cat::Building { before: false } | Cat::Floor { before: false } | Cat::Hop | Cat::CmpAfter(_) => digit(a.checked_sub(1)) || digit(a.checked_sub(2)),
                        // 번호 앞에만: 第103 · 负1
                        Cat::Building { before: true } | Cat::Floor { before: true } => digit(Some(b)),
                        // 높음 · 낮음 한 글자는 낱말 끝에서만 (压力高的 · 温度高): 高压 · 低压 의 高 · 低 는 아님
                        Cat::Quality(_) => after.is_none_or(|c| "的了いくめなのがはでに，。、 ".contains(c) || c.is_ascii()),
                        // 부정 한 글자는 내용 글자 앞에서만
                        Cat::NegBefore => after.is_some_and(|c| !c.is_ascii_digit() && !"的了".contains(c)),
                        Cat::Kind(_) | Cat::Type(_) | Cat::Generic | Cat::Measure(_) | Cat::Unit(_) => true,
                        _ => false,
                    };
                    if !ok {
                        continue;
                    }
                }
                let (s, t) = (tok_of(a), tok_of(b - 1) + 1);
                hits.push(Hit { start: s, end: t.max(s + 1), c0: a, c1: b, cat: e.cat.clone(), score: w.len(), entry: e.joined.clone(), src, exact: true });
            }
        }
    }
    let _ = toks;
    hits
}

/// 겹치는 맞춤은 긴 것 하나만 (같은 길이면 앞의 것). 같은 자리에 쓰임이 다른 맞춤이 둘 이상이면 질의 언어 사전의 것을 남깁니다
/// (스페인어 'planta'(층) ↔ 포르투갈어 'planta'(도면): 판정이 비겨 함께 본 사전이 질의 언어의 뜻을 덮지 않게).
fn suppress(mut hits: Vec<Hit>) -> Vec<Hit> {
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(b.exact.cmp(&a.exact)).then(a.src.cmp(&b.src)).then(a.c0.cmp(&b.c0)).then((b.end - b.start).cmp(&(a.end - a.start))));
    let mut kept: Vec<Hit> = Vec::new();
    for h in hits {
        let same_span = |k: &Hit| k.c0 == h.c0 && k.c1 == h.c1;
        // 같은 자리 · 같은 사전 · 같은 정확도면 뜻이 둘(독일어 'stock' 층 앞 · 뒤)이라 둘 다. 글자 그대로 맞은 것이 있으면 굴절로 맞은 다른 뜻은 버림
        let clash = kept.iter().any(|k| h.c0 < k.c1 && k.c0 < h.c1 && !(same_span(k) && k.cat != h.cat && k.src == h.src && k.exact == h.exact));
        if !clash && !kept.iter().any(|k| same_span(k) && k.cat == h.cat) {
            kept.push(h);
        }
    }
    kept.sort_by_key(|h| (h.c0, h.c1));
    kept
}

/// 질의 낱말들을 사전에 댑니다. `lexes[0]` 이 질의 언어, 나머지는 판정이 비긴 언어.
pub fn match_query(toks: &[QueryToken], lexes: &[&Lex]) -> Vec<Hit> {
    let glued = lexes.first().is_some_and(|l| l.writing == Writing::Glued);
    let mut starts = Vec::with_capacity(toks.len());
    let mut text: Vec<char> = Vec::new();
    for t in toks {
        if !glued && !text.is_empty() {
            text.push(' ');
        }
        starts.push(text.len());
        let f = fold(&t.text);
        text.extend(f.chars());
    }
    let hits = if glued { match_glued(toks, lexes, &starts, &text) } else { match_spaced(toks, lexes, &starts) };
    suppress(hits)
}

/// 다국어 질의에서 뽑은 것
#[derive(Debug, Default)]
pub struct Parsed {
    pub types: Vec<DocKind>,
    /// 머리 낱말이 설비 종류 · 일반 설비 낱말인가, 머리 낱말
    pub head: Option<(bool, String)>,
    pub(super) conds: Vec<Cond>,
    pub notes: Vec<String>,
    /// 연결: (상류 Some(true) · 하류 Some(false) · 주변 None, 홉)
    pub relation: Option<(Option<bool>, Option<usize>)>,
    /// 관계 · 비교 · 부정 · 홉 낱말 (NMS 창 · 글자 경로에서 뺌)
    pub closed: Vec<bool>,
    pub hits: Vec<Hit>,
}

fn has_digit(s: &str) -> bool {
    s.chars().any(|c| c.is_ascii_digit())
}

/// 서수 꼴 숫자: 2. · 2e · 2ème · 2º · 2ª · 2° · 2nd
fn ordinal_number(text: &str) -> Option<u32> {
    let digits: String = text.chars().take_while(|c| c.is_ascii_digit()).collect();
    let rest = &text[digits.len()..];
    let n: u32 = digits.parse().ok()?;
    matches!(rest, "" | "." | "e" | "er" | "ere" | "ère" | "eme" | "ème" | "º" | "ª" | "°" | "nd" | "rd" | "th" | "st" | "o" | "a" | "ste" | "de" | "ten").then_some(n)
}

/// 다국어 질의를 조건으로 풉니다. `quoted[i]`: 따옴표 안 낱말 (데이터 값이라 조건을 만들지 않음).
pub fn parse(toks: &[QueryToken], lexes: &[&Lex], db: &Lattice, quoted: &[bool]) -> Parsed {
    let lex = lexes[0];
    let glued = lex.writing == Writing::Glued;
    let hits: Vec<Hit> = match_query(toks, lexes).into_iter().filter(|h| !(h.start..h.end).any(|i| quoted.get(i).copied().unwrap_or(false))).collect();
    let n = toks.len();
    let mut out = Parsed { closed: vec![false; n], ..Default::default() };
    let at = |i: usize| hits.iter().filter(move |h| h.start <= i && i < h.end);
    let cat_at = |i: usize, f: &dyn Fn(&Cat) -> bool| at(i).any(|h| f(&h.cat));
    for h in &hits {
        if matches!(h.cat, Cat::Up | Cat::Down | Cat::Near | Cat::Hop | Cat::CmpBefore(_) | Cat::CmpAfter(_) | Cat::NegBefore | Cat::NegAfter) {
            for i in h.start..h.end {
                out.closed[i] = true;
            }
        }
    }
    let func = |i: usize| toks[i].function;
    // 붙여 쓰는 언어: 이어 붙인 글의 [a, b) 에 조사 · 기능 글자가 있는가
    let joined: Vec<char> = toks.iter().flat_map(|t| fold(&t.text).chars().collect::<Vec<_>>()).collect();
    let has_particle = |a: usize, b: usize| -> bool { joined.get(a..b.max(a)).is_some_and(|s| s.iter().any(|c| "のをがはにでとへもや的了在和与及 ".contains(*c))) };

    // 부정어: 앞(라틴 · 아랍 · 중국어 不/非) 3낱말 안, 뒤(일본어 ない · 以外) 3낱말 안, 앞가지(체코어 ne-)
    let negated = |i: usize| -> bool {
        let before = hits.iter().any(|h| h.cat == Cat::NegBefore && h.end <= i && i - h.end <= 2 && (h.end..i).all(func));
        let after = hits.iter().any(|h| h.cat == Cat::NegAfter && h.start > i && h.start - i <= 3 && (i + 1..h.start).all(func));
        let after_glued = glued && hits.iter().any(|h| h.cat == Cat::NegAfter && at(i).any(|x| x.c1 <= h.c0 && h.c0 - x.c1 <= 4) && !matches!(at(i).next().map(|x| &x.cat), Some(Cat::NegAfter)));
        let prefix = lex.neg_prefix.iter().any(|p| {
            let t = fold(&toks[i].text);
            t.len() > p.len() + 3 && t.starts_with(p.as_str()) && cat_at(i, &|c| matches!(c, Cat::Quality(_))) == false && {
                let rest = &t[p.len()..];
                lex.entries.iter().any(|e| matches!(e.cat, Cat::Quality(_)) && e.words.len() == 1 && inflected(&rest.chars().collect::<Vec<_>>(), &e.words[0].chars().collect::<Vec<_>>(), false))
            }
        });
        before || after || after_glued || prefix
    };
    let neg: Vec<bool> = (0..n).map(negated).collect();
    for (i, t) in toks.iter().enumerate() {
        if neg[i] && !t.function && !out.closed[i] {
            out.notes.push(format!("부정어가 붙은 '{}' 는 조건으로 쓰지 않습니다", t.text));
        }
    }
    let live = |h: &&Hit| !(h.start..h.end).any(|i| neg[i]);

    // 유형
    for h in hits.iter().filter(live) {
        if let Cat::Type(d) = h.cat {
            if !out.types.contains(&d) {
                out.types.push(d);
            }
        }
    }

    // 숫자: 동 · 층 · 홉 · 수치
    let num_of = |i: usize| -> Option<(f64, String)> {
        let t = &toks[i].text;
        if !has_digit(t) || t.chars().filter(|c| c.is_ascii_alphabetic()).count() >= 3 {
            return None;
        }
        super::number_unit(t)
    };
    // (앞에 있음, 뒤에 있음): 바로 앞 낱말 · 바로 뒤 낱말(기능어 하나 건너도 됨)의 사전 낱말
    let adjacent = |i: usize, f: &dyn Fn(&Cat) -> bool| -> (bool, bool) {
        let before = hits.iter().any(|h| f(&h.cat) && h.end <= i && (h.end..i).all(func) && i - h.end <= 1);
        let after = hits.iter().any(|h| f(&h.cat) && h.start > i && (i + 1..h.start).all(func) && h.start - i <= 2);
        (before, after)
    };
    let mut used_num = vec![false; n];
    let mut hop_count = None;
    for i in 0..n {
        if quoted.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((value, rest)) = num_of(i) else { continue };
        let whole = value.fract() == 0.0 && value >= 0.0;
        let ord = ordinal_number(&fold(&toks[i].text));
        let (b_before, b_after) = adjacent(i, &|c| matches!(c, Cat::Building { .. }));
        let (f_before, f_after) = adjacent(i, &|c| matches!(c, Cat::Floor { .. }));
        let (h_before, h_after) = adjacent(i, &|c| *c == Cat::Hop);
        if whole && (b_before || b_after) {
            used_num[i] = true;
            match super::sector_by_number(db, value as u32) {
                Some(key) => out.conds.push(Cond::Sector(key)),
                None => out.notes.push(format!("{} 동은 원장에 없습니다", value as u32)),
            }
        } else if whole && (h_after || h_before) {
            used_num[i] = true;
            out.closed[i] = true;
            hop_count = Some(value as usize);
        } else if (f_before || f_after) && ord.is_some() {
            used_num[i] = true;
            if let Some(f) = super::floor_code(ord.unwrap_or(0)) {
                out.conds.push(Cond::Floor(f.into()));
            }
        } else if ord.is_some() && rest.len() > 0 && !rest.chars().all(|c| c.is_alphabetic() && c.is_ascii()) {
            // "2º" 만 있고 층 낱말이 없음: 수치가 아님
            used_num[i] = true;
        }
        let _ = rest;
    }
    // 서수 낱말 + 층 낱말 ("zweite Etage", "segundo piso", "الطابق الثاني")
    for h in hits.iter().filter(live) {
        if let Cat::Ordinal(k) = h.cat {
            let near_floor = hits.iter().any(|x| matches!(x.cat, Cat::Floor { .. }) && (x.end == h.start || h.end == x.start || x.end + 1 == h.start || h.end + 1 == x.start));
            if near_floor {
                if let Some(f) = super::floor_code(k) {
                    out.conds.push(Cond::Floor(f.into()));
                }
            }
        }
    }
    for h in hits.iter().filter(live) {
        match h.cat {
            Cat::Basement => out.conds.push(Cond::Floor("B1F".into())),
            Cat::Outdoor => out.conds.push(Cond::Floor("OUT".into())),
            Cat::Ground => out.conds.push(Cond::Floor("1F".into())),
            _ => {}
        }
    }

    // 수치 + 비교어
    for i in 0..n {
        if used_num[i] || quoted.get(i).copied().unwrap_or(false) {
            continue;
        }
        let Some((value, rest)) = num_of(i) else { continue };
        // 비교어: 앞(같은 낱말 앞, 기능어 하나 건너도 됨) · 뒤(단위 다음까지)
        let before = hits.iter().filter(|h| matches!(h.cat, Cat::CmpBefore(_)) && h.end <= i && i - h.end <= 1 && (h.end..i).all(func)).max_by_key(|h| h.end);
        let unit_tok = hits.iter().find(|h| matches!(h.cat, Cat::Unit(_)) && h.start == i + 1);
        let after = if glued {
            let me = hits.iter().filter(|h| h.start <= i && i < h.end).map(|h| h.c1).max();
            let end_c = me.unwrap_or(0);
            let tok_end: usize = toks[..=i].iter().map(|t| fold(&t.text).chars().count()).sum();
            hits.iter().filter(|h| matches!(h.cat, Cat::CmpAfter(_)) && h.c0 >= tok_end.max(end_c) && h.c0 <= tok_end + 3).min_by_key(|h| h.c0)
        } else {
            let a0 = i + 1 + usize::from(unit_tok.is_some());
            hits.iter().filter(|h| matches!(h.cat, Cat::CmpAfter(_)) && (h.start == a0 || h.start == i + 1)).min_by_key(|h| h.start)
        };
        let op = match (before, after) {
            (_, Some(h)) | (Some(h), None) => match h.cat {
                Cat::CmpBefore(op) | Cat::CmpAfter(op) => op,
                _ => continue,
            },
            _ => continue,
        };
        let unit_measure = super::unit_measure(&rest)
            .or_else(|| super::lexicon::unit_measure_en(&rest))
            .or_else(|| unit_tok.and_then(|h| if let Cat::Unit(m) = h.cat { Some(m) } else { None }))
            .or_else(|| {
                // 붙여 쓰는 언어: 숫자 바로 뒤 단위 글자 (600度以上)
                let tok_end: usize = toks[..=i].iter().map(|t| fold(&t.text).chars().count()).sum();
                hits.iter().find(|h| matches!(h.cat, Cat::Unit(_)) && glued && h.c0 == tok_end).and_then(|h| if let Cat::Unit(m) = h.cat { Some(m) } else { None })
            })
            .or_else(|| toks.get(i + 1).and_then(|t| super::unit_measure(&t.text).or_else(|| super::lexicon::unit_measure_en(&t.text))));
        let measure = unit_measure.map(str::to_string).or_else(|| {
            hits.iter()
                .filter(|h| matches!(h.cat, Cat::Measure(_)) && (h.end <= i && i - h.end <= 4 || h.start > i && h.start - i <= 3))
                .min_by_key(|h| if h.end <= i { i - h.end } else { h.start - i })
                .and_then(|h| if let Cat::Measure(m) = h.cat { Some(m.to_string()) } else { None })
        });
        used_num[i] = true;
        out.conds.push(Cond::Number { measure, op, value });
    }

    // 설비 종류 · 계측 · 상태 · 질적 상태
    let kind_tok = |i: usize| cat_at(i, &|c| matches!(c, Cat::Kind(_)));
    // 계측어가 설비 이름의 일부인가 ("válvula de control de caudal", "Durchfluss·regel·ventil", "流量調節弁")
    let genitive = ["de", "du", "des", "del", "della", "dell", "di", "do", "da", "dos", "das", "od", "van", "von", "من", "في", "control", "controle", "regulation", "regulacion", "regolazione", "regulace", "regelung", "regel"];
    let in_name = |h: &Hit| -> bool {
        let i = h.start;
        if kind_tok(i) && at(i).any(|x| matches!(x.cat, Cat::Kind(_)) && x.c0 >= h.c1) {
            return true;
        }
        if glued {
            // 계측어 바로 뒤(네 글자 안)에 조사 없이 설비 종류가 오면 이름 (流量調節弁 · 流量调节阀)
            return hits.iter().any(|k| matches!(k.cat, Cat::Kind(_)) && k.c0 >= h.c1 && k.c0 - h.c1 <= 4 && !has_particle(h.c1, k.c0));
        }
        match lex.head {
            // 앞이 머리: 왼쪽으로 가며 'de · do · od …' 와 내용어만 지나 설비 종류 낱말을 만나면 이름
            Head::FirstNoun => {
                let mut j = i;
                while j > 0 {
                    j -= 1;
                    if kind_tok(j) {
                        return true;
                    }
                    let f = fold(&toks[j].text);
                    let blocked = cat_at(j, &|c| matches!(c, Cat::Quality(_) | Cat::CmpBefore(_) | Cat::CmpAfter(_) | Cat::NegBefore | Cat::Type(_) | Cat::Measure(_) | Cat::Building { .. } | Cat::Floor { .. }))
                        || has_digit(&f)
                        || (toks[j].function && !genitive.contains(&f.as_str()));
                    if blocked {
                        return false;
                    }
                }
                false
            }
            // 뒤가 머리 (영어식): 바로 뒤 내용어(두 낱말까지)에 설비 종류
            _ => (i + 1..(i + 3).min(n)).take_while(|j| !toks[*j].function && !has_digit(&toks[*j].text)).any(kind_tok),
        }
    };
    let mut kinds_seen: Vec<String> = Vec::new();
    for h in hits.iter().filter(live) {
        match &h.cat {
            Cat::Kind(k) => {
                if !kinds_seen.contains(k) {
                    kinds_seen.push(k.clone());
                    out.conds.push(Cond::Kind(vec![k.clone()]));
                }
            }
            Cat::Measure(m) => {
                if in_name(h) {
                    out.notes.push(format!("'{}' 는 설비 이름 안의 계측어라 조건이 아닙니다", h.entry));
                } else {
                    out.conds.push(Cond::Measure(m.to_string()));
                }
            }
            Cat::Level(l) => out.conds.push(Cond::Level(l)),
            Cat::Quality(q) => {
                let measure = hits
                    .iter()
                    .filter(|x| matches!(x.cat, Cat::Measure(_)))
                    .filter(|x| {
                        if glued {
                            (x.c1 <= h.c0 && h.c0 - x.c1 <= 6) || (x.c0 >= h.c1 && x.c0 - h.c1 <= 4)
                        } else {
                            (x.start <= h.start && h.start - x.start <= 2) || x.start == h.end || x.start == h.start
                        }
                    })
                    .min_by_key(|x| if x.c1 <= h.c0 { h.c0 - x.c1 } else { x.c0.saturating_sub(h.c1) })
                    .and_then(|x| if let Cat::Measure(m) = x.cat { Some(m.to_string()) } else { None });
                out.conds.push(Cond::Signal { measure, want: *q });
            }
            _ => {}
        }
    }
    let pending = hits.iter().filter(live).any(|h| h.cat == Cat::Pending);
    if out.types.contains(&DocKind::Moc) {
        if pending {
            out.conds.push(Cond::Status("승인 대기".into()));
        } else if hits.iter().filter(live).any(|h| h.cat == Cat::Approved) {
            out.conds.push(Cond::Status("승인".into()));
        }
    }
    if out.types.contains(&DocKind::Aop) && hits.iter().filter(live).any(|h| h.cat == Cat::HighRisk) {
        out.conds.push(Cond::Status("고위험".into()));
    }

    // 연결
    let up = hits.iter().any(|h| h.cat == Cat::Up);
    let down = hits.iter().any(|h| h.cat == Cat::Down);
    let near = hits.iter().any(|h| h.cat == Cat::Near);
    if up || down || near {
        if hop_count.is_none() && glued {
            // 붙여 쓰는 언어: 上流1ホップ · 上游1跳 — 홉 낱말 바로 앞 숫자
            let text: Vec<char> = toks.iter().flat_map(|t| fold(&t.text).chars().collect::<Vec<_>>()).collect();
            for h in hits.iter().filter(|h| h.cat == Cat::Hop) {
                let digits: String = text[..h.c0].iter().rev().take_while(|c| c.is_ascii_digit()).collect::<Vec<_>>().into_iter().rev().collect();
                if let Ok(k) = digits.parse::<usize>() {
                    hop_count = Some(k);
                }
            }
        }
        let dir = if up && !down { Some(true) } else if down && !up { Some(false) } else { None };
        out.relation = Some((dir, hop_count));
    }

    // 머리 낱말
    let content = |i: usize| -> bool {
        !toks[i].function && !out.closed[i] && !has_digit(&toks[i].text) && !quoted.get(i).copied().unwrap_or(false) && !toks[i].text.is_empty() && !lex.is_command(&fold(&toks[i].text))
    };
    let modifier = |i: usize| cat_at(i, &|c| matches!(c, Cat::Quality(_) | Cat::Level(_) | Cat::Pending | Cat::Approved | Cat::Building { .. } | Cat::Floor { .. } | Cat::Basement | Cat::Outdoor | Cat::Ordinal(_)));
    let head = match lex.head {
        Head::LastContent => (0..n).rev().find(|i| content(*i) && !modifier(*i)),
        Head::FirstNoun => {
            let tagged = toks.iter().any(|t| t.upos.is_some());
            (0..n).find(|i| content(*i) && !modifier(*i) && (!tagged || toks[*i].is_noun() || kind_tok(*i) || cat_at(*i, &|c| *c == Cat::Generic)))
        }
        Head::FirstPhraseEnd => {
            let end = (0..n).find(|i| toks[*i].function && !lex.is_command(&fold(&toks[*i].text))).unwrap_or(n);
            (0..end).rev().find(|i| content(*i) && !modifier(*i)).or_else(|| (0..n).find(|i| content(*i) && !modifier(*i)))
        }
    };
    if let Some(i) = head {
        let kindish = kind_tok(i) || cat_at(i, &|c| *c == Cat::Generic);
        out.head = Some((kindish, toks[i].text.clone()));
    }
    out.hits = hits;
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::search::analyze::analyze;

    fn cats(q: &str) -> Vec<(String, Cat)> {
        let a = analyze(q, |_| None);
        let lexes: Vec<&Lex> = std::iter::once(a.lang).chain(a.also.iter().copied()).filter_map(lex).collect();
        match_query(&a.tokens, &lexes).into_iter().map(|h| (h.entry, h.cat)).collect()
    }

    fn has(q: &str, f: impl Fn(&Cat) -> bool) -> bool {
        cats(q).iter().any(|(_, c)| f(c))
    }

    #[test]
    fn inflections_compounds_and_longest_match() {
        // 굴절: 복수 · 격 어미
        assert!(has("válvulas", |c| *c == Cat::Kind("valve".into())));
        assert!(has("čerpadla v suterénu", |c| *c == Cat::Kind("pump".into())));
        // 꼬리가 긴 다른 낱말은 굴절이 아님: ventilátor(팬) 은 ventil(밸브)이 아니고, 스페인어 planta(층) 는 plano(도면)가 아님
        assert!(!has("kouřový ventilátor", |c| *c == Cat::Kind("valve".into())));
        assert!(has("kouřový ventilátor", |c| *c == Cat::Kind("fan".into())));
        assert!(!has("válvulas del edificio 103 en la planta 2", |c| *c == Cat::Type(DocKind::Drawing)));
        // 독일어 합성어: 앞 · 뒤 조각 둘 다 (Lager·schwingung → 베어링 + 진동), 'erhitzer' 는 'über' 바로 뒤라 아님
        let c = cats("Lagerschwingung");
        assert!(c.iter().any(|(_, x)| *x == Cat::Kind("bearing".into())) && c.iter().any(|(_, x)| *x == Cat::Measure("진동")), "{c:?}");
        assert!(has("Speisewasserpumpen", |c| *c == Cat::Kind("pump".into())));
        assert!(!has("Überhitzer", |c| *c == Cat::Kind("heater".into())));
        // 긴 것이 이김: mehr als(비교어) ⊃ mehr, 不正常(비정상) ⊃ 不(부정)
        assert!(has("über 600 Grad", |c| *c == Cat::CmpBefore(Op::Gt)));
        assert!(!has("不正常的泵", |c| *c == Cat::NegBefore));
    }

    #[test]
    fn glued_single_characters_need_context() {
        // 高压 의 高 는 '높음' 이 아니고, 压力高的 의 高 는 높음
        assert!(!has("高压汽轮机", |c| *c == Cat::Quality(Quality::High)));
        assert!(has("压力高的泵", |c| *c == Cat::Quality(Quality::High)));
        // 동 · 층 한 글자는 번호 뒤에서만: 103号楼 는 동, 2层 은 층, '号楼' 의 楼 는 층이 아님
        let c = cats("103号楼2层的阀门");
        assert!(c.iter().any(|(_, x)| *x == Cat::Building { before: false }), "{c:?}");
        assert_eq!(c.iter().filter(|(_, x)| matches!(x, Cat::Floor { .. })).count(), 1, "{c:?}");
        // 아랍어 접어: المضخات → مضخات (펌프)
        assert!(has("المضخات في القبو", |c| *c == Cat::Kind("pump".into())));
        assert!(has("المضخات في القبو", |c| *c == Cat::Basement));
        // 접어를 뗀 짧은 꼴이 더 긴 사전 낱말의 앞부분일 뿐이면 굴절이 아님: بخار(증기) → خار 는 خارجي(옥외)가 아님
        assert!(!has("ضغط بخار رئيسي", |c| *c == Cat::Outdoor));
        assert!(has("ضغط بخار رئيسي", |c| *c == Cat::Measure("압력")));
    }
}
