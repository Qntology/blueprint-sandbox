//! 선택된 양자화 모델 열기 (모델 폴더의 `<모델>/active.json` → GGUF).
//!
//! 텐서 데이터는 읽지 않고, 파일 머리(메타데이터)만 읽어 구조가 맞는지 확인합니다.
//! 이 함수는 엔진이 쓸 파일 경로와 구조 값을 정하는 입구이고, 가중치는 `ai::residency` 가 단계별로 올립니다.

use std::collections::HashMap;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use candle_core::quantized::gguf_file::{Content, Value};

use super::weights::WeightFile;

#[derive(Debug, Clone, PartialEq)]
pub struct ActiveModel {
    /// 모델 폴더 이름 (예: `zoro`, `gocr`, `siglip2`)
    pub model: String,
    /// 선택된 양자화 (`q4` 또는 `q8`)
    pub quant: String,
    /// 선택된 GGUF 파일
    pub path: PathBuf,
    /// 비전 타워를 따로 둔 파일 (`mmproj-*.gguf`). 없으면 비전 타워도 `path` 에 있습니다.
    pub mmproj: Option<PathBuf>,
    /// GGUF 의 `general.architecture` (`zoro`, `gocr`, `siglip2`)
    pub architecture: String,
    pub block_count: u32,
    pub embedding_length: u32,
    pub vocab_size: u32,
}

impl ActiveModel {
    /// 비전 타워 파일을 엽니다. 매핑만 하고 텐서는 비전 단계에서 읽습니다.
    pub fn open_mmproj(&self) -> Result<Option<Arc<WeightFile>>> {
        self.mmproj.as_deref().map(|path| Ok(Arc::new(WeightFile::open(path)?))).transpose()
    }
}

/// `model_dir/active.json` 이 가리키는 GGUF 를 열고 구조 값을 읽습니다 (q8 파일).
pub fn load_active(model_dir: &Path) -> Result<ActiveModel> {
    load_record(model_dir, false)
}

/// [`load_active`] 와 같되, `device` 가 K 형식을 장치에서 곱하고 피라미드 파일(`ai::pyramid`)이 있으면 그 파일을 고릅니다
/// (`BLUEPRINT_PYRAMID=0` 이면 늘 q8, `1` 이면 장치와 상관없이 있으면). 고른 양자화는 `quant` 에 `pyramid:<배분>` 으로 적힙니다.
pub fn load_active_on(model_dir: &Path, device: &candle_core::Device) -> Result<ActiveModel> {
    load_record(model_dir, super::pyramid::wanted(device))
}

fn load_record(model_dir: &Path, pyramid: bool) -> Result<ActiveModel> {
    let record_path = model_dir.join("active.json");
    let record: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(&record_path)
            .with_context(|| format!("선택 기록을 읽을 수 없습니다: {}. 앱을 실행하면 모델을 내려받습니다", record_path.display()))?,
    )?;
    let mut file = record["file"]
        .as_str()
        .ok_or_else(|| anyhow!("active.json 에 file 이 없습니다"))?
        .to_string();
    let mut quant = record["active"]
        .as_str()
        .ok_or_else(|| anyhow!("active.json 에 active 가 없습니다"))?
        .to_string();
    let model = record["model"]
        .as_str()
        .map(str::to_string)
        .unwrap_or_else(|| model_dir.file_name().map(|s| s.to_string_lossy().to_string()).unwrap_or_default());
    let mut mmproj_file = record["mmproj"].as_str().map(str::to_string);
    let p = &record["pyramid"];
    // 지금 레시피(원본 BF16 에서 한 번 양자화)로 만든 파일만 엽니다. 옛 레시피 파일은 앱이 다시 만들 때까지 q8 를 씁니다.
    let fresh = p["recipe"].as_u64() == Some(super::pyramid::RECIPE as u64);
    if let Some(pf) = p["file"].as_str().filter(|f| pyramid && fresh && model_dir.join(f).exists()) {
        // 비전 파일은 피라미드 쪽이 있으면 그것, 없으면(비전을 그대로 둔 배분) 원래 것
        let vision = p["mmproj"].as_str().map(str::to_string).filter(|m| model_dir.join(m).exists());
        if p["mmproj"].is_null() || vision.is_some() {
            file = pf.to_string();
            mmproj_file = vision.or(mmproj_file);
            quant = format!("pyramid:{}", p["plan"].as_str().unwrap_or("?"));
        }
    }
    let quant = quant.as_str();
    let file = file.as_str();

    let path = model_dir.join(file);
    let mmproj = mmproj_file.map(|f| model_dir.join(f)).filter(|p| p.exists());
    let mut reader = BufReader::new(
        File::open(&path).with_context(|| format!("GGUF 를 열 수 없습니다: {}", path.display()))?,
    );
    let content = Content::read(&mut reader)?;
    let architecture = get_str(&content.metadata, "general.architecture")?;
    let block_count = get_u32(&content.metadata, &format!("{architecture}.block_count"))?;
    let embedding_length = get_u32(&content.metadata, &format!("{architecture}.embedding_length"))?;
    let vocab_size = get_u32(&content.metadata, &format!("{architecture}.vocab_size"))?;
    if !matches!(architecture.as_str(), "zoro" | "gocr" | "siglip2") {
        bail!("이 엔진이 아직 다루지 않는 구조입니다: {architecture}");
    }
    Ok(ActiveModel {
        model,
        quant: quant.to_string(),
        path,
        mmproj,
        architecture,
        block_count,
        embedding_length,
        vocab_size,
    })
}

fn get_str(meta: &HashMap<String, Value>, key: &str) -> Result<String> {
    match meta.get(key) {
        Some(Value::String(s)) => Ok(s.clone()),
        _ => bail!("GGUF 메타데이터 {key} (문자열) 가 없습니다"),
    }
}

fn get_u32(meta: &HashMap<String, Value>, key: &str) -> Result<u32> {
    match meta.get(key) {
        Some(Value::U32(v)) => Ok(*v),
        _ => bail!("GGUF 메타데이터 {key} (u32) 가 없습니다"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use candle_core::quantized::{gguf_file, GgmlDType, QTensor};
    use candle_core::{Device, Tensor};

    #[test]
    fn reads_the_active_gguf_structure() -> Result<()> {
        let dir = std::env::temp_dir().join(format!("blueprint-active-{}", std::process::id()));
        std::fs::create_dir_all(&dir)?;
        let meta = [
            ("general.architecture", Value::String("zoro".into())),
            ("zoro.block_count", Value::U32(32)),
            ("zoro.embedding_length", Value::U32(2560)),
            ("zoro.vocab_size", Value::U32(248078)),
        ];
        let meta_refs: Vec<(&str, &Value)> = meta.iter().map(|(k, v)| (*k, v)).collect();
        let t = QTensor::quantize(&Tensor::zeros((2, 256), candle_core::DType::F32, &Device::Cpu)?, GgmlDType::F32)?;
        let mut f = File::create(dir.join("tiny-q8.gguf"))?;
        gguf_file::write(&mut f, &meta_refs, &[("token_embd.weight", &t)])?;
        drop(f);
        std::fs::write(
            dir.join("active.json"),
            r#"{"model":"tiny","active":"q8","file":"tiny-q8.gguf","mmproj":"mmproj-tiny-bf16.gguf","reason":"test"}"#,
        )?;

        let m = load_active(&dir)?;
        assert_eq!(m.architecture, "zoro");
        assert_eq!(m.quant, "q8");
        assert_eq!((m.block_count, m.embedding_length, m.vocab_size), (32, 2560, 248078));
        assert_eq!(m.mmproj, None, "파일이 없으면 판단은 그대로 돕니다");
        std::fs::write(dir.join("mmproj-tiny-bf16.gguf"), b"")?;
        assert_eq!(load_active(&dir)?.mmproj, Some(dir.join("mmproj-tiny-bf16.gguf")));
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    /// 실제 모델 파일이 있을 때 확인합니다. `cargo test -- --ignored` 로 실행하세요.
    #[test]
    #[ignore]
    fn real_active_models_open() -> Result<()> {
        let zoro = load_active(&blueprint_models::model_dir("zoro"))?;
        assert_eq!((zoro.architecture.as_str(), zoro.block_count, zoro.embedding_length), ("zoro", 32, 2560));
        let gocr = load_active(&blueprint_models::model_dir("gocr"))?;
        assert_eq!((gocr.architecture.as_str(), gocr.block_count, gocr.embedding_length), ("gocr", 16, 1536));
        Ok(())
    }
}
