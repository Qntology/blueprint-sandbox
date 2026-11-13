//! 피라미드 양자화: 텐서마다 정밀도를 달리한 GGUF 를 **원본 BF16 가중치에서 한 번** 양자화해 만듭니다.
//!
//! - 재료: 모델 폴더 `bf16/` 의 원본 safetensors ([`super::original`], Hugging Face 의 trillionlabs 저장소 그대로). 저장 형식이 원본
//!   그대로인 텐서(BF16 mmproj)는 그 파일 값을 써도 같습니다 (원본과 비트까지 같음을 `Original::identical` 로 확인).
//!   q8 파일의 Q8_0 을 풀어 다시 양자화하는 길은 없습니다 (두 번 양자화한 오차가 겹침, [`material_rows`] 가 거부).
//! - 인코더: GGML 형식(Q8_0 · Q6_K · Q5_K · Q4_K)에 gravity-ocr `quant.py` 의 척도 탐색(중요도 `rms(블록) + |x|`, 있으면 중요도 행렬)을
//!   씁니다 ([`super::kquant`]). Q3_K · NF4 는 만들지 않습니다 (gravity-ocr 배분의 q3 · nf4 자리는 Q4_K).
//! - 배분: gravity-ocr-qsa-ngram 이 같은 모델(GravityOCR-KO)의 53개 묶음 × 6개 형식을 teacher forcing ΔNLL 로 재서 정한 것([`GOCR_MEASURED`])과
//!   비전 16 ~ 22 층 · downsample · merger 보호 규칙, Zoro-1 의 보호 규칙([`ZORO_SPEC`]). 커널이 다르므로 이 엔진에서 다시 재서 고릅니다
//!   (`examples/quant_screen.rs`, `docs/verification.md` 15절). 정규화 · 편향 · 패치 임베딩(합성곱)은 저장된 그대로 둡니다.
//! - 출력 투영 잘라 두기([`HeadPlan`]): 쓰는 글자의 행(특수 토큰 · 한글 · 바이트 조각 · 표 태그 · 도면 영어 · 원장 글)을
//!   `output.weight` 로, 나머지 행을 `output_rest.weight` 로 나누고 남긴 행의 번호를 `<구조>.head.ids` 에 둡니다.
//!   디코딩은 남긴 행으로 고르고, 잘린 행의 최댓값이 이기면 그 토큰을 씁니다 (잘린 행 검사, gravity-ocr 의 `PRESSURE` → `RESSURE` 결함).
//!
//! 만든 파일은 모델 폴더에 두고 `active.json` 의 `pyramid` 에 적습니다 (배분 · 재료 · [`RECIPE`]). 레시피가 바뀌면 앱이 다시 만듭니다
//! ([`ensure`], 사용할 때 양자화). 엔진은 K 형식을 장치에서 곱하는 백엔드(ROCm · CUDA · Metal · CPU)에서만 이 파일을 엽니다.
//! 포크의 Vulkan 은 Q8_0 만 장치에서 곱하고 나머지는 호스트에서 곱하므로 q8 파일 그대로입니다 ([`supports_k_quants`]).

use std::collections::BTreeSet;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{anyhow, bail, Context, Result};
use candle_core::quantized::gguf_file::Value;
use candle_core::quantized::GgmlDType;
use candle_core::Device;
use rayon::prelude::*;
use serde_json::json;

use super::kquant;
use super::original::{rows_cols, stored_gather, Family, Original};
use super::tokenizer::ModelTokenizer;
use super::weights::WeightFile;

/// 피라미드 파일을 쓸지 정하는 환경 변수: `auto`(기본, 백엔드가 K 형식을 장치에서 곱하고 파일이 있으면) · `0`(끔) · `1`(파일이 있으면 늘)
pub const PYRAMID_ENV: &str = "BLUEPRINT_PYRAMID";
/// GGUF 메타데이터: 배분 이름 · 재료 · 레시피 번호
pub const META_PLAN: &str = "blueprint.pyramid.plan";
pub const META_SOURCE: &str = "blueprint.pyramid.source";
pub const META_RECIPE: &str = "blueprint.pyramid.recipe";
/// 인코더 · 배분 규칙의 번호. 바꾸면 앱이 피라미드 파일을 다시 만듭니다 ([`ensure`]).
/// 1: q8 파일에서 다시 양자화(폐기), 2: 원본 BF16 에서 quant.py 척도 탐색으로 한 번 양자화
pub const RECIPE: u32 = 2;
/// 한 번에 읽어 양자화하는 원소 수 (메모리를 묶어 둠)
const CHUNK_ELEMS: usize = 1 << 22;
const ALIGN: u64 = 32;

/// 이 장치가 K 형식(Q4_K 등)을 장치에서 곱하는가. 포크의 Vulkan 은 Q8_0 만 장치에서 곱하고 나머지는 호스트 메모리에서 CPU 로 곱합니다
/// (`candle-core/src/quantized/vulkan.rs`). 그런 백엔드에서 피라미드 파일을 쓰면 가중치가 VRAM 대신 RAM 에서 CPU 로 계산됩니다.
pub fn supports_k_quants(device: &Device) -> bool {
    !device.is_vulkan()
}

/// `BLUEPRINT_PYRAMID` 와 장치로 피라미드 파일을 쓸지 정합니다.
pub fn wanted(device: &Device) -> bool {
    match std::env::var(PYRAMID_ENV).map(|v| v.trim().to_ascii_lowercase()) {
        Ok(v) if v == "0" || v == "off" || v == "q8" => false,
        Ok(v) if v == "1" || v == "on" => true,
        _ => supports_k_quants(device),
    }
}

// ---------------------------------------------------------------------------------------------
// 양자화 재료 (원본에서만)
// ---------------------------------------------------------------------------------------------

/// 진단 전용: `1` 이면 원본이 없는 양자화 텐서(Q8_0)를 풀어 다시 양자화합니다. 두 번 양자화의 영향을 같은 인코더로 재는 데만 씁니다
/// (`examples/quant_screen` 의 `q8src:` 배분, `docs/verification.md` 15절). 앱은 이 변수를 보지 않는 경로로 엽니다.
pub const REQUANT_DIAG_ENV: &str = "BLUEPRINT_REQUANT_FROM_Q8";

/// 양자화 재료: 텐서 `name` 의 `ids` 번 행 (F32, `[행, 열]` 보기).
/// 원본(`original`)에 있으면 원본에서, 없으면 저장 형식이 원본 그대로(BF16 · F16 · F32)일 때만 파일에서 읽습니다.
/// 양자화된 텐서(Q8_0 등)를 풀어 다시 양자화하는 것은 거부합니다 (두 번 양자화, 진단 변수 [`REQUANT_DIAG_ENV`] 밖).
pub fn material_gather(file: &WeightFile, name: &str, original: Option<&Original>, ids: &[u32]) -> Result<Vec<f32>> {
    if let Some(o) = original.filter(|o| o.has(name)) {
        return o.gather(name, ids);
    }
    let meta = file.info(name)?;
    if !meta.is_float() && !std::env::var(REQUANT_DIAG_ENV).is_ok_and(|v| v == "1") {
        bail!("{name}: 저장된 {:?} 를 다시 양자화하지 않습니다 — 원본 BF16 (모델 폴더의 {}/) 이 필요합니다", meta.dtype, super::original::DIR);
    }
    stored_gather(file, name, ids)
}

/// [`material_gather`] 의 이어진 행 `[start, start + count)`
pub fn material_rows(file: &WeightFile, name: &str, original: Option<&Original>, start: usize, count: usize) -> Result<Vec<f32>> {
    if let Some(o) = original.filter(|o| o.has(name)) {
        return o.rows(name, start, count);
    }
    let ids: Vec<u32> = (start as u32..(start + count) as u32).collect();
    material_gather(file, name, original, &ids)
}

/// 텐서 하나를 `dtype` 로 양자화한 바이트 (행 묶음씩 읽어 메모리를 묶어 둠). `weights` 는 열마다의 중요도입니다.
pub fn quantize_tensor(file: &WeightFile, name: &str, original: Option<&Original>, dtype: GgmlDType, weights: Option<&[f32]>) -> Result<Vec<u8>> {
    let (rows, cols) = rows_cols(&file.info(name)?.dims);
    let per = (CHUNK_ELEMS / cols).max(1);
    let mut out = Vec::with_capacity(rows * cols / dtype.block_size() * dtype.type_size());
    let mut r = 0;
    while r < rows {
        let n = per.min(rows - r);
        out.extend(kquant::encode(&material_rows(file, name, original, r, n)?, cols, dtype, weights)?);
        r += n;
    }
    Ok(out)
}

/// `ids` 번 행만 골라 `dtype` 로 양자화한 바이트 (잘라 둔 출력 투영)
pub fn quantize_rows_of(file: &WeightFile, name: &str, original: Option<&Original>, ids: &[u32], dtype: GgmlDType, weights: Option<&[f32]>) -> Result<Vec<u8>> {
    let cols = rows_cols(&file.info(name)?.dims).1;
    let per = (CHUNK_ELEMS / cols).max(1);
    let mut out = Vec::with_capacity(ids.len() * cols / dtype.block_size() * dtype.type_size());
    for chunk in ids.chunks(per) {
        out.extend(kquant::encode(&material_gather(file, name, original, chunk)?, cols, dtype, weights)?);
    }
    Ok(out)
}

/// 중요도 행렬: 텐서 이름 → 열마다 입력 활성값의 제곱 평균 (`residency::imatrix` 가 모음, llama.cpp `imatrix` 와 같은 뜻)
pub type Imatrix = std::collections::HashMap<String, Vec<f32>>;

const IMATRIX_MAGIC: &[u8; 4] = b"BPIM";

/// 중요도 행렬을 파일로 (`BPIM`, 이름 순)
pub fn save_imatrix(path: &Path, m: &Imatrix) -> Result<()> {
    let mut keys: Vec<&String> = m.keys().collect();
    keys.sort();
    let mut out = Vec::new();
    out.extend_from_slice(IMATRIX_MAGIC);
    out.extend_from_slice(&(keys.len() as u32).to_le_bytes());
    for k in keys {
        let v = &m[k];
        out.extend_from_slice(&(k.len() as u32).to_le_bytes());
        out.extend_from_slice(k.as_bytes());
        out.extend_from_slice(&(v.len() as u32).to_le_bytes());
        v.iter().for_each(|x| out.extend_from_slice(&x.to_le_bytes()));
    }
    std::fs::write(path, out)?;
    Ok(())
}

pub fn load_imatrix(path: &Path) -> Result<Imatrix> {
    let raw = std::fs::read(path).with_context(|| format!("{} 를 읽을 수 없습니다", path.display()))?;
    let mut at = 0usize;
    let mut take = |n: usize| -> Result<&[u8]> {
        let s = raw.get(at..at + n).ok_or_else(|| anyhow!("{}: 파일이 잘렸습니다", path.display()))?;
        at += n;
        Ok(s)
    };
    if take(4)? != IMATRIX_MAGIC {
        bail!("{} 는 중요도 행렬 파일이 아닙니다", path.display());
    }
    let u32_of = |b: &[u8]| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize;
    let n = u32_of(take(4)?);
    let mut m = Imatrix::new();
    for _ in 0..n {
        let kl = u32_of(take(4)?);
        let k = String::from_utf8(take(kl)?.to_vec())?;
        let vl = u32_of(take(4)?);
        let v: Vec<f32> = take(vl * 4)?.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
        m.insert(k, v);
    }
    Ok(m)
}

// ---------------------------------------------------------------------------------------------
// GGUF 쓰기 (텐서마다 바이트를 차례가 올 때 만들어 바로 씁니다)
// ---------------------------------------------------------------------------------------------

type Produce = Box<dyn FnOnce() -> Result<Vec<u8>> + Send>;

/// 새 GGUF 의 텐서 하나. 바이트는 쓸 차례가 되면 만듭니다.
pub struct OutTensor {
    pub name: String,
    /// candle 순서의 모양 (마지막 차원이 열)
    pub dims: Vec<usize>,
    pub dtype: GgmlDType,
    produce: Produce,
}

impl OutTensor {
    pub fn new(name: impl Into<String>, dims: Vec<usize>, dtype: GgmlDType, produce: impl FnOnce() -> Result<Vec<u8>> + Send + 'static) -> Self {
        Self { name: name.into(), dims, dtype, produce: Box::new(produce) }
    }

    fn bytes(&self) -> u64 {
        (self.dims.iter().product::<usize>() / self.dtype.block_size() * self.dtype.type_size()) as u64
    }
}

/// 쓴 바이트 수를 세는 쓰개
struct Counting<W: Write> {
    inner: W,
    at: u64,
}

impl<W: Write> Write for Counting<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(buf)?;
        self.at += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

fn write_str(w: &mut impl Write, s: &str) -> Result<()> {
    w.write_all(&(s.len() as u64).to_le_bytes())?;
    w.write_all(s.as_bytes())?;
    Ok(())
}

/// GGUF 값 형식 번호
fn value_type(v: &Value) -> u32 {
    match v {
        Value::U8(_) => 0,
        Value::I8(_) => 1,
        Value::U16(_) => 2,
        Value::I16(_) => 3,
        Value::U32(_) => 4,
        Value::I32(_) => 5,
        Value::F32(_) => 6,
        Value::Bool(_) => 7,
        Value::String(_) => 8,
        Value::Array(_) => 9,
        Value::U64(_) => 10,
        Value::I64(_) => 11,
        Value::F64(_) => 12,
    }
}

fn write_value(w: &mut impl Write, v: &Value) -> Result<()> {
    match v {
        Value::U8(x) => w.write_all(&[*x])?,
        Value::I8(x) => w.write_all(&x.to_le_bytes())?,
        Value::U16(x) => w.write_all(&x.to_le_bytes())?,
        Value::I16(x) => w.write_all(&x.to_le_bytes())?,
        Value::U32(x) => w.write_all(&x.to_le_bytes())?,
        Value::I32(x) => w.write_all(&x.to_le_bytes())?,
        Value::U64(x) => w.write_all(&x.to_le_bytes())?,
        Value::I64(x) => w.write_all(&x.to_le_bytes())?,
        Value::F32(x) => w.write_all(&x.to_le_bytes())?,
        Value::F64(x) => w.write_all(&x.to_le_bytes())?,
        Value::Bool(x) => w.write_all(&[*x as u8])?,
        Value::String(s) => write_str(w, s)?,
        Value::Array(items) => {
            let t = items.first().map(value_type).unwrap_or(4);
            if items.iter().any(|i| value_type(i) != t) {
                bail!("GGUF 배열의 값 형식이 섞여 있습니다");
            }
            w.write_all(&t.to_le_bytes())?;
            w.write_all(&(items.len() as u64).to_le_bytes())?;
            for i in items {
                write_value(w, i)?;
            }
        }
    }
    Ok(())
}

/// GGML 형식 번호 (ggml.h 의 `ggml_type`)
fn ggml_type_id(dtype: GgmlDType) -> u32 {
    match dtype {
        GgmlDType::F32 => 0,
        GgmlDType::F16 => 1,
        GgmlDType::Q4_0 => 2,
        GgmlDType::Q4_1 => 3,
        GgmlDType::Q5_0 => 6,
        GgmlDType::Q5_1 => 7,
        GgmlDType::Q8_0 => 8,
        GgmlDType::Q8_1 => 9,
        GgmlDType::Q2K => 10,
        GgmlDType::Q3K => 11,
        GgmlDType::Q4K => 12,
        GgmlDType::Q5K => 13,
        GgmlDType::Q6K => 14,
        GgmlDType::Q8K => 15,
        GgmlDType::BF16 => 30,
    }
}

fn pad(w: &mut Counting<impl Write>) -> Result<()> {
    let n = (ALIGN - w.at % ALIGN) % ALIGN;
    w.write_all(&vec![0u8; n as usize])?;
    Ok(())
}

/// GGUF v3 를 씁니다. `<path>.tmp` 에 끝까지 쓴 뒤 이름을 바꿉니다 (쓰다 멈춘 파일이 모델로 열리지 않게). 쓴 바이트 수를 돌려줍니다.
pub fn write_gguf(path: &Path, metadata: &[(String, Value)], tensors: Vec<OutTensor>) -> Result<u64> {
    let tmp = path.with_extension("gguf.tmp");
    let mut w = Counting { inner: BufWriter::with_capacity(8 << 20, File::create(&tmp).with_context(|| format!("쓸 수 없습니다: {}", tmp.display()))?), at: 0 };
    w.write_all(b"GGUF")?;
    w.write_all(&3u32.to_le_bytes())?;
    w.write_all(&(tensors.len() as u64).to_le_bytes())?;
    w.write_all(&(metadata.len() as u64).to_le_bytes())?;
    for (k, v) in metadata {
        write_str(&mut w, k)?;
        w.write_all(&value_type(v).to_le_bytes())?;
        write_value(&mut w, v)?;
    }
    let mut offset = 0u64;
    for t in &tensors {
        write_str(&mut w, &t.name)?;
        w.write_all(&(t.dims.len() as u32).to_le_bytes())?;
        for d in t.dims.iter().rev() {
            w.write_all(&(*d as u64).to_le_bytes())?;
        }
        w.write_all(&ggml_type_id(t.dtype).to_le_bytes())?;
        w.write_all(&offset.to_le_bytes())?;
        offset += t.bytes();
        offset = offset.div_ceil(ALIGN) * ALIGN;
    }
    pad(&mut w)?;
    for t in tensors {
        let want = t.bytes();
        let name = t.name.clone();
        let bytes = (t.produce)().with_context(|| format!("{name} 를 만들 수 없습니다"))?;
        if bytes.len() as u64 != want {
            bail!("{name}: {} 바이트를 만들었는데 목차는 {want} 바이트입니다", bytes.len());
        }
        w.write_all(&bytes)?;
        pad(&mut w)?;
    }
    w.flush()?;
    let total = w.at;
    drop(w);
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("{} 를 바꿀 수 없습니다 (앱이 열고 있으면 끄고 다시 하세요)", path.display()))?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(total)
}

// ---------------------------------------------------------------------------------------------
// 배분
// ---------------------------------------------------------------------------------------------

/// 텐서 이름 → 형식 (`None` 이면 저장된 그대로)
pub type Rule = fn(&str) -> Option<GgmlDType>;

/// 출력 투영 잘라 두기: 남길 행 수와 두 부분의 형식
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeadPlan {
    pub budget: usize,
    pub kept: GgmlDType,
    pub rest: GgmlDType,
}

/// 모델 하나의 배분
#[derive(Clone, Copy)]
pub struct Plan {
    pub name: &'static str,
    /// 본 파일(언어 · 텍스트 블록)
    pub text: Rule,
    /// 비전 파일(mmproj)
    pub vision: Rule,
    pub head: Option<HeadPlan>,
}

/// `blk.<층>.<역할>.weight` → (층, 역할)
fn block_role(name: &str) -> Option<(usize, &str)> {
    let rest = name.strip_prefix("blk.")?;
    let (layer, tail) = rest.split_once('.')?;
    Some((layer.parse().ok()?, tail.strip_suffix(".weight")?))
}

/// `model.visual.blocks.<층>.<역할>.weight` → (층, 역할)
fn vision_role(name: &str) -> Option<(usize, &str)> {
    let rest = name.strip_prefix("model.visual.blocks.")?;
    let (layer, tail) = rest.split_once('.')?;
    Some((layer.parse().ok()?, tail.strip_suffix(".weight")?))
}

use GgmlDType::{Q4K, Q5K, Q6K, Q8_0};

/// gocr 언어: gravity-ocr 측정 배분 (`build/quant/final.json`, 예산 ΔNLL 0.002). q3 · nf4 자리는 Q4_K (Q3_K 는 만들지 않음).
fn gocr_measured_text(name: &str) -> Option<GgmlDType> {
    let (i, role) = block_role(name)?;
    Some(match (i, role) {
        (0..=1, "attn_q" | "attn_k" | "attn_v") => Q4K,
        (0..=1, "attn_output") => Q8_0,
        (2..=5, "attn_q" | "attn_k" | "attn_v" | "attn_output") => Q4K,
        (6..=9, "attn_q" | "attn_k" | "attn_v") => Q4K,
        (6..=9, "attn_output") => Q5K,
        (10..=13, "attn_q" | "attn_k") => Q4K,
        (10..=13, "attn_v") => Q5K,
        (10..=13, "attn_output") => Q6K,
        (10..=13, "ffn_gate_up") => Q5K,
        (14..=15, "attn_q" | "attn_v" | "attn_output") => Q5K,
        (14..=15, "attn_k") => Q4K,
        (_, "ffn_gate_up" | "ffn_down") => Q4K,
        _ => return None,
    })
}

/// gocr 언어의 민감층 보호 혼합: 주의 q · k · v · o 와 경계 층(0 · 15)은 Q8_0, 가운데 층 MLP 는 gate · up Q4_K · down Q5_K
fn gocr_spec_text(name: &str) -> Option<GgmlDType> {
    let (i, role) = block_role(name)?;
    Some(match (i, role) {
        (0 | 15, _) => Q8_0,
        (_, "attn_q" | "attn_k" | "attn_v" | "attn_output") => Q8_0,
        (_, "ffn_gate_up") => Q4K,
        (_, "ffn_down") => Q5K,
        _ => return None,
    })
}

/// gocr 비전: gravity-ocr 측정 배분에 관문 보호를 더한 것. 16 ~ 22 층(가장 민감, q4 ΔNLL 0.0012 ~ 0.005)은 주의 Q8_0 · MLP Q6_K,
/// downsample · merger(언어로 넘어가는 관문)는 `gate` (None 이면 원본 BF16 그대로). q3 자리는 Q4_K.
fn gocr_vision_with(name: &str, gate: Option<GgmlDType>) -> Option<GgmlDType> {
    if name == "model.visual.downsample.weight" || (name.starts_with("model.visual.merger.") && name.ends_with("proj.weight")) {
        return gate;
    }
    let (i, role) = vision_role(name)?;
    let kind = match role {
        "attn.qkv" => 0,
        "attn.proj" => 1,
        "mlp.gate_proj" | "mlp.up_proj" => 2,
        "mlp.down_proj" => 3,
        _ => return None,
    };
    let row: [GgmlDType; 4] = match i {
        0 => [Q4K, Q8_0, Q4K, Q4K],
        1..=7 => [Q4K, Q4K, Q4K, Q4K],
        8..=15 => [Q5K, Q5K, Q5K, Q6K],
        16..=22 => [Q8_0, Q8_0, Q6K, Q6K],
        _ => [Q5K, Q8_0, Q4K, Q6K],
    };
    Some(row[kind])
}

/// 비전 피라미드, 관문 Q8_0
fn gocr_vision(name: &str) -> Option<GgmlDType> {
    gocr_vision_with(name, Some(Q8_0))
}

/// 비전의 선형 가중치 전부(블록 · downsample · merger)를 `t` 로 (정규화 · 편향 · 패치 임베딩은 그대로)
fn gocr_vision_all(name: &str, t: GgmlDType) -> Option<GgmlDType> {
    let gate = name == "model.visual.downsample.weight" || (name.starts_with("model.visual.merger.") && name.ends_with("proj.weight"));
    let block = vision_role(name).is_some_and(|(_, role)| matches!(role, "attn.qkv" | "attn.proj" | "mlp.gate_proj" | "mlp.up_proj" | "mlp.down_proj"));
    (gate || block).then_some(t)
}

/// 비전 전부 Q8_0 (BF16 의 절반 크기)
fn gocr_vision_q8(name: &str) -> Option<GgmlDType> {
    gocr_vision_all(name, Q8_0)
}

/// 비전 Q6_K, 16 ~ 22 층과 관문은 Q8_0
fn gocr_vision_q6(name: &str) -> Option<GgmlDType> {
    let protected = vision_role(name).is_some_and(|(i, _)| (16..=22).contains(&i)) || !name.starts_with("model.visual.blocks.");
    gocr_vision_all(name, if protected { Q8_0 } else { Q6K })
}

/// 비전 피라미드, 관문은 원본 BF16 그대로
fn gocr_vision_bf16_gate(name: &str) -> Option<GgmlDType> {
    gocr_vision_with(name, None)
}

fn keep(_: &str) -> Option<GgmlDType> {
    None
}

/// gocr 언어 가중치를 원본 그대로 F32 로 (양자화하지 않음). ROCm(gfx1032)에서는 F32 일반 행렬곱이 Q8_0 커널보다 빨라서
/// 정확도와 속도가 함께 오릅니다 (VRAM 은 늘어남, `docs/verification.md` 15절).
fn gocr_text_f32(name: &str) -> Option<GgmlDType> {
    let (_, role) = block_role(name)?;
    matches!(role, "attn_q" | "attn_k" | "attn_v" | "attn_output" | "ffn_gate_up" | "ffn_down").then_some(GgmlDType::F32)
}

/// 출력 투영 · 입력 임베딩까지 F32
fn gocr_text_f32_all(name: &str) -> Option<GgmlDType> {
    if name == "output.weight" || name == "token_embd.weight" {
        return Some(GgmlDType::F32);
    }
    gocr_text_f32(name)
}

/// gocr 출력 투영: 16,384 행 (gravity-ocr 슬라이스 v4 와 같은 크기), 남긴 행 Q5_K · 잘린 행 Q4_K (gravity-ocr 패키지와 같은 구성)
pub const GOCR_HEAD: HeadPlan = HeadPlan { budget: 16_384, kept: Q5K, rest: Q4K };
/// gocr 출력 투영을 정밀도 손실 없이 자르기만: 두 부분 모두 Q8_0
pub const GOCR_HEAD_Q8: HeadPlan = HeadPlan { budget: 16_384, kept: Q8_0, rest: Q8_0 };

/// gocr 배분: 비전만 피라미드(gravity-ocr 측정 배분 + 16 ~ 22 층 · 관문 보호), 언어는 q8 그대로 (비교용, 정확도 손실로 기본 아님)
pub const GOCR_VISION: Plan = Plan { name: "vision", text: keep, vision: gocr_vision, head: None };

/// 이름으로 고르는 gocr 배분 (`examples/quant_screen.rs` · `examples/pyramid.rs`). 앞에 `txt:` · `vis:` 를 붙이면 그 부분만 바꿉니다 (quant_screen).
pub fn gocr_plan(name: &str) -> Option<Plan> {
    let p = |name, text: Rule, vision: Rule, head| Plan { name, text, vision, head };
    Some(match name {
        "q8" => p("q8", keep, keep, None),
        "vision" => GOCR_VISION,
        "vision-bf16gate" => p("vision-bf16gate", keep, gocr_vision_bf16_gate, None),
        "vision-q8" => p("vision-q8", keep, gocr_vision_q8, None),
        "vision-q6" => p("vision-q6", keep, gocr_vision_q6, None),
        "vision+head" => p("vision+head", keep, gocr_vision, Some(GOCR_HEAD_Q8)),
        "vision-q8+head" => p("vision-q8+head", keep, gocr_vision_q8, Some(GOCR_HEAD_Q8)),
        "vision-q6+head" => p("vision-q6+head", keep, gocr_vision_q6, Some(GOCR_HEAD_Q8)),
        "head" => p("head", keep, keep, Some(GOCR_HEAD_Q8)),
        "f32" => p("f32", gocr_text_f32_all, keep, None),
        "f32+vision" => p("f32+vision", gocr_text_f32_all, gocr_vision, None),
        "f32+vision-q8" => p("f32+vision-q8", gocr_text_f32_all, gocr_vision_q8, None),
        "f32+vision-q6" => p("f32+vision-q6", gocr_text_f32_all, gocr_vision_q6, None),
        "measured" => p("measured", gocr_measured_text, gocr_vision, Some(GOCR_HEAD)),
        "measured-fullhead" => p("measured-fullhead", gocr_measured_text, gocr_vision, None),
        "spec" => p("spec", gocr_spec_text, gocr_vision, None),
        _ => return None,
    })
}

/// Zoro-1 배분의 손잡이. 층은 0 ~ 31 (선형 주의 24 · 전체 주의 8), 판단(`jev_head`)은 마지막 층 출력의 은닉 상태를 씁니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ZoroSpec {
    /// 처음 이만큼의 층은 `edge` 형식 (FFN 포함)
    pub first: usize,
    /// 마지막 이만큼의 층은 `edge` 형식 (판단과 바로 이어지는 층)
    pub last: usize,
    pub edge: GgmlDType,
    /// 가운데 층의 주의 · 선형 주의 투영 (전체 주의 q · k · v · o, 선형 주의 qkv · 게이트 z · 출력)
    pub attn: GgmlDType,
    /// 가운데 층의 FFN gate · up 과 down
    pub ffn_in: GgmlDType,
    pub ffn_out: GgmlDType,
}

/// 지시문의 표: 마지막 층과 주의 · 선형 주의 게이트는 Q8_0 · Q6_K 로 보호, 가운데 층 FFN 은 Q4_K · Q5_K 혼합
pub const ZORO_SPEC: ZoroSpec = ZoroSpec { first: 4, last: 4, edge: Q8_0, attn: Q6K, ffn_in: Q4K, ffn_out: Q5K };

/// Zoro-1 텐서 하나의 형식 (`spec` 대로). 선형 주의의 작은 텐서(ssm_alpha · ssm_beta · conv1d · 정규화)는 저장된 그대로입니다.
pub fn zoro_type(spec: ZoroSpec, layers: usize, name: &str) -> Option<GgmlDType> {
    let (i, role) = block_role(name)?;
    let edge = i < spec.first || i + spec.last >= layers;
    match role {
        "ffn_gate" | "ffn_up" => Some(if edge { spec.edge } else { spec.ffn_in }),
        "ffn_down" => Some(if edge { spec.edge } else { spec.ffn_out }),
        "attn_q" | "attn_k" | "attn_v" | "attn_output" | "attn_qkv" | "attn_gate" | "ssm_out" => Some(if edge { spec.edge } else { spec.attn }),
        _ => None,
    }
}

/// Zoro-1 의 층 수 (GGUF `zoro.block_count`)
pub const ZORO_LAYERS: usize = 32;

fn zoro_spec_text(name: &str) -> Option<GgmlDType> {
    zoro_type(ZORO_SPEC, ZORO_LAYERS, name)
}

/// Zoro-1 출력 투영 (묶인 임베딩 248,078 행): 남긴 행 32,768 (Q8_0) · 잘린 행 Q4_K. 입력 임베딩은 파일에서 행만 읽으므로 그대로입니다.
pub const ZORO_HEAD: HeadPlan = HeadPlan { budget: 32_768, kept: Q8_0, rest: Q4K };
/// Zoro-1 출력 투영을 정밀도 손실 없이 자르기만 (두 부분 모두 q8 파일의 Q8_0 행 그대로)
pub const ZORO_HEAD_Q8: HeadPlan = HeadPlan { budget: 32_768, kept: Q8_0, rest: Q8_0 };

/// 이름으로 고르는 Zoro-1 배분. 비전(mmproj)은 원본 BF16 그대로 둡니다 (Zoro-1 비전은 다국어 판독 경로에서만 올라옴).
pub fn zoro_plan(name: &str) -> Option<Plan> {
    let p = |name, text: Rule, head| Plan { name, text, vision: keep, head };
    Some(match name {
        "q8" => p("q8", keep, None),
        "zoro-spec" => p("zoro-spec", zoro_spec_text, None),
        "zoro-spec+head" => p("zoro-spec+head", zoro_spec_text, Some(ZORO_HEAD)),
        "q8-head" => p("q8-head", keep, Some(ZORO_HEAD_Q8)),
        _ => return None,
    })
}

/// 출력 투영 잘라 두기의 도메인 글: 두 도메인의 원장 · 시나리오 · 절차서 지시문. `multilingual` 이면 12개 언어 검색 낱말 · 다국어 원장 · 영어 화면 글도.
pub fn corpus(multilingual: bool) -> Vec<String> {
    use crate::domain::DomainMode;
    let mut texts: Vec<&str> = Vec::new();
    for mode in [DomainMode::PowerPlant, DomainMode::AiDataCenter] {
        texts.push(mode.seed_json());
        texts.push(mode.scenarios_json());
    }
    if multilingual {
        texts.push(include_str!("../search/lexicon_ml.json"));
        texts.push(include_str!("../../fixtures/search_ml/synthetic_db.en.json"));
        texts.push(include_str!("../../fixtures/search_ml/synthetic_db.ja.json"));
        texts.push(include_str!("../../fixtures/search_ml/synthetic_db.de.json"));
        texts.push(include_str!("../../../app/src/i18n/en.json"));
    }
    let mut out = domain_corpus(&texts);
    if multilingual {
        for mode in [DomainMode::PowerPlant, DomainMode::AiDataCenter] {
            out.push(crate::i18n::drafting_system_en(mode).to_string());
        }
        out.push(crate::domain::Terms::default().drafting_system());
        out.push(super::pointer::SYSTEM_PROMPT.to_string());
    }
    out
}

// ---------------------------------------------------------------------------------------------
// 출력 투영 잘라 두기 (gravity-ocr `surgery.py` 슬라이스 v4 를 옮김)
// ---------------------------------------------------------------------------------------------

/// 도면 · 원장에 나오는 영어 (계기 · 설비 · 표제란 · 주기). 대문자 · 첫 글자만 대문자 · 소문자, 앞에 띄어쓰기 있고 없고를 모두 넣습니다.
const ENGLISH: &str = "pressure temperature flow level valve pump motor control gauge transmitter indicator switch alarm high low normal \
open close closed inlet outlet drain vent supply return cooling water steam oil air gas fuel tank vessel heat exchanger \
compressor fan filter strainer check gate globe ball butterfly safety relief isolation bypass sample analyzer conductivity \
differential position limit speed vibration current voltage power frequency breaker transformer panel cabinet cable tray \
local remote auto manual start stop run trip reset emergency standby duty main auxiliary boiler turbine generator condenser \
deaerator feedwater chiller cooler heater humidity damper duct rack server battery rectifier inverter drawing revision \
checked approved designed scale sheet title project note notes detail section elevation plan view typical reference \
specification material quantity description remarks size rating class schedule insulation painting welding support hanger \
flange elbow reducer nozzle orifice venturi thermowell controller recorder totalizer solenoid actuator positioner \
interlock sequence logic signal input output analog digital setpoint override permissive shutdown startup maintenance \
inspection test calibration operation instrument instrumentation electrical mechanical piping civil structural architectural \
ventilation fire protection sprinkler hydrant detector smoke lighting grounding earthing lightning \
substation switchgear busbar feeder load capacity efficiency demand consumption meter metering redundancy tower \
containment aisle hot cold raised floor ceiling distribution unit precision humidifier dehumidifier makeup blowdown chemical \
dosing injection treatment softener demineralizer condensate deaeration economizer superheater reheater attemperator drum";

const UNITS: [&str; 31] = [
    "mm", "mm/s", "m/s", "m³/h", "t/h", "kg/h", "bar", "kPa", "MPa", "kV", "kW", "MW", "A", "V", "Hz", "rpm", "℃", "°C", "%", "ppm", "μm", "Ω", "kΩ",
    "Nm³/h", "L/min", "dB", "lux", "kVA", "MVA", "pH", "μS/cm",
];

const TABLE_TAGS: [&str; 16] = [
    "<table>", "</table>", "<tr>", "</tr>", "<td>", "</td>", "</td><td>", "</td></tr>", "<tr><td>", "</td></tr><tr><td>", "</td></tr></table>", "<th>",
    "</th>", "<br>", "<td colspan=\"2\">", "<td rowspan=\"2\">",
];

/// 원장 · 시나리오 JSON 의 모든 글 + 도면 영어 · 단위 · 날짜 · 표 태그 (출력 투영 잘라 두기의 도메인 글)
pub fn domain_corpus(json_texts: &[&str]) -> Vec<String> {
    fn walk(v: &serde_json::Value, out: &mut Vec<String>) {
        match v {
            serde_json::Value::String(s) => out.push(s.clone()),
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            serde_json::Value::Object(o) => o.iter().for_each(|(k, x)| {
                out.push(k.clone());
                walk(x, out)
            }),
            serde_json::Value::Number(n) => out.push(n.to_string()),
            _ => {}
        }
    }
    let mut out = Vec::new();
    for text in json_texts {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
            walk(&v, &mut out);
        }
    }
    for w in ENGLISH.split_whitespace() {
        let title = w[..1].to_uppercase() + &w[1..];
        for f in [w.to_uppercase(), title, w.to_string()] {
            out.push(format!(" {f}"));
            out.push(f);
        }
    }
    out.extend(TABLE_TAGS.iter().map(|s| s.to_string()));
    // 고정 시드의 값 · 날짜 (gravity-ocr 와 같은 모양)
    let mut seed = 7u64;
    let mut next = || {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        (seed >> 33) as f64 / (1u64 << 31) as f64
    };
    for u in UNITS {
        for _ in 0..4 {
            let v = next() * 2000.0;
            out.push(format!("{} {u}", thousands(v)));
            out.push(format!("{v:.2}{u}"));
        }
    }
    for _ in 0..200 {
        let (y, m, d) = (2015 + (next() * 16.0) as u32, 1 + (next() * 12.0) as u32, 1 + (next() * 28.0) as u32);
        out.push(format!("{y}-{m:02}-{d:02}"));
        out.push(format!("{y}.{m:02}.{d:02}"));
        out.push(format!("Rev.{}", (next() * 9.0) as u32));
    }
    out
}

/// `1,234.5` 꼴
fn thousands(v: f64) -> String {
    let s = format!("{v:.1}");
    let (int, frac) = s.split_once('.').unwrap_or((&s, "0"));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(c);
    }
    format!("{grouped}.{frac}")
}

/// 토큰 종류 (slice v4 의 `token_class`)
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TokenClass {
    Special,
    /// 혼자서는 UTF-8 이 아닌 바이트 조각 (한글 한 글자를 이루는 조각들)
    Partial,
    Hangul,
    Html,
    Symbol,
    Greek,
    Ascii,
    Cjk,
    Other,
}

fn script(c: char) -> u8 {
    let o = c as u32;
    if o < 128 {
        0
    } else if (0xAC00..=0xD7A3).contains(&o) || (0x1100..=0x11FF).contains(&o) || (0x3130..=0x318F).contains(&o) || (0xA960..=0xA97F).contains(&o) || (0xD7B0..=0xD7FF).contains(&o) {
        1
    } else if (0x4E00..=0x9FFF).contains(&o) || (0x3400..=0x4DBF).contains(&o) || (0xF900..=0xFAFF).contains(&o) {
        2
    } else if (0x0370..=0x03FF).contains(&o) {
        3
    } else if (0x2000..=0x2BFF).contains(&o) || (0x3200..=0x33FF).contains(&o) || (0x3000..=0x303F).contains(&o) || (0xFF00..=0xFFEF).contains(&o) || (0x00A0..=0x00BF).contains(&o) {
        4
    } else {
        5
    }
}

fn is_html(s: &str) -> bool {
    let t = s.trim();
    if t == ">" || t == "/>" {
        return true;
    }
    ["table", "thead", "tbody", "tr", "td", "th", "br", "caption"].iter().any(|tag| {
        ["<", "</"].iter().any(|open| {
            s.match_indices(&format!("{open}{tag}")).any(|(i, m)| {
                let after = s[i + m.len()..].chars().next();
                after.is_none_or(|c| !(c.is_ascii_alphanumeric() || c == '_'))
            })
        })
    })
}

pub fn token_class(piece: &[u8], special: bool) -> TokenClass {
    if special {
        return TokenClass::Special;
    }
    let Ok(s) = std::str::from_utf8(piece) else { return TokenClass::Partial };
    if s.is_empty() {
        return TokenClass::Other;
    }
    let kinds: BTreeSet<u8> = s.chars().map(script).collect();
    let within = |allowed: &[u8]| kinds.iter().all(|k| allowed.contains(k));
    if kinds.contains(&1) {
        TokenClass::Hangul
    } else if within(&[0]) {
        if is_html(s) { TokenClass::Html } else { TokenClass::Ascii }
    } else if within(&[0, 4]) {
        TokenClass::Symbol
    } else if within(&[0, 4, 3]) {
        TokenClass::Greek
    } else if within(&[0, 4, 2]) {
        TokenClass::Cjk
    } else {
        TokenClass::Other
    }
}

/// ` ?[A-Z0-9]+` (설비 코드 · 대문자 영어 조각)
fn caps_piece(b: &[u8]) -> bool {
    let body = b.strip_prefix(b" ").unwrap_or(b);
    !body.is_empty() && body.iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

/// `[0-9 \n.,:;\-/()%+=#&*'"!?]+` 이면서 띄어쓰기 셋이 이어지지 않음 (본문 · 숫자의 구두점)
fn text_punct(b: &[u8]) -> bool {
    !b.is_empty() && b.iter().all(|c| c.is_ascii_digit() || b" \n.,:;-/()%+=#&*'\"!?".contains(c)) && !b.windows(3).any(|w| w == b"   ")
}

/// ` ?[A-Za-z]+` (영어 낱말 조각)
fn word_piece(b: &[u8]) -> bool {
    let body = b.strip_prefix(b" ").unwrap_or(b);
    !body.is_empty() && body.iter().all(|c| c.is_ascii_alphabetic())
}

/// 잘라 둔 출력 투영의 남긴 행
#[derive(Debug, Clone, PartialEq)]
pub struct HeadSlice {
    /// 남긴 행의 전체 어휘 번호 (오름차순). `ids[행]` 이 그 행의 토큰입니다.
    pub ids: Vec<u32>,
    /// 단계마다 더한 수 (must · domain · cjk · words · ascii · rest)
    pub tiers: Vec<(&'static str, usize)>,
}

impl HeadSlice {
    /// 남기지 않은 번호 (오름차순): 잘린 행 `output_rest.weight` 의 순서
    pub fn rest(&self, vocab: usize) -> Vec<u32> {
        let mut keep = self.ids.iter().peekable();
        (0..vocab as u32)
            .filter(|i| {
                if keep.peek() == Some(&i) {
                    keep.next();
                    false
                } else {
                    true
                }
            })
            .collect()
    }
}

/// 우선순위로 남길 행을 고릅니다: 반드시 남길 것(특수 · 한글 · 바이트 조각 · 표 태그 · 기호 · 그리스 · 한 바이트 · 대문자 · 숫자 조각 · 구두점) →
/// 도메인 글이 쓰는 토큰 → 자주 쓰는 한자 `cjk_fill` 개 → 영어 낱말 조각 → 나머지 ASCII → 나머지, 각 단계 안은 번호 순(BPE 병합 순 = 빈도 순).
pub fn slice_head(vocab: usize, tokenizer: &ModelTokenizer, corpus: &[String], budget: usize, cjk_fill: usize) -> Result<HeadSlice> {
    if vocab < budget {
        bail!("어휘 {vocab} 가 남길 행 {budget} 보다 작습니다");
    }
    let pieces: Vec<(Vec<u8>, TokenClass)> = (0..vocab as u32)
        .into_par_iter()
        .map(|i| {
            let special = tokenizer.is_added(i);
            let piece = tokenizer.token_bytes(i).unwrap_or_default();
            let class = token_class(&piece, special);
            (piece, class)
        })
        .collect();
    let mut keep: BTreeSet<u32> = BTreeSet::new();
    let mut tiers = Vec::new();
    let mut add = |name: &'static str, ids: Vec<u32>, keep: &mut BTreeSet<u32>| {
        let before = keep.len();
        for i in ids {
            if keep.len() >= budget {
                break;
            }
            keep.insert(i);
        }
        tiers.push((name, keep.len() - before));
    };
    use TokenClass::*;
    let ids = |f: &dyn Fn(u32, &[u8], TokenClass) -> bool| -> Vec<u32> { (0..vocab as u32).filter(|&i| f(i, &pieces[i as usize].0, pieces[i as usize].1)).collect() };
    let must = ids(&|_, p, c| {
        matches!(c, Special | Hangul | Partial | Html | Symbol | Greek) || p.len() == 1 || (c == Ascii && (caps_piece(p) || text_punct(p)))
    });
    add("must", must, &mut keep);
    let mut seen: BTreeSet<u32> = BTreeSet::new();
    for text in corpus {
        seen.extend(tokenizer.encode(text)?.ids.into_iter().filter(|&i| (i as usize) < vocab));
    }
    add("domain", seen.into_iter().collect(), &mut keep);
    let cjk: Vec<u32> = ids(&|i, _, c| c == Cjk && !keep.contains(&i)).into_iter().take(cjk_fill).collect();
    add("cjk", cjk, &mut keep);
    let words = ids(&|i, p, c| c == Ascii && !keep.contains(&i) && word_piece(p));
    add("words", words, &mut keep);
    let ascii = ids(&|i, _, c| c == Ascii && !keep.contains(&i));
    add("ascii", ascii, &mut keep);
    let rest = ids(&|i, _, c| !keep.contains(&i) && matches!(c, Cjk | Other));
    add("rest", rest, &mut keep);
    if keep.len() < budget {
        bail!("남길 행 {budget} 개를 다 채우지 못했습니다 ({}개)", keep.len());
    }
    Ok(HeadSlice { ids: keep.into_iter().collect(), tiers })
}

/// 글들의 토큰 가운데 남긴 행에 든 몫과, 잘린 행에 든 토큰 (빈도순 앞 20개)
pub fn coverage(tokenizer: &ModelTokenizer, slice: &HeadSlice, texts: &[String]) -> Result<(usize, usize, Vec<(String, usize)>)> {
    let kept: std::collections::HashSet<u32> = slice.ids.iter().copied().collect();
    let (mut total, mut missing) = (0usize, 0usize);
    let mut lost: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
    for t in texts {
        for id in tokenizer.encode(t)?.ids {
            total += 1;
            if !kept.contains(&id) {
                missing += 1;
                *lost.entry(id).or_default() += 1;
            }
        }
    }
    let mut lost: Vec<(String, usize)> = lost.into_iter().map(|(id, n)| (String::from_utf8_lossy(&tokenizer.token_bytes(id).unwrap_or_default()).to_string(), n)).collect();
    lost.sort_by(|a, b| b.1.cmp(&a.1));
    lost.truncate(20);
    Ok((total, missing, lost))
}

// ---------------------------------------------------------------------------------------------
// 파일 만들기
// ---------------------------------------------------------------------------------------------

/// 만든 파일 하나
#[derive(Debug, Clone)]
pub struct Built {
    pub path: PathBuf,
    pub bytes: u64,
    /// 형식별 텐서 바이트 (장치에 올라가는 2차원 가중치 기준)
    pub by_type: Vec<(String, u64)>,
}

/// 잘라 둘 출력 투영 텐서: gocr 는 따로 둔 `output.weight`, zoro 는 묶인 임베딩 `token_embd.weight`
pub fn head_source(model: &str) -> &'static str {
    if model == "zoro" {
        "token_embd.weight"
    } else {
        "output.weight"
    }
}

/// 한 파일의 텐서 목록을 배분대로 만듭니다. 바꾸는 텐서는 원본(`original`, 없으면 원본 그대로 저장된 값)에서 한 번 양자화하고,
/// 나머지는 저장된 바이트 그대로 옮깁니다. `head` 가 있으면 `head_source` 텐서를 남긴 행 · 잘린 행으로 나눕니다.
/// `imatrix` 가 있으면 K 형식을 그 열 가중으로 양자화합니다.
fn plan_file(src: &Arc<WeightFile>, original: Option<&Arc<Original>>, rule: Rule, head: Option<(&str, &HeadSlice, HeadPlan, usize)>, imatrix: Option<&Arc<Imatrix>>) -> Result<Vec<OutTensor>> {
    let mut out = Vec::new();
    let weights_of = |name: &str, cols: usize| -> Option<Vec<f32>> { imatrix.and_then(|m| m.get(name).cloned()).filter(|w| w.len() == cols) };
    for name in src.names() {
        let meta = src.info(&name)?;
        let (rows, cols) = rows_cols(&meta.dims);
        if let Some((head_source, slice, plan, vocab)) = head {
            if name == head_source {
                // 출력 투영을 따로 둔 모델(gocr)은 그 자리에, 묶인 임베딩 모델(zoro)은 따로 둔 출력 투영 묶음(`Group::Head`)으로 씁니다.
                let (kept_name, rest_name) = if head_source == "output.weight" {
                    ("output.weight", super::residency::OUTPUT_REST)
                } else {
                    (super::residency::HEAD_KEPT, super::residency::HEAD_REST)
                };
                for (part, ids, dtype) in [(kept_name, slice.ids.clone(), plan.kept), (rest_name, slice.rest(vocab), plan.rest)] {
                    let (f, o, n, w) = (Arc::clone(src), original.cloned(), name.clone(), weights_of(&name, cols));
                    out.push(if dtype == meta.dtype {
                        // 저장과 같은 형식이면 저장된 행의 바이트를 그대로 옮깁니다 (다시 양자화하지 않아 원본 없이도 만듦).
                        OutTensor::new(part, vec![ids.len(), cols], dtype, move || stored_rows_bytes(&f, &n, &ids))
                    } else {
                        OutTensor::new(part, vec![ids.len(), cols], dtype, move || quantize_rows_of(&f, &n, o.as_deref(), &ids, dtype, w.as_deref()))
                    });
                }
                if head_source != "output.weight" {
                    // 묶인 임베딩(zoro): 입력 임베딩은 파일에서 행만 읽으므로 그대로 둡니다.
                    let (f, n) = (Arc::clone(src), name.clone());
                    out.push(OutTensor::new(name.clone(), meta.dims.clone(), meta.dtype, move || Ok(f.raw(&n)?.to_vec())));
                }
                continue;
            }
        }
        // 저장 형식과 같은 형식(q8 파일의 Q8_0 → Q8_0)은 저장된 바이트 그대로 둡니다 (원본에서 한 번 양자화한 그 값, 올릴 때 양자화와 같은 규칙).
        let target = rule(&name).filter(|t| *t != meta.dtype && meta.dims.len() >= 2 && cols.is_multiple_of(t.block_size()));
        let (f, n) = (Arc::clone(src), name.clone());
        out.push(match target {
            // 4차원 합성곱(downsample)은 선형 가중치와 같아서 [출력, 입력 × 2 × 2] 로 펴서 둡니다 (gocr 비전의 PREPARE 와 같은 모양).
            Some(t) => {
                let (o, w) = (original.cloned(), weights_of(&name, cols));
                OutTensor::new(name.clone(), vec![rows, cols], t, move || quantize_tensor(&f, &n, o.as_deref(), t, w.as_deref()))
            }
            None => OutTensor::new(name.clone(), meta.dims.clone(), meta.dtype, move || Ok(f.raw(&n)?.to_vec())),
        });
    }
    Ok(out)
}

/// 저장된 텐서의 `ids` 번 행 바이트를 그대로 이어 붙입니다 (형식을 바꾸지 않는 행 고르기)
fn stored_rows_bytes(file: &WeightFile, name: &str, ids: &[u32]) -> Result<Vec<u8>> {
    let meta = file.info(name)?;
    let (rows, cols) = rows_cols(&meta.dims);
    let row_bytes = cols / meta.dtype.block_size() * meta.dtype.type_size();
    let raw = file.raw(name)?;
    let mut out = Vec::with_capacity(ids.len() * row_bytes);
    for &r in ids {
        let r = r as usize;
        if r >= rows {
            bail!("{name}: 행 {r} 이 범위 밖입니다 (행 수 {rows})");
        }
        out.extend_from_slice(&raw[r * row_bytes..(r + 1) * row_bytes]);
    }
    Ok(out)
}

fn by_type(tensors: &[OutTensor]) -> Vec<(String, u64)> {
    let mut m: std::collections::BTreeMap<String, u64> = std::collections::BTreeMap::new();
    for t in tensors {
        *m.entry(format!("{:?}", t.dtype)).or_default() += t.bytes();
    }
    m.into_iter().collect()
}

fn metadata_with(src: &WeightFile, extra: Vec<(String, Value)>) -> Vec<(String, Value)> {
    let keys: BTreeSet<String> = extra.iter().map(|(k, _)| k.clone()).collect();
    let mut meta: Vec<(String, Value)> = src.metadata_entries().into_iter().filter(|(k, _)| !keys.contains(k)).collect();
    meta.extend(extra);
    meta.sort_by(|a, b| a.0.cmp(&b.0));
    meta
}

/// 배분이 바꾸는 텐서 이름들 (`rule` 이 저장과 다른 형식을 주는 텐서와 잘라 둘 출력 투영)
fn changed(file: &WeightFile, rule: Rule, head: Option<&str>) -> Vec<String> {
    file.names().into_iter().filter(|n| rule(n).is_some_and(|t| file.info(n).is_ok_and(|m| m.dtype != t)) || Some(n.as_str()) == head).collect()
}

/// 바꾸는 텐서의 재료를 쓰기 전에 확인합니다. 원본이 있으면 대응표가 맞는지(몇 행의 값 비교, Q8_0 저장이면 그 오차 이내),
/// 없으면 저장 형식이 원본 그대로(BF16 · F16 · F32)인지. 돌려주는 값: (원본에서 읽는 텐서 수, 저장값에서 읽는 수, 대응 확인의 최대 상대오차)
pub fn check_material(file: &WeightFile, original: Option<&Original>, names: &[String]) -> Result<(usize, usize, f32)> {
    let (mut from_original, mut from_stored, mut worst) = (0, 0, 0f32);
    for name in names {
        match original.filter(|o| o.has(name)) {
            Some(o) => {
                worst = worst.max(o.check(file, name, 0.03)?);
                from_original += 1;
            }
            None => {
                let meta = file.info(name)?;
                if !meta.is_float() {
                    bail!("{name}: 원본 BF16 이 없어 양자화하지 않습니다 (저장된 {:?} 를 다시 양자화하지 않음, 모델 폴더의 {}/ 에 원본을 두세요)", meta.dtype, super::original::DIR);
                }
                from_stored += 1;
            }
        }
    }
    Ok((from_original, from_stored, worst))
}

fn read_record(model_dir: &Path) -> Result<serde_json::Value> {
    let path = model_dir.join("active.json");
    Ok(serde_json::from_str(&std::fs::read_to_string(&path).with_context(|| format!("{} 를 읽을 수 없습니다", path.display()))?)?)
}

fn record_file(record: &serde_json::Value) -> Result<String> {
    Ok(record["file"].as_str().ok_or_else(|| anyhow!("active.json 에 file 이 없습니다"))?.to_string())
}

/// 출력 투영 잘라 두기가 다시 양자화를 하는가 (남긴 행 · 잘린 행 중 저장과 다른 형식이 있으면). 같은 형식이면 행만 옮깁니다.
fn head_requantizes(file: &WeightFile, model: &str, hp: &HeadPlan) -> bool {
    file.info(head_source(model)).map_or(true, |m| hp.kept != m.dtype || hp.rest != m.dtype)
}

/// 이 배분을 만들려면 원본 BF16 (safetensors) 이 꼭 있어야 하는가: 바꾸는 텐서 중 저장 형식이 양자화된 것이 있으면 그렇습니다.
/// 비전만 바꾸는 배분은 BF16 으로 저장된 mmproj 에서, Q8_0 그대로 행만 나누는 출력 투영은 q8 파일에서 만들 수 있습니다.
pub fn needs_original(model_dir: &Path, model: &str, plan: &Plan) -> Result<bool> {
    let record = read_record(model_dir)?;
    let text = WeightFile::open(&model_dir.join(record_file(&record)?))?;
    let head = plan.head.filter(|hp| head_requantizes(&text, model, hp)).map(|_| head_source(model));
    let quantized = |f: &WeightFile, names: Vec<String>| names.iter().any(|n| f.info(n).is_ok_and(|m| !m.is_float()));
    let mut any = quantized(&text, changed(&text, plan.text, head));
    if let Some(m) = record["mmproj"].as_str() {
        let vision = WeightFile::open(&model_dir.join(m))?;
        any |= quantized(&vision, changed(&vision, plan.vision, None));
    }
    Ok(any)
}

/// 피라미드 파일을 만듭니다: 모델 폴더의 q8 파일 · mmproj 가 구조 · 메타데이터 · 바꾸지 않는 텐서를, 원본 BF16 (`bf16/`) 이 바꾸는 텐서의 재료를 줍니다.
/// `out_dir` 이 없으면 모델 폴더에 쓰고 `active.json` 의 `pyramid` 에 적습니다 (있으면 그 폴더에 쓰기만, 측정용).
/// `corpus` 는 잘라 둘 출력 투영의 남길 행을 고르는 글, `imatrix` 가 있으면 K 형식을 그 열 가중으로 양자화합니다 (없는 텐서는 균일 가중).
pub fn build(model_dir: &Path, model: &str, plan: &Plan, corpus: &[String], imatrix: Option<Arc<Imatrix>>, out_dir: Option<&Path>, log: &mut dyn FnMut(&str)) -> Result<Vec<Built>> {
    let family = Family::of(model).ok_or_else(|| anyhow!("모르는 모델 {model}"))?;
    let mut record = read_record(model_dir)?;
    let file_name = record_file(&record)?;
    let src = Arc::new(WeightFile::open(&model_dir.join(&file_name))?);
    let original = match Original::find(model_dir, family) {
        Some(o) => Some(Arc::new(o?)),
        None => None,
    };
    let material = if original.is_some() { "bf16" } else { "stored" };
    let arch = src.meta_str("general.architecture")?.to_string();
    let t0 = Instant::now();
    let dest = out_dir.unwrap_or(model_dir);
    std::fs::create_dir_all(dest)?;
    let head_name = plan.head.filter(|hp| head_requantizes(&src, model, hp)).map(|_| head_source(model));
    let (o_n, s_n, worst) = check_material(&src, original.as_deref(), &changed(&src, plan.text, head_name))?;
    log(&format!("[피라미드] {model} {} · 언어 재료: 원본 {o_n} · 저장값 {s_n} 텐서 (원본 대응 최대 상대오차 {worst:.4})", plan.name));
    let mut extra = vec![
        (META_PLAN.to_string(), Value::String(plan.name.to_string())),
        (META_SOURCE.to_string(), Value::String(material.to_string())),
        (META_RECIPE.to_string(), Value::U32(RECIPE)),
        ("blueprint.pyramid.imatrix".to_string(), Value::U32(imatrix.as_ref().map_or(0, |m| m.len() as u32))),
    ];
    let head = match plan.head {
        Some(hp) => {
            let source = head_source(model);
            let vocab = src.info(source)?.dims[0];
            let tokenizer = ModelTokenizer::from_file(&model_dir.join("tokenizer.json"))?;
            let slice = slice_head(vocab, &tokenizer, corpus, hp.budget, 1024)?;
            let (total, missing, _) = coverage(&tokenizer, &slice, corpus)?;
            log(&format!("[피라미드] 출력 투영 {vocab} 행 → 남김 {} ({:?}), 도메인 글 토큰 {total} 중 잘린 행 {missing}", slice.ids.len(), slice.tiers));
            extra.push((format!("{arch}.head.ids"), Value::Array(slice.ids.iter().map(|&i| Value::U32(i)).collect())));
            Some((source, slice, hp, vocab))
        }
        None => None,
    };
    let stem = file_name.trim_end_matches(".gguf").replace("-q8", "");
    let mut built = Vec::new();
    // 언어를 그대로 두는 배분(비전만 바꿈)은 q8 파일을 그대로 씁니다 (같은 텐서를 한 번 더 쓰지 않음).
    let text_file = if o_n + s_n == 0 && head.is_none() {
        log(&format!("[피라미드] 언어는 그대로입니다 ({file_name})"));
        file_name.clone()
    } else {
        let tensors = plan_file(&src, original.as_ref(), plan.text, head.as_ref().map(|(s, sl, hp, v)| (*s, sl, *hp, *v)), imatrix.as_ref())?;
        let types = by_type(&tensors);
        let path = dest.join(format!("{stem}-pyramid.gguf"));
        log(&format!("[피라미드] {} 쓰는 중 ({} 텐서)", path.display(), tensors.len()));
        let bytes = write_gguf(&path, &metadata_with(&src, extra.clone()), tensors)?;
        built.push(Built { path: path.clone(), bytes, by_type: types });
        path.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default()
    };
    let mut mmproj_name = record["mmproj"].as_str().map(str::to_string);
    if let Some(m) = mmproj_name.clone() {
        let vision = Arc::new(WeightFile::open(&model_dir.join(&m))?);
        let names = changed(&vision, plan.vision, None);
        if names.is_empty() {
            // 비전을 그대로 두는 배분이면 원래 mmproj 를 같이 씁니다 (같은 파일을 두 번 쓰지 않음).
            log(&format!("[피라미드] 비전은 그대로입니다 ({m})"));
        } else {
            let (o_n, s_n, worst) = check_material(&vision, original.as_deref(), &names)?;
            log(&format!("[피라미드] 비전 재료: 원본 {o_n} · 저장값(BF16) {s_n} 텐서 (원본 대응 최대 상대오차 {worst:.4})"));
            let tensors = plan_file(&vision, original.as_ref(), plan.vision, None, imatrix.as_ref())?;
            let types = by_type(&tensors);
            let name = m.replace("-bf16", "-pyramid");
            let path = dest.join(&name);
            log(&format!("[피라미드] {} 쓰는 중 ({} 텐서)", path.display(), tensors.len()));
            let meta = metadata_with(
                &vision,
                vec![
                    (META_PLAN.to_string(), Value::String(plan.name.to_string())),
                    (META_SOURCE.to_string(), Value::String(material.to_string())),
                    (META_RECIPE.to_string(), Value::U32(RECIPE)),
                ],
            );
            let bytes = write_gguf(&path, &meta, tensors)?;
            built.push(Built { path, bytes, by_type: types });
            mmproj_name = Some(name);
        }
    }
    if out_dir.is_none() {
        record["pyramid"] = json!({
            "plan": plan.name,
            "file": text_file,
            "mmproj": mmproj_name,
            "source": file_name,
            "material": material,
            "recipe": RECIPE,
            "seconds": (t0.elapsed().as_secs_f64() * 10.0).round() / 10.0,
        });
        std::fs::write(model_dir.join("active.json"), serde_json::to_string_pretty(&record)?)?;
    }
    Ok(built)
}

/// 측정 기준 파일: 원본 BF16 을 F32 로 푼 값을 그대로 둔 GGUF (`<out>/<model>-f32.gguf`, 비전이 있으면 `<out>/mmproj-<model>-f32.gguf`).
/// F32 로 두는 까닭: 엔진은 BF16 · F16 일반 가중치를 ROCm 에서 BF16 행렬곱(활성값도 BF16)으로 곱하고, F32 로 저장된 텐서는 F32 로 곱합니다.
/// 원본에 있는 텐서는 원본에서(모양이 다르면 거부), 원본에 없는 텐서는 저장된 그대로 옮깁니다.
pub fn reference(model_dir: &Path, model: &str, out_dir: &Path, log: &mut dyn FnMut(&str)) -> Result<Vec<Built>> {
    let family = Family::of(model).ok_or_else(|| anyhow!("모르는 모델 {model}"))?;
    let original = Arc::new(Original::find(model_dir, family).ok_or_else(|| anyhow!("{} 에 원본 BF16 이 없습니다", model_dir.join(super::original::DIR).display()))??);
    let record = read_record(model_dir)?;
    std::fs::create_dir_all(out_dir)?;
    let mut files = vec![(record_file(&record)?, format!("{model}-f32.gguf"))];
    if let Some(m) = record["mmproj"].as_str() {
        files.push((m.to_string(), format!("mmproj-{model}-f32.gguf")));
    }
    let mut built = Vec::new();
    for (from, to) in files {
        let src = Arc::new(WeightFile::open(&model_dir.join(&from))?);
        let mut tensors = Vec::new();
        let (mut mapped, mut kept) = (0usize, 0usize);
        for name in src.names() {
            let meta = src.info(&name)?;
            let (rows, cols) = rows_cols(&meta.dims);
            if original.has(&name) {
                if rows_cols(&original.shape(&name)?) != (rows, cols) {
                    bail!("{name}: 원본 모양 {:?} 가 파일의 {:?} 와 다릅니다", original.shape(&name)?, meta.dims);
                }
                let (o, n) = (Arc::clone(&original), name.clone());
                tensors.push(OutTensor::new(name.clone(), meta.dims.clone(), GgmlDType::F32, move || kquant::encode(&o.rows(&n, 0, rows)?, cols, GgmlDType::F32, None)));
                mapped += 1;
            } else {
                let (f, n) = (Arc::clone(&src), name.clone());
                tensors.push(OutTensor::new(name.clone(), meta.dims.clone(), meta.dtype, move || Ok(f.raw(&n)?.to_vec())));
                kept += 1;
            }
        }
        let path = out_dir.join(&to);
        log(&format!("[기준] {} · 원본 F32 {mapped} · 그대로 {kept} 텐서", path.display()));
        let types = by_type(&tensors);
        let meta = metadata_with(&src, vec![(META_PLAN.to_string(), Value::String("f32-reference".into())), (META_SOURCE.to_string(), Value::String("bf16".into()))]);
        let bytes = write_gguf(&path, &meta, tensors)?;
        built.push(Built { path, bytes, by_type: types });
    }
    Ok(built)
}

/// 원본과 지금 파일들의 대응 감사: 텐서마다 (이름, 저장 형식, 상대오차, 같은가). 같은가는 BF16 · F16 · F32 이면 원본과 비트까지 같은지,
/// Q8_0 이면 원본을 GGML 기준(블록 최댓값 / 127, candle `from_float`)으로 양자화한 바이트와 같은지입니다 (원본에서 한 번 양자화한 파일인지).
/// 텐서마다 앞 · 가운데 · 끝 8행만 봅니다.
pub fn audit(model_dir: &Path, model: &str) -> Result<Vec<(String, String, f32, bool)>> {
    let family = Family::of(model).ok_or_else(|| anyhow!("모르는 모델 {model}"))?;
    let original = Original::find(model_dir, family).ok_or_else(|| anyhow!("{} 에 원본 BF16 이 없습니다", model_dir.join(super::original::DIR).display()))??;
    let record = read_record(model_dir)?;
    let mut files = vec![record_file(&record)?];
    files.extend(record["mmproj"].as_str().map(str::to_string));
    let mut out = Vec::new();
    for f in files {
        let file = WeightFile::open(&model_dir.join(&f))?;
        for name in file.names() {
            if !original.has(&name) {
                out.push((name, "원본 없음".into(), f32::NAN, false));
                continue;
            }
            let meta = file.info(&name)?;
            let err = original.check(&file, &name, f32::INFINITY)?;
            let same = if meta.is_float() {
                // BF16 은 비트까지, F32 (원본 BF16 을 넓힌 정규화 등)는 값이 같은지
                original.identical(&file, &name)? || err == 0.0
            } else if meta.dtype == GgmlDType::Q8_0 {
                let (rows, cols) = rows_cols(&meta.dims);
                let mut ids: Vec<u32> = (0..8).map(|k| (k * (rows - 1) / 7) as u32).collect();
                ids.dedup();
                let xs = original.gather(&name, &ids)?;
                let mut q = GgmlDType::Q8_0.cpu_zeros(xs.len());
                q.from_float(&xs);
                let ours = unsafe { std::slice::from_raw_parts(q.as_ptr(), q.storage_size_in_bytes()) };
                let row_bytes = cols / 32 * 34;
                let raw = file.raw(&name)?;
                ids.iter().enumerate().all(|(k, &r)| ours[k * row_bytes..(k + 1) * row_bytes] == raw[r as usize * row_bytes..(r as usize + 1) * row_bytes])
            } else {
                false
            };
            out.push((name, format!("{:?}", meta.dtype), err, same));
        }
    }
    Ok(out)
}

/// 앱이 쓰는 배분 (측정으로 고른 것, `docs/verification.md` 15절).
/// gocr: 비전 전부 Q8_0 (원본 BF16 mmproj 에서 한 번) + 출력 투영 잘라 두기 (Q8_0 행 그대로). 정답 기준 파이프라인이 main 과 모두 같고
/// 문서 판독 −11.8 %, 최대 VRAM 828 → 590 MB. 언어 K 형식 · 비전 피라미드는 정확도를 잃어 쓰지 않습니다.
pub const DEFAULT_GOCR_PLAN: &str = "vision-q8+head";
pub const DEFAULT_ZORO_PLAN: &str = "q8";

/// 기본 배분을 바꾸는 환경 변수 (예: `BLUEPRINT_GOCR_PLAN=vision+head` 면 비전 피라미드). 모르는 이름이면 기본을 씁니다.
pub const GOCR_PLAN_ENV: &str = "BLUEPRINT_GOCR_PLAN";
pub const ZORO_PLAN_ENV: &str = "BLUEPRINT_ZORO_PLAN";

/// 모델의 기본 배분 이름 (`BLUEPRINT_GOCR_PLAN` · `BLUEPRINT_ZORO_PLAN` 이 있으면 그 배분)
pub fn default_plan(model: &str) -> &'static str {
    let (env, fallback, known): (&str, &'static str, fn(&str) -> Option<Plan>) =
        if model == "zoro" { (ZORO_PLAN_ENV, DEFAULT_ZORO_PLAN, zoro_plan) } else { (GOCR_PLAN_ENV, DEFAULT_GOCR_PLAN, gocr_plan) };
    match std::env::var(env).ok().and_then(|v| known(v.trim())) {
        Some(p) => p.name,
        None => fallback,
    }
}

/// 모델 폴더의 피라미드 파일이 지금 배분 · 지금 레시피로 만들어져 있는가
pub fn current(model_dir: &Path, plan: &str) -> bool {
    let Ok(record) = read_record(model_dir) else { return false };
    let p = &record["pyramid"];
    p["plan"].as_str() == Some(plan) && p["recipe"].as_u64() == Some(RECIPE as u64) && p["file"].as_str().is_some_and(|f| model_dir.join(f).exists())
}

/// 사용할 때 양자화: 이 장치가 피라미드 파일을 쓰는데(`wanted`) 기본 배분 · 레시피의 파일이 없으면 만듭니다 (CPU).
/// 앱이 모델을 열기 전에 부릅니다. 원본 BF16 이 꼭 필요한 배분인데 원본이 없으면 만들지 않고 q8 를 씁니다.
/// `BLUEPRINT_PYRAMID=1` 이면 만들지 않고 모델 폴더에 있는 피라미드 파일을 그대로 씁니다 (측정에서 배분을 고정할 때).
/// 중요도 행렬은 모델 폴더의 `imatrix.bin` 이 있으면 씁니다.
pub fn ensure(models_dir: &Path, device: &Device, log: &mut dyn FnMut(&str)) -> Result<()> {
    let pinned = std::env::var(PYRAMID_ENV).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "on"));
    if !wanted(device) || pinned {
        return Ok(());
    }
    for model in ["gocr", "zoro"] {
        let plan_name = default_plan(model);
        let dir = models_dir.join(model);
        if plan_name == "q8" || !dir.join("active.json").exists() || current(&dir, plan_name) {
            continue;
        }
        let plan = if model == "gocr" { gocr_plan(plan_name) } else { zoro_plan(plan_name) }.ok_or_else(|| anyhow!("모르는 배분 {plan_name}"))?;
        let family = Family::of(model).ok_or_else(|| anyhow!("모르는 모델 {model}"))?;
        if needs_original(&dir, model, &plan)? && Original::find(&dir, family).is_none() {
            log(&format!("[피라미드] {model} {plan_name}: 원본 BF16 ({}/) 이 없어 만들지 않습니다 (q8 그대로)", super::original::DIR));
            continue;
        }
        let imatrix = dir.join("imatrix.bin");
        let imatrix = if imatrix.exists() { Some(Arc::new(load_imatrix(&imatrix)?)) } else { None };
        log(&format!("[피라미드] {model} 파일을 만듭니다 ({plan_name}, 레시피 {RECIPE})"));
        build(&dir, model, &plan, &corpus(model == "zoro"), imatrix, None, log)?;
    }
    Ok(())
}

/// 피라미드 파일을 지우고 `active.json` 에서 뺍니다 (q8 로 돌아감).
pub fn remove(model_dir: &Path) -> Result<()> {
    let record_path = model_dir.join("active.json");
    let mut record = read_record(model_dir)?;
    if let Some(p) = record.get("pyramid").cloned() {
        // 원래 파일(q8 · 비전을 그대로 둔 mmproj)은 지우지 않습니다.
        let originals: Vec<&str> = ["file", "mmproj"].iter().filter_map(|k| record[*k].as_str()).collect();
        for key in ["file", "mmproj"] {
            if let Some(f) = p[key].as_str().filter(|f| !originals.contains(f)) {
                let path = model_dir.join(f);
                if path.exists() {
                    std::fs::remove_file(&path).with_context(|| format!("{} 를 지울 수 없습니다 (앱이 열고 있으면 끄고 다시 하세요)", path.display()))?;
                }
            }
        }
        record.as_object_mut().map(|o| o.remove("pyramid"));
        std::fs::write(&record_path, serde_json::to_string_pretty(&record)?)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::original::testing::write_st;
    use crate::ai::verify::det_normal;
    use crate::ai::weights::testing::{write_gguf as write_test_gguf, TempDir};
    use candle_core::Tensor;

    /// q8 파일(언어 Q8_0 · 정규화 F32 · 비전 BF16) + 원본 BF16 (`bf16/model.safetensors`)
    fn sample(dir: &TempDir) -> (Arc<WeightFile>, Arc<Original>, Vec<f32>) {
        // 원본은 BF16 이므로 BF16 으로 반올림한 값이 원본 값입니다.
        let bf = |v: Vec<f32>, s: f32| -> Vec<f32> { v.iter().map(|x| half::bf16::from_f32(x * s).to_f32()).collect() };
        let q = bf(det_normal(16 * 512, 1), 0.02);
        let head = bf(det_normal(10 * 256, 4), 0.05);
        let t = |v: &[f32], rows: usize, cols: usize| Tensor::from_vec(v.to_vec(), (rows, cols), &Device::Cpu).unwrap();
        let path = dir.0.join("src.gguf");
        write_test_gguf(
            &path,
            &[("general.architecture", Value::String("gocr".into())), ("zeze.quant", Value::String("Q8_0".into()))],
            &[
                ("blk.0.attn_q.weight".into(), t(&q, 16, 512), GgmlDType::Q8_0),
                ("blk.0.attn_norm.weight".into(), Tensor::from_vec(det_normal(512, 2), 512, &Device::Cpu).unwrap(), GgmlDType::F32),
                ("model.visual.blocks.0.mlp.down_proj.weight".into(), t(&det_normal(8 * 256, 3), 8, 256), GgmlDType::BF16),
                ("output.weight".into(), t(&head, 10, 256), GgmlDType::Q8_0),
            ],
        )
        .unwrap();
        std::fs::create_dir_all(dir.0.join("bf16")).unwrap();
        write_st(&dir.0.join("bf16/model.safetensors"), &[("model.language_model.layers.0.self_attn.q_proj.weight", vec![16, 512], q.clone()), ("lm_head.weight", vec![10, 256], head)]);
        let original = Original::find(&dir.0, Family::Gocr).unwrap().unwrap();
        (Arc::new(WeightFile::open(&path).unwrap()), Arc::new(original), q)
    }

    #[test]
    fn changed_tensors_come_from_the_original_once_and_the_rest_is_kept_bit_for_bit() -> Result<()> {
        let dir = TempDir::new("pyr-write");
        let (src, original, q) = sample(&dir);
        let rule: Rule = |n| (n == "blk.0.attn_q.weight").then_some(Q4K).or((n == "model.visual.blocks.0.mlp.down_proj.weight").then_some(Q6K));
        let tensors = plan_file(&src, Some(&original), rule, None, None)?;
        let out = dir.0.join("out.gguf");
        write_gguf(&out, &metadata_with(&src, vec![(META_PLAN.into(), Value::String("t".into()))]), tensors)?;
        let f = WeightFile::open(&out)?;
        assert_eq!(f.meta_str(META_PLAN)?, "t");
        assert_eq!(f.meta_str("zeze.quant")?, "Q8_0", "원래 메타데이터");
        assert_eq!(f.info("blk.0.attn_q.weight")?.dtype, Q4K);
        assert_eq!(f.info("model.visual.blocks.0.mlp.down_proj.weight")?.dtype, Q6K);
        assert_eq!(f.raw("blk.0.attn_norm.weight")?, src.raw("blk.0.attn_norm.weight")?, "정규화는 그대로");
        assert_eq!(f.raw("output.weight")?, src.raw("output.weight")?);
        // Q4_K 는 q8 파일의 Q8_0 이 아니라 원본 값을 한 번 양자화한 바이트입니다.
        assert_eq!(f.raw("blk.0.attn_q.weight")?, kquant::encode(&q, 512, Q4K, None)?.as_slice());
        let back = stored_gather(&f, "blk.0.attn_q.weight", &(0..16).collect::<Vec<u32>>())?;
        assert!(kquant::rel(&back, &q) < 0.08);
        // BF16 으로 저장된 비전 텐서는 원본이 없어도 그 값 그대로가 재료입니다.
        let bf16 = stored_gather(&src, "model.visual.blocks.0.mlp.down_proj.weight", &(0..8).collect::<Vec<u32>>())?;
        assert_eq!(f.raw("model.visual.blocks.0.mlp.down_proj.weight")?, kquant::encode(&bf16, 256, Q6K, None)?.as_slice());
        Ok(())
    }

    #[test]
    fn a_quantized_tensor_is_never_quantized_again_without_its_original() -> Result<()> {
        let dir = TempDir::new("pyr-refuse");
        let (src, original, _) = sample(&dir);
        let err = material_rows(&src, "blk.0.attn_q.weight", None, 0, 1).unwrap_err().to_string();
        assert!(err.contains("다시 양자화하지 않습니다"), "{err}");
        assert!(material_rows(&src, "blk.0.attn_q.weight", Some(&original), 0, 1).is_ok());
        assert!(material_rows(&src, "model.visual.blocks.0.mlp.down_proj.weight", None, 0, 1).is_ok(), "BF16 저장값은 원본 그대로");
        assert!(check_material(&src, None, &["blk.0.attn_q.weight".into()]).is_err());
        let (o, s, worst) = check_material(&src, Some(&original), &["blk.0.attn_q.weight".into(), "model.visual.blocks.0.mlp.down_proj.weight".into()])?;
        assert_eq!((o, s), (1, 1));
        assert!(worst > 0.0 && worst < 0.01, "Q8_0 저장값과 원본의 차이 {worst}");
        Ok(())
    }

    #[test]
    fn a_wrong_mapping_is_caught_before_writing() -> Result<()> {
        let dir = TempDir::new("pyr-map");
        let (src, _, q) = sample(&dir);
        // 행 순서를 뒤집은 원본: 모양은 같지만 값이 다릅니다.
        let flipped: Vec<f32> = q.chunks_exact(512).rev().flatten().copied().collect();
        write_st(&dir.0.join("bf16/model.safetensors"), &[("model.language_model.layers.0.self_attn.q_proj.weight", vec![16, 512], flipped)]);
        let original = Original::find(&dir.0, Family::Gocr).unwrap()?;
        assert!(check_material(&src, Some(&original), &["blk.0.attn_q.weight".into()]).is_err());
        Ok(())
    }

    #[test]
    fn head_rows_split_into_kept_and_rest_in_id_order() -> Result<()> {
        let dir = TempDir::new("pyr-head");
        let (src, original, _) = sample(&dir);
        let slice = HeadSlice { ids: vec![1, 4, 7], tiers: vec![] };
        assert_eq!(slice.rest(10), vec![0, 2, 3, 5, 6, 8, 9]);
        let hp = HeadPlan { budget: 3, kept: Q8_0, rest: Q4K };
        let tensors = plan_file(&src, Some(&original), keep, Some(("output.weight", &slice, hp, 10)), None)?;
        let out = dir.0.join("head.gguf");
        write_gguf(&out, &metadata_with(&src, vec![]), tensors)?;
        let f = WeightFile::open(&out)?;
        assert_eq!(f.info("output.weight")?.dims, vec![3, 256]);
        assert_eq!(f.info(crate::ai::residency::OUTPUT_REST)?.dims, vec![7, 256]);
        let kept = stored_gather(&f, "output.weight", &[0, 1, 2])?;
        let want = original.gather("output.weight", &[1, 4, 7])?;
        assert!(kquant::rel(&kept, &want) < 0.01, "남긴 행은 원본 행의 Q8_0");
        // 저장과 같은 형식(Q8_0)인 남긴 행은 다시 양자화하지 않고 저장된 행의 바이트를 그대로 옮깁니다.
        assert_eq!(f.raw("output.weight")?, stored_rows_bytes(&src, "output.weight", &[1, 4, 7])?.as_slice());
        // 원본이 없어도 Q8_0 그대로 나누기는 됩니다 (잘린 행도 Q8_0 일 때).
        let same = HeadPlan { budget: 3, kept: Q8_0, rest: Q8_0 };
        let tensors = plan_file(&src, None, keep, Some(("output.weight", &slice, same, 10)), None)?;
        let out = dir.0.join("head_q8.gguf");
        write_gguf(&out, &metadata_with(&src, vec![]), tensors)?;
        let g = WeightFile::open(&out)?;
        assert_eq!(g.raw(crate::ai::residency::OUTPUT_REST)?, stored_rows_bytes(&src, "output.weight", &slice.rest(10))?.as_slice());
        assert!(!head_requantizes(&src, "gocr", &same) && head_requantizes(&src, "gocr", &hp));
        Ok(())
    }

    #[test]
    fn token_classes_follow_slice_v4() {
        assert_eq!(token_class("한".as_bytes(), false), TokenClass::Hangul);
        assert_eq!(token_class(&[0xEC, 0x9E], false), TokenClass::Partial);
        assert_eq!(token_class(b"</td>", false), TokenClass::Html);
        assert_eq!(token_class(b"<tdx", false), TokenClass::Ascii);
        assert_eq!(token_class(b" PRESS", false), TokenClass::Ascii);
        assert!(caps_piece(b" PRESS") && caps_piece(b"10LAB") && !caps_piece(b"Press"));
        assert_eq!(token_class("μm".as_bytes(), false), TokenClass::Greek);
        assert_eq!(token_class("℃".as_bytes(), false), TokenClass::Symbol);
        assert_eq!(token_class("給水".as_bytes(), false), TokenClass::Cjk);
        assert_eq!(token_class("Привет".as_bytes(), false), TokenClass::Other);
        assert!(text_punct(b", ") && !text_punct(b"   ") && word_piece(b" valve") && !word_piece(b"v2"));
        assert_eq!(thousands(1234.56), "1,234.6");
    }

    #[test]
    fn plans_cover_the_roles_they_name() {
        assert_eq!(gocr_measured_text("blk.0.attn_output.weight"), Some(Q8_0));
        assert_eq!(gocr_measured_text("blk.3.attn_q.weight"), Some(Q4K), "nf4 → Q4_K");
        assert_eq!(gocr_measured_text("blk.0.attn_q.weight"), Some(Q4K), "q3 → Q4_K");
        assert_eq!(gocr_measured_text("blk.12.ffn_gate_up.weight"), Some(Q5K));
        assert_eq!(gocr_measured_text("blk.12.ffn_norm.weight"), None, "정규화는 그대로");
        for i in 16..=22 {
            assert_eq!(gocr_vision(&format!("model.visual.blocks.{i}.attn.qkv.weight")), Some(Q8_0), "16 ~ 22 층 보호");
            assert_eq!(gocr_vision(&format!("model.visual.blocks.{i}.mlp.gate_proj.weight")), Some(Q6K));
        }
        assert_eq!(gocr_vision("model.visual.blocks.3.mlp.gate_proj.weight"), Some(Q4K));
        assert_eq!(gocr_vision("model.visual.blocks.18.attn.qkv.bias"), None);
        assert_eq!(gocr_vision("model.visual.downsample.weight"), Some(Q8_0), "관문");
        assert_eq!(gocr_vision("model.visual.merger.gate_proj.weight"), Some(Q8_0), "관문");
        assert_eq!(gocr_vision_bf16_gate("model.visual.merger.proj.weight"), None, "관문은 원본 BF16");
        assert_eq!(gocr_vision("model.visual.merger.post_projection_norm.weight"), None);
        assert_eq!(gocr_vision("model.visual.patch_embed.proj.weight"), None, "합성곱 패치 임베딩은 그대로");
        assert_eq!(gocr_spec_text("blk.0.ffn_down.weight"), Some(Q8_0), "경계 층");
        assert_eq!(gocr_spec_text("blk.7.ffn_gate_up.weight"), Some(Q4K));
        assert_eq!(gocr_vision_q8("model.visual.blocks.3.mlp.gate_proj.weight"), Some(Q8_0));
        assert_eq!(gocr_vision_q8("model.visual.merger.proj.weight"), Some(Q8_0));
        assert_eq!(gocr_vision_q8("model.visual.blocks.3.norm1.weight"), None);
        assert_eq!(gocr_vision_q6("model.visual.blocks.3.attn.qkv.weight"), Some(Q6K));
        assert_eq!(gocr_vision_q6("model.visual.blocks.18.attn.qkv.weight"), Some(Q8_0));
        assert_eq!(gocr_vision_q6("model.visual.downsample.weight"), Some(Q8_0));
        assert_eq!(gocr_text_f32_all("blk.4.ffn_down.weight"), Some(GgmlDType::F32));
        assert_eq!(gocr_text_f32_all("output.weight"), Some(GgmlDType::F32));
        assert_eq!(gocr_text_f32_all("blk.4.ffn_norm.weight"), None, "정규화는 이미 F32");
        for name in ["q8", "vision", "vision-bf16gate", "vision-q8", "vision-q6", "vision+head", "vision-q8+head", "vision-q6+head", "head", "measured", "measured-fullhead", "spec", "f32", "f32+vision", "f32+vision-q8", "f32+vision-q6"] {
            assert_eq!(gocr_plan(name).map(|p| p.name), Some(name));
        }
        assert!(gocr_plan("gocr-measured").is_none(), "q8 파일에서 다시 양자화하던 옛 배분 이름은 없음");
        for name in ["q8", "zoro-spec", "zoro-spec+head", "q8-head"] {
            assert_eq!(zoro_plan(name).map(|p| p.name), Some(name));
        }
        assert_eq!(gocr_plan(DEFAULT_GOCR_PLAN).map(|p| p.name), Some(DEFAULT_GOCR_PLAN));
        assert_eq!(zoro_plan(DEFAULT_ZORO_PLAN).map(|p| p.name), Some(DEFAULT_ZORO_PLAN));
    }
}
