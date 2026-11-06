//! 문서 한 장 읽기: 기울기 보정 → SigLIP2 로 읽을 조각 계획 → gocr 로 조각 인식 (명세 3.1 의 Stage 1 ~ 3).
//!
//! 단계마다 필요한 가중치만 올라옵니다. SigLIP2 비전 → (내림) → gocr 비전 → (내림) → gocr 언어 순서이고,
//! 모델 전환은 상주 관리자가 맡습니다. 시각 앵커 파일이 없으면 버리는 조각 없이 잉크 조각을 모두 읽습니다.
//!
//! 여러 장은 [`DocumentReader::read_pages`] 로 함께 읽어야 합니다. 모델을 장마다 오가지 않고 단계마다 한 번만 올립니다.
//! 픽셀 계산은 페이지 · 조각 단위로 병렬이고, 모델 계산은 한 번에 한 묶음입니다.
//!
//! 마스터 도면을 알면 [`DocumentReader::read_pages_delta`] 로 바뀐 조각만 읽습니다 (SDS 델타 패치, `siglip2::delta`).
//! 어느 마스터인지는 계획 단계에서 이미 뽑는 SigLIP2 특징 격자로, 무엇이 바뀌었는지는 잉크로 가리므로 모델 계산이 더 들지 않습니다.

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
use super::siglip2::delta::{clip_to_changes, ink_changes, moved, retain_changed, trim_ignored, DeltaParams, Master, MasterSet};
use super::siglip2::image::{prepare as siglip2_prepare, PatchInput};
use super::siglip2::ink::{deskew, estimate_skew, Bbox, InkPage};
use super::siglip2::plan::{plan_page, plan_unfiltered, CropKind, CropPlan, PagePlan, PlanParams, Scores};
use super::siglip2::probe::ProbeBank;
use super::siglip2::{PatchGrid, Siglip2};
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
    /// 마스터와 견준 결과 (`read_pages_delta` 일 때만)
    pub delta: Option<DeltaReading>,
}

/// 마스터 대조 기준 (SDS 델타 패치)
pub enum Baseline<'a> {
    /// 페이지마다 가장 닮은 마스터를 고릅니다 (현장 스캔)
    Best(&'a MasterSet),
    /// 페이지마다 정해 둔 마스터 (A0 타일처럼 자리를 아는 경우). 없는 페이지는 전부 읽습니다.
    Each(&'a [Option<&'a Master>]),
}

/// 마스터와 견준 결과 (페이지 하나)
#[derive(Debug, Clone, PartialEq)]
pub struct DeltaReading {
    /// 견준 마스터
    pub master: Option<String>,
    /// SigLIP2 패치 코사인 평균 (마스터를 고른 근거)
    pub similarity: f32,
    /// 두 번째로 닮은 마스터와의 평균 코사인 차이 (정해 둔 마스터면 0)
    pub margin: f32,
    /// 잉크가 바뀐 칸 수 · 전체 칸 수
    pub changed: usize,
    pub blocks: usize,
    /// 마스터와 같아 읽지 않은 조각
    pub skipped: Vec<Bbox>,
    /// 바뀐 곳 둘레로 좁혀 읽은 조각 수와, 좁힌 뒤 글자가 없어 뺀 조각
    pub clipped: usize,
    pub clipped_blank: Vec<Bbox>,
    /// 델타를 쓰지 않고 전부 읽은 까닭
    pub fallback: Option<String>,
    /// 패치마다 마스터와의 SigLIP2 코사인 (진단용)
    pub cosines: Vec<f32>,
    /// 바뀐 칸 상자 (진단용)
    pub changed_boxes: Vec<Bbox>,
    /// 고정 안내문 자리(`ignore_zones`)를 잘라 낸 조각 수
    pub trimmed: usize,
}

/// 계획까지 마친 페이지
struct Planned {
    image: RgbImage,
    skew: f32,
    plan: PagePlan,
    timings: Timings,
    delta: Option<DeltaReading>,
}

/// 기울기를 바로잡고 잉크를 분석합니다 (페이지마다 병렬).
fn straighten(pages: &[&RgbImage]) -> Vec<(RgbImage, f32, InkPage)> {
    pages
        .par_iter()
        .map(|page| {
            let skew = estimate_skew(&gray_of(page));
            let image = deskew(page, skew);
            // 유채색 잉크(붉은 · 푸른 펜)를 따로 표시해 델타가 흑백 획에 겹친 수정선도 봅니다 (개선안 1-2).
            let ink = InkPage::new(&gray_of(&image)).with_color(&image);
            (image, skew, ink)
        })
        .collect()
}

/// 바뀐 곳에 걸치지 않는 조각을 계획에서 뺍니다. 마스터를 고르지 못했거나 너무 많이 바뀌었으면 계획을 그대로 둡니다.
fn apply_delta(plan: &mut PagePlan, grid: &PatchGrid, ink: &InkPage, baseline: &Baseline<'_>, page: usize, p: &DeltaParams) -> DeltaReading {
    let mut out = DeltaReading { master: None, similarity: 0.0, margin: 0.0, changed: 0, blocks: 0, skipped: Vec::new(), clipped: 0, clipped_blank: Vec::new(), fallback: None, cosines: Vec::new(), changed_boxes: Vec::new(), trimmed: 0 };
    let master = match baseline {
        Baseline::Best(set) => {
            let Some(m) = set.best_match(grid) else {
                out.fallback = Some("격자가 같은 마스터가 없습니다".into());
                return out;
            };
            (out.similarity, out.margin) = (m.similarity, m.margin);
            let master = &set.pages[m.index];
            out.master = Some(master.id().to_string());
            out.cosines = master.patches.cosines(grid).unwrap_or_default();
            if m.margin < p.min_margin {
                out.fallback = Some(format!("어느 마스터인지 확신하지 못했습니다 (패치 코사인 차이 {:.3})", m.margin));
                return out;
            }
            master
        }
        Baseline::Each(list) => {
            let Some(master) = list.get(page).copied().flatten() else {
                out.fallback = Some("정해 둔 마스터가 없습니다".into());
                return out;
            };
            out.master = Some(master.id().to_string());
            out.cosines = master.patches.cosines(grid).unwrap_or_default();
            out.similarity = out.cosines.iter().sum::<f32>() / out.cosines.len().max(1) as f32;
            master
        }
    };
    let Some(diff) = ink_changes(master, ink, p) else {
        out.fallback = Some("마스터와 크기가 다릅니다".into());
        return out;
    };
    out.blocks = diff.diff.len();
    out.changed = diff.changed(p.ink_threshold);
    out.changed_boxes = diff.changed_boxes(p.ink_threshold);
    if out.changed as f32 > out.blocks as f32 * p.max_changed {
        out.fallback = Some(format!("바뀐 칸이 {}/{} 로 많아 전부 읽습니다", out.changed, out.blocks));
        return out;
    }
    out.skipped = retain_changed(plan, |b| diff.touches(b, p.ink_threshold));
    if p.clip {
        (out.clipped, out.clipped_blank) = clip_to_changes(plan, ink, &diff, p.ink_threshold);
    }
    // 고정 안내문 자리는 서명란과 한 띠로 묶여도 읽지 않습니다 (개선안 1-1). 자리는 마스터 좌표라 도각 이동만큼 옮겨 댑니다.
    out.trimmed = trim_ignored(plan, ink, &moved(&master.ignore, diff.offset));
    out
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
    /// `scale` 이 1 보다 작으면 자른 영역을 그만큼 줄여 읽습니다 (300 DPI 도면의 글자를 150 DPI 크기로: gocr 이미지 토큰이 넓이에 비례).
    pub fn read_regions(&self, page: &RgbImage, regions: &[Bbox], inset: u32, scale: f32) -> Result<Vec<OcrOutput>> {
        let inputs: Vec<GocrInput> = regions
            .par_iter()
            .map(|&(x0, y0, x1, y1)| {
                let b = (x0 + inset, y0 + inset, x1.saturating_sub(inset).max(x0 + inset + 1), y1.saturating_sub(inset).max(y0 + inset + 1));
                let piece = cut(page, b, 1.0);
                let piece = if scale < 0.99 {
                    let (w, h) = (((piece.width() as f32) * scale).round().max(1.0) as u32, ((piece.height() as f32) * scale).round().max(1.0) as u32);
                    resize(&piece, w, h, FilterType::Lanczos3)
                } else {
                    piece
                };
                Ok(GocrInput { image: gocr_prepare(&piece, GOCR_PATCH, GOCR_MERGE)?, prompt: TEXT_PROMPT.to_string() })
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
        if trace::enabled() {
            // 받아들인 심볼만 좌표 · 종류 · 유사도 · 마진을 남깁니다 (시연 영상의 그림 사전 장면)
            let accepted: Vec<_> = hits
                .iter()
                .enumerate()
                .flat_map(|(i, hs)| hs.iter().filter(|h| h.matched.class.is_some()).map(move |h| json!({ "page": i, "bbox": h.bbox, "class": h.matched.class, "sim": h.matched.sim, "margin": h.matched.margin })))
                .collect();
            trace::record("siglip2", "심볼 그림 사전", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages.len(), "candidates": crops.len(), "accepted": accepted.len(), "hits": accepted }));
        }
        Ok(hits)
    }

    /// 페이지를 곧게 펴고 읽을 조각을 정합니다. SigLIP2 비전 타워만 씁니다.
    pub fn plan(&self, page: &RgbImage) -> Result<(RgbImage, f32, PagePlan, Timings)> {
        let p = self.plan_pages(std::slice::from_ref(page), None)?.pop().ok_or_else(|| anyhow!("페이지 결과가 없습니다"))?;
        Ok((p.image, p.skew, p.plan, p.timings))
    }

    /// 마스터 도면(또는 A0 타일)의 특징 격자와 잉크를 만들어 둡니다 (델타 판독의 기준). 읽을 때와 같게 곧게 편 뒤에 만듭니다.
    pub fn index_masters(&self, pages: &[(String, RgbImage)], p: &DeltaParams) -> Result<MasterSet> {
        let refs: Vec<&RgbImage> = pages.iter().map(|(_, p)| p).collect();
        let straight = straighten(&refs);
        let grids = self.grids_of(&straight.iter().map(|s| &s.0).collect::<Vec<_>>())?;
        let masters = straight.into_par_iter().zip(grids).zip(pages.par_iter()).map(|(((_, _, ink), grid), (id, _))| Master::new(id.clone(), &grid, ink, p.radius)).collect();
        Ok(MasterSet { pages: masters })
    }

    fn grids_of(&self, images: &[&RgbImage]) -> Result<Vec<PatchGrid>> {
        let patch = self.siglip2.config().patch;
        let inputs: Vec<PatchInput> = images.par_iter().map(|image| siglip2_prepare(image, patch, DEFAULT_MAX_PATCHES)).collect::<Result<_>>()?;
        let t = Instant::now();
        let grids = self.siglip2.encode_images(&inputs)?;
        trace::record("siglip2", "패치 특징", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": images.len(), "patches": inputs.iter().map(|i| i.rows * i.cols).sum::<usize>() }));
        Ok(grids)
    }

    /// 픽셀 계산은 페이지마다 병렬로 하고, SigLIP2 는 격자가 같은 페이지를 묶어 한 번만 올려 계산합니다.
    /// `delta` 가 있으면 마스터와 견주어 바뀌지 않은 조각을 계획에서 뺍니다.
    fn plan_pages(&self, pages: &[RgbImage], delta: Option<(&Baseline<'_>, &DeltaParams)>) -> Result<Vec<Planned>> {
        let t = Instant::now();
        let straight = straighten(&pages.iter().collect::<Vec<_>>());
        let per_page = |t: Instant| t.elapsed().as_secs_f64() * 1e3 / pages.len().max(1) as f64;
        let layout_ms = per_page(t);
        trace::record("규칙", "기울기 보정 · 잉크 분석", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages.len() }));

        if self.probes.is_none() && delta.is_none() {
            let plan = |(image, skew, ink): (RgbImage, f32, InkPage)| Planned { plan: plan_unfiltered(&ink, &self.options.plan), image, skew, timings: Timings { layout_ms, ..Timings::default() }, delta: None };
            return Ok(straight.into_iter().map(plan).collect());
        }
        let t = Instant::now();
        let grids = self.grids_of(&straight.iter().map(|s| &s.0).collect::<Vec<_>>())?;
        let siglip2_ms = per_page(t);
        let t = Instant::now();
        let planned: Vec<Planned> = straight
            .into_par_iter()
            .zip(grids)
            .enumerate()
            .map(|(i, ((image, skew, ink), grid))| {
                let mut plan = match &self.probes {
                    Some(bank) => plan_page(&ink, &Scores::new(bank, &grid)?, &self.options.plan),
                    None => plan_unfiltered(&ink, &self.options.plan),
                };
                let delta = delta.map(|(baseline, params)| apply_delta(&mut plan, &grid, &ink, baseline, i, params));
                Ok(Planned { image, skew, plan, timings: Timings::default(), delta })
            })
            .collect::<Result<_>>()?;
        let layout_ms = layout_ms + per_page(t);
        if trace::enabled() {
            let pages: Vec<_> = planned
                .iter()
                .map(|p| {
                    json!({
                        "skew": p.skew,
                        "crops": p.plan.crops.iter().map(|c| json!({ "kind": c.kind.name(), "bbox": [c.bbox.0, c.bbox.1, c.bbox.2, c.bbox.3] })).collect::<Vec<_>>(),
                        // 버린 조각: 안내문(dropped) · 글자 없는 띠(blank). 시연 영상이 계획의 버림 규칙을 그립니다.
                        "dropped": p.plan.dropped.iter().map(|b| [b.0, b.1, b.2, b.3]).collect::<Vec<_>>(),
                        "blank": p.plan.blank.iter().map(|b| [b.0, b.1, b.2, b.3]).collect::<Vec<_>>(),
                        "delta": p.delta.as_ref().map(|d| json!({
                            "master": d.master, "similarity": d.similarity, "margin": d.margin, "changed": d.changed, "blocks": d.blocks,
                            "skipped": d.skipped.len(), "clipped": d.clipped, "fallback": d.fallback, "trimmed": d.trimmed,
                            "skipped_boxes": d.skipped.iter().map(|b| [b.0, b.1, b.2, b.3]).collect::<Vec<_>>(),
                            "changed_boxes": d.changed_boxes.iter().map(|b| [b.0, b.1, b.2, b.3]).collect::<Vec<_>>(),
                        })),
                    })
                })
                .collect();
            trace::record("규칙", "조각 계획", t.elapsed().as_secs_f64() * 1e3, json!({ "pages": pages }));
        }
        Ok(planned.into_iter().map(|p| Planned { timings: Timings { layout_ms, siglip2_ms, ..Timings::default() }, ..p }).collect())
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
        let planned = self.plan_pages(pages, None)?;
        self.read_planned(planned)
    }

    /// 마스터와 견주어 바뀐 조각만 읽습니다 (SDS 델타 패치). 표제란 · 서명은 늘 읽고,
    /// 닮은 마스터가 없거나 너무 많이 바뀐 페이지는 [`read_pages`](Self::read_pages) 와 같이 전부 읽습니다.
    pub fn read_pages_delta(&self, pages: &[RgbImage], baseline: &Baseline<'_>, params: &DeltaParams) -> Result<Vec<PageReading>> {
        let planned = self.plan_pages(pages, Some((baseline, params)))?;
        self.read_planned(planned)
    }

    fn read_planned(&self, planned: Vec<Planned>) -> Result<Vec<PageReading>> {
        let pages = planned.len();
        // 심볼은 비전 타워가 올라와 있을 때(gocr 로 넘어가기 전) 찾습니다.
        let t = Instant::now();
        let mut symbols = if self.options.symbols { self.find_symbols(&planned.iter().map(|p| &p.image).collect::<Vec<_>>())? } else { vec![Vec::new(); planned.len()] }.into_iter();
        let symbol_ms = t.elapsed().as_secs_f64() * 1e3 / pages.max(1) as f64;
        let t = Instant::now();
        let crops: Vec<(&RgbImage, &CropPlan)> = planned.iter().flat_map(|p| p.plan.crops.iter().filter(|c| c.kind != CropKind::Sign).map(move |c| (&p.image, c))).collect();
        let inputs: Vec<GocrInput> = crops
            .into_par_iter()
            .map(|(straight, c)| Ok(GocrInput { image: gocr_prepare(&cut(straight, c.bbox, c.upscale), GOCR_PATCH, GOCR_MERGE)?, prompt: self.prompt_of(c.kind).to_string() }))
            .collect::<Result<_>>()?;
        let outputs = self.gocr.recognize(&inputs)?;
        let ocr_ms = t.elapsed().as_secs_f64() * 1e3 / pages.max(1) as f64;
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
            .map(|Planned { skew, plan, mut timings, delta, .. }| {
                timings.ocr_ms = ocr_ms;
                timings.symbol_ms = if self.options.symbols { symbol_ms } else { 0.0 };
                let crops = plan
                    .crops
                    .iter()
                    .filter(|c| c.kind != CropKind::Sign)
                    .zip(outputs.by_ref())
                    .map(|(c, output)| CropReading { kind: c.kind, bbox: c.bbox, output })
                    .collect();
                PageReading { skew, plan, crops, symbols: symbols.next().unwrap_or_default(), timings, delta }
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
