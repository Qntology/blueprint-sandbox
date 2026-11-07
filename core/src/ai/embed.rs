//! 문장 임베딩: granite-embedding-97m-multilingual-r2 (ModernBERT 12층 · 384차원 · 다국어). 자연어 검색이 씁니다.
//!
//! logis-center 와 같은 모델 · 같은 풀링(CLS + L2 정규화, "modernbert-cls-v1")이고 순전파는 이 저장소에서 따로 썼습니다.
//! HF transformers `ModernBertModel`(F32 · eager)과 같은 식입니다: LayerNorm(편향 없음) · 3층마다 전역 주의(RoPE θ 150000),
//! 나머지 층은 ±64 창 주의(θ 160000) · GLU(SiLU) MLP · 마지막 LayerNorm. 대조는 `tests` 의 `matches_transformers` (모델이 있을 때).
//!
//! CPU 에서만 돕니다. 질의 하나는 수 ms 이고 검색 표(원장 100여 건)는 처음 한 번만 임베딩해 캐시에 둡니다.
//! GPU 의 "한 번에 한 모델" 상주 관리(`residency`)와 겹치지 않게 하려는 것입니다.

use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Context, Result};
use candle_core::{DType, Device, Tensor, D};
use serde::Deserialize;
use tokenizers::Tokenizer;

/// 한 번에 넣는 최대 토큰 수 (logis-center 와 같은 512)
pub const MAX_TOKENS: usize = 512;

#[derive(Debug, Clone, Deserialize)]
struct Config {
    hidden_size: usize,
    num_attention_heads: usize,
    num_hidden_layers: usize,
    intermediate_size: usize,
    #[serde(default = "default_eps")]
    norm_eps: f64,
    #[serde(default = "default_every")]
    global_attn_every_n_layers: usize,
    #[serde(default = "default_global_theta")]
    global_rope_theta: f64,
    local_rope_theta: Option<f64>,
    #[serde(default = "default_local")]
    local_attention: usize,
    #[serde(default)]
    norm_bias: bool,
    #[serde(default)]
    attention_bias: bool,
    #[serde(default)]
    mlp_bias: bool,
    #[serde(default)]
    hidden_activation: Option<String>,
}

fn default_eps() -> f64 {
    1e-5
}
fn default_every() -> usize {
    3
}
fn default_global_theta() -> f64 {
    160_000.0
}
fn default_local() -> usize {
    128
}

struct Layer {
    attn_norm: Option<Tensor>,
    /// (hidden, 3·hidden) 로 미리 전치
    wqkv: Tensor,
    wo: Tensor,
    mlp_norm: Tensor,
    wi: Tensor,
    mlp_wo: Tensor,
    global: bool,
}

/// granite 임베딩 모델 (CPU · F32 계산, 낱말 임베딩 표만 BF16 그대로)
pub struct TextEmbedder {
    tokenizer: Tokenizer,
    tok_embeddings: Tensor,
    emb_norm: Tensor,
    layers: Vec<Layer>,
    final_norm: Tensor,
    cfg: Config,
    device: Device,
}

impl std::fmt::Debug for TextEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEmbedder").field("layers", &self.layers.len()).field("dim", &self.cfg.hidden_size).finish()
    }
}

fn layer_norm(x: &Tensor, w: &Tensor, eps: f64) -> Result<Tensor> {
    let mean = x.mean_keepdim(D::Minus1)?;
    let xc = x.broadcast_sub(&mean)?;
    let var = xc.sqr()?.mean_keepdim(D::Minus1)?;
    Ok(xc.broadcast_div(&(var + eps)?.sqrt()?)?.broadcast_mul(w)?)
}

fn rotate_half(x: &Tensor) -> Result<Tensor> {
    let half = x.dim(D::Minus1)? / 2;
    let a = x.narrow(D::Minus1, 0, half)?;
    let b = x.narrow(D::Minus1, half, half)?;
    Ok(Tensor::cat(&[&b.neg()?, &a], D::Minus1)?)
}

impl TextEmbedder {
    /// 모델 폴더(`config.json` · `tokenizer.json` · `model.safetensors`)를 읽습니다.
    pub fn load(dir: &Path) -> Result<Self> {
        let cfg: Config = serde_json::from_str(&std::fs::read_to_string(dir.join("config.json")).with_context(|| format!("config.json 이 없습니다: {}", dir.display()))?)?;
        if cfg.norm_bias || cfg.attention_bias || cfg.mlp_bias {
            bail!("편향이 있는 ModernBERT 는 지원하지 않습니다");
        }
        if cfg.hidden_activation.as_deref().is_some_and(|a| a != "silu") {
            bail!("SiLU 가 아닌 활성 함수는 지원하지 않습니다: {:?}", cfg.hidden_activation);
        }
        let mut tokenizer = Tokenizer::from_file(dir.join("tokenizer.json")).map_err(|e| anyhow!("{e}")).context("tokenizer.json")?;
        tokenizer
            .with_truncation(Some(tokenizers::TruncationParams { max_length: MAX_TOKENS, ..Default::default() }))
            .map_err(|e| anyhow!("{e}"))?;
        tokenizer.with_padding(None);
        let device = Device::Cpu;
        let path = dir.join("model.safetensors");
        let mut t = candle_core::safetensors::load(&path, &device).with_context(|| format!("가중치를 읽을 수 없습니다: {}", path.display()))?;
        let mut take = |name: &str| -> Result<Tensor> { t.remove(name).ok_or_else(|| anyhow!("가중치가 없습니다: {name}")) };
        let f32w = |x: Tensor| -> Result<Tensor> { Ok(x.to_dtype(DType::F32)?) };
        let lin = |x: Tensor| -> Result<Tensor> { Ok(x.to_dtype(DType::F32)?.t()?.contiguous()?) };
        let tok_embeddings = take("embeddings.tok_embeddings.weight")?;
        let emb_norm = f32w(take("embeddings.norm.weight")?)?;
        let mut layers = Vec::with_capacity(cfg.num_hidden_layers);
        for i in 0..cfg.num_hidden_layers {
            let p = format!("layers.{i}.");
            layers.push(Layer {
                attn_norm: if i == 0 { None } else { Some(f32w(take(&format!("{p}attn_norm.weight"))?)?) },
                wqkv: lin(take(&format!("{p}attn.Wqkv.weight"))?)?,
                wo: lin(take(&format!("{p}attn.Wo.weight"))?)?,
                mlp_norm: f32w(take(&format!("{p}mlp_norm.weight"))?)?,
                wi: lin(take(&format!("{p}mlp.Wi.weight"))?)?,
                mlp_wo: lin(take(&format!("{p}mlp.Wo.weight"))?)?,
                global: i % cfg.global_attn_every_n_layers == 0,
            });
        }
        let final_norm = f32w(take("final_norm.weight")?)?;
        Ok(Self { tokenizer, tok_embeddings, emb_norm, layers, final_norm, cfg, device })
    }

    pub fn dim(&self) -> usize {
        self.cfg.hidden_size
    }

    /// 토큰 번호 ([CLS] … [SEP], 최대 512)
    pub fn token_ids(&self, text: &str) -> Result<Vec<u32>> {
        Ok(self.tokenizer.encode(text, true).map_err(|e| anyhow!("{e}"))?.get_ids().to_vec())
    }

    fn rope(&self, len: usize, theta: f64) -> Result<(Tensor, Tensor)> {
        let hd = self.cfg.hidden_size / self.cfg.num_attention_heads;
        let inv: Vec<f32> = (0..hd / 2).map(|i| (1.0 / theta.powf((2 * i) as f64 / hd as f64)) as f32).collect();
        let inv = Tensor::from_vec(inv, (1, hd / 2), &self.device)?;
        let pos = Tensor::arange(0u32, len as u32, &self.device)?.to_dtype(DType::F32)?.reshape((len, 1))?;
        let freqs = pos.broadcast_mul(&inv)?;
        let emb = Tensor::cat(&[&freqs, &freqs], D::Minus1)?;
        Ok((emb.cos()?, emb.sin()?))
    }

    /// 한 글의 문장 벡터 (L2 정규화)
    pub fn embed(&self, text: &str) -> Result<Vec<f32>> {
        let ids = self.token_ids(text)?;
        self.embed_ids(&ids)
    }

    pub fn embed_ids(&self, ids: &[u32]) -> Result<Vec<f32>> {
        let cfg = &self.cfg;
        let len = ids.len();
        let heads = cfg.num_attention_heads;
        let hd = cfg.hidden_size / heads;
        let ids_t = Tensor::from_vec(ids.to_vec(), (len,), &self.device)?;
        let mut h = layer_norm(&self.tok_embeddings.index_select(&ids_t, 0)?.to_dtype(DType::F32)?, &self.emb_norm, cfg.norm_eps)?;
        let global_rope = self.rope(len, cfg.global_rope_theta)?;
        let local_rope = self.rope(len, cfg.local_rope_theta.unwrap_or(cfg.global_rope_theta))?;
        // 창 주의: |i - j| ≤ local_attention / 2 만 봅니다 (transformers sliding_window_bidirectional_overlay)
        let half = (cfg.local_attention / 2) as i64;
        let window: Vec<f32> = (0..len as i64).flat_map(|i| (0..len as i64).map(move |j| if (i - j).abs() <= half { 0.0 } else { f32::NEG_INFINITY })).collect();
        let window = Tensor::from_vec(window, (1, len, len), &self.device)?;
        let scale = (hd as f64).powf(-0.5);
        for layer in &self.layers {
            let x = match &layer.attn_norm {
                Some(w) => layer_norm(&h, w, cfg.norm_eps)?,
                None => h.clone(),
            };
            let qkv = x.matmul(&layer.wqkv)?.reshape((len, 3, heads, hd))?;
            let q = qkv.narrow(1, 0, 1)?.squeeze(1)?.transpose(0, 1)?.contiguous()?;
            let k = qkv.narrow(1, 1, 1)?.squeeze(1)?.transpose(0, 1)?.contiguous()?;
            let v = qkv.narrow(1, 2, 1)?.squeeze(1)?.transpose(0, 1)?.contiguous()?;
            let (cos, sin) = if layer.global { &global_rope } else { &local_rope };
            let q = (q.broadcast_mul(cos)? + rotate_half(&q)?.broadcast_mul(sin)?)?;
            let k = (k.broadcast_mul(cos)? + rotate_half(&k)?.broadcast_mul(sin)?)?;
            let mut scores = (q.matmul(&k.t()?)? * scale)?;
            if !layer.global {
                scores = scores.broadcast_add(&window)?;
            }
            let probs = candle_nn::ops::softmax_last_dim(&scores)?;
            let attn = probs.matmul(&v)?.transpose(0, 1)?.contiguous()?.reshape((len, cfg.hidden_size))?;
            h = (h + attn.matmul(&layer.wo)?)?;
            let x = layer_norm(&h, &layer.mlp_norm, cfg.norm_eps)?;
            let wi = x.matmul(&layer.wi)?;
            let input = wi.narrow(D::Minus1, 0, cfg.intermediate_size)?;
            let gate = wi.narrow(D::Minus1, cfg.intermediate_size, cfg.intermediate_size)?;
            let mlp = (candle_nn::ops::silu(&input)? * gate)?.matmul(&layer.mlp_wo)?;
            h = (h + mlp)?;
        }
        let h = layer_norm(&h, &self.final_norm, cfg.norm_eps)?;
        let cls: Vec<f32> = h.get(0)?.to_vec1()?;
        let norm = cls.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-12);
        Ok(cls.into_iter().map(|x| x / norm).collect())
    }
}

/// 같은 글을 다시 임베딩하지 않게 캐시를 둔 임베딩 (검색 표 · 개념 뱅크는 질의마다 바뀌지 않습니다)
pub struct CachedEmbedder {
    pub model: TextEmbedder,
    cache: Mutex<HashMap<String, Arc<Vec<f32>>>>,
}

impl std::fmt::Debug for CachedEmbedder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CachedEmbedder").field("model", &self.model).field("cached", &self.cache.lock().map(|c| c.len()).unwrap_or(0)).finish()
    }
}

impl CachedEmbedder {
    pub fn new(model: TextEmbedder) -> Self {
        Self { model, cache: Mutex::new(HashMap::new()) }
    }

    pub fn load(dir: &Path) -> Result<Self> {
        Ok(Self::new(TextEmbedder::load(dir)?))
    }

    pub fn embed(&self, text: &str) -> Result<Arc<Vec<f32>>> {
        if let Some(v) = self.cache.lock().map_err(|_| anyhow!("캐시 잠금"))?.get(text) {
            return Ok(v.clone());
        }
        let v = Arc::new(self.model.embed(text)?);
        self.cache.lock().map_err(|_| anyhow!("캐시 잠금"))?.insert(text.to_string(), v.clone());
        Ok(v)
    }

    pub fn embed_all(&self, texts: &[String]) -> Result<Vec<Arc<Vec<f32>>>> {
        texts.iter().map(|t| self.embed(t)).collect()
    }

    pub fn cached(&self) -> usize {
        self.cache.lock().map(|c| c.len()).unwrap_or(0)
    }
}

/// 두 정규화 벡터의 코사인 (= 내적)
pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// HF transformers ModernBertModel(F32 · eager) 기준 벡터와 비교합니다. 모델이 있어야 해서 --ignored 입니다.
    /// 기준 벡터 JSON 은 `BLUEPRINT_EMBED_REF` (text · ids · vec 목록), 모델 폴더는 `BLUEPRINT_EMBED_DIR` 또는 모델 폴더의 `embed`.
    #[test]
    #[ignore]
    fn matches_transformers() {
        let dir = std::env::var_os("BLUEPRINT_EMBED_DIR").map(std::path::PathBuf::from).unwrap_or_else(|| crate::ai::model_store::model_dir("embed"));
        let model = TextEmbedder::load(&dir).expect("모델");
        let reference: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(std::env::var("BLUEPRINT_EMBED_REF").expect("BLUEPRINT_EMBED_REF")).unwrap()).unwrap();
        for r in reference.as_array().unwrap() {
            let text = r["text"].as_str().unwrap();
            let ids: Vec<u32> = r["ids"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap() as u32).collect();
            assert_eq!(model.token_ids(text).unwrap(), ids, "토큰 '{text}'");
            let want: Vec<f32> = r["vec"].as_array().unwrap().iter().map(|x| x.as_f64().unwrap() as f32).collect();
            let got = model.embed(text).unwrap();
            let c = cosine(&got, &want);
            assert!(c > 0.9999, "'{text}' 코사인 {c}");
        }
    }
}
