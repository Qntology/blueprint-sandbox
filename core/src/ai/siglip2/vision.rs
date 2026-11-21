//! siglip2 비전 타워: 패치 → 층 27개 → 패치별 공유 공간 특징 + 이미지 전체 풀링 벡터.
//!
//! 풀링 헤드의 어텐션 출력은 `out_proj(Σ αᵢ · v(xᵢ))` 이므로, 패치 i 의 몫은 `out_proj(v(xᵢ))` 입니다.
//! 여기에 헤드의 잔차 블록을 똑같이 적용하면 텍스트 헤드 출력과 같은 좌표계의 패치 특징이 됩니다 (logis-center 의 방식).
//! 이 값과 앵커 문구의 코사인이 히트맵의 원본입니다.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use anyhow::{bail, Context, Result};
use candle_core::{DType, Device, Tensor, TensorId};

use super::config::{Siglip2Config, PATCH_EMBED, VISION_POS};
use super::image::{interpolate_grid, PatchInput};
use super::tower::Stack;
use crate::ai::attention::{full_attention, ATTN_QUERY_CHUNK};
use crate::ai::residency::{dense_linear, Loaded};

const HEAD: &str = "head.attention";

struct CachedInProj {
    qw: Tensor,
    qb: Tensor,
    kw: Tensor,
    kb: Tensor,
    vw: Tensor,
    vb: Tensor,
}

fn pos_host_cache() -> &'static Mutex<HashMap<TensorId, Arc<Vec<f32>>>> {
    static CACHE: OnceLock<Mutex<HashMap<TensorId, Arc<Vec<f32>>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn pos_grid_cache() -> &'static Mutex<HashMap<(TensorId, usize, usize), Tensor>> {
    static CACHE: OnceLock<Mutex<HashMap<(TensorId, usize, usize), Tensor>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn in_proj_cache() -> &'static Mutex<HashMap<(TensorId, TensorId), Arc<CachedInProj>>> {
    static CACHE: OnceLock<Mutex<HashMap<(TensorId, TensorId), Arc<CachedInProj>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn cached_pos_embed(pos_w: &Tensor, side: usize, d: usize, rows: usize, cols: usize, device: &Device) -> Result<Tensor> {
    let tid = pos_w.id();
    if let Some(t) = pos_grid_cache().lock().unwrap_or_else(|p| p.into_inner()).get(&(tid, rows, cols)) {
        if t.device().location() == device.location() {
            return Ok(t.clone());
        }
    }
    let table: Arc<Vec<f32>> = {
        let mut host = pos_host_cache().lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = host.get(&tid) {
            Arc::clone(v)
        } else {
            let v = Arc::new(pos_w.to_device(&Device::Cpu)?.to_dtype(DType::F32)?.flatten_all()?.to_vec1::<f32>()?);
            if host.len() >= 8 {
                host.clear();
            }
            host.insert(tid, Arc::clone(&v));
            v
        }
    };
    let pos = interpolate_grid(&table, side, d, rows, cols);
    let pos = Tensor::from_vec(pos, (1, rows * cols, d), device)?;
    let mut grid = pos_grid_cache().lock().unwrap_or_else(|p| p.into_inner());
    if grid.len() >= 32 {
        grid.clear();
    }
    grid.insert((tid, rows, cols), pos.clone());
    Ok(pos)
}

fn cached_in_proj(s: &Stack<'_>) -> Result<Arc<CachedInProj>> {
    let w = s.w.dense(&format!("{}.{HEAD}.in_proj_weight", s.prefix))?;
    let b_raw = s.w.dense(&format!("{}.{HEAD}.in_proj_bias", s.prefix))?;
    let key = (w.id(), b_raw.id());
    if let Some(c) = in_proj_cache().lock().unwrap_or_else(|p| p.into_inner()).get(&key) {
        return Ok(Arc::clone(c));
    }
    let d = s.cfg.hidden;
    let b = if b_raw.dtype() == DType::F32 { b_raw.clone() } else { b_raw.to_dtype(DType::F32)? };
    let entry = Arc::new(CachedInProj {
        qw: w.narrow(0, 0, d)?.contiguous()?,
        qb: b.narrow(0, 0, d)?.contiguous()?,
        kw: w.narrow(0, d, d)?.contiguous()?,
        kb: b.narrow(0, d, d)?.contiguous()?,
        vw: w.narrow(0, 2 * d, d)?.contiguous()?,
        vb: b.narrow(0, 2 * d, d)?.contiguous()?,
    });
    let mut map = in_proj_cache().lock().unwrap_or_else(|p| p.into_inner());
    if map.len() >= 8 {
        map.clear();
    }
    map.insert(key, Arc::clone(&entry));
    Ok(entry)
}

/// 비전 단계의 결과 (`w` 의 장치). `patches`: `[B, N, D]`, `pooled`: `[B, D]`. 둘 다 L2 정규화 전입니다.
pub struct VisionOutput {
    pub patches: Tensor,
    pub pooled: Tensor,
}

fn stack<'a>(cfg: &'a Siglip2Config, w: &'a Loaded) -> Stack<'a> {
    Stack { w, prefix: "vision_model", cfg: &cfg.vision }
}

/// 패치 임베딩 + 격자에 맞춰 다시 뽑은 위치 임베딩 `[B, N, D]`
fn embed(cfg: &Siglip2Config, w: &Loaded, imgs: &[&PatchInput], device: &Device) -> Result<Tensor> {
    let d = cfg.vision.hidden;
    let img = imgs.first().context("이미지가 없습니다")?;
    if imgs.iter().any(|i| (i.rows, i.cols) != (img.rows, img.cols)) {
        bail!("묶음 안의 격자가 다릅니다");
    }
    let n = img.n_patches();
    let pixels: Vec<f32> = imgs.iter().flat_map(|i| i.pixels.iter().copied()).collect();
    let px = Tensor::from_vec(pixels, (imgs.len(), n, cfg.patch_len()), device)?;
    let y = w.linear(PATCH_EMBED, &px)?;
    let bias_raw = w.dense("vision_model.embeddings.patch_embedding.bias")?;
    let bias = if bias_raw.dtype() == DType::F32 { bias_raw.clone() } else { bias_raw.to_dtype(DType::F32)? };
    let pos = cached_pos_embed(w.dense(VISION_POS)?, cfg.pos_side, d, img.rows, img.cols, device)?;
    Ok(y.broadcast_add(&bias)?.broadcast_add(&pos)?)
}

/// 헤드의 잔차 블록 `h + mlp(norm(h))`
fn head_residual(s: &Stack<'_>, h: &Tensor) -> Result<Tensor> {
    Ok((h + s.mlp("head.mlp", &s.norm("head.layernorm", h)?)?)?)
}

/// 단일 패스 MapHead: `V` 를 한 번만 투영해 `patches` 와 `pooled` 양쪽에서 공유합니다.
fn map_head_all(s: &Stack<'_>, hidden: &Tensor) -> Result<VisionOutput> {
    let (batch, _, d) = hidden.dims3()?;
    let ip = cached_in_proj(s)?;
    // V 를 한 번만 계산해 패치 투영과 풀링 어텐션이 공유합니다.
    let v_flat = dense_linear(hidden, &ip.vw)?.broadcast_add(&ip.vb)?;
    let patches_o = s.linear(&format!("{HEAD}.out_proj"), &v_flat)?;
    let patches = head_residual(s, &patches_o)?;

    let probe = s.tensor("head.probe")?.broadcast_as((batch, 1, d))?.contiguous()?;
    let q = s.split(&dense_linear(&probe, &ip.qw)?.broadcast_add(&ip.qb)?)?;
    let k = s.split(&dense_linear(hidden, &ip.kw)?.broadcast_add(&ip.kb)?)?;
    let v = s.split(&v_flat)?;
    let o = full_attention(&q, &k, &v, s.scale(), ATTN_QUERY_CHUNK)?;
    let o = s.linear(&format!("{HEAD}.out_proj"), &s.join(&o)?)?;
    let pooled = head_residual(s, &o)?.squeeze(1)?;
    Ok(VisionOutput { patches, pooled })
}

/// 격자가 같은 이미지들을 한 번에 읽습니다.
pub fn encode(cfg: &Siglip2Config, w: &Loaded, imgs: &[&PatchInput], device: &Device) -> Result<VisionOutput> {
    let s = stack(cfg, w);
    let x = s.encoder(embed(cfg, w, imgs, device)?)?;
    let hidden = s.norm("post_layernorm", &x)?;
    map_head_all(&s, &hidden)
}
