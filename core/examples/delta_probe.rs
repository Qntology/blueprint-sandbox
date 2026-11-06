//! SDS 델타 패치 실측 (개선안 2절). 마스터와 견주어 바뀐 조각만 읽을 때의 시간과 판독 결과를 전부 읽을 때와 비교합니다.
//!
//!   cargo run -p blueprint-core --release --example delta_probe -- [결과 JSON] [--no-scans] [--no-a0] [--no-full] [--no-clip]
//!       [--domain aidc] [--no-ignore] [--no-chroma] [--no-align] [--only 스캔이름] [--shift 9x6]
//!
//! `--domain aidc` 는 AI 데이터센터 원장 · 마스터(MEP · SLD)와 D3 · D3F Red-line · D1 점검표를 씁니다 (A0 는 건너뜀).
//! 개선안 1단계 전처리는 기본으로 켜져 있고 하나씩 끌 수 있습니다: `--no-ignore` 고정 안내문 자리(1-1) · `--no-chroma` 유채색 잉크(1-2) ·
//! `--no-align` 도각 평행 이동 맞춤(1-3). `--shift 9x6` 은 (cmd 는 쉼표를 인자 구분으로 읽음) 스캔(과 정답 그림)을 오른쪽 9 px · 아래 6 px 밀어 급지가 밀린 스캔을 흉내 냅니다
//! (빈 곳은 흰색, 정답은 늘 도각을 맞춰 셉니다). `--only P3_redline` 은 그 스캔만 읽습니다.
//!
//! 1. 마스터 색인: 마스터 도면 5장의 SigLIP2 특징 격자(int8)와 잉크를 만듭니다. 패치 특징은 `out/delta/master_patches.bin` 에 씁니다.
//! 2. 현장 스캔 (`out/field`): P3 · P3F Red-line 과 P1 점검표를 전부 읽기 / 델타로 읽기. 시간 · 읽은 조각 · 뽑은 값을 비교합니다.
//!    스캔 잡음이 없는 Red-line(`*_clean.png`)과 마스터의 잉크 차이를 바뀐 곳의 정답으로 삼아, 스캔에서 찾은 바뀐 칸과 견주고
//!    마스터와의 SigLIP2 패치 코사인 분포(바뀐 곳 · 그대로인 곳)를 함께 적습니다.
//! 3. A0 (`out/a0`): 다시 스캔한 A0(바뀐 곳 없음)와 PID-103-LAB 칸에 Red-line 을 그린 A0 를 원장 격자로 잘라 타일마다 잉크를 견주고,
//!    바뀐 타일만 델타로 읽습니다 (같은 타일을 전부 읽은 시간과 비교, `--no-full` 이면 전부 읽기를 건너뜀).
//!
//! 그림은 `python scripts/gen_pid_drawings.py` (마스터 · 현장 스캔 · A0) 와 `--a0-delta` (A0 두 장) 가 만듭니다.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::document::{Baseline, PageReading};
use blueprint_core::ai::runtime;
use blueprint_core::ai::siglip2::delta::{cells_of, frame_offset, ink_changes, DeltaParams, Master};
use blueprint_core::ai::siglip2::ink::{deskew, estimate_skew, InkPage};
use blueprint_core::domain::DomainMode;
use blueprint_core::extract::{extract, Extracted, PageText};
use blueprint_core::lattice::Lattice;
use blueprint_core::tiling::{tiles, Tile};
use image::RgbImage;
use serde_json::{json, Value};

const A0_ID: &str = "PID-100-ALL-A0";

fn open(path: &Path) -> Result<RgbImage> {
    let mut reader = image::ImageReader::open(path).with_context(|| format!("그림이 없습니다: {} (python scripts/gen_pid_drawings.py)", path.display()))?.with_guessed_format()?;
    reader.no_limits();
    Ok(reader.decode()?.to_rgb8())
}

fn ink_of(img: &RgbImage) -> InkPage {
    InkPage::new(&image::imageops::grayscale(img)).with_color(img)
}

/// 그림을 (dx, dy) 만큼 옮깁니다 (크기 그대로, 빈 곳은 흰색).
fn shifted(img: &RgbImage, (dx, dy): (i64, i64)) -> RgbImage {
    let (w, h) = img.dimensions();
    RgbImage::from_fn(w, h, |x, y| {
        let (sx, sy) = (x as i64 - dx, y as i64 - dy);
        if sx < 0 || sy < 0 || sx >= w as i64 || sy >= h as i64 {
            image::Rgb([255, 255, 255])
        } else {
            *img.get_pixel(sx as u32, sy as u32)
        }
    })
}

/// 읽을 때와 같게 곧게 편 잉크
fn straight_ink(img: &RgbImage) -> InkPage {
    let skew = estimate_skew(&image::imageops::grayscale(img));
    ink_of(&deskew(img, skew))
}

fn crop(img: &RgbImage, t: &Tile) -> RgbImage {
    image::imageops::crop_imm(img, t.x0, t.y0, t.width(), t.height()).to_image()
}

fn texts(r: &PageReading) -> Vec<Value> {
    r.crops.iter().map(|c| json!({ "kind": c.kind.name(), "bbox": [c.bbox.0, c.bbox.1, c.bbox.2, c.bbox.3], "text": c.output.text })).collect()
}

fn values(x: &Extracted) -> Value {
    json!({ "kind": x.kind.map(|k| k.label()), "drawing": x.drawing, "rev": x.drawing_rev, "kks": x.kks, "tag": x.tag, "reading": x.reading, "moc_no": x.moc_no, "old": x.old_value, "new": x.new_value, "worker": x.worker_id, "signed": x.signed, "kks_hits": x.kks_hits.len() })
}

/// 분포 요약 (최소 · 1 · 5 · 50 · 95 백분위 · 최대)
fn quantiles(mut v: Vec<f32>) -> Value {
    if v.is_empty() {
        return Value::Null;
    }
    v.sort_by(f32::total_cmp);
    let q = |p: f32| v[((v.len() - 1) as f32 * p).round() as usize];
    json!({ "n": v.len(), "min": v[0], "p01": q(0.01), "p05": q(0.05), "p50": q(0.5), "p95": q(0.95), "max": v[v.len() - 1] })
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let flag = |f: &str| args.iter().any(|a| a == f);
    let out_path = PathBuf::from(args.iter().find(|a| a.ends_with(".json")).cloned().unwrap_or_else(|| "out/delta_probe.json".into()));
    let params = DeltaParams { clip: !flag("--no-clip"), chroma: !flag("--no-chroma"), align: !flag("--no-align"), ..DeltaParams::default() };
    let aidc = args.windows(2).any(|w| w[0] == "--domain" && w[1] == "aidc");
    let only = args.windows(2).find(|w| w[0] == "--only").map(|w| w[1].clone());
    let shift: Option<(i64, i64)> = args.windows(2).find(|w| w[0] == "--shift").and_then(|w| {
        let (a, b) = w[1].split_once(['x', ','])?;
        Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
    });
    let db = Lattice::embedded_for(if aidc { DomainMode::AiDataCenter } else { DomainMode::PowerPlant })?;
    let scan_names: [&str; 3] = if aidc { ["D3_redline", "D3F_redline", "D1_gauge"] } else { ["P3_redline", "P3F_redline", "P1_gauge"] };
    let t = Instant::now();
    let rt = runtime::open()?;
    println!("[엔진] {} · 모델 열기 {:.1}s · {params:?}", rt.backend, t.elapsed().as_secs_f64());
    std::fs::create_dir_all("out/delta")?;

    // 1. 마스터 색인
    let masters: Vec<(String, RgbImage)> = db.drawings().iter().map(|d| Ok((d.id.clone(), open(Path::new(&format!("core/fixtures/drawings/{}.png", d.id)))?))).collect::<Result<_>>()?;
    let t = Instant::now();
    let mut set = rt.reader.index_masters(&masters, &params)?;
    if !flag("--no-ignore") {
        for d in db.drawings() {
            set.set_ignore(&d.id, &d.ignore_zones);
        }
    }
    let index_ms = t.elapsed().as_secs_f64() * 1e3;
    let raw = set.patches_to_bytes();
    std::fs::write("out/delta/master_patches.bin", &raw)?;
    println!("[마스터 색인] {}장 · {:.1}s · 패치 특징 {} KB (int8, 패치 {}개 × {}차원)", set.pages.len(), index_ms / 1e3, raw.len() / 1024, set.pages[0].patches.len(), set.pages[0].patches.dim);
    let mut report = json!({ "backend": rt.backend, "params": format!("{params:?}"), "index": { "pages": set.pages.len(), "ms": index_ms, "patch_bytes": raw.len() } });

    // 2. 현장 스캔
    if !flag("--no-scans") {
        let mut scans = Vec::new();
        for name in scan_names {
            if only.as_deref().is_some_and(|o| o != name) {
                continue;
            }
            let path = PathBuf::from(format!("out/field/{name}.png"));
            if !path.exists() {
                println!("[스캔] {} 없음", path.display());
                continue;
            }
            let img = open(&path)?;
            let img = match shift {
                Some(s) => shifted(&img, s),
                None => img,
            };
            let t = Instant::now();
            let full = rt.reader.read_pages(std::slice::from_ref(&img))?.remove(0);
            let full_ms = t.elapsed().as_secs_f64() * 1e3;
            let t = Instant::now();
            let delta = rt.reader.read_pages_delta(std::slice::from_ref(&img), &Baseline::Best(&set), &params)?.remove(0);
            let delta_ms = t.elapsed().as_secs_f64() * 1e3;
            let x_full = extract(&PageText::from_reading(name, img.dimensions(), &full), &db);
            let x_delta = extract(&PageText::from_reading(name, img.dimensions(), &delta), &db);
            let d = delta.delta.clone().expect("델타 결과");
            let same = values(&x_full) == values(&x_delta);
            let title_ok = d.master.is_none() || x_delta.drawing.is_none() || x_delta.drawing == d.master;
            println!(
                "[스캔 {name}] 전부 {:.1}s (조각 {} · 빈 조각 {}) · 델타 {:.1}s (조각 {} · 건너뜀 {} · 좁힘 {} · 안내문 자름 {}) · 마스터 {:?} 코사인 {:.3} 차이 {:.3} · 바뀐 칸 {}/{} {} · 값 {} · 표제란 {}",
                full_ms / 1e3,
                full.crops.len(),
                full.plan.blank.len(),
                delta_ms / 1e3,
                delta.crops.len(),
                d.skipped.len(),
                d.clipped,
                d.trimmed,
                d.master,
                d.similarity,
                d.margin,
                d.changed,
                d.blocks,
                d.fallback.clone().unwrap_or_default(),
                if same { "같음" } else { "다름" },
                if title_ok { "마스터와 같음" } else { "마스터와 다름 → 전부 다시 읽어야 함" }
            );
            if !same {
                println!("  전부: {}\n  델타: {}", values(&x_full), values(&x_delta));
            }
            // 정답: 스캔 잡음 없는 Red-line 과 마스터의 잉크 차이
            let clean = PathBuf::from(format!("out/field/{name}_clean.png"));
            let mut analysis = Value::Null;
            if let (Some(m), true) = (d.master.as_deref().and_then(|id| set.get(id)), clean.exists()) {
                let (clean_img, truth_params) = match shift {
                    Some(s) => (shifted(&open(&clean)?, s), DeltaParams { align: true, ..params }),
                    None => (open(&clean)?, params),
                };
                let truth = ink_changes(m, &ink_of(&clean_img), &truth_params).context("크기")?;
                let scan_ink = straight_ink(&img);
                if shift.is_some() || params.align {
                    println!("  도각으로 잰 평행 이동 {:?} (밀어 넣은 값 {:?})", frame_offset(&m.ink, &scan_ink), shift.unwrap_or((0, 0)));
                }
                let found = ink_changes(m, &scan_ink, &params).context("크기")?;
                let (t_set, f_set): (Vec<bool>, Vec<bool>) = (truth.diff.iter().map(|v| *v > params.ink_threshold).collect(), found.diff.iter().map(|v| *v > params.ink_threshold).collect());
                let hit = t_set.iter().zip(&f_set).filter(|(a, b)| **a && **b).count();
                let (n_truth, n_found) = (t_set.iter().filter(|v| **v).count(), f_set.iter().filter(|v| **v).count());
                // 정답 바뀐 칸 가운데 전부 읽은 조각이 덮는 칸을 델타 조각도 덮는가 (칸 가운데가 조각 안). 띠를 좁힐 때 글자 없는 선
                // (구름 테두리 · 화살표)은 글자 경계 밖이라 빠질 수 있어, 놓친 칸은 그림으로 따로 확인합니다.
                let inside = |b: (u32, u32, u32, u32), crops: &[(u32, u32, u32, u32)]| {
                    let (cx, cy) = ((b.0 + b.2) / 2, (b.1 + b.3) / 2);
                    crops.iter().any(|c| c.0 <= cx && cx < c.2 && c.1 <= cy && cy < c.3)
                };
                let full_boxes: Vec<_> = full.crops.iter().map(|c| c.bbox).collect();
                let delta_boxes: Vec<_> = delta.crops.iter().map(|c| c.bbox).collect();
                let needed: Vec<_> = truth.changed_boxes(params.ink_threshold).into_iter().filter(|b| inside(*b, &full_boxes)).collect();
                let missed = needed.iter().filter(|b| !inside(**b, &delta_boxes)).count();
                // SigLIP2 패치 코사인: 바뀐 곳 · 그대로인 곳
                let (rows, cols) = (m.patches.rows, m.patches.cols);
                let mut patch_truth = vec![false; rows * cols];
                for b in truth.changed_boxes(params.ink_threshold) {
                    for i in cells_of(b, (m.ink.width, m.ink.height), rows, cols) {
                        patch_truth[i] = true;
                    }
                }
                let cos_changed: Vec<f32> = d.cosines.iter().zip(&patch_truth).filter(|(_, t)| **t).map(|(c, _)| *c).collect();
                let cos_same: Vec<f32> = d.cosines.iter().zip(&patch_truth).filter(|(_, t)| !**t).map(|(c, _)| *c).collect();
                println!("  바뀐 칸 정답 {n_truth} · 찾음 {n_found} · 겹침 {hit} · 전부 읽은 조각이 덮는 정답 칸 {} 중 델타가 놓침 {missed}", needed.len());
                println!("  SigLIP2 패치 코사인 (바뀐 곳) {} · (그대로인 곳) {}", quantiles(cos_changed.clone()), quantiles(cos_same.clone()));
                analysis = json!({ "truth_blocks": n_truth, "found_blocks": n_found, "overlap": hit, "needed_blocks": needed.len(), "missed_blocks": missed, "cos_changed": quantiles(cos_changed), "cos_same": quantiles(cos_same) });
            }
            scans.push(json!({
                "scan": name,
                "full": { "ms": full_ms, "crops": full.crops.len(), "blank": full.plan.blank.len(), "values": values(&x_full), "texts": texts(&full) },
                "delta": { "ms": delta_ms, "crops": delta.crops.len(), "skipped": d.skipped.len(), "clipped": d.clipped, "trimmed": d.trimmed, "master": d.master, "similarity": d.similarity, "margin": d.margin, "changed": d.changed, "blocks": d.blocks, "fallback": d.fallback, "values": values(&x_delta), "texts": texts(&delta) },
                "same_values": same,
                "title_matches_master": title_ok,
                "analysis": analysis,
            }));
        }
        report["scans"] = Value::Array(scans);
    }

    // 3. A0: 타일마다 잉크를 견주어 바뀐 타일만 델타로
    if !flag("--no-a0") && !aidc {
        let a0 = db.a0(A0_ID).context("원장에 A0 배치가 없습니다")?.clone();
        let base = open(Path::new(&format!("out/a0/{A0_ID}.png")))?;
        let grid = tiles(base.dimensions(), a0.grid.rows, a0.grid.cols, a0.grid.overlap);
        let master_tiles: Vec<(String, RgbImage)> = grid.iter().map(|t| (format!("{A0_ID}/{}", t.id), crop(&base, t))).collect();
        let t = Instant::now();
        let tile_set = rt.reader.index_masters(&master_tiles, &params)?;
        let tile_index_ms = t.elapsed().as_secs_f64() * 1e3;
        println!("[A0 타일 색인] {}장 · {:.1}s", tile_set.pages.len(), tile_index_ms / 1e3);
        let mut variants = Vec::new();
        for variant in ["rescan", "redline"] {
            let path = PathBuf::from(format!("out/a0/{A0_ID}-{variant}.jpg"));
            if !path.exists() {
                println!("[A0 {variant}] {} 없음 (python scripts/gen_pid_drawings.py --a0-delta)", path.display());
                continue;
            }
            let img = open(&path)?;
            let scanned: Vec<RgbImage> = grid.iter().map(|t| crop(&img, t)).collect();
            let mut per_tile = Vec::new();
            let mut changed_tiles = Vec::new();
            let t_all = Instant::now();
            for (i, (t, s)) in grid.iter().zip(&scanned).enumerate() {
                let t0 = Instant::now();
                let d = ink_changes(&tile_set.pages[i], &ink_of(s), &params).context("타일 크기")?;
                let ms = t0.elapsed().as_secs_f64() * 1e3;
                let n = d.changed(params.ink_threshold);
                if n > 0 {
                    changed_tiles.push(i);
                }
                per_tile.push(json!({ "tile": t.id, "ms": ms, "max": d.max(), "changed_blocks": n }));
            }
            let check_ms = t_all.elapsed().as_secs_f64() * 1e3;
            let ids: Vec<&str> = changed_tiles.iter().map(|&i| grid[i].id.as_str()).collect();
            println!("[A0 {variant}] 타일 잉크 대조 {:.2}s ({:.0} ms/타일) · 바뀐 타일 {ids:?}", check_ms / 1e3, check_ms / grid.len() as f64);
            let mut entry = json!({ "variant": variant, "check_ms": check_ms, "tiles": per_tile, "changed_tiles": ids });
            if !changed_tiles.is_empty() {
                let pages: Vec<RgbImage> = changed_tiles.iter().map(|&i| scanned[i].clone()).collect();
                let baselines: Vec<Option<&Master>> = changed_tiles.iter().map(|&i| Some(&tile_set.pages[i])).collect();
                let t = Instant::now();
                let delta = rt.reader.read_pages_delta(&pages, &Baseline::Each(&baselines), &params)?;
                let delta_ms = t.elapsed().as_secs_f64() * 1e3;
                let (full, full_ms) = if flag("--no-full") {
                    (Vec::new(), 0.0)
                } else {
                    let t = Instant::now();
                    let f = rt.reader.read_pages(&pages)?;
                    (f, t.elapsed().as_secs_f64() * 1e3)
                };
                let mut tiles_out = Vec::new();
                for (k, (&i, d)) in changed_tiles.iter().zip(&delta).enumerate() {
                    let xd = extract(&PageText::from_reading("a0", pages[k].dimensions(), d), &db);
                    let dd = d.delta.clone().expect("델타");
                    let f = full.get(k);
                    let xf = f.map(|f| extract(&PageText::from_reading("a0", pages[k].dimensions(), f), &db));
                    println!(
                        "  타일 {} · 전부 조각 {} → 델타 조각 {} (건너뜀 {} · 좁힘 {} · 바뀐 칸 {}/{} {}) · 값(델타) {}",
                        grid[i].id,
                        f.map(|f| f.crops.len().to_string()).unwrap_or("-".into()),
                        d.crops.len(),
                        dd.skipped.len(),
                        dd.clipped,
                        dd.changed,
                        dd.blocks,
                        dd.fallback.clone().unwrap_or_default(),
                        values(&xd)
                    );
                    tiles_out.push(json!({ "tile": grid[i].id, "full": f.map(|f| json!({ "crops": f.crops.len(), "values": xf.as_ref().map(values), "texts": texts(f) })), "delta": { "crops": d.crops.len(), "skipped": dd.skipped.len(), "clipped": dd.clipped, "changed": dd.changed, "blocks": dd.blocks, "similarity": dd.similarity, "fallback": dd.fallback, "values": values(&xd), "texts": texts(d) } }));
                }
                println!("  바뀐 타일 {}장: 전부 읽기 {:.1}s · 델타 {:.1}s", pages.len(), full_ms / 1e3, delta_ms / 1e3);
                entry["read"] = json!({ "full_ms": full_ms, "delta_ms": delta_ms, "tiles": tiles_out });
            }
            variants.push(entry);
        }
        report["a0"] = json!({ "tile_index_ms": tile_index_ms, "variants": variants });
    }

    if let Some(dir) = out_path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(&out_path, serde_json::to_string_pretty(&report)?)?;
    println!("[결과] {}", out_path.display());
    // ROCm 을 쓴 Windows 프로세스는 보통 종료하면 rocblas 정리에서 멈춰 GPU 메모리를 쥔 채 남습니다 (accel::finish_process).
    blueprint_core::ai::accel::finish_process(&rt.device, 0);
    Ok(())
}
