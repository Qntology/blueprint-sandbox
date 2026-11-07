//! 모델 폴더 · 모델 목록 · 상태 · 삭제 · zoro-gravity 에서 가져오기.
//!
//! 모델은 저장소가 아니라 OS 의 앱 데이터 폴더에 둡니다 (zoro-gravity · logis-center 와 같은 방식).
//!
//! | OS | 위치 |
//! | --- | --- |
//! | Windows | `%LOCALAPPDATA%\blueprint\models` (`C:\Users\<사용자>\AppData\Local\...`) |
//! | macOS | `~/Library/Application Support/blueprint/models` |
//! | Linux | `$XDG_DATA_HOME/blueprint/models` (없으면 `~/.local/share/blueprint/models`) |
//!
//! 모델은 zoro-gravity 와 같습니다 (`zoro` 판단 · 글쓰기, `gocr` 문서 인식, `siglip2` 크롭). 그래서 zoro-gravity 가 이미 받아 둔
//! 모델이 있으면 내려받지 않고 가져옵니다 ([`import_from`]). 큰 가중치(GGUF · safetensors)는 하드 링크로 걸어 디스크를 두 번 쓰지 않고,
//! 작은 파일(`active.json`, 크롭 앵커 `crop_probe.bin` 등)은 복사합니다. 작은 파일은 앱이 고쳐 쓸 수 있어서 두 앱이 같은 파일을 보면 안 됩니다.
//! 하드 링크를 걸 수 없으면 (다른 드라이브 등) 큰 파일도 복사합니다.
//!
//! 검색용 모델 두 가지는 선택입니다 (없어도 앱은 돌고, 검색이 규칙 · 글자 경로만 씁니다).
//! - `embed`: granite-embedding-97m-multilingual-r2 (IBM, Apache-2.0). logis-center 가 받아 둔 것이 있으면 가져오고, 없으면 Hugging Face 에서 받습니다.
//! - `stanza`: Stanza en · ko 품사 · 원형 ONNX. 공개 저장소가 없어 `stanza-onnx-export` 로 변환한 폴더(`onnx_final`)에서 가져옵니다
//!   (`BLUEPRINT_STANZA_FROM` 또는 `blueprint-models import stanza <폴더>`).
//!
//! 다운로드는 `download` 기능에 있습니다. 코어 엔진은 폴더 위치만 씁니다.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

#[cfg(feature = "download")]
mod download;
#[cfg(feature = "download")]
pub use download::{download, Progress, MIRROR_ENV};

/// 앱 데이터 폴더 이름
pub const APP_DIR: &str = "blueprint";
/// 모델 폴더를 직접 정하는 환경 변수 (시험 · 공용 저장소용)
pub const MODELS_DIR_ENV: &str = "BLUEPRINT_MODELS_DIR";
/// 가져올 모델 폴더를 정하는 환경 변수. 없으면 zoro-gravity 의 앱 데이터 폴더(`zorogravity/models`)를 봅니다.
pub const IMPORT_FROM_ENV: &str = "BLUEPRINT_IMPORT_FROM";
/// zoro-gravity 의 앱 데이터 폴더 이름
pub const ZOROGRAVITY_APP_DIR: &str = "zorogravity";
/// logis-center 의 앱 데이터 폴더 이름 (임베딩 모델을 가져옴)
pub const LOGIS_APP_DIR: &str = "logis-center";
/// 임베딩 모델을 가져올 폴더를 정하는 환경 변수 (없으면 logis-center 의 모델 폴더)
pub const EMBED_FROM_ENV: &str = "BLUEPRINT_EMBED_FROM";
/// Stanza 변환본(`stanza-onnx-export` 의 onnx_final)을 가져올 폴더를 정하는 환경 변수
pub const STANZA_FROM_ENV: &str = "BLUEPRINT_STANZA_FROM";

/// 가져올 곳
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// zoro-gravity 모델 폴더의 같은 이름 하위 폴더
    ZoroGravity,
    /// logis-center 모델 폴더의 이 하위 폴더 (`BLUEPRINT_EMBED_FROM` 이 있으면 그 폴더)
    LogisCenter(&'static str),
    /// `BLUEPRINT_STANZA_FROM` (stanza-onnx-export 출력 폴더)
    StanzaExport,
}

/// 내려받을 파일 하나. `bytes` 는 진행률과 화면 표시에 쓰는 크기입니다 (실제 완료 판단은 서버가 알려 준 크기로 합니다).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelFile {
    pub name: &'static str,
    pub bytes: u64,
}

const fn file(name: &'static str, bytes: u64) -> ModelFile {
    ModelFile { name, bytes }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelEntry {
    /// 폴더 이름. `load_active` 와 예제가 이 이름을 씁니다.
    pub name: &'static str,
    pub label: &'static str,
    pub license: &'static str,
    /// Hugging Face 저장소. 없으면 내려받지 못하고 직접 변환해서 넣습니다.
    pub repo: Option<&'static str>,
    /// `active.json` 을 맨 뒤에 둡니다. 끝까지 받기 전에는 모델이 열리지 않습니다.
    pub files: &'static [ModelFile],
    /// 내려받지는 않지만 가져올 때 함께 가져오는 파일 (있을 때만). 예: 학습한 크롭 앵커
    pub extras: &'static [&'static str],
    /// 없으면 앱이 설정을 열어 받게 하는가 (검색용 모델은 선택이라 false)
    pub required: bool,
    pub source: Source,
}

impl ModelEntry {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }
}

pub const CATALOG: [ModelEntry; 5] = [
    ModelEntry {
        name: "zoro",
        label: "Zoro-1 판단 · 글쓰기",
        license: "Apache-2.0",
        repo: Some("Qntology/zoro-1-gguf"),
        files: &[
            file("zoro-q8.gguf", 4_482_356_320),
            file("mmproj-zoro-bf16.gguf", 667_052_960),
            file("tokenizer.json", 19_989_510),
            file("jev_head.safetensors", 5_245_232),
            file("block_diffusion.json", 34),
            file("active.json", 216),
        ],
        extras: &[],
        required: true,
        source: Source::ZoroGravity,
    },
    ModelEntry {
        name: "gocr",
        label: "GravityOCR 문서 인식",
        license: "MIT",
        repo: Some("Qntology/GravityOCR-KO-gguf"),
        files: &[
            file("gocr-q8.gguf", 715_315_392),
            file("mmproj-gocr-bf16.gguf", 868_268_128),
            file("tokenizer.json", 6_838_959),
            file("block_diffusion.json", 237),
            file("active.json", 216),
        ],
        extras: &[],
        required: true,
        source: Source::ZoroGravity,
    },
    ModelEntry {
        name: "siglip2",
        label: "SigLIP2 도면 · 점검표 크롭",
        license: "Apache-2.0",
        repo: Some("Qntology/siglip2-so400m-patch16-naflex-gguf"),
        files: &[
            file("siglip2-q8.gguf", 878_951_840),
            file("mmproj-siglip2-bf16.gguf", 855_814_752),
            file("tokenizer.json", 34_356_304),
            file("active.json", 208),
        ],
        // 시각 앵커는 zoro-gravity 가 합성 서식으로 학습한 파일입니다. 없으면 크롭이 버리는 조각 없이 모두 읽습니다.
        extras: &["crop_probe.bin", "precision-q8.json"],
        required: true,
        source: Source::ZoroGravity,
    },
    ModelEntry {
        name: "embed",
        label: "검색 임베딩 (granite-embedding-97m)",
        license: "Apache-2.0",
        repo: Some("ibm-granite/granite-embedding-97m-multilingual-r2"),
        files: &[file("config.json", 1_261), file("tokenizer.json", 27_136_500), file("model.safetensors", 194_889_568)],
        extras: &[],
        required: false,
        source: Source::LogisCenter("granite-embedding-97m-multilingual-r2"),
    },
    ModelEntry {
        name: "stanza",
        label: "검색 품사 · 원형 (Stanza en · ko ONNX)",
        license: "Apache-2.0",
        repo: None,
        files: &[
            file("en/pos.onnx", 176_310_517),
            file("en/pretrain_vocab.json", 2_833_253),
            file("en/lemma_encoder.onnx", 630_677),
            file("en/lemma_decoder.onnx", 1_563_390),
            file("en/lemma_dict.json", 15_011_754),
            file("ko/pos.onnx", 121_091_769),
            file("ko/pretrain_vocab.json", 3_389_054),
            file("ko/lemma_encoder.onnx", 1_045_877),
            file("ko/lemma_decoder.onnx", 3_664_307),
            file("ko/lemma_dict.json", 2_835_424),
            // 사전을 마지막에 둡니다. vocab.json 이 있어야 그 언어를 여는데, 큰 파일을 다 가져오기 전에 열리지 않게
            file("en/vocab.json", 197_296),
            file("ko/vocab.json", 272_693),
        ],
        extras: &[],
        required: false,
        source: Source::StanzaExport,
    },
];

pub fn entry(name: &str) -> Option<&'static ModelEntry> {
    CATALOG.iter().find(|e| e.name == name)
}

/// 앱 데이터 폴더. OS 가 위치를 알려 주지 않으면 실행 폴더 아래에 둡니다.
pub fn app_dir() -> PathBuf {
    match dirs::data_local_dir() {
        Some(dir) => dir.join(APP_DIR),
        None => PathBuf::from("blueprint-data"),
    }
}

/// 모델 폴더
pub fn models_dir() -> PathBuf {
    match std::env::var_os(MODELS_DIR_ENV) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => app_dir().join("models"),
    }
}

/// 모델 하나의 폴더 (`zoro`, `gocr`, `siglip2`)
pub fn model_dir(name: &str) -> PathBuf {
    models_dir().join(name)
}

/// 가져올 곳: `BLUEPRINT_IMPORT_FROM` 또는 zoro-gravity 의 모델 폴더
pub fn import_source() -> Option<PathBuf> {
    match std::env::var_os(IMPORT_FROM_ENV) {
        Some(dir) if !dir.is_empty() => Some(PathBuf::from(dir)),
        _ => dirs::data_local_dir().map(|d| d.join(ZOROGRAVITY_APP_DIR).join("models")),
    }
}

/// 이 모델을 가져올 폴더 (모델 폴더 자체). 모르면 None
pub fn source_dir(entry: &ModelEntry) -> Option<PathBuf> {
    let env = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from);
    match entry.source {
        Source::ZoroGravity => import_source().map(|s| s.join(entry.name)),
        Source::LogisCenter(folder) => env(EMBED_FROM_ENV).or_else(|| dirs::data_local_dir().map(|d| d.join(LOGIS_APP_DIR).join("models").join(folder))),
        Source::StanzaExport => env(STANZA_FROM_ENV),
    }
}

/// 화면에 보여 줄 모델 상태
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelStatus {
    pub entry: &'static ModelEntry,
    /// 필요한 파일이 모두 있는가
    pub installed: bool,
    /// 폴더가 차지하는 바이트 수 (받다가 멈춘 파일 포함, 하드 링크도 크기대로 셉니다)
    pub bytes_on_disk: u64,
    /// 가져올 곳에 이 모델이 다 있는가 (설치되어 있으면 false)
    pub importable: bool,
}

/// `.tmp` 를 끝까지 받은 뒤에 이름을 바꾸므로, 파일이 있으면 완전한 파일입니다.
fn installed(dir: &Path, entry: &ModelEntry) -> bool {
    let has = |name: &str| fs::metadata(dir.join(name)).is_ok_and(|m| m.len() > 0);
    if entry.files.is_empty() {
        return has("active.json");
    }
    entry.files.iter().all(|f| has(f.name))
}

fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter_map(|e| e.metadata().ok().map(|m| (e.path(), m)))
        .map(|(p, m)| if m.is_dir() { dir_bytes(&p) } else { m.len() })
        .sum()
}

pub fn status() -> Vec<ModelStatus> {
    CATALOG
        .iter()
        .map(|entry| {
            let dir = model_dir(entry.name);
            let here = installed(&dir, entry);
            let importable = !here && source_dir(entry).is_some_and(|s| s != dir && installed(&s, entry));
            ModelStatus { entry, installed: here, bytes_on_disk: dir_bytes(&dir), importable }
        })
        .collect()
}

/// 가져오기 결과
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ImportReport {
    /// 하드 링크로 건 파일
    pub linked: Vec<String>,
    /// 복사한 파일
    pub copied: Vec<String>,
    /// 이미 같은 크기로 있어서 건드리지 않은 파일
    pub kept: Vec<String>,
    /// 가져온 바이트 수 (링크 포함)
    pub bytes: u64,
}

/// 큰 가중치만 하드 링크로 겁니다. 나머지는 복사해서 한쪽이 고쳐도 다른 쪽이 바뀌지 않게 합니다.
fn link_worthy(name: &str) -> bool {
    name.ends_with(".gguf") || name.ends_with(".safetensors") || name.ends_with(".onnx")
}

/// 파일 하나를 가져옵니다. 받는 쪽에 같은 크기의 파일이 있으면 그대로 둡니다.
fn bring(from: &Path, to: &Path, name: &str, report: &mut ImportReport) -> Result<()> {
    let src = from.join(name);
    let dst = to.join(name);
    let size = fs::metadata(&src).with_context(|| format!("가져올 파일이 없습니다: {}", src.display()))?.len();
    if fs::metadata(&dst).is_ok_and(|m| m.len() == size) {
        report.kept.push(name.to_string());
        return Ok(());
    }
    if dst.exists() {
        fs::remove_file(&dst).with_context(|| format!("모델 파일을 바꿀 수 없습니다 (쓰는 중일 수 있습니다): {}", dst.display()))?;
    }
    if let Some(parent) = dst.parent() {
        fs::create_dir_all(parent)?;
    }
    if link_worthy(name) && fs::hard_link(&src, &dst).is_ok() {
        report.linked.push(name.to_string());
    } else {
        // 끝까지 쓴 뒤에 이름을 바꿔서, 복사하다 멈춘 파일이 모델로 열리지 않게 합니다.
        let tmp = to.join(format!("{name}.tmp"));
        fs::copy(&src, &tmp).with_context(|| format!("복사할 수 없습니다: {} → {}", src.display(), tmp.display()))?;
        fs::rename(&tmp, &dst)?;
        report.copied.push(name.to_string());
    }
    report.bytes += size;
    Ok(())
}

/// `source` 폴더(예: zoro-gravity 의 모델 폴더)에서 모델 하나를 가져옵니다. `active.json` 을 맨 마지막에 둡니다.
pub fn import_from(source: &Path, name: &str) -> Result<ImportReport> {
    let Some(entry) = entry(name) else { bail!("모르는 모델입니다: {name}") };
    import_dir(&source.join(entry.name), name)
}

/// 모델 하나를 그 모델의 가져올 곳([`source_dir`])에서 가져옵니다.
pub fn import_auto(name: &str) -> Result<ImportReport> {
    let Some(entry) = entry(name) else { bail!("모르는 모델입니다: {name}") };
    let env = match entry.source {
        Source::StanzaExport => STANZA_FROM_ENV,
        Source::LogisCenter(_) => EMBED_FROM_ENV,
        Source::ZoroGravity => IMPORT_FROM_ENV,
    };
    let Some(from) = source_dir(entry) else { bail!("{name} 을 가져올 곳을 모릅니다 ({env})") };
    import_dir(&from, name)
}

/// `from` (그 모델의 폴더)에서 모델 하나를 가져옵니다.
pub fn import_dir(from: &Path, name: &str) -> Result<ImportReport> {
    let Some(entry) = entry(name) else { bail!("모르는 모델입니다: {name}") };
    let from = from.to_path_buf();
    let to = model_dir(entry.name);
    if from == to {
        bail!("가져올 곳과 모델 폴더가 같습니다: {}", to.display());
    }
    if !installed(&from, entry) {
        bail!("가져올 곳에 {} 모델이 다 있지 않습니다: {}", entry.name, from.display());
    }
    fs::create_dir_all(&to).with_context(|| format!("모델 폴더를 만들 수 없습니다: {}", to.display()))?;
    let mut report = ImportReport::default();
    let (record, rest): (Vec<&ModelFile>, Vec<&ModelFile>) = entry.files.iter().partition(|f| f.name == "active.json");
    for f in rest {
        bring(&from, &to, f.name, &mut report)?;
    }
    for extra in entry.extras.iter().filter(|x| from.join(x).exists()) {
        bring(&from, &to, extra, &mut report)?;
    }
    for f in record {
        bring(&from, &to, f.name, &mut report)?;
    }
    Ok(report)
}

/// 여기 없고 가져올 곳에 있는 모델을 모두 가져옵니다. 가져온 모델 이름과 결과를 돌려줍니다.
pub fn import_missing() -> Result<Vec<(&'static str, ImportReport)>> {
    let mut out = Vec::new();
    for s in status().into_iter().filter(|s| s.importable) {
        out.push((s.entry.name, import_auto(s.entry.name)?));
    }
    Ok(out)
}

/// 모델 하나의 폴더를 지웁니다. 목록에 없는 이름은 지우지 않습니다. 하드 링크는 링크만 지워지고 zoro-gravity 쪽 파일은 남습니다.
pub fn delete_model(name: &str) -> Result<()> {
    let Some(entry) = entry(name) else { bail!("모르는 모델입니다: {name}") };
    let dir = model_dir(entry.name);
    if dir.exists() {
        fs::remove_dir_all(&dir).with_context(|| format!("모델 폴더를 지울 수 없습니다: {}", dir.display()))?;
    }
    Ok(())
}

/// 목록에 있는 모델을 모두 지웁니다.
pub fn delete_all() -> Result<()> {
    CATALOG.iter().try_for_each(|e| delete_model(e.name))
}

#[cfg(test)]
pub(crate) mod testing {
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, MutexGuard};

    static ENV: Mutex<()> = Mutex::new(());

    /// 임시 모델 폴더. 환경 변수를 바꾸므로 시험을 한 번에 하나씩 돌립니다.
    pub struct TempModels {
        pub dir: PathBuf,
        pub source: PathBuf,
        _guard: MutexGuard<'static, ()>,
    }

    impl TempModels {
        pub fn new(tag: &str) -> Self {
            let guard = ENV.lock().unwrap_or_else(|p| p.into_inner());
            let root = std::env::temp_dir().join(format!("blueprint-models-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            let dir = root.join("models");
            let source = root.join("zorogravity-models");
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::create_dir_all(&source).unwrap();
            std::env::set_var(super::MODELS_DIR_ENV, &dir);
            std::env::set_var(super::IMPORT_FROM_ENV, &source);
            // 이 PC 의 logis-center 모델 폴더 · 변환본을 보지 않게 (시험이 실제 폴더에 따라 달라지지 않게)
            std::env::set_var(super::EMBED_FROM_ENV, root.join("logis-embed"));
            std::env::remove_var(super::STANZA_FROM_ENV);
            Self { dir, source, _guard: guard }
        }

        pub fn path(&self) -> &Path {
            &self.dir
        }
    }

    impl Drop for TempModels {
        fn drop(&mut self) {
            std::env::remove_var(super::MODELS_DIR_ENV);
            std::env::remove_var(super::IMPORT_FROM_ENV);
            std::env::remove_var(super::EMBED_FROM_ENV);
            std::env::remove_var(super::STANZA_FROM_ENV);
            if let Some(root) = self.dir.parent() {
                let _ = std::fs::remove_dir_all(root);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testing::TempModels;
    use super::*;

    fn fill(dir: &Path, names: &[&str]) {
        fs::create_dir_all(dir).unwrap();
        for n in names {
            fs::write(dir.join(n), format!("data of {n}")).unwrap();
        }
    }

    fn names(e: &ModelEntry) -> Vec<&'static str> {
        e.files.iter().map(|f| f.name).collect()
    }

    #[test]
    fn models_live_in_the_os_app_data_folder() {
        let dir = app_dir();
        assert_eq!(dir.file_name().unwrap(), APP_DIR);
        if let Some(base) = dirs::data_local_dir() {
            assert!(dir.starts_with(base));
        }
        #[cfg(windows)]
        assert!(dir.to_string_lossy().to_ascii_lowercase().contains("appdata"));
        #[cfg(target_os = "macos")]
        assert!(dir.to_string_lossy().contains("Application Support"));
    }

    #[test]
    fn the_environment_can_move_the_models_folder() {
        let t = TempModels::new("env");
        assert_eq!(models_dir(), t.path());
        assert_eq!(model_dir("zoro"), t.path().join("zoro"));
        assert_eq!(import_source().as_deref(), Some(t.source.as_path()));
    }

    #[test]
    fn the_catalog_uses_the_same_repositories_as_zoro_gravity_and_ends_with_the_active_record() {
        assert_eq!(entry("zoro").unwrap().repo, Some("Qntology/zoro-1-gguf"));
        assert_eq!(entry("gocr").unwrap().repo, Some("Qntology/GravityOCR-KO-gguf"));
        assert_eq!(entry("siglip2").unwrap().repo, Some("Qntology/siglip2-so400m-patch16-naflex-gguf"));
        assert!(entry("없음").is_none());
        for e in CATALOG.iter().filter(|e| e.repo.is_some_and(|r| r.starts_with("Qntology/"))) {
            assert_eq!(e.files.last().unwrap().name, "active.json", "{}", e.name);
            assert!(e.total_bytes() > 1_000_000_000);
            assert!(e.required);
        }
        // 검색용 모델은 선택이고, 가져올 곳이 따로 있습니다
        assert!(!entry("embed").unwrap().required && !entry("stanza").unwrap().required);
        assert_eq!(entry("embed").unwrap().repo, Some("ibm-granite/granite-embedding-97m-multilingual-r2"));
        assert!(entry("stanza").unwrap().repo.is_none());
    }

    #[test]
    fn search_models_import_from_their_own_sources_with_subfolders() {
        let t = TempModels::new("search-models");
        let export = t.source.parent().unwrap().join("onnx_final");
        for n in names(entry("stanza").unwrap()) {
            let p = export.join(n);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(&p, format!("data of {n}")).unwrap();
        }
        std::env::set_var(STANZA_FROM_ENV, &export);
        let st = status();
        assert!(st.iter().find(|s| s.entry.name == "stanza").unwrap().importable);
        let r = import_auto("stanza").unwrap();
        assert!(r.linked.contains(&"ko/pos.onnx".to_string()), "ONNX 는 하드 링크 {r:?}");
        assert!(r.copied.contains(&"ko/vocab.json".to_string()));
        assert!(t.path().join("stanza/en/lemma_dict.json").exists());
        assert!(status().iter().find(|s| s.entry.name == "stanza").unwrap().bytes_on_disk > 0, "하위 폴더 크기도 셉니다");
        std::env::remove_var(STANZA_FROM_ENV);
        assert!(import_auto("stanza").is_err(), "가져올 곳을 모르면 거부");
    }

    #[test]
    fn a_model_counts_as_installed_only_with_every_file() {
        let t = TempModels::new("status");
        let zoro = t.path().join("zoro");
        fill(&zoro, &["zoro-q8.gguf", "tokenizer.json"]);
        assert!(!status()[0].installed);
        assert!(status()[0].bytes_on_disk > 0);
        fill(&zoro, &names(entry("zoro").unwrap()));
        assert!(status()[0].installed);
        assert!(!status()[0].importable, "이미 있는 모델은 가져올 대상이 아닙니다");
    }

    #[test]
    fn a_model_complete_in_zoro_gravity_is_importable_and_arrives_whole() {
        let t = TempModels::new("import");
        let siglip2 = entry("siglip2").unwrap();
        fill(&t.source.join("siglip2"), &names(siglip2));
        fill(&t.source.join("siglip2"), &["crop_probe.bin", "model.safetensors"]);
        fill(&t.source.join("gocr"), &["gocr-q8.gguf"]);
        let st = status();
        assert!(st[2].importable);
        assert!(!st[1].importable, "다 받지 못한 모델은 가져오지 않습니다");

        let report = import_from(&t.source, "siglip2").unwrap();
        let here = t.path().join("siglip2");
        assert!(installed(&here, siglip2));
        assert!(here.join("crop_probe.bin").exists(), "크롭 앵커도 함께 옵니다");
        assert!(!here.join("model.safetensors").exists(), "변환 전 원본은 가져오지 않습니다");
        assert_eq!(report.linked.len() + report.copied.len(), 5);
        assert!(report.copied.contains(&"active.json".to_string()), "작은 파일은 복사합니다");
        assert!(!report.linked.contains(&"crop_probe.bin".to_string()));
        assert_eq!(fs::read_to_string(here.join("siglip2-q8.gguf")).unwrap(), "data of siglip2-q8.gguf");

        // 다시 가져오면 같은 크기의 파일은 건드리지 않습니다.
        let again = import_from(&t.source, "siglip2").unwrap();
        assert_eq!(again.kept.len(), 5);
        assert_eq!(again.bytes, 0);
    }

    #[test]
    fn a_small_file_changed_here_does_not_change_the_source() {
        let t = TempModels::new("isolated");
        fill(&t.source.join("siglip2"), &names(entry("siglip2").unwrap()));
        fill(&t.source.join("siglip2"), &["crop_probe.bin"]);
        import_from(&t.source, "siglip2").unwrap();
        fs::write(t.path().join("siglip2/crop_probe.bin"), b"retrained for drawings").unwrap();
        assert_eq!(fs::read_to_string(t.source.join("siglip2/crop_probe.bin")).unwrap(), "data of crop_probe.bin");
    }

    #[test]
    fn import_missing_brings_only_what_is_missing() {
        let t = TempModels::new("missing");
        fill(&t.source.join("zoro"), &names(entry("zoro").unwrap()));
        fill(&t.source.join("gocr"), &names(entry("gocr").unwrap()));
        fill(&t.path().join("gocr"), &names(entry("gocr").unwrap()));
        let done = import_missing().unwrap();
        assert_eq!(done.iter().map(|(n, _)| *n).collect::<Vec<_>>(), vec!["zoro"]);
        assert!(status()[0].installed);
    }

    #[test]
    fn importing_refuses_unknown_names_and_incomplete_sources() {
        let t = TempModels::new("refuse");
        assert!(import_from(&t.source, "..").is_err());
        assert!(import_from(&t.source, "zoro").is_err());
        assert!(!t.path().join("zoro").exists(), "실패하면 폴더를 만들지 않습니다");
    }

    #[test]
    fn deleting_removes_only_known_model_folders() {
        let t = TempModels::new("delete");
        fill(&t.path().join("zoro"), &["a.gguf"]);
        fill(&t.path().join("gocr"), &["b.gguf"]);
        fill(&t.path().join("my-notes"), &["keep.txt"]);
        assert!(delete_model("..").is_err());
        assert!(delete_model("my-notes").is_err());
        delete_model("zoro").unwrap();
        assert!(!t.path().join("zoro").exists());
        assert!(t.path().join("gocr").exists());
        delete_model("zoro").unwrap();
        delete_all().unwrap();
        assert!(!t.path().join("gocr").exists());
        assert!(t.path().join("my-notes/keep.txt").exists());
    }

    #[test]
    fn deleting_an_imported_model_keeps_the_zoro_gravity_files() {
        let t = TempModels::new("keep-source");
        fill(&t.source.join("zoro"), &names(entry("zoro").unwrap()));
        import_from(&t.source, "zoro").unwrap();
        delete_model("zoro").unwrap();
        assert!(installed(&t.source.join("zoro"), entry("zoro").unwrap()));
    }
}
