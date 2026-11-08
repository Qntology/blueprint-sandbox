//! NMS 조각 접지 (logis-center `parse_commerce_query` Stage-1 의 슬라이딩 창 → 임베딩 채점 → NMS 배틀을 원장 개념에 옮김).
//!
//! 1. 질의 낱말 중 내용어(기능어 · 숫자 · 식별자 · 비교어 · 부정어가 아닌 것)로 시작하고 끝나는 1 ~ 5 낱말 창을 모두 만듭니다.
//! 2. 창마다 임베딩하고 개념 뱅크(설비 이름 · 센서 · 설비 종류 · 계측 · 공간 · 문서 유형 · 질적 상태, 한국어 · 영어 구)와 코사인을 잽니다.
//!    개념 점수는 그 개념 구들 중 최댓값입니다.
//! 3. 놀람(surprisal) 문턱: 최고 개념 점수가 뱅크 전체 분포에서 얼마나 튀는지(z = (최고 − 평균) / 표준편차)를 봅니다.
//!    이 모델의 코사인은 무관한 쌍도 0.7 ~ 0.8 이라 절댓값 문턱이 아니라 분포 대비로 봐야 합니다
//!    (logis-center SURPRISAL GATE 와 같은 이유: "무작위 기대치를 넘지 못하면 라우팅하지 않음"). 2위 개념과의 차이도 봅니다.
//! 4. 개체(설비 · 센서)를 가리키는 창만 남깁니다. 일반 개념(종류 · 계측 · 상태 · 유형 · 공간)은 뱅크에서 경쟁자로만 씁니다:
//!    평가 질의로 후보를 모두 찍어 보니 일반 개념에 붙은 창은 대부분 틀렸고('지하' → 낮음, '배출' → 출력, '경보' → 사건),
//!    맞는 것은 규칙 낱말 목록이 이미 잡는 낱말이었습니다. 개체에 붙은 창은 대부분 맞았습니다('id fan' → 유인통풍기 A).
//! 5. NMS 배틀: 개체 후보를 코사인 → 길이 순으로 세우고, 이미 이긴 창과 겹치면 탈락시킵니다. z 로 세우면 짧은 창이
//!    분포상 더 튀어서 'main feedwater'(주급수펌프 A, z 3.79)가 'main feedwater flow control valve'(유량조절밸브, 코사인 0.988, z 3.68)를 이겼습니다.
//!
//! 이긴 조각은 조건을 직접 걸지 않습니다. 순위 경로(개념 경로)와 관계 질의의 기준 설비로만 씁니다 —
//! 임베딩이 틀려도 정답을 지우지 않게 (logis-center: "조건이 정답을 지우면 안 됨").

use std::sync::Arc;

use anyhow::Result;
use serde::Serialize;

use super::analyze::QueryToken;
use super::lexicon::{self, names};
use super::{DocKind, Quality, KIND_WORDS, MEASURE_WORDS, QUALITY_WORDS, TYPE_WORDS};
use crate::ai::embed::{cosine, CachedEmbedder};
use crate::lattice::Lattice;

/// 개념이 가리키는 것
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "family", content = "value", rename_all = "snake_case")]
pub enum Target {
    Equipment(String),
    /// (태그, 설비 코드)
    Sensor(String, String),
    Kind(Vec<String>),
    Measure(String),
    Sector(String),
    Type(DocKind),
    Quality(Quality),
}

impl Target {
    pub fn family(&self) -> &'static str {
        match self {
            Target::Equipment(_) => "설비",
            Target::Sensor(..) => "센서",
            Target::Kind(_) => "설비 종류",
            Target::Measure(_) => "계측",
            Target::Sector(_) => "공간",
            Target::Type(_) => "유형",
            Target::Quality(_) => "상태",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Concept {
    pub label: String,
    pub target: Target,
    pub phrases: Vec<String>,
}

/// 원장에서 만든 개념 뱅크 (구 벡터는 임베딩 캐시에 있음)
#[derive(Debug, Clone)]
pub struct ConceptBank {
    pub concepts: Vec<Concept>,
    /// (개념 번호, 구 벡터)
    phrases: Vec<(usize, Arc<Vec<f32>>)>,
}

fn push_unique(v: &mut Vec<String>, s: impl Into<String>) {
    let s = s.into();
    let t = s.trim().to_string();
    if !t.is_empty() && !v.iter().any(|x| x.eq_ignore_ascii_case(&t)) {
        v.push(t);
    }
}

impl ConceptBank {
    pub fn build(db: &Lattice, emb: &CachedEmbedder) -> Result<Self> {
        let n = names();
        let mut concepts: Vec<Concept> = Vec::new();
        for e in db.equipment_all() {
            let mut phrases = vec![e.name.clone()];
            if let Some(en) = n.en(&e.name) {
                push_unique(&mut phrases, en);
            }
            concepts.push(Concept { label: e.name.clone(), target: Target::Equipment(e.kks.clone()), phrases });
        }
        for s in db.sensors() {
            let Some(e) = db.equipment(&s.kks) else { continue };
            let mut phrases = vec![format!("{} {}", e.name, s.label)];
            if let (Some(a), Some(b)) = (n.en(&e.name), n.en(&s.label)) {
                push_unique(&mut phrases, format!("{a} {b}"));
            }
            concepts.push(Concept { label: format!("{} {}", e.name, s.label), target: Target::Sensor(s.tag.clone(), e.kks.clone()), phrases });
        }
        // 원장에 있는 설비 종류만 (없는 종류는 조건이 될 수 없음)
        let kinds: Vec<&str> = db.equipment_all().iter().map(|e| e.kind.as_str()).collect();
        let mut kind_groups: Vec<(Vec<String>, Vec<String>)> = Vec::new();
        for (word, ks) in KIND_WORDS.iter().map(|(w, k)| (*w, *k)).chain(lexicon::KIND_WORDS_EN.iter().map(|(w, k)| (*w, *k))) {
            if !ks.iter().any(|k| kinds.contains(k)) {
                continue;
            }
            let key: Vec<String> = ks.iter().map(|x| x.to_string()).collect();
            match kind_groups.iter_mut().find(|(k, _)| *k == key) {
                Some((_, p)) => push_unique(p, word),
                None => kind_groups.push((key, vec![word.to_string()])),
            }
        }
        for (ks, phrases) in kind_groups {
            concepts.push(Concept { label: phrases[0].clone(), target: Target::Kind(ks), phrases });
        }
        for (m, _) in MEASURE_WORDS {
            let mut phrases = vec![m.to_string()];
            for (en, ko) in lexicon::MEASURE_WORDS_EN {
                if ko == m {
                    push_unique(&mut phrases, en);
                }
            }
            concepts.push(Concept { label: m.to_string(), target: Target::Measure(m.to_string()), phrases });
        }
        for s in db.sectors() {
            let mut phrases = vec![s.label.clone(), format!("{} {}", s.building, s.label)];
            if let Some(en) = n.en(&s.label) {
                push_unique(&mut phrases, en);
            }
            push_unique(&mut phrases, format!("building {}", s.building.trim_end_matches('동')));
            concepts.push(Concept { label: format!("{} {}", s.building, s.label), target: Target::Sector(s.key.clone()), phrases });
        }
        // 설비 자체를 가리키는 말 (없으면 'equipment' 가 가장 가까운 유형인 계측에 붙음)
        concepts.push(Concept {
            label: DocKind::Equipment.label().to_string(),
            target: Target::Type(DocKind::Equipment),
            phrases: ["설비", "장비", "기기", "equipment", "device", "asset"].iter().map(|x| x.to_string()).collect(),
        });
        for (kind, ko) in TYPE_WORDS {
            let mut phrases: Vec<String> = Vec::new();
            for w in ko.iter().filter(|w| !w.ends_with('-')) {
                push_unique(&mut phrases, *w);
            }
            if let Some((_, en)) = lexicon::TYPE_WORDS_EN.iter().find(|(k, _)| *k == kind) {
                for w in en.iter().filter(|w| !w.ends_with('-')) {
                    push_unique(&mut phrases, *w);
                }
            }
            concepts.push(Concept { label: kind.label().to_string(), target: Target::Type(kind), phrases });
        }
        let mut qualities: Vec<(Quality, Vec<String>)> = Vec::new();
        for (w, q) in QUALITY_WORDS.iter().map(|(w, q)| (*w, *q)).chain(lexicon::QUALITY_WORDS_EN.iter().map(|(w, q)| (*w, *q))) {
            if w.chars().count() < 2 {
                continue;
            }
            match qualities.iter_mut().find(|(k, _)| *k == q) {
                Some((_, p)) => push_unique(p, w),
                None => qualities.push((q, vec![w.to_string()])),
            }
        }
        for (q, phrases) in qualities {
            concepts.push(Concept { label: q.label().to_string(), target: Target::Quality(q), phrases });
        }
        let mut phrases = Vec::new();
        for (i, c) in concepts.iter().enumerate() {
            for p in &c.phrases {
                phrases.push((i, emb.embed(p)?));
            }
        }
        Ok(Self { concepts, phrases })
    }

    /// 개념마다 최고 코사인
    pub fn scores(&self, v: &[f32]) -> Vec<f32> {
        let mut best = vec![f32::MIN; self.concepts.len()];
        for (i, p) in &self.phrases {
            let c = cosine(v, p);
            if c > best[*i] {
                best[*i] = c;
            }
        }
        best
    }
}

/// NMS 에서 이긴 조각 하나
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Segment {
    /// 질의 낱말 자리 [start, end)
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub label: String,
    pub target: Target,
    pub cos: f32,
    /// 뱅크 분포 대비 놀람 (z)
    pub z: f32,
    /// 2위 개념과의 코사인 차이
    pub margin: f32,
}

/// 놀람 문턱. 뱅크(100여 개념)의 최댓값 기대치(정규 분포 n 개의 기대 최댓값 ≈ 2.5)보다 높게 둡니다.
pub const Z_GATE: f32 = 3.0;
/// 2위 개념과 이만큼은 떨어져야 합니다 (이름이 비슷한 두 설비 사이에서 고르지 않게)
pub const MARGIN_GATE: f32 = 0.015;
/// 한 낱말 창은 이만큼 가까워야 개체로 봅니다 ('steam' → 주증기밸브 0.875 를 막음). 두 낱말 이상은 뜻풀이라 문턱 없이 z 만 봅니다.
pub const SINGLE_WORD_COS: f32 = 0.90;
/// 창 최대 낱말 수
pub const MAX_SPAN: usize = 5;
/// 앞에서부터 이만큼의 낱말만 창으로 봅니다 (긴 붙여넣기 질의에서 창이 수백 개로 늘지 않게, 창 하나 임베딩 수 ms)
pub const MAX_TOKENS: usize = 16;

/// 후보 하나 (기록용)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Candidate {
    pub text: String,
    pub label: String,
    pub cos: f32,
    pub z: f32,
    pub margin: f32,
    pub passed: bool,
}

/// `usable[i]`: 창의 처음 · 끝이 될 수 있는 낱말 (내용어). 반환: (이긴 조각, 모든 후보)
pub fn ground(tokens: &[QueryToken], usable: &[bool], bank: &ConceptBank, emb: &CachedEmbedder) -> Result<(Vec<Segment>, Vec<Candidate>)> {
    let mut spans: Vec<(usize, usize, String)> = Vec::new();
    let limit = tokens.len().min(MAX_TOKENS);
    for s in 0..limit {
        if !usable[s] {
            continue;
        }
        for e in s + 1..=(s + MAX_SPAN).min(limit) {
            if !usable[e - 1] {
                continue;
            }
            let text = tokens[s..e].iter().map(|t| t.text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
            spans.push((s, e, text));
        }
    }
    let mut found: Vec<Segment> = Vec::new();
    let mut log = Vec::new();
    for (s, e, text) in spans {
        let v = emb.embed(&text)?;
        let scores = bank.scores(&v);
        let n = scores.len() as f32;
        let mean = scores.iter().sum::<f32>() / n;
        let std = (scores.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / n).sqrt().max(1e-6);
        let mut order: Vec<usize> = (0..scores.len()).collect();
        order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
        let best = order[0];
        // 2위는 다른 대상을 가리키는 개념 중에서 (같은 설비의 센서는 경쟁자로 치지 않음)
        let rival = order.iter().skip(1).find(|i| !related(&bank.concepts[best].target, &bank.concepts[**i].target)).map(|i| scores[*i]).unwrap_or(mean);
        let z = (scores[best] - mean) / std;
        let margin = scores[best] - rival;
        let target = &bank.concepts[best].target;
        let entity = matches!(target, Target::Equipment(_) | Target::Sensor(..));
        let words = tokens[s..e].iter().filter(|t| !t.function).count();
        let passed = entity && z >= Z_GATE && margin >= MARGIN_GATE && (words >= 2 || scores[best] >= SINGLE_WORD_COS);
        log.push(Candidate { text: text.clone(), label: bank.concepts[best].label.clone(), cos: scores[best], z, margin, passed });
        if passed {
            found.push(Segment { start: s, end: e, text, label: bank.concepts[best].label.clone(), target: bank.concepts[best].target.clone(), cos: scores[best], z, margin });
        }
    }
    // NMS 배틀: 코사인 → 길이. 겹치면 탈락
    found.sort_by(|a, b| b.cos.total_cmp(&a.cos).then((b.end - b.start).cmp(&(a.end - a.start))));
    let mut winners: Vec<Segment> = Vec::new();
    for seg in found {
        if winners.iter().all(|w| seg.end <= w.start || seg.start >= w.end) {
            winners.push(seg);
        }
    }
    winners.sort_by_key(|w| w.start);
    Ok((winners, log))
}

/// 같은 대상의 다른 꼴인가 (설비 ↔ 그 설비의 센서, 같은 종류)
fn related(a: &Target, b: &Target) -> bool {
    match (a, b) {
        (Target::Equipment(x), Target::Sensor(_, k)) | (Target::Sensor(_, k), Target::Equipment(x)) => x == k,
        _ => a == b,
    }
}
