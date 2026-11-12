//! 검색이 쓰는 모델 묶음 (모두 선택). 없는 것은 건너뛰고 규칙 · 글자 경로만으로 찾습니다.
//!
//! | 모델 | 폴더 | 하는 일 |
//! | --- | --- | --- |
//! | Stanza ko · en (ONNX, tract) | `<models>/stanza/{ko,en}` | 품사 · 원형 → 조사 · 어미를 뗀 줄기, 복수 → 단수, 기능어 표시 |
//! | granite-embedding-97m-multilingual-r2 (candle) | `<models>/embed` | 질의 벡터 경로 · 유형 제한 벡터 경로 · NMS 조각 접지 |
//!
//! 모두 CPU 에서 돕니다. 처음 검색 때 검색 표 · 개념 뱅크를 임베딩해 캐시에 두고, 그 뒤 질의마다 낱말 창 몇 개만 임베딩합니다.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use super::nms::ConceptBank;
use crate::ai::embed::CachedEmbedder;
use crate::lattice::Lattice;
use crate::nlp::StanzaLang;

#[derive(Debug, Default)]
pub struct SearchModels {
    /// 시작 때 읽은 Stanza (한국어 · 영어)
    pub stanza: HashMap<String, Arc<StanzaLang>>,
    pub embedder: Option<CachedEmbedder>,
    /// 읽은 것 · 못 읽은 것 (화면 · 기록용)
    pub notes: Vec<String>,
    banks: Mutex<HashMap<String, Arc<ConceptBank>>>,
    /// 나머지 언어 Stanza 를 찾을 폴더 (`<models>/stanza`)
    stanza_dir: Option<std::path::PathBuf>,
    /// 처음 질의 때 읽은 나머지 언어 (최근 순, [`EXTRA_LANGS`] 개까지 · 못 읽은 언어는 None 으로 기억)
    extra: Mutex<Vec<(String, Option<Arc<StanzaLang>>)>>,
}

/// 한국어 · 영어 밖 Stanza 를 메모리에 둘 언어 수. 언어 하나가 품사 · 원형 모델로 수백 MB(CPU RAM)라 최근 둘만 둡니다.
/// 같은 언어로 이어 찾으면 다시 읽지 않고, 셋째 언어가 오면 가장 오래 안 쓴 언어를 내립니다 (logis-center CACHE_LANGS 와 같은 생각).
pub const EXTRA_LANGS: usize = 2;

impl SearchModels {
    /// 모델 없이 (규칙 · 글자 경로만)
    pub fn none() -> Self {
        Self::default()
    }

    /// `stanza_dir/<lang>` 과 `embed_dir` 에서 읽습니다. 없거나 읽을 수 없으면 그 모델 없이 갑니다 (기록에 이유).
    pub fn load(stanza_dir: Option<&Path>, embed_dir: Option<&Path>) -> Self {
        let mut me = Self { stanza_dir: stanza_dir.map(Path::to_path_buf), ..Self::default() };
        if let Some(dir) = stanza_dir {
            let others: Vec<&str> = super::lang::LANGS.iter().copied().filter(|l| !matches!(*l, "ko" | "en") && dir.join(l).join("vocab.json").exists()).collect();
            if !others.is_empty() {
                me.notes.push(format!("Stanza {} 는 처음 질의 때 읽음", others.join(" · ")));
            }
            for lang in ["ko", "en"] {
                let d = dir.join(lang);
                if !d.join("vocab.json").exists() {
                    me.notes.push(format!("Stanza {lang} 없음 ({})", d.display()));
                    continue;
                }
                let t = std::time::Instant::now();
                match StanzaLang::load(&d) {
                    Ok(s) => {
                        me.notes.push(format!("Stanza {lang} 읽음 ({:.1} s)", t.elapsed().as_secs_f32()));
                        me.stanza.insert(lang.to_string(), Arc::new(s));
                    }
                    Err(e) => me.notes.push(format!("Stanza {lang} 을 쓸 수 없습니다: {e:#}")),
                }
            }
        }
        if let Some(dir) = embed_dir {
            if dir.join("model.safetensors").exists() {
                let t = std::time::Instant::now();
                match CachedEmbedder::load(dir) {
                    Ok(e) => {
                        me.notes.push(format!("임베딩 모델 읽음 ({:.1} s)", t.elapsed().as_secs_f32()));
                        me.embedder = Some(e);
                    }
                    Err(e) => me.notes.push(format!("임베딩 모델을 쓸 수 없습니다: {e:#}")),
                }
            } else {
                me.notes.push(format!("임베딩 모델 없음 ({})", dir.display()));
            }
        }
        me
    }

    /// 모델 폴더(`blueprint-models`)의 `stanza` · `embed`
    pub fn load_default() -> Self {
        let stanza = crate::ai::model_store::model_dir("stanza");
        let embed = crate::ai::model_store::model_dir("embed");
        Self::load(Some(&stanza), Some(&embed))
    }

    /// 그 언어 Stanza. 한국어 · 영어는 시작 때 읽은 것, 나머지는 처음 부를 때 `<models>/stanza/<lang>` 에서 읽습니다.
    pub fn stanza_for(&self, lang: &str) -> Option<Arc<StanzaLang>> {
        if let Some(s) = self.stanza.get(lang) {
            return Some(s.clone());
        }
        let dir = self.stanza_dir.as_ref()?.join(lang);
        let mut extra = self.extra.lock().ok()?;
        if let Some(p) = extra.iter().position(|(l, _)| l == lang) {
            let hit = extra.remove(p);
            let out = hit.1.clone();
            extra.insert(0, hit);
            return out;
        }
        if !dir.join("vocab.json").exists() || matches!(lang, "ko" | "en") {
            return None;
        }
        let loaded = StanzaLang::load(&dir).ok().map(Arc::new);
        extra.insert(0, (lang.to_string(), loaded.clone()));
        // 못 읽은 언어(None)는 자리를 차지하지 않게 뒤로, 읽은 언어는 EXTRA_LANGS 개까지
        let cap = std::env::var("BLUEPRINT_STANZA_LANGS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(EXTRA_LANGS).max(1);
        let mut kept = 0;
        extra.retain(|(_, m)| {
            if m.is_some() {
                kept += 1;
                kept <= cap
            } else {
                true
            }
        });
        loaded
    }

    /// 쓸 수 있는 Stanza 언어 (읽은 것 + 폴더에 있어 처음 질의 때 읽을 것)
    pub fn stanza_langs(&self) -> Vec<String> {
        let mut v: Vec<String> = self.stanza.keys().cloned().collect();
        if let Some(dir) = &self.stanza_dir {
            for l in super::lang::LANGS {
                if !v.iter().any(|x| x == l) && dir.join(l).join("vocab.json").exists() {
                    v.push(l.to_string());
                }
            }
        }
        v.sort();
        v
    }

    pub fn has_any(&self) -> bool {
        !self.stanza.is_empty() || self.embedder.is_some()
    }

    /// 원장마다 개념 뱅크 하나 (처음 부를 때 만들고 둡니다). 열쇠는 도메인 + 설비 이름들: 같은 도메인이라도 원장 언어가 다르면
    /// (평가의 ko · en · ja · de 원장) 다른 뱅크입니다 — 도메인만 열쇠로 두었을 때 먼저 만든 독일어 원장 뱅크를 한국어 원장이 썼습니다.
    pub fn bank(&self, db: &Lattice) -> Option<Arc<ConceptBank>> {
        let emb = self.embedder.as_ref()?;
        let names: Vec<&str> = db.equipment_all().iter().map(|e| e.name.as_str()).collect();
        let key = format!("{:?}:{}", db.domain(), names.join("|"));
        if let Some(b) = self.banks.lock().ok()?.get(&key) {
            return Some(b.clone());
        }
        let bank = Arc::new(ConceptBank::build(db, emb).ok()?);
        self.banks.lock().ok()?.insert(key, bank.clone());
        Some(bank)
    }

    /// 한 줄 요약 ("Stanza ko · en · 임베딩")
    pub fn summary(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        let langs = self.stanza_langs();
        if !langs.is_empty() {
            parts.push(format!("Stanza {}", langs.join(" · ")));
        }
        if self.embedder.is_some() {
            parts.push("임베딩 granite-97m".into());
        }
        if parts.is_empty() {
            "규칙만".into()
        } else {
            parts.join(" · ")
        }
    }
}
