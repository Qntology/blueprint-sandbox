//! 판단 한 번의 입력(사실 · 선택지)과 모델 인터페이스 (zoro-gravity 와 같은 약속).
//!
//! 규칙 엔진은 계측 · 원장 · 계통 그래프에서 판정을 정하고, 모델은 같은 질문에 선택지별 확률을 냅니다.
//! 모델에 주는 `facts` 에는 근무자 이름 · 사번을 넣지 않습니다. 설비 코드와 계측값, 규칙이 찾은 결과만 넣습니다.

use anyhow::Result;

use crate::verdict::Verdict;

/// 모델에 주는 사실 한 줄
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fact {
    pub key: String,
    pub value: String,
}

impl Fact {
    pub fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self { key: key.into(), value: value.into() }
    }
}

/// 판단 질문: 제목, 사실, 선택지. 선택지 순서가 확률 벡터의 순서입니다.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecisionTask {
    pub title: String,
    pub facts: Vec<Fact>,
    pub options: Vec<Verdict>,
}

pub trait DecisionModel: Send + Sync {
    /// `task.options` 순서대로의 확률. 합은 1 입니다.
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn facts_keep_their_order() {
        let task = DecisionTask {
            title: "t".into(),
            facts: vec![Fact::new("b", "2"), Fact::new("a", "1")],
            options: vec![Verdict::G3EligibleNormal],
        };
        assert_eq!(task.facts[0].key, "b");
        assert_eq!(task.facts[1], Fact::new("a", "1"));
    }
}
