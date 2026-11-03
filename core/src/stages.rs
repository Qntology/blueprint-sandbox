//! 파이프라인 단계 기록 (기획 Part 8 6.2절의 하단 파이프라인 바, Stage 1 ~ 5).
//!
//! 단계마다 실제로 걸린 시간과 판정 상태를 남깁니다. 숫자는 그 실행에서 잰 값이고 지어내지 않습니다.
//! 현장 스캔이 없는 순수 신호 알람은 Stage 1 · 2 (SigLIP2 · gocr) 를 건너뜁니다 (문서 모델을 띄우지 않음).

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StageStatus {
    /// 정상 완료
    Done,
    /// 실행하지 않음 (스캔 없음 등)
    Skipped,
    /// 오독을 원장으로 교정함
    Corrected,
    /// 경보 발령 · 원인 지목 등 주의가 필요한 결과
    Alert,
    /// 현장 확인 · 결재를 기다림
    Waiting,
    /// 가드레일이 막음
    Blocked,
}

impl StageStatus {
    pub fn label(self) -> &'static str {
        match self {
            StageStatus::Done => "정상완료",
            StageStatus::Skipped => "건너뜀",
            StageStatus::Corrected => "교정완료",
            StageStatus::Alert => "경보발령",
            StageStatus::Waiting => "대기",
            StageStatus::Blocked => "차단",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct StageRecord {
    pub stage: u8,
    pub name: &'static str,
    /// 이 단계를 맡은 것 (`SigLIP2`, `gocr + 대장`, `규칙`, `Zoro-1 판단`, `Zoro-1 글쓰기`)
    pub actor: &'static str,
    pub ms: f64,
    pub status: StageStatus,
    /// 카드에 보이는 한 줄
    pub summary: String,
}

pub const NAMES: [(&str, &str); 5] = [
    ("SigLIP2 크롭", "SigLIP2"),
    ("gocr 판독 + 대장 교정", "gocr + Lattice"),
    ("Gate 1~2 원장 대조", "규칙"),
    ("Gate 3 + Zoro-1 판단", "Zoro-1 판단"),
    ("Zoro-1 AOP 초안", "Zoro-1 글쓰기"),
];

pub fn record(stage: u8, ms: f64, status: StageStatus, summary: impl Into<String>) -> StageRecord {
    let (name, actor) = NAMES[(stage.clamp(1, 5) - 1) as usize];
    StageRecord { stage, name, actor, ms, status, summary: summary.into() }
}

/// 시간 표시 (1초 미만은 ms, 넘으면 초)
pub fn duration_text(ms: f64) -> String {
    if ms <= 0.0 {
        "-".to_string()
    } else if ms < 1000.0 {
        format!("{ms:.1}ms")
    } else {
        format!("{:.2}초", ms / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_carry_the_stage_names_from_the_plan() {
        let r = record(4, 630.0, StageStatus::Alert, "상류원인 확신 0.97");
        assert_eq!(r.name, "Gate 3 + Zoro-1 판단");
        assert_eq!(r.status.label(), "경보발령");
        assert_eq!(duration_text(1.4), "1.4ms");
        assert_eq!(duration_text(3800.0), "3.80초");
        assert_eq!(duration_text(0.0), "-");
    }
}
