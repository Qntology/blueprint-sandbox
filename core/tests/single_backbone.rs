//! 언어 · 비전 언어 모델은 Zoro-1 하나뿐인지 (logis-center 계보의 두 번째 소형 백본이 없는지) 확인합니다.
//!
//! - 모델 목록(`blueprint-models`)은 판단 · 글쓰기 · 다국어 판독 `zoro`, 문서 인식 `gocr`, 크롭 `siglip2`, 검색 `embed` · `stanza` 다섯뿐입니다.
//! - 엔진의 모델 종류(`ModelId`)는 zoro · gocr · siglip2 셋뿐입니다.
//! - 저장소의 코드 · 설정 · 스크립트 · 시험 자료에 2B 모델 이름이 없습니다 (빌드 산출물 · `node_modules` · `out` 제외).
//!   문서(`.md`)는 제거 경위를 적는 곳이라 보지 않고, 이 파일도 찾을 이름을 담고 있어 뺍니다.

use std::path::{Path, PathBuf};

use blueprint_core::ai::residency::ModelId;

/// 찾을 이름. 이 파일 자체가 걸리지 않게 조각을 이어 만듭니다.
fn forbidden() -> Vec<String> {
    let q = "qwen";
    vec![
        format!("{q}3.5-2b"),
        format!("{q}3.5_2b"),
        format!("{q}3_5_2b"),
        format!("{q}3.5 2b"),
        format!("{q}2b"),
        format!("{q}-2b"),
        format!("{q}_2b"),
        format!("{q}35_2b"),
    ]
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("저장소").to_path_buf()
}

fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let p = e.path();
        let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        if p.is_dir() {
            if name.starts_with("target") || name == "node_modules" || name == "out" || name == ".git" || name == "dist" || name == "__pycache__" {
                continue;
            }
            walk(&p, out);
        } else if name != "single_backbone.rs"
            && ["rs", "toml", "json", "ts", "tsx", "js", "py", "bat", "sh", "yml", "yaml", "html", "css", "command"].iter().any(|ext| name.ends_with(&format!(".{ext}")))
        {
            out.push(p);
        }
    }
}

#[test]
fn no_file_names_a_second_qwen_backbone() {
    let mut files = Vec::new();
    walk(&repo_root(), &mut files);
    assert!(files.len() > 100, "저장소 파일을 찾지 못했습니다 ({}개)", files.len());
    let words = forbidden();
    let mut hits = Vec::new();
    for f in &files {
        let Ok(text) = std::fs::read_to_string(f) else { continue };
        let lower = text.to_lowercase();
        for w in &words {
            if lower.contains(w.as_str()) {
                hits.push(format!("{} : {w}", f.display()));
            }
        }
    }
    assert!(hits.is_empty(), "2B 백본의 흔적: {hits:?}");
}

#[test]
fn the_model_registry_holds_one_language_backbone() {
    let names: Vec<&str> = blueprint_models::CATALOG.iter().map(|e| e.name).collect();
    assert_eq!(names, vec!["zoro", "gocr", "siglip2", "embed", "stanza"]);
    for e in blueprint_models::CATALOG.iter() {
        let repo = e.repo.unwrap_or("").to_lowercase();
        assert!(!repo.contains("qwen"), "{}: {repo}", e.name);
        assert!(!e.label.to_lowercase().contains("2b"), "{}: {}", e.name, e.label);
    }
    // Zoro-1 은 비전(mmproj)까지 한 묶음입니다: 다국어 문서 판독은 같은 모델의 비전 타워가 맡습니다.
    let zoro = blueprint_models::entry("zoro").expect("zoro");
    assert!(zoro.files.iter().any(|f| f.name.starts_with("mmproj-zoro")));
    // 엔진이 아는 모델 종류
    for arch in ["zoro", "gocr", "siglip2"] {
        assert!(ModelId::from_architecture(arch).is_ok());
    }
    assert!(ModelId::from_architecture("qwen35").is_err());
}
