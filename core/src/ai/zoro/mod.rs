//! zoro(Zoro-1) 백본: 텍스트 순전파를 직접 구현했습니다 (다른 구현을 가져오지 않았습니다).
//!
//! - `config`: GGUF 메타데이터에서 읽는 구조 값
//! - `ops`: 게이트 델타 규칙 선형 주의 (RMSNorm · RoPE · 인과 주의는 `ai::attention`)
//! - `model`: 층 순전파와 단계별 가중치 사용 (`ai::residency`)
//!
//! [`ZoroBackbone`] 이 `ai::pointer::Backbone` 을 구현하고, [`open_decision_model`] 이 `jev_head` 와 묶어 `DecisionModel` 을 만듭니다.
//! 글쓰기는 [`ZoroBackbone::write`] 입니다. 판단과 같은 몸통에 임베딩 표를 출력 투영으로 붙입니다 (명세의 듀얼 헤드).

pub mod config;
pub mod model;
pub mod ops;
#[cfg(test)]
pub(crate) mod reference;
pub mod vision;

use std::path::Path;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde_json::json;

use self::model::{rows_to_host, GenerateOptions, StreamMode, ZoroModel};
use super::gguf_model::{load_active, load_active_on};
use super::residency::Requant;
use super::jev_head::{JevHead, HIDDEN};
use super::pointer::{Backbone, PointerHidden, PointerModel, PointerPrompt};
use super::residency::SharedResidency;
use super::speculative::{BlockDiffusion, Speculation};
use super::tokenizer::{token_covering, ModelTokenizer};
use super::trace;
use super::weights::WeightFile;
use crate::drafting::{through_last_sentence, TextWriter};

pub use config::{LayerKind, ZoroConfig};

/// 글쓰기의 추측을 `마스크:탐색:폭` 으로 정하는 환경 변수. `0:0:0` 이면 한 토큰씩 읽습니다.
pub const SPECULATION_ENV: &str = "BLUEPRINT_ZORO_SPECULATION";
/// 끝 토큰 (`<|im_end|>`, `<|endoftext|>`)
const STOP_TOKENS: [&str; 2] = ["<|im_end|>", "<|endoftext|>"];
/// 생각 구간을 비워 바로 답하게 하는 어시스턴트 머리말 (모델의 채팅 템플릿에서 `enable_thinking=false` 일 때의 형태)
const ANSWER_START: &str = "<|im_start|>assistant\n<think>\n\n</think>\n\n";

/// `마스크:탐색:폭` 형식의 추측 설정을 읽습니다.
pub fn parse_speculation(text: &str) -> Result<Speculation> {
    let mut parts = text.trim().split(':').map(str::parse::<usize>);
    let mut next = || parts.next().and_then(|v| v.ok());
    let parsed = (|| Some(Speculation { masks: next()?, lookup: next()?, width: next()? }))();
    parsed.with_context(|| format!("{SPECULATION_ENV}={text:?}: `마스크:탐색:폭` 형식의 정수여야 합니다"))
}

/// 판단 · 글쓰기 프롬프트를 남길 폴더 (`pointer.jsonl` · `writer.jsonl`). 양자화 배분을 같은 입력으로 다시 잴 때 씁니다
/// (`examples/quant_screen.rs zoro`). 합성 데이터 측정 전용이라 앱은 켜지 않습니다.
pub const DUMP_ENV: &str = "BLUEPRINT_ZORO_DUMP";

fn dump(kind: &str, row: serde_json::Value) {
    let Some(dir) = std::env::var_os(DUMP_ENV).filter(|v| !v.is_empty()) else { return };
    let path = std::path::PathBuf::from(dir).join(format!("{kind}.jsonl"));
    let line = format!("{row}\n");
    let write = || -> std::io::Result<()> {
        if let Some(p) = path.parent() {
            std::fs::create_dir_all(p)?;
        }
        use std::io::Write;
        std::fs::OpenOptions::new().create(true).append(true).open(&path)?.write_all(line.as_bytes())
    };
    if let Err(e) = write() {
        eprintln!("[BLUEPRINT-ZORO] {} 에 남기지 못했습니다: {e}", path.display());
    }
}

/// 모델이 쓴 글
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub text: String,
    pub tokens: usize,
    /// 끝 토큰까지 썼는가 (아니면 길이 제한이나 반복으로 멈춤)
    pub finished: bool,
    pub min_confidence: f32,
    pub prefill_ms: f64,
    pub decode_ms: f64,
    /// 순전파 횟수
    pub reads: usize,
    /// 검증에 넘긴 후보 수와 그중 채택한 수
    pub drafted: usize,
    pub accepted: usize,
}

pub struct ZoroBackbone {
    model: ZoroModel,
    tokenizer: ModelTokenizer,
    /// 글쓰기에 쓰는 추측의 양
    speculation: Speculation,
    /// 생성 하나(출력 투영 다시 자르기 → 생성)를 통째로 묶는 잠금. 글쓰기(writer)와 문서 판독(reader)이 이 백본 하나를 나눠 쓰는데
    /// (`SharedBackbone`), 투영은 모델 안의 공유 상태라 둘이 겹치면 한쪽이 자른 행으로 다른 쪽이 생성하고, 상주 관리자가 올린 행과
    /// 행 번호표가 어긋날 수 있었습니다. GPU 는 어차피 한 번에 생성 하나를 돌립니다.
    run: std::sync::Mutex<()>,
}

impl ZoroBackbone {
    /// `model_dir/active.json` 이 가리키는 GGUF 와 `model_dir/tokenizer.json` 을 엽니다. 가중치는 여기서 올리지 않고,
    /// 첫 판단에서 필요한 묶음만 올라옵니다. 장치가 K 형식을 장치에서 곱하고 피라미드 파일이 있으면 그 파일입니다 (`ai::pyramid`).
    pub fn open(model_dir: &Path, residency: SharedResidency) -> Result<Self> {
        Self::open_with(model_dir, residency, StreamMode::from_env()?)
    }

    pub fn open_with(model_dir: &Path, residency: SharedResidency, mode: StreamMode) -> Result<Self> {
        Self::open_variant(model_dir, residency, mode, false)
    }

    /// `q8_only` 면 피라미드 파일이 있어도 q8 파일을 엽니다 (측정의 기준).
    pub fn open_variant(model_dir: &Path, residency: SharedResidency, mode: StreamMode, q8_only: bool) -> Result<Self> {
        let device = residency.lock().unwrap_or_else(|p| p.into_inner()).device().clone();
        let active = if q8_only { load_active(model_dir)? } else { load_active_on(model_dir, &device)? };
        if active.architecture != "zoro" {
            bail!("zoro 모델 폴더가 아닙니다: {} ({})", model_dir.display(), active.architecture);
        }
        let file = Arc::new(WeightFile::open(&active.path)?);
        let tokenizer = ModelTokenizer::from_file(&model_dir.join("tokenizer.json"))?;
        // GGUF 에 없으면 모델 폴더의 설정을 씁니다 (예전에 변환한 파일).
        let diffusion = BlockDiffusion::resolve(&file, model_dir)?;
        let mut model = ZoroModel::new(file, residency, mode)?.with_diffusion(diffusion)?.with_trim(&tokenizer)?;
        if let Some(path) = &active.mmproj {
            // 매핑만 합니다. 비전 단계가 없으면 이 파일의 텐서는 읽지 않습니다.
            model = model.with_mmproj(Arc::new(WeightFile::open(path)?));
        }
        model.announce();
        let speculation = match std::env::var(SPECULATION_ENV) {
            Ok(v) => parse_speculation(&v)?,
            Err(_) => model.default_speculation(),
        };
        Ok(Self { model, tokenizer, speculation, run: std::sync::Mutex::new(()) })
    }

    fn lock_run(&self) -> std::sync::MutexGuard<'_, ()> {
        self.run.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn model(&self) -> &ZoroModel {
        &self.model
    }

    /// 올릴 때 이 배분으로 다시 양자화합니다 (정밀도 배분 측정용).
    pub fn with_requant(mut self, label: impl Into<String>, requant: Requant) -> Self {
        self.model = self.model.with_requant(label, requant);
        self
    }

    /// 다시 양자화할 때 이 중요도 행렬을 씁니다.
    pub fn with_imatrix(mut self, imatrix: Arc<super::pyramid::Imatrix>) -> Self {
        self.model = self.model.with_imatrix(imatrix);
        self
    }

    /// 올릴 때 양자화하는 텐서의 재료를 이 원본 BF16 에서 읽습니다.
    pub fn with_original(mut self, original: Arc<super::original::Original>) -> Self {
        self.model = self.model.with_original(original);
        self
    }

    /// 이 백본이 연 양자화 (`q8` 또는 `pyramid:<배분>`, 측정 중이면 다시 양자화하는 배분 이름)
    pub fn quant(&self) -> String {
        let spec = self.model.spec();
        if !spec.variant().is_empty() {
            return format!("requant:{}", spec.variant());
        }
        spec.file.meta_str(super::pyramid::META_PLAN).map_or_else(|_| "q8".to_string(), |p| format!("pyramid:{p}"))
    }

    pub fn tokenizer(&self) -> &ModelTokenizer {
        &self.tokenizer
    }

    /// 문서 언어로 출력 투영을 다시 자릅니다 ([`ZoroModel::retrim`]).
    pub fn retrim(&self, langs: crate::ai::trim::Langs) -> Result<bool> {
        let _run = self.lock_run();
        self.model.retrim(&self.tokenizer, langs)
    }

    /// 지금 남긴 출력 투영 행 수
    pub fn head_rows(&self) -> usize {
        self.model.head_rows()
    }

    /// 지시문과 입력으로 글을 씁니다. 가장 확률이 높은 토큰만 고르므로 같은 입력이면 같은 글이 나옵니다.
    pub fn write(&self, system: &str, user: &str, max_tokens: usize) -> Result<Written> {
        self.write_with(system, user, max_tokens, self.speculation)
    }

    /// 추측의 양을 정해서 씁니다. 얼마이든 글은 같고 시간만 달라집니다.
    pub fn write_with(&self, system: &str, user: &str, max_tokens: usize, speculation: Speculation) -> Result<Written> {
        let _run = self.lock_run();
        // 🌟 [DOC LANG] 텍스트 맥락: 쓰기 전에 지시문 · 입력 글로 언어를 정하고 그 언어의 행만 남깁니다 (`BLUEPRINT_TRIM_LANGS=auto`).
        if self.model.trim_auto() {
            self.model.retrim(&self.tokenizer, crate::ai::trim::Langs::of_document(&format!("{system}\n{user}")))?;
        }
        let prompt = format!("<|im_start|>system\n{system}<|im_end|>\n<|im_start|>user\n{user}<|im_end|>\n{ANSWER_START}");
        let ids = self.tokenizer.encode(&prompt)?.ids;
        let stop = STOP_TOKENS.iter().filter_map(|t| self.tokenizer.token_to_id(t)).collect();
        let out = self.model.generate(&ids, &GenerateOptions::new(max_tokens, stop).with_speculation(speculation))?;
        let text = self.tokenizer.decode(&out.tokens)?;
        dump("writer", json!({ "system": system, "user": user, "max_tokens": max_tokens, "prompt_ids": ids, "tokens": out.tokens, "text": text, "finished": out.finished, "quant": self.quant() }));
        if trace::enabled() {
            let shown = if trace::with_text() { text.as_str() } else { "" };
            // 지시문은 고정된 글이라 사건 내용이 없습니다. 입력은 남기지 않습니다.
            let detail = json!({ "system": system, "tokens": out.tokens.len(), "reads": out.reads, "drafted": out.drafted, "accepted": out.accepted, "text": shown });
            trace::record("zoro", "글쓰기", out.prefill_ms + out.decode_ms, detail);
        }
        Ok(Written {
            text,
            tokens: out.tokens.len(),
            finished: out.finished,
            min_confidence: out.confidences.iter().copied().fold(1.0, f32::min),
            prefill_ms: out.prefill_ms,
            decode_ms: out.decode_ms,
            reads: out.reads,
            drafted: out.drafted,
            accepted: out.accepted,
        })
    }
}

/// 이미지 판독 지시문 (다국어 문서 · 도면 조각을 그대로 옮겨 적기)
pub const TRANSCRIBE: &str = "Transcribe all text in this image exactly as it appears, in its original language, keeping line breaks. Output only the text.";

/// 이미지 한 장을 읽은 결과
#[derive(Debug, Clone, PartialEq)]
pub struct ImageReading {
    pub text: String,
    pub tokens: usize,
    pub finished: bool,
    pub min_confidence: f32,
    /// 언어 모델에 넣은 이미지 토큰 수 (2 × 2 병합 뒤)
    pub image_tokens: usize,
    pub vision_ms: f64,
    pub prefill_ms: f64,
    pub decode_ms: f64,
}

impl ZoroBackbone {
    /// 이미지 한 장을 읽고 `instruction` 대로 씁니다 (Zoro-1 비전: 다국어 문서 · 도면 판독, gocr 저신뢰 조각 대체 판독).
    /// 비전 단계(mmproj) → 비전을 내림 → 텍스트 블록 순서이고 (`ZoroModel::generate_with_image`), 판단 프롬프트와 달리 지시문 언어는 자유입니다.
    /// `max_pixels` 는 이미지 토큰 수를 묶는 화소 상한입니다 (`zoro::vision::MAX_PIXELS`).
    pub fn read_image(&self, image: &image::RgbImage, instruction: &str, max_tokens: usize, max_pixels: usize) -> Result<ImageReading> {
        self.read_images(std::slice::from_ref(image), instruction, max_tokens, max_pixels)?.pop().context("결과가 없습니다")
    }

    /// 이미지 여러 장을 같은 지시문으로 읽습니다. 비전 단계 한 번(모두 인코딩) → 비전을 내림 → 텍스트 블록 한 번 올려 차례로 씁니다.
    /// 🌟 [DOC LANG] 이미지 맥락은 언어를 미리 모르므로 `BLUEPRINT_TRIM_LANGS=auto` 면 첫 장을 12 개 언어의 행으로 읽고, 그 글에 라틴 밖
    /// 글자가 있으면 그 언어로 다시 잘라 나머지를 읽습니다 (한 문서의 조각들, [`crate::ai::trim::Langs::decisive`]).
    pub fn read_images(&self, images: &[image::RgbImage], instruction: &str, max_tokens: usize, max_pixels: usize) -> Result<Vec<ImageReading>> {
        let _run = self.lock_run();
        let auto = self.model.trim_auto();
        if auto {
            // 새 문서는 12 개 언어의 행으로 시작합니다 (앞 문서의 언어로 잘린 행을 이어 쓰지 않음, 이미지가 한 장이어도)
            self.model.retrim(&self.tokenizer, crate::ai::trim::Langs::all())?;
        }
        // 모든 이미지를 비전 단계 1회로 인코딩한 뒤 텍스트 블록을 올리고, 첫 장에서 문서 언어가 확정되면
        // 32개 텍스트 블록은 유지한 채 `Group::Head` 만 교체합니다.
        self.read_images_held(images, instruction, max_tokens, max_pixels, auto)
    }

    /// 지금의 출력 투영으로 읽습니다.
    pub fn read_images_now(&self, images: &[image::RgbImage], instruction: &str, max_tokens: usize, max_pixels: usize) -> Result<Vec<ImageReading>> {
        let _run = self.lock_run();
        self.read_images_held(images, instruction, max_tokens, max_pixels, false)
    }

    /// [`Self::read_images_now`] 의 본체 (생성 잠금을 쥔 쪽에서 부릅니다)
    fn read_images_held(&self, images: &[image::RgbImage], instruction: &str, max_tokens: usize, max_pixels: usize, auto_retrim: bool) -> Result<Vec<ImageReading>> {
        let cfg = self.model.vision_config()?;
        let pad = self.tokenizer.token_to_id("<|image_pad|>").context("토크나이저에 <|image_pad|> 가 없습니다")?;
        let head = self.tokenizer.encode("<|im_start|>user\n<|vision_start|>")?.ids;
        let tail = self.tokenizer.encode(&format!("<|vision_end|>{instruction}<|im_end|>\n{ANSWER_START}"))?.ids;
        let mut prepared = Vec::with_capacity(images.len());
        for image in images {
            let p = vision::prepare(image, &cfg, max_pixels)?;
            let slot = model::ImageSlot { at: head.len(), rows: p.grid_h / cfg.merge, cols: p.grid_w / cfg.merge };
            let mut ids = head.clone();
            ids.extend(std::iter::repeat_n(pad, slot.tokens()));
            ids.extend_from_slice(&tail);
            prepared.push((ids, p, slot));
        }
        let stop = STOP_TOKENS.iter().filter_map(|t| self.tokenizer.token_to_id(t)).collect();
        let opts = GenerateOptions::new(max_tokens, stop).with_speculation(self.speculation);
        let items: Vec<(&[u32], &vision::VisionImage, model::ImageSlot)> = prepared.iter().map(|(ids, p, s)| (ids.as_slice(), p, *s)).collect();
        let outs = if auto_retrim && items.len() > 1 {
            self.model.generate_with_images_retrim(&items, &opts, Some(&self.tokenizer), |first| {
                let text = self.tokenizer.decode(&first.tokens)?;
                Ok(crate::ai::trim::Langs::decisive(&text))
            })?
        } else {
            self.model.generate_with_images(&items, &opts)?
        };
        outs.into_iter()
            .zip(&prepared)
            .map(|((out, vision_ms), (_, _, slot))| {
                let text = self.tokenizer.decode(&out.tokens)?;
                if trace::enabled() {
                    let shown = if trace::with_text() { text.as_str() } else { "" };
                    trace::record("zoro", "이미지 판독", vision_ms + out.prefill_ms + out.decode_ms, json!({ "image_tokens": slot.tokens(), "tokens": out.tokens.len(), "vision_ms": vision_ms, "text": shown }));
                }
                Ok(ImageReading {
                    text,
                    tokens: out.tokens.len(),
                    finished: out.finished,
                    min_confidence: out.confidences.iter().copied().fold(1.0, f32::min),
                    image_tokens: slot.tokens(),
                    vision_ms,
                    prefill_ms: out.prefill_ms,
                    decode_ms: out.decode_ms,
                })
            })
            .collect()
    }
}

impl Backbone for ZoroBackbone {
    fn encode(&self, prompt: &PointerPrompt) -> Result<PointerHidden> {
        let enc = self.tokenizer.encode(&prompt.text)?;
        let query = token_covering(&enc.offsets, prompt.query_byte)
            .with_context(|| format!("질의 위치(바이트 {})를 덮는 토큰이 없습니다", prompt.query_byte))?;
        let mut rows = vec![query];
        for (i, &b) in prompt.option_bytes.iter().enumerate() {
            rows.push(
                token_covering(&enc.offsets, b)
                    .with_context(|| format!("선택지 {i} 의 위치(바이트 {b})를 덮는 토큰이 없습니다"))?,
            );
        }
        let hidden = self.model.hidden_states(&enc.ids)?;
        let picked = rows_to_host(&hidden, &rows)?;
        dump("pointer", json!({ "text": prompt.text, "query_byte": prompt.query_byte, "option_bytes": prompt.option_bytes, "quant": self.quant() }));
        if picked.iter().any(|r| r.len() != HIDDEN) {
            bail!("은닉 차원이 {HIDDEN} 이 아닙니다: {}", picked[0].len());
        }
        let mut it = picked.into_iter();
        let query = it.next().expect("질의 행");
        Ok(PointerHidden { query, options: it.flatten().collect() })
    }
}

impl TextWriter for ZoroBackbone {
    /// 길이 제한이나 반복으로 멈춘 글은 마지막으로 완성된 문장까지만 넘깁니다.
    fn complete(&self, system: &str, user: &str, max_tokens: usize) -> Result<String> {
        let w = self.write(system, user, max_tokens)?;
        Ok(if w.finished { w.text } else { through_last_sentence(&w.text).to_string() })
    }
}

/// 판단과 글쓰기가 함께 쓰는 백본 (명세의 듀얼 헤드)
#[derive(Clone)]
pub struct SharedBackbone(pub Arc<ZoroBackbone>);

impl Backbone for SharedBackbone {
    fn encode(&self, prompt: &PointerPrompt) -> Result<PointerHidden> {
        self.0.encode(prompt)
    }
}

impl TextWriter for SharedBackbone {
    fn complete(&self, system: &str, user: &str, max_tokens: usize) -> Result<String> {
        self.0.complete(system, user, max_tokens)
    }
}

/// 판단 모델과 글쓰기 모델을 한 백본으로 엽니다. 가중치는 쓸 때 단계별로 올라옵니다.
pub fn open_engine_models(model_dir: &Path, residency: SharedResidency) -> Result<(PointerModel, SharedBackbone)> {
    let backbone = SharedBackbone(Arc::new(ZoroBackbone::open(model_dir, residency)?));
    let head = JevHead::from_safetensors(&model_dir.join("jev_head.safetensors"))?;
    Ok((PointerModel::new(Box::new(backbone.clone()), head), backbone))
}

/// zoro 백본과 `jev_head` 로 판단 모델을 만듭니다. `residency` 는 gocr 등 다른 모델과 공유해야 "한 번에 한 모델"이 지켜집니다.
pub fn open_decision_model(model_dir: &Path, residency: SharedResidency) -> Result<PointerModel> {
    let backbone = ZoroBackbone::open(model_dir, residency)?;
    let head = JevHead::from_safetensors(&model_dir.join("jev_head.safetensors"))?;
    Ok(PointerModel::new(Box::new(backbone), head))
}
