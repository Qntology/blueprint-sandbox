//! 화면 언어 (한국어 · 영어). 콘솔 설정에서 고르고, 앱이 프로세스 전체에 한 번 정합니다.
//!
//! 엔진 · 원장 · 모델의 안쪽 글(판단 질문 · 사실 줄 · 원장 이름)은 언제나 한국어입니다. 화면에 나가는 글은 프런트가
//! 번역 표(`app/src/i18n/en.json`)로 바꾸고, 엔진이 이 설정을 보는 곳은 모델이 직접 쓰는 글(절차서 초안 요약) 하나입니다.
//! 번역 표로는 모델이 쓴 자유 서술을 옮길 수 없으므로, 영어 화면이면 글쓰기 모델에게 처음부터 영어로 쓰게 합니다.

use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Serialize};

use crate::domain::DomainMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lang {
    #[default]
    Ko,
    En,
}

impl Lang {
    pub const ALL: [Lang; 2] = [Lang::Ko, Lang::En];

    pub fn code(self) -> &'static str {
        match self {
            Lang::Ko => "ko",
            Lang::En => "en",
        }
    }

    pub fn parse(code: &str) -> Option<Lang> {
        match code.trim().to_ascii_lowercase().as_str() {
            "ko" | "kor" | "korean" | "한국어" => Some(Lang::Ko),
            "en" | "eng" | "english" | "영어" => Some(Lang::En),
            _ => None,
        }
    }

    /// 설정 화면에 쓰는 이름 (그 언어로)
    pub fn label(self) -> &'static str {
        match self {
            Lang::Ko => "한국어",
            Lang::En => "English",
        }
    }
}

static LANG: AtomicU8 = AtomicU8::new(0);

/// 지금 화면 언어
pub fn current() -> Lang {
    match LANG.load(Ordering::Relaxed) {
        1 => Lang::En,
        _ => Lang::Ko,
    }
}

pub fn set(lang: Lang) {
    LANG.store(if lang == Lang::En { 1 } else { 0 }, Ordering::Relaxed);
}

/// 영어 화면의 절차서 초안 지시문 (한국어는 [`crate::domain::Terms::drafting_system`])
pub fn drafting_system_en(mode: DomainMode) -> &'static str {
    match mode {
        DomainMode::PowerPlant => {
            "You draft abnormal operating procedure (AOP) summaries for the main control room of a 1.2 GW coal-fired power plant. \
             Write in English. Use only the given facts. Never invent numbers, equipment codes, or times that are not in the facts. \
             Do not write internal verdict codes such as G3_..."
        }
        DomainMode::AiDataCenter => {
            "You draft emergency operating procedure (EOP) summaries for the network operations center (NOC) of a 100 MW AI data center. \
             Write in English. Use only the given facts. Never invent numbers, asset codes, or times that are not in the facts. \
             Do not write internal verdict codes such as G3_..."
        }
    }
}

/// 영어 화면의 초안 요청. 사실 줄은 원장 그대로(한국어)이고, 모델이 영어로 옮겨 씁니다.
/// 위치 · 단위 낱말(103동 · 2층 · 2차측 · 4.3초 · 2홉)은 모델이 그대로 베끼므로 미리 영어로 바꿔 줍니다 ([`english_tokens`]).
/// 숫자는 그대로라 숫자 대조(factcheck)는 원래 사실 줄과 견주어도 같습니다.
pub fn draft_ask_en(facts: &str, aop_id: &str, aop_title: &str) -> String {
    let facts = english_tokens(facts);
    format!(
        "Write a root-cause diagnosis summary in English, in at most two sentences, from the facts below. \
         Translate equipment names into English, and copy every number, unit, location and equipment code exactly as written in the facts.\n\n\
         Facts:\n{facts}\n\nStandard procedure: {aop_id} {aop_title}"
    )
}

/// 사실 줄의 위치 · 단위 낱말을 영어로: `103동` → `Bldg 103`, `2층` → `2F`, `1호기` → `Unit 1`, `2차측` → `secondary-side`,
/// `1차` → `primary`, `4.3초` → `4.3 s`, `2홉` → `2 hops`. 설비 이름 같은 나머지 한국어는 그대로 둡니다 (모델이 옮김).
pub fn english_tokens(text: &str) -> String {
    let s = text.replace("2차측", "secondary-side").replace("2차", "secondary").replace("1차측", "primary-side").replace("1차", "primary");
    let suffixes: [(&str, fn(&str) -> String); 5] = [
        ("호기", |n| format!("Unit {n}")),
        ("동", |n| format!("Bldg {n}")),
        ("층", |n| format!("{n}F")),
        ("홉", |n| if n == "1" { "1 hop".to_string() } else { format!("{n} hops") }),
        ("초", |n| format!("{n} s")),
    ];
    let mut out = String::with_capacity(s.len() + 16);
    let mut rest = s.as_str();
    'scan: while !rest.is_empty() {
        // 숫자(소수점 포함)로 시작하는 자리에서 뒤에 단위 낱말이 붙어 있으면 바꿉니다
        let digits = rest.char_indices().take_while(|(_, c)| c.is_ascii_digit() || *c == '.').last().map(|(i, c)| i + c.len_utf8()).unwrap_or(0);
        let starts_word = out.chars().last().is_none_or(|c| !c.is_ascii_alphanumeric() && c != '.' && c != '-');
        if digits > 0 && starts_word && rest.as_bytes()[0].is_ascii_digit() {
            let (num, after) = rest.split_at(digits);
            for (suffix, make) in &suffixes {
                if let Some(tail) = after.strip_prefix(suffix) {
                    out.push_str(&make(num));
                    rest = tail;
                    continue 'scan;
                }
            }
            out.push_str(num);
            rest = after;
            continue;
        }
        let c = rest.chars().next().unwrap_or_default();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// 영어 초안이 대조에 걸렸을 때 다시 묻는 말
pub fn draft_retry_en(ask: &str, known: &str) -> String {
    format!("{ask}\n\nNote: use only numbers and equipment codes that appear in the facts. Values you may use: {known}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_round_trip() {
        for l in Lang::ALL {
            assert_eq!(Lang::parse(l.code()), Some(l));
        }
        assert_eq!(Lang::parse("English"), Some(Lang::En));
        assert_eq!(Lang::parse("fr"), None);
        assert_eq!(serde_json::to_string(&Lang::En).unwrap(), "\"en\"");
    }

    #[test]
    fn location_and_unit_words_become_english_but_numbers_stay() {
        let facts = "상류 이상 설비: 103동 2F (EL.+9.0m) CDU 2차측 유량조절밸브 밸브 개도 18.0 % · 전파 시차: 4.3초 (허용 2.0 ~ 12.0초) · 1홉 · 2홉 · 1호기 3층";
        assert_eq!(
            english_tokens(facts),
            "상류 이상 설비: Bldg 103 2F (EL.+9.0m) CDU secondary-side 유량조절밸브 밸브 개도 18.0 % · 전파 시차: 4.3 s (허용 2.0 ~ 12.0 s) · 1 hop · 2 hops · Unit 1 3F"
        );
        // 코드 안의 숫자 뒤 글자는 건드리지 않습니다 (10LAB20AA102 · B200)
        assert_eq!(english_tokens("10LAB20AA102 · B200 NVL72 · 105동"), "10LAB20AA102 · B200 NVL72 · Bldg 105");
        // 영어 요청문에도 바뀐 사실 줄이 들어갑니다
        assert!(draft_ask_en("101동 5F 최종과열기", "AOP-103-FW02", "급수 유량 저하").contains("Bldg 101 5F"));
    }
}
