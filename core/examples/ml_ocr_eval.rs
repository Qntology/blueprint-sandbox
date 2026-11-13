//! 12개 언어 문서 조각 판독 실측: gocr(문서 인식) 와 Zoro-1 비전(mmproj) 을 같은 조각으로 견줍니다.
//!
//!   python scripts/gen_ml_crops.py out/ml_crops
//!   cargo run -p blueprint-core --release --example ml_ocr_eval -- [조각 폴더] [결과 JSON] [--no-gocr] [--no-zoro]
//!
//! 글자 오류율(CER)은 띄어쓰기 · 줄바꿈을 한 칸으로 줄인 뒤 편집 거리 / 정답 글자 수입니다. 언어별 · 전체 표를 출력합니다.
//! 두 모델은 상주 관리자를 함께 써서 한 번에 하나만 올라옵니다 (gocr 비전 → gocr 언어 → Zoro-1 비전 → Zoro-1 텍스트 블록).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::accel;
use blueprint_core::ai::gocr::image::prepare as gocr_prepare;
use blueprint_core::ai::gocr::{open as open_gocr, GocrInput, Strategy, EARLY_EXIT, TEXT_PROMPT};
use blueprint_core::ai::model_store::model_dir;
use blueprint_core::ai::residency::{Budget, Residency};
use blueprint_core::ai::zoro::{vision, ZoroBackbone, TRANSCRIBE};
use image::RgbImage;
use serde_json::{json, Value};

fn normalize(s: &str) -> Vec<char> {
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().collect()
}

fn edits(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.iter().enumerate() {
        let mut cur = vec![i + 1; b.len() + 1];
        for (j, cb) in b.iter().enumerate() {
            cur[j + 1] = (prev[j] + (ca != cb) as usize).min(prev[j + 1] + 1).min(cur[j] + 1);
        }
        prev = cur;
    }
    prev[b.len()]
}

fn cer(pred: &str, truth: &str) -> (usize, usize) {
    let (p, t) = (normalize(pred), normalize(truth));
    (edits(&p, &t), t.len())
}

/// 흰 바탕의 글자 둘레로 자릅니다 (여백 10 px)
fn trim(img: &RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    let (mut x0, mut y0, mut x1, mut y1) = (w, h, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p.0.iter().any(|&c| c < 200) {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    if x1 < x0 {
        return img.clone();
    }
    let pad = 10;
    let (x0, y0) = (x0.saturating_sub(pad), y0.saturating_sub(pad));
    let (x1, y1) = ((x1 + pad).min(w - 1), (y1 + pad).min(h - 1));
    image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image()
}

fn main() -> Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let (no_gocr, no_zoro) = (all.iter().any(|a| a == "--no-gocr"), all.iter().any(|a| a == "--no-zoro"));
    let args: Vec<String> = all.into_iter().filter(|a| !a.starts_with("--")).collect();
    let dir = PathBuf::from(args.first().cloned().unwrap_or_else(|| "out/ml_crops".into()));
    let out = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "out/ml_ocr_eval.json".into()));
    let list: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(dir.join("crops.json")).context("crops.json (scripts/gen_ml_crops.py)")?)?;
    let images: Vec<RgbImage> = list.iter().map(|c| Ok(trim(&image::open(dir.join(c["image"].as_str().unwrap()))?.to_rgb8()))).collect::<Result<_>>()?;
    let device = accel::select_device()?;
    let res = Residency::shared(device.clone(), Budget::auto(&device));
    let mut rows: Vec<Value> = list.iter().map(|c| json!({ "image": c["image"], "lang": c["lang"], "truth": c["text"] })).collect();

    if !no_gocr {
        let gocr = open_gocr(&model_dir("gocr"), res.clone(), 512, Strategy::Speculative { early_exit: Some(EARLY_EXIT) })?;
        let inputs: Vec<GocrInput> = images.iter().map(|im| Ok(GocrInput { image: gocr_prepare(im, 14, 2)?, prompt: TEXT_PROMPT.to_string() })).collect::<Result<_>>()?;
        let t = Instant::now();
        let outs = gocr.recognize(&inputs)?;
        println!("[gocr] {} ({}) · {}개 · {:.1}s", accel::backend_name(&device), gocr.quant(), outs.len(), t.elapsed().as_secs_f64());
        for (r, o) in rows.iter_mut().zip(&outs) {
            let (e, n) = cer(&o.text, r["truth"].as_str().unwrap());
            r["gocr"] = json!({ "text": o.text, "edits": e, "chars": n, "min_conf": o.min_confidence, "ms": o.vision_ms + o.prefill_ms + o.decode_ms });
        }
    }
    if !no_zoro {
        let zoro = ZoroBackbone::open(&model_dir("zoro"), res.clone())?;
        let t = Instant::now();
        let outs = zoro.read_images(&images, TRANSCRIBE, 256, vision::MAX_PIXELS)?;
        println!("[zoro] {} ({}) · {}개 · {:.1}s", accel::backend_name(&device), zoro.quant(), outs.len(), t.elapsed().as_secs_f64());
        for (r, o) in rows.iter_mut().zip(&outs) {
            let (e, n) = cer(&o.text, r["truth"].as_str().unwrap());
            r["zoro"] = json!({ "text": o.text, "edits": e, "chars": n, "min_conf": o.min_confidence, "image_tokens": o.image_tokens, "ms": o.vision_ms + o.prefill_ms + o.decode_ms });
        }
    }
    // 언어별 표
    let mut by: BTreeMap<String, [(usize, usize, usize); 2]> = BTreeMap::new();
    for r in &rows {
        let lang = r["lang"].as_str().unwrap().to_string();
        let e = by.entry(lang).or_insert([(0, 0, 0); 2]);
        for (k, key) in ["gocr", "zoro"].iter().enumerate() {
            if let Some(v) = r.get(*key) {
                e[k].0 += v["edits"].as_u64().unwrap() as usize;
                e[k].1 += v["chars"].as_u64().unwrap() as usize;
                e[k].2 += (v["edits"].as_u64() == Some(0)) as usize;
            }
        }
    }
    println!("{:<4} {:>10} {:>8} {:>10} {:>8}", "언어", "gocr CER", "완전", "zoro CER", "완전");
    let mut table = Vec::new();
    for (lang, [g, z]) in &by {
        let f = |x: &(usize, usize, usize)| if x.1 == 0 { f64::NAN } else { x.0 as f64 / x.1 as f64 };
        println!("{lang:<4} {:>10.3} {:>8} {:>10.3} {:>8}", f(g), g.2, f(z), z.2);
        table.push(json!({ "lang": lang, "gocr_cer": f(g), "gocr_exact": g.2, "zoro_cer": f(z), "zoro_exact": z.2 }));
    }
    let report = json!({ "backend": accel::backend_name(&device), "rows": rows, "by_lang": table });
    if let Some(p) = out.parent() {
        std::fs::create_dir_all(p)?;
    }
    std::fs::write(&out, serde_json::to_string_pretty(&report)?)?;
    println!("[결과] {}", out.display());
    accel::finish_process(&device, 0);
    Ok(())
}
