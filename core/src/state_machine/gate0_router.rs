//! Gate 0: 알람 라우팅과 도면 좌표 즉시 호출 (기획 Part 8 5절 Stage 1).
//!
//! 알람 태그를 받으면 원장에서 소속 섹터(Lv2) · 층(Lv3) · 도면(Lv4) · 도면 안 좌표(`norm_bbox`)를 메모리에서 바로 꺼내 화면에 핀포인팅합니다.
//! 분류는 원장이 정합니다. 모델에는 묻지 않습니다.

use anyhow::{anyhow, Result};
use serde::Serialize;

use crate::case::Incident;
use crate::lattice::{Lattice, SpatialAddress};
use crate::plant::Sector;
use crate::verdict::Verdict;

pub fn verdict_of(sector: Sector) -> Verdict {
    match sector {
        Sector::Boiler => Verdict::Sec01Boiler,
        Sector::Turbine => Verdict::Sec02Turbine,
        Sector::Feedwater => Verdict::Sec03Feedwater,
        Sector::Electric => Verdict::Sec04Electric,
        Sector::EnvSafety => Verdict::Sec05EnvSafety,
    }
}

/// 라우팅 결과
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Route {
    pub sector: Sector,
    pub verdict: Verdict,
    /// 알람 · 보고가 가리키는 센서 태그
    pub tag: Option<String>,
    pub kks: String,
    pub address: SpatialAddress,
}

/// 사건이 가리키는 설비를 찾아 공간 주소를 붙입니다.
pub fn route(db: &Lattice, incident: &Incident) -> Result<Route> {
    let tag = incident.focus_tag().map(str::to_string);
    let kks = match tag.as_deref().and_then(|t| db.sensor(t)) {
        Some(s) => s.kks.clone(),
        None => incident.reports.iter().find_map(|r| r.kks.clone()).ok_or_else(|| anyhow!("사건이 가리키는 설비가 없습니다: {}", incident.id))?,
    };
    let address = db.address(&kks).ok_or_else(|| anyhow!("원장에 없는 설비입니다: {kks}"))?;
    let sector = Sector::from_key(&address.sector).ok_or_else(|| anyhow!("모르는 섹터입니다: {}", address.sector))?;
    Ok(Route { sector, verdict: verdict_of(sector), tag, kks, address })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scenario::ScenarioSet;

    #[test]
    fn the_superheater_alarm_is_pinpointed_on_the_boiler_drawing() {
        let db = Lattice::embedded().unwrap();
        let set = ScenarioSet::embedded().unwrap();
        let inc = set.get("P2").unwrap().incident(&set, "INC-P2");
        let r = route(&db, &inc).unwrap();
        assert_eq!(r.sector, Sector::Boiler);
        assert_eq!(r.verdict.code(), "SEC_01_BOILER");
        assert_eq!(r.address.drawing, "PID-101-HAD");
        assert_eq!(r.address.floor, "5F");
        assert_eq!(r.address.bbox, db.equipment("10HAH10BB001").unwrap().bbox);
    }
}
