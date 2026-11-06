//! 도메인 모드: 1.2GW 화력발전소 ↔ 100MW AI 데이터센터 (추가 개선안 Part 1 · 4).
//!
//! 코어 엔진(5-Gate · Δt 역추적 · SDS · 잉크 차분 · 물리엔진)은 두 도메인이 같고, 다른 것은 데이터입니다.
//! 원장 시드(`fixtures/synthetic_db.json` · `fixtures/aidc_db.json`)가 설비 · 센서 · 간선 · 절차서와 함께
//! 코드 체계(KKS · DCIM), 화면 · 글쓰기 용어([`Terms`]), 물리엔진 계통([`PhysicsConfig`])을 들고 옵니다.
//!
//! 엔진이 도메인 때문에 바뀐 곳은 셋입니다 (개선안의 "코어는 한 줄도 바꿀 필요 없음" 은 사실이 아니었습니다).
//! - 설비 코드: KKS 자리 꼴(`99AAA99AA999`)이 코드에 박혀 있어 DCIM(`AA9AAA99AA99`)을 못 읽었습니다 → [`crate::plant::CodeScheme`]
//! - 섹터 키 · 매체: 발전소 키만 받았습니다 → 다섯 라우팅 자리에 구역 키를 함께 받고, 냉각수 · 냉수 등 매체와 방향 규칙을 더했습니다.
//! - 물리엔진 태그 · 상수와 글쓰기 지시문의 "발전소 주제어실": 상수였습니다 → 원장 데이터로 옮겼습니다.

use serde::{Deserialize, Serialize};

/// 관제 대상 도메인
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DomainMode {
    /// 1.2GW 초초임계압 석탄화력발전소
    #[default]
    PowerPlant,
    /// 100MW 하이퍼스케일 AI 데이터센터 (DLC 수냉 GPU 클러스터)
    #[serde(rename = "aidc")]
    AiDataCenter,
}

impl DomainMode {
    pub const ALL: [DomainMode; 2] = [DomainMode::PowerPlant, DomainMode::AiDataCenter];

    pub fn key(self) -> &'static str {
        match self {
            DomainMode::PowerPlant => "power_plant",
            DomainMode::AiDataCenter => "aidc",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        match key.trim() {
            "power_plant" | "plant" | "powerplant" => Some(DomainMode::PowerPlant),
            "aidc" | "ai_data_center" | "datacenter" => Some(DomainMode::AiDataCenter),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            DomainMode::PowerPlant => "1.2GW 화력발전소",
            DomainMode::AiDataCenter => "100MW AI 데이터센터",
        }
    }

    /// 저장소에 든 원장 시드
    pub fn seed_json(self) -> &'static str {
        match self {
            DomainMode::PowerPlant => include_str!("../fixtures/synthetic_db.json"),
            DomainMode::AiDataCenter => include_str!("../fixtures/aidc_db.json"),
        }
    }

    /// 저장소에 든 합성 사건 시나리오
    pub fn scenarios_json(self) -> &'static str {
        match self {
            DomainMode::PowerPlant => include_str!("../fixtures/plant_scenarios.json"),
            DomainMode::AiDataCenter => include_str!("../fixtures/aidc_scenarios.json"),
        }
    }

    /// 시나리오 번호 머리 (`P1` · `D1`)
    pub fn scenario_prefix(self) -> &'static str {
        match self {
            DomainMode::PowerPlant => "P",
            DomainMode::AiDataCenter => "D",
        }
    }
}

/// 화면 · 문서 · 글쓰기 지시문에 쓰는 도메인 용어. 원장 시드의 `plant.terms` 에서 옵니다 (없으면 발전소 용어).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct Terms {
    /// 설비 코드 이름 (`KKS` · `자산코드`)
    pub code: String,
    /// 표준 절차서 (`AOP` · `EOP`)
    pub procedure: String,
    /// 표준 절차서 긴 이름
    pub procedure_long: String,
    /// 변경 관리 문서 (`MOC` · `MOP`)
    pub change: String,
    /// 계측 · 제어 시스템 (`DCS` · `BMC·DCIM`)
    pub telemetry: String,
    /// 관제실 (`주제어실` · `NOC 관제실`)
    pub control_room: String,
    /// 계통 도면 (`P&ID` · `MEP·SLD`)
    pub drawing: String,
    /// 시설 (`발전소` · `AI 데이터센터`)
    pub site: String,
    /// 현장 작업자 (`현장 운전원` · `현장 엔지니어`)
    pub field_worker: String,
    /// 보호 정지 (`트립` · `셧다운`)
    pub trip: String,
    /// 확정 권한자 (두 도메인 모두 `교대감독`: 데이터센터 NOC 도 교대 근무 감독이 확정합니다)
    pub supervisor: String,
    /// 보관 문서 번호 머리 (`MCR` 주제어실 · `NOC` 관제실)
    pub archive_prefix: String,
    /// 안전작업 확인 (`PTW · LOTO` · `MOP · LoTo`)
    pub permit: String,
    /// 우회 완화 조치 이름 (`급수 바이패스` · `CDU 우회밸브`)
    pub bypass: String,
    /// 콘솔 · 단말 제목
    pub console_title: String,
    pub kiosk_title: String,
}

impl Default for Terms {
    fn default() -> Self {
        Self {
            code: "KKS".into(),
            procedure: "AOP".into(),
            procedure_long: "비정상 운전절차서(AOP)".into(),
            change: "MOC".into(),
            telemetry: "DCS".into(),
            control_room: "주제어실".into(),
            drawing: "P&ID".into(),
            site: "발전소".into(),
            field_worker: "현장 운전원".into(),
            trip: "트립".into(),
            supervisor: "교대감독".into(),
            archive_prefix: "MCR".into(),
            permit: "PTW · LOTO".into(),
            bypass: "급수 바이패스".into(),
            console_title: "1.2GW 발전소 관제 콘솔".into(),
            kiosk_title: "1.2GW 발전소 현장 순시 · 정비 단말".into(),
        }
    }
}

impl Terms {
    /// 조치 절차서 초안을 쓰는 모델의 지시문
    pub fn drafting_system(&self) -> String {
        format!(
            "{} {}의 {} 초안 작성 담당입니다. 주어진 사실만 씁니다. 사실에 없는 숫자, 설비 코드, 시각을 만들지 않습니다. 영문 판정 코드를 쓰지 않습니다.",
            self.site, self.control_room, self.procedure_long
        )
    }

    /// 현장 자유 서술 소명을 분류하는 모델의 지시문
    pub fn excuse_system(&self) -> String {
        format!("{} {}의 편차 소명을 표준 코드로 분류합니다. 코드 하나만 답합니다.", self.site, self.field_worker)
    }

    /// Gate 3 판정 질문의 제목
    pub fn decision_title(&self) -> String {
        format!("{} 계통 이상 근본 원인 및 처분 판정", self.site)
    }
}

/// 내장 물리엔진이 푸는 계통 한 줄 (추가 개선안 2.2절의 `SimConfig`). 원장 시드의 `physics` 에서 옵니다.
///
/// 발전소: 주급수펌프 → 급수조절밸브 · 바이패스 → 수냉벽 → 과열기.
/// AI 데이터센터: CDU 2차 순환펌프 → 2차측 유량조절밸브 · 우회밸브 → 랙 매니폴드 → GPU 콜드플레이트 (식은 같고 상수만 다릅니다).
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct PhysicsConfig {
    /// 주 조절밸브 · 우회밸브 설비 코드
    pub valve: String,
    pub bypass: String,
    /// 밸브 개도 · 우회 개도 · 유량 · 펌프 토출압 · 하류 온도 센서
    pub tag_valve: String,
    pub tag_bypass: String,
    pub tag_flow: String,
    pub tag_pump: String,
    pub tag_hot: String,
    /// 설계 유량과 정상 개도, 우회 밸브 유량 계수
    pub qd: f64,
    pub u0: f64,
    pub kb: f64,
    /// 정상 토출압과, 유량 40 % 감소에 오르는 토출압
    pub p0: f64,
    pub p_gain: f64,
    /// 하류 정상 온도 · 입구(냉각) 기준 온도 · 열용량
    pub t0: f64,
    pub tin: f64,
    pub m: f64,
    /// 밸브 → 하류 이류 지연 (밀리초)
    pub advection_ms: i64,
    /// 물리엔진 화면에 그릴 태그
    pub shown: Vec<String>,
    /// 센서 바이어스 고장(전송기 드리프트) 기본 주입
    pub drift_tag: String,
    pub drift_bias: f64,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            valve: "10LAB20AA102".into(),
            bypass: "10LAB20AA103".into(),
            tag_valve: "10LAB20CG102".into(),
            tag_bypass: "10LAB20CG103".into(),
            tag_flow: "10LAB20CF001".into(),
            tag_pump: "10LAB10CP001".into(),
            tag_hot: "10HAH10CT001".into(),
            qd: 1850.0,
            u0: 0.78,
            kb: 0.42,
            p0: 28.5,
            p_gain: 0.9,
            t0: 610.0,
            tin: 300.0,
            m: 255_922.0,
            advection_ms: 4200,
            shown: ["10LAB20CG102", "10LAB20CG103", "10LAB20CF001", "10LAB10CP001", "10HAH10CT001"].map(String::from).to_vec(),
            drift_tag: "10MAA10CP001".into(),
            drift_bias: -11.8,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_round_trip_and_default_terms_are_the_plant() {
        for m in DomainMode::ALL {
            assert_eq!(DomainMode::from_key(m.key()), Some(m));
        }
        assert_eq!(serde_json::to_string(&DomainMode::AiDataCenter).unwrap(), "\"aidc\"");
        let t = Terms::default();
        assert_eq!(t.drafting_system(), "발전소 주제어실의 비정상 운전절차서(AOP) 초안 작성 담당입니다. 주어진 사실만 씁니다. 사실에 없는 숫자, 설비 코드, 시각을 만들지 않습니다. 영문 판정 코드를 쓰지 않습니다.");
        assert_eq!(t.decision_title(), "발전소 계통 이상 근본 원인 및 처분 판정");
    }
}
