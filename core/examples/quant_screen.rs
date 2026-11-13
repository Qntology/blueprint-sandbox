//! 양자화 배분 측정 (피라미드 양자화, `ai::pyramid`). 기준은 원본 BF16 을 F32 로 푼 모델입니다 (gocr).
//!
//!   cargo run -p blueprint-core --release --example quant_screen -- crops <조각 폴더>
//!   cargo run -p blueprint-core --release --example quant_screen -- gocr <조각 폴더> <결과 JSON> <배분>... [--no-free] [--file] [--ref <폴더>]
//!   cargo run -p blueprint-core --release --example quant_screen -- zoro <덤프 폴더> <결과 JSON> <배분>... [--no-free] [--file]
//!   cargo run -p blueprint-core --release --example quant_screen -- imatrix <gocr|zoro> <출력> <재료 폴더>...
//!
//! - `crops`: 합성 도면 · 현장 스캔(발전소 · AI 데이터센터 마스터 10장, 점검표 · Red-line 6장)을 엔진과 같게 SigLIP2 로 계획해
//!   gocr 에 넣을 조각을 PNG 와 `crops.json`(조각마다 지시문)으로 저장합니다. 측정마다 같은 입력을 쓰기 위해서입니다.
//! - `gocr`: 처음에는 기준 모델 `ref` (원본 BF16 을 F32 로 둔 파일, `pyramid -- reference gocr <폴더>`)로 조각을 모두 읽어
//!   기준 토큰열(`ref_f32.json`)을 저장합니다. 배분마다 올릴 때 원본에서 한 번 양자화해 (파일을 쓰지 않음, `ModelSpec::with_requant`
//!   + `with_original`) 기준 토큰열을 teacher forcing 으로 읽어 ΔNLL(기준 대비) · 1위 뒤집힘을 재고, `--no-free` 가 없으면 자유롭게 읽어
//!   글이 기준과 같은 조각 수를 셉니다. 비전 · 언어 단계의 장치 상주량과 시간도 적습니다.
//!   `file:<언어 GGUF>[,<mmproj GGUF>]` 은 그 파일들을 그대로 엽니다 (출력 투영 잘라 두기 포함, `pyramid -- gocr <배분> --out <폴더>`).
//!   `--file` 이면 배분 이름 대신 모델 폴더의 피라미드 파일(`active.json` 의 `pyramid`)을 엽니다.
//! - `zoro`: 기준은 q8 (Zoro-1 의 F32 는 8 GB VRAM 에 들어가지 않음). 판단 프롬프트의 jev_head 확률과 글쓰기를 견줍니다.
//!
//! 배분 이름은 `ai::pyramid::gocr_plan` · `zoro_plan`. 앞에 `txt:` · `vis:` 를 붙이면 언어 · 비전 한쪽만, 끝에 `+im` 을 붙이면
//! `--imatrix <파일>` 의 중요도 행렬로 양자화합니다.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use anyhow::{bail, Context, Result};
use blueprint_core::ai::accel;
use blueprint_core::ai::document::{cut, DocumentReader};
use blueprint_core::ai::gocr::image::prepare as gocr_prepare;
use blueprint_core::ai::gocr::{open_variant, GocrEngine, GocrInput, Strategy, Variant, EARLY_EXIT};
use blueprint_core::ai::model_store::{model_dir, models_dir};
use blueprint_core::ai::original::{Family, Original};
use blueprint_core::ai::pyramid::{gocr_plan, Plan};
use blueprint_core::ai::residency::{Action, Budget, Group, Residency, SharedResidency};
use blueprint_core::ai::siglip2::plan::CropKind;
use blueprint_core::ai::weights::TensorMeta;
use blueprint_core::domain::DomainMode;
use blueprint_core::lattice::Lattice;
use serde_json::{json, Value};

const MAX_TOKENS: usize = 1024;

fn pages() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for mode in [DomainMode::PowerPlant, DomainMode::AiDataCenter] {
        let db = Lattice::embedded_for(mode).expect("원장");
        for d in db.drawings() {
            out.push(PathBuf::from(format!("core/fixtures/drawings/{}.png", d.id)));
        }
    }
    for s in ["P1_gauge", "P3_redline", "P3F_redline", "D1_gauge", "D3_redline", "D3F_redline"] {
        out.push(PathBuf::from(format!("out/field/{s}.png")));
    }
    out
}

fn extract_crops(dir: &Path) -> Result<()> {
    std::fs::create_dir_all(dir)?;
    let device = accel::select_device()?;
    let residency = Residency::shared(device.clone(), Budget::auto(&device));
    let reader = DocumentReader::open(&models_dir(), residency)?;
    let mut list = Vec::new();
    for page in pages() {
        let img = image::open(&page).with_context(|| format!("{} 가 없습니다 (python scripts/gen_pid_drawings.py)", page.display()))?.to_rgb8();
        let (straight, _, plan, _) = reader.plan(&img)?;
        let stem = page.file_stem().unwrap().to_string_lossy().to_string();
        for (i, c) in plan.crops.iter().filter(|c| c.kind != CropKind::Sign).enumerate() {
            let piece = cut(&straight, c.bbox, c.upscale);
            let name = format!("{stem}_{i:02}.png");
            piece.save(dir.join(&name))?;
            // 엔진(`DocumentReader::prompt_of`)과 같은 지시문: 표 · 입력란은 표, 제목은 글자
            let prompt = if matches!(c.kind, CropKind::Table | CropKind::Form) { "Table Recognition:" } else { "Text Recognition:" };
            list.push(json!({ "image": name, "page": stem, "kind": c.kind.name(), "prompt": prompt, "w": piece.width(), "h": piece.height() }));
        }
        println!("[조각] {} → {}개", page.display(), list.iter().filter(|v| v["page"] == stem).count());
    }
    std::fs::write(dir.join("crops.json"), serde_json::to_string_pretty(&list)?)?;
    println!("[조각] 모두 {}개 → {}", list.len(), dir.display());
    blueprint_core::ai::accel::finish_process(&device, 0);
    Ok(())
}

fn load_inputs(dir: &Path) -> Result<(Vec<Value>, Vec<GocrInput>)> {
    let mut list: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(dir.join("crops.json")).context("crops.json 이 없습니다 (먼저 crops)")?)?;
    // `--limit N`: 앞 N 조각만 (프로필 등 짧은 측정)
    if let Some(n) = flag_value("--limit").and_then(|v| v.parse::<usize>().ok()) {
        list.truncate(n);
    }
    let inputs = list
        .iter()
        .map(|c| {
            let img = image::open(dir.join(c["image"].as_str().unwrap()))?.to_rgb8();
            Ok(GocrInput { image: gocr_prepare(&img, 14, 2)?, prompt: c["prompt"].as_str().unwrap().to_string() })
        })
        .collect::<Result<Vec<_>>>()?;
    Ok((list, inputs))
}

fn flag_value(flag: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).cloned()
}

fn requant_of(plan: Plan) -> blueprint_core::ai::residency::Requant {
    Arc::new(move |meta: &TensorMeta| if meta.name.starts_with("model.visual.") { (plan.vision)(&meta.name) } else { (plan.text)(&meta.name) })
}

/// 상주 기록에서 비전 · 언어 단계에 올린 바이트
fn phase_bytes(res: &SharedResidency) -> (u64, u64) {
    let r = res.lock().unwrap();
    let (mut vision, mut language) = (0u64, 0u64);
    for t in r.history().iter().filter(|t| t.action == Action::Load) {
        match t.group {
            Group::Vision => vision = vision.max(t.bytes),
            Group::Blocks { .. } | Group::Output | Group::Head => language += t.bytes,
            _ => {}
        }
    }
    (vision, language)
}

/// `--imatrix <파일>` 로 준 중요도 행렬 (배분 이름 끝의 `+im` 이 이것을 씀)
fn imatrix_arg() -> Option<Arc<blueprint_core::ai::pyramid::Imatrix>> {
    let p = flag_value("--imatrix")?;
    Some(Arc::new(blueprint_core::ai::pyramid::load_imatrix(Path::new(&p)).expect("중요도 행렬 파일")))
}

/// 모델 폴더의 원본 BF16 (`bf16/`)
fn original_of(model: &str) -> Result<Option<Arc<Original>>> {
    let family = Family::of(model).context("모르는 모델")?;
    Ok(match Original::find(&model_dir(model), family) {
        Some(o) => Some(Arc::new(o?)),
        None => None,
    })
}

/// 측정 기준 파일 폴더 (`--ref <폴더>`, 기본 out/quant/ref)
fn ref_dir() -> PathBuf {
    PathBuf::from(flag_value("--ref").unwrap_or_else(|| "out/quant/ref".into()))
}

fn open(label: &str, res: SharedResidency, from_file: bool, original: Option<&Arc<Original>>) -> Result<GocrEngine> {
    let dir = model_dir("gocr");
    let strategy = Strategy::Speculative { early_exit: Some(EARLY_EXIT) };
    if from_file {
        std::env::set_var(blueprint_core::ai::pyramid::PYRAMID_ENV, "1");
        let e = open_variant(&dir, res, MAX_TOKENS, strategy, Variant::Auto)?;
        if e.quant() == "q8" {
            bail!("모델 폴더에 피라미드 파일이 없습니다 (examples/pyramid)");
        }
        return Ok(e);
    }
    // `ref`: 원본 BF16 을 F32 로 둔 두 파일 (`pyramid -- reference gocr <폴더>`)
    if label == "ref" {
        let r = ref_dir();
        let (text, vision) = (r.join("gocr-f32.gguf"), r.join("mmproj-gocr-f32.gguf"));
        if !text.exists() || !vision.exists() {
            bail!("{} 에 기준 파일이 없습니다 (pyramid -- reference gocr {})", r.display(), r.display());
        }
        return blueprint_core::ai::gocr::open_paths(&dir, &text, Some(&vision), res, MAX_TOKENS, strategy);
    }
    // `file:<언어 GGUF>[,<mmproj GGUF>]`: 그 파일들로 (mmproj 를 주지 않으면 모델 폴더의 원래 mmproj)
    if let Some(paths) = label.strip_prefix("file:") {
        let (text, vision) = match paths.split_once(',') {
            Some((t, v)) => (PathBuf::from(t), Some(PathBuf::from(v))),
            None => (PathBuf::from(paths), blueprint_core::ai::gguf_model::load_active(&dir)?.mmproj),
        };
        return blueprint_core::ai::gocr::open_paths(&dir, &text, vision.as_deref(), res, MAX_TOKENS, strategy);
    }
    let e = open_variant(&dir, res, MAX_TOKENS, strategy, Variant::Q8)?;
    // `q8src:<배분>`: 진단 전용. 원본 대신 q8 파일의 Q8_0 을 풀어 같은 인코더로 다시 양자화합니다 (두 번 양자화의 영향,
    // `BLUEPRINT_REQUANT_FROM_Q8=1` 과 함께).
    let (label_rest, original) = match label.strip_prefix("q8src:") {
        Some(rest) => (rest, None),
        None => (label, original),
    };
    let (name, with_im) = match label_rest.strip_suffix("+im") {
        Some(base) => (base, true),
        None => (label_rest, false),
    };
    // `txt:<배분>` · `vis:<배분>` 은 언어 · 비전 한쪽만 그 배분으로 (나머지는 q8 · BF16 그대로): 오차가 어디서 오는지 가립니다.
    let (only, name) = match name.split_once(':') {
        Some((part @ ("txt" | "vis"), rest)) => (Some(part), rest),
        _ => (None, name),
    };
    if name == "q8" {
        return Ok(e);
    }
    let mut plan = gocr_plan(name).with_context(|| format!("모르는 배분: {name}"))?;
    if plan.head.is_some() {
        bail!("{name}: 출력 투영을 자르는 배분은 파일로 만들어 재세요 (pyramid -- gocr {name} --out <폴더>, 그다음 file:<파일>)");
    }
    match only {
        Some("txt") => plan.vision = |_| None,
        Some("vis") => plan.text = |_| None,
        _ => {}
    }
    let mut e = e.with_requant(label, requant_of(plan));
    if let Some(o) = original {
        e = e.with_original(Arc::clone(o));
    }
    if with_im {
        e = e.with_imatrix(imatrix_arg().context("+im 배분에는 --imatrix <파일> 이 필요합니다")?);
    }
    Ok(e)
}

/// 중요도 행렬 모으기: gocr 는 gravity-ocr 의 합성 보정 조각(평가 조각과 따로)을, zoro 는 저장소 문서(docs/*.md) 글을 q8 로 읽으며 모읍니다.
fn collect_imatrix(model: &str, out: &Path, sources: &[String]) -> Result<()> {
    use blueprint_core::ai::residency::imatrix;
    let device = accel::select_device()?;
    let res = Residency::shared(device.clone(), Budget::auto(&device));
    let t = Instant::now();
    let rows = if model == "gocr" {
        let mut inputs = Vec::new();
        for dir in sources {
            let dir = Path::new(dir);
            let samples: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(dir.join("samples.json"))?)?;
            for s in samples {
                let img = image::open(dir.join(s["image"].as_str().unwrap()))?.to_rgb8();
                let prompt = if s["prompt"] == "table" { "Table Recognition:" } else { "Text Recognition:" };
                inputs.push(GocrInput { image: gocr_prepare(&img, 14, 2)?, prompt: prompt.to_string() });
            }
        }
        let e = open("q8", res.clone(), false, None)?;
        imatrix::start();
        let outs = e.recognize(&inputs)?;
        println!("[중요도] gocr 보정 조각 {}개 · 토큰 {}", inputs.len(), outs.iter().map(|o| o.tokens.len()).sum::<usize>());
        imatrix::stop()
    } else {
        use blueprint_core::ai::zoro::model::StreamMode;
        use blueprint_core::ai::zoro::ZoroBackbone;
        let bb = ZoroBackbone::open_variant(&model_dir("zoro"), res.clone(), StreamMode::Auto, true)?;
        let mut texts = Vec::new();
        for dir in sources {
            for e in std::fs::read_dir(dir)?.flatten().filter(|e| e.path().extension().is_some_and(|x| x == "md")) {
                texts.push(std::fs::read_to_string(e.path())?);
            }
        }
        imatrix::start();
        let mut tokens = 0usize;
        for text in &texts {
            let ids = bb.tokenizer().encode(text)?.ids;
            // 문서 하나를 256 토큰 조각으로: 앞 절반을 프롬프트, 뒤 절반을 teacher forcing (블록 · 출력 투영을 모두 지남).
            // 어휘 248,078 의 로짓이 행마다 1 MB 라 조각을 짧게 둡니다.
            for chunk in ids.chunks(256).filter(|c| c.len() >= 64) {
                let half = chunk.len() / 2;
                bb.model().score_text(&chunk[..half], &chunk[half..])?;
                tokens += chunk.len();
            }
        }
        println!("[중요도] zoro 문서 {}개 · 토큰 {tokens}", texts.len());
        imatrix::stop()
    };
    let m: blueprint_core::ai::pyramid::Imatrix = rows.into_iter().map(|(k, (v, _))| (k, v)).collect();
    blueprint_core::ai::pyramid::save_imatrix(out, &m)?;
    println!("[중요도] {} 텐서 → {} · {:.1}s", m.len(), out.display(), t.elapsed().as_secs_f64());
    res.lock().unwrap().release_all()?;
    accel::finish_process(&device, 0);
    Ok(())
}

fn screen_gocr(dir: &Path, out: &Path, labels: &[String], free: bool, from_file: bool) -> Result<()> {
    let (list, inputs) = load_inputs(dir)?;
    let device = accel::select_device()?;
    let original = original_of("gocr")?;
    let ref_path = dir.join("ref_f32.json");
    if !ref_path.exists() {
        let res = Residency::shared(device.clone(), Budget::auto(&device));
        let e = open("ref", res.clone(), false, None)?;
        let t = Instant::now();
        let outs = e.recognize(&inputs)?;
        let refs: Vec<Value> = outs.iter().map(|o| json!({ "tokens": o.tokens, "text": o.text })).collect();
        std::fs::write(&ref_path, serde_json::to_string(&refs)?)?;
        println!("[기준] 원본 F32 로 {}개 읽음 · {:.1}s → {}", outs.len(), t.elapsed().as_secs_f64(), ref_path.display());
        res.lock().unwrap().release_all()?;
    }
    let refs: Vec<Value> = serde_json::from_str(&std::fs::read_to_string(&ref_path)?)?;
    let targets: Vec<Vec<u32>> = refs.iter().map(|r| r["tokens"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect()).collect();
    let texts: Vec<String> = refs.iter().map(|r| r["text"].as_str().unwrap().to_string()).collect();
    let mut report: Vec<Value> = if out.exists() { serde_json::from_str(&std::fs::read_to_string(out)?).unwrap_or_default() } else { Vec::new() };
    // ΔNLL 의 기준: 이번 실행이나 앞선 실행의 `ref` 행
    let mut base_mean: Option<f64> = report.iter().find(|r| r["plan"] == "ref").and_then(|r| r["nll"].as_f64());
    for label in labels {
        let res = Residency::shared(device.clone(), Budget::auto(&device));
        let e = open(label, res.clone(), from_file, original.as_ref())?;
        // 끝 토큰까지 맞히는지도 보려고 정답 끝에 끝 토큰을 붙입니다 (gocr eos 의 첫 번호).
        let eos = e.decoder().eos_first();
        let full: Vec<Vec<u32>> = targets.iter().map(|t| t.iter().copied().chain(std::iter::once(eos)).collect()).collect();
        let t = Instant::now();
        let scored = e.score(&inputs, &full)?;
        let score_s = t.elapsed().as_secs_f64();
        let nll: Vec<Vec<f32>> = scored.iter().map(|s| s.iter().map(|x| x.0).collect()).collect();
        let tokens: usize = full.iter().map(Vec::len).sum();
        let flips: usize = scored.iter().zip(&full).map(|(s, t)| s.iter().zip(t).filter(|(x, want)| x.1 != **want).count()).sum();
        let mean = nll.iter().flatten().map(|v| *v as f64).sum::<f64>() / tokens.max(1) as f64;
        if label == "ref" && !from_file {
            base_mean = Some(mean);
        }
        let (vision_bytes, language_bytes) = phase_bytes(&res);
        let mut row = json!({
            "plan": label, "quant": e.quant(), "file": from_file, "tokens": tokens, "nll": mean, "dnll": base_mean.map(|b| mean - b), "flips": flips,
            "flip_rate": flips as f64 / tokens.max(1) as f64, "score_s": score_s, "vision_mb": vision_bytes as f64 / 1048576.0, "language_mb": language_bytes as f64 / 1048576.0,
        });
        println!("[측정] {label} ({}) · NLL {mean:.5} ΔNLL {:?} · 뒤집힘 {flips}/{tokens} · 비전 {:.0} MB · 언어 {:.0} MB · {score_s:.1}s", e.quant(), base_mean.map(|b| mean - b), vision_bytes as f64 / 1048576.0, language_bytes as f64 / 1048576.0);
        if free {
            res.lock().unwrap().release_all()?;
            let t = Instant::now();
            let outs = e.recognize(&inputs)?;
            let secs = t.elapsed().as_secs_f64();
            let same = outs.iter().zip(&texts).filter(|(o, t)| o.text == **t).count();
            let diffs: Vec<Value> = outs
                .iter()
                .zip(&texts)
                .zip(&list)
                .filter(|((o, t), _)| o.text != **t)
                .take(30)
                .map(|((o, t), c)| json!({ "crop": c["image"], "ref": t, "got": o.text }))
                .collect();
            let (vis, pre, dec) = outs.iter().fold((0.0, 0.0, 0.0), |a, o| (a.0 + o.vision_ms, a.1 + o.prefill_ms, a.2 + o.decode_ms));
            let toks: usize = outs.iter().map(|o| o.tokens.len()).sum();
            let prof = blueprint_core::ai::gocr::text::profile::take();
            let prof: serde_json::Map<String, Value> = blueprint_core::ai::gocr::text::profile::NAMES.iter().zip(prof).map(|(n, ms)| (n.to_string(), json!(ms))).collect();
            if blueprint_core::ai::gocr::text::profile::on() {
                println!("[프로필] {label} · {prof:?}");
            }
            row["free"] = json!({ "same": same, "total": outs.len(), "secs": secs, "vision_ms": vis, "prefill_ms": pre, "decode_ms": dec, "tokens": toks, "ms_per_token": dec / toks.max(1) as f64, "diffs": diffs, "profile": prof });
            println!("[자유] {label} · 글 같음 {same}/{} · {secs:.1}s (비전 {:.1}s · 프리필 {:.1}s · 디코드 {:.1}s, 토큰당 {:.2} ms)", outs.len(), vis / 1e3, pre / 1e3, dec / 1e3, dec / toks.max(1) as f64);
        }
        res.lock().unwrap().release_all()?;
        report.retain(|r| r["plan"] != *label || r["file"] != json!(from_file));
        report.push(row);
        std::fs::write(out, serde_json::to_string_pretty(&report)?)?;
    }
    accel::finish_process(&device, 0);
    Ok(())
}

/// `z-<처음>-<마지막>-<가장자리>-<주의>-<FFN in>-<FFN out>` (예: `z-4-4-q8-q6-q4-q5`) 또는 이름 있는 배분(`zoro-spec` 등)
fn zoro_requant(label: &str) -> Result<Option<blueprint_core::ai::residency::Requant>> {
    use blueprint_core::ai::pyramid::{zoro_plan, zoro_type, ZoroSpec, ZORO_LAYERS};
    use candle_core::quantized::GgmlDType;
    if label == "q8" {
        return Ok(None);
    }
    if let Some(p) = zoro_plan(label) {
        if p.head.is_some() {
            bail!("{label}: 출력 투영을 자르는 배분은 파일로 만들어 --file 로 재세요");
        }
        let rule = p.text;
        return Ok(Some(Arc::new(move |m: &TensorMeta| rule(&m.name))));
    }
    let parts: Vec<&str> = label.split('-').collect();
    let ty = |s: &str| -> Result<GgmlDType> {
        Ok(match s {
            "q4" => GgmlDType::Q4K,
            "q5" => GgmlDType::Q5K,
            "q6" => GgmlDType::Q6K,
            "q8" => GgmlDType::Q8_0,
            _ => bail!("모르는 형식 {s} (q4 · q5 · q6 · q8)"),
        })
    };
    if parts.len() != 7 || parts[0] != "z" {
        bail!("모르는 배분: {label}");
    }
    let spec = ZoroSpec { first: parts[1].parse()?, last: parts[2].parse()?, edge: ty(parts[3])?, attn: ty(parts[4])?, ffn_in: ty(parts[5])?, ffn_out: ty(parts[6])? };
    Ok(Some(Arc::new(move |m: &TensorMeta| zoro_type(spec, ZORO_LAYERS, &m.name))))
}

fn read_jsonl(path: &Path) -> Result<Vec<Value>> {
    Ok(std::fs::read_to_string(path).with_context(|| format!("{} 가 없습니다 (BLUEPRINT_ZORO_DUMP 로 plant_probe 를 돌리세요)", path.display()))?.lines().filter(|l| !l.trim().is_empty()).map(serde_json::from_str).collect::<std::result::Result<_, _>>()?)
}

fn screen_zoro(dir: &Path, out: &Path, labels: &[String], free: bool, from_file: bool) -> Result<()> {
    use blueprint_core::ai::jev_head::JevHead;
    use blueprint_core::ai::pointer::{Backbone, PointerPrompt};
    use blueprint_core::ai::zoro::model::StreamMode;
    use blueprint_core::ai::zoro::ZoroBackbone;
    let mut seen = std::collections::HashSet::new();
    let prompts: Vec<PointerPrompt> = read_jsonl(&dir.join("pointer.jsonl"))?
        .into_iter()
        .filter(|v| seen.insert(v["text"].as_str().unwrap_or("").to_string()))
        .map(|v| PointerPrompt { text: v["text"].as_str().unwrap().to_string(), query_byte: v["query_byte"].as_u64().unwrap() as usize, option_bytes: v["option_bytes"].as_array().unwrap().iter().map(|b| b.as_u64().unwrap() as usize).collect() })
        .collect();
    let mut seen = std::collections::HashSet::new();
    let writes: Vec<Value> = read_jsonl(&dir.join("writer.jsonl"))?.into_iter().filter(|v| v["quant"] == "q8" && seen.insert(format!("{}{}", v["system"], v["user"]))).collect();
    println!("[zoro] 판단 프롬프트 {} · 글쓰기 {}", prompts.len(), writes.len());
    let zdir = model_dir("zoro");
    let jev = JevHead::from_safetensors(&zdir.join("jev_head.safetensors"))?;
    let device = accel::select_device()?;
    let original = original_of("zoro")?;
    let base_path = dir.join("pointer_q8.json");
    let mut report: Vec<Value> = if out.exists() { serde_json::from_str(&std::fs::read_to_string(out)?).unwrap_or_default() } else { Vec::new() };
    for label in labels {
        let res = Residency::shared(device.clone(), Budget::auto(&device));
        if from_file {
            std::env::set_var(blueprint_core::ai::pyramid::PYRAMID_ENV, "1");
        }
        let mut bb = ZoroBackbone::open_variant(&zdir, res.clone(), StreamMode::Auto, !from_file)?;
        if !from_file {
            let (name, with_im) = match label.strip_suffix("+im") {
                Some(base) => (base, true),
                None => (label.as_str(), false),
            };
            if let Some(rq) = zoro_requant(name)? {
                bb = bb.with_requant(label.clone(), rq);
                if let Some(o) = &original {
                    bb = bb.with_original(Arc::clone(o));
                }
                if with_im {
                    bb = bb.with_imatrix(imatrix_arg().context("+im 배분에는 --imatrix <파일> 이 필요합니다")?);
                }
            }
        }
        // 판단: jev_head 확률
        let t = Instant::now();
        let mut probs: Vec<Vec<f32>> = Vec::with_capacity(prompts.len());
        for p in &prompts {
            let h = bb.encode(p)?;
            probs.push(jev.probabilities(&h.query, &h.options)?);
        }
        let judge_s = t.elapsed().as_secs_f64();
        if label == "q8" && !from_file {
            std::fs::write(&base_path, serde_json::to_string(&probs)?)?;
        }
        let base: Option<Vec<Vec<f32>>> = std::fs::read_to_string(&base_path).ok().and_then(|s| serde_json::from_str(&s).ok());
        let mut row = json!({ "plan": label, "quant": bb.quant(), "file": from_file, "prompts": prompts.len(), "judge_s": judge_s });
        if let Some(base) = &base {
            let (mut max_d, mut sum_d, mut n, mut argmax_same, mut gate_same) = (0f32, 0f64, 0usize, 0usize, 0usize);
            let argmax = |v: &[f32]| v.iter().enumerate().max_by(|a, b| a.1.total_cmp(b.1)).map(|(i, _)| i).unwrap_or(0);
            for (a, b) in probs.iter().zip(base) {
                for (x, y) in a.iter().zip(b) {
                    max_d = max_d.max((x - y).abs());
                    sum_d += (x - y).abs() as f64;
                    n += 1;
                }
                argmax_same += (argmax(a) == argmax(b)) as usize;
                // 자동 확정 기준 0.95 를 넘는지가 같은가
                let top = |v: &[f32]| v.iter().copied().fold(0f32, f32::max) >= 0.95;
                gate_same += (top(a) == top(b)) as usize;
            }
            row["max_dp"] = json!(max_d);
            row["mean_dp"] = json!(sum_d / n.max(1) as f64);
            row["argmax_same"] = json!(argmax_same);
            row["gate_same"] = json!(gate_same);
            println!("[판단] {label} ({}) · 확률 차 최대 {max_d:.4} · 평균 {:.5} · 1위 같음 {argmax_same}/{} · 0.95 기준 같음 {gate_same}/{} · {judge_s:.1}s", bb.quant(), sum_d / n.max(1) as f64, prompts.len(), prompts.len());
        }
        // 글쓰기: 기준 글을 teacher forcing 으로
        let t = Instant::now();
        let (mut nll_sum, mut toks, mut flips) = (0f64, 0usize, 0usize);
        let stop = bb.tokenizer().token_to_id("<|im_end|>").unwrap_or(0);
        for w in &writes {
            let prompt: Vec<u32> = w["prompt_ids"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
            let mut target: Vec<u32> = w["tokens"].as_array().unwrap().iter().map(|v| v.as_u64().unwrap() as u32).collect();
            if w["finished"].as_bool() == Some(true) {
                target.push(stop);
            }
            let s = bb.model().score_text(&prompt, &target)?;
            nll_sum += s.iter().map(|x| x.0 as f64).sum::<f64>();
            toks += s.len();
            flips += s.iter().zip(&target).filter(|(x, t)| x.1 != **t).count();
        }
        row["write_nll"] = json!(nll_sum / toks.max(1) as f64);
        row["write_tokens"] = json!(toks);
        row["write_flips"] = json!(flips);
        row["write_score_s"] = json!(t.elapsed().as_secs_f64());
        println!("[글 점수] {label} · NLL {:.5} · 뒤집힘 {flips}/{toks}", nll_sum / toks.max(1) as f64);
        if free {
            let t = Instant::now();
            let mut same = 0;
            let mut diffs = Vec::new();
            let (mut decode_ms, mut tokens) = (0f64, 0usize);
            for w in &writes {
                let got = bb.write(w["system"].as_str().unwrap(), w["user"].as_str().unwrap(), w["max_tokens"].as_u64().unwrap() as usize)?;
                decode_ms += got.decode_ms;
                tokens += got.tokens;
                if got.text == w["text"].as_str().unwrap() {
                    same += 1;
                } else if diffs.len() < 12 {
                    diffs.push(json!({ "q8": w["text"], "got": got.text }));
                }
            }
            row["free"] = json!({ "same": same, "total": writes.len(), "secs": t.elapsed().as_secs_f64(), "ms_per_token": decode_ms / tokens.max(1) as f64, "diffs": diffs });
            println!("[자유 글] {label} · 같음 {same}/{} · 토큰당 {:.1} ms", writes.len(), decode_ms / tokens.max(1) as f64);
        }
        let (_, language) = phase_bytes(&res);
        row["text_mb"] = json!(language as f64 / 1048576.0);
        res.lock().unwrap().release_all()?;
        report.retain(|r| r["plan"] != *label || r["file"] != json!(from_file));
        report.push(row);
        std::fs::write(out, serde_json::to_string_pretty(&report)?)?;
    }
    accel::finish_process(&device, 0);
    Ok(())
}

fn main() -> Result<()> {
    let all: Vec<String> = std::env::args().skip(1).collect();
    let free = !all.iter().any(|a| a == "--no-free");
    let from_file = all.iter().any(|a| a == "--file");
    // `--imatrix <파일>` · `--limit <N>` · `--ref <폴더>` 의 값은 위치 인자에서 뺍니다.
    let values: Vec<usize> = all.iter().enumerate().filter(|(_, a)| matches!(a.as_str(), "--imatrix" | "--limit" | "--ref")).map(|(i, _)| i + 1).collect();
    let args: Vec<String> = all.iter().enumerate().filter(|(i, a)| !a.starts_with("--") && !values.contains(i)).map(|(_, a)| a.clone()).collect();
    match args.first().map(String::as_str) {
        Some("imatrix") => {
            let model = args.get(1).map(String::as_str).unwrap_or("gocr");
            let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| format!("out/quant/{model}_imatrix.bin")));
            collect_imatrix(model, &out, &args[3.min(args.len())..])
        }
        Some("crops") => extract_crops(Path::new(args.get(1).map(String::as_str).unwrap_or("out/quant/crops"))),
        Some("gocr") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "out/quant/crops".into()));
            let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "out/quant/gocr_screen.json".into()));
            let labels: Vec<String> = if args.len() > 3 { args[3..].to_vec() } else { vec!["ref".into(), "q8".into(), "vision".into()] };
            screen_gocr(&dir, &out, &labels, free, from_file)
        }
        Some("zoro") => {
            let dir = PathBuf::from(args.get(1).cloned().unwrap_or_else(|| "out/quant/zoro_dump".into()));
            let out = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| "out/quant/zoro_screen.json".into()));
            let labels: Vec<String> = if args.len() > 3 { args[3..].to_vec() } else { vec!["q8".into(), "zoro-spec".into()] };
            screen_zoro(&dir, &out, &labels, free, from_file)
        }
        _ => bail!("사용법: quant_screen crops <폴더> | gocr <조각 폴더> <결과 JSON> <배분>... | zoro <덤프 폴더> <결과 JSON> <배분>... | imatrix <모델> <출력> <폴더>... [--no-free] [--file] [--ref <폴더>]"),
    }
}
