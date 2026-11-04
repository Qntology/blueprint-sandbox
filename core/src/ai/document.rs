//! 문서 한 장 읽기: 기울기 보정 → SigLIP2 로 읽을 조각 계획 → gocr 로 조각 인식 (명세 3.1 의 Stage 1 ~ 3).
//!
//! 단계마다 필요한 가중치만 올라옵니다. SigLIP2 비전 → (내림) → gocr 비전 → (내림) → gocr 언어 순서이고,
//! 모델 전환은 상주 관리자가 맡습니다. 시각 앵커 파일이 없으면 버리는 조각 없이 잉크 조각을 모두 읽습니다.
//!
//! 여러 장은 [`DocumentReader::read_pages`] 로 함께 읽어야 합니다. 모델을 장마다 오가지 않고 단계마다 한 번만 올립니다.
//! 픽셀 계산은 페이지 · 조각 단위로 병렬이고, 모델 계산은 한 번에 한 묶음입니다.

use std::path::Path;
use std::time::Instant;

use anyhow::{anyhow, Result};
use image::imageops::{resize, FilterType};
use image::{GrayImage, RgbImage};
use rayon::prelude::*;
use serde_json::json;

use crate::symbols::{candidates, crop_square, CandidateParams, Feature, SymbolDict, SymbolHit, DICT_FILE};
use super::gocr::image::prepare as gocr_prepare;
use super::gocr::{open as open_gocr, GocrEngine, GocrInput, OcrOutput, Strategy, EARLY_EXIT, TEXT_PROMPT};
use super::residency::SharedResidency;
use super::siglip2::config::DEFAULT_MAX_PATCHES;
use super::siglip2::image::{prepare as siglip2_prepare, PatchInput};
use super::siglip2::ink::{deskew, estimate_skew, Bbox, InkPage};
use super::siglip2::plan::{plan_page, plan_unfiltered, CropKind, CropPlan, PagePlan, PlanParams, Scores};
use super::siglip2::probe::ProbeBank;
use super::siglip2::Siglip2;
use super::trace;

/// 시각 앵커 파일 이름 (siglip2 모델 폴더 안)
pub const PROBE_FILE: &str = "crop_probe.bin";
const TABLE_PROMPT: &str = "Table Recognition:";
/// 조각 하나에서 읽을 최대 토큰 수
const MAX_TOKENS: usize = 1024;
const GOCR_PATCH: usize = 14;
const GOCR_MERGE: usize = 2;

/// 조각 하나를 읽은 결과
#[derive(Debug, Clone, PartialEq)]
pub struct CropReading {
    pub kind: CropKind,
    /// 곧게 편 페이지의 좌표
    pub bbox: Bbox,
    pub output: OcrOutput,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Timings {
    pub layout_ms: f64,
    pub siglip2_ms: f64,
    pub ocr_ms: f64,
    /// 심볼 후보 추출 + 그림 사전 대조 (켰을 때만)
    pub symbol_ms: f64,
}

#[derive(Debug, Clone)]
pub struct PageReading {
    /// 곧게 펴려고 돌린 각도 (도)
    pub skew: f32,
    pub plan: PagePlan,
    pub crops: Vec<CropReading>,
    /// 심볼 후보와 그림 사전 결과 (`ReadOptions::symbols` 를 켜고 사전이 있을 때). 버린 후보도 들어 있습니다 (`class == None`).
    pub symbols: Vec<SymbolHit>,
    pub timings: Timings,
}

/// 읽는 방식
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReadOptions {
    pub plan: PlanParams,
    /// 입력란도 표 프롬프트로 읽습니다. 항목과 값이 칸으로 묶여 나오고, 값을 건너뛰는 일이 줄어듭니다.
    pub forms_as_tables: bool,
    /// 도면의 설비 심볼을 찾습니다 (곧게 편 페이지의 잉크 후보 → SigLIP2 → 그림 사전). 비전 타워가 올라와 있는 계획 단계 바로 뒤에 돕니다.
    pub symbols: bool,
}

impl Default for ReadOptions {
    fn default() -> Self {
        Self { plan: PlanParams::default(), forms_as_tables: true, symbols: false }
    }
}

/// 이미지 한 장의 SigLIP2 벡터 (모두 L2 정규화)
#[derive(Debug, Clone, PartialEq)]
pub struct ImageVectors {
    pub pooled: Vec<f32>,
    pub mean_patch: Vec<f32>,
}

impl ImageVectors {
    pub fn get(&self, feature: Feature) -> &[f32] {
        match feature {
            Feature::Pooled => &self.pooled,
            Feature::MeanPatch => &self.mean_patch,
        }
    }
}

/// 심볼 조각을 SigLIP2 에 넣을 때의 패치 상한. 조각은 한 변 90 px 안팎이라 8 × 8 패치(128 px)면 모양이 다 들어갑니다.
pub const SYMBOL_MAX_PATCHES: usize = 64;

pub struct DocumentReader {
    siglip2: Siglip2,
    probes: Option<ProbeBank>,
    symbols: Option<SymbolDict>,
    gocr: GocrEngine,
    options: ReadOptions,
}

/// 이미지의 상자를 잘라 배율만큼 키웁니다.
pub fn cut(img: &RgbImage, (x0, y0, x1, y1): Bbox, upscale: f32) -> RgbImage {
    let piece = image::imageops::crop_imm(img, x0, y0, x1 - x0, y1 - y0).to_image();
    if upscale <= 1.01 {
        return piece;
    }
    let (w, h) = (((x1 - x0) as f32 * upscale).round() as u32, ((y1 - y0) as f32 * upscale).round() as u32);
    resize(&piece, w.max(1), h.max(1), FilterType::Lanczos3)
}

fn gray_of(img: &RgbImage) -> GrayImage {
    GrayImage::from_fn(img.width(), img.height(), |x, y| {
        let p = img.get_pixel(x, y).0;
        image::Luma([((p[0] as u32 * 77 + p[1] as u32 * 150 + p[2] as u32 * 29) >> 8) as u8])
    })
}

impl DocumentReader {
    /// `models_dir` 아래의 `siglip2` · `gocr` 모델을 엽니다. 가중치는 읽을 때 단계별로 올라옵니다.
    pub fn open(models_dir: &Path, residency: SharedResidency) -> Result<Self> {
        let dir = models_dir.join("siglip2");
        let siglip2 = Siglip2::open(&dir, residency.clone())?;
        let probe = dir.join(PROBE_FILE);
        let probes = probe.exists().then(|| ProbeBank::from_file(&probe)).transpose()?;
        let dict = dir.join(DICT_FILE);
        let symbols = dict.exists().then(|| SymbolDict::from_file(&dict)).transpose()?;
        let gocr = open_gocr(&models_dir.join("gocr"), residency, MAX_TOKENS, Strategy::Speculative { early_exit: Some(EARLY_EXIT) })?;
        Ok(Self { siglip2, probes, symbols, gocr, options: ReadOptions::default() })
    }

    pub fn with_options(mut self, options: ReadOptions) -> Self {
        self.options = options;
        self
    }

    pub fn options(&self) -> ReadOptions {
        self.options
    }

    pub fn set_options(&mut self, options: ReadOptions) {
        self.options = options;
    }

    /// 그림 사전을 바꿉니다 (사전을 새로 만든 직후 시험할 때)
    pub fn set_symbol_dict(&mut self, dict: Option<SymbolDict>) {
        self.symbols = dict;
    }

    pub fn symbol_dict(&self) -> Option<&SymbolDict> {
        self.symbols.as_ref()
    }

    /// 이미지마다 SigLIP2 풀링 벡터와 패치 평균 벡터. 그림 사전을 만들고 대조할 때 씁니다.
    pub fn embed_images(&self, images: &[RgbImage], max_patches: usize) -> Result<Vec<ImageVectors>> {
        let patch = self.siglip2.config().patch;
        let inputs: Vec<PatchInput> = images.par_iter().map(|im| siglip2_prepare(im, patch, max_patches)).collect::<Result<_>>()?;
        let grids = self.siglip2.encode_images(&inputs)?;
        Ok(grids
            .into_iter()
            .map(|g| {
                let mut mean = vec![0.0f32; g.dim];
                for p in g.patches.chunks(g.dim) {
                    mean.iter_mut().zip(p).for_each(|(a, b)| *a += b);
                }
                crate::symbols::normalize(&mut mean);
                ImageVectors { pooled: g.pooled, mean_patch: mean }
            })
            .collect())
    }

    /// 페이지의 정해진 영역만 다시 읽습니다 (부분 재판독). 도면은 조각 계획이 한 행의 상자들을 가로 띠 하나로 읽어서
    /// 상자별 글자를 떼어 낼 수 없으므로, KKS 를 못 읽은 상자는 그 상자만 글자 프롬프트로 다시 읽습니다.
    /// 영역은 테두리 괘선이 글자로 읽히지 않게 안쪽으로 `inset` 화소 줄여 자릅니다.
    pub fn read_regions(&self, page: &RgbImage, regions: &[Bbox], inset: u32) -> Result<Vec<OcrOutput>> {
        let inputs: Vec<GocrInput> = regions
            .par_iter()
            .map(|&(x0, y0, x1, y1)| {
                let b = (x0 + inset, y0 + inset, x1.saturating_sub(inset).max(x0 + inset + 1), y1.saturating_sub(inset).max(y0 + inset + 1));
                Ok(GocrInput { image: gocr_prepare(&cut(page, b, 1.0), GOCR_PATCH, GOCR_MERGE)?, prompt: TEXT_PROMPT.to_string() })
            })
            .collect::<Result<_>>()?;
        self.gocr.recognize(&inputs)
    }

    /// 페이지마다 심볼 후보를 찾아 그림 사전에 댑니다. 사전이 없으면 빈 결과입니다.
    pub fn find_symbols(&self, pages: &[&RgbImage]) -> Result<Vec<Vec<SymbolHit>>> {
        let Some(dict) = &self.symbols else { return Ok(vec![Vec::new(); pages.len()]) };
        let t = Instant::now();
        let params = CandidateParams::default();
        let found: Vec<Vec<_>> = pages.par_iter().map(|p| candidates(p, &params)).collect();
        let crops: Vec<RgbImage> = found.iter().zip(pages).flat_map(|(cs, p)| cs.iter().map(move |c| crop_square(p, c.bbox))).collect();
        let vectors = self.embed_images(&crops, SYMBOL_MAX_PATCHES)?;
        let mut it = vectors.iter();
        let hits: Vec<Vec<SymbolHit>> = found
            .iter()
            .map(|cs| cs.iter().zip(it.by_ref()).map(|(c, v)| SymbolHit { bbox: c.bbox, matched: dict.classify(v.get(dict.feature)) }).collect())
            .collect();
        trace::record("siglip2", "심볼 그림 사전", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages.len(), "candidates": crops.len(), "accepted": hits.iter().flatten().filter(|h| h.matched.class.is_some()).count() }));
        Ok(hits)
    }

    /// 페이지를 곧게 펴고 읽을 조각을 정합니다. SigLIP2 비전 타워만 씁니다.
    pub fn plan(&self, page: &RgbImage) -> Result<(RgbImage, f32, PagePlan, Timings)> {
        self.plan_pages(std::slice::from_ref(page))?.pop().ok_or_else(|| anyhow!("페이지 결과가 없습니다"))
    }

    /// 픽셀 계산은 페이지마다 병렬로 하고, SigLIP2 는 격자가 같은 페이지를 묶어 한 번만 올려 계산합니다.
    fn plan_pages(&self, pages: &[RgbImage]) -> Result<Vec<(RgbImage, f32, PagePlan, Timings)>> {
        let t = Instant::now();
        let straight: Vec<(RgbImage, f32, InkPage)> = pages
            .par_iter()
            .map(|page| {
                let skew = estimate_skew(&gray_of(page));
                let image = deskew(page, skew);
                let ink = InkPage::new(&gray_of(&image));
                (image, skew, ink)
            })
            .collect();
        let per_page = |t: Instant| t.elapsed().as_secs_f64() * 1e3 / pages.len().max(1) as f64;
        let layout_ms = per_page(t);
        trace::record("규칙", "기울기 보정 · 잉크 분석", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages.len() }));

        let Some(bank) = &self.probes else {
            let plan = |(image, skew, ink): (RgbImage, f32, InkPage)| {
                let plan = plan_unfiltered(&ink, &self.options.plan);
                (image, skew, plan, Timings { layout_ms, ..Timings::default() })
            };
            return Ok(straight.into_iter().map(plan).collect());
        };
        let t = Instant::now();
        let patch = self.siglip2.config().patch;
        let inputs: Vec<PatchInput> = straight.par_iter().map(|(image, ..)| siglip2_prepare(image, patch, DEFAULT_MAX_PATCHES)).collect::<Result<_>>()?;
        let grids = self.siglip2.encode_images(&inputs)?;
        let siglip2_ms = per_page(t);
        trace::record("siglip2", "패치 특징", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages.len(), "patches": inputs.iter().map(|i| i.rows * i.cols).sum::<usize>() }));
        let t = Instant::now();
        let planned: Vec<(RgbImage, f32, PagePlan)> = straight
            .into_par_iter()
            .zip(grids)
            .map(|((image, skew, ink), grid)| Ok((image, skew, plan_page(&ink, &Scores::new(bank, &grid)?, &self.options.plan))))
            .collect::<Result<_>>()?;
        let layout_ms = layout_ms + per_page(t);
        if trace::enabled() {
            let pages: Vec<_> = planned
                .iter()
                .map(|(_, skew, plan)| json!({ "skew": skew, "crops": plan.crops.iter().map(|c| json!({ "kind": c.kind.name(), "bbox": [c.bbox.0, c.bbox.1, c.bbox.2, c.bbox.3] })).collect::<Vec<_>>() }))
                .collect();
            trace::record("규칙", "조각 계획", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages }));
        }
        Ok(planned.into_iter().map(|(image, skew, plan)| (image, skew, plan, Timings { layout_ms, siglip2_ms, ..Timings::default() })).collect())
    }

    fn prompt_of(&self, kind: CropKind) -> &'static str {
        match kind {
            CropKind::Table => TABLE_PROMPT,
            CropKind::Form if self.options.forms_as_tables => TABLE_PROMPT,
            _ => TEXT_PROMPT,
        }
    }

    pub fn read(&self, page: &RgbImage) -> Result<PageReading> {
        let mut out = self.read_pages(std::slice::from_ref(page))?;
        out.pop().ok_or_else(|| anyhow!("페이지 결과가 없습니다"))
    }

    /// 페이지들을 읽습니다. 모델을 페이지마다 오가지 않도록 계획을 모두 세운 뒤 조각을 한꺼번에 읽습니다.
    /// 서명 조각은 글자로 읽지 않고 좌표만 계획에 남깁니다.
    pub fn read_pages(&self, pages: &[RgbImage]) -> Result<Vec<PageReading>> {
        let planned = self.plan_pages(pages)?;
        // 심볼은 비전 타워가 올라와 있을 때(gocr 로 넘어가기 전) 찾습니다.
        let t = Instant::now();
        let mut symbols = if self.options.symbols { self.find_symbols(&planned.iter().map(|p| &p.0).collect::<Vec<_>>())? } else { vec![Vec::new(); planned.len()] }.into_iter();
        let symbol_ms = t.elapsed().as_secs_f64() * 1e3 / pages.len().max(1) as f64;
        let t = Instant::now();
        let crops: Vec<(&RgbImage, &CropPlan)> = planned.iter().flat_map(|(straight, _, plan, _)| plan.crops.iter().filter(|c| c.kind != CropKind::Sign).map(move |c| (straight, c))).collect();
        let inputs: Vec<GocrInput> = crops
            .into_par_iter()
            .map(|(straight, c)| Ok(GocrInput { image: gocr_prepare(&cut(straight, c.bbox, c.upscale), GOCR_PATCH, GOCR_MERGE)?, prompt: self.prompt_of(c.kind).to_string() }))
            .collect::<Result<_>>()?;
        let outputs = self.gocr.recognize(&inputs)?;
        let ocr_ms = t.elapsed().as_secs_f64() * 1e3 / pages.len().max(1) as f64;
        if trace::enabled() {
            let texts: Vec<_> = if trace::with_text() { outputs.iter().map(|o| json!({ "text": o.text, "conf": o.min_confidence })).collect() } else { Vec::new() };
            let detail = json!({
                "crops": outputs.len(),
                "tokens": outputs.iter().map(|o| o.tokens.len()).sum::<usize>(),
                "image_tokens": outputs.iter().map(|o| o.image_tokens).sum::<usize>(),
                "drafts": outputs.iter().map(|o| o.drafts).sum::<usize>(),
                "verifies": outputs.iter().map(|o| o.verifies).sum::<usize>(),
                "texts": texts,
            });
            trace::record("gocr", "결과", 0.0, detail);
        }
        let mut outputs = outputs.into_iter();
        Ok(planned
            .into_iter()
            .map(|(_, skew, plan, mut timings)| {
                timings.ocr_ms = ocr_ms;
                timings.symbol_ms = if self.options.symbols { symbol_ms } else { 0.0 };
                let crops = plan
                    .crops
                    .iter()
                    .filter(|c| c.kind != CropKind::Sign)
                    .zip(outputs.by_ref())
                    .map(|(c, output)| CropReading { kind: c.kind, bbox: c.bbox, output })
                    .collect();
                PageReading { skew, plan, crops, symbols: symbols.next().unwrap_or_default(), timings }
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;

    #[test]
    fn cutting_keeps_the_box_and_enlarges_on_request() {
        let mut img = RgbImage::from_pixel(100, 80, Rgb([255, 255, 255]));
        img.put_pixel(20, 10, Rgb([0, 0, 0]));
        let piece = cut(&img, (20, 10, 60, 30), 1.0);
        assert_eq!(piece.dimensions(), (40, 20));
        assert_eq!(piece.get_pixel(0, 0), &Rgb([0, 0, 0]));
        assert_eq!(cut(&img, (20, 10, 60, 30), 2.5).dimensions(), (100, 50));
    }

    #[test]
    fn grayscale_matches_the_usual_weights() {
        let img = RgbImage::from_pixel(1, 1, Rgb([255, 0, 0]));
        let v = gray_of(&img).get_pixel(0, 0)[0];
        assert!((75..=78).contains(&v), "{v}");
    }
}
