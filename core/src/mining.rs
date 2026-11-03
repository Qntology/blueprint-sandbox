//! 팔란티어식 프로세스 마이닝 오버레이 (기획 Part 8-1B).
//!
//! 새 엔진 없이 지금 가진 기록에서 바로 계산합니다.
//! - 간선 굵기 = 실시간 흐름: 흐름 센서가 있는 간선은 `현재값 / 정상치`. 20% 넘게 줄면 병목입니다 (예: 급수 1,120 / 1,850 t/h = -39.5%).
//! - 신호 전파 Δt: 근본 원인 경로의 시차를 간선에 찍습니다.
//! - 의사결정 병목: 현장 게이지를 기다리는 사건(HOLD)과 공동 결재를 기다리는 사건의 대기 시간 (`세션이 열린 시각` 부터).
//! - 의사결정 레인: 사건이 지금 어느 게이트 단계에 머무는지 셉니다.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::lattice::Lattice;
use crate::plant::fmt_value;
use crate::state_machine::analysis::Analysis;
use crate::store::{SessionStatus, Store};
use crate::timeseries::Telemetry;

/// 병목으로 보는 흐름 비율
pub const BOTTLENECK_RATIO: f64 = 0.8;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EdgeFlow {
    pub from: String,
    pub to: String,
    pub medium: String,
    pub label: String,
    pub flow_tag: Option<String>,
    pub nominal: Option<f64>,
    pub current: Option<f64>,
    pub unit: Option<String>,
    /// 현재 / 정상 (흐름 센서가 없으면 1.0)
    pub ratio: f64,
    pub bottleneck: bool,
    /// 근본 원인 경로 위의 간선이면 그 경로의 관측 시차 (초)
    pub lag_secs: Option<f64>,
    pub on_path: bool,
}

/// 시각 `at_ms` 의 간선 흐름. 계측이 없으면 모두 정상 흐름으로 봅니다.
pub fn edge_flows(db: &Lattice, telemetry: Option<&Telemetry>, at_ms: i64, analysis: Option<&Analysis>) -> Vec<EdgeFlow> {
    let root = analysis.and_then(|a| a.causal_root());
    db.edges()
        .iter()
        .map(|e| {
            let sensor = e.flow_tag.as_deref().and_then(|t| db.sensor(t));
            let current = e.flow_tag.as_deref().and_then(|t| telemetry?.value_at(t, at_ms));
            let nominal = sensor.map(|s| s.normal);
            let ratio = match (current, nominal) {
                (Some(c), Some(n)) if n.abs() > f64::EPSILON => c / n,
                _ => 1.0,
            };
            let on_path = root.is_some_and(|r| r.path.windows(2).any(|w| w[0] == e.from && w[1] == e.to));
            let lag_secs = root.filter(|r| on_path && r.path.first() == Some(&e.from)).map(|r| r.lag_secs);
            EdgeFlow {
                from: e.from.clone(),
                to: e.to.clone(),
                medium: e.medium.clone(),
                label: e.label.clone(),
                flow_tag: e.flow_tag.clone(),
                nominal,
                current,
                unit: sensor.map(|s| s.unit.clone()),
                ratio,
                bottleneck: ratio < BOTTLENECK_RATIO,
                lag_secs,
                on_path,
            }
        })
        .collect()
}

/// 병목 카드 (우측 분석기)
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Bottleneck {
    /// "physical" (물리 계통) 또는 "decision" (의사결정 파이프라인)
    pub kind: &'static str,
    pub title: String,
    pub incident: String,
    pub kks: Option<String>,
    pub lines: Vec<String>,
    /// 대기 시간 (초, 의사결정 병목)
    pub waiting_secs: Option<i64>,
}

fn mmss(secs: i64) -> String {
    format!("{:02}분 {:02}초", secs / 60, secs % 60)
}

/// 진행 중 사건의 병목. 물리 병목은 흐름 손실이 있는 원인 경로, 의사결정 병목은 현장 · 결재 대기입니다.
pub fn bottlenecks(db: &Lattice, store: &Store, now: DateTime<Utc>) -> Vec<Bottleneck> {
    let mut out = Vec::new();
    for s in store.sessions() {
        let a = &s.analysis;
        let id = s.incident.id.clone();
        if let Some(root) = a.causal_root() {
            let flows = edge_flows(db, Some(&s.incident.telemetry), s.incident.now_ms(), Some(a));
            if let Some(f) = flows.iter().find(|f| f.on_path && f.bottleneck) {
                let place = db.address(&root.kks).map(|x| format!("{} {} {}", x.building, x.floor_text(), x.name)).unwrap_or_default();
                let (c, n) = (f.current.unwrap_or(0.0), f.nominal.unwrap_or(0.0));
                let unit = f.unit.clone().unwrap_or_default();
                let up_now = db.sensor(&root.tag).and_then(|sr| Some((sr, s.incident.telemetry.value_at(&root.tag, s.incident.now_ms())?)));
                let mut lines = vec![format!("위치: {place}"), format!("KKS: {}", root.kks)];
                if let Some((sr, v)) = up_now {
                    lines.push(format!("현상: {} {} {}", sr.label, fmt_value(v), sr.unit));
                }
                lines.push(format!("병목 손실: {} {unit} ({:+.1}%)", fmt_value(c - n), (f.ratio - 1.0) * 100.0));
                let alarm = db.address(&a.route.kks).map(|x| format!("{} {}", x.building, x.name)).unwrap_or_default();
                lines.push(format!("전파 지연: {:.1}초 뒤 {alarm} 이상", root.lag_secs));
                out.push(Bottleneck { kind: "physical", title: "물리 계통 병목".into(), incident: id.clone(), kks: Some(root.kks.clone()), lines, waiting_secs: None });
            }
        }
        let waited = (now - s.opened_at).num_seconds().max(0);
        if s.status == SessionStatus::AwaitingField {
            out.push(Bottleneck {
                kind: "decision",
                title: "의사결정 병목 · 현장 확인 대기".into(),
                incident: id.clone(),
                kks: Some(a.route.kks.clone()),
                lines: vec![format!("위치: Gate 2 정합성 심사 · {}", a.route.address.building), format!("사유: {}", s.reason), format!("대기 시간: {} 경과", mmss(waited))],
                waiting_secs: Some(waited),
            });
        } else if a.high_risk() {
            out.push(Bottleneck {
                kind: "decision",
                title: "결재 병목 · 고위험 공동 결재 대기".into(),
                incident: id.clone(),
                kks: Some(a.route.kks.clone()),
                lines: vec![
                    format!("절차서: {}", a.draft.as_ref().map(|d| d.aop_id.as_str()).unwrap_or("-")),
                    "policy: 고위험 조치는 교대감독 파트장 공동 결재 필수".into(),
                    format!("대기 시간: {} 경과", mmss(waited)),
                ],
                waiting_secs: Some(waited),
            });
        }
    }
    out
}

/// 의사결정 프로세스 마이닝 레인 한 칸
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lane {
    pub key: &'static str,
    pub label: &'static str,
    pub count: usize,
    /// 이 칸에 머무는 사건 중 가장 오래 기다린 초
    pub max_wait_secs: i64,
    /// 이 단계의 평균 처리 시간 (ms, 기록된 단계 시간)
    pub mean_ms: f64,
    pub incidents: Vec<String>,
}

pub fn lanes(store: &Store, now: DateTime<Utc>) -> Vec<Lane> {
    let sessions: Vec<_> = store.sessions().collect();
    let stage_ms = |n: usize| {
        let xs: Vec<f64> = sessions.iter().filter_map(|s| s.analysis.stages.get(n).map(|r| r.ms)).filter(|ms| *ms > 0.0).collect();
        if xs.is_empty() {
            0.0
        } else {
            xs.iter().sum::<f64>() / xs.len() as f64
        }
    };
    let pick = |f: &dyn Fn(&&crate::store::Session) -> bool| -> (usize, i64, Vec<String>) {
        let hit: Vec<&&crate::store::Session> = sessions.iter().filter(|s| f(s)).collect();
        let wait = hit.iter().map(|s| (now - s.opened_at).num_seconds()).max().unwrap_or(0).max(0);
        (hit.len(), wait, hit.iter().map(|s| s.incident.id.clone()).collect())
    };
    let total = sessions.len() + store.archive().len();
    let (field_n, field_w, field_ids) = pick(&|s| s.status == SessionStatus::AwaitingField);
    let (sign_n, sign_w, sign_ids) = pick(&|s| s.status == SessionStatus::SupervisorReview);
    vec![
        Lane { key: "intake", label: "알람 유입", count: total, max_wait_secs: 0, mean_ms: 0.0, incidents: Vec::new() },
        Lane { key: "gate01", label: "Gate 0~1 형식", count: total, max_wait_secs: 0, mean_ms: stage_ms(0) + stage_ms(1), incidents: Vec::new() },
        Lane { key: "gate2", label: "Gate 2 정합성 (현장 대기)", count: field_n, max_wait_secs: field_w, mean_ms: stage_ms(2), incidents: field_ids },
        Lane { key: "gate3", label: "Gate 3 마인드맵 + Zoro-1", count: sessions.len(), max_wait_secs: 0, mean_ms: stage_ms(3), incidents: Vec::new() },
        Lane { key: "gate45", label: "Gate 4~5 AOP 초안 · Factcheck", count: sessions.iter().filter(|s| s.analysis.draft.is_some()).count(), max_wait_secs: 0, mean_ms: stage_ms(4), incidents: Vec::new() },
        Lane { key: "approval", label: "교대감독 최종결재 대기", count: sign_n, max_wait_secs: sign_w, mean_ms: 0.0, incidents: sign_ids },
        Lane { key: "done", label: "처리 완료", count: store.archive().len(), max_wait_secs: 0, mean_ms: 0.0, incidents: store.archive().iter().map(|a| a.incident_id.clone()).collect() },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::AutomationPolicy;
    use crate::scenario::ScenarioSet;
    use crate::state_machine::Engine;

    #[test]
    fn the_stuck_valve_starves_the_feedwater_edge_by_about_forty_percent() {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let s = set.get("P2").unwrap();
        let mut e = Engine::new(b"s", AutomationPolicy::default(), None);
        e.run(&db, s.incident(&set, "INC-P2"), s.time(s.now_s)).unwrap();
        let a = &e.store.active("INC-P2").unwrap().analysis;
        let inc = &e.store.active("INC-P2").unwrap().incident;
        let flows = edge_flows(&db, Some(&inc.telemetry), inc.now_ms(), Some(a));
        let fw = flows.iter().find(|f| f.from == "10LAB20AA102" && f.to == "10HAD10BB001").unwrap();
        assert!(fw.bottleneck && fw.on_path);
        assert!((fw.ratio - 1120.0 / 1850.0).abs() < 0.02, "{}", fw.ratio);
        assert!(fw.lag_secs.is_some());
        let normal = flows.iter().find(|f| f.from == "10BBA10CB001").unwrap();
        assert!(!normal.bottleneck);

        let later = s.time(s.now_s + 222.0);
        let b = bottlenecks(&db, &e.store, later);
        assert!(b.iter().any(|x| x.kind == "physical" && x.lines.iter().any(|l| l.contains("-39.") || l.contains("-40."))), "{b:?}");
        assert!(b.iter().any(|x| x.kind == "decision" && x.title.contains("결재")), "고위험 공동 결재 대기");
        let l = lanes(&e.store, later);
        assert_eq!(l.iter().find(|x| x.key == "approval").unwrap().count, 1);
    }
}
