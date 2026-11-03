//! 합성 사건 시나리오 (`fixtures/plant_scenarios.json`, `scripts/gen_plant_cases.py` 가 만듭니다).
//!
//! DCS 연계가 없어서 시나리오를 재생해 알람을 넣습니다 (앱의 DCS 시뮬레이터, 시험의 E2E).
//! 시나리오의 시각은 기준 시각 `t0` 에서 몇 초인지로 적혀 있고, 여기서 실제 시각(epoch ms)으로 바꿉니다.

use std::collections::BTreeMap;

use anyhow::{Context, Result};
use chrono::{DateTime, Duration, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::case::{AlarmEvent, AlarmKind, FieldKind, FieldReport, Incident, IncidentSource, Signature};
use crate::plant::Sector;
use crate::timeseries::{Series, Telemetry};

const EMBEDDED: &str = include_str!("../fixtures/plant_scenarios.json");

#[derive(Debug, Clone, Deserialize)]
pub struct AlarmRow {
    pub tag: String,
    pub channel: Option<String>,
    pub at_s: f64,
    pub value: f64,
    pub kind: String,
    pub message: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScanRow {
    pub kind: String,
    pub at_s: f64,
    pub sector: String,
    #[serde(default)]
    pub drawing: Option<String>,
    #[serde(default)]
    pub rev: Option<String>,
    pub kks: String,
    pub tag: String,
    #[serde(default)]
    pub value: Option<f64>,
    #[serde(default)]
    pub moc_no: Option<String>,
    #[serde(default)]
    pub old: Option<f64>,
    #[serde(default)]
    pub new: Option<f64>,
    pub unit: String,
    pub worker: String,
    pub signed: bool,
    pub loto: bool,
    pub image: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Scenario {
    pub id: String,
    pub title: String,
    pub story: String,
    pub t0: DateTime<chrono::FixedOffset>,
    pub now_s: f64,
    pub alarm: Option<AlarmRow>,
    pub series: BTreeMap<String, Vec<f64>>,
    #[serde(default)]
    pub channels: BTreeMap<String, BTreeMap<String, Vec<f64>>>,
    #[serde(default)]
    pub scans: Vec<ScanRow>,
    #[serde(default)]
    pub expect: Value,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ScenarioSet {
    pub from_s: i64,
    pub period_s: i64,
    pub scenarios: Vec<Scenario>,
}

impl ScenarioSet {
    pub fn embedded() -> Result<Self> {
        serde_json::from_str(EMBEDDED).context("합성 시나리오를 읽을 수 없습니다")
    }

    pub fn get(&self, id: &str) -> Option<&Scenario> {
        self.scenarios.iter().find(|s| s.id == id)
    }
}

fn at(t0: DateTime<Utc>, secs: f64) -> DateTime<Utc> {
    t0 + Duration::milliseconds((secs * 1000.0).round() as i64)
}

impl Scenario {
    pub fn t0_utc(&self) -> DateTime<Utc> {
        self.t0.with_timezone(&Utc)
    }

    /// 시나리오 기준 `secs` 초의 실제 시각
    pub fn time(&self, secs: f64) -> DateTime<Utc> {
        at(self.t0_utc(), secs)
    }

    pub fn telemetry(&self, set: &ScenarioSet) -> Telemetry {
        let start = self.t0_utc().timestamp_millis() + set.from_s * 1000;
        let period = set.period_s * 1000;
        let series = self.series.iter().map(|(tag, v)| (tag.clone(), Series::new(start, period, v.clone()))).collect();
        let channels = self
            .channels
            .iter()
            .map(|(tag, chs)| (tag.clone(), chs.iter().map(|(c, v)| (c.clone(), Series::new(start, period, v.clone()))).collect()))
            .collect();
        Telemetry { series, channels }
    }

    pub fn alarm_event(&self) -> Option<AlarmEvent> {
        let a = self.alarm.as_ref()?;
        Some(AlarmEvent {
            tag: a.tag.clone(),
            channel: a.channel.clone(),
            at: self.time(a.at_s),
            value: a.value,
            kind: AlarmKind::from_key(&a.kind).unwrap_or(AlarmKind::High),
            message: a.message.clone(),
        })
    }

    /// 시나리오의 사건 (현장 보고 없이). 분석 시각은 `now_s` 입니다.
    pub fn incident(&self, set: &ScenarioSet, id: &str) -> Incident {
        let alarm = self.alarm_event();
        Incident {
            id: id.to_string(),
            source: if alarm.is_some() { IncidentSource::Dcs } else { IncidentSource::Field },
            alarm,
            telemetry: self.telemetry(set),
            reports: Vec::new(),
            received_at: self.time(self.now_s),
            scenario: Some(self.id.clone()),
        }
    }

    /// 현장 단말이 보낼 보고 (시나리오의 정답 값). 이름은 원장에서 채웁니다.
    pub fn field_reports(&self, worker_name: impl Fn(&str) -> Option<String>) -> Vec<FieldReport> {
        self.scans
            .iter()
            .map(|s| {
                let kind = if s.kind == "redline" { FieldKind::Redline } else { FieldKind::GaugeCard };
                let mut r = FieldReport::new(kind, self.time(s.at_s));
                r.sector = Sector::from_key(&s.sector);
                r.kks = Some(s.kks.clone());
                r.tag = Some(s.tag.clone());
                r.reading = s.value.or(s.new);
                r.unit = Some(s.unit.clone());
                r.drawing = s.drawing.clone();
                r.drawing_rev = s.rev.clone();
                r.moc_no = s.moc_no.clone();
                r.old_value = s.old;
                r.worker_id = Some(s.worker.clone());
                r.worker_name = worker_name(&s.worker);
                r.signature = s.signed.then(Signature::on_paper);
                r.permit_confirmed = s.loto;
                r
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_scenario_loads_with_full_series() {
        let set = ScenarioSet::embedded().unwrap();
        let db = crate::lattice::Lattice::embedded().unwrap();
        assert_eq!(set.scenarios.len(), 8);
        for s in &set.scenarios {
            assert_eq!(s.series.len(), db.sensors().len(), "{}", s.id);
            assert!(s.series.values().all(|v| v.len() == 151), "{}", s.id);
            assert!(s.alarm.is_some() || !s.scans.is_empty(), "{}: 알람이나 현장 보고가 있어야 합니다", s.id);
        }
    }

    #[test]
    fn scenario_times_become_real_times() {
        let set = ScenarioSet::embedded().unwrap();
        let p1 = set.get("P1").unwrap();
        let inc = p1.incident(&set, "INC-1");
        assert_eq!(inc.received_at.to_rfc3339(), "2026-11-04T05:20:01+00:00");
        let series = inc.telemetry.get("10MAA10CP001").unwrap();
        assert_eq!(series.t0_ms, p1.t0_utc().timestamp_millis() - 60_000);
        assert_eq!(inc.telemetry.channels["10MAA10CP001"].len(), 3);
        let reports = p1.field_reports(|_| Some("이승민".into()));
        assert_eq!(reports[0].taken_at.to_rfc3339(), "2026-11-04T05:21:05+00:00", "기획안의 촬영 시각 14:21:05");
        assert_eq!(reports[0].reading, Some(26.0));
    }
}
