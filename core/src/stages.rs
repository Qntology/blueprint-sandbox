//! 파이프라인 단계 기록 (기획 Part 8 6.2절의 하단 파이프라인 바, Stage 1 ~ 5).
//!
//! 단계마다 실제로 걸린 시간과 판정 상태를 남깁니다. 숫자는 그 실행에서 잰 값이고 지어내지 않습니다.
//! 현장 스캔이 없는 순수 신호 알람은 Stage 1 · 2 (SigLIP2 · gocr) 를 건너뜁니다 (문서 모델을 띄우지 않음).

use serde::ser::{SerializeStruct, Serializer};
use serde::Serialize;

use crate::i18n::{self, Lang};

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

/// 파이프라인 단계 기록 한 줄. 화면에 나가는 이름 · 요약은 보낼 때(직렬화) 지금 화면 언어로 만듭니다 (UI 단순화 D4).
/// 엔진 안에는 키 + 값([`StageNote`])만 두고, 예전처럼 한국어 문장을 보낸 뒤 화면이 정규식 틀(`scripts/i18n/en_engine.py`)로 옮기지 않습니다.
#[derive(Debug, Clone, PartialEq)]
pub struct StageRecord {
    pub stage: u8,
    pub ms: f64,
    pub status: StageStatus,
    /// 카드에 보이는 한 줄 (키 + 값)
    pub note: StageNote,
}

/// 단계 이름과 맡은 것 `[(한국어 이름, 맡은 것), (영어 이름, 맡은 것)]`
pub const NAMES: [[(&str, &str); 2]; 5] = [
    [("SigLIP2 크롭", "SigLIP2"), ("SigLIP2 Crop", "SigLIP2")],
    [("gocr 판독 + 대장 교정", "gocr + Lattice"), ("gocr Reading + Register Correction", "gocr + Lattice")],
    [("Gate 1~2 원장 대조", "규칙"), ("Gate 1~2 Ledger Check", "Rules")],
    [("Gate 3 + Zoro-1 판단", "Zoro-1 판단"), ("Gate 3 + Zoro-1 Verdict", "Zoro-1 Verdict")],
    [("Zoro-1 AOP 초안", "Zoro-1 글쓰기"), ("Zoro-1 AOP Draft", "Zoro-1 Writing")],
];

impl StageRecord {
    fn names(&self, lang: Lang) -> (&'static str, &'static str) {
        NAMES[(self.stage.clamp(1, 5) - 1) as usize][usize::from(lang == Lang::En)]
    }

    /// 단계 이름 (기획 Part 8 6.2절의 하단 파이프라인 바)
    pub fn name(&self, lang: Lang) -> &'static str {
        self.names(lang).0
    }

    /// 이 단계를 맡은 것 (`SigLIP2`, `gocr + Lattice`, `규칙`, `Zoro-1 판단`, `Zoro-1 글쓰기`)
    pub fn actor(&self, lang: Lang) -> &'static str {
        self.names(lang).1
    }

    /// 한국어 한 줄 (시험 · 기록)
    pub fn summary(&self) -> String {
        self.note.text(Lang::Ko)
    }
}

/// 화면에는 예전과 같은 모양(`name` · `actor` · `summary`)으로 나가고, 글은 지금 화면 언어입니다. `key` 는 E2E · 기록이 봅니다.
impl Serialize for StageRecord {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let lang = i18n::current();
        let mut s = serializer.serialize_struct("StageRecord", 7)?;
        s.serialize_field("stage", &self.stage)?;
        s.serialize_field("name", self.name(lang))?;
        s.serialize_field("actor", self.actor(lang))?;
        s.serialize_field("ms", &self.ms)?;
        s.serialize_field("status", &self.status)?;
        s.serialize_field("summary", &self.note.text(lang))?;
        s.serialize_field("key", self.note.key())?;
        s.end()
    }
}

/// 단계 카드 한 줄의 키 + 값 (UI 단순화 D4). 판정 이름(`label`)은 원장 · 판정 표의 한국어 이름 그대로 넣고,
/// 영어 화면에서는 다른 곳의 판정 이름처럼 화면의 낱말 표가 바꿉니다 (판정 이름 표를 두 벌 두지 않음).
#[derive(Debug, Clone, PartialEq)]
pub enum StageNote {
    /// 신호 알람이라 스캔이 없음
    NoScan,
    /// 현장 수치를 직접 입력해 스캔이 없음
    ManualEntry,
    Read { crops: usize, dropped: usize, blank: usize, skipped: usize },
    Corrected(usize),
    ReadClean,
    MocVerified,
    MocContradiction,
    MocHold,
    ChannelDrift { logic: String, channel: String, field_agrees: bool },
    VoteAgrees { logic: String },
    Flatline,
    SpikeFiltered,
    SignalOk,
    RulesOnly { label: String },
    Confident { label: String, p: f32 },
    NoConfidence { label: String },
    DraftModel { aop: String, high_risk: bool },
    DraftTemplate { aop: String },
    DraftMismatch { aop: String, problems: usize },
    NoProcedure { moc: bool },
    Stopped,
    /// 규칙이 돌려준 사유 그대로 (서명 재요청 등, 화면 번역 표가 바꿈)
    Text(String),
}

impl StageNote {
    pub fn key(&self) -> &'static str {
        match self {
            StageNote::NoScan => "stage.no_scan",
            StageNote::ManualEntry => "stage.manual_entry",
            StageNote::Read { .. } => "stage.read",
            StageNote::Corrected(_) => "stage.corrected",
            StageNote::ReadClean => "stage.read_clean",
            StageNote::MocVerified => "stage.moc_verified",
            StageNote::MocContradiction => "stage.moc_contradiction",
            StageNote::MocHold => "stage.moc_hold",
            StageNote::ChannelDrift { .. } => "stage.channel_drift",
            StageNote::VoteAgrees { .. } => "stage.vote_agrees",
            StageNote::Flatline => "stage.flatline",
            StageNote::SpikeFiltered => "stage.spike_filtered",
            StageNote::SignalOk => "stage.signal_ok",
            StageNote::RulesOnly { .. } => "stage.rules_only",
            StageNote::Confident { .. } => "stage.confident",
            StageNote::NoConfidence { .. } => "stage.no_confidence",
            StageNote::DraftModel { .. } => "stage.draft_model",
            StageNote::DraftTemplate { .. } => "stage.draft_template",
            StageNote::DraftMismatch { .. } => "stage.draft_mismatch",
            StageNote::NoProcedure { .. } => "stage.no_procedure",
            StageNote::Stopped => "stage.stopped",
            StageNote::Text(_) => "stage.text",
        }
    }

    /// 그 언어의 한 줄. 한국어는 예전 문장과 같고, 영어는 예전 번역 틀이 만들던 문장입니다.
    pub fn text(&self, lang: Lang) -> String {
        let en = lang == Lang::En;
        let pick = |ko: &str, en_text: &str| if en { en_text.to_string() } else { ko.to_string() };
        match self {
            StageNote::NoScan => pick("스캔 없음 (신호 알람은 문서 모델을 띄우지 않음)", "No scan (signal alarms don't load the document models)"),
            StageNote::ManualEntry => pick("현장 수치 직접 입력", "Manual field entry"),
            StageNote::Read { crops, dropped, blank, skipped } => {
                let mut parts = vec![if en { format!("{crops} crops read") } else { format!("조각 {crops}개 판독") }, if en { format!("{dropped} notice crops dropped") } else { format!("안내문 {dropped}개 버림") }];
                if *blank > 0 {
                    parts.push(if en { format!("{blank} blank") } else { format!("빈 조각 {blank}개") });
                }
                if *skipped > 0 {
                    parts.push(if en { format!("{skipped} same as the master, skipped") } else { format!("마스터와 같은 조각 {skipped}개 건너뜀") });
                }
                parts.join(" · ")
            }
            StageNote::Corrected(n) => match (en, n) {
                (true, 1) => "1 misread corrected against the register".to_string(),
                (true, n) => format!("{n} misreads corrected against the register"),
                (false, n) => format!("대장 대조로 오독 {n}건 교정"),
            },
            StageNote::ReadClean => pick("판독 완료 · 교정 없음", "Read · no corrections"),
            StageNote::MocVerified => pick("MOC 원장 일치", "MOC ledger match"),
            StageNote::MocContradiction => pick("MOC 원장 불일치 · 잠금", "MOC ledger mismatch · locked"),
            StageNote::MocHold => pick("MOC 승인 · 권한 확인 필요", "MOC approval · authority check needed"),
            StageNote::ChannelDrift { logic, channel, field_agrees } => match (en, field_agrees) {
                (true, true) => format!("{logic} channel {channel} deviation · field agrees"),
                (true, false) => format!("{logic} channel {channel} deviation"),
                (false, true) => format!("{logic} 채널 {channel} 편차 · 현장 일치"),
                (false, false) => format!("{logic} 채널 {channel} 편차"),
            },
            StageNote::VoteAgrees { logic } => if en { format!("{logic} voting agrees") } else { format!("{logic} 보팅 일치") },
            StageNote::Flatline => pick("값 고정 (동결) 검출", "Fixed value (freeze) detected"),
            StageNote::SpikeFiltered => pick("순간 잡음 걸러 냄", "Momentary noise filtered"),
            StageNote::SignalOk => pick("신호 정상 · 원장 대조 완료", "Signal normal · ledger check done"),
            StageNote::RulesOnly { label } => if en { format!("{label} (rules only, no model)") } else { format!("{label} (규칙만, 모델 없음)") },
            StageNote::Confident { label, p } => if en { format!("{label} conf. {p:.2}") } else { format!("{label} 확신 {p:.2}") },
            StageNote::NoConfidence { label } => if en { format!("{label} (model dissent · no confidence)") } else { format!("{label} (모델 이견 · 확신도 없음)") },
            StageNote::DraftModel { aop, high_risk } => match (en, high_risk) {
                (true, true) => format!("{aop} draft · figure check passed · awaiting co-sign"),
                (true, false) => format!("{aop} draft · figure check passed"),
                (false, true) => format!("{aop} 초안 · 숫자 대조 통과 · 공동결재 대기"),
                (false, false) => format!("{aop} 초안 · 숫자 대조 통과"),
            },
            StageNote::DraftTemplate { aop } => if en { format!("{aop} template (no writing model)") } else { format!("{aop} 템플릿 (글쓰기 모델 없음)") },
            StageNote::DraftMismatch { aop, problems } => if en { format!("{aop} model draft: {problems} figure mismatch(es) → template") } else { format!("{aop} 모델 초안 수치 불일치 {problems}건 → 템플릿") },
            StageNote::NoProcedure { moc: true } => pick("절차서 없음 (변경 반영 심사)", "No procedure (change review)"),
            StageNote::NoProcedure { moc: false } => pick("절차서 없음", "No procedure"),
            StageNote::Stopped => pick("앞 단계에서 멈춤", "Stopped at an earlier stage"),
            StageNote::Text(t) => t.clone(),
        }
    }
}

pub fn record(stage: u8, ms: f64, status: StageStatus, note: StageNote) -> StageRecord {
    StageRecord { stage, ms, status, note }
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
        let r = record(4, 630.0, StageStatus::Alert, StageNote::Confident { label: "상류원인".into(), p: 0.97 });
        assert_eq!(r.name(Lang::Ko), "Gate 3 + Zoro-1 판단");
        assert_eq!(r.name(Lang::En), "Gate 3 + Zoro-1 Verdict");
        assert_eq!(r.summary(), "상류원인 확신 0.97");
        assert_eq!(r.status.label(), "경보발령");
        assert_eq!(duration_text(1.4), "1.4ms");
        assert_eq!(duration_text(3800.0), "3.80초");
        assert_eq!(duration_text(0.0), "-");
    }

    /// 키 + 값 요약은 영어 화면에 한글을 남기지 않습니다 (판정 이름 · 규칙 사유만 화면 낱말 표가 바꿈, D4)
    #[test]
    fn stage_notes_render_in_both_languages_without_hangul_in_english() {
        let notes = [
            StageNote::NoScan,
            StageNote::ManualEntry,
            StageNote::Read { crops: 12, dropped: 3, blank: 1, skipped: 2 },
            StageNote::Corrected(1),
            StageNote::Corrected(2),
            StageNote::ReadClean,
            StageNote::MocVerified,
            StageNote::MocContradiction,
            StageNote::MocHold,
            StageNote::ChannelDrift { logic: "2oo3".into(), channel: "A".into(), field_agrees: true },
            StageNote::VoteAgrees { logic: "2oo3".into() },
            StageNote::Flatline,
            StageNote::SpikeFiltered,
            StageNote::SignalOk,
            StageNote::DraftModel { aop: "AOP-103-FW02".into(), high_risk: true },
            StageNote::DraftTemplate { aop: "EOP-103-CDU02".into() },
            StageNote::DraftMismatch { aop: "AOP-102-TB01".into(), problems: 2 },
            StageNote::NoProcedure { moc: true },
            StageNote::Stopped,
        ];
        let hangul = |s: &str| s.chars().any(|c| ('가'..='힣').contains(&c));
        for n in &notes {
            assert!(hangul(&n.text(Lang::Ko)), "{n:?}");
            assert!(!hangul(&n.text(Lang::En)), "{n:?} → {}", n.text(Lang::En));
        }
        assert_eq!(StageNote::Read { crops: 12, dropped: 3, blank: 0, skipped: 2 }.text(Lang::Ko), "조각 12개 판독 · 안내문 3개 버림 · 마스터와 같은 조각 2개 건너뜀");
        assert_eq!(StageNote::DraftModel { aop: "AOP-103-FW02".into(), high_risk: true }.text(Lang::Ko), "AOP-103-FW02 초안 · 숫자 대조 통과 · 공동결재 대기");
        // 화면에 나가는 모양은 예전과 같고 (name · actor · summary), 글은 지금 화면 언어입니다
        let r = record(5, 0.0, StageStatus::Done, StageNote::DraftTemplate { aop: "AOP-103-FW02".into() });
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["key"], "stage.draft_template");
        assert_eq!(v["summary"], StageNote::DraftTemplate { aop: "AOP-103-FW02".into() }.text(i18n::current()));
    }
}
