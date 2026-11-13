//! Zoro-1 비전(mmproj) 확인.
//!
//!   cargo run -p blueprint-core --release --example zoro_vision -- dump <이미지> <출력 폴더>
//!       패치 행렬(`patches.f32`)과 이미지 토큰(`tokens.f32`), 격자(`meta.json`)를 씁니다. `BLUEPRINT_DEVICE=cpu` 면 CPU(F32) 로
//!       계산해 transformers `Qwen3_5VisionModel` 과 수치를 대조합니다 (`scripts/zoro_vision_reference.py`).
//!   cargo run -p blueprint-core --release --example zoro_vision -- read <이미지> [지시문]
//!       이미지를 읽고 지시문대로 씁니다 (비전 단계 → 비전을 내림 → 텍스트 블록). 상주 기록으로 두 단계가 겹치지 않았는지 봅니다.

use std::path::PathBuf;

use anyhow::{bail, Result};
use blueprint_core::ai::accel;
use blueprint_core::ai::model_store::model_dir;
use blueprint_core::ai::residency::{Action, Budget, Group, Residency};
use blueprint_core::ai::zoro::model::StreamMode;
use blueprint_core::ai::zoro::{vision, ZoroBackbone, TRANSCRIBE};
use serde_json::json;

fn f32_bytes(v: &[f32]) -> Vec<u8> {
    v.iter().flat_map(|x| x.to_le_bytes()).collect()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let device = accel::select_device()?;
    let res = Residency::shared(device.clone(), Budget::auto(&device));
    let bb = ZoroBackbone::open_with(&model_dir("zoro"), res.clone(), StreamMode::Auto)?;
    match (args.first().map(String::as_str), args.get(1)) {
        (Some("dump"), Some(img)) => {
            let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "out/zoro_vision".into()));
            std::fs::create_dir_all(&out)?;
            let cfg = bb.model().vision_config()?;
            let image = image::open(img)?.to_rgb8();
            let prepared = vision::prepare(&image, &cfg, vision::MAX_PIXELS)?;
            let t = std::time::Instant::now();
            let tokens = bb.model().encode_image(&prepared)?;
            let ms = t.elapsed().as_secs_f64() * 1e3;
            std::fs::write(out.join("patches.f32"), f32_bytes(&prepared.patches))?;
            std::fs::write(out.join("tokens.f32"), f32_bytes(&tokens.flatten_all()?.to_vec1::<f32>()?))?;
            let meta = json!({ "grid_h": prepared.grid_h, "grid_w": prepared.grid_w, "tokens": tokens.dims(), "device": accel::backend_name(&device), "ms": ms, "patch": cfg.patch });
            std::fs::write(out.join("meta.json"), serde_json::to_string_pretty(&meta)?)?;
            println!("[비전] {} · 격자 {} × {} → 이미지 토큰 {:?} · {ms:.0} ms → {}", accel::backend_name(&device), prepared.grid_h, prepared.grid_w, tokens.dims(), out.display());
        }
        (Some("read"), Some(img)) => {
            let image = image::open(img)?.to_rgb8();
            let instruction = args.get(2).cloned().unwrap_or_else(|| TRANSCRIBE.to_string());
            let r = bb.read_image(&image, &instruction, 512, vision::MAX_PIXELS)?;
            println!("[판독] 이미지 토큰 {} · 비전 {:.0} ms · 프리필 {:.0} ms · 디코드 {:.0} ms · 토큰 {} · 최소 확신 {:.3}\n{}", r.image_tokens, r.vision_ms, r.prefill_ms, r.decode_ms, r.tokens, r.min_confidence, r.text);
            let r = res.lock().unwrap();
            let log: Vec<String> = r.history().iter().map(|t| format!("{} {}", if t.action == Action::Load { "올림" } else { "내림" }, t.group)).collect();
            println!("[상주] {}", log.join(" → "));
            // 비전과 텍스트 블록이 함께 올라와 있던 때가 없어야 합니다.
            let mut vision_up = false;
            for t in r.history() {
                match (t.action, t.group) {
                    (Action::Load, Group::Vision) => vision_up = true,
                    (Action::Release, Group::Vision) => vision_up = false,
                    (Action::Load, Group::Blocks { .. }) if vision_up => bail!("비전이 올라온 채로 텍스트 블록을 올렸습니다"),
                    _ => {}
                }
            }
        }
        _ => bail!("사용법: zoro_vision dump <이미지> <폴더> | read <이미지> [지시문]"),
    }
    accel::finish_process(&device, 0);
    Ok(())
}
