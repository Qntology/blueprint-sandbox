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
    pub stanza: HashMap<String, StanzaLang>,
    pub embedder: Option<CachedEmbedder>,
    /// 읽은 것 · 못 읽은 것 (화면 · 기록용)
    pub notes: Vec<String>,
    banks: Mutex<HashMap<String, Arc<ConceptBank>>>,
}

impl SearchModels {
    /// 모델 없이 (규칙 · 글자 경로만)
    pub fn none() -> Self {
        Self::default()
    }

    /// `stanza_dir/<lang>` 과 `embed_dir` 에서 읽습니다. 없거나 읽을 수 없으면 그 모델 없이 갑니다 (기록에 이유).
    pub fn load(stanza_dir: Option<&Path>, embed_dir: Option<&Path>) -> Self {
        let mut me = Self::default();
        if let Some(dir) = stanza_dir {
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
                        me.stanza.insert(lang.to_string(), s);
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

    pub fn stanza_for(&self, lang: &str) -> Option<&StanzaLang> {
        self.stanza.get(lang)
    }

    pub fn has_any(&self) -> bool {
        !self.stanza.is_empty() || self.embedder.is_some()
    }

    /// 원장(도메인)마다 개념 뱅크 하나 (처음 부를 때 만들고 둡니다)
    pub fn bank(&self, db: &Lattice) -> Option<Arc<ConceptBank>> {
        let emb = self.embedder.as_ref()?;
        let key = format!("{:?}", db.domain());
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
        let mut langs: Vec<&str> = self.stanza.keys().map(String::as_str).collect();
        langs.sort_unstable();
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
