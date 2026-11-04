//! A0 대형 도면을 실제 모델로 읽어 봅니다 (기획 Part 8-4 1절).
//!
//!   cargo run -p blueprint-core --release --example a0_probe -- [A0 PNG] [결과 JSON] [겹침 비율 …] [--no-whole] [--no-tiles]
//!
//! 1. 통째로: A0 한 장을 그대로 `read_pages` 에 넣습니다 (글자가 페이지 높이의 0.4 % 라 엔진 가정 밖). `--no-whole` 이면 건너뜀
//! 2. 타일: 원장 격자(4 × 4)로 잘라 한 번에 넣고, 찾은 KKS 상자를 A0 좌표로 되돌려 겹친 띠의 중복을 합칩니다.
//!    세 번째 인자부터 겹침 비율을 더 주면(예: 0.2) 같은 격자를 그 겹침으로도 읽습니다. `--no-tiles` 면 건너뜀
//! 3. 빠른 확인 경로: 배치를 원장이 아는 A0 는 원장 설비 상자만 잘라 다시 읽고 원장 KKS 와 대조합니다.
//!    못 읽은 상자만 그 상자를 덮는 타일로 다시 읽습니다 (대체 경로). 300 DPI 처럼 원장보다 큰 그림은
//!    상자를 원장 해상도로 줄여 읽는 것도 함께 잽니다 (gocr 이미지 토큰이 넓이에 비례).
//! 4. 부분 재확인: 103동 급수조절밸브 상자 하나만 다시 읽기 (빠른 경로)와, 그 상자가 든 타일을 다시 읽기 (이전 방식)
//!
//! 그림 크기가 원장(`pixel_size`)의 몇 배인지 보고 상자 좌표 · 테두리 여백을 맞춥니다 (150 DPI = 1 배, 300 DPI = 2 배).
//! A0 그림은 `python scripts/gen_pid_drawings.py` 가 `out/a0/PID-100-ALL-A0.png` 에, `--a0-dpi 300` 이면 `@300dpi.png` 로 만듭니다.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::runtime;
use blueprint_core::extract::{extract, PageText};
use blueprint_core::lattice::Lattice;
use blueprint_core::tiling::{a0_bbox, check_box, covering, iou, ledger_boxes, merge, tiles, BoxCheck, GlobalHit, LedgerBox, Tile};
use image::RgbImage;
use serde_json::{json, Value};

const A0_ID: &str = "PID-100-ALL-A0";

/// 300 DPI A0(1.4억 화소)는 `image` 의 기본 디코딩 한도(512 MiB)를 넘으므로 한도 없이 엽니다.
fn open_large(path: &PathBuf) -> Result<RgbImage> {
    let mut reader = image::ImageReader::open(path).with_context(|| format!("A0 그림이 없습니다: {} (python scripts/gen_pid_drawings.py)", path.display()))?.with_guessed_format()?;
    reader.no_limits();
    Ok(reader.decode()?.to_rgb8())
}

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

/// 원장 상자만 다시 읽어 원장 KKS 와 대조합니다 (빠른 확인 경로): (대조 결과, 시간 ms)
fn boxes(rt: &runtime::Runtime, db: &Lattice, img: &RgbImage, list: &[LedgerBox], inset: u32, region_scale: f32) -> Result<(Vec<BoxCheck>, f64)> {
    let regions: Vec<(u32, u32, u32, u32)> = list.iter().map(|b| (b.px[0], b.px[1], b.px[2], b.px[3])).collect();
    let t = Instant::now();
    let out = rt.reader.read_regions(img, &regions, inset, region_scale)?;
    let ms = t.elapsed().as_secs_f64() * 1e3;
    Ok((list.iter().zip(&out).map(|(b, o)| check_box(db, &b.kks, &o.text)).collect(), ms))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let no_whole = args.iter().any(|a| a == "--no-whole");
    let no_tiles = args.iter().any(|a| a == "--no-tiles");
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with("--")).collect();
    let path = PathBuf::from(positional.first().map(|s| s.to_string()).unwrap_or_else(|| format!("out/a0/{A0_ID}.png")));
    let out_path = PathBuf::from(positional.get(1).map(|s| s.to_string()).unwrap_or_else(|| "out/a0_probe.json".into()));
    let extra_overlaps: Vec<f32> = positional.iter().skip(2).filter_map(|a| a.parse().ok()).collect();
    let db = Lattice::embedded()?;
    let a0 = db.a0(A0_ID).context("원장에 A0 배치가 없습니다")?.clone();
    let img = open_large(&path)?;
    let page = img.dimensions();
    // 원장 배치(150 DPI) 대비 배율: 300 DPI 그림이면 2
    let scale = page.0 as f32 / a0.pixel_size[0] as f32;
    let inset = (5.0 * scale).round() as u32;
    let t = Instant::now();
    let rt = runtime::open()?;
    println!("[엔진] {} · 모델 열기 {:.1}s · A0 {} × {} px (원장의 {scale:.1} 배)", rt.backend, t.elapsed().as_secs_f64(), page.0, page.1);
    let mut runs = Vec::new();

    // 1. 통째로
    if !no_whole {
        let (whole, whole_ms) = read_pages(&rt, &db, std::slice::from_ref(&img))?;
        let (found, crops, dropped) = &whole[0];
        let whole_hits = merge(found.iter().map(|(c, b, k)| GlobalHit { code: c.clone(), bbox: *b, tile: "전체".into(), corrected: *k }).collect());
        let (w_found, w_extra, w_iou, w_missing) = score(&db, &whole_hits);
        println!("[통째] KKS {w_found}/{} · 원장 밖 {w_extra} · IoU {w_iou:.2} · 조각 {crops} · 버림 {dropped} · {:.1}s", db.equipment_all().len(), whole_ms / 1e3);
        runs.push(json!({ "mode": "whole", "found": w_found, "expected": db.equipment_all().len(), "extra": w_extra, "mean_iou": w_iou, "missing": w_missing, "ms": whole_ms, "crops": crops, "dropped": dropped }));
    }

    // 2. 타일 (원장 겹침 + 더 준 겹침)
    if !no_tiles {
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
    }

    // 3. 빠른 확인 경로: 원장 상자만 (원장보다 큰 그림은 원장 해상도로 줄여 읽기도)
    let all = ledger_boxes(&db, &a0, page, None);
    let mut region_scales = vec![1.0f32];
    if scale > 1.01 {
        region_scales.push(1.0 / scale);
    }
    let grid = tiles(page, a0.grid.rows, a0.grid.cols, a0.grid.overlap);
    for rs in &region_scales {
        let (checks, ms) = boxes(&rt, &db, &img, &all, inset, *rs)?;
        let ok = checks.iter().filter(|c| c.ok).count();
        let corrected = checks.iter().filter(|c| c.ok && c.corrected).count();
        let other = checks.iter().filter(|c| !c.ok && c.read.is_some()).count();
        let unread = checks.iter().filter(|c| c.read.is_none()).count();
        // 대체 경로: 원장 KKS 를 못 읽은 상자만, 그 상자를 덮는 타일로 다시 읽습니다.
        let failed: Vec<&LedgerBox> = all.iter().zip(&checks).filter(|(_, c)| !c.ok).map(|(b, _)| b).collect();
        let (recovered, fallback_tiles, fallback_ms) = if failed.is_empty() {
            (0, Vec::new(), 0.0)
        } else {
            let mut ids = BTreeSet::new();
            let mut need: Vec<Tile> = Vec::new();
            for b in &failed {
                for t in covering(&grid, b.bbox, page) {
                    if ids.insert(t.id.clone()) {
                        need.push(t.clone());
                    }
                }
            }
            let (hits, ms, _) = tiled(&rt, &db, &img, &need)?;
            (failed.iter().filter(|b| hits.iter().any(|h| h.code == b.kks)).count(), ids.into_iter().collect::<Vec<_>>(), ms)
        };
        println!(
            "[원장 상자 · 배율 {rs:.2}] KKS 맞음 {ok}/{} (교정 {corrected}) · 다른 코드 {other} · 못 읽음 {unread} · {:.1}s ({:.2}s/상자) · 대체 타일 {fallback_tiles:?} → 되찾음 {recovered} ({:.1}s)",
            all.len(),
            ms / 1e3,
            ms / 1e3 / all.len().max(1) as f64,
            fallback_ms / 1e3
        );
        runs.push(json!({ "mode": "boxes", "region_scale": rs, "found": ok, "corrected": corrected, "other_code": other, "unread": unread, "expected": all.len(), "ms": ms, "fallback_tiles": fallback_tiles, "recovered": recovered, "fallback_ms": fallback_ms, "checks": checks }));
    }

    // 4. 부분 재확인: 급수조절밸브 상자 하나 (빠른 경로) · 그 상자가 든 타일 (이전 방식)
    let valve_box = ledger_boxes(&db, &a0, page, Some(&["10LAB20AA102"]));
    for rs in &region_scales {
        let (checks, ms) = boxes(&rt, &db, &img, &valve_box, inset, *rs)?;
        let ok = checks.first().is_some_and(|c| c.ok);
        println!("[부분 재확인 · 상자 하나 · 배율 {rs:.2}] 밸브 {} · {:.1}s", if ok { "맞음" } else { "못 읽음" }, ms / 1e3);
        runs.push(json!({ "mode": "box_partial", "region_scale": rs, "found_valve": ok, "ms": ms }));
    }
    let valve = a0_bbox(&db, &a0, "10LAB20AA102").context("밸브 좌표")?;
    let part: Vec<Tile> = covering(&grid, valve, page).into_iter().cloned().collect();
    let (hits, ms, _) = tiled(&rt, &db, &img, &part)?;
    let ok = hits.iter().any(|h| h.code == "10LAB20AA102" && iou(h.bbox, valve) > 0.0);
    println!("[부분 재스캔 · 타일] 타일 {:?} · 밸브 {} · {:.1}s", part.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), if ok { "찾음" } else { "못 찾음" }, ms / 1e3);
    runs.push(json!({ "mode": "partial", "tiles": part.iter().map(|t| t.id.clone()).collect::<Vec<_>>(), "found_valve": ok, "ms": ms }));

    let doc = json!({ "at": chrono::Utc::now().to_rfc3339(), "backend": rt.backend.to_string(), "a0": A0_ID, "page": [page.0, page.1], "scale": scale, "runs": runs });
    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&doc)?)?;
    println!("[결과] {}", out_path.display());
    // ROCm 을 쓴 Windows 프로세스는 보통 종료하면 rocblas 정리에서 멈춰 GPU 메모리를 쥔 채 남습니다 (accel::finish_process).
    blueprint_core::ai::accel::finish_process(&rt.device, 0);
    Ok(())
}
