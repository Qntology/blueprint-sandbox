//! 피라미드 양자화 파일 만들기 (`ai::pyramid`). 모델 폴더 `bf16/` 의 원본 BF16 에서 한 번 양자화해 같은 폴더에 두고
//! `active.json` 의 `pyramid` 에 적습니다 (`--out <폴더>` 를 주면 그 폴더에 쓰기만, 측정용).
//!
//!   cargo run -p blueprint-core --release --example pyramid -- gocr [배분] [--imatrix <파일>] [--out <폴더>]   (기본 vision)
//!   cargo run -p blueprint-core --release --example pyramid -- zoro [배분] [--imatrix <파일>] [--out <폴더>]   (기본 q8)
//!   cargo run -p blueprint-core --release --example pyramid -- reference <gocr|zoro> <폴더>   원본을 F32 로 둔 측정 기준 파일
//!   cargo run -p blueprint-core --release --example pyramid -- audit <gocr|zoro>              원본과 지금 파일의 대응 · 한 번 양자화 확인
//!   cargo run -p blueprint-core --release --example pyramid -- ensure                          앱처럼 기본 배분 파일이 없으면 만듦
//!   cargo run -p blueprint-core --release --example pyramid -- remove <gocr|zoro>
//!
//! GPU 를 쓰지 않습니다 (CPU 로 양자화). 앱이 그 모델의 피라미드 파일을 열고 있으면 바꿀 수 없으니 앱을 끄고 하세요.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use blueprint_core::ai::model_store::model_dir;
use blueprint_core::ai::pyramid::{self, build, default_plan, gocr_plan, zoro_plan};

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut log = |m: &str| println!("{m}");
    let positional: Vec<&str> = {
        let values: Vec<usize> = args.iter().enumerate().filter(|(_, a)| *a == "--imatrix" || *a == "--out").map(|(i, _)| i + 1).collect();
        args.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && !values.contains(i)).map(|(_, a)| a.as_str()).collect()
    };
    match (positional.first().copied(), positional.get(1).copied()) {
        // 앱과 같은 사용할 때 양자화: 기본 배분 · 레시피의 파일이 없으면 만듭니다 (`runtime::open` 이 부르는 `pyramid::ensure`)
        (Some("ensure"), _) => {
            let device = blueprint_core::ai::accel::select_device()?;
            let t = std::time::Instant::now();
            pyramid::ensure(&blueprint_core::ai::model_store::models_dir(), &device, &mut log)?;
            println!("[피라미드] ensure · {:.1}s", t.elapsed().as_secs_f64());
            blueprint_core::ai::accel::finish_process(&device, 0);
        }
        (Some("remove"), Some(model)) => {
            pyramid::remove(&model_dir(model))?;
            println!("[피라미드] {model} 의 피라미드 파일을 지웠습니다 (q8 로 돌아감)");
        }
        (Some("reference"), Some(model)) => {
            let out = PathBuf::from(positional.get(2).copied().unwrap_or("out/quant/ref"));
            for b in pyramid::reference(&model_dir(model), model, &out, &mut log)? {
                println!("[기준] {} · {:.1} MB", b.path.display(), b.bytes as f64 / 1048576.0);
            }
        }
        (Some("audit"), Some(model)) => {
            let rows = pyramid::audit(&model_dir(model), model)?;
            // 저장 형식마다: 텐서 수 · 원본 대응 최대 상대오차 · 같은 텐서 수 (BF16 은 비트까지, Q8_0 은 원본의 GGML 기준 양자화와 바이트까지)
            let mut by: BTreeMap<String, (usize, f32, usize)> = BTreeMap::new();
            for (name, dtype, err, same) in &rows {
                let e = by.entry(dtype.clone()).or_default();
                e.0 += 1;
                if err.is_finite() {
                    e.1 = e.1.max(*err);
                }
                e.2 += *same as usize;
                if !same && dtype != "원본 없음" && dtype != "F32" {
                    println!("  다름: {name} ({dtype}, 상대오차 {err:.5})");
                }
            }
            for (dtype, (n, worst, same)) in &by {
                println!("[감사] {model} {dtype}: 텐서 {n} · 원본 대응 최대 상대오차 {worst:.5} · 같음 {same}/{n}");
            }
        }
        (Some(model @ ("gocr" | "zoro")), plan) => {
            let name = plan.unwrap_or(default_plan(model));
            let plan = match model {
                "gocr" => gocr_plan(name),
                _ => zoro_plan(name),
            }
            .with_context(|| format!("모르는 배분: {name}"))?;
            let corpus = pyramid::corpus(model == "zoro");
            // 중요도 행렬: `--imatrix <파일>` (quant_screen imatrix 가 만듦). 없으면 균일 가중 탐색
            let imatrix = match flag(&args, "--imatrix") {
                Some(p) => Some(std::sync::Arc::new(pyramid::load_imatrix(Path::new(&p))?)),
                None => None,
            };
            let out = flag(&args, "--out").map(PathBuf::from);
            let built = build(&model_dir(model), model, &plan, &corpus, imatrix, out.as_deref(), &mut log)?;
            for b in &built {
                println!("[피라미드] {} · {:.1} MB · {:?}", b.path.display(), b.bytes as f64 / 1048576.0, b.by_type.iter().map(|(t, n)| format!("{t} {:.1} MB", *n as f64 / 1048576.0)).collect::<Vec<_>>());
            }
        }
        _ => bail!("사용법: pyramid <gocr|zoro> [배분] [--imatrix <파일>] [--out <폴더>] | reference <모델> <폴더> | audit <모델> | ensure | remove <모델>"),
    }
    Ok(())
}
