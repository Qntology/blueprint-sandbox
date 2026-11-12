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

use std::sync::Arc;

use serde::Serialize;

use super::{lang, lexicon};
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
    /// 질의 언어 (12개 언어 중 하나, [`lang::detect`])
    pub lang: &'static str,
    /// 언어를 정한 근거 ("문자 체계" · "고유 글자" · "사전 투표" · "whatlang" · "기본")
    pub how: &'static str,
    /// 투표가 비겨 함께 볼 언어
    pub also: Vec<&'static str>,
    /// 품사 분석기 ("Stanza ko") · 없으면 None (규칙만)
    pub tagger: Option<String>,
    pub tokens: Vec<QueryToken>,
    /// 낱말마다 따옴표 안인가 (데이터 값 · 이름)
    pub quoted: Vec<bool>,
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
/// 언어는 [`lang::detect`] 가 정합니다 (12개 언어). 한국어 · 영어는 공백 낱말 그대로, 나머지는 [`analyze_ml`].
pub fn analyze(query: &str, stanza: impl Fn(&str) -> Option<Arc<StanzaLang>>) -> Analysis {
    let det = lang::detect(query);
    if det.lang != "ko" && det.lang != "en" {
        return analyze_ml(query, det, stanza);
    }
    let raw: Vec<&str> = query.split_whitespace().collect();
    let texts: Vec<String> = raw.iter().map(|w| trim(w)).collect();
    let lang = det.lang;
    let quoted = quoted_flags(query, &raw);
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
    Analysis { lang, how: det.how, also: det.also, tagger, tokens, quoted }
}

/// 공백 낱말마다 따옴표 안에 있는가 (원문에서 차례로 찾아 맞춤)
fn quoted_flags(query: &str, words: &[&str]) -> Vec<bool> {
    let ranges = lang::quoted_ranges(query);
    let mut at = 0;
    words
        .iter()
        .map(|w| match (!w.is_empty()).then(|| query[at..].find(w)).flatten() {
            Some(off) => {
                let start = at + off;
                at = start + w.len();
                ranges.iter().any(|r| r.start <= start && at <= r.end)
            }
            None => false,
        })
        .collect()
}

/// 글자 묶음 (붙여 쓰는 언어를 나눌 때): 한자 · 히라가나 · 가타카나 · 로마자 · 숫자 · 그 밖
#[derive(Clone, Copy, PartialEq, Eq)]
enum Run {
    Han,
    Hira,
    Kata,
    Latin,
    Digit,
    Other,
}

fn run_of(c: char) -> Option<Run> {
    Some(match c as u32 {
        0x3040..=0x309F => Run::Hira,
        0x30A0..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Run::Kata,
        0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x3005 => Run::Han,
        _ if c.is_ascii_digit() => Run::Digit,
        _ if c == '.' || c == ',' => return None,
        _ if c.is_alphanumeric() => Run::Latin,
        _ => Run::Other,
    })
}

/// 질의를 낱말로 나눕니다 (원문 바이트 자리와 함께).
/// - 라틴 · 아랍 문자: 공백 낱말. 프랑스어 · 이탈리아어 생략 부호는 나눕니다 (dell'edificio → dell + edificio, d'eau → d + eau).
/// - 일본어 · 중국어: 같은 문자 묶음(한자 · 히라가나 · 가타카나 · 로마자 · 숫자)마다. 중국어 한자 묶음은 '的' 에서도 나눕니다.
///   Stanza 토크나이저가 있으면 그것을 씁니다 ([`analyze_ml`]).
fn split_words(query: &str, lang: &str) -> Vec<(String, usize)> {
    let mut out: Vec<(String, usize)> = Vec::new();
    if lang::glued(lang) {
        let mut cur = String::new();
        let mut cur_at = 0;
        let mut cur_run: Option<Run> = None;
        let flush = |cur: &mut String, at: usize, out: &mut Vec<(String, usize)>| {
            let t = cur.trim_matches(|c: char| !c.is_alphanumeric() && c != 'ー');
            if !t.is_empty() {
                out.push((t.to_string(), at));
            }
            cur.clear();
        };
        for (i, c) in query.char_indices() {
            let r = run_of(c);
            // 숫자 사이 소수점 · 쉼표는 숫자에 붙임 (1,400 · 9.0)
            let joins_number = r.is_none() && cur_run == Some(Run::Digit);
            let split_here = c.is_whitespace() || r == Some(Run::Other) || (lang == "zh-hans" && c == '的') || (!joins_number && r != cur_run && !(c == 'ー' && cur_run == Some(Run::Kata)));
            if split_here {
                flush(&mut cur, cur_at, &mut out);
                cur_at = i;
                if lang == "zh-hans" && c == '的' {
                    out.push((c.to_string(), i));
                    cur_at = i + c.len_utf8();
                    cur_run = None;
                    continue;
                }
            }
            if !c.is_whitespace() && r != Some(Run::Other) {
                if cur.is_empty() {
                    cur_at = i;
                }
                cur.push(c);
                if !joins_number {
                    cur_run = r;
                }
            }
        }
        flush(&mut cur, cur_at, &mut out);
        return out;
    }
    let elide = matches!(lang, "fr" | "it");
    let mut at = 0;
    for w in query.split_whitespace() {
        let start = at + query[at..].find(w).unwrap_or(0);
        at = start + w.len();
        if elide {
            if let Some(p) = w.find(['\'', '’']) {
                let (head, tail) = (&w[..p], &w[p + w[p..].chars().next().map_or(1, char::len_utf8)..]);
                if !head.is_empty() && !tail.is_empty() && head.chars().all(char::is_alphabetic) {
                    out.push((head.to_string(), start));
                    out.push((tail.to_string(), start + p + 1));
                    continue;
                }
            }
        }
        out.push((w.to_string(), start));
    }
    out
}

/// 한국어 · 영어 밖 언어: 낱말 나누기 → (있으면) Stanza 품사 · 원형 → 기능어 표시
fn analyze_ml(query: &str, det: lang::Detected, stanza: impl Fn(&str) -> Option<Arc<StanzaLang>>) -> Analysis {
    let lang_code = det.lang;
    let lex = super::ml::lex(lang_code);
    let model = stanza(lang_code);
    // 붙여 쓰는 언어는 Stanza 토크나이저가 있으면 그 낱말을 씁니다 (품사 모델이 그 낱말로 학습됨)
    let pieces: Vec<(String, usize)> = match (&model, lang::glued(lang_code)) {
        (Some(m), true) => match m.tokenize(query) {
            Ok(ws) if !ws.is_empty() => {
                let mut at = 0;
                ws.into_iter()
                    .filter_map(|w| {
                        let off = query[at..].find(&w)?;
                        let start = at + off;
                        at = start + w.len();
                        Some((w, start))
                    })
                    .collect()
            }
            _ => split_words(query, lang_code),
        },
        _ => split_words(query, lang_code),
    };
    let ranges = lang::quoted_ranges(query);
    let quoted: Vec<bool> = pieces.iter().map(|(w, s)| ranges.iter().any(|r| r.start <= *s && s + w.len() <= r.end)).collect();
    let texts: Vec<String> = pieces.iter().map(|(w, _)| trim(w)).collect();
    let words: Vec<String> = pieces.iter().map(|(w, _)| w.trim_matches(|c: char| !c.is_alphanumeric() && !matches!(c, '-' | '&' | '%' | '/' | '+' | 'ー')).to_string()).collect();
    let keep: Vec<usize> = (0..texts.len()).filter(|i| !texts[*i].is_empty()).collect();
    let mut tags: Vec<Option<Tagged>> = vec![None; texts.len()];
    let mut tagger = None;
    if let Some(m) = &model {
        let refs: Vec<&str> = keep.iter().map(|i| words[*i].as_str()).collect();
        if !refs.is_empty() {
            if let Ok(tagged) = m.tag(&refs) {
                for (k, t) in keep.iter().zip(tagged) {
                    tags[*k] = Some(t);
                }
                tagger = Some(format!("Stanza {lang_code}"));
            }
        }
    }
    let tokens = texts
        .iter()
        .zip(tags)
        .map(|(text, tag)| {
            let mut forms = vec![text.clone()];
            if let Some(t) = &tag {
                let l = t.lemma.to_lowercase();
                if !l.is_empty() && l != "_" && !forms.contains(&l) {
                    forms.push(l);
                }
            }
            let folded = lang::fold(text);
            let function = match &tag {
                // 닫힌 품사(관사 · 전치사 · 대명사 · 접속사 · 조동사 · 문장부호)와 명령 동사. 부정어 · 비교어는 사전이 따로 봅니다.
                // 사전의 기능어 목록도 함께 봅니다: Stanza 는 아랍어 'ذات'(…을 가진)을 명사로 달아, '압력' 이 펌프 이름 안의 계측어로 읽혀 조건이 사라졌습니다
                Some(t) => {
                    matches!(t.upos.as_str(), "DET" | "ADP" | "AUX" | "CCONJ" | "SCONJ" | "PART" | "PRON" | "PUNCT" | "INTJ")
                        || lex.is_some_and(|l| l.is_function(&folded) || l.is_command(&folded) || l.is_command(&lang::fold(&t.lemma)))
                }
                None => lex.is_some_and(|l| l.is_function(&folded)),
            };
            QueryToken {
                text: text.clone(),
                hangul: false,
                upos: tag.as_ref().map(|t| t.upos.clone()),
                xpos: tag.as_ref().map(|t| t.xpos.clone()),
                lemma: tag.as_ref().map(|t| t.lemma.clone()),
                forms,
                function: function || text.is_empty(),
            }
        })
        .collect();
    Analysis { lang: lang_code, how: det.how, also: det.also, tagger, tokens, quoted }
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
