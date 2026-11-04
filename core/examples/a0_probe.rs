//! A0 대형 도면을 실제 모델로 읽어 봅니다 (기획 Part 8-4 1절).
//!
//!   cargo run -p blueprint-core --release --example a0_probe -- [A0 PNG] [결과 JSON] [겹침 비율 …]
//!
//! 1. 통째로: 7016 × 4961 px 한 장을 그대로 `read_pages` 에 넣습니다 (글자가 페이지 높이의 0.4 % 라 엔진 가정 밖).
//! 2. 타일: 원장 격자(4 × 4)로 잘라 한 번에 넣고, 찾은 KKS 상자를 A0 좌표로 되돌려 겹친 띠의 중복을 합칩니다.
//!    세 번째 인자부터 겹침 비율을 더 주면(예: 0.2) 같은 격자를 그 겹침으로도 읽습니다.
//! 3. 부분 재스캔: 103동 급수조절밸브가 걸친 타일만 다시 읽습니다.
//!
//! A0 그림은 `python scripts/gen_pid_drawings.py` 가 `out/a0/PID-100-ALL-A0.png` 에 만듭니다.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::runtime;
use blueprint_core::extract::{extract, PageText};
use blueprint_core::lattice::Lattice;
use blueprint_core::tiling::{a0_bbox, covering, iou, merge, tiles, GlobalHit, Tile};
use image::RgbImage;
use serde_json::{json, Value};

const A0_ID: &str = "PID-100-ALL-A0";

/// 페이지들을 한 번에 읽고, 페이지마다 (KKS 상자 목록, 조각 수, 버린 수)
fn read_pages(rt: &runtime::Runtime, db: &Lattice, pages: &[RgbImage]) -> Result<(Vec<(Vec<(String, [f32; 4], bool)>, usize, usize)>, f64)> {
    let t = Instant::now();
    let readings = rt.reader.read_pages(pages)?;
    let ms = t.elapsed().as_secs_f64() * 1e3;
    let out = readings
        .iter()
        .zip(pages)
        .map(|(r, img)| {
            let x = extract(&PageText::from_reading("a0", img.dimensions(), r), db);
            (x.kks_hits.iter().map(|h| (h.code.clone(), h.bbox, h.corrected)).collect(), r.crops.len(), r.plan.dropped.len())
        })
        .collect();
    Ok((out, ms))
}

/// 찾은 상자를 원장 좌표와 맞춥니다: (찾은 설비 수, 원장에 없는 코드 수, 평균 IoU, 못 찾은 설비)
fn score(db: &Lattice, hits: &[GlobalHit]) -> (usize, usize, f32, Vec<String>) {
    let a0 = db.a0(A0_ID).expect("원장에 A0 배치가 있습니다");
    let expected: Vec<(String, [f32; 4])> = db.equipment_all().iter().filter_map(|e| Some((e.kks.clone(), a0_bbox(db, a0, &e.kks)?))).collect();
    let found: BTreeSet<&str> = hits.iter().map(|h| h.code.as_str()).collect();
    let ious: Vec<f32> = expected.iter().filter_map(|(k, b)| hits.iter().filter(|h| &h.code == k).map(|h| iou(h.bbox, *b)).reduce(f32::max)).collect();
    let missing: Vec<String> = expected.iter().filter(|(k, _)| !found.contains(k.as_str())).map(|(k, _)| k.clone()).collect();
    let extra = found.iter().filter(|k| db.equipment(k).is_none()).count();
    let mean = if ious.is_empty() { 0.0 } else { ious.iter().sum::<f32>() / ious.len() as f32 };
    (expected.len() - missing.len(), extra, mean, missing)
}

fn crop(img: &RgbImage, t: &Tile) -> RgbImage {
    image::imageops::crop_imm(img, t.x0, t.y0, t.width(), t.height()).to_image()
}

fn tiled(rt: &runtime::Runtime, db: &Lattice, img: &RgbImage, grid: &[Tile]) -> Result<(Vec<GlobalHit>, f64, Vec<Value>)> {
    let pages: Vec<RgbImage> = grid.iter().map(|t| crop(img, t)).collect();
    let (per_page, ms) = read_pages(rt, db, &pages)?;
    let mut hits = Vec::new();
    let mut detail = Vec::new();
    for (t, (found, crops, dropped)) in grid.iter().zip(per_page) {
        detail.push(json!({ "tile": t.id, "crops": crops, "dropped": dropped, "kks": found.len() }));
        for (code, local, corrected) in found {
            hits.push(GlobalHit { code, bbox: t.to_global(local, img.dimensions()), tile: t.id.clone(), corrected });
        }
    }
    Ok((merge(hits), ms, detail))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let path = PathBuf::from(args.first().cloned().unwrap_or_else(|| format!("out/a0/{A0_ID}.png")));
    let out_path = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "out/a0_probe.json".into()));
    let extra_overlaps: Vec<f32> = args.iter().skip(2).filter_map(|a| a.parse().ok()).collect();
    let db = Lattice::embedded()?;
    let a0 = db.a0(A0_ID).context("원장에 A0 배치가 없습니다")?.clone();
    let img = image::open(&path).with_context(|| format!("A0 그림이 없습니다: {} (python scripts/gen_pid_drawings.py)", path.display()))?.to_rgb8();
    let page = img.dimensions();
    let t = Instant::now();
    let rt = runtime::open()?;
    println!("[엔진] {} · 모델 열기 {:.1}s · A0 {} × {} px", rt.backend, t.elapsed().as_secs_f64(), page.0, page.1);

    // 1. 통째로
    let (whole, whole_ms) = read_pages(&rt, &db, std::slice::from_ref(&img))?;
    let (found, crops, dropped) = &whole[0];
    let whole_hits = merge(found.iter().map(|(c, b, k)| GlobalHit { code: c.clone(), bbox: *b, tile: "전체".into(), corrected: *k }).collect());
    let (w_found, w_extra, w_iou, w_missing) = score(&db, &whole_hits);
    println!("[통째] KKS {w_found}/{} · 원장 밖 {w_extra} · IoU {w_iou:.2} · 조각 {crops} · 버림 {dropped} · {:.1}s", db.equipment_all().len(), whole_ms / 1e3);
    let mut runs = vec![json!({ "mode": "whole", "found": w_found, "expected": db.equipment_all().len(), "extra": w_extra, "mean_iou": w_iou, "missing": w_missing, "ms": whole_ms, "crops": crops, "dropped": dropped })];

    // 2. 타일 (원장 겹침 + 더 준 겹침)
    let mut overlaps = vec![a0.grid.overlap];
    overlaps.extend(extra_overlaps);
    for overlap in overlaps {
        let grid = tiles(page, a0.grid.rows, a0.grid.cols, overlap);
        let (hits, ms, detail) = tiled(&rt, &db, &img, &grid)?;
        let (found, extra, mean_iou, missing) = score(&db, &hits);
        println!(
            "[타일 {}x{} 겹침 {:.0}%] KKS {found}/{} · 원장 밖 {extra} · IoU {mean_iou:.2} · {:.1}s ({:.1}s/타일) · 못 찾음 {:?}",
            a0.grid.rows,
            a0.grid.cols,
            overlap * 100.0,
            db.equipment_all().len(),
            ms / 1e3,
            ms / 1e3 / grid.len() as f64,
            missing
        );
        runs.push(json!({ "mode": "tiled", "overlap": overlap, "tile_px": [grid[0].width(), grid[0].height()], "found": found, "expected": db.equipment_all().len(), "extra": extra, "mean_iou": mean_iou, "missing": missing, "ms": ms, "tiles": detail }));
    }

    // 3. 부분 재스캔: 급수조절밸브가 걸친 타일만
    let grid = tiles(page, a0.grid.rows, a0.grid.cols, a0.grid.overlap);
    let valve = a0_bbox(&db, &a0, "10LAB20AA102").context("밸브 좌표")?;
    let part: Vec<Tile> = covering(&grid, valve, page).into_iter().cloned().collect();
    let (hits, ms, _) = tiled(&rt, &db, &img, &part)?;
    let ok = hits.iter().any(|h| h.code == "10LAB20AA102" && iou(h.bbox, valve) > 0.0);
    println!("[부분 재스캔] 타일 {:?} · 밸브 {} · {:.1}s", part.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), if ok { "찾음" } else { "못 찾음" }, ms / 1e3);
    runs.push(json!({ "mode": "partial", "tiles": part.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), "found_valve": ok, "ms": ms }));

    let doc = json!({ "at": chrono::Utc::now().to_rfc3339(), "backend": rt.backend.to_string(), "a0": A0_ID, "page": [page.0, page.1], "runs": runs });
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)?)?;
    println!("[결과] {}", out_path.display());
    // ROCm 을 쓴 Windows 프로세스는 보통 종료하면 rocblas 정리에서 멈춰 GPU 메모리를 쥔 채 남습니다 (accel::finish_process).
    blueprint_core::ai::accel::finish_process(&rt.device, 0);
    Ok(())
}
