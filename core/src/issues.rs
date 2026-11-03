//! 단일 · 절차 · 복합(다중) 이슈 분류와 분해 (기획 Part 8-6).
//!
//! 알람 하나에 원인 하나를 붙이는 대신, 사건을 하위 이슈로 쪼개 각각 해결 여부를 봅니다.
//! - 단일 이슈(A): 센서 하나의 드리프트 · 동결 · 잡음(A-1), 설비 하나의 국소 이상(A-2), 스캔 한 글자 오독(A-3)
//! - 절차 이슈(B): 서명 · 안전작업허가 불비(B-1), 변경관리(MOC) 원장 불일치(B-2), 선행 절차(인터록) 미완료 · 순서 위반(B-3)
//! - 복합 · 다중 이슈(C): 여러 섹터로 번진 연쇄 파급(C-1), 설비 이상 위에 센서 잡음이 겹침(C-2), 설비 고장에 무입증 승인 시도가 겹침(C-3)
//!
//! 분해는 규칙(분석 결과 · 현장 보고 · 감사 기록)만으로 합니다. 판정과 자동 확정 여부는 바꾸지 않고, 주제어실 화면에
//! "이 사건은 [설비 상류 파급] + [절차 미완료] 가 결합된 건입니다" 처럼 보여 주는 용도입니다. 어느 하나라도 절차 이슈가 열려 있으면
//! 정책(`policy.rs`)과 인터록이 확정을 막습니다.

use serde::Serialize;

use crate::case::FieldReport;
use crate::lattice::Lattice;
use crate::state_machine::analysis::Analysis;
use crate::state_machine::gate1_formal::SignalState;
use crate::state_machine::gate2_consistency::{DriftFinding, MocOutcome};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IssueKind {
    Single,
    Procedural,
    Compound,
}

impl IssueKind {
    pub fn label(self) -> &'static str {
        match self {
            IssueKind::Single => "단일 이슈",
            IssueKind::Procedural => "절차 이슈",
            IssueKind::Compound => "복합·다중 이슈",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SubKind {
    /// 원인 설비 (기계적 고착 · 국소 열화)
    Equipment,
    /// 원인에서 번진 파급 (하류 경보 · 역류 여파)
    Cascade,
    /// 계측기 (드리프트 · 동결 · 잡음)
    Sensor,
    /// 사람의 절차 (서명 · 허가 · 인터록 · 무입증 승인)
    Procedure,
    /// 변경관리 원장
    Moc,
}

impl SubKind {
    fn physical(self) -> bool {
        matches!(self, SubKind::Equipment | SubKind::Cascade | SubKind::Sensor)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SubIssue {
    /// 화면의 `이슈 #n`
    pub n: usize,
    pub kind: SubKind,
    pub kks: Option<String>,
    /// 근본 원인을 0 으로 둔 깊이 (하류 +)
    pub depth: Option<i32>,
    pub title: String,
    pub detail: String,
    /// 처분 · 해소 조건
    pub resolution: String,
    pub resolved: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IssueReport {
    pub kind: IssueKind,
    /// 기획안 이슈 카탈로그 번호 (A-1 ~ C-3), 해당하는 것 모두
    pub codes: Vec<String>,
    pub label: String,
    pub subs: Vec<SubIssue>,
    pub open: usize,
    pub resolved: usize,
}

fn fact<'a>(a: &'a Analysis, key: &str) -> Option<&'a str> {
    a.substantive.as_ref()?.facts.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

fn name_of(db: &Lattice, kks: &str) -> String {
    db.address(kks).map(|x| format!("{} {} {}", x.building, x.floor_text(), x.name)).unwrap_or_else(|| kks.to_string())
}

#[allow(clippy::too_many_arguments)]
fn add(subs: &mut Vec<SubIssue>, kind: SubKind, kks: Option<String>, depth: Option<i32>, title: &str, detail: String, resolution: &str, resolved: bool) {
    let n = subs.len() + 1;
    subs.push(SubIssue { n, kind, kks, depth, title: title.to_string(), detail, resolution: resolution.to_string(), resolved });
}

/// 분석 · 현장 보고 · 감사 기록(이 사건에서 가드레일이 막은 시도)으로 사건을 하위 이슈로 나눕니다.
pub fn decompose(db: &Lattice, a: &Analysis, reports: &[FieldReport], intercepted: &[String]) -> IssueReport {
    let mut subs: Vec<SubIssue> = Vec::new();
    let mut codes: Vec<&str> = Vec::new();
    let aop_step = a.draft.as_ref().and_then(|d| d.steps.first().map(|s| format!("{} 조치: {s}", d.aop_id)));

    // 1) 물리: 근본 설비 · 파급
    if let Some(root) = a.causal_root() {
        let detail = fact(a, "상류 이상 설비").map(str::to_string).unwrap_or_else(|| name_of(db, &root.kks));
        add(&mut subs, SubKind::Equipment, Some(root.kks.clone()), Some(0), "근본 설비 이슈", detail, aop_step.as_deref().unwrap_or("현장 점검"), false);
        let alarm = &a.route.kks;
        if alarm != &root.kks {
            let detail = format!("{} · 원인에서 Δt {:.1}초 뒤 전파 ({}홉)", fact(a, "경보 계측값").unwrap_or(alarm), root.lag_secs, root.hops);
            add(&mut subs, SubKind::Cascade, Some(alarm.clone()), Some(root.hops as i32), "연쇄 파급 이슈", detail, "설비 자체 결함 아님 (상류 원인을 해소하면 함께 해소)", false);
        }
        // 역류 여파: 경로 위 상류 쪽이 원인보다 늦게 변한 것 (원인이 막혀 압력이 거꾸로 오름)
        let causal = a.substantive.as_ref().and_then(|s| s.causal.as_ref());
        for p in causal.map(|c| c.propagation.iter().filter(|p| p.backflow).collect::<Vec<_>>()).unwrap_or_default() {
            add(&mut subs, SubKind::Cascade, Some(p.upstream_kks.clone()), Some(-1), "역류 여파", format!("{} {} 상승 · Δt {:.1}초", name_of(db, &p.upstream_kks), p.upstream_tag, p.observed_lag_secs.abs()), "원인 해소 시 회복", false);
        }
        let sectors: std::collections::BTreeSet<String> = [root.kks.as_str(), alarm.as_str()].iter().filter_map(|k| db.equipment(k).map(|e| e.sector.clone())).collect();
        codes.push(if sectors.len() > 1 { "C-1" } else { "A-2" });
    } else if a.candidate.code() == "G3_VIOLATION_HIGH_RISK" || a.candidate.code() == "G3_ELIGIBLE_NORMAL" {
        // 상류 원인이 없는 단위 설비 이상 (국소 열화) 또는 정상 범위
        if a.candidate.code() == "G3_VIOLATION_HIGH_RISK" {
            let detail = fact(a, "경보 계측값").map(|v| format!("{} · {v}", name_of(db, &a.route.kks))).unwrap_or_else(|| name_of(db, &a.route.kks));
            add(&mut subs, SubKind::Equipment, Some(a.route.kks.clone()), Some(0), "단일 설비 국소 이상", detail, aop_step.as_deref().unwrap_or("예비기 전환 · 현장 점검"), false);
            codes.push("A-2");
        }
    }

    // 2) 계측기
    if let Some(s) = &a.signal {
        match &s.state {
            SignalState::Flatline { secs, value } => {
                add(&mut subs, SubKind::Sensor, Some(a.route.kks.clone()), None, "계측기 동결", format!("{} 가 {secs}초 동안 {value} 에서 움직이지 않음", s.tag), "계측팀 정비 배정 (판정에서 제외)", false);
                codes.push("A-1");
            }
            SignalState::SpikeFiltered { spikes } => {
                add(&mut subs, SubKind::Sensor, Some(a.route.kks.clone()), None, "계측 순간 잡음", format!("{} 의 불가능한 슬루율 표본 {}개를 걸러냄", s.tag, spikes.len()), "걸러낸 값으로 판정 (자동 해소)", true);
                codes.push(if subs.iter().any(|x| x.kind == SubKind::Equipment) { "C-2" } else { "A-1" });
            }
            _ => {}
        }
    }
    if let Some(DriftFinding::Isolated { channel, field_confirmed }) = &a.drift {
        let detail = format!("{} 채널 {channel} 단독 편차{}", a.route.tag.as_deref().unwrap_or(""), if *field_confirmed { " (현장 게이지로 확인)" } else { " (현장 확인 대기)" });
        add(&mut subs, SubKind::Sensor, Some(a.route.kks.clone()), None, "단일 계측기 드리프트", detail, "보팅에서 제외 · 트립 보류 · 계측팀 정비", *field_confirmed);
        codes.push("A-1");
    }

    // 3) 변경관리
    if let Some(m) = &a.moc {
        match &m.outcome {
            MocOutcome::Contradiction { reason } => {
                add(&mut subs, SubKind::Moc, m.ledger.as_ref().map(|l| l.kks.clone()), None, "변경관리(MOC) 절차 위반", reason.clone(), "무증빙 기준치 변경 잠금 · 기술책임자 확인", false);
                codes.push("B-2");
            }
            MocOutcome::Hold { reason } => {
                add(&mut subs, SubKind::Moc, m.ledger.as_ref().map(|l| l.kks.clone()), None, "변경관리 승인 대기", reason.clone(), "승인 · 권한 확인 뒤 반영", false);
                codes.push("B-2");
            }
            MocOutcome::Verified => {
                add(&mut subs, SubKind::Moc, m.ledger.as_ref().map(|l| l.kks.clone()), None, "변경관리 승인 원장 일치", m.moc_no.clone().unwrap_or_default(), "기준치 오버레이 반영", true);
            }
        }
    }

    // 4) 사람의 절차: 현장 서명 · 허가, 선행 절차 인터록, 가드레일이 막은 시도
    for r in reports {
        if !r.uncertain.is_empty() {
            add(&mut subs, SubKind::Procedure, r.kks.clone(), None, "현장 판독 확인 필요", format!("확인할 칸: {}", r.uncertain.join(", ")), "현장 운전원 확인", false);
            codes.push("B-1");
        }
        if !r.corrections.is_empty() && r.uncertain.is_empty() {
            add(&mut subs, SubKind::Sensor, r.kks.clone(), None, "스캔 문서 단일 오독", format!("대장으로 교정 {}건", r.corrections.len()), "자동 교정 후 통과", true);
            codes.push("A-3");
        }
    }
    if let Some(il) = &a.interlock {
        let left: Vec<String> = il.missing().iter().map(|s| format!("Step {} {}", s.no, s.text)).collect();
        let resolved = left.is_empty();
        let detail = if resolved { format!("{} 선행 절차 {}단계 모두 확인", il.aop_id, il.steps.len()) } else { left.join(" / ") };
        add(&mut subs, SubKind::Procedure, Some(il.target.clone()), Some(0), "선행 절차(인터록)", detail, if resolved { "확정 가능" } else { "순서대로 끝나기 전까지 확정 잠금" }, resolved);
        if !resolved {
            codes.push("B-3");
        }
    }
    for text in intercepted {
        let rubber = text.contains("공동 결재 없이") || text.contains("근거가 맞지 않는");
        add(&mut subs, SubKind::Procedure, None, None, if rubber { "무입증 고위험 승인 시도" } else { "절차 위반 시도" }, text.clone(), "가드레일이 차단 (감사 기록)", true);
        codes.push(if rubber && subs.iter().any(|s| s.kind == SubKind::Equipment) { "C-3" } else { "B-3" });
    }

    let physical = subs.iter().filter(|s| s.kind.physical() && !(s.kind == SubKind::Sensor && s.title == "스캔 문서 단일 오독")).count();
    let procedural = subs.iter().filter(|s| matches!(s.kind, SubKind::Procedure | SubKind::Moc) && !(s.kind == SubKind::Moc && s.resolved)).count();
    let cascades = subs.iter().filter(|s| s.kind == SubKind::Cascade).count();
    let kind = if physical >= 2 || (physical >= 1 && procedural >= 1) {
        IssueKind::Compound
    } else if procedural >= 1 {
        IssueKind::Procedural
    } else {
        IssueKind::Single
    };
    let label = match kind {
        IssueKind::Compound => {
            let mut parts = Vec::new();
            if physical > 0 {
                parts.push(if cascades > 0 { format!("설비 파급 {physical}건") } else { format!("설비 · 계측 {physical}건") });
            }
            if procedural > 0 {
                parts.push(format!("절차 {procedural}건"));
            }
            format!("{} ({} 결합)", kind.label(), parts.join(" + "))
        }
        _ => format!("{} ({})", kind.label(), subs.first().map(|s| s.title.clone()).unwrap_or_else(|| "이상 없음".into())),
    };
    let mut codes: Vec<String> = codes.into_iter().map(str::to_string).collect();
    codes.sort();
    codes.dedup();
    let open = subs.iter().filter(|s| !s.resolved).count();
    IssueReport { kind, codes, label, open, resolved: subs.len() - open, subs }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::interlock::{record, Evidence};
    use crate::scenario::ScenarioSet;
    use crate::state_machine::analysis::{analyze, Brains};

    fn analysis(id: &str) -> (Lattice, Analysis) {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get(id).unwrap();
        let incident = s.incident(&set, &format!("T-{id}"));
        let a = analyze(&db, &incident, Brains { model: None, writer: None }, "SDS-T").unwrap();
        (db, a)
    }

    #[test]
    fn a_stuck_valve_with_pending_interlock_is_a_compound_issue() {
        let (db, mut a) = analysis("P2");
        let r = decompose(&db, &a, &[], &[]);
        assert_eq!(r.kind, IssueKind::Compound, "{r:?}");
        assert!(r.codes.contains(&"C-1".to_string()) && r.codes.contains(&"B-3".to_string()), "{:?}", r.codes);
        let root = &r.subs[0];
        assert_eq!((root.kind, root.kks.as_deref(), root.depth), (SubKind::Equipment, Some("10LAB20AA102"), Some(0)));
        assert!(r.subs.iter().any(|s| s.kind == SubKind::Cascade && s.kks.as_deref() == Some("10HAH10BB001") && s.depth == Some(2)), "{:?}", r.subs);
        assert!(r.label.contains("설비 파급") && r.label.contains("절차"), "{}", r.label);
        // 선행 절차를 끝내면 절차 이슈가 해소됩니다
        let il = a.interlock.as_mut().unwrap();
        record(il, 2, Evidence::Value { value: 65.0, how: "현장 무전".into() }).unwrap();
        record(il, 3, Evidence::RadioLoto { supervisor: "OP-201".into() }).unwrap();
        let after = decompose(&db, &a, &[], &[]);
        assert!(after.subs.iter().filter(|s| s.kind == SubKind::Procedure).all(|s| s.resolved));
        assert!(!after.codes.contains(&"B-3".to_string()));
    }

    #[test]
    fn a_single_channel_drift_is_a_single_sensor_issue() {
        let (db, a) = analysis("P1");
        let r = decompose(&db, &a, &[], &[]);
        assert_eq!(r.kind, IssueKind::Single, "{r:?}");
        assert_eq!(r.codes, vec!["A-1".to_string()]);
        assert_eq!(r.subs[0].kind, SubKind::Sensor);
    }

    #[test]
    fn a_bearing_fault_without_upstream_cause_is_a_local_equipment_issue_and_a_rubber_stamp_attempt_makes_it_compound() {
        let (db, a) = analysis("P4");
        let r = decompose(&db, &a, &[], &[]);
        assert_eq!((r.kind, r.codes.clone()), (IssueKind::Single, vec!["A-2".to_string()]), "{r:?}");
        let r = decompose(&db, &a, &[], &["고위험 조치 절차서를 공동 결재 없이 확정 시도 → 차단".to_string()]);
        assert_eq!(r.kind, IssueKind::Compound);
        assert!(r.codes.contains(&"C-3".to_string()), "{:?}", r.codes);
    }

    #[test]
    fn a_frozen_transmitter_is_a_single_sensor_issue() {
        let (db, a) = analysis("P6");
        let r = decompose(&db, &a, &[], &[]);
        assert_eq!(r.kind, IssueKind::Single);
        assert!(r.subs.iter().any(|s| s.title == "계측기 동결"), "{:?}", r.subs);
    }
}
