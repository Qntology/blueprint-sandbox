//! A0 대형 도면 타일링 (기획 Part 8-4 1절).
//!
//! 문서 엔진(`ink.rs` 의 `TEXT_UNIT = 0.012`, 크롭 한 변 2048 px)은 A4 쪽 글자 크기를 가정합니다. A0 도면(1189 × 841 mm)을
//! 한 장으로 넣으면 KKS 글자가 페이지 높이의 0.3 ~ 0.4 % 라서 부스러기로 버려지거나 여러 상자가 한 덩어리가 됩니다.
//! 엔진 상수는 그대로 두고, 읽기 전에 A0 를 `rows × cols` 격자 타일(이웃과 `overlap` 만큼 겹침)로 잘라 `read_pages` 에 묶어 넘깁니다.
//! 타일 하나가 A3 ~ A4 비율이 되어 글자 크기 가정이 맞고, 타일에서 찾은 상자는 타일 오프셋을 더해 A0 전체 좌표로 되돌립니다.
//! 겹친 띠에서 두 번 찾은 같은 KKS 는 상자 겹침(IoU ≥ 0.5)이나 포함으로 하나로 합칩니다(NMS).

use serde::Serialize;

use crate::lattice::{A0Row, Lattice};

/// 타일 하나 (A0 픽셀 좌표). `id` 는 행 글자 + 열 번호입니다 (`B3` = 2행 3열).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Tile {
    pub id: String,
    pub row: u32,
    pub col: u32,
    pub x0: u32,
    pub y0: u32,
    pub x1: u32,
    pub y1: u32,
}

impl Tile {
    pub fn width(&self) -> u32 {
        self.x1 - self.x0
    }

    pub fn height(&self) -> u32 {
        self.y1 - self.y0
    }

    /// 타일 안의 정규화 상자를 A0 전체의 정규화 상자로 바꿉니다.
    /// `X = (offset_x + x_local) / W_A0`, `Y = (offset_y + y_local) / H_A0`
    pub fn to_global(&self, local: [f32; 4], page: (u32, u32)) -> [f32; 4] {
        let (w, h) = (self.width() as f32, self.height() as f32);
        let (pw, ph) = (page.0 as f32, page.1 as f32);
        [
            (self.x0 as f32 + local[0] * w) / pw,
            (self.y0 as f32 + local[1] * h) / ph,
            (self.x0 as f32 + local[2] * w) / pw,
            (self.y0 as f32 + local[3] * h) / ph,
        ]
    }

    /// A0 정규화 상자가 이 타일 안에 (전부) 들어오는가
    pub fn contains(&self, global: [f32; 4], page: (u32, u32)) -> bool {
        let (pw, ph) = (page.0 as f32, page.1 as f32);
        global[0] * pw >= self.x0 as f32 && global[2] * pw <= self.x1 as f32 && global[1] * ph >= self.y0 as f32 && global[3] * ph <= self.y1 as f32
    }

    /// A0 정규화 상자와 겹치는가
    pub fn touches(&self, global: [f32; 4], page: (u32, u32)) -> bool {
        let (pw, ph) = (page.0 as f32, page.1 as f32);
        global[0] * pw < self.x1 as f32 && global[2] * pw > self.x0 as f32 && global[1] * ph < self.y1 as f32 && global[3] * ph > self.y0 as f32
    }
}

/// 격자 타일. 타일 크기는 겹침을 넣고 페이지를 정확히 덮도록 `W / (cols·(1 − o) + o)` 입니다.
pub fn tiles(page: (u32, u32), rows: u32, cols: u32, overlap: f32) -> Vec<Tile> {
    let (w, h) = (page.0 as f32, page.1 as f32);
    let tw = w / (cols as f32 * (1.0 - overlap) + overlap);
    let th = h / (rows as f32 * (1.0 - overlap) + overlap);
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let x0 = (c as f32 * tw * (1.0 - overlap)).round() as u32;
            let y0 = (r as f32 * th * (1.0 - overlap)).round() as u32;
            let x1 = if c + 1 == cols { page.0 } else { ((x0 as f32) + tw).round().min(w) as u32 };
            let y1 = if r + 1 == rows { page.1 } else { ((y0 as f32) + th).round().min(h) as u32 };
            let id = format!("{}{}", (b'A' + r as u8) as char, c + 1);
            out.push(Tile { id, row: r, col: c, x0, y0, x1, y1 });
        }
    }
    out
}

/// A0 도면 행의 격자 타일
pub fn a0_tiles(a0: &A0Row) -> Vec<Tile> {
    tiles((a0.pixel_size[0], a0.pixel_size[1]), a0.grid.rows, a0.grid.cols, a0.grid.overlap)
}

/// 설비 상자의 A0 전체 정규화 좌표 (마스터 도면 배치 + 도면 안 좌표)
pub fn a0_bbox(db: &Lattice, a0: &A0Row, kks: &str) -> Option<[f32; 4]> {
    let e = db.equipment(kks)?;
    let sh = a0.sheets.iter().find(|s| s.drawing == e.drawing)?;
    let (sw, sh_h) = (a0.sheet_px[0] as f32, a0.sheet_px[1] as f32);
    let (pw, ph) = (a0.pixel_size[0] as f32, a0.pixel_size[1] as f32);
    let b = e.bbox;
    Some([(sh.x as f32 + b[0] * sw) / pw, (sh.y as f32 + b[1] * sh_h) / ph, (sh.x as f32 + b[2] * sw) / pw, (sh.y as f32 + b[3] * sh_h) / ph])
}

/// 설비 상자의 가운데가 든 타일 (공간 뎁스 Lv4 의 타일 이름, 미니맵 강조)
pub fn tile_of<'a>(tiles: &'a [Tile], global: [f32; 4], page: (u32, u32)) -> Option<&'a Tile> {
    let cx = (global[0] + global[2]) / 2.0 * page.0 as f32;
    let cy = (global[1] + global[3]) / 2.0 * page.1 as f32;
    // 겹친 띠에서는 가운데에 더 가까운 타일을 고릅니다.
    tiles
        .iter()
        .filter(|t| cx >= t.x0 as f32 && cx < t.x1 as f32 && cy >= t.y0 as f32 && cy < t.y1 as f32)
        .min_by(|a, b| {
            let d = |t: &Tile| ((t.x0 + t.x1) as f32 / 2.0 - cx).powi(2) + ((t.y0 + t.y1) as f32 / 2.0 - cy).powi(2);
            d(a).total_cmp(&d(b))
        })
}

/// 부분 재스캔: 상자에 걸친 타일만 고릅니다 (알람 · Red-line 이 난 자리만 다시 읽기)
pub fn covering<'a>(tiles: &'a [Tile], global: [f32; 4], page: (u32, u32)) -> Vec<&'a Tile> {
    // 상자를 통째로 담는 타일이 있으면 그 하나로 충분합니다.
    if let Some(t) = tiles.iter().find(|t| t.contains(global, page)) {
        return vec![t];
    }
    tiles.iter().filter(|t| t.touches(global, page)).collect()
}

/// 타일에서 찾은 코드 하나 (A0 전체 좌표)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct GlobalHit {
    pub code: String,
    pub bbox: [f32; 4],
    pub tile: String,
    pub corrected: bool,
}

pub fn iou(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let inter = ix * iy;
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let union = area(a) + area(b) - inter;
    if union <= 0.0 {
        0.0
    } else {
        inter / union
    }
}

/// 작은 상자가 큰 상자 안에 든 비율
fn contained(a: [f32; 4], b: [f32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) - a[0].max(b[0])).max(0.0);
    let iy = (a[3].min(b[3]) - a[1].max(b[1])).max(0.0);
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    let small = area(a).min(area(b));
    if small <= 0.0 {
        0.0
    } else {
        ix * iy / small
    }
}

/// 겹친 띠에서 두 번 찾은 같은 코드를 하나로 합칩니다 (NMS). 같은 코드이고 IoU ≥ 0.5 이거나 작은 상자의 80 % 이상이
/// 큰 상자 안이면 같은 것으로 보고 더 큰 상자를 남깁니다. 같은 코드라도 멀리 떨어져 있으면 둘 다 남깁니다 (같은 설비가 두 곳에 그려진 도면).
pub fn merge(mut hits: Vec<GlobalHit>) -> Vec<GlobalHit> {
    let area = |r: [f32; 4]| (r[2] - r[0]).max(0.0) * (r[3] - r[1]).max(0.0);
    hits.sort_by(|a, b| area(b.bbox).total_cmp(&area(a.bbox)).then(a.code.cmp(&b.code)));
    let mut kept: Vec<GlobalHit> = Vec::new();
    for h in hits {
        let dup = kept.iter().any(|k| k.code == h.code && (iou(k.bbox, h.bbox) >= 0.5 || contained(k.bbox, h.bbox) >= 0.8));
        if !dup {
            kept.push(h);
        }
    }
    kept.sort_by(|a, b| a.bbox[1].total_cmp(&b.bbox[1]).then(a.bbox[0].total_cmp(&b.bbox[0])));
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_four_by_four_grid_covers_the_a0_page_with_overlap() {
        let page = (7016, 4961);
        let ts = tiles(page, 4, 4, 0.10);
        assert_eq!(ts.len(), 16);
        assert_eq!(ts[0].id, "A1");
        assert_eq!(ts[6].id, "B3");
        assert_eq!(ts[15].id, "D4");
        assert_eq!((ts[15].x1, ts[15].y1), page, "마지막 타일이 페이지 끝까지 덮습니다");
        // 타일은 A 계열 비율(√2)에 가깝고, 이웃 타일과 타일 폭의 10 % 만큼 겹칩니다.
        let t = &ts[0];
        assert!((t.width() as f32 / t.height() as f32 - 1.414).abs() < 0.01, "{t:?}");
        let overlap = ts[0].x1 - ts[1].x0;
        assert!((overlap as f32 / t.width() as f32 - 0.10).abs() < 0.01, "{overlap}");
        // 페이지 높이 대비 글자 비율: 21 px 글자가 A0 전체(4961)에서는 0.42 %, 타일(≈1341)에서는 1.6 %
        assert!(21.0 / t.height() as f32 > 0.012);
        assert!(21.0 / (page.1 as f32) < 0.012 / 2.0);
    }

    #[test]
    fn local_boxes_map_back_to_global_coordinates() {
        let page = (7016, 4961);
        let ts = tiles(page, 4, 4, 0.10);
        let b3 = ts.iter().find(|t| t.id == "B3").unwrap();
        let g = b3.to_global([0.0, 0.0, 1.0, 1.0], page);
        assert!((g[0] - b3.x0 as f32 / 7016.0).abs() < 1e-6 && (g[3] - b3.y1 as f32 / 4961.0).abs() < 1e-6);
        let mid = b3.to_global([0.5, 0.5, 0.5, 0.5], page);
        let back = tile_of(&ts, mid, page).unwrap();
        assert_eq!(back.id, "B3");
    }

    #[test]
    fn duplicates_from_the_overlap_band_merge_into_one() {
        let a = GlobalHit { code: "10LAB20AA102".into(), bbox: [0.70, 0.30, 0.75, 0.33], tile: "B3".into(), corrected: false };
        let b = GlobalHit { code: "10LAB20AA102".into(), bbox: [0.705, 0.301, 0.75, 0.33], tile: "B4".into(), corrected: false };
        let far = GlobalHit { code: "10LAB20AA102".into(), bbox: [0.10, 0.80, 0.15, 0.83], tile: "D1".into(), corrected: false };
        let other = GlobalHit { code: "10LAB10AP001".into(), bbox: [0.70, 0.30, 0.75, 0.33], tile: "B3".into(), corrected: false };
        let m = merge(vec![b, a.clone(), far, other]);
        assert_eq!(m.len(), 3, "겹친 띠의 같은 코드만 합칩니다: {m:?}");
        assert!(m.iter().any(|h| h.code == "10LAB20AA102" && h.bbox == a.bbox), "큰 상자를 남깁니다");
    }

    #[test]
    fn the_seed_a0_sheet_places_every_equipment_box_and_a_partial_rescan_reads_one_tile() {
        let db = Lattice::embedded().unwrap();
        let a0 = db.a0("PID-100-ALL-A0").unwrap();
        let page = (a0.pixel_size[0], a0.pixel_size[1]);
        let ts = a0_tiles(a0);
        for e in db.equipment_all() {
            let g = a0_bbox(&db, a0, &e.kks).unwrap();
            assert!(g.iter().all(|v| (0.0..=1.0).contains(v)), "{} {g:?}", e.kks);
            assert!(tile_of(&ts, g, page).is_some(), "{}", e.kks);
            assert!(!covering(&ts, g, page).is_empty());
        }
        // 103동 급수조절밸브는 타일 하나(또는 경계에 걸리면 둘)만 다시 읽으면 됩니다.
        let valve = a0_bbox(&db, a0, "10LAB20AA102").unwrap();
        assert!(covering(&ts, valve, page).len() <= 2);
    }
}
