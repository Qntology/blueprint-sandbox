//! 원본 가중치: Hugging Face 에서 받은 safetensors (BF16). 피라미드 양자화는 이 값에서 **한 번만** 양자화합니다.
//! 이미 양자화된 q8 파일(Q8_0)을 풀어 다시 양자화하지 않습니다 (두 번 양자화한 오차가 겹침).
//!
//! - 위치: 모델 폴더의 `bf16/` 에 `model.safetensors` 하나, 또는 `model.safetensors.index.json` 과 조각들 ([`DIR`]).
//!   gocr 는 trillionlabs/GravityOCR-KO, zoro 는 trillionlabs/Zoro-1 의 파일 그대로입니다.
//! - GGUF 텐서 이름 → 원본 이름은 모델마다의 대응표([`Family`])로 바꿉니다. 대응이 맞는지는 지금 쓰는 파일(q8 · mmproj)의 같은 텐서와
//!   값을 견줘 확인합니다 ([`Original::check`]): 저장 형식이 Q8_0 이면 그 양자화 오차(약 0.4 %), BF16 이면 0 이어야 합니다.
//! - safetensors 는 머리말(JSON)만 해석하고 값은 메모리 매핑에서 행 단위로 읽습니다. 범위를 벗어나는 오프셋은 거부합니다.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};

use super::kquant;
use super::weights::WeightFile;

/// 모델 폴더 안의 원본 폴더 이름
pub const DIR: &str = "bf16";

/// 원본 텐서의 저장 형식
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Dtype {
    BF16,
    F16,
    F32,
}

impl Dtype {
    fn size(self) -> usize {
        match self {
            Dtype::BF16 | Dtype::F16 => 2,
            Dtype::F32 => 4,
        }
    }
}

struct Entry {
    shard: usize,
    dtype: Dtype,
    shape: Vec<usize>,
    begin: usize,
    end: usize,
}

/// 어느 모델의 대응표를 쓰는지
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// GravityOCR-KO (`GlmOcrForConditionalGeneration`)
    Gocr,
    /// Zoro-1 (Qwen3.5-4B 바탕)
    Zoro,
}

impl Family {
    /// 모델 폴더 이름으로
    pub fn of(model: &str) -> Option<Self> {
        match model {
            "gocr" => Some(Self::Gocr),
            "zoro" => Some(Self::Zoro),
            _ => None,
        }
    }

    /// GGUF 텐서 이름 → 원본 safetensors 이름
    pub fn source_name(self, gguf: &str) -> Option<String> {
        match self {
            Family::Gocr => gocr_name(gguf),
            Family::Zoro => zoro_name(gguf),
        }
    }
}

/// `blk.<층>.<역할>.weight` → (층, 역할)
fn block_role(name: &str) -> Option<(usize, &str)> {
    let rest = name.strip_prefix("blk.")?;
    let (layer, tail) = rest.split_once('.')?;
    Some((layer.parse().ok()?, tail.strip_suffix(".weight")?))
}

/// gocr: 언어는 `model.language_model.*`, 비전(mmproj)은 원본 이름 그대로 (`patch_embed` 만 5차원 → 4차원으로 모양만 바뀜)
fn gocr_name(name: &str) -> Option<String> {
    if name.starts_with("model.visual.") {
        return Some(name.to_string());
    }
    let lm = "model.language_model";
    match name {
        "token_embd.weight" => return Some(format!("{lm}.embed_tokens.weight")),
        "output.weight" => return Some("lm_head.weight".into()),
        "output_norm.weight" => return Some(format!("{lm}.norm.weight")),
        _ => {}
    }
    let (i, role) = block_role(name)?;
    let part = match role {
        "attn_q" => "self_attn.q_proj",
        "attn_k" => "self_attn.k_proj",
        "attn_v" => "self_attn.v_proj",
        "attn_output" => "self_attn.o_proj",
        "ffn_gate_up" => "mlp.gate_up_proj",
        "ffn_down" => "mlp.down_proj",
        "attn_norm" => "input_layernorm",
        "post_attn_norm" => "post_self_attn_layernorm",
        "ffn_norm" => "post_attention_layernorm",
        "post_ffn_norm" => "post_mlp_layernorm",
        _ => return None,
    };
    Some(format!("{lm}.layers.{i}.{part}.weight"))
}

/// zoro (Qwen3.5-4B 바탕, `Qwen3_5ForConditionalGeneration`): 층 32개 = 선형 주의(Gated DeltaNet) 24 · 전체 주의 8 (4층마다).
/// 출력 투영은 입력 임베딩과 묶여 있습니다 (`tie_word_embeddings`). 비전(mmproj)은 원본 이름 그대로입니다.
/// 대응은 2차원 가중치와 정규화까지 적습니다. `ssm_a`(= −exp(A_log)) 처럼 값을 바꿔 저장한 텐서는 적지 않습니다 (양자화하지 않음).
fn zoro_name(name: &str) -> Option<String> {
    if name.starts_with("model.visual.") {
        return Some(name.to_string());
    }
    let lm = "model.language_model";
    match name {
        "token_embd.weight" => return Some(format!("{lm}.embed_tokens.weight")),
        "output_norm.weight" => return Some(format!("{lm}.norm.weight")),
        _ => {}
    }
    let (i, role) = block_role(name)?;
    let part = match role {
        "attn_norm" => "input_layernorm",
        "post_attention_norm" => "post_attention_layernorm",
        "ffn_gate" => "mlp.gate_proj",
        "ffn_up" => "mlp.up_proj",
        "ffn_down" => "mlp.down_proj",
        // 전체 주의 (q 투영은 질의와 출력 게이트를 함께 냄)
        "attn_q" => "self_attn.q_proj",
        "attn_k" => "self_attn.k_proj",
        "attn_v" => "self_attn.v_proj",
        "attn_output" => "self_attn.o_proj",
        "attn_q_norm" => "self_attn.q_norm",
        "attn_k_norm" => "self_attn.k_norm",
        // 선형 주의
        "attn_qkv" => "linear_attn.in_proj_qkv",
        "attn_gate" => "linear_attn.in_proj_z",
        "ssm_alpha" => "linear_attn.in_proj_a",
        "ssm_beta" => "linear_attn.in_proj_b",
        "ssm_out" => "linear_attn.out_proj",
        "ssm_conv1d" => "linear_attn.conv1d",
        "ssm_norm" => "linear_attn.norm",
        _ => return None,
    };
    Some(format!("{lm}.layers.{i}.{part}.weight"))
}

/// 원본 safetensors 묶음 (조각 여러 개면 함께)
pub struct Original {
    dir: PathBuf,
    family: Family,
    shards: Vec<memmap2::Mmap>,
    entries: HashMap<String, Entry>,
}

impl Original {
    /// `dir` 의 safetensors 를 엽니다 (`model.safetensors.index.json` 이 있으면 그 조각들).
    pub fn open(dir: &Path, family: Family) -> Result<Self> {
        let index = dir.join("model.safetensors.index.json");
        let files: Vec<PathBuf> = if index.exists() {
            let v: serde_json::Value = serde_json::from_str(&fs::read_to_string(&index)?).with_context(|| format!("{} 를 해석할 수 없습니다", index.display()))?;
            let map = v["weight_map"].as_object().ok_or_else(|| anyhow!("{}: weight_map 이 없습니다", index.display()))?;
            let mut names: Vec<&str> = map.values().filter_map(|x| x.as_str()).collect();
            names.sort();
            names.dedup();
            for n in &names {
                // 받은 파일이므로 폴더 밖을 가리키는 이름은 거부합니다.
                if n.contains(['/', '\\']) || n.contains("..") || !n.ends_with(".safetensors") {
                    bail!("{}: 조각 이름 {n:?} 를 쓰지 않습니다", index.display());
                }
            }
            names.iter().map(|n| dir.join(n)).collect()
        } else {
            vec![dir.join("model.safetensors")]
        };
        let mut shards = Vec::with_capacity(files.len());
        let mut entries = HashMap::new();
        for (k, path) in files.iter().enumerate() {
            let file = fs::File::open(path).with_context(|| format!("{} 를 열 수 없습니다", path.display()))?;
            let map = unsafe { memmap2::Mmap::map(&file)? };
            for (name, e) in parse_header(&map, k).with_context(|| format!("{} 머리말", path.display()))? {
                entries.insert(name, e);
            }
            shards.push(map);
        }
        Ok(Self { dir: dir.to_path_buf(), family, shards, entries })
    }

    /// 모델 폴더의 `bf16/` 에 원본이 있으면 엽니다.
    pub fn find(model_dir: &Path, family: Family) -> Option<Result<Self>> {
        let dir = model_dir.join(DIR);
        let present = dir.join("model.safetensors").exists() || dir.join("model.safetensors.index.json").exists();
        present.then(|| Self::open(&dir, family))
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn family(&self) -> Family {
        self.family
    }

    /// 원본 텐서 이름들
    pub fn names(&self) -> Vec<&str> {
        let mut v: Vec<&str> = self.entries.keys().map(String::as_str).collect();
        v.sort();
        v
    }

    fn entry(&self, gguf: &str) -> Result<(&Entry, String)> {
        let name = self.family.source_name(gguf).ok_or_else(|| anyhow!("{gguf}: 원본 대응표에 없는 텐서입니다"))?;
        let e = self.entries.get(&name).ok_or_else(|| anyhow!("{gguf}: 원본에 {name} 이 없습니다 ({})", self.dir.display()))?;
        Ok((e, name))
    }

    /// 이 GGUF 텐서의 원본이 있는가
    pub fn has(&self, gguf: &str) -> bool {
        self.entry(gguf).is_ok()
    }

    /// 원본 모양 (safetensors 순서, 첫 차원이 가장 느림)
    pub fn shape(&self, gguf: &str) -> Result<Vec<usize>> {
        Ok(self.entry(gguf)?.0.shape.clone())
    }

    fn row_bytes(e: &Entry) -> (usize, usize, usize) {
        let (rows, cols) = rows_cols(&e.shape);
        (rows, cols, cols * e.dtype.size())
    }

    fn decode(e: &Entry, bytes: &[u8]) -> Vec<f32> {
        match e.dtype {
            Dtype::BF16 => bytes.chunks_exact(2).map(|b| f32::from_bits((u16::from_le_bytes([b[0], b[1]]) as u32) << 16)).collect(),
            Dtype::F16 => bytes.chunks_exact(2).map(|b| half::f16::from_bits(u16::from_le_bytes([b[0], b[1]])).to_f32()).collect(),
            Dtype::F32 => bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(),
        }
    }

    /// `[행, 열]` 보기(첫 차원이 행, 나머지를 편 것이 열)의 행 `[start, start + count)` 를 F32 로
    pub fn rows(&self, gguf: &str, start: usize, count: usize) -> Result<Vec<f32>> {
        let (e, name) = self.entry(gguf)?;
        let (rows, _, rb) = Self::row_bytes(e);
        if start + count > rows {
            bail!("{name}: 행 {start}..{} 이 행 수 {rows} 를 넘습니다", start + count);
        }
        let data = &self.shards[e.shard][e.begin..e.end];
        Ok(Self::decode(e, &data[start * rb..(start + count) * rb]))
    }

    /// `ids` 번 행만 F32 로 (순서대로 이어 붙임)
    pub fn gather(&self, gguf: &str, ids: &[u32]) -> Result<Vec<f32>> {
        let (e, name) = self.entry(gguf)?;
        let (rows, cols, rb) = Self::row_bytes(e);
        let data = &self.shards[e.shard][e.begin..e.end];
        let mut out = Vec::with_capacity(ids.len() * cols);
        for &r in ids {
            let r = r as usize;
            if r >= rows {
                bail!("{name}: 행 {r} 이 범위 밖입니다 (행 수 {rows})");
            }
            out.extend(Self::decode(e, &data[r * rb..(r + 1) * rb]));
        }
        Ok(out)
    }

    /// 원본과 `file` 의 같은 텐서를 견줍니다: 행 · 열이 같고 몇 행의 상대오차가 `tol` 이하여야 합니다 (대응표 확인).
    /// 돌려주는 값은 그 상대오차입니다 (Q8_0 이면 양자화 오차, BF16 이면 0).
    pub fn check(&self, file: &WeightFile, gguf: &str, tol: f32) -> Result<f32> {
        let meta = file.info(gguf)?;
        let (rows, cols) = rows_cols(&meta.dims);
        let (orows, ocols) = rows_cols(&self.shape(gguf)?);
        if (rows, cols) != (orows, ocols) {
            bail!("{gguf}: 원본 모양 [{orows}, {ocols}] 이 파일의 [{rows}, {cols}] 와 다릅니다");
        }
        let mut ids: Vec<u32> = (0..8).map(|k| (k * (rows - 1) / 7) as u32).collect();
        ids.dedup();
        let ours = self.gather(gguf, &ids)?;
        let theirs = stored_gather(file, gguf, &ids)?;
        let e = kquant::rel(&theirs, &ours);
        if !(e <= tol) {
            bail!("{gguf}: 원본 대응이 맞지 않습니다 (상대오차 {e:.4} > {tol})");
        }
        Ok(e)
    }

    /// 원본과 `file` 의 같은 텐서가 비트까지 같은가 (BF16 로 저장된 mmproj 가 원본 그대로인지)
    pub fn identical(&self, file: &WeightFile, gguf: &str) -> Result<bool> {
        let meta = file.info(gguf)?;
        let (e, _) = self.entry(gguf)?;
        let same_type = matches!((meta.dtype, e.dtype), (candle_core::quantized::GgmlDType::BF16, Dtype::BF16) | (candle_core::quantized::GgmlDType::F16, Dtype::F16) | (candle_core::quantized::GgmlDType::F32, Dtype::F32));
        Ok(same_type && file.raw(gguf)? == &self.shards[e.shard][e.begin..e.end])
    }
}

/// 텐서를 `[행, 열]` 로 봅니다: 첫 차원이 행, 나머지를 편 것이 열 (1차원은 한 행).
pub fn rows_cols(dims: &[usize]) -> (usize, usize) {
    match dims {
        [] => (1, 1),
        [n] => (1, *n),
        [rows, rest @ ..] => (*rows, rest.iter().product()),
    }
}

/// GGUF 에 저장된 텐서의 `ids` 번 행을 F32 로 풉니다. **대응표 확인 · 측정 전용**입니다: 양자화 재료로 쓰면 두 번 양자화가 되므로
/// 양자화는 [`Original`] 이나 원본 그대로 저장된(BF16 · F16 · F32) 텐서에서만 합니다.
pub fn stored_gather(file: &WeightFile, name: &str, ids: &[u32]) -> Result<Vec<f32>> {
    let meta = file.info(name)?;
    let (rows, cols) = rows_cols(&meta.dims);
    let row_bytes = cols / meta.dtype.block_size() * meta.dtype.type_size();
    let raw = file.raw(name)?;
    let mut bytes = Vec::with_capacity(ids.len() * row_bytes);
    for &r in ids {
        let r = r as usize;
        if r >= rows {
            bail!("{name}: 행 {r} 이 범위 밖입니다 (행 수 {rows})");
        }
        bytes.extend_from_slice(&raw[r * row_bytes..(r + 1) * row_bytes]);
    }
    kquant::decode(meta.dtype, &bytes, ids.len(), cols)
}

/// safetensors 머리말: 8 바이트 길이 + JSON `{이름: {dtype, shape, data_offsets}}`
fn parse_header(map: &[u8], shard: usize) -> Result<Vec<(String, Entry)>> {
    if map.len() < 8 {
        bail!("파일이 너무 짧습니다");
    }
    let n = u64::from_le_bytes(map[..8].try_into()?) as usize;
    let start = 8usize.checked_add(n).filter(|&s| s <= map.len()).ok_or_else(|| anyhow!("머리말 길이 {n} 이 파일보다 깁니다"))?;
    let head: serde_json::Value = serde_json::from_slice(&map[8..start])?;
    let obj = head.as_object().ok_or_else(|| anyhow!("머리말이 객체가 아닙니다"))?;
    let mut out = Vec::with_capacity(obj.len());
    for (name, v) in obj {
        if name == "__metadata__" {
            continue;
        }
        let dtype = match v["dtype"].as_str() {
            Some("BF16") => Dtype::BF16,
            Some("F16") => Dtype::F16,
            Some("F32") => Dtype::F32,
            other => bail!("{name}: 형식 {other:?} 는 읽지 않습니다"),
        };
        let shape: Vec<usize> = v["shape"].as_array().ok_or_else(|| anyhow!("{name}: shape 없음"))?.iter().map(|d| d.as_u64().map(|d| d as usize)).collect::<Option<_>>().ok_or_else(|| anyhow!("{name}: shape 가 정수가 아님"))?;
        let off = v["data_offsets"].as_array().ok_or_else(|| anyhow!("{name}: data_offsets 없음"))?;
        let (b, e) = match (off.first().and_then(|x| x.as_u64()), off.get(1).and_then(|x| x.as_u64())) {
            (Some(b), Some(e)) => (b as usize, e as usize),
            _ => bail!("{name}: data_offsets 가 두 정수가 아님"),
        };
        let elems: usize = shape.iter().product();
        let (begin, end) = (start + b, start + e);
        if b > e || end > map.len() || e - b != elems * dtype.size() {
            bail!("{name}: 데이터 범위 {b}..{e} 가 모양 {shape:?} · 파일 크기와 맞지 않습니다");
        }
        out.push((name.clone(), Entry { shard, dtype, shape, begin, end }));
    }
    Ok(out)
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::Path;

    /// 작은 safetensors 파일 (BF16 텐서들)
    pub fn write_st(path: &Path, tensors: &[(&str, Vec<usize>, Vec<f32>)]) {
        let mut head = serde_json::Map::new();
        let mut data = Vec::new();
        for (name, shape, vals) in tensors {
            let b = data.len();
            data.extend(vals.iter().flat_map(|v| half::bf16::from_f32(*v).to_bits().to_le_bytes()));
            head.insert(name.to_string(), serde_json::json!({"dtype": "BF16", "shape": shape, "data_offsets": [b, data.len()]}));
        }
        let h = serde_json::to_vec(&serde_json::Value::Object(head)).unwrap();
        let mut out = (h.len() as u64).to_le_bytes().to_vec();
        out.extend(h);
        out.extend(data);
        std::fs::write(path, out).unwrap();
    }
}

#[cfg(test)]
mod tests {
    use super::testing::write_st;
    use super::*;
    use crate::ai::weights::testing::TempDir;

    #[test]
    fn rows_and_gathers_come_from_the_mapped_tensor() -> Result<()> {
        let dir = TempDir::new("orig-rows");
        let vals: Vec<f32> = (0..24).map(|i| i as f32 * 0.5).collect();
        write_st(&dir.0.join("model.safetensors"), &[("model.language_model.layers.3.self_attn.q_proj.weight", vec![4, 6], vals.clone()), ("lm_head.weight", vec![2, 3], vec![1.0; 6])]);
        let o = Original::open(&dir.0, Family::Gocr)?;
        assert!(o.has("blk.3.attn_q.weight") && o.has("output.weight") && !o.has("blk.3.attn_k.weight"));
        assert_eq!(o.shape("blk.3.attn_q.weight")?, vec![4, 6]);
        assert_eq!(o.rows("blk.3.attn_q.weight", 1, 2)?, vals[6..18].to_vec());
        assert_eq!(o.gather("blk.3.attn_q.weight", &[3, 0])?, [&vals[18..24], &vals[0..6]].concat());
        assert!(o.rows("blk.3.attn_q.weight", 3, 2).is_err(), "행 범위 밖");
        assert!(o.gather("blk.3.attn_q.weight", &[4]).is_err());
        Ok(())
    }

    #[test]
    fn shards_are_read_through_the_index_and_bad_names_are_refused() -> Result<()> {
        let dir = TempDir::new("orig-shards");
        write_st(&dir.0.join("model-00000.safetensors"), &[("lm_head.weight", vec![2, 2], vec![1.0, 2.0, 3.0, 4.0])]);
        write_st(&dir.0.join("model-00001.safetensors"), &[("model.language_model.norm.weight", vec![2], vec![0.5, 0.25])]);
        let index = serde_json::json!({"weight_map": {"lm_head.weight": "model-00000.safetensors", "model.language_model.norm.weight": "model-00001.safetensors"}});
        fs::write(dir.0.join("model.safetensors.index.json"), index.to_string())?;
        let o = Original::open(&dir.0, Family::Gocr)?;
        assert_eq!(o.rows("output.weight", 1, 1)?, vec![3.0, 4.0]);
        assert_eq!(o.rows("output_norm.weight", 0, 1)?, vec![0.5, 0.25]);
        let bad = serde_json::json!({"weight_map": {"x": "../evil.safetensors"}});
        fs::write(dir.0.join("model.safetensors.index.json"), bad.to_string())?;
        assert!(Original::open(&dir.0, Family::Gocr).is_err(), "폴더 밖 조각 이름 거부");
        Ok(())
    }

    #[test]
    fn a_truncated_or_lying_header_is_refused() {
        let dir = TempDir::new("orig-bad");
        let h = br#"{"w":{"dtype":"BF16","shape":[4],"data_offsets":[0,64]}}"#;
        let mut out = (h.len() as u64).to_le_bytes().to_vec();
        out.extend_from_slice(h);
        out.extend([0u8; 8]);
        fs::write(dir.0.join("model.safetensors"), out).unwrap();
        assert!(Original::open(&dir.0, Family::Gocr).is_err());
    }

    #[test]
    fn gocr_names_cover_every_language_weight_and_keep_vision_names() {
        assert_eq!(gocr_name("blk.15.ffn_gate_up.weight").as_deref(), Some("model.language_model.layers.15.mlp.gate_up_proj.weight"));
        assert_eq!(gocr_name("blk.0.post_attn_norm.weight").as_deref(), Some("model.language_model.layers.0.post_self_attn_layernorm.weight"));
        assert_eq!(gocr_name("model.visual.blocks.3.attn.qkv.weight").as_deref(), Some("model.visual.blocks.3.attn.qkv.weight"));
        assert_eq!(gocr_name("blk.0.unknown.weight"), None);
    }
}
