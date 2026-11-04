//! SigLIP2 그림 사전(설비 심볼)을 만들고 재며, KKS 없는 설비 판독을 실제 모델로 잽니다 (개선안: 글자 없는 심볼 · 명판 훼손).
//!
//!   python scripts/gen_pid_drawings.py --symbols
//!   cargo run -p blueprint-core --release --example symbol_probe -- [--save] [--skip-damaged]
//!
//! 1. 학습 표본(out/symbols/train)으로 벡터 종류(풀링 · 패치 평균) × 패치 상한(64 · 256)마다 5겹 교차 검증을 하고,
//!    교차 검증 정확도가 가장 높은 설정으로 사전을 만듭니다. 받아들임 기준은 교차 검증에서 맞힌 표본 유사도 · 차이의 2 백분위수입니다.
//! 2. 시험 표본(out/symbols/test, 시드가 다름): 정확도 · 혼동 · 종류별 AUC · none 오수용 · 기권
//! 3. 마스터 도면 5장: 후보 추출 재현율(정답 심볼 상자와 IoU ≥ 0.5) · 오검출 · 분류, 그리고 심볼을 켠 판독의 KKS 회수와 추가 시간
//! 4. 명판 훼손 도면 L1(KKS 줄) · L2(KKS · 이름 줄) · L3(상자 글자 전부), 대조군 L0(마스터): 표제란과 원장 상자 영역만 gocr 로 다시 읽고
//!    (도면 조각 계획은 한 행의 상자들을 가로 띠 하나로 읽어 상자별 글자를 떼어 낼 수 없음) 심볼과 함께 KKS 없이 설비를 고릅니다.
//!    좌표 단서는 찾은 심볼의 위치만 씁니다.
//!
//! 결과는 out/symbol_probe.json 이고, `--save` 면 모델 폴더의 siglip2/symbol_dict.bin 에 사전을 씁니다 (crop_probe.bin 은 건드리지 않습니다).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use blueprint_core::ai::accel;
use blueprint_core::ai::document::{DocumentReader, ImageVectors, ReadOptions};
use blueprint_core::ai::model_store::models_dir;
use blueprint_core::ai::residency::{Budget, Residency};
use blueprint_core::extract::{extract, strip_tags, PageText};
use blueprint_core::identify::{clues_for_box, identify_only, ClueKind, ALL_CLUES};
use blueprint_core::lattice::{edit_distance, Lattice};
use blueprint_core::symbols::{candidates, crop_square, CandidateParams, Feature, SymbolDict, DICT_FILE, NONE};
use image::RgbImage;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
struct Index {
    classes: Vec<String>,
    samples: Vec<Sample>,
}

#[derive(Deserialize)]
struct Sample {
    path: String,
    class: String,
}

#[derive(Deserialize)]
struct SymbolBox {
    kks: String,
    class: String,
    #[serde(rename = "box")]
    bbox: [u32; 4],
}

#[derive(Deserialize)]
struct SheetJson {
    #[serde(default)]
    symbols: Vec<SymbolBox>,
}

/// 표제란 (도면 생성기의 TITLE_BOX 와 같은 정규화 좌표)
const TITLE_BOX: [f32; 4] = [0.030, 0.020, 0.970, 0.095];

fn load_split(dir: &Path, split: &str) -> Result<(Vec<String>, Vec<usize>, Vec<RgbImage>)> {
    let idx: Index = serde_json::from_str(&std::fs::read_to_string(dir.join(format!("{split}.json"))).with_context(|| format!("{split}.json 이 없습니다 (python scripts/gen_pid_drawings.py --symbols)"))?)?;
    let mut labels = Vec::new();
    let mut images = Vec::new();
    for s in &idx.samples {
        labels.push(idx.classes.iter().position(|c| c == &s.class).context("모르는 종류")?);
        images.push(image::open(dir.join(&s.path))?.to_rgb8());
    }
    Ok((idx.classes, labels, images))
}

fn pick(v: &[ImageVectors], f: Feature) -> Vec<Vec<f32>> {
    v.iter().map(|x| x.get(f).to_vec()).collect()
}

/// 5겹 교차 검증 (종류 순서로 놓인 표본을 5 개마다 나눔): (정답, 가장 가까운 종류, 유사도, 차이)
fn cross_validate(classes: &[String], labels: &[usize], xs: &[Vec<f32>], feature: Feature) -> Result<Vec<(usize, usize, f32, f32)>> {
    let mut out = Vec::new();
    for fold in 0..5 {
        let train: Vec<(usize, Vec<f32>)> = (0..xs.len()).filter(|i| i % 5 != fold).map(|i| (labels[i], xs[i].clone())).collect();
        let dict = SymbolDict::from_samples(feature, classes.to_vec(), &train)?;
        for i in (0..xs.len()).filter(|i| i % 5 == fold) {
            let m = dict.classify(&xs[i]);
            out.push((labels[i], classes.iter().position(|c| c == &m.nearest).unwrap(), m.sim, m.margin));
        }
    }
    Ok(out)
}

fn percentile(mut xs: Vec<f32>, p: f32) -> f32 {
    if xs.is_empty() {
        return 0.0;
    }
    xs.sort_by(f32::total_cmp);
    xs[((xs.len() - 1) as f32 * p).round() as usize]
}

/// 한 종류 대 나머지 AUC (Mann–Whitney)
fn auc(pos: &[f32], neg: &[f32]) -> f32 {
    let mut s = 0.0f64;
    for p in pos {
        for n in neg {
            s += if p > n { 1.0 } else if p == n { 0.5 } else { 0.0 };
        }
    }
    (s / (pos.len() * neg.len()).max(1) as f64) as f32
}

fn iou(a: [u32; 4], b: [u32; 4]) -> f32 {
    let ix = (a[2].min(b[2]) as f32 - a[0].max(b[0]) as f32).max(0.0);
    let iy = (a[3].min(b[3]) as f32 - a[1].max(b[1]) as f32).max(0.0);
    let inter = ix * iy;
    let area = |r: [u32; 4]| ((r[2] - r[0]) * (r[3] - r[1])) as f32;
    inter / (area(a) + area(b) - inter).max(1.0)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let save = args.iter().any(|a| a == "--save");
    let skip_damaged = args.iter().any(|a| a == "--skip-damaged");
    let root = PathBuf::from("out/symbols");
    let db = Lattice::embedded()?;
    let t = Instant::now();
    let device = accel::select_device()?;
    let backend = accel::backend_name(&device);
    let residency = Residency::shared(device.clone(), Budget::auto(&device));
    let mut reader = DocumentReader::open(&models_dir(), residency)?;
    println!("[엔진] {backend} · 모델 열기 {:.1}s", t.elapsed().as_secs_f64());

    // 1. 표본 벡터 (패치 상한마다)
    let (classes, train_y, train_img) = load_split(&root, "train")?;
    let (test_classes, test_y, test_img) = load_split(&root, "test")?;
    anyhow::ensure!(classes == test_classes, "학습 · 시험 종류가 다릅니다");
    let mut embedded: BTreeMap<usize, (Vec<ImageVectors>, Vec<ImageVectors>, f64)> = BTreeMap::new();
    for budget in [64usize, 256] {
        let t = Instant::now();
        let tr = reader.embed_images(&train_img, budget)?;
        let te = reader.embed_images(&test_img, budget)?;
        let ms = t.elapsed().as_secs_f64() * 1e3 / (train_img.len() + test_img.len()) as f64;
        println!("[벡터] 패치 상한 {budget}: 학습 {} · 시험 {} · 조각당 {ms:.1} ms", tr.len(), te.len());
        embedded.insert(budget, (tr, te, ms));
    }
    let none = classes.iter().position(|c| c == NONE).context("none 종류가 없습니다")?;
    let mut configs = Vec::new();
    let mut best: Option<(f32, usize, Feature)> = None;
    for (&budget, (tr, _, ms)) in &embedded {
        for feature in [Feature::Pooled, Feature::MeanPatch] {
            let cv = cross_validate(&classes, &train_y, &pick(tr, feature), feature)?;
            let acc = cv.iter().filter(|r| r.0 == r.1).count() as f32 / cv.len() as f32;
            println!("[교차 검증] {feature:?} · 패치 {budget}: 정확도 {:.3}", acc);
            configs.push(json!({ "feature": format!("{feature:?}"), "max_patches": budget, "cv_accuracy": acc, "ms_per_crop": ms }));
            if best.is_none_or(|b| acc > b.0) {
                best = Some((acc, budget, feature));
            }
        }
    }
    let (cv_acc, budget, feature) = best.unwrap();
    let (tr, te, _) = &embedded[&budget];
    let cv = cross_validate(&classes, &train_y, &pick(tr, feature), feature)?;
    let correct: Vec<&(usize, usize, f32, f32)> = cv.iter().filter(|r| r.0 == r.1 && r.0 != none).collect();
    let min_sim = percentile(correct.iter().map(|r| r.2).collect(), 0.02);
    let min_margin = percentile(correct.iter().map(|r| r.3).collect(), 0.02).min(0.02);
    let train: Vec<(usize, Vec<f32>)> = train_y.iter().copied().zip(pick(tr, feature)).collect();
    let mut dict = SymbolDict::from_samples(feature, classes.clone(), &train)?;
    dict.min_sim = min_sim;
    dict.min_margin = min_margin;
    println!("[사전] {feature:?} · 패치 {budget} (교차 검증 {cv_acc:.3}) · 받아들임: 유사도 ≥ {min_sim:.3}, 차이 ≥ {min_margin:.3}");

    // 2. 시험 표본
    let xs = pick(te, feature);
    let k = classes.len();
    let mut confusion = vec![vec![0usize; k]; k];
    let (mut accepted_right, mut accepted_wrong, mut abstain, mut none_accepted) = (0, 0, 0, 0);
    let mut scores: Vec<Vec<f32>> = Vec::new();
    for (x, &y) in xs.iter().zip(&test_y) {
        let m = dict.classify(x);
        let n = classes.iter().position(|c| c == &m.nearest).unwrap();
        confusion[y][n] += 1;
        match (&m.class, y == none) {
            (Some(_), true) => none_accepted += 1,
            (Some(c), false) if c == &classes[y] => accepted_right += 1,
            (Some(_), false) => accepted_wrong += 1,
            (None, false) => abstain += 1,
            (None, true) => {}
        }
        scores.push(dict.scores(x));
    }
    let argmax_acc = (0..k).map(|i| confusion[i][i]).sum::<usize>() as f32 / xs.len() as f32;
    let aucs: BTreeMap<String, f32> = (0..k)
        .map(|c| {
            let pos: Vec<f32> = scores.iter().zip(&test_y).filter(|(_, y)| **y == c).map(|(s, _)| s[c]).collect();
            let neg: Vec<f32> = scores.iter().zip(&test_y).filter(|(_, y)| **y != c).map(|(s, _)| s[c]).collect();
            (classes[c].clone(), auc(&pos, &neg))
        })
        .collect();
    let symbols_n = test_y.iter().filter(|y| **y != none).count();
    println!(
        "[시험] 가장 가까운 종류 정확도 {argmax_acc:.3} · 심볼 {symbols_n} 중 받아들여 맞음 {accepted_right} · 틀림 {accepted_wrong} · 기권 {abstain} · none {} 중 오수용 {none_accepted}",
        xs.len() - symbols_n
    );
    println!("[시험] 종류별 AUC {:?}", aucs.iter().map(|(c, a)| format!("{c} {a:.3}")).collect::<Vec<_>>());
    for (i, row) in confusion.iter().enumerate() {
        let off: Vec<String> = row.iter().enumerate().filter(|(j, n)| *j != i && **n > 0).map(|(j, n)| format!("{}×{n}", classes[j])).collect();
        if !off.is_empty() {
            println!("[혼동] {} → {}", classes[i], off.join(", "));
        }
    }

    // 3. 마스터 도면: 후보 추출 · 분류 · 심볼을 켠 판독
    reader.set_symbol_dict(Some(dict.clone()));
    reader.set_options(ReadOptions { symbols: true, ..reader.options() });
    let params = CandidateParams::default();
    let mut masters = Vec::new();
    let (mut gt_total, mut found_total, mut right_total, mut false_cands, mut false_accepted) = (0, 0, 0, 0, 0);
    for d in db.drawings() {
        let path = PathBuf::from(format!("core/fixtures/drawings/{}.png", d.id));
        let img = image::open(&path)?.to_rgb8();
        let sheet: SheetJson = serde_json::from_str(&std::fs::read_to_string(path.with_extension("json"))?)?;
        let t = Instant::now();
        let cands = candidates(&img, &params);
        let cand_ms = t.elapsed().as_secs_f64() * 1e3;
        let crops: Vec<RgbImage> = cands.iter().map(|c| crop_square(&img, c.bbox)).collect();
        let t = Instant::now();
        let vectors = reader.embed_images(&crops, budget)?;
        let embed_ms = t.elapsed().as_secs_f64() * 1e3;
        let matches: Vec<_> = vectors.iter().map(|v| dict.classify(v.get(feature))).collect();
        let mut used = vec![false; cands.len()];
        let mut rows = Vec::new();
        for g in &sheet.symbols {
            let hit = cands.iter().enumerate().map(|(i, c)| (i, iou(c.bbox, g.bbox))).filter(|(_, v)| *v >= 0.5).max_by(|a, b| a.1.total_cmp(&b.1));
            let (got, right) = match hit {
                Some((i, _)) => {
                    used[i] = true;
                    (matches[i].class.clone(), matches[i].class.as_deref() == Some(g.class.as_str()))
                }
                None => (None, false),
            };
            found_total += hit.is_some() as usize;
            right_total += right as usize;
            rows.push(json!({ "kks": g.kks, "class": g.class, "found": hit.is_some(), "got": got, "nearest": hit.map(|(i, _)| matches[i].nearest.clone()), "sim": hit.map(|(i, _)| matches[i].sim) }));
        }
        let extra: Vec<Value> = cands.iter().zip(&matches).zip(&used).filter(|(_, u)| !**u).map(|((c, m), _)| json!({ "bbox": c.bbox, "nearest": m.nearest, "accepted": m.class, "sim": m.sim })).collect();
        let extra_accepted = extra.iter().filter(|e| !e["accepted"].is_null()).count();
        gt_total += sheet.symbols.len();
        false_cands += extra.len();
        false_accepted += extra_accepted;
        // 심볼을 켠 판독: KKS 는 그대로 읽히는가, 심볼 단계가 얼마나 더 걸리는가
        let t = Instant::now();
        let reading = reader.read_pages(std::slice::from_ref(&img))?.remove(0);
        let read_ms = t.elapsed().as_secs_f64() * 1e3;
        let x = extract(&PageText::from_reading("probe", img.dimensions(), &reading), &db);
        let expected = db.on_drawing(&d.id);
        let kks_found = expected.iter().filter(|e| x.kks_hits.iter().any(|h| h.code == e.kks)).count();
        println!(
            "[도면] {} 심볼 {}/{} 찾음 · 분류 맞음 {} · 후보 {} (오검출 {} · 그중 받아들임 {}) · 후보 {cand_ms:.0} ms · 벡터 {embed_ms:.0} ms · KKS {kks_found}/{} · 판독 {:.1}s (심볼 {:.0} ms)",
            d.id,
            rows.iter().filter(|r| r["found"] == true).count(),
            sheet.symbols.len(),
            rows.iter().filter(|r| r["got"] == r["class"]).count(),
            cands.len(),
            extra.len(),
            extra_accepted,
            expected.len(),
            read_ms / 1e3,
            reading.timings.symbol_ms
        );
        masters.push(json!({ "drawing": d.id, "symbols": rows, "false_candidates": extra, "candidate_ms": cand_ms, "embed_ms": embed_ms, "kks_found": kks_found, "kks_expected": expected.len(), "read_ms": read_ms, "symbol_ms": reading.timings.symbol_ms }));
    }
    println!("[도면 합계] 심볼 {found_total}/{gt_total} 찾음 · 분류 맞음 {right_total} · 오검출 후보 {false_cands} · 그중 받아들임 {false_accepted}");

    // 4. 명판 훼손 도면 (L0 = 마스터 대조군). 도면은 조각 계획이 한 행의 상자들을 가로 띠 하나로 읽어 상자별 글자를 떼어 낼 수 없으므로,
    //    표제란과 원장 상자 영역만 다시 읽습니다 (부분 재판독). 좌표 단서는 찾은 심볼의 위치만 씁니다 (원장 상자 자체가 아님).
    let masks: [(&str, &[ClueKind]); 8] = [
        ("이름 · 위치", &[ClueKind::Name, ClueKind::Location]),
        ("항목 · 단위", &[ClueKind::Item]),
        ("계기 태그", &[ClueKind::Tag]),
        ("심볼", &[ClueKind::Symbol]),
        ("심볼 + 위치", &[ClueKind::Symbol, ClueKind::Location]),
        ("심볼 위치(좌표)", &[ClueKind::Coordinate]),
        ("좌표 빼고 전체", &[ClueKind::Name, ClueKind::Location, ClueKind::Tag, ClueKind::Item, ClueKind::Symbol]),
        ("전체", &ALL_CLUES),
    ];
    let mut damaged = Vec::new();
    if !skip_damaged {
        for level in 0..=3 {
            let mut tally: BTreeMap<&str, [usize; 4]> = BTreeMap::new();
            let mut boxes = Vec::new();
            let (mut kks_direct, mut drawing_ok) = (0, 0);
            let mut ms = 0.0;
            for d in db.drawings() {
                let path = if level == 0 { PathBuf::from(format!("core/fixtures/drawings/{}.png", d.id)) } else { root.join(format!("damaged/L{level}/{}.png", d.id)) };
                let img = image::open(&path).with_context(|| format!("{} 이 없습니다", path.display()))?.to_rgb8();
                let (w, h) = (img.width() as f32, img.height() as f32);
                let px = |b: [f32; 4]| [(b[0] * w) as u32, (b[1] * h) as u32, (b[2] * w) as u32, (b[3] * h) as u32];
                let equipment: Vec<(String, [f32; 4])> = db.on_drawing(&d.id).iter().map(|e| (e.kks.clone(), e.bbox)).collect();
                let t = Instant::now();
                let hits = reader.find_symbols(&[&img])?.remove(0);
                let mut regions = vec![px(TITLE_BOX)];
                regions.extend(equipment.iter().map(|(_, b)| px(*b)));
                let tuples: Vec<(u32, u32, u32, u32)> = regions.iter().map(|r| (r[0], r[1], r[2], r[3])).collect();
                let texts = reader.read_regions(&img, &tuples, 5)?;
                ms += t.elapsed().as_secs_f64() * 1e3;
                // 표제란의 도면 번호 (원장 번호와 편집 거리 2 이내면 고칩니다)
                let title = strip_tags(&texts[0].text);
                let drawing_read = db.drawings().iter().map(|x| (x, title.split_whitespace().map(|w| edit_distance(w.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '-'), &x.id)).min().unwrap_or(99))).filter(|(_, dist)| *dist <= 2).min_by_key(|(_, dist)| *dist).map(|(x, _)| x.id.clone());
                drawing_ok += (drawing_read.as_deref() == Some(d.id.as_str())) as usize;
                for (i, (kks, bbox)) in equipment.iter().enumerate() {
                    let text = &texts[i + 1].text;
                    let read_kks = strip_tags(text).split_whitespace().any(|w| db.match_kks(w.trim_matches(|c: char| !c.is_ascii_alphanumeric())).code() == Some(kks.as_str()));
                    kks_direct += read_kks as usize;
                    let clues = clues_for_box(text, &hits, img.dimensions(), drawing_read.clone(), *bbox);
                    for (name, kinds) in &masks {
                        let r = identify_only(&db, &clues, kinds);
                        let slot = match (r.best.as_deref() == Some(kks.as_str()), r.ambiguous) {
                            (true, false) => 0,
                            (true, true) => 1,
                            (false, true) => 2,
                            (false, false) => 3,
                        };
                        tally.entry(name).or_default()[slot] += 1;
                        if *name == "전체" {
                            boxes.push(json!({ "drawing": d.id, "kks": kks, "kks_read": read_kks, "best": r.best, "ambiguous": r.ambiguous, "reason": r.reason, "drawing_read": drawing_read, "lines": clues.lines, "symbol": clues.symbol }));
                        }
                    }
                }
            }
            let n = boxes.len();
            println!("[훼손 L{level}] 상자 {n} · KKS 직접 {kks_direct} · 도면 번호 {drawing_ok}/{} · 심볼 + 영역 재판독 {:.1}s/장", db.drawings().len(), ms / 1e3 / db.drawings().len() as f64);
            for (name, _) in &masks {
                let [ok, amb_top, amb, wrong] = tally[name];
                println!("    {name:<14} 맞음 {ok:>2} · 애매(1위 맞음) {amb_top:>2} · 애매 {amb:>2} · 틀림 {wrong:>2}  (/{n})");
            }
            damaged.push(json!({ "level": level, "boxes": n, "kks_direct": kks_direct, "drawing_read": drawing_ok, "ms_per_sheet": ms / db.drawings().len() as f64, "tally": tally.iter().map(|(k, v)| (k.to_string(), json!({ "correct": v[0], "ambiguous_top": v[1], "ambiguous": v[2], "wrong": v[3] }))).collect::<BTreeMap<_, _>>(), "detail": boxes }));
        }
    }

    let out = json!({
        "at": chrono::Utc::now().to_rfc3339(),
        "backend": backend,
        "configs": configs,
        "chosen": { "feature": format!("{feature:?}"), "max_patches": budget, "cv_accuracy": cv_acc, "min_sim": min_sim, "min_margin": min_margin },
        "test": { "samples": xs.len(), "symbols": symbols_n, "argmax_accuracy": argmax_acc, "accepted_right": accepted_right, "accepted_wrong": accepted_wrong, "abstain": abstain, "none_accepted": none_accepted, "auc": aucs, "confusion": confusion, "classes": classes },
        "masters": masters,
        "master_totals": { "symbols": gt_total, "found": found_total, "right": right_total, "false_candidates": false_cands, "false_accepted": false_accepted },
        "damaged": damaged,
    });
    std::fs::create_dir_all("out")?;
    std::fs::write("out/symbol_probe.json", serde_json::to_string_pretty(&out)?)?;
    println!("결과: out/symbol_probe.json");
    if save {
        let path = models_dir().join("siglip2").join(DICT_FILE);
        dict.save(&path)?;
        println!("사전 저장: {}", path.display());
    }
    // ROCm 을 쓴 Windows 프로세스는 보통 종료하면 rocblas 정리에서 멈춰 GPU 메모리를 쥔 채 남습니다 (accel::finish_process).
    accel::finish_process(&device, 0);
    Ok(())
}
