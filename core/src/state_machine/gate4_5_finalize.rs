//! Gate 4: 조치 절차서(AOP) 문서 · Gate 5: 확정 기록과 블랙박스 봉인 (기획 Part 8 5절 Stage 5).
//!
//! - 절차서 고르기: 원인 태그 → 경보 태그 순으로 표준 절차서를 찾습니다. 계측 이상은 공통 절차서입니다.
//! - 템플릿 요약: 사실 줄로만 만든 원인 진단 요약입니다. 모델 글이 숫자 대조에 걸리면 이 글을 씁니다.
//! - 현장 단말로 가는 글: 내부 판정 코드 · 확신도 · 다른 근무자 실명이 섞이면 막습니다 (Part 8-2 보안 불변식).
//! - 보관 행: 확정 코드 · 주제어실 문서 · 현장 안내 · 봉인 해시 · 보존 기한(5년)

use chrono::{DateTime, Months, NaiveDate, Utc};
use serde::Serialize;

use crate::causal::CausalReport;
use crate::lattice::{AopRow, Lattice};
use crate::plant::fmt_value;
use crate::privacy::{find_leaks, FIELD_FORBIDDEN};
use crate::verdict::Verdict;

/// 보존 기간 (년)
pub const RETENTION_YEARS: u32 = 5;
/// 공통 절차서
pub const AOP_FLATLINE: &str = "AOP-COM-IC02";
pub const AOP_SPIKE: &str = "AOP-COM-EMI01";

/// 판정과 원인으로 표준 절차서를 고릅니다. 승인된 변경 반영 · 위조 잠금에는 절차서가 없습니다.
pub fn select_aop<'a>(db: &'a Lattice, verdict: Verdict, alarm_tag: Option<&str>, causal: Option<&CausalReport>) -> Option<&'a AopRow> {
    match verdict {
        Verdict::G1SensorFlatline => db.aop(AOP_FLATLINE),
        Verdict::G1SpikeFiltered | Verdict::G3EligibleNormal => db.aop(AOP_SPIKE),
        Verdict::G2SensorDriftIsolated | Verdict::G2FieldRecheckRequired => alarm_tag.and_then(|t| db.aop_for(&[t])).or_else(|| db.aop(AOP_FLATLINE)),
        Verdict::G3AdverseUpstreamCause => {
            let root = causal.and_then(|c| c.root.as_ref()).map(|r| r.tag.as_str());
            let tags: Vec<&str> = root.into_iter().chain(alarm_tag).collect();
            db.aop_for(&tags)
        }
        Verdict::G3ViolationHighRisk => alarm_tag.and_then(|t| db.aop_for(&[t])),
        _ => None,
    }
}

/// 사실 줄로만 만든 원인 진단 요약
pub fn template_summary(db: &Lattice, verdict: Verdict, alarm_tag: Option<&str>, causal: Option<&CausalReport>, now_value: Option<f64>, root_value: Option<f64>) -> String {
    let sensor = alarm_tag.and_then(|t| db.sensor(t));
    let place = |kks: &str| db.address(kks).map(|a| format!("{} {} {}({kks})", a.building, a.floor_text(), a.name)).unwrap_or_else(|| kks.to_string());
    let alarm = sensor.map(|s| match now_value {
        Some(v) => format!("{} {} 가 {} {}", place(&s.kks), s.label, fmt_value(v), s.unit),
        None => format!("{} {}", place(&s.kks), s.label),
    });
    match (verdict, causal.and_then(|c| c.root.as_ref())) {
        (Verdict::G3AdverseUpstreamCause, Some(root)) => {
            let up = db.sensor(&root.tag);
            let cause = match (up, root_value) {
                (Some(s), Some(v)) => format!("{} {} 가 {} {} (정상 {} {})", place(&root.kks), s.label, fmt_value(v), s.unit, fmt_value(s.normal), s.unit),
                _ => place(&root.kks),
            };
            format!(
                "{} 입니다. {}홉 상류의 {} 로 바뀐 {:.1}초 뒤 하류가 변하기 시작해, 상류 설비 이상이 원인으로 보입니다.",
                alarm.unwrap_or_default(),
                root.hops,
                cause,
                root.lag_secs
            )
        }
        (Verdict::G3ViolationHighRisk, _) => format!("{} 입니다. 상류 3홉 안에서 앞서 변한 설비가 없어 단위 설비 단독 이상으로 보입니다.", alarm.unwrap_or_default()),
        (Verdict::G2SensorDriftIsolated, _) => format!("{} 로 표시되지만 다른 채널과 현장 게이지는 정상이라 전송기 단독 편차로 보입니다.", alarm.unwrap_or_default()),
        (Verdict::G1SensorFlatline, _) => format!("{} 에서 값이 바뀌지 않아 계측기 동결로 보입니다.", alarm.unwrap_or_default()),
        (Verdict::G1SpikeFiltered | Verdict::G3EligibleNormal, _) => format!("{} 의 순간 값은 물리적으로 불가능한 잡음이라 걸러 냈고, 걸러 낸 값은 정상 범위입니다.", sensor.map(|s| place(&s.kks)).unwrap_or_default()),
        _ => alarm.unwrap_or_default(),
    }
}

/// 현장 단말로 가는 글의 누출 검사. 내부 용어와 `extra`(다른 근무자 실명 등)가 있으면 막습니다.
pub fn check_field_text(text: &str, extra: &[String]) -> Result<(), (Verdict, usize)> {
    let mut found = find_leaks(text, FIELD_FORBIDDEN).len();
    found += find_leaks(text, extra).len();
    if found == 0 {
        Ok(())
    } else {
        Err((Verdict::G4BlockInternalLeak, found))
    }
}

/// 현장 단말에 보여 줄 확정 안내. 내부 판정은 넣지 않습니다.
pub fn field_notice(incident_id: &str, final_code: &str) -> String {
    let body = if final_code.contains("G2_MOC_VERIFIED") {
        "제출한 설비 변경 내용이 승인 기록과 확인되어 주제어실 기준치에 반영되었습니다."
    } else if final_code.contains("G2_REJECT_CONTRADICTION_LOCK") {
        "제출한 변경 내용을 승인 기록에서 확인하지 못했습니다. 기술책임자에게 변경 승인 여부를 확인해 주세요."
    } else if final_code.starts_with("SUPERVISOR_REJECT") {
        "주제어실에서 제출 내용을 반려했습니다. 필요하면 다시 점검해 주세요."
    } else {
        "주제어실이 현장 보고를 확인하고 조치를 확정했습니다. 추가 지시는 무전으로 안내합니다."
    };
    format!("처리 결과 안내 (접수 번호 {incident_id})\n{body}")
}

/// Gate 5 보관 행
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ArchiveRow {
    pub archive_id: String,
    pub incident_id: String,
    pub title: String,
    pub final_code: String,
    /// 주제어실 결재 문서 (원인 진단 · 조치 지시 · 결재)
    pub mcr_doc: String,
    /// 현장 단말 안내
    pub field_notice: String,
    /// 근거 없는 판정 뒤집기 등 감사실 보고 표시
    pub special_audit_flag: bool,
    /// SDS 블랙박스 봉인 해시
    pub seal: String,
    pub auto: bool,
    pub closed_at: DateTime<Utc>,
    pub retention_until: NaiveDate,
    pub scenario: Option<String>,
}

pub fn retention_until(today: NaiveDate) -> NaiveDate {
    today.checked_add_months(Months::new(12 * RETENTION_YEARS)).unwrap_or(today)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::causal::trace_root_cause;
    use crate::scenario::ScenarioSet;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    #[test]
    fn procedures_follow_the_cause_first() {
        let db = db();
        let set = ScenarioSet::embedded().unwrap();
        let p2 = set.get("P2").unwrap().incident(&set, "i");
        let causal = trace_root_cause(&db, &p2.telemetry, "10HAH10CT001", p2.now_ms());
        assert_eq!(select_aop(&db, Verdict::G3AdverseUpstreamCause, Some("10HAH10CT001"), Some(&causal)).unwrap().id, "AOP-103-FW02");
        assert_eq!(select_aop(&db, Verdict::G3ViolationHighRisk, Some("10MAD10CY001"), None).unwrap().id, "AOP-102-TB01");
        assert_eq!(select_aop(&db, Verdict::G2SensorDriftIsolated, Some("10MAA10CP001"), None).unwrap().id, "AOP-102-IC01");
        assert_eq!(select_aop(&db, Verdict::G1SensorFlatline, Some("10MAV10CT001"), None).unwrap().id, AOP_FLATLINE);
        assert!(select_aop(&db, Verdict::G2MocVerified, None, None).is_none());
        let text = template_summary(&db, Verdict::G3AdverseUpstreamCause, Some("10HAH10CT001"), Some(&causal), Some(631.0), Some(18.0));
        assert!(text.contains("10LAB20AA102") && text.contains("18.0") && text.contains("2홉"), "{text}");
    }

    #[test]
    fn field_text_with_internal_terms_or_other_names_is_blocked() {
        assert!(check_field_text(&field_notice("F-1", "G2_MOC_VERIFIED"), &[]).is_ok());
        assert!(check_field_text("G3_VIOLATION_HIGH_RISK 확신도 0.97", &[]).is_err());
        assert!(check_field_text("김태현 엔지니어의 변경", &["김태현".to_string()]).is_err());
        assert_eq!(retention_until(NaiveDate::from_ymd_opt(2026, 11, 4).unwrap()), NaiveDate::from_ymd_opt(2031, 11, 4).unwrap());
    }
}
