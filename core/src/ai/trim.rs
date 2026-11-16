//! 어휘 잘라 내기 (vocabulary trimming): 출력 투영에서 남길 행 고르기.
//!
//! 잘린 행은 어디에도 남기지 않습니다 (예전 `output_rest.weight` · `head_rest.weight` 없음). 다음 토큰은 남긴 K 행 안에서만 고르고,
//! 확률도 그 K 행의 소프트맥스입니다. 그래서 남길 행을 고를 때 쓸 수 있는 글이 빠지지 않게 합니다:
//!
//! - **공통 코어**: 추가 · 특수 토큰, 한 바이트 토큰 256 개(바이트 수준 BPE 의 바이트 폴백), 혼자서는 UTF-8 이 아닌 바이트 조각 전부,
//!   기호 · 단위 · 그리스 문자 토큰, 그리고 ASCII 토큰 전부 (영어는 어느 문서에서나 전부 디코딩됩니다).
//! - **지원 언어의 문자 체계**: 한글(ko) · 라틴 확장(de · fr · es · it · pt · nl · cs) · 가나(ja) · 아랍 문자(ar) · 한자(ja · zh) 를 담은 토큰 **전부**.
//!   한자를 쓰지 않는 문서에 한자가 조금 섞였으면(한국어 문서의 한자 등, [`Langs::hanja`]) 도메인 글에 나온 것 + 병합 순 상위 [`CJK_COMMON`] 개.
//!   (처음 규칙은 일본어 · 중국어 문서도 상위 4,096 개만 남겼는데, Zoro-1 어휘(한자 토큰 53,433)에서 일본어 한자 設 · 場 · 階 가
//!   간체 设 · 场 · 阶 로 바뀌고 중국어 여러 글자 토큰이 빠져 글자가 사라졌습니다 — `docs/verification.md` 16.)
//! - **도메인 글**: 원장 · 시나리오 · 다국어 검색 낱말 · 화면 글이 쓰는 토큰 전부 (`pyramid::corpus`).
//! - 남긴 수는 GPU 타일에 맞게 64 의 배수로 채웁니다 (다음 한자, 그다음 ASCII 를 번호 순으로).
//!
//! 행 수에 예산을 두는 규칙은 없습니다. 예산을 두었던 `compact` 는 남은 자리만 영어 낱말에 주어 "영어 전부 디코딩" 을 어겼고
//! 라틴 언어 낱말이 잘려 글이 바뀌었습니다 (`docs/verification.md` 16).
//!
//! 문서의 언어를 알면 ([`Langs`]) 그 언어의 문자 체계만 남깁니다 (예: `ko,en` → 공통 코어 + 한글). 기본은 문서 언어 자동(`auto`)입니다:
//! 텍스트 맥락은 쓰기 전에 지시문 · 입력 글로, 이미지 맥락은 첫 조각(장)을 12 개 언어의 행으로 읽은 글로 정합니다
//! (라틴 밖 글자가 있을 때만, [`Langs::decisive`]) ([`document_langs`] = logis-center `detect_document_language` 의 현지 언어 + 영어, 그리고 글에 나온 문자 체계).

use std::collections::{BTreeSet, HashMap};
use std::sync::Mutex;

use anyhow::{bail, Result};
use rayon::prelude::*;

use super::head::HeadMap;
use super::tokenizer::ModelTokenizer;
use super::weights::WeightFile;

/// 남길 한자 토큰 수 (도메인 글에 나온 것 밖으로, 병합 순 상위)
pub const CJK_COMMON: usize = 4096;
/// 남긴 행 수를 맞추는 배수
pub const ALIGN: usize = 64;
/// 고르는 규칙 (`full` · `off`)
pub const TRIM_ENV: &str = "BLUEPRINT_TRIM";
/// 문서 언어: `auto`(기본, 문서 글로 정함) · `all`(12 개 언어 고정) · `ko,en` 처럼 쉼표로 (`docs/verification.md` 16)
pub const LANGS_ENV: &str = "BLUEPRINT_TRIM_LANGS";

/// 지원하는 12 개 언어
pub const LANGS: [&str; 12] = ["ko", "en", "ja", "zh", "de", "fr", "es", "it", "pt", "nl", "cs", "ar"];

/// 글자의 문자 체계
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Script {
    Ascii,
    Hangul,
    Kana,
    Cjk,
    Arabic,
    Latin,
    Greek,
    Symbol,
    Other,
}

pub fn script(c: char) -> Script {
    let o = c as u32;
    match o {
        0..=0x7F => Script::Ascii,
        0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F | 0xA960..=0xA97F | 0xD7B0..=0xD7FF => Script::Hangul,
        0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF65..=0xFF9F => Script::Kana,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF => Script::Cjk,
        0x0600..=0x06FF | 0x0750..=0x077F | 0x08A0..=0x08FF | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
        0x00C0..=0x024F | 0x1E00..=0x1EFF | 0x0300..=0x036F => Script::Latin,
        0x0370..=0x03FF => Script::Greek,
        0x00A0..=0x00BF | 0x2000..=0x2BFF | 0x3000..=0x303F | 0x3200..=0x33FF | 0xFF00..=0xFFEF => Script::Symbol,
        _ => Script::Other,
    }
}

/// 토큰 한 개의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// 추가 · 특수 토큰
    Special,
    /// 한 바이트 (바이트 폴백)
    Byte,
    /// 혼자서는 UTF-8 이 아닌 바이트 조각
    Partial,
    /// ASCII 만
    Ascii,
    /// ASCII · 기호 · 단위 · 그리스 문자 (ASCII 만은 아님)
    Neutral,
    /// 위 중립 문자에 더해 이 문자 체계들을 씀 (한글 · 가나 · 한자 · 아랍 · 라틴 확장의 조합)
    Script { hangul: bool, kana: bool, cjk: bool, arabic: bool, latin: bool },
    /// 지원하지 않는 문자 체계를 씀 (키릴 · 태국 · 데바나가리 …)
    Other,
}

pub fn kind_of(piece: &[u8], added: bool) -> Kind {
    if added {
        return Kind::Special;
    }
    if piece.len() == 1 {
        return Kind::Byte;
    }
    let Ok(s) = std::str::from_utf8(piece) else { return Kind::Partial };
    let (mut hangul, mut kana, mut cjk, mut arabic, mut latin, mut ascii_only) = (false, false, false, false, false, true);
    for c in s.chars() {
        match script(c) {
            Script::Ascii => {}
            Script::Greek | Script::Symbol => ascii_only = false,
            Script::Hangul => hangul = true,
            Script::Kana => kana = true,
            Script::Cjk => cjk = true,
            Script::Arabic => arabic = true,
            Script::Latin => latin = true,
            Script::Other => return Kind::Other,
        }
    }
    if hangul || kana || cjk || arabic || latin {
        Kind::Script { hangul, kana, cjk, arabic, latin }
    } else if ascii_only {
        Kind::Ascii
    } else {
        Kind::Neutral
    }
}

/// 남길 문자 체계 (문서 언어에서)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Langs {
    pub hangul: bool,
    pub kana: bool,
    /// 한자를 쓰는 언어(ja · zh): 한자 토큰 전부
    pub cjk: bool,
    pub arabic: bool,
    pub latin: bool,
    /// 한자를 쓰지 않는 문서에 섞인 한자: 도메인 글 + 상위 [`CJK_COMMON`] 개
    pub hanja: bool,
}

impl Langs {
    fn none() -> Self {
        Self { hangul: false, kana: false, cjk: false, arabic: false, latin: false, hanja: false }
    }

    /// 12 개 언어 전부
    pub fn all() -> Self {
        Self { hangul: true, kana: true, cjk: true, arabic: true, latin: true, hanja: false }
    }

    /// 언어 코드(`ko`, `en`, `ja`, `zh` · `zh-hans`, `de`, `fr`, `es`, `it`, `pt`, `nl`, `cs`, `ar`)에서. 모르는 코드는 거부합니다.
    pub fn from_codes<S: AsRef<str>>(codes: &[S]) -> Result<Self> {
        let mut l = Self::none();
        for code in codes {
            match code.as_ref().trim().to_ascii_lowercase().as_str() {
                "ko" => l.hangul = true,
                "en" => {}
                "ja" => {
                    l.kana = true;
                    l.cjk = true;
                }
                "zh" | "zh-hans" | "zh-cn" => l.cjk = true,
                "de" | "fr" | "es" | "it" | "pt" | "nl" | "cs" => l.latin = true,
                "ar" => l.arabic = true,
                other => bail!("모르는 언어 코드 {other:?} (지원: {})", LANGS.join(", ")),
            }
        }
        Ok(l)
    }

    /// `all` 이거나 쉼표로 나눈 코드
    pub fn parse(spec: &str) -> Result<Self> {
        let spec = spec.trim();
        if spec.is_empty() || spec.eq_ignore_ascii_case("all") {
            return Ok(Self::all());
        }
        Self::from_codes(&spec.split(',').collect::<Vec<_>>())
    }

    /// `BLUEPRINT_TRIM_LANGS` (없거나 `auto` 면 12 개 언어 전부로 시작해 문서 글로 정함). 둘째 값은 `auto` 인가.
    /// 기본이 `auto` 인 까닭: 12 개 언어 고정과 정확도가 같고(조각 72 · 시나리오 · 도면 · 델타 모두 기준과 같음) Zoro-1 이 5 % 빠르며
    /// 출력 투영 VRAM 이 언어에 따라 23 ~ 46 % 작습니다 (`docs/verification.md` 16). 쓸 글의 언어가 지시문 · 입력에 없는 작업
    /// (예: 한국어 입력을 일본어로 번역)은 `all` 로 둡니다.
    pub fn from_env() -> Result<(Self, bool)> {
        match std::env::var(LANGS_ENV) {
            Ok(v) if v.trim().is_empty() || v.trim().eq_ignore_ascii_case("auto") => Ok((Self::all(), true)),
            Ok(v) => Ok((Self::parse(&v)?, false)),
            Err(_) => Ok((Self::all(), true)),
        }
    }

    /// 문서 글에서: [`document_langs`] 의 현지 언어 + 영어, 그리고 글에 실제로 나온 문자 체계([`Self::present`]).
    /// 지원하지 않는 언어가 나오면 12 개 언어 전부입니다 (그 글자를 자를 수 없음). `document_langs` 는 라틴 언어를
    /// fr · de · es · it · pt · nl · cs 밖이면 `en` 으로 돌려주므로(폴란드어 ł · ś 등) 판정만 따르면 문서에 있는 글자를 잘라 냅니다.
    pub fn of_document(text: &str) -> Self {
        Self::from_codes(&document_langs(text)).unwrap_or_else(|_| Self::all()).union(Self::present(text))
    }

    /// 이미지 문서의 첫 판독으로 문서 언어를 정할 수 있으면 [`Self::of_document`]. 라틴 밖 문자 체계(한글 · 가나 · 한자 · 아랍 문자 등)의
    /// 글자가 3 자 이상 나와야 정합니다 (logis-center `vocab_trim::observe_read` 와 같은 규칙). 라틴 문자뿐인 판독으로는 정하지 않습니다:
    /// 한국어 도면도 첫 조각(KKS 띠 · 도면번호 · 영문 주기)은 라틴 문자뿐인 일이 흔해서, 영어로 자르면 뒤 조각의 한글을 쓸 수 없게 됩니다.
    pub fn decisive(text: &str) -> Option<Self> {
        let n = text.chars().filter(|&c| c.is_alphabetic() && !c.is_ascii() && !matches!(c as u32, 0x00C0..=0x024F | 0x1E00..=0x1EFF)).count();
        (n >= 3).then(|| Self::of_document(text))
    }

    /// 글에 실제로 나온 문자 체계 (한글 · 가나 · 한자 · 아랍 문자 · 라틴 확장). 한자는 섞인 한자([`Self::hanja`])로 봅니다:
    /// 한자를 쓰는 언어인지는 판정 언어(ja · zh)가 정합니다.
    pub fn present(text: &str) -> Self {
        let mut l = Self::none();
        for c in text.chars() {
            match script(c) {
                Script::Hangul => l.hangul = true,
                Script::Kana => l.kana = true,
                Script::Cjk => l.hanja = true,
                Script::Arabic => l.arabic = true,
                Script::Latin => l.latin = true,
                _ => {}
            }
        }
        l
    }

    pub fn union(self, o: Self) -> Self {
        let cjk = self.cjk || o.cjk;
        Self {
            hangul: self.hangul || o.hangul,
            kana: self.kana || o.kana,
            cjk,
            arabic: self.arabic || o.arabic,
            latin: self.latin || o.latin,
            hanja: !cjk && (self.hanja || o.hanja),
        }
    }

    pub fn is_all(&self) -> bool {
        *self == Self::all()
    }

    /// 이 종류의 토큰을 문자 체계로 남기는가 (섞인 한자만이면 한자만 쓰는 토큰은 따로: 도메인 · 상위 [`CJK_COMMON`])
    fn keeps_script(&self, k: Kind) -> bool {
        match k {
            Kind::Script { hangul, kana, cjk, arabic, latin } => {
                let allowed = (!hangul || self.hangul)
                    && (!kana || self.kana)
                    && (!cjk || self.cjk || self.hanja)
                    && (!arabic || self.arabic)
                    && (!latin || self.latin);
                // 한자를 쓰는 언어면 한자만 쓰는 토큰도 전부. 섞인 한자면 다른 문자와 섞인 토큰만 여기서 (한자만 쓰는 토큰은 상위 한자 단계)
                allowed && (hangul || kana || arabic || latin || (cjk && self.cjk))
            }
            _ => false,
        }
    }

    /// 짧은 이름 (로그 · 지문)
    pub fn label(&self) -> String {
        if self.is_all() {
            return "all".into();
        }
        let mut s = Vec::new();
        for (on, name) in [(self.hangul, "hangul"), (self.kana, "kana"), (self.cjk, "cjk"), (self.arabic, "arabic"), (self.latin, "latin"), (self.hanja, "hanja")] {
            if on {
                s.push(name);
            }
        }
        if s.is_empty() { "ascii".into() } else { s.join("+") }
    }
}

/// 고르는 규칙
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrimRule {
    /// 잘라 내지 않음 (전체 어휘)
    Off,
    /// ASCII 토큰 전부를 남김
    Full,
}

impl TrimRule {
    /// `full` · `off`
    pub fn parse(spec: &str) -> Result<Self> {
        Ok(match spec.trim().to_ascii_lowercase().as_str() {
            "" | "full" => Self::Full,
            "off" | "none" | "0" => Self::Off,
            _ => bail!("{TRIM_ENV}={spec:?} 를 읽지 못했습니다 (full · off)"),
        })
    }

    /// `BLUEPRINT_TRIM` (없으면 [`TrimRule::Full`])
    pub fn from_env() -> Result<Self> {
        match std::env::var(TRIM_ENV) {
            Ok(v) => Self::parse(&v),
            Err(_) => Ok(Self::Full),
        }
    }

    pub fn label(&self) -> String {
        match self {
            Self::Off => "off".into(),
            Self::Full => "full".into(),
        }
    }
}

/// 남긴 행
#[derive(Debug, Clone, PartialEq)]
pub struct Trimmed {
    /// 남긴 행의 전체 어휘 번호 (오름차순). `ids[행]` 이 그 행의 토큰입니다.
    pub ids: Vec<u32>,
    /// 단계마다 더한 수
    pub tiers: Vec<(&'static str, usize)>,
}

impl Trimmed {
    pub fn summary(&self) -> String {
        let t: Vec<String> = self.tiers.iter().map(|(n, k)| format!("{n} {k}")).collect();
        format!("{} 행 ({})", self.ids.len(), t.join(" · "))
    }
}

/// 어휘 `vocab` 에서 남길 행을 고릅니다. `universe` 가 있으면 그 번호들 안에서만 고릅니다 (이미 잘라 둔 파일의 행에서 더 줄일 때).
pub fn select(vocab: usize, tokenizer: &ModelTokenizer, corpus: &[String], rule: TrimRule, langs: Langs, universe: Option<&[u32]>) -> Result<Trimmed> {
    let (kinds, domain) = kinds_and_domain(vocab, tokenizer, corpus)?;
    select_from(&kinds, &domain, rule, langs, universe)
}

/// 토큰 종류와 도메인 글의 토큰 번호. 언어와 상관없으므로 [`HeadSource`] 는 한 번만 셉니다 (언어 조합마다 다시 세면
/// 어휘 전체의 종류 판정과 도메인 글 토큰화를 매번 되풀이합니다: logis-center 2B 어휘에서 번에 5 ~ 6 s).
pub fn kinds_and_domain(vocab: usize, tokenizer: &ModelTokenizer, corpus: &[String]) -> Result<(Vec<Kind>, BTreeSet<u32>)> {
    let kinds: Vec<Kind> = (0..vocab as u32)
        .into_par_iter()
        .map(|i| kind_of(&tokenizer.token_bytes(i).unwrap_or_default(), tokenizer.is_added(i)))
        .collect();
    let mut domain: BTreeSet<u32> = BTreeSet::new();
    for text in corpus {
        domain.extend(tokenizer.encode(text)?.ids.into_iter().filter(|&i| (i as usize) < vocab));
    }
    Ok((kinds, domain))
}

/// [`select`] 의 본체 (토큰 종류 · 도메인 번호가 주어졌을 때)
pub fn select_from(kinds: &[Kind], domain: &BTreeSet<u32>, rule: TrimRule, langs: Langs, universe: Option<&[u32]>) -> Result<Trimmed> {
    let vocab = kinds.len();
    let allowed: Option<BTreeSet<u32>> = universe.map(|u| u.iter().copied().collect());
    let ok = |i: u32| allowed.as_ref().is_none_or(|a| a.contains(&i));
    if rule == TrimRule::Off {
        let ids: Vec<u32> = (0..vocab as u32).filter(|&i| ok(i)).collect();
        let n = ids.len();
        return Ok(Trimmed { ids, tiers: vec![("off", n)] });
    }
    let mut keep: BTreeSet<u32> = BTreeSet::new();
    let mut tiers: Vec<(&'static str, usize)> = Vec::new();
    let mut add = |name: &'static str, ids: &mut dyn Iterator<Item = u32>, keep: &mut BTreeSet<u32>, cap: Option<usize>| {
        let before = keep.len();
        for i in ids {
            if cap.is_some_and(|c| keep.len() >= c) {
                break;
            }
            if ok(i) {
                keep.insert(i);
            }
        }
        tiers.push((name, keep.len() - before));
    };
    let all = || 0..vocab as u32;
    // 1. 공통 코어
    add("core", &mut all().filter(|&i| matches!(kinds[i as usize], Kind::Special | Kind::Byte | Kind::Partial | Kind::Neutral)), &mut keep, None);
    // 2. 지원 언어의 문자 체계 전부
    add("scripts", &mut all().filter(|&i| langs.keeps_script(kinds[i as usize])), &mut keep, None);
    // 3. 도메인 글
    add("domain", &mut domain.iter().copied(), &mut keep, None);
    // 4. ASCII 전부
    add("ascii", &mut all().filter(|&i| kinds[i as usize] == Kind::Ascii), &mut keep, None);
    // 5. 섞인 한자: 도메인 밖의 상위 한자 (병합 순). 한자를 쓰는 언어면 2 에서 이미 전부
    if langs.hanja && !langs.cjk {
        let common: Vec<u32> = all()
            .filter(|&i| matches!(kinds[i as usize], Kind::Script { cjk: true, hangul: false, kana: false, arabic: false, latin: false }) && ok(i) && !keep.contains(&i))
            .take(CJK_COMMON)
            .collect();
        add("cjk", &mut common.into_iter(), &mut keep, None);
    }
    // 6. 64 의 배수로: 다음 한자, 그다음 ASCII, 그다음 나머지
    let target = keep.len().next_multiple_of(ALIGN);
    let pad_cjk = langs.cjk || langs.hanja;
    add(
        "pad",
        &mut all()
            .filter(|&i| pad_cjk && matches!(kinds[i as usize], Kind::Script { cjk: true, .. }))
            .chain(all().filter(|&i| kinds[i as usize] == Kind::Ascii))
            .chain(all()),
        &mut keep,
        Some(target),
    );
    let ids: Vec<u32> = keep.into_iter().collect();
    if ids.is_empty() {
        bail!("남길 행이 없습니다");
    }
    Ok(Trimmed { ids, tiers })
}

/// 🌟 [DOC LANG] 문서 글의 언어 코드: 현지 언어와 영어 두 개 (현지 언어가 영어면 하나). logis-center 의
/// `utils::lang_utils::detect_document_language` 와 같은 규칙입니다 — 유니코드 블록이 분명한 문자(한글 → ko, 가나 → ja, 아랍 → ar,
/// 키릴 · 태국 · 데바나가리 … → 그 언어, 한자만 → zh)를 먼저 보고, 라틴 문자뿐이면 whatlang 으로 fr · de · es · it · pt · nl · cs 를 가르고
/// 나머지는 en 입니다. 기술 문서는 현지 언어와 영어(설비 코드 · 단위 · 표 머리)가 섞이므로 영어를 늘 함께 둡니다.
pub fn document_langs(text: &str) -> Vec<&'static str> {
    let (mut ko, mut ja, mut zh, mut ru, mut ar, mut th, mut hi, mut el, mut he, mut vi, mut latin) = (false, false, false, false, false, false, false, false, false, false, false);
    for c in text.chars() {
        let u = c as u32;
        match u {
            0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F => ko = true,
            0x3040..=0x30FF => ja = true,
            0x4E00..=0x9FFF => zh = true,
            0x0400..=0x04FF => ru = true,
            0x0600..=0x06FF => ar = true,
            0x0E00..=0x0E7F => th = true,
            0x0900..=0x09FF => hi = true,
            0x0370..=0x03FF => el = true,
            0x0590..=0x05FF => he = true,
            0x1EA0..=0x1EF9 => vi = true,
            0x41..=0x5A | 0x61..=0x7A | 0xC0..=0x24F => latin = true,
            _ => {}
        }
    }
    let local = if ko {
        "ko"
    } else if ja {
        "ja"
    } else if th {
        "th"
    } else if ru {
        "ru"
    } else if ar {
        "ar"
    } else if hi {
        "hi"
    } else if el {
        "el"
    } else if he {
        "he"
    } else if vi {
        "vi"
    } else if zh {
        "zh"
    } else if latin {
        match whatlang::detect(text).map(|i| i.lang()) {
            Some(whatlang::Lang::Fra) => "fr",
            Some(whatlang::Lang::Deu) => "de",
            Some(whatlang::Lang::Spa) => "es",
            Some(whatlang::Lang::Ita) => "it",
            Some(whatlang::Lang::Por) => "pt",
            Some(whatlang::Lang::Nld) => "nl",
            Some(whatlang::Lang::Ces) => "cs",
            _ => "en",
        }
    } else {
        "en"
    };
    if local == "en" { vec!["en"] } else { vec![local, "en"] }
}

/// 실행 중 출력 투영 잘라 내기의 설정: 규칙 · 문서 언어 · 기준 행(파일이 이미 잘라 둔 행). 언어별로 고른 행을 기억합니다.
pub struct HeadSource {
    pub model: &'static str,
    /// 행 보기의 원본 텐서 (gocr `output.weight`, zoro 묶인 `token_embd.weight`)
    pub base_name: &'static str,
    /// 파일이 잘라 둔 출력 투영 (gocr 피라미드 파일). 없으면 원본 텐서의 행 = 전체 어휘
    pub base: Option<HeadMap>,
    pub rule: TrimRule,
    /// 처음 쓸 언어
    pub langs: Langs,
    /// `BLUEPRINT_TRIM_LANGS=auto`: 문서 글을 읽은 뒤 그 언어로 다시 고름
    pub auto: bool,
    corpus: Vec<String>,
    chosen: Mutex<HashMap<String, Vec<u32>>>,
    /// 토큰 종류 · 도메인 글 토큰 (어휘마다 한 번, [`kinds_and_domain`])
    base_kinds: Mutex<Option<std::sync::Arc<(Vec<Kind>, BTreeSet<u32>)>>>,
}

impl HeadSource {
    /// `BLUEPRINT_TRIM` (기본 `full`) · `BLUEPRINT_TRIM_LANGS` (기본 문서 언어 자동) 로
    pub fn from_env(model: &'static str, base_name: &'static str, base: Option<HeadMap>) -> Result<Self> {
        let rule = TrimRule::from_env()?;
        let (langs, auto) = Langs::from_env()?;
        Ok(Self::new(model, base_name, base, rule, langs, auto))
    }

    pub fn new(model: &'static str, base_name: &'static str, base: Option<HeadMap>, rule: TrimRule, langs: Langs, auto: bool) -> Self {
        let corpus = if rule == TrimRule::Off { Vec::new() } else { super::pyramid::corpus(true) };
        Self { model, base_name, base, rule, langs, auto, corpus, chosen: Mutex::new(HashMap::new()), base_kinds: Mutex::new(None) }
    }

    fn kinds(&self, vocab: usize, tokenizer: &ModelTokenizer) -> Result<std::sync::Arc<(Vec<Kind>, BTreeSet<u32>)>> {
        let mut slot = self.base_kinds.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(b) = slot.as_ref().filter(|b| b.0.len() == vocab) {
            return Ok(std::sync::Arc::clone(b));
        }
        let b = std::sync::Arc::new(kinds_and_domain(vocab, tokenizer, &self.corpus)?);
        *slot = Some(std::sync::Arc::clone(&b));
        Ok(b)
    }

    /// `langs` 로 남길 행의 출력 투영. 규칙이 `off` 면 파일 그대로(잘라 둔 파일이면 그 행), 기준 행과 같으면 기준 행,
    /// 아니면 기준 텐서의 행 보기 ([`HeadMap::view`]) 입니다.
    pub fn head_for(&self, file: &WeightFile, tokenizer: &ModelTokenizer, vocab: usize, langs: Langs) -> Result<Option<HeadMap>> {
        if self.rule == TrimRule::Off || (self.rule == TrimRule::Full && langs.is_all() && self.base.is_some()) {
            // 앞 문서가 만든 행 보기를 지웁니다: 남겨 두면 출력 묶음이 보기를 올리고 디코더는 저장된 행(`output.weight`)을 찾습니다
            // (문서 언어 자동에서 한국어 도면 → 다음 도면의 12 개 언어 첫 판독이 이렇게 실패했음).
            file.clear_row_view(super::residency::HEAD_VIEW);
            return Ok(self.base.clone());
        }
        let key = format!("{}:{}", self.rule.label(), langs.label());
        let ids = {
            let mut chosen = self.chosen.lock().unwrap_or_else(|p| p.into_inner());
            match chosen.get(&key) {
                Some(ids) => ids.clone(),
                None => {
                    let t0 = std::time::Instant::now();
                    let universe = self.base.as_ref().map(|b| b.kept.as_slice());
                    let base = self.kinds(vocab, tokenizer)?;
                    let picked = select_from(&base.0, &base.1, self.rule, langs, universe)?;
                    println!(
                        "[TRIM] {} {} · {}: {vocab} → {} ({:.2} s)",
                        self.model,
                        self.rule.label(),
                        langs.label(),
                        picked.summary(),
                        t0.elapsed().as_secs_f64()
                    );
                    chosen.insert(key, picked.ids.clone());
                    picked.ids
                }
            }
        };
        if let Some(b) = &self.base {
            if b.kept == ids {
                file.clear_row_view(super::residency::HEAD_VIEW);
                return Ok(Some(b.clone()));
            }
        }
        Ok(Some(HeadMap::view(file, self.base_name, self.base.as_ref().map(|b| b.kept.as_slice()), ids, vocab)?))
    }
}

/// 글들의 토큰 가운데 남긴 행 밖의 것 (총 토큰, 잘린 토큰)
pub fn coverage(tokenizer: &ModelTokenizer, ids: &[u32], texts: &[String]) -> Result<(usize, usize)> {
    let kept: std::collections::HashSet<u32> = ids.iter().copied().collect();
    let (mut total, mut missing) = (0usize, 0usize);
    for t in texts {
        for id in tokenizer.encode(t)?.ids {
            total += 1;
            missing += usize::from(!kept.contains(&id));
        }
    }
    Ok((total, missing))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_follow_the_scripts() {
        assert_eq!(kind_of(b"a", false), Kind::Byte);
        assert_eq!(kind_of(&[0xEC, 0x9E], false), Kind::Partial);
        assert_eq!(kind_of(" PRESSURE".as_bytes(), false), Kind::Ascii);
        assert_eq!(kind_of("°C".as_bytes(), false), Kind::Neutral);
        assert_eq!(kind_of("압력".as_bytes(), false), Kind::Script { hangul: true, kana: false, cjk: false, arabic: false, latin: false });
        assert_eq!(kind_of("日本の".as_bytes(), false), Kind::Script { hangul: false, kana: true, cjk: true, arabic: false, latin: false });
        assert_eq!(kind_of("Größe".as_bytes(), false), Kind::Script { hangul: false, kana: false, cjk: false, arabic: false, latin: true });
        assert_eq!(kind_of("давление".as_bytes(), false), Kind::Other);
        assert_eq!(kind_of("<|im_end|>".as_bytes(), true), Kind::Special);
    }

    #[test]
    fn document_languages_follow_logis_center() {
        assert_eq!(document_langs("급수 펌프 FW-101 압력"), vec!["ko", "en"]);
        assert_eq!(document_langs("給水ポンプの圧力"), vec!["ja", "en"]);
        assert_eq!(document_langs("给水泵压力"), vec!["zh", "en"]);
        assert_eq!(document_langs("ضغط مضخة المياه"), vec!["ar", "en"]);
        assert_eq!(document_langs("Druck der Speisewasserpumpe wird überwacht und geregelt"), vec!["de", "en"]);
        assert_eq!(document_langs("FEEDWATER PUMP PRESSURE"), vec!["en"]);
        assert_eq!(Langs::of_document("Давление насоса"), Langs::all(), "지원하지 않는 언어는 자르지 않음");
        assert_eq!(Langs::of_document("급수 펌프"), Langs::parse("ko,en").unwrap());
        // 판정은 en(폴란드어 → en)이어도 글의 라틴 확장 글자는 남김
        let pl = Langs::of_document("Pompa wody zasilającej, ciśnienie tłoczenia");
        assert!(pl.latin && !pl.hangul, "{pl:?}");
    }

    #[test]
    fn image_reads_decide_on_non_latin_letters_only() {
        // 한국어 도면의 첫 조각이 KKS 띠 · 영문 주기면 정하지 않음 (12 개 언어의 행 그대로)
        assert_eq!(Langs::decisive("10LAB10 AA001 FEEDWATER PUMP DISCHARGE"), None);
        assert_eq!(Langs::decisive("Druck der Speisewasserpumpe wird überwacht"), None, "라틴 확장 글자도 라틴");
        assert_eq!(Langs::decisive("급수 FW-101"), None, "두 자로는 정하지 않음");
        assert_eq!(Langs::decisive("급수 펌프 FW-101"), Some(Langs::parse("ko,en").unwrap()));
        assert_eq!(Langs::decisive("給水ポンプ"), Some(Langs::parse("ja").unwrap()));
        assert_eq!(Langs::decisive("ضغط المضخة"), Some(Langs::parse("ar").unwrap()));
    }

    #[test]
    fn languages_pick_their_scripts() {
        let ko_en = Langs::from_codes(&["ko", "en"]).unwrap();
        assert!(ko_en.hangul && !ko_en.kana && !ko_en.latin && !ko_en.cjk && !ko_en.hanja);
        // 한국어 문서에 섞인 한자는 상위 한자만, 한자를 쓰는 언어로 판정되면 한자 전부
        let ko_hanja = Langs::of_document("급수 펌프(給水) 압력");
        assert!(ko_hanja.hangul && ko_hanja.hanja && !ko_hanja.cjk, "{ko_hanja:?}");
        let zh = Langs::of_document("给水泵压力");
        assert!(zh.cjk && !zh.hanja, "{zh:?}");
        let ja = Langs::parse("ja").unwrap();
        assert!(ja.kana && ja.cjk && !ja.hangul);
        assert!(Langs::parse("all").unwrap().is_all());
        assert!(Langs::parse("xx").is_err());
        assert_eq!(TrimRule::parse("full").unwrap(), TrimRule::Full);
        assert_eq!(TrimRule::parse("off").unwrap(), TrimRule::Off);
        assert!(TrimRule::parse("compact").is_err(), "예산을 둔 규칙은 없습니다 (영어 전부 디코딩)");
        assert!(TrimRule::parse("half").is_err());
    }

    /// 작은 어휘: 0 특수 · 1 바이트 · 2 조각 · 3 ' PRESS' · 4 ' the' · 5 '압력' · 6 '日本' · 7 'давление' · 8 'Größe' · 9 'العربية' · 10 'の'
    fn tiny() -> Vec<Kind> {
        let pieces: Vec<Vec<u8>> = vec![
            b"<s>".to_vec(),
            b"a".to_vec(),
            vec![0xEC, 0x9E],
            b" PRESS".to_vec(),
            b" the".to_vec(),
            "압력".as_bytes().to_vec(),
            "日本".as_bytes().to_vec(),
            "давление".as_bytes().to_vec(),
            "Größe".as_bytes().to_vec(),
            "العربية".as_bytes().to_vec(),
            "の".as_bytes().to_vec(),
        ];
        pieces.iter().enumerate().map(|(i, p)| kind_of(p, i == 0)).collect()
    }

    #[test]
    fn full_keeps_every_ascii_token_and_the_supported_scripts_and_drops_the_rest() {
        let kinds = tiny();
        let t = select_from(&kinds, &BTreeSet::new(), TrimRule::Full, Langs::all(), None).unwrap();
        // 64 로 채우므로 어휘 11 이면 모두 들어갑니다. 채우기 전 단계를 봅니다.
        let before_pad: usize = t.tiers.iter().filter(|(n, _)| *n != "pad").map(|(_, k)| k).sum();
        assert_eq!(before_pad, 10, "키릴 'давление' 만 빠짐: {:?}", t.tiers);
        let ko = select_from(&kinds, &BTreeSet::new(), TrimRule::Full, Langs::parse("ko,en").unwrap(), None).unwrap();
        let before_pad: usize = ko.tiers.iter().filter(|(n, _)| *n != "pad").map(|(_, k)| k).sum();
        assert_eq!(before_pad, 6, "ko,en 은 코어 · ASCII · 한글만: {:?}", ko.tiers);
        // 한자를 쓰는 언어(zh)면 한자만 쓰는 토큰 '日本' 은 문자 체계 단계에서 남김
        let zh = select_from(&kinds, &BTreeSet::new(), TrimRule::Full, Langs::parse("zh").unwrap(), None).unwrap();
        assert!(zh.tiers.iter().any(|(n, k)| *n == "scripts" && *k == 1), "{:?}", zh.tiers);
        assert!(!zh.tiers.iter().any(|(n, _)| *n == "cjk"), "{:?}", zh.tiers);
    }

    #[test]
    fn the_kept_count_is_a_multiple_of_64_and_a_universe_bounds_the_choice() {
        let n = 200;
        let kinds: Vec<Kind> = (0..n).map(|i| if i % 2 == 0 { Kind::Ascii } else { Kind::Other }).collect();
        let t = select_from(&kinds, &BTreeSet::new(), TrimRule::Full, Langs::all(), None).unwrap();
        assert_eq!(t.ids.len() % ALIGN, 0);
        assert!(t.ids.windows(2).all(|w| w[0] < w[1]));
        let universe: Vec<u32> = (0..50).collect();
        let u = select_from(&kinds, &BTreeSet::new(), TrimRule::Full, Langs::all(), Some(&universe)).unwrap();
        assert!(u.ids.iter().all(|&i| i < 50));
    }
}
