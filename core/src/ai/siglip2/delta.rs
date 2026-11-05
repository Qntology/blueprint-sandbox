//! SDS 델타 패치: 마스터 도면과 비교해 바뀐 곳만 다시 읽습니다 (`docs/powerplant-timeseries-sds-plan.md` Stage 1 · 2).
//!
//! 두 가지를 마스터마다 미리 만들어 둡니다 ([`Master`]).
//! - **어느 마스터인가**: SigLIP2 패치 특징(int8, 패치마다 배율 하나 · 1,152 차원 → 1.1 KB). 스캔이 들어오면 조각 계획에 쓰는
//!   그 특징 격자를 마스터마다 견주어(패치 코사인 평균) 가장 닮은 것을 고르고, 두 번째와 차이가 작으면 전부 읽습니다.
//! - **무엇이 바뀌었나**: 잉크. 스캔의 잉크 가운데 마스터 잉크를 `radius` 화소 넓힌 영역 밖에 있는 것(새 획)과,
//!   마스터 잉크 가운데 스캔 잉크를 넓힌 영역 밖에 있는 것(지운 획)을 칸마다 셉니다 ([`ink_changes`]).
//!   바뀐 칸을 덮지 않는 조각은 gocr 에 넣지 않습니다.
//!
//! 바뀐 곳을 패치 코사인으로 고르지 않는 까닭 (실측, `docs/verification.md`): 스캔 흐림 · 색조 · 잡음만으로 마스터와 같은 도면의
//! 패치 코사인 평균이 0.69 로 떨어져, 개선안의 기준 0.92 로는 거의 모든 패치가 바뀐 것이 됩니다.
//!
//! 표제란(도면번호 · 개정)과 서명은 늘 읽습니다. 구버전 도면은 `Rev.` 한 글자 차이라 잉크로도 작게 보일 수 있고,
//! 도면번호 · 개정은 MOC 대조의 근거입니다. 읽은 도면번호가 고른 마스터와 다르면 부르는 쪽이 전부 다시 읽어야 합니다.
//! 어긋남은 기울기 보정 뒤 `radius` 화소까지만 견딥니다 (배율 · 이동 맞춤은 하지 않음).

use anyhow::{bail, Context, Result};

use super::ink::{Bbox, InkPage};
use super::plan::{CropKind, PagePlan};
use super::PatchGrid;

const MAGIC: &[u8; 4] = b"ZZD1";

/// 델타 판독의 기준값
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeltaParams {
    /// 가장 닮은 마스터와 두 번째의 패치 코사인 평균 차이가 이보다 작으면 어느 마스터인지 확신하지 못해 전부 읽습니다.
    pub min_margin: f32,
    /// 같은 획으로 볼 어긋남 (화소). 기울기 보정의 남은 오차 · 스캐너 흐림을 덮습니다.
    pub radius: u32,
    /// 잉크를 세는 칸 크기 (화소)
    pub block: u32,
    /// 칸 넓이 대비 새 획 + 지운 획 화소가 이 비율을 넘으면 바뀐 칸
    pub ink_threshold: f32,
    /// 페이지 가장자리 이 폭(화소)은 보지 않습니다 (스캐너 가장자리 그늘).
    pub edge: u32,
    /// 바뀐 칸이 이 비율을 넘으면(다른 장 · 크게 고친 장) 델타를 쓰지 않고 전부 읽습니다.
    pub max_changed: f32,
    /// 남긴 조각을 바뀐 곳 둘레로 가로로 좁혀 읽습니다 ([`clip_to_changes`]).
    pub clip: bool,
}

impl Default for DeltaParams {
    fn default() -> Self {
        Self { min_margin: 0.03, radius: 3, block: 16, ink_threshold: 0.03, edge: 32, max_changed: 0.25, clip: true }
    }
}

/// 마스터 한 장의 SigLIP2 패치 특징 (int8)
#[derive(Debug, Clone, PartialEq)]
pub struct MasterPatches {
    pub id: String,
    pub rows: usize,
    pub cols: usize,
    pub dim: usize,
    /// 특징을 뽑은 그림의 크기 (곧게 편 페이지)
    pub width: u32,
    pub height: u32,
    /// 패치마다 배율 (`값 ≈ q × scale`)
    scales: Vec<f32>,
    /// `[rows × cols, dim]`
    codes: Vec<i8>,
    /// 패치마다 `‖q‖` (코사인용)
    norms: Vec<f32>,
}

impl MasterPatches {
    pub fn from_grid(id: impl Into<String>, grid: &PatchGrid) -> Self {
        let n = grid.len();
        let mut scales = Vec::with_capacity(n);
        let mut codes = Vec::with_capacity(n * grid.dim);
        let mut norms = Vec::with_capacity(n);
        for i in 0..n {
            let p = grid.patch(i);
            let max = p.iter().fold(0f32, |m, v| m.max(v.abs()));
            let scale = if max > 0.0 { max / 127.0 } else { 1.0 };
            let mut sq = 0f32;
            for v in p {
                let q = (v / scale).round().clamp(-127.0, 127.0) as i8;
                sq += (q as f32) * (q as f32);
                codes.push(q);
            }
            scales.push(scale);
            norms.push(sq.sqrt());
        }
        Self { id: id.into(), rows: grid.rows, cols: grid.cols, dim: grid.dim, width: grid.width, height: grid.height, scales, codes, norms }
    }

    pub fn len(&self) -> usize {
        self.rows * self.cols
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 호스트에 두는 크기 (바이트)
    pub fn bytes(&self) -> usize {
        self.codes.len() + (self.scales.len() + self.norms.len()) * 4
    }

    /// 패치 하나를 되살린 값
    pub fn patch(&self, i: usize) -> Vec<f32> {
        self.codes[i * self.dim..(i + 1) * self.dim].iter().map(|q| *q as f32 * self.scales[i]).collect()
    }

    /// 같은 격자의 스캔과 패치마다 코사인 (스캔 특징은 L2 정규화돼 있음). 격자가 다르면 None.
    pub fn cosines(&self, scan: &PatchGrid) -> Option<Vec<f32>> {
        if scan.rows != self.rows || scan.cols != self.cols || scan.dim != self.dim {
            return None;
        }
        Some(
            (0..self.len())
                .map(|i| {
                    let q = &self.codes[i * self.dim..(i + 1) * self.dim];
                    let s = scan.patch(i);
                    let dot: f32 = s.iter().zip(q).map(|(s, q)| s * *q as f32).sum();
                    let norm = s.iter().map(|v| v * v).sum::<f32>().sqrt() * self.norms[i];
                    if norm > 0.0 {
                        dot / norm
                    } else {
                        1.0
                    }
                })
                .collect(),
        )
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&(self.id.len() as u32).to_le_bytes());
        out.extend_from_slice(self.id.as_bytes());
        for v in [self.rows as u32, self.cols as u32, self.dim as u32, self.width, self.height] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        for v in self.scales.iter().chain(&self.norms) {
            out.extend_from_slice(&v.to_le_bytes());
        }
        out.extend(self.codes.iter().map(|q| *q as u8));
    }
}

/// 패치 특징 파일 (`ZZD1`): 장 수, 장마다 이름 · 격자 · 배율 · ‖q‖ · int8 값
pub fn write_patches(pages: &[&MasterPatches]) -> Vec<u8> {
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&(pages.len() as u32).to_le_bytes());
    for p in pages {
        p.write(&mut out);
    }
    out
}

pub fn read_patches(raw: &[u8]) -> Result<Vec<MasterPatches>> {
    let mut r = Reader { raw, at: 0 };
    if r.take(4)? != MAGIC {
        bail!("마스터 패치 파일의 머리 표식이 다릅니다");
    }
    let n = r.u32()? as usize;
    let mut pages = Vec::with_capacity(n);
    for _ in 0..n {
        let len = r.u32()? as usize;
        let id = String::from_utf8(r.take(len)?.to_vec())?;
        let (rows, cols, dim, width, height) = (r.u32()? as usize, r.u32()? as usize, r.u32()? as usize, r.u32()?, r.u32()?);
        let cells = rows * cols;
        let scales = r.f32s(cells)?;
        let norms = r.f32s(cells)?;
        let codes: Vec<i8> = r.take(cells * dim)?.iter().map(|b| *b as i8).collect();
        pages.push(MasterPatches { id, rows, cols, dim, width, height, scales, codes, norms });
    }
    Ok(pages)
}

struct Reader<'a> {
    raw: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        let end = self.at.checked_add(n).filter(|e| *e <= self.raw.len()).context("마스터 패치 파일이 잘렸습니다")?;
        let out = &self.raw[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32s(&mut self, n: usize) -> Result<Vec<f32>> {
        Ok(self.take(n * 4)?.as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect())
    }
}

/// 마스터 한 장: 어느 마스터인지 가릴 패치 특징과, 무엇이 바뀌었는지 볼 잉크 (곧게 편 페이지)
pub struct Master {
    pub patches: MasterPatches,
    pub ink: InkPage,
    /// 잉크를 `radius` 넓힌 지도
    near: Vec<u8>,
    radius: u32,
}

impl Master {
    pub fn new(id: impl Into<String>, grid: &PatchGrid, ink: InkPage, radius: u32) -> Self {
        let near = ink.dilate(radius);
        Self { patches: MasterPatches::from_grid(id, grid), ink, near, radius }
    }

    pub fn id(&self) -> &str {
        &self.patches.id
    }
}

/// 마스터 여러 장
#[derive(Default)]
pub struct MasterSet {
    pub pages: Vec<Master>,
}

/// 스캔 한 장이 어느 마스터와 닮았는지
#[derive(Debug, Clone, PartialEq)]
pub struct Matched {
    pub index: usize,
    /// 패치 코사인 평균
    pub similarity: f32,
    /// 두 번째로 닮은 마스터와의 차이 (마스터가 하나면 1)
    pub margin: f32,
}

impl MasterSet {
    pub fn get(&self, id: &str) -> Option<&Master> {
        self.pages.iter().find(|p| p.id() == id)
    }

    /// 격자가 같은 마스터 가운데 가장 닮은 것
    pub fn best_match(&self, scan: &PatchGrid) -> Option<Matched> {
        let mut scored: Vec<(usize, f32)> = self
            .pages
            .iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let c = m.patches.cosines(scan)?;
                Some((i, c.iter().sum::<f32>() / c.len().max(1) as f32))
            })
            .collect();
        scored.sort_by(|a, b| b.1.total_cmp(&a.1));
        let (index, similarity) = *scored.first()?;
        let margin = scored.get(1).map(|s| similarity - s.1).unwrap_or(1.0);
        Some(Matched { index, similarity, margin })
    }

    /// 패치 특징만 파일로 (`ZZD1`)
    pub fn patches_to_bytes(&self) -> Vec<u8> {
        write_patches(&self.pages.iter().map(|p| &p.patches).collect::<Vec<_>>())
    }
}

/// 칸마다 새 획 + 지운 획 화소의 비율
#[derive(Debug, Clone, PartialEq)]
pub struct BlockDiff {
    pub block: u32,
    pub rows: u32,
    pub cols: u32,
    pub diff: Vec<f32>,
}

impl BlockDiff {
    /// 비율이 `threshold` 를 넘는 칸 수
    pub fn changed(&self, threshold: f32) -> usize {
        self.diff.iter().filter(|d| **d > threshold).count()
    }

    pub fn max(&self) -> f32 {
        self.diff.iter().fold(0f32, |m, d| m.max(*d))
    }

    /// 바뀐 칸 상자 (화소)
    pub fn changed_boxes(&self, threshold: f32) -> Vec<Bbox> {
        self.diff
            .iter()
            .enumerate()
            .filter(|(_, d)| **d > threshold)
            .map(|(i, _)| {
                let (r, c) = (i as u32 / self.cols, i as u32 % self.cols);
                (c * self.block, r * self.block, (c + 1) * self.block, (r + 1) * self.block)
            })
            .collect()
    }

    /// 상자가 바뀐 칸에 걸치는가
    pub fn touches(&self, (x0, y0, x1, y1): Bbox, threshold: f32) -> bool {
        let b = self.block;
        let (c0, c1) = (x0 / b, (x1.max(x0 + 1) - 1) / b);
        let (r0, r1) = (y0 / b, (y1.max(y0 + 1) - 1) / b);
        (r0..=r1.min(self.rows.saturating_sub(1))).any(|r| (c0..=c1.min(self.cols.saturating_sub(1))).any(|c| self.diff[(r * self.cols + c) as usize] > threshold))
    }
}

/// 마스터와 스캔(같은 크기, 곧게 편 페이지)의 잉크를 견주어 칸마다 새 획 + 지운 획 비율을 잽니다. 크기가 다르면 None.
/// 서로 `radius` 화소 안의 잉크는 같은 획으로 봅니다. 가장자리 `edge` 화소는 세지 않습니다.
pub fn ink_changes(master: &Master, scan: &InkPage, p: &DeltaParams) -> Option<BlockDiff> {
    let (w, h) = (scan.width, scan.height);
    if (master.ink.width, master.ink.height) != (w, h) || p.block == 0 {
        return None;
    }
    let scan_near = scan.dilate(p.radius);
    let master_near = if p.radius == master.radius { std::borrow::Cow::Borrowed(&master.near) } else { std::borrow::Cow::Owned(master.ink.dilate(p.radius)) };
    let (cols, rows) = (w.div_ceil(p.block), h.div_ceil(p.block));
    let mut counts = vec![0u32; (rows * cols) as usize];
    let (sm, mm) = (scan.mask(), master.ink.mask());
    for y in p.edge..h.saturating_sub(p.edge) {
        let row = (y / p.block) * cols;
        let s = (y * w) as usize;
        for x in p.edge..w.saturating_sub(p.edge) {
            let i = s + x as usize;
            let added = sm[i] == 1 && master_near[i] == 0;
            let removed = mm[i] == 1 && scan_near[i] == 0;
            if added || removed {
                counts[(row + x / p.block) as usize] += 1;
            }
        }
    }
    let diff = counts
        .iter()
        .enumerate()
        .map(|(i, n)| {
            let (r, c) = (i as u32 / cols, i as u32 % cols);
            let area = ((c + 1) * p.block).min(w).saturating_sub(c * p.block) * ((r + 1) * p.block).min(h).saturating_sub(r * p.block);
            *n as f32 / area.max(1) as f32
        })
        .collect();
    Some(BlockDiff { block: p.block, rows, cols, diff })
}

/// 상자가 덮는 격자 칸 (행 우선 인덱스)
pub fn cells_of((x0, y0, x1, y1): Bbox, page: (u32, u32), rows: usize, cols: usize) -> Vec<usize> {
    let (w, h) = (page.0.max(1) as usize, page.1.max(1) as usize);
    let (x1, y1) = (x1.max(x0 + 1) as usize, y1.max(y0 + 1) as usize);
    let (r0, r1) = (y0 as usize * rows / h, ((y1 - 1) * rows / h).min(rows - 1));
    let (c0, c1) = (x0 as usize * cols / w, ((x1 - 1) * cols / w).min(cols - 1));
    (r0..=r1).flat_map(|r| (c0..=c1).map(move |c| r * cols + c)).collect()
}

/// 좁힌 구간이 조각 너비의 이 비율을 넘으면 좁히지 않습니다.
const CLIP_KEEP_SHARE: f32 = 0.6;
/// 바뀐 칸 구간 사이가 이 거리(글자 높이 배수)보다 가까우면 한 구간으로 합칩니다.
const CLIP_MERGE_UNITS: f32 = 4.0;
/// 바뀐 구간을 이만큼(글자 높이 배수) 넓혀 읽습니다.
const CLIP_MARGIN_UNITS: f32 = 3.0;
/// 넓힌 끝에서 바깥쪽으로 빈 세로 틈을 찾는 거리 (글자 높이 배수)
const CLIP_SNAP_UNITS: f32 = 8.0;
/// 빈 세로 틈으로 칠 최소 폭 (글자 높이 배수). 글자 사이 틈(2 ~ 4 px)보다 넓어야 낱말을 자르지 않습니다.
const CLIP_GAP_UNITS: f32 = 0.5;
/// 조각을 자를 때 남기는 여백 (화소, `plan` 과 같음)
const CLIP_PAD: u32 = 6;

/// 남긴 조각을 바뀐 곳 둘레로 가로로 좁힙니다. 도면 조각은 너비 전체의 가로 띠라서 상자 하나만 바뀌어도 띠 전체를 읽게 됩니다.
/// 바뀐 칸의 가로 구간을 합치고 글자 높이의 3 배만큼 넓힌 뒤, 바깥쪽으로 가장 가까운 빈 세로 틈(글자 사이 틈보다 넓은 것)까지 늘립니다.
/// 좁힌 구간이 조각 너비의 60 % 를 넘으면 그대로 두고, 좁힌 조각에 글자 모양 덩어리가 둘 미만이면 읽지 않습니다.
/// 제목 · 서명은 건드리지 않습니다. 돌려주는 값: (좁힌 조각 수, 좁힌 뒤 글자가 없어 뺀 상자)
pub fn clip_to_changes(plan: &mut PagePlan, ink: &InkPage, diff: &BlockDiff, threshold: f32) -> (usize, Vec<Bbox>) {
    let unit = ink.text_unit();
    let (merge, margin, snap, gap) = ((unit * CLIP_MERGE_UNITS) as u32, (unit * CLIP_MARGIN_UNITS) as u32, (unit * CLIP_SNAP_UNITS) as u32, ((unit * CLIP_GAP_UNITS) as u32).max(3));
    let b = diff.block;
    let mut clipped = 0;
    let mut blank = Vec::new();
    let mut out = Vec::with_capacity(plan.crops.len());
    for c in std::mem::take(&mut plan.crops) {
        let (x0, y0, x1, y1) = c.bbox;
        if matches!(c.kind, CropKind::Title | CropKind::Sign) || x1 <= x0 + 1 || y1 <= y0 + 1 || diff.cols == 0 || diff.rows == 0 {
            out.push(c);
            continue;
        }
        // 조각 안 바뀐 칸의 가로 구간 (화소)
        let (r0, r1) = (y0 / b, ((y1 - 1) / b).min(diff.rows - 1));
        let (c0, c1) = (x0 / b, ((x1 - 1) / b).min(diff.cols - 1));
        let mut spans: Vec<(u32, u32)> = Vec::new();
        for col in c0..=c1 {
            if (r0..=r1).any(|r| diff.diff[(r * diff.cols + col) as usize] > threshold) {
                let (a, z) = ((col * b).max(x0), ((col + 1) * b).min(x1));
                match spans.last_mut() {
                    Some(last) if a <= last.1 + merge => last.1 = z,
                    _ => spans.push((a, z)),
                }
            }
        }
        if spans.is_empty() {
            out.push(c);
            continue;
        }
        // 넓히고 바깥쪽 빈 세로 틈까지 늘림
        let counts = ink.col_counts((x0, y0, x1, y1));
        let thin = ((y1 - y0) as f32 * 0.08).max(4.0) as u32;
        let empty = |x: u32| counts[(x - x0) as usize] <= thin;
        // [s, s + gap) 가 모두 빈 세로줄인가
        let gap_run = |s: u32| s >= x0 && s + gap <= x1 && (s..s + gap).all(empty);
        let mut widened: Vec<(u32, u32)> = Vec::new();
        for (a, z) in spans {
            let mut a2 = a.saturating_sub(margin).max(x0);
            let mut z2 = (z + margin).min(x1);
            // 왼쪽 끝은 가장 가까운 왼쪽 틈 가운데에서, 오른쪽 끝은 가장 가까운 오른쪽 틈 가운데에서 자릅니다.
            if let Some(s) = (a2.saturating_sub(snap).max(x0)..=a2).rev().find(|s| gap_run(*s)) {
                a2 = (s + gap / 2).min(a2);
            }
            if let Some(s) = (z2.saturating_sub(gap).max(x0)..(z2 + snap).min(x1)).find(|s| gap_run(*s)) {
                z2 = (s + gap / 2).max(z2).min(x1);
            }
            match widened.last_mut() {
                Some(last) if a2 <= last.1 => last.1 = last.1.max(z2),
                _ => widened.push((a2, z2)),
            }
        }
        let covered: u32 = widened.iter().map(|(a, z)| z - a).sum();
        if covered as f32 >= (x1 - x0) as f32 * CLIP_KEEP_SHARE {
            out.push(c);
            continue;
        }
        clipped += 1;
        for (a, z) in widened {
            let Some((tx0, ty0, tx1, ty1)) = ink.text_box((a, y0, z, y1)).or_else(|| ink.tighten((a, y0, z, y1))) else { continue };
            let bbox = (tx0.saturating_sub(CLIP_PAD).max(x0), ty0.saturating_sub(CLIP_PAD).max(y0), (tx1 + CLIP_PAD).min(x1), (ty1 + CLIP_PAD).min(y1));
            if ink.glyphs(bbox, 2) < 2 {
                blank.push(bbox);
                continue;
            }
            out.push(super::plan::CropPlan { kind: c.kind, bbox, upscale: c.upscale, guide_margin: None });
        }
    }
    plan.crops = out;
    (clipped, blank)
}

/// 조각 계획에서 바뀐 곳에 걸치지 않는 조각을 빼고, 뺀 조각의 상자를 돌려줍니다. 제목(표제란)과 서명은 늘 남깁니다.
pub fn retain_changed(plan: &mut PagePlan, changed: impl Fn(Bbox) -> bool) -> Vec<Bbox> {
    let mut skipped = Vec::new();
    plan.crops.retain(|c| {
        let keep = matches!(c.kind, CropKind::Title | CropKind::Sign) || changed(c.bbox);
        if !keep {
            skipped.push(c.bbox);
        }
        keep
    });
    skipped
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::siglip2::plan::CropPlan;
    use image::{GrayImage, Luma};

    fn grid(rows: usize, cols: usize, dim: usize, seed: u64) -> PatchGrid {
        let mut s = seed;
        let mut patches = Vec::with_capacity(rows * cols * dim);
        for _ in 0..rows * cols {
            let mut v: Vec<f32> = (0..dim)
                .map(|_| {
                    s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                    ((s >> 33) as f32 / (1u64 << 31) as f32) - 0.5
                })
                .collect();
            let n = v.iter().map(|x| x * x).sum::<f32>().sqrt();
            v.iter_mut().for_each(|x| *x /= n);
            patches.extend(v);
        }
        PatchGrid { rows, cols, dim, patches, pooled: vec![0.0; dim], width: cols as u32 * 100, height: rows as u32 * 100 }
    }

    fn page(w: u32, h: u32, strokes: &[(u32, u32, u32, u32)], paper: u8) -> InkPage {
        let mut g = GrayImage::from_pixel(w, h, Luma([paper]));
        for &(x0, y0, x1, y1) in strokes {
            for y in y0..y1 {
                for x in x0..x1 {
                    g.put_pixel(x, y, Luma([30]));
                }
            }
        }
        InkPage::new(&g)
    }

    #[test]
    fn int8_patches_keep_the_cosine_and_the_file_round_trips() -> Result<()> {
        let g = grid(3, 4, 64, 7);
        let m = MasterPatches::from_grid("PID-103-LAB", &g);
        let cos = m.cosines(&g).unwrap();
        assert!(cos.iter().all(|c| *c > 0.999), "{cos:?}");
        let back = m.patch(5);
        let err: f32 = back.iter().zip(g.patch(5)).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(err < 0.01, "{err}");
        assert!(m.bytes() < g.patches.len() * 4 / 3, "int8 는 f32 의 1/4 남짓");
        let other = MasterPatches::from_grid("PID-101-HAD", &grid(3, 4, 64, 9));
        let raw = write_patches(&[&m, &other]);
        assert_eq!(read_patches(&raw)?, vec![m, other]);
        assert!(read_patches(&raw[..raw.len() - 1]).is_err());
        Ok(())
    }

    #[test]
    fn the_nearest_master_is_found_by_patch_features() {
        let master = grid(3, 4, 64, 7);
        let other = grid(3, 4, 64, 11);
        let mut scan = master.clone();
        let swap = other.patch(6).to_vec();
        scan.patches[6 * 64..7 * 64].copy_from_slice(&swap);
        let ink = || page(400, 300, &[], 250);
        let set = MasterSet { pages: vec![Master::new("다른 도면", &other, ink(), 3), Master::new("원본", &master, ink(), 3)] };
        let best = set.best_match(&scan).unwrap();
        assert_eq!(set.pages[best.index].id(), "원본");
        assert!(best.similarity > 0.9 && best.margin > 0.5, "{best:?}");
        assert!(set.pages[0].patches.cosines(&grid(2, 4, 64, 1)).is_none(), "격자가 다르면 견주지 않습니다");
    }

    #[test]
    fn new_and_erased_strokes_are_found_but_a_small_shift_is_not() {
        let p = DeltaParams { block: 16, edge: 0, ..DeltaParams::default() };
        // 가로 획 둘 · 세로 획 하나
        let strokes = [(10, 10, 60, 13), (10, 40, 60, 43), (80, 5, 83, 60)];
        let master = Master::new("m", &grid(1, 1, 4, 1), page(128, 64, &strokes, 255), p.radius);
        // 스캔: 바탕이 어둡고(색조) 전체가 2 px 밀림 → 바뀐 칸 없음
        let shifted: Vec<_> = strokes.iter().map(|(a, b, c, d)| (a + 2, b + 1, c + 2, d + 1)).collect();
        let d = ink_changes(&master, &page(128, 64, &shifted, 236), &p).unwrap();
        assert_eq!(d.changed(p.ink_threshold), 0, "{:?}", d.diff);
        // 새 획(오른쪽 위)과 지운 획(두 번째 가로 획)
        let edited = [(10, 10, 60, 13), (80, 5, 83, 60), (100, 20, 120, 24)];
        let d = ink_changes(&master, &page(128, 64, &edited, 250), &p).unwrap();
        let boxes = d.changed_boxes(p.ink_threshold);
        assert!(boxes.iter().any(|b| b.0 >= 96 && b.1 <= 20 && b.3 >= 24), "새 획: {boxes:?}");
        assert!(boxes.iter().any(|b| b.1 <= 40 && b.3 >= 43 && b.0 < 60), "지운 획: {boxes:?}");
        assert!(d.touches((100, 18, 126, 30), p.ink_threshold));
        assert!(!d.touches((10, 0, 60, 16), p.ink_threshold), "그대로인 첫 획");
        assert!(ink_changes(&master, &page(64, 64, &[], 250), &p).is_none(), "크기가 다르면 견주지 않습니다");
    }

    #[test]
    fn only_crops_over_changes_survive_but_the_title_and_signature_stay() {
        let crop = |kind, bbox| CropPlan { kind, bbox, upscale: 1.0, guide_margin: None };
        let mut plan = PagePlan {
            width: 400,
            height: 300,
            crops: vec![
                crop(CropKind::Title, (0, 0, 400, 60)),
                crop(CropKind::Form, (10, 110, 180, 190)),
                crop(CropKind::Table, (210, 120, 290, 180)),
                crop(CropKind::Form, (300, 220, 390, 290)),
                crop(CropKind::Sign, (310, 230, 380, 280)),
            ],
            dropped: Vec::new(),
            blank: Vec::new(),
            unfiltered: false,
        };
        let skipped = retain_changed(&mut plan, |b| b.0 >= 200 && b.2 <= 300);
        let kinds: Vec<CropKind> = plan.crops.iter().map(|c| c.kind).collect();
        assert_eq!(kinds, vec![CropKind::Title, CropKind::Table, CropKind::Sign]);
        assert_eq!(skipped, vec![(10, 110, 180, 190), (300, 220, 390, 290)]);
    }

    /// 2000 × 1400 페이지의 가로 띠 하나에 상자 셋 (글자 모양 덩어리: 10 × 14, 낱자 사이 4 px, 낱말 사이 12 px, 상자 사이 300 px)
    fn band_page() -> (InkPage, CropPlan) {
        let mut strokes = Vec::new();
        for box_x in [100u32, 800, 1500] {
            for line_y in [430u32, 460] {
                let mut x = box_x;
                while x + 10 <= box_x + 400 {
                    for _ in 0..5 {
                        if x + 10 > box_x + 400 {
                            break;
                        }
                        strokes.push((x, line_y, x + 10, line_y + 14));
                        x += 14;
                    }
                    x += 8;
                }
            }
        }
        (page(2000, 1400, &strokes, 255), CropPlan { kind: CropKind::Table, bbox: (0, 400, 2000, 520), upscale: 1.0, guide_margin: None })
    }

    fn diff_with(changed: &[(u32, u32, u32, u32)]) -> BlockDiff {
        let (cols, rows) = (2000u32.div_ceil(16), 1400u32.div_ceil(16));
        let mut diff = vec![0f32; (cols * rows) as usize];
        for &(x0, y0, x1, y1) in changed {
            for r in y0 / 16..y1.div_ceil(16) {
                for c in x0 / 16..x1.div_ceil(16) {
                    diff[(r * cols + c) as usize] = 0.5;
                }
            }
        }
        BlockDiff { block: 16, rows, cols, diff }
    }

    #[test]
    fn a_band_is_clipped_to_the_changed_box_at_whitespace() {
        let (ink, band) = band_page();
        let title = CropPlan { kind: CropKind::Title, bbox: (0, 0, 2000, 100), upscale: 1.0, guide_margin: None };
        let mut plan = PagePlan { width: 2000, height: 1400, crops: vec![title.clone(), band], dropped: Vec::new(), blank: Vec::new(), unfiltered: false };
        let (clipped, blank) = clip_to_changes(&mut plan, &ink, &diff_with(&[(950, 440, 1010, 470)]), 0.03);
        assert_eq!((clipped, blank.len()), (1, 0));
        assert_eq!(plan.crops[0], title, "제목은 그대로");
        let c = &plan.crops[1];
        assert!(c.bbox.0 > 500 && c.bbox.0 <= 950 && c.bbox.2 >= 1010 && c.bbox.2 < 1500, "가운데 상자만: {:?}", c.bbox);
        // 자른 끝이 글자 덩어리를 가로지르지 않습니다: 조각 잉크의 바로 바깥 열은 비어 있습니다 (빈 틈에서 자름).
        let column_ink = |x: u32| ink.col_counts((x, 400, x + 1, 520))[0];
        assert!(column_ink(c.bbox.0 + CLIP_PAD) > 0, "왼쪽 끝은 글자에 붙음: {:?}", c.bbox);
        assert_eq!(column_ink(c.bbox.0 + CLIP_PAD - 1), 0, "{:?}", c.bbox);
        assert_eq!(column_ink(c.bbox.2 - CLIP_PAD), 0, "{:?}", c.bbox);
    }

    #[test]
    fn a_band_changed_across_most_of_its_width_is_read_whole() {
        let (ink, band) = band_page();
        let mut plan = PagePlan { width: 2000, height: 1400, crops: vec![band.clone()], dropped: Vec::new(), blank: Vec::new(), unfiltered: false };
        let (clipped, _) = clip_to_changes(&mut plan, &ink, &diff_with(&[(150, 440, 1850, 470)]), 0.03);
        assert_eq!(clipped, 0);
        assert_eq!(plan.crops, vec![band]);
    }

    #[test]
    fn a_clip_with_no_text_is_not_read() {
        let (ink, band) = band_page();
        let mut plan = PagePlan { width: 2000, height: 1400, crops: vec![band], dropped: Vec::new(), blank: Vec::new(), unfiltered: false };
        // 상자 사이 빈 곳 (예: 대각선 화살표 한 획)
        let (clipped, blank) = clip_to_changes(&mut plan, &ink, &diff_with(&[(640, 440, 660, 470)]), 0.03);
        assert_eq!(clipped, 1);
        assert!(plan.crops.is_empty(), "{:?}", plan.crops);
        assert!(blank.len() <= 1);
    }

    #[test]
    fn the_scanner_edge_is_not_counted() {
        let p = DeltaParams { block: 16, edge: 8, ..DeltaParams::default() };
        let master = Master::new("m", &grid(1, 1, 4, 1), page(64, 64, &[], 255), p.radius);
        // 왼쪽 가장자리 그늘 (6 px)
        let d = ink_changes(&master, &page(64, 64, &[(0, 0, 6, 64)], 255), &p).unwrap();
        assert_eq!(d.changed(0.0), 0);
    }
}
