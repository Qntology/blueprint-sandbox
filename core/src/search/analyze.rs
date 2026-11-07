//! 질의 분석: 공백 낱말마다 언어 · 품사 · 원형 · 줄기 · 기능어 표시.
//!
//! 낱말은 공백으로만 나눕니다 (logis-center 실측: Stanza 토크나이저가 '이벤트로' 를 '이+벤트로' 로 쪼개 NMS 좌표가 깨짐).
//! Stanza 가 있으면 공백 낱말을 한 문장으로 넣어 품사 · 원형을 붙입니다.
//! - 한국어: 원형이 형태소를 `+` 로 이은 꼴이고 xpos 가 같은 수의 세종 품사라, 끝의 조사(j…) · 어미(e…) · 접미(xs…)를 떼어
//!   줄기를 만듭니다 (`급수펌프의` nq+jcm → `급수펌프`, `고착된` ncpa+xsv+etm → `고착`, `높은` paa+etm → `높`).
//! - 영어: 원형(`valves` → `valve`, `rising` → `rise`)을 줄기로 씁니다.
//! - 기능어(관사 · 전치사 · 대명사 · 접속사 · 조동사 · 명령 동사)는 글자 경로 · NMS 조각에서 뺍니다. 부정어는 기능어여도 따로 봅니다.
//!
//! Stanza 가 없으면 영어는 규칙 단수화와 낱말 목록, 한국어는 끝 조사 하나를 뗀 꼴을 원문과 함께 씁니다 (원문을 버리지 않으므로 잘못 떼도 덜 잃음).

use serde::Serialize;

use super::lexicon;
use crate::nlp::{StanzaLang, Tagged};

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct QueryToken {
    /// 가장자리 문장부호를 뗀 원문 (소문자)
    pub text: String,
    pub hangul: bool,
    pub upos: Option<String>,
    pub xpos: Option<String>,
    pub lemma: Option<String>,
    /// 비교에 쓰는 꼴: 원문, 줄기, 원형 (겹치지 않게)
    pub forms: Vec<String>,
    /// 관사 · 전치사 · 대명사 · 접속사 · 조동사 · 명령 동사
    pub function: bool,
}

impl QueryToken {
    pub fn stem(&self) -> &str {
        self.forms.get(1).map(String::as_str).unwrap_or(&self.text)
    }

    pub fn is_noun(&self) -> bool {
        matches!(self.upos.as_deref(), Some("NOUN" | "PROPN"))
    }

    /// 꼴 중 하나가 `f` 를 만족하는가
    pub fn any(&self, f: impl Fn(&str) -> bool) -> bool {
        self.forms.iter().any(|x| f(x))
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Analysis {
    /// "ko" 또는 "en" (한글이 하나라도 있으면 한국어)
    pub lang: &'static str,
    /// 품사 분석기 ("Stanza ko") · 없으면 None (규칙만)
    pub tagger: Option<String>,
    pub tokens: Vec<QueryToken>,
}

pub fn is_hangul(c: char) -> bool {
    ('가'..='힣').contains(&c) || ('ㄱ'..='ㆎ').contains(&c)
}

fn trim(raw: &str) -> String {
    raw.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '&' | '%' | '/' | '+')).to_lowercase()
}

/// 한국어 원형 · xpos 에서 줄기 (끝의 조사 · 어미 · 접미 · 문장부호를 뗌)
pub fn ko_stem(lemma: &str, xpos: &str) -> Option<String> {
    let parts: Vec<&str> = lemma.split('+').collect();
    let tags: Vec<&str> = xpos.split('+').collect();
    if parts.len() != tags.len() || parts.is_empty() {
        return None;
    }
    let mut end = parts.len();
    while end > 1 {
        let t = tags[end - 1];
        // 문장부호(sf · sp · ss · se · so · sw)만 뗍니다. sl(외국어) · sn(숫자) · sh(한자)는 낱말 몸통입니다.
        let punct = matches!(t, "sf" | "sp" | "ss" | "se" | "so" | "sw");
        if t.starts_with('j') || t.starts_with('e') || t.starts_with("xs") || punct {
            end -= 1;
        } else {
            break;
        }
    }
    let stem: String = parts[..end].concat();
    (!stem.is_empty()).then_some(stem)
}

fn function_en(t: &Tagged) -> bool {
    let lemma = t.lemma.to_lowercase();
    match t.upos.as_str() {
        "DET" | "ADP" | "AUX" | "CCONJ" | "SCONJ" | "PART" | "PRON" | "PUNCT" | "INTJ" => true,
        "VERB" => lexicon::COMMAND_VERBS_EN.contains(&lemma.as_str()),
        _ => false,
    }
}

fn function_ko(t: &Tagged, stem: &str) -> bool {
    match t.upos.as_str() {
        "ADP" | "CCONJ" | "SCONJ" | "PART" | "PUNCT" => true,
        "VERB" | "AUX" => lexicon::COMMAND_VERBS_KO.iter().any(|v| stem.starts_with(v)),
        _ => false,
    }
}

fn command_ko(w: &str) -> bool {
    w.ends_with("줘") || w.ends_with("주세요") || w.ends_with("줄래") || matches!(w, "검색" | "찾기" | "보기")
}

/// 질의를 분석합니다. `stanza` 는 언어 코드 → 그 언어 Stanza (없으면 None).
pub fn analyze<'a>(query: &str, stanza: impl Fn(&str) -> Option<&'a StanzaLang>) -> Analysis {
    let raw: Vec<&str> = query.split_whitespace().collect();
    let texts: Vec<String> = raw.iter().map(|w| trim(w)).collect();
    let lang = if query.chars().any(is_hangul) { "ko" } else { "en" };
    // 원문 대소문자를 살려 넣습니다 (영어 품사는 대문자 약어 · 고유명사에 기댑니다). 빈 낱말(문장부호만)은 빼고 넣고 자리를 맞춥니다.
    let keep: Vec<usize> = (0..raw.len()).filter(|i| !texts[*i].is_empty()).collect();
    let words: Vec<String> = keep.iter().map(|i| raw[*i].trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '&' | '%' | '/' | '+')).to_string()).collect();
    let mut tags: Vec<Option<Tagged>> = vec![None; raw.len()];
    let mut tagger = None;
    if let Some(model) = stanza(lang) {
        let refs: Vec<&str> = words.iter().map(String::as_str).collect();
        if !refs.is_empty() {
            if let Ok(tagged) = model.tag(&refs) {
                for (k, t) in keep.iter().zip(tagged) {
                    tags[*k] = Some(t);
                }
                tagger = Some(format!("Stanza {lang}"));
            }
        }
    }
    let tokens = texts
        .iter()
        .zip(tags)
        .map(|(text, tag)| {
            let hangul = text.chars().any(is_hangul);
            let mut forms = vec![text.clone()];
            let mut push = |f: String| {
                if !f.is_empty() && f != "_" && !forms.contains(&f) {
                    forms.push(f);
                }
            };
            let function = match &tag {
                Some(t) if hangul => {
                    let stem = ko_stem(&t.lemma, &t.xpos).unwrap_or_else(|| text.clone());
                    push(stem.to_lowercase());
                    function_ko(t, &stem) || command_ko(text)
                }
                Some(t) => {
                    push(t.lemma.to_lowercase());
                    if !hangul && t.lemma.to_lowercase() == *text {
                        // 원형이 그대로여도 복수형 규칙 하나는 더 봅니다 (원장 낱말이 단수)
                        push(lexicon::singular_en(text));
                    }
                    function_en(t)
                }
                None if hangul => {
                    if let Some(s) = lexicon::strip_particle_ko(text) {
                        push(s);
                    }
                    command_ko(text)
                }
                None => {
                    push(lexicon::singular_en(text));
                    lexicon::FILLER_EN.contains(&text.as_str())
                }
            };
            QueryToken {
                text: text.clone(),
                hangul,
                upos: tag.as_ref().map(|t| t.upos.clone()),
                xpos: tag.as_ref().map(|t| t.xpos.clone()),
                lemma: tag.as_ref().map(|t| t.lemma.clone()),
                forms,
                function: function || text.is_empty(),
            }
        })
        .collect();
    Analysis { lang, tagger, tokens }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn korean_stems_drop_particles_endings_and_suffixes() {
        assert_eq!(ko_stem("급수펌프+의", "nq+jcm").as_deref(), Some("급수펌프"));
        assert_eq!(ko_stem("고착+되+ㄴ", "ncpa+xsv+etm").as_deref(), Some("고착"));
        assert_eq!(ko_stem("높+ㄴ", "paa+etm").as_deref(), Some("높"));
        assert_eq!(ko_stem("103+동", "nnc+nbu").as_deref(), Some("103동"));
        assert_eq!(ko_stem("유인+통풍기", "ncn+ncn").as_deref(), Some("유인통풍기"));
        assert_eq!(ko_stem("밸브+를", "ncn"), None, "형태소와 품사 수가 다르면 쓰지 않습니다");
    }

    #[test]
    fn without_a_tagger_rules_still_give_stems_and_fillers() {
        let a = analyze("Show me the stuck valves", |_| None);
        assert_eq!(a.lang, "en");
        assert!(a.tagger.is_none());
        let t: Vec<(&str, bool)> = a.tokens.iter().map(|t| (t.stem(), t.function)).collect();
        assert_eq!(t, [("show", true), ("me", true), ("the", true), ("stuck", false), ("valve", false)]);
        let a = analyze("급수펌프의 압력 보여줘", |_| None);
        assert_eq!(a.lang, "ko");
        assert_eq!(a.tokens[0].forms, ["급수펌프의", "급수펌프"]);
        assert!(a.tokens[2].function, "명령 동사");
    }
}
