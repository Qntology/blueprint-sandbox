//! 설비 심볼 판독 (개선안: 글자 없는 그림 심볼).
//!
//! gocr 은 글자만 읽습니다. 상자에 KKS · 이름 없이 나비넥타이(▷◁) 그림만 있으면 빈 글자나 `><` 를 냅니다.
//! 그래서 그림은 SigLIP2 비전 특징으로 따로 봅니다.
//!
//! 1. [`candidates`]: 검은 잉크(붉은 펜 · 회색 인쇄 · 파란 손글씨는 뺌)의 연결 요소 가운데 심볼 크기 · 모양인 것.
//!    큰 요소 안에 든 작은 조각(발전기 원 안의 사인파)은 합칩니다. 줄마다 잉크 구간(런)으로 묶어서 A0 한 장(3,500만 화소)도 화소 표 없이 셉니다.
//! 2. [`SymbolDict`] (그림 사전): 종류별 표본의 SigLIP2 벡터 평균(원형)과 코사인 유사도.
//!    글자 · 배관 · 표 칸 표본으로 만든 `none` 원형이 가장 가깝거나, 유사도 · 1 · 2위 차이가 기준보다 낮으면 버립니다 (모르는 그림은 모른다고 합니다).
//!
//! 사전 파일은 문서 영역 프로브(`crop_probe.bin`, zoro-gravity 와 같은 파일)와 다른 `symbol_dict.bin` 입니다. 만드는 법은 `examples/symbol_probe.rs`.

use std::collections::HashMap;
use std::path::Path;

use anyhow::{bail, Context, Result};
use image::{Rgb, RgbImage};
use serde::Serialize;

/// 그림 사전 파일 이름 (SigLIP2 모델 폴더 안)
pub const DICT_FILE: &str = "symbol_dict.bin";
/// 심볼이 아닌 조각의 종류 이름
pub const NONE: &str = "none";
const MAGIC: &[u8; 4] = b"ZZS1";

/// 후보 추출 기준. 기본값은 2000 px 너비 도면(마스터 · A0 의 한 칸)의 한 변 60 px 심볼에 맞춘 값입니다.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CandidateParams {
    /// 심볼 상자의 긴 변 범위 (px)
    pub min_side: u32,
    pub max_side: u32,
    /// 짧은 변 / 긴 변의 하한
    pub min_aspect: f32,
    /// 잉크 비율 (잉크 화소 / 상자 넓이) 범위. 선 그림은 낮고, 칠한 덩어리 · 굵은 글자 뭉치는 높습니다.
    pub min_fill: f32,
    pub max_fill: f32,
    /// 이보다 어두우면 잉크 (밝기 0 ~ 255). 회색 인쇄(배관 이름 · 범례, 밝기 130)는 빠집니다.
    pub dark: u8,
}

impl Default for CandidateParams {
    fn default() -> Self {
        Self { min_side: 34, max_side: 120, min_aspect: 0.35, min_fill: 0.04, max_fill: 0.62, dark: 120 }
    }
}

impl CandidateParams {
    /// 도면 배율이 다를 때 (예: 2배로 스캔한 도면은 2.0)
    pub fn scaled(scale: f32) -> Self {
        let d = Self::default();
        Self { min_side: (d.min_side as f32 * scale).round() as u32, max_side: (d.max_side as f32 * scale).round() as u32, ..d }
    }
}

/// 심볼 후보 하나. 상자는 `[x0, y0, x1, y1)` 화소 좌표입니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct Candidate {
    pub bbox: [u32; 4],
    pub ink: u32,
}

impl Candidate {
    pub fn width(&self) -> u32 {
        self.bbox[2] - self.bbox[0]
    }

    pub fn height(&self) -> u32 {
        self.bbox[3] - self.bbox[1]
    }

    pub fn fill(&self) -> f32 {
        self.ink as f32 / (self.width() * self.height()).max(1) as f32
    }
}

fn is_ink(p: &Rgb<u8>, dark: u8) -> bool {
    let [r, g, b] = p.0;
    let lum = (r as u32 * 77 + g as u32 * 150 + b as u32 * 29) >> 8;
    let chroma = r.max(g).max(b) - r.min(g).min(b);
    lum < dark as u32 && chroma < 60
}

struct Dsu(Vec<u32>);

impl Dsu {
    fn find(&mut self, mut x: u32) -> u32 {
        while self.0[x as usize] != x {
            let up = self.0[self.0[x as usize] as usize];
            self.0[x as usize] = up;
            x = up;
        }
        x
    }

    fn union(&mut self, a: u32, b: u32) {
        let (ra, rb) = (self.find(a), self.find(b));
        if ra != rb {
            self.0[ra.max(rb) as usize] = ra.min(rb);
        }
    }
}

/// 8방향 연결 요소 (상자 · 잉크 화소 수). 줄마다 잉크 구간을 만들고 윗줄 구간과 닿으면 합칩니다.
pub fn components(img: &RgbImage, dark: u8) -> Vec<Candidate> {
    let (w, h) = img.dimensions();
    let mut runs: Vec<(u32, u32, u32)> = Vec::new();
    let mut dsu = Dsu(Vec::new());
    let mut prev = 0..0;
    for y in 0..h {
        let start = runs.len();
        let mut x = 0;
        while x < w {
            if !is_ink(img.get_pixel(x, y), dark) {
                x += 1;
                continue;
            }
            let x0 = x;
            while x < w && is_ink(img.get_pixel(x, y), dark) {
                x += 1;
            }
            dsu.0.push(runs.len() as u32);
            runs.push((y, x0, x - 1));
        }
        let mut j = prev.start;
        for i in start..runs.len() {
            let (_, a0, a1) = runs[i];
            while j < prev.end && runs[j].2 + 1 < a0 {
                j += 1;
            }
            let mut k = j;
            while k < prev.end && runs[k].1 <= a1 + 1 {
                dsu.union(i as u32, k as u32);
                k += 1;
            }
        }
        prev = start..runs.len();
    }
    let mut index: HashMap<u32, usize> = HashMap::new();
    let mut out: Vec<Candidate> = Vec::new();
    for (i, &(y, x0, x1)) in runs.iter().enumerate() {
        let root = dsu.find(i as u32);
        let n = *index.entry(root).or_insert_with(|| {
            out.push(Candidate { bbox: [x0, y, x1 + 1, y + 1], ink: 0 });
            out.len() - 1
        });
        let c = &mut out[n];
        c.bbox = [c.bbox[0].min(x0), c.bbox[1].min(y), c.bbox[2].max(x1 + 1), c.bbox[3].max(y + 1)];
        c.ink += x1 - x0 + 1;
    }
    out
}

/// 심볼 후보. 큰 상자 테두리 · 배관(한 덩어리로 이어져 큼), 글자 낱자(작음), 가는 선 · 칠한 덩어리(모양)는 빠집니다.
pub fn candidates(img: &RgbImage, p: &CandidateParams) -> Vec<Candidate> {
    let (w, h) = img.dimensions();
    let small: Vec<Candidate> = components(img, p.dark).into_iter().filter(|c| c.width() <= p.max_side && c.height() <= p.max_side).collect();
    let mut seeds: Vec<Candidate> = small
        .iter()
        .filter(|c| c.width().max(c.height()) >= p.min_side)
        .filter(|c| c.bbox[0] > 0 && c.bbox[1] > 0 && c.bbox[2] < w && c.bbox[3] < h)
        .copied()
        .collect();
    // 씨앗 상자 안에 든 작은 조각을 합칩니다 (발전기 원 안의 사인파, 끊긴 획).
    for c in small.iter().filter(|c| c.width().max(c.height()) < p.min_side) {
        if let Some(s) = seeds.iter_mut().find(|s| s.bbox[0] <= c.bbox[0] + 2 && s.bbox[1] <= c.bbox[1] + 2 && c.bbox[2] <= s.bbox[2] + 2 && c.bbox[3] <= s.bbox[3] + 2) {
            s.ink += c.ink;
        }
    }
    seeds.retain(|s| {
        let aspect = s.width().min(s.height()) as f32 / s.width().max(s.height()) as f32;
        aspect >= p.min_aspect && (p.min_fill..=p.max_fill).contains(&s.fill())
    });
    seeds.sort_by_key(|s| (s.bbox[1], s.bbox[0]));
    seeds
}

/// 후보를 가운데 두고 긴 변의 1.5 배 정사각형으로 자릅니다 (도면 밖은 흰 바탕). 그림 사전 표본과 같은 여백입니다.
pub fn crop_square(img: &RgbImage, bbox: [u32; 4]) -> RgbImage {
    let side = ((bbox[2] - bbox[0]).max(bbox[3] - bbox[1]) as f32 * 1.5).round().max(8.0) as i64;
    let cx = (bbox[0] + bbox[2]) as i64 / 2;
    let cy = (bbox[1] + bbox[3]) as i64 / 2;
    let (x0, y0) = (cx - side / 2, cy - side / 2);
    let (w, h) = (img.width() as i64, img.height() as i64);
    RgbImage::from_fn(side as u32, side as u32, |x, y| {
        let (sx, sy) = (x0 + x as i64, y0 + y as i64);
        if (0..w).contains(&sx) && (0..h).contains(&sy) {
            *img.get_pixel(sx as u32, sy as u32)
        } else {
            Rgb([255, 255, 255])
        }
    })
}

/// `bbox` 안쪽(`margin` 만큼 줄인 영역)에 있는 잉크의 범위. 테두리 괘선은 margin 으로 빼고, 옅은 빗금(훼손 표시) · 회색 인쇄는 잉크가 아닙니다.
pub fn ink_extent(img: &RgbImage, bbox: [u32; 4], margin: u32, dark: u8) -> Option<[u32; 4]> {
    let (x0, y0) = (bbox[0] + margin, bbox[1] + margin);
    let (x1, y1) = (bbox[2].saturating_sub(margin).min(img.width()), bbox[3].saturating_sub(margin).min(img.height()));
    let mut out: Option<[u32; 4]> = None;
    for y in y0..y1 {
        for x in x0..x1 {
            if is_ink(img.get_pixel(x, y), dark) {
                out = Some(out.map_or([x, y, x + 1, y + 1], |o| [o[0].min(x), o[1].min(y), o[2].max(x + 1), o[3].max(y + 1)]));
            }
        }
    }
    out
}

/// 사전이 쓰는 SigLIP2 벡터
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Feature {
    /// 비전 타워의 어텐션 풀링 출력
    Pooled,
    /// 패치 벡터의 평균
    MeanPatch,
}

pub fn normalize(v: &mut [f32]) {
    let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if n > 0.0 {
        v.iter_mut().for_each(|x| *x /= n);
    }
}

fn dot(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// 그림 하나를 사전에 대어 본 결과
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolMatch {
    /// 받아들인 심볼 종류. `none` 이 가장 가깝거나 기준 미달이면 None
    pub class: Option<String>,
    /// 가장 가까운 종류 (`none` 포함)
    pub nearest: String,
    /// 가장 가까운 종류와의 코사인 유사도
    pub sim: f32,
    /// 1위와, 1위가 아닌 종류 가운데 가장 가까운 것의 유사도 차이
    pub margin: f32,
}

/// SigLIP2 그림 사전 (종류별 원형 벡터)
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolDict {
    pub feature: Feature,
    /// 종류 이름 (`none` 포함)
    pub classes: Vec<String>,
    pub dim: usize,
    /// (종류 번호, 단위 벡터). 종류마다 하나 이상입니다.
    pub protos: Vec<(usize, Vec<f32>)>,
    /// 받아들이는 최소 유사도 · 최소 차이
    pub min_sim: f32,
    pub min_margin: f32,
}

impl SymbolDict {
    /// 종류별 표본 벡터의 평균을 원형으로 둡니다. 벡터는 정규화되어 있어야 합니다.
    pub fn from_samples(feature: Feature, classes: Vec<String>, samples: &[(usize, Vec<f32>)]) -> Result<Self> {
        let dim = samples.first().map(|s| s.1.len()).context("표본이 없습니다")?;
        let mut protos = Vec::new();
        for c in 0..classes.len() {
            let mut sum = vec![0.0f32; dim];
            let mut n = 0;
            for (k, v) in samples.iter().filter(|s| s.0 == c) {
                debug_assert_eq!(*k, c);
                sum.iter_mut().zip(v).for_each(|(a, b)| *a += b);
                n += 1;
            }
            if n == 0 {
                bail!("종류 {} 의 표본이 없습니다", classes[c]);
            }
            normalize(&mut sum);
            protos.push((c, sum));
        }
        Ok(Self { feature, classes, dim, protos, min_sim: 0.0, min_margin: 0.0 })
    }

    /// 종류마다 가장 가까운 원형과의 코사인 유사도
    pub fn scores(&self, v: &[f32]) -> Vec<f32> {
        let mut best = vec![f32::NEG_INFINITY; self.classes.len()];
        for (c, p) in &self.protos {
            best[*c] = best[*c].max(dot(p, v));
        }
        best
    }

    pub fn classify(&self, v: &[f32]) -> SymbolMatch {
        let scores = self.scores(v);
        let mut order: Vec<usize> = (0..scores.len()).collect();
        order.sort_by(|a, b| scores[*b].total_cmp(&scores[*a]));
        let (top, second) = (order[0], order.get(1).copied());
        let sim = scores[top];
        let margin = second.map_or(sim, |s| sim - scores[s]);
        let nearest = self.classes[top].clone();
        let accepted = nearest != NONE && sim >= self.min_sim && margin >= self.min_margin;
        SymbolMatch { class: accepted.then(|| nearest.clone()), nearest, sim, margin }
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut b = MAGIC.to_vec();
        b.push(match self.feature {
            Feature::Pooled => 0,
            Feature::MeanPatch => 1,
        });
        b.extend((self.classes.len() as u32).to_le_bytes());
        for c in &self.classes {
            b.extend((c.len() as u16).to_le_bytes());
            b.extend(c.as_bytes());
        }
        b.extend((self.dim as u32).to_le_bytes());
        b.extend((self.protos.len() as u32).to_le_bytes());
        for (c, v) in &self.protos {
            b.extend((*c as u32).to_le_bytes());
            v.iter().for_each(|x| b.extend(x.to_le_bytes()));
        }
        b.extend(self.min_sim.to_le_bytes());
        b.extend(self.min_margin.to_le_bytes());
        b
    }

    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut at = 0usize;
        let mut take = |n: usize| -> Result<&[u8]> {
            let s = b.get(at..at + n).context("그림 사전 파일이 잘렸습니다")?;
            at += n;
            Ok(s)
        };
        if take(4)? != MAGIC {
            bail!("그림 사전 파일이 아닙니다");
        }
        let feature = match take(1)?[0] {
            0 => Feature::Pooled,
            1 => Feature::MeanPatch,
            x => bail!("모르는 벡터 종류 {x}"),
        };
        let u32_at = |s: &[u8]| u32::from_le_bytes([s[0], s[1], s[2], s[3]]);
        let k = u32_at(take(4)?) as usize;
        let mut classes = Vec::with_capacity(k);
        for _ in 0..k {
            let n = u16::from_le_bytes(take(2)?.try_into()?) as usize;
            classes.push(String::from_utf8(take(n)?.to_vec())?);
        }
        let dim = u32_at(take(4)?) as usize;
        let n = u32_at(take(4)?) as usize;
        let mut protos = Vec::with_capacity(n);
        for _ in 0..n {
            let c = u32_at(take(4)?) as usize;
            if c >= k {
                bail!("원형의 종류 번호가 범위를 벗어납니다");
            }
            let v = take(dim * 4)?.chunks_exact(4).map(|x| f32::from_le_bytes([x[0], x[1], x[2], x[3]])).collect();
            protos.push((c, v));
        }
        let min_sim = f32::from_le_bytes(take(4)?.try_into()?);
        let min_margin = f32::from_le_bytes(take(4)?.try_into()?);
        Ok(Self { feature, classes, dim, protos, min_sim, min_margin })
    }

    pub fn from_file(path: &Path) -> Result<Self> {
        Self::from_bytes(&std::fs::read(path).with_context(|| format!("그림 사전을 읽을 수 없습니다: {}", path.display()))?)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        std::fs::write(path, self.to_bytes()).with_context(|| format!("그림 사전을 쓸 수 없습니다: {}", path.display()))
    }
}

/// 도면에서 찾은 심볼 하나
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SymbolHit {
    pub bbox: [u32; 4],
    #[serde(flatten)]
    pub matched: SymbolMatch,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> RgbImage {
        RgbImage::from_pixel(400, 300, Rgb([255, 255, 255]))
    }

    fn rect(img: &mut RgbImage, [x0, y0, x1, y1]: [u32; 4], color: [u8; 3], width: u32) {
        for y in y0..y1 {
            for x in x0..x1 {
                let edge = x < x0 + width || x + width >= x1 || y < y0 + width || y + width >= y1;
                if edge {
                    img.put_pixel(x, y, Rgb(color));
                }
            }
        }
    }

    #[test]
    fn components_follow_8_connectivity_and_count_ink() {
        let mut img = page();
        rect(&mut img, [10, 10, 30, 20], [0, 0, 0], 1);
        // 대각선으로만 닿는 두 점은 한 요소입니다.
        img.put_pixel(100, 100, Rgb([0, 0, 0]));
        img.put_pixel(101, 101, Rgb([0, 0, 0]));
        let mut c = components(&img, 120);
        c.sort_by_key(|x| x.bbox);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].bbox, [10, 10, 30, 20]);
        assert_eq!(c[0].ink, 2 * 20 + 2 * 8);
        assert_eq!(c[1].bbox, [100, 100, 102, 102]);
        assert_eq!(c[1].ink, 2);
    }

    #[test]
    fn symbols_are_kept_and_boxes_glyphs_red_ink_and_blobs_are_dropped() {
        let mut img = page();
        // 심볼 크기의 선 그림 (60 px 정사각 + 대각선 하나)
        rect(&mut img, [50, 50, 110, 110], [20, 20, 25], 3);
        for i in 0..60 {
            img.put_pixel(50 + i, 50 + i, Rgb([20, 20, 25]));
        }
        let alone = components(&img, 120).iter().find(|c| c.bbox == [50, 50, 110, 110]).unwrap().ink;
        // 그 안의 작은 조각은 합쳐집니다 (10 × 10 테두리 = 36 화소).
        rect(&mut img, [70, 85, 80, 95], [20, 20, 25], 1);
        // 큰 상자 (테두리), 작은 낱자, 붉은 도장, 칠한 덩어리
        rect(&mut img, [150, 20, 390, 290], [20, 20, 25], 3);
        rect(&mut img, [170, 40, 182, 56], [20, 20, 25], 2);
        rect(&mut img, [200, 100, 260, 160], [205, 25, 25], 4);
        rect(&mut img, [280, 100, 330, 150], [20, 20, 25], 25);
        let c = candidates(&img, &CandidateParams::default());
        assert_eq!(c.len(), 1, "{c:?}");
        assert_eq!(c[0].bbox, [50, 50, 110, 110]);
        assert_eq!(c[0].ink, alone + 36, "안쪽 조각을 더했습니다");
    }

    #[test]
    fn the_square_crop_pads_with_white_outside_the_page() {
        let mut img = page();
        rect(&mut img, [0, 0, 40, 20], [0, 0, 0], 2);
        let crop = crop_square(&img, [0, 0, 40, 20]);
        assert_eq!(crop.dimensions(), (60, 60));
        assert_eq!(crop.get_pixel(0, 0).0, [255, 255, 255], "도면 밖은 흰 바탕");
        assert_eq!(crop.get_pixel(10, 20).0, [0, 0, 0], "잘라 낸 자리에는 원래 잉크");
    }

    fn unit(v: &[f32]) -> Vec<f32> {
        let mut v = v.to_vec();
        normalize(&mut v);
        v
    }

    #[test]
    fn the_dictionary_picks_the_nearest_prototype_and_rejects_none_or_weak_matches() {
        let classes = vec!["pump".to_string(), "valve".to_string(), NONE.to_string()];
        let samples = vec![
            (0, unit(&[1.0, 0.1, 0.0])),
            (0, unit(&[1.0, -0.1, 0.0])),
            (1, unit(&[0.0, 1.0, 0.1])),
            (2, unit(&[0.0, 0.0, 1.0])),
        ];
        let mut dict = SymbolDict::from_samples(Feature::Pooled, classes, &samples).unwrap();
        assert_eq!(dict.protos.len(), 3);
        let m = dict.classify(&unit(&[0.9, 0.2, 0.0]));
        assert_eq!(m.class.as_deref(), Some("pump"));
        assert!(m.margin > 0.0);
        let none = dict.classify(&unit(&[0.1, 0.1, 1.0]));
        assert_eq!(none.nearest, NONE);
        assert_eq!(none.class, None, "글자 · 배관 조각이 가장 가까우면 버립니다");
        // 둘 사이 애매한 그림은 차이 기준으로 버립니다.
        dict.min_margin = 0.2;
        let unsure = dict.classify(&unit(&[0.7, 0.7, 0.0]));
        assert_eq!(unsure.class, None, "{unsure:?}");
        // 파일로 쓰고 다시 읽어도 같습니다.
        let again = SymbolDict::from_bytes(&dict.to_bytes()).unwrap();
        assert_eq!(again, dict);
        assert!(SymbolDict::from_bytes(b"ZZP1....").is_err(), "문서 영역 프로브 파일과 섞이지 않습니다");
    }
}
