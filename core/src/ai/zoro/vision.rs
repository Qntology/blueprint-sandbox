//! Zoro-1 비전 인코더 (mmproj 의 `model.visual.*`, Qwen3.5 비전 타워): 이미지 → 언어 모델 차원(2560)의 이미지 토큰 `[병합 토큰, 2560]`.
//!
//! 패치 임베딩(3차원 합성곱: 같은 프레임 두 장이라 시간축을 더한 선형) + 학습된 위치 표(48 × 48 격자를 쌍선형으로 맞춤)
//! → 블록 24개 (LayerNorm · 2차원 RoPE 전체 주의 · GELU(tanh) MLP) → 병합기 (LayerNorm → 이웃 2 × 2 패치를 한 줄로 → fc1 · GELU · fc2).
//! 패치는 2 × 2 병합 창 순서로 늘어섭니다 (transformers `Qwen2VLImageProcessor` · `get_vision_position_ids` 와 같음).
//! 수치는 transformers `Qwen3_5VisionModel` 과 대조했습니다 (`docs/verification.md` 15절, `examples/zoro_vision.rs`).
//!
//! 이 비전 단계의 결과(이미지 토큰)는 CPU 로 옮긴 뒤 비전 타워를 내리고, 텍스트 블록을 올려 `<|image_pad|>` 자리에 넣습니다
//! (`ZoroModel::generate_with_image`). 판단(포인터)과 글쓰기에는 비전 타워가 올라오지 않습니다.

use anyhow::{bail, Context, Result};
use candle_core::{DType, Device, Tensor};
use image::imageops::{resize, FilterType};
use image::RgbImage;

use crate::ai::attention::{full_attention, layer_norm, Rope, ATTN_QUERY_CHUNK};
use crate::ai::residency::Loaded;
use crate::ai::weights::WeightFile;

const P: &str = "model.visual";
/// 비전 LayerNorm 의 eps (transformers `Qwen3_5VisionBlock` · 병합기 모두 1e-6)
const LN_EPS: f64 = 1e-6;
/// 전처리 정규화 (`preprocessor_config.json`: image_mean = image_std = 0.5)
const MEAN: f32 = 0.5;
const STD: f32 = 0.5;
/// 최소 · 최대 화소 수 (Qwen3.5 기본은 65,536 · 16,777,216). 문서 조각은 글자가 읽히는 만큼만 키우고, 이미지 토큰이 넓이에 비례하므로 최대를 줄여 둡니다.
pub const MIN_PIXELS: usize = 65_536;
pub const MAX_PIXELS: usize = 1_048_576;

#[derive(Debug, Clone, PartialEq)]
pub struct VisionConfig {
    pub depth: usize,
    pub hidden: usize,
    pub ffn: usize,
    pub heads: usize,
    pub patch: usize,
    pub merge: usize,
    pub temporal: usize,
    /// 언어 모델 차원
    pub out: usize,
    /// 위치 표 한 변 (48)
    pub grid_side: usize,
}

impl VisionConfig {
    pub fn from_file(file: &WeightFile) -> Result<Self> {
        let arch = file.meta_str("general.architecture")?;
        let n = |key: &str| file.meta_usize(&format!("{arch}.vision.{key}"));
        let table = file.info(&format!("{P}.pos_embed.weight"))?.dims;
        let side = (table[0] as f64).sqrt().round() as usize;
        let cfg = Self {
            depth: n("block_count")?,
            hidden: n("embedding_length")?,
            ffn: n("feed_forward_length")?,
            heads: n("attention.head_count")?,
            patch: n("patch_size")?,
            merge: n("spatial_merge_size")?,
            temporal: n("temporal_patch_size")?,
            out: n("projection_dim")?,
            grid_side: side,
        };
        if side * side != table[0] || table[1] != cfg.hidden {
            bail!("위치 표 {table:?} 가 정사각 격자 × {} 가 아닙니다", cfg.hidden);
        }
        if cfg.heads == 0 || !cfg.hidden.is_multiple_of(cfg.heads) {
            bail!("비전 차원 {} 는 헤드 {} 의 배수여야 합니다", cfg.hidden, cfg.heads);
        }
        Ok(cfg)
    }

    pub fn head_dim(&self) -> usize {
        self.hidden / self.heads
    }

    /// 병합 토큰 한 변의 화소 (32)
    pub fn factor(&self) -> usize {
        self.patch * self.merge
    }
}

/// 비전 인코더에 넣을 이미지 (패치 행렬과 패치 격자)
#[derive(Debug, Clone, PartialEq)]
pub struct VisionImage {
    /// `[패치 수, 3 × patch × patch]` (병합 창 순서, 패치마다 채널 · 행 · 열)
    pub patches: Vec<f32>,
    pub grid_h: usize,
    pub grid_w: usize,
}

impl VisionImage {
    pub fn n_patches(&self) -> usize {
        self.grid_h * self.grid_w
    }

    /// 언어 모델에 들어갈 이미지 토큰 수 (2 × 2 병합 뒤)
    pub fn n_tokens(&self, merge: usize) -> usize {
        self.n_patches() / (merge * merge)
    }
}

/// transformers `smart_resize`: 양 변을 `factor` 의 배수로 반올림하고 화소 수를 `[min, max]` 안으로 맞춥니다.
pub fn smart_resize(h: usize, w: usize, factor: usize, min_pixels: usize, max_pixels: usize) -> (usize, usize) {
    let round = |v: f64| ((v / factor as f64).round() as usize).max(1) * factor;
    let (mut hb, mut wb) = (round(h as f64), round(w as f64));
    if hb * wb > max_pixels {
        let beta = ((h * w) as f64 / max_pixels as f64).sqrt();
        hb = (((h as f64 / beta) / factor as f64).floor() as usize).max(1) * factor;
        wb = (((w as f64 / beta) / factor as f64).floor() as usize).max(1) * factor;
    } else if hb * wb < min_pixels {
        let beta = (min_pixels as f64 / (h * w) as f64).sqrt();
        hb = ((h as f64 * beta) / factor as f64).ceil() as usize * factor;
        wb = ((w as f64 * beta) / factor as f64).ceil() as usize * factor;
    }
    (hb, wb)
}

/// 이미지를 패치 행렬로 만듭니다. 크기를 맞춘 뒤 `(x − 0.5) / 0.5` 로 정규화하고, 패치를 2 × 2 병합 창 순서로 늘어놓습니다.
/// 시간축(같은 프레임 두 장)은 가중치 쪽에서 더해 두므로(`PREPARE`) 패치마다 한 프레임 값만 둡니다.
pub fn prepare(img: &RgbImage, cfg: &VisionConfig, max_pixels: usize) -> Result<VisionImage> {
    let (w0, h0) = (img.width() as usize, img.height() as usize);
    if w0 == 0 || h0 == 0 {
        bail!("빈 이미지입니다");
    }
    let (h, w) = smart_resize(h0, w0, cfg.factor(), MIN_PIXELS.min(max_pixels), max_pixels);
    let resized = if (w, h) == (w0, h0) { img.clone() } else { resize(img, w as u32, h as u32, FilterType::CatmullRom) };
    let (ps, m) = (cfg.patch, cfg.merge);
    let (gh, gw) = (h / ps, w / ps);
    let per = 3 * ps * ps;
    let mut patches = Vec::with_capacity(gh * gw * per);
    let px = |x: usize, y: usize, c: usize| (resized.get_pixel(x as u32, y as u32).0[c] as f32 / 255.0 - MEAN) / STD;
    for br in 0..gh / m {
        for bc in 0..gw / m {
            for i in 0..m {
                for j in 0..m {
                    let (r, col) = (br * m + i, bc * m + j);
                    for c in 0..3 {
                        for y in 0..ps {
                            for x in 0..ps {
                                patches.push(px(col * ps + x, r * ps + y, c));
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(VisionImage { patches, grid_h: gh, grid_w: gw })
}

/// 올릴 때 한 번 하는 변환: 패치 임베딩 합성곱 `[D, 3 × 2, 16, 16]` (채널 × 시간을 합친 모양)을 시간축으로 더해 `[D, 3 × 16 × 16]` 로 폅니다.
pub fn prepare_patch_embed(t: Tensor) -> candle_core::Result<Tensor> {
    let (d, ct, h, w) = t.dims4()?;
    let dtype = t.dtype();
    let summed = t.to_dtype(DType::F32)?.reshape((d, 3, ct / 3, h, w))?.sum(2)?;
    summed.reshape((d, 3 * h * w))?.to_dtype(dtype)
}

pub const PATCH_EMBED: &str = "model.visual.patch_embed.proj.weight";

fn linear_bias(w: &Loaded, name: &str, x: &Tensor) -> Result<Tensor> {
    let y = w.linear(&format!("{P}.{name}.weight"), &x.to_dtype(DType::F32)?)?;
    let bias = w.dense(&format!("{P}.{name}.bias"))?.to_dtype(DType::F32)?;
    Ok(y.broadcast_add(&bias)?)
}

fn ln(w: &Loaded, name: &str, x: &Tensor) -> Result<Tensor> {
    let weight = w.dense(&format!("{P}.{name}.weight"))?.to_dtype(DType::F32)?;
    let bias = w.dense(&format!("{P}.{name}.bias"))?.to_dtype(DType::F32)?;
    layer_norm(x, &weight, &bias, LN_EPS)
}

/// 패치마다 (행, 열) 위치 — 병합 창 순서 (`get_vision_position_ids`)
fn positions(cfg: &VisionConfig, gh: usize, gw: usize) -> Vec<(usize, usize)> {
    let m = cfg.merge;
    let mut out = Vec::with_capacity(gh * gw);
    for br in 0..gh / m {
        for bc in 0..gw / m {
            for i in 0..m {
                for j in 0..m {
                    out.push((br * m + i, bc * m + j));
                }
            }
        }
    }
    out
}

/// 2차원 RoPE 표: 회전 차원 head_dim/2 의 주파수(theta 10000)를 행 위치 · 열 위치에 한 벌씩 (`Qwen3_5VisionRotaryEmbedding`)
fn rope(cfg: &VisionConfig, pos: &[(usize, usize)], device: &Device) -> Result<Rope> {
    let half = cfg.head_dim() / 2;
    let inv: Vec<f64> = (0..half / 2).map(|i| 10000f64.powf(-(2.0 * i as f64) / half as f64)).collect();
    let (mut cos, mut sin) = (Vec::with_capacity(pos.len() * half), Vec::with_capacity(pos.len() * half));
    for &(r, c) in pos {
        for p in [r, c] {
            for f in &inv {
                let a = p as f64 * f;
                cos.push(a.cos() as f32);
                sin.push(a.sin() as f32);
            }
        }
    }
    Rope::from_tables(Tensor::from_vec(cos, (pos.len(), half), device)?, Tensor::from_vec(sin, (pos.len(), half), device)?)
}

/// 위치 표 48 × 48 을 패치 격자에 쌍선형으로 맞춘 값 `[패치, D]` (`get_vision_bilinear_indices_and_weights` 와 같은 모서리 · 가중치)
fn position_embeddings(w: &Loaded, cfg: &VisionConfig, gh: usize, gw: usize, pos: &[(usize, usize)], device: &Device) -> Result<Tensor> {
    let side = cfg.grid_side;
    let lin = |n: usize| -> Vec<f64> { (0..n).map(|i| if n == 1 { 0.0 } else { i as f64 * (side - 1) as f64 / (n - 1) as f64 }).collect() };
    let (hs, ws) = (lin(gh), lin(gw));
    let mut idx: [Vec<u32>; 4] = Default::default();
    let mut wt: [Vec<f32>; 4] = Default::default();
    for &(r, c) in pos {
        let (hf, wf) = (hs[r], ws[c]);
        let (h0, w0) = (hf as usize, wf as usize);
        let (h1, w1) = ((h0 + 1).min(side - 1), (w0 + 1).min(side - 1));
        let (dh, dw) = ((hf - h0 as f64) as f32, (wf - w0 as f64) as f32);
        let corners = [(h0, w0, (1.0 - dh) * (1.0 - dw)), (h0, w1, (1.0 - dh) * dw), (h1, w0, dh * (1.0 - dw)), (h1, w1, dh * dw)];
        for (k, (hh, ww, weight)) in corners.into_iter().enumerate() {
            idx[k].push((hh * side + ww) as u32);
            wt[k].push(weight);
        }
    }
    let table = w.dense(&format!("{P}.pos_embed.weight"))?;
    let mut acc: Option<Tensor> = None;
    for k in 0..4 {
        let rows = table.index_select(&Tensor::new(idx[k].as_slice(), table.device())?, 0)?.to_dtype(DType::F32)?.to_device(device)?;
        let weights = Tensor::from_vec(wt[k].clone(), (pos.len(), 1), device)?;
        let term = rows.broadcast_mul(&weights)?;
        acc = Some(match acc {
            Some(a) => (a + term)?,
            None => term,
        });
    }
    acc.context("위치가 없습니다")
}

fn block(cfg: &VisionConfig, w: &Loaded, i: usize, x: &Tensor, rope: &Rope) -> Result<Tensor> {
    let b = format!("blocks.{i}");
    let n = x.dim(0)?;
    let (nh, d) = (cfg.heads, cfg.head_dim());
    let h = ln(w, &format!("{b}.norm1"), x)?;
    let qkv = linear_bias(w, &format!("{b}.attn.qkv"), &h)?.reshape((n, 3, nh, d))?.permute((1, 2, 0, 3))?.contiguous()?;
    let (q, k, v) = (qkv.narrow(0, 0, 1)?.squeeze(0)?, qkv.narrow(0, 1, 1)?.squeeze(0)?, qkv.narrow(0, 2, 1)?.squeeze(0)?.contiguous()?);
    let q = rope.apply(&q.contiguous()?)?;
    let k = rope.apply(&k.contiguous()?)?;
    let o = full_attention(&q, &k, &v, 1.0 / (d as f64).sqrt(), ATTN_QUERY_CHUNK)?;
    let o = o.transpose(0, 1)?.contiguous()?.reshape((n, nh * d))?;
    let x = (x + linear_bias(w, &format!("{b}.attn.proj"), &o)?)?;
    let h = ln(w, &format!("{b}.norm2"), &x)?;
    let m = linear_bias(w, &format!("{b}.mlp.linear_fc1"), &h)?.gelu()?;
    Ok((x + linear_bias(w, &format!("{b}.mlp.linear_fc2"), &m)?)?)
}

/// 이미지 한 장을 이미지 토큰 `[병합 토큰, out]` (F32, `device`) 으로 바꿉니다. `w` 는 비전 단계의 묶음입니다.
pub fn encode(cfg: &VisionConfig, w: &Loaded, img: &VisionImage, device: &Device) -> Result<Tensor> {
    let n = img.n_patches();
    if img.patches.len() != n * 3 * cfg.patch * cfg.patch {
        bail!("패치 값 {} 개가 패치 {n} × {} 와 다릅니다", img.patches.len(), 3 * cfg.patch * cfg.patch);
    }
    if !img.grid_h.is_multiple_of(cfg.merge) || !img.grid_w.is_multiple_of(cfg.merge) {
        bail!("패치 격자 {} × {} 가 병합 {} 의 배수가 아닙니다", img.grid_h, img.grid_w, cfg.merge);
    }
    let pos = positions(cfg, img.grid_h, img.grid_w);
    let x = Tensor::from_vec(img.patches.clone(), (n, 3 * cfg.patch * cfg.patch), device)?;
    let mut x = (linear_bias(w, "patch_embed.proj", &x)? + position_embeddings(w, cfg, img.grid_h, img.grid_w, &pos, device)?)?;
    let rope = rope(cfg, &pos, device)?;
    for i in 0..cfg.depth {
        x = block(cfg, w, i, &x, &rope)?;
    }
    let mm = cfg.merge * cfg.merge;
    let x = ln(w, "merger.norm", &x)?.reshape((n / mm, mm * cfg.hidden))?;
    let h = linear_bias(w, "merger.linear_fc1", &x)?.gelu_erf()?;
    linear_bias(w, "merger.linear_fc2", &h)
}

/// 시험용 작은 비전 mmproj (hidden 32 · 헤드 2 · 패치 2 · 병합 2 · 블록 1 · 위치 표 4 × 4, 출력 `out`)
#[cfg(test)]
pub(crate) fn tiny_mmproj(dir: &crate::ai::weights::testing::TempDir, out: usize) -> std::sync::Arc<WeightFile> {
    use crate::ai::verify::det_normal;
    use candle_core::quantized::gguf_file::Value;
    use candle_core::quantized::GgmlDType;
    let (d, f, ps) = (32usize, 64usize, 2usize);
    let list: Vec<(String, Vec<usize>)> = vec![
        ("patch_embed.proj.weight".into(), vec![d, 6, ps, ps]),
        ("patch_embed.proj.bias".into(), vec![d]),
        ("pos_embed.weight".into(), vec![16, d]),
        ("blocks.0.norm1.weight".into(), vec![d]),
        ("blocks.0.norm1.bias".into(), vec![d]),
        ("blocks.0.norm2.weight".into(), vec![d]),
        ("blocks.0.norm2.bias".into(), vec![d]),
        ("blocks.0.attn.qkv.weight".into(), vec![3 * d, d]),
        ("blocks.0.attn.qkv.bias".into(), vec![3 * d]),
        ("blocks.0.attn.proj.weight".into(), vec![d, d]),
        ("blocks.0.attn.proj.bias".into(), vec![d]),
        ("blocks.0.mlp.linear_fc1.weight".into(), vec![f, d]),
        ("blocks.0.mlp.linear_fc1.bias".into(), vec![f]),
        ("blocks.0.mlp.linear_fc2.weight".into(), vec![d, f]),
        ("blocks.0.mlp.linear_fc2.bias".into(), vec![d]),
        ("merger.norm.weight".into(), vec![d]),
        ("merger.norm.bias".into(), vec![d]),
        ("merger.linear_fc1.weight".into(), vec![4 * d, 4 * d]),
        ("merger.linear_fc1.bias".into(), vec![4 * d]),
        ("merger.linear_fc2.weight".into(), vec![out, 4 * d]),
        ("merger.linear_fc2.bias".into(), vec![out]),
    ];
    let tensors: Vec<(String, Tensor, GgmlDType)> = list
        .into_iter()
        .enumerate()
        .map(|(i, (name, shape))| {
            let n = shape.iter().product();
            let gain = if name.contains("norm") && name.ends_with("weight") { 0.0 } else { 0.3 };
            // LayerNorm 가중치는 1 근처로
            let v: Vec<f32> = det_normal(n, 500 + i as u64).into_iter().map(|x| x * gain + if gain == 0.0 { 1.0 } else { 0.0 }).collect();
            (format!("model.visual.{name}"), Tensor::from_vec(v, shape, &Device::Cpu).unwrap(), GgmlDType::F32)
        })
        .collect();
    let meta = [
        ("general.architecture", Value::String("zoro".into())),
        ("zoro.vision.block_count", Value::U32(1)),
        ("zoro.vision.embedding_length", Value::U32(d as u32)),
        ("zoro.vision.feed_forward_length", Value::U32(f as u32)),
        ("zoro.vision.attention.head_count", Value::U32(2)),
        ("zoro.vision.patch_size", Value::U32(ps as u32)),
        ("zoro.vision.spatial_merge_size", Value::U32(2)),
        ("zoro.vision.temporal_patch_size", Value::U32(2)),
        ("zoro.vision.projection_dim", Value::U32(out as u32)),
    ];
    let path = dir.0.join("mmproj-tiny.gguf");
    crate::ai::weights::testing::write_gguf(&path, &meta, &tensors).unwrap();
    std::sync::Arc::new(WeightFile::open(&path).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() -> VisionConfig {
        VisionConfig { depth: 1, hidden: 32, ffn: 64, heads: 2, patch: 2, merge: 2, temporal: 2, out: 24, grid_side: 4 }
    }

    #[test]
    fn a_tiny_tower_turns_patches_into_one_token_per_merge_window() -> Result<()> {
        use crate::ai::residency::{Budget, Group, ModelId, ModelSpec, Phase, Residency};
        let dir = crate::ai::weights::testing::TempDir::new("zv-tiny");
        let file = tiny_mmproj(&dir, 24);
        let c = VisionConfig::from_file(&file)?;
        assert_eq!((c.hidden, c.heads, c.patch, c.merge, c.out, c.grid_side), (32, 2, 2, 2, 24, 4));
        let spec = ModelSpec::new(ModelId::Zoro, std::sync::Arc::clone(&file))
            .with_mmproj(file)
            .with_prepare(crate::ai::residency::Prepare { touches: |n| n == PATCH_EMBED, apply: |_, t| prepare_patch_embed(t) });
        let mut res = Residency::new(Device::Cpu, Budget::unlimited());
        let h = res.enter(&spec, &Phase::new("비전", [Group::Vision]))?;
        let mut img = RgbImage::new(8, 12);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 30) as u8, (y * 20) as u8, ((x + y) * 9) as u8]);
        }
        let v = prepare(&img, &c, 96)?;
        assert_eq!((v.grid_h, v.grid_w), (6, 4));
        let out = encode(&c, &h[0], &v, &Device::Cpu)?;
        assert_eq!(out.dims(), &[6, 24], "병합 창 (6/2) × (4/2) = 6 토큰");
        assert!(out.flatten_all()?.to_vec1::<f32>()?.iter().all(|x| x.is_finite()));
        // 다른 그림은 다른 토큰
        let other = prepare(&RgbImage::from_pixel(8, 12, image::Rgb([255, 255, 255])), &c, 96)?;
        let out2 = encode(&c, &h[0], &other, &Device::Cpu)?;
        assert!((out - out2)?.abs()?.max_all()?.to_scalar::<f32>()? > 1e-3);
        Ok(())
    }

    #[test]
    fn smart_resize_follows_transformers() {
        // 32 의 배수로 반올림
        assert_eq!(smart_resize(500, 700, 32, 65_536, 16_777_216), (512, 704));
        // 너무 크면 화소 수를 넘지 않게 내림
        let (h, w) = smart_resize(3000, 4000, 32, 65_536, 1_048_576);
        assert!(h * w <= 1_048_576 && h % 32 == 0 && w % 32 == 0, "{h} × {w}");
        // 너무 작으면 올림
        let (h, w) = smart_resize(40, 120, 32, 65_536, 16_777_216);
        assert!(h * w >= 65_536 && h % 32 == 0 && w % 32 == 0, "{h} × {w}");
    }

    #[test]
    fn patches_follow_the_merge_window_order() -> Result<()> {
        let c = cfg();
        // 4 × 4 화소, 패치 2 → 패치 격자 2 × 2, 병합 창 하나
        let mut img = RgbImage::new(64, 64);
        for (x, y, p) in img.enumerate_pixels_mut() {
            *p = image::Rgb([(x * 4) as u8, (y * 4) as u8, 128]);
        }
        let v = prepare(&img, &c, 64 * 64)?;
        assert_eq!((v.grid_h, v.grid_w), (32, 32), "4096 화소 → 최소 화소 맞춤 없이 64 × 64");
        let pos = positions(&c, 4, 4);
        assert_eq!(&pos[..4], &[(0, 0), (0, 1), (1, 0), (1, 1)], "첫 병합 창의 네 패치");
        assert_eq!(pos[4], (0, 2), "다음 병합 창은 오른쪽");
        // 첫 패치의 첫 값은 (0,0) 화소의 빨강: (0/255 − 0.5)/0.5 = −1
        assert!((v.patches[0] + 1.0).abs() < 1e-6);
        Ok(())
    }

    #[test]
    fn the_patch_embedding_sums_the_two_identical_frames() -> Result<()> {
        let t = Tensor::arange(0f32, (2 * 6 * 2 * 2) as f32, &Device::Cpu)?.reshape((2, 6, 2, 2))?;
        let out = prepare_patch_embed(t.clone())?;
        assert_eq!(out.dims(), &[2, 12]);
        // 채널 c 의 두 프레임은 6 축의 2c · 2c+1 (GGUF 가 [D, 3, 2, 16, 16] 을 [D, 6, 16, 16] 으로 합친 순서)
        let v = out.to_vec2::<f32>()?;
        let raw = t.flatten_all()?.to_vec1::<f32>()?;
        assert_eq!(v[0][0], raw[0] + raw[4]);
        assert_eq!(v[1][4 + 3], raw[24 + 8 + 3] + raw[24 + 12 + 3]);
        Ok(())
    }
}
