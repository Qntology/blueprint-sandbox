//! 모델이 쓰는 초안: 비정상 운전절차서(AOP) 원인 진단 요약, 현장 자유 서술 소명의 정규화.
//!
//! 기획 Part 8 5절 Stage 5: 판단과 같은 Zoro-1 몸통에 임베딩 표를 출력 투영으로 붙여(`TextWriter`) 글을 씁니다.
//! 엔진은 [`TextWriter`] 에만 기대므로 실제 모델이든 시험 대역이든 같은 방식으로 꽂힙니다.
//!
//! 초안은 판정이 아닙니다. 글의 설비 코드와 숫자는 모두 사실 줄과 1:1 로 대조하고(`factcheck::check`), 어긋나면 사실의 숫자를 알려 주고
//! 한 번 다시 쓰게 합니다. 그래도 어긋나면 모델 글을 버리고 사실 줄로만 만든 템플릿 요약을 씁니다 (`G4_INTERCEPT_DOC_CORRUPTION`).
//! 조치 단계는 표준 절차서의 문장을 그대로 씁니다. 모델이 조치를 새로 만들지 않습니다.

use std::time::Instant;

use anyhow::Result;
use serde::Serialize;

use crate::case::DeviationExcuse;
use crate::factcheck::{check, codes, matched_numbers, mentions};
use crate::lattice::AopRow;

/// 지시문과 입력을 받아 글을 쓰는 모델
pub trait TextWriter: Send + Sync {
    fn complete(&self, system: &str, user: &str, max_tokens: usize) -> Result<String>;
}

const AOP_TOKENS: usize = 140;
const EXCUSE_TOKENS: usize = 24;

/// 조치 절차서 초안
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AopDraft {
    pub aop_id: String,
    pub title: String,
    /// 원인 진단 요약
    pub summary: String,
    /// 표준 조치 지시 (절차서 단계 그대로)
    pub steps: Vec<String>,
    /// 요약을 모델이 썼는가 (아니면 템플릿)
    pub by_model: bool,
    /// 모델 글이 숫자 · 코드 대조를 통과했는가. 템플릿이면 사실 줄로만 만들어 통과로 봅니다.
    pub verified: bool,
    /// 모델 글에서 걸린 내용 (원문에 없는 숫자 · 코드 등)
    pub problems: Vec<String>,
    /// 요약의 숫자와 원문 대조 결과 `(글의 표기, 원문에 있음)`
    pub numbers: Vec<(String, bool)>,
    pub high_risk: bool,
    pub risk_actions: Vec<String>,
    /// 대조 원문 (사실 줄)
    pub facts: Vec<String>,
    /// 글쓰기에 걸린 시간 (모델이 없으면 0)
    pub ms: f64,
    /// 모델에 몇 번 물었는가
    pub attempts: usize,
}

impl AopDraft {
    /// 결재 문서 본문 (주제어실 전용)
    pub fn text(&self) -> String {
        let mut out = format!("[원인 진단 요약]\n{}\n\n[표준 조치 지시 ({} {})]", self.summary, self.aop_id, self.title);
        for (i, s) in self.steps.iter().enumerate() {
            out.push_str(&format!("\n{}. {s}", i + 1));
        }
        out
    }
}

/// `G3_ADVERSE_UPSTREAM_CAUSE` 같은 내부 판정 코드가 글에 있는가 (설비 코드는 밑줄이 없어 걸리지 않습니다)
pub fn has_internal_code(text: &str) -> bool {
    text.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_')).any(|w| w.contains('_') && w.bytes().any(|b| b.is_ascii_uppercase()))
}

/// 끝맺지 못한 글을 마지막으로 완성된 문장까지만 남깁니다. 숫자 안의 소수점에서는 끊지 않습니다.
pub fn through_last_sentence(text: &str) -> &str {
    let b = text.as_bytes();
    let end = (0..b.len()).rev().find(|&i| matches!(b[i], b'.' | b'!' | b'?' | b'\n') && !(b[i] == b'.' && b.get(i + 1).is_some_and(u8::is_ascii_digit)));
    end.map_or("", |i| text[..=i].trim_end())
}

fn clean(text: &str) -> String {
    let t = text.trim();
    let t = t.trim_start_matches("[원인 진단 요약]").trim_start_matches("원인 진단 요약").trim_start_matches(['：', ':', ' ', '\n']);
    t.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ")
}

fn problems_of(text: &str, source: &str) -> Vec<String> {
    let mut p = check(text, source);
    if text.trim().is_empty() {
        p.push("글이 비었습니다".to_string());
    }
    if has_internal_code(text) {
        p.push("내부 판정 코드가 그대로 나왔습니다".to_string());
    }
    p
}

/// AOP 초안을 씁니다. 모델이 없거나 두 번 다 대조에 걸리면 `template` 요약을 씁니다.
/// `system` 은 도메인 용어로 만든 지시문입니다 ([`crate::domain::Terms::drafting_system`]: 발전소 주제어실 · 데이터센터 NOC 관제실).
pub fn draft_aop(writer: Option<&dyn TextWriter>, system: &str, aop: &AopRow, facts: &[String], template: &str) -> AopDraft {
    // 표준 절차서 문장의 숫자(EL.+9.0m 등)도 원문으로 봅니다.
    let source = format!("{}\n{}\n{}", facts.join("\n"), aop.steps.join("\n"), aop.id);
    let base = AopDraft {
        aop_id: aop.id.clone(),
        title: aop.title.clone(),
        summary: template.to_string(),
        steps: aop.steps.clone(),
        by_model: false,
        verified: true,
        problems: Vec::new(),
        numbers: matched_numbers(template, &source),
        high_risk: aop.high_risk,
        risk_actions: aop.risk_actions.clone(),
        facts: facts.to_vec(),
        ms: 0.0,
        attempts: 0,
    };
    let Some(writer) = writer else { return base };
    let t = Instant::now();
    let ask = format!(
        "다음 사실로 원인 진단 요약을 두 문장 이내로 쓰세요. 숫자와 설비 코드는 사실에 적힌 그대로 옮기세요.\n\n사실:\n{}\n\n표준 절차서: {} {}",
        facts.join("\n"),
        aop.id,
        aop.title
    );
    let mut problems = Vec::new();
    let mut attempts = 0;
    let mut tries = vec![ask.clone()];
    let known: Vec<String> = mentions(&facts.join("\n")).into_iter().map(|m| m.text).chain(codes(&facts.join("\n"))).collect();
    tries.push(format!("{ask}\n\n주의: 숫자와 설비 코드는 사실에 있는 것만 그대로 쓰세요. 쓸 수 있는 값: {}", known.join(", ")));
    for prompt in tries {
        attempts += 1;
        let text = match writer.complete(system, &prompt, AOP_TOKENS) {
            Ok(t) => clean(&t),
            Err(e) => {
                problems.push(format!("글쓰기 실패: {e}"));
                break;
            }
        };
        let found = problems_of(&text, &source);
        if found.is_empty() {
            return AopDraft { numbers: matched_numbers(&text, &source), summary: text, by_model: true, ms: t.elapsed().as_secs_f64() * 1e3, attempts, ..base };
        }
        problems.extend(found);
    }
    problems.dedup();
    AopDraft { problems, ms: t.elapsed().as_secs_f64() * 1e3, attempts, ..base }
}

/// 현장 자유 서술 소명을 표준 소명으로 바꿉니다. 모르는 답이면 None 이고, 그대로 교대감독에게 갑니다.
pub fn normalize_excuse(writer: &dyn TextWriter, system: &str, text: &str) -> Result<Option<DeviationExcuse>> {
    let ask = format!(
        "코드 목록:\nEXCUSE_TRANSMITTER_DRIFT: 현장 게이지는 정상이고 전송기 · 계측기 값이 흔들린다는 내용\nEXCUSE_MOC_CHANGE: 정비 · 부품 교체로 운전 사양이 바뀌었다는 내용\nEXCUSE_MANUAL_BYPASS: 현장에서 바이패스 밸브를 손으로 조작 중이라는 내용\nEXCUSE_OTHER: 그 밖의 내용\n\n소명: {text}"
    );
    let answer = writer.complete(system, &ask, EXCUSE_TOKENS)?;
    Ok(if answer.contains("EXCUSE_TRANSMITTER_DRIFT") {
        Some(DeviationExcuse::TransmitterDrift)
    } else if answer.contains("EXCUSE_MOC_CHANGE") {
        Some(DeviationExcuse::MocChange)
    } else if answer.contains("EXCUSE_MANUAL_BYPASS") {
        Some(DeviationExcuse::ManualBypass)
    } else {
        None
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// 정해 둔 답을 차례로 돌려주는 대역. 받은 입력을 기록합니다.
    pub(crate) struct Script {
        answers: Mutex<Vec<String>>,
        pub asked: Mutex<Vec<String>>,
    }

    pub(crate) fn script(answers: &[&str]) -> Script {
        Script { answers: Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()), asked: Mutex::new(Vec::new()) }
    }

    impl TextWriter for Script {
        fn complete(&self, _system: &str, user: &str, _max_tokens: usize) -> Result<String> {
            self.asked.lock().unwrap().push(user.to_string());
            Ok(self.answers.lock().unwrap().pop().unwrap_or_default())
        }
    }

    fn aop() -> AopRow {
        AopRow {
            id: "AOP-103-FW02".into(),
            title: "급수 유량 저하 · 급수조절밸브 고착".into(),
            sectors: vec!["feedwater".into()],
            triggers: vec![],
            steps: vec!["103동 2층(EL.+9.0m) 보조 급수 바이패스 밸브를 수동으로 연다".into(), "출력을 감발한다".into()],
            risk_actions: vec!["출력 감발".into()],
            high_risk: true,
            role: None,
        }
    }

    fn facts() -> Vec<String> {
        vec![
            "경보: 101동 5F 최종과열기(10HAH10BB001) 과열기 출구 증기 온도 638.0 °C (경보 한계 625.0 °C)".into(),
            "상류 원인 후보: 103동 2F 주급수 유량조절밸브(10LAB20AA102) 밸브 개도 18.0 % (정상 78.0 %)".into(),
            "급수 유량: 1,120 t/h (정상 1,850 t/h)".into(),
            "전파 시차: 4.2초 (허용 2.0 ~ 12.0초)".into(),
        ]
    }

    const TEMPLATE: &str = "과열기 출구 증기 온도가 638.0 °C 로 올랐습니다.";
    const SYS: &str = "발전소 주제어실의 비정상 운전절차서(AOP) 초안 작성 담당입니다.";

    #[test]
    fn a_clean_model_summary_is_kept() {
        let w = script(&["101동 과열기 출구 온도 638.0 °C 상승은 103동 급수조절밸브(10LAB20AA102) 개도가 18.0 %로 고착되어 급수 유량이 1,120 t/h 로 줄었기 때문입니다."]);
        let d = draft_aop(Some(&w), SYS, &aop(), &facts(), TEMPLATE);
        assert!(d.by_model && d.verified, "{:?}", d.problems);
        assert_eq!(d.attempts, 1);
        assert!(d.numbers.iter().all(|(_, ok)| *ok), "{:?}", d.numbers);
        assert!(d.text().contains("1. 103동 2층(EL.+9.0m)"));
        assert!(d.high_risk);
    }

    #[test]
    fn an_invented_number_is_retried_with_the_real_values_then_replaced_by_the_template() {
        let bad = "급수조절밸브 개도가 15.0 %로 고착되었습니다.";
        let w = script(&[bad, bad]);
        let d = draft_aop(Some(&w), SYS, &aop(), &facts(), TEMPLATE);
        assert!(!d.by_model);
        assert_eq!(d.summary, TEMPLATE, "모델 글을 버리고 템플릿을 씁니다");
        assert!(d.problems.iter().any(|p| p.contains("15.0")), "{:?}", d.problems);
        let asked = w.asked.lock().unwrap();
        assert_eq!(asked.len(), 2);
        assert!(asked[1].contains("18.0"), "다시 쓸 때 사실의 값을 알려 줍니다");
    }

    #[test]
    fn a_wrong_equipment_code_or_an_internal_code_is_not_accepted() {
        let w = script(&["10LAB20AA103 밸브 고착", "G3_ADVERSE_UPSTREAM_CAUSE 판정입니다."]);
        let d = draft_aop(Some(&w), SYS, &aop(), &facts(), TEMPLATE);
        assert!(!d.by_model);
        assert!(d.problems.iter().any(|p| p.contains("10LAB20AA103")));
        assert!(d.problems.iter().any(|p| p.contains("내부 판정 코드")));
    }

    #[test]
    fn without_a_writer_the_template_is_used() {
        let d = draft_aop(None, SYS, &aop(), &facts(), TEMPLATE);
        assert!(!d.by_model && d.verified && d.attempts == 0);
        assert_eq!(d.steps.len(), 2);
    }

    #[test]
    fn free_text_excuses_map_to_standard_codes() {
        let n = |answer: &str| normalize_excuse(&script(&[answer]), SYS, "1층 윤활유 냉각기 스트레이너 차압 청소 중");
        assert_eq!(n("EXCUSE_TRANSMITTER_DRIFT").unwrap(), Some(DeviationExcuse::TransmitterDrift));
        assert_eq!(n("EXCUSE_MANUAL_BYPASS").unwrap(), Some(DeviationExcuse::ManualBypass));
        assert_eq!(n("EXCUSE_OTHER").unwrap(), None);
        assert_eq!(n("모르겠습니다").unwrap(), None);
    }

    #[test]
    fn cut_off_text_keeps_finished_sentences_and_codes_are_spotted() {
        assert_eq!(through_last_sentence("원인은 밸브 고착입니다. 유량 1,120"), "원인은 밸브 고착입니다.");
        assert_eq!(through_last_sentence("비율은 3.5"), "");
        assert!(has_internal_code("판정 G3_VIOLATION_HIGH_RISK"));
        assert!(!has_internal_code("설비 10LAB20AA102 와 AOP-103-FW02"));
    }
}
