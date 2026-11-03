//! 모델 폴더 명령줄 도구. 앱을 띄우지 않고 모델을 준비할 때 씁니다.
//!
//!   blueprint-models status            모델 폴더와 모델 상태
//!   blueprint-models import [이름...]   zoro-gravity 모델 폴더에서 가져오기 (이름이 없으면 없는 모델 모두)
//!   blueprint-models download <이름>    Hugging Face 에서 내려받기

use anyhow::{bail, Result};
use blueprint_models::{download, import_from, import_missing, import_source, models_dir, status};

fn gb(bytes: u64) -> String {
    format!("{:.2} GB", bytes as f64 / 1e9)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        None | Some("status") => {
            println!("모델 폴더: {}", models_dir().display());
            println!("가져올 곳: {}", import_source().map(|p| p.display().to_string()).unwrap_or_else(|| "없음".into()));
            for s in status() {
                let state = if s.installed {
                    "있음"
                } else if s.importable {
                    "가져올 수 있음"
                } else {
                    "없음"
                };
                println!("  {:<8} {:<16} {}", s.entry.name, state, gb(s.bytes_on_disk));
            }
        }
        Some("import") => {
            let done = if args.len() > 1 {
                let Some(source) = import_source() else { bail!("가져올 곳을 모릅니다. BLUEPRINT_IMPORT_FROM 을 주세요") };
                args[1..].iter().map(|n| import_from(&source, n).map(|r| (n.clone(), r))).collect::<Result<Vec<_>>>()?
            } else {
                import_missing()?.into_iter().map(|(n, r)| (n.to_string(), r)).collect()
            };
            if done.is_empty() {
                println!("가져올 모델이 없습니다 (이미 있거나 가져올 곳에 없습니다).");
            }
            for (name, r) in done {
                println!("{name}: 링크 {}개 · 복사 {}개 · 그대로 {}개 · {}", r.linked.len(), r.copied.len(), r.kept.len(), gb(r.bytes));
            }
        }
        Some("download") => {
            let Some(name) = args.get(1) else { bail!("내려받을 모델 이름을 주세요 (zoro, gocr, siglip2)") };
            download(name, &mut |p| println!("[{:>3}%] {}", p.percent, p.message))?;
        }
        Some(other) => bail!("모르는 명령입니다: {other} (status · import · download)"),
    }
    Ok(())
}
