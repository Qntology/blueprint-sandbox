//! 앱이 쓰는 모델 묶음. 판단 · 글쓰기 · 문서 읽기가 상주 관리자를 함께 씁니다 (한 번에 한 모델).

use std::sync::Arc;

use anyhow::Result;
use candle_core::Device;

use super::accel;
use super::document::{DocumentReader, VlmFallback};
use super::pyramid;
use super::model_store::{model_dir, models_dir};
use super::pointer::PointerModel;
use super::residency::{Budget, Coexist, Residency};
use super::zoro::{open_engine_models, SharedBackbone};

pub struct Runtime {
    /// 표시용 장치 이름
    pub backend: &'static str,
    pub device: Device,
    pub model: PointerModel,
    pub writer: SharedBackbone,
    pub reader: Arc<DocumentReader>,
}

/// 장치를 고르고 모델을 엽니다. 가중치는 쓸 때 단계별로 올라옵니다.
pub fn open() -> Result<Runtime> {
    let device = accel::select_device()?;
    let backend = accel::backend_name(&device);
    // 사용할 때 양자화: 기본 배분의 피라미드 파일이 없거나 레시피가 바뀌었으면 원본 BF16 (또는 원본 그대로인 BF16 mmproj)에서
    // 한 번 양자화해 만듭니다 (K 형식을 장치에서 곱하는 백엔드만, `BLUEPRINT_PYRAMID=0` 이면 끔). 만들지 못하면 q8 파일로 엽니다.
    if let Err(e) = pyramid::ensure(&models_dir(), &device, &mut |m| eprintln!("{m}")) {
        eprintln!("[피라미드] 파일을 만들지 못해 q8 로 엽니다: {e:#}");
    }
    // 공존(`BLUEPRINT_COEXIST=siglip2`)은 켰을 때만입니다. 기본은 한 번에 한 모델입니다.
    let residency = Residency::shared_with(device.clone(), Budget::auto(&device), Coexist::from_env());
    let (model, writer) = open_engine_models(&model_dir("zoro"), residency.clone())?;
    // 다국어 · 저신뢰 조각은 같은 Zoro-1 의 비전으로 다시 읽을 수 있습니다 (`BLUEPRINT_VLM`, 기본 꺼짐).
    let reader = Arc::new(DocumentReader::open(&models_dir(), residency)?.with_vlm(Arc::clone(&writer.0), VlmFallback::from_env()));
    Ok(Runtime { backend, device, model, writer, reader })
}
