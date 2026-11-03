//! 사건 한 건의 입력 모델. DCS 알람, 계측 시계열, 현장 단말이 보낸 점검표 · Red-line 이 이 구조로 모입니다.
//!
//! 근무자 이름은 `FieldReport::worker_name` 에만 있습니다. 모델에 주는 사실에는 넣지 않고, 주제어실 화면에는 가려서 보여 줍니다.
//! `Incident` 가 버려질 때 이름과 자유 서술은 0 으로 덮어씁니다 (zeroize).

use std::fmt;

use chrono::{DateTime, Utc};
use serde::Serialize;
use zeroize::Zeroize;

use crate::plant::Sector;
use crate::timeseries::Telemetry;

/// 알람의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum AlarmKind {
    High,
    Low,
    /// 경보 한계 전이지만 기울기로 트립 도달이 가까운 선행 경보 (SDS 동적 기울기 트리거)
    PreAlarm,
}

impl AlarmKind {
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "high" => Some(AlarmKind::High),
            "low" => Some(AlarmKind::Low),
            "pre_alarm" => Some(AlarmKind::PreAlarm),
            _ => None,
        }
    }
}

/// DCS 가 낸 알람 하나
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AlarmEvent {
    pub tag: String,
    /// 다중 채널 센서에서 알람을 낸 채널 (`A` 등)
    pub channel: Option<String>,
    pub at: DateTime<Utc>,
    pub value: f64,
    pub kind: AlarmKind,
    pub message: String,
}

/// 서명 측정값 (zoro-gravity 와 같은 기준)
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Signature {
    pub stroke_count: u32,
    /// 서명 바운딩 박스가 서명란을 차지하는 비율 (0.0 ~ 1.0)
    pub bbox_area_ratio: f32,
    /// 한 줄 직선처럼 생긴 획인지
    pub single_line: bool,
}

impl Signature {
    /// 종이에 한 서명. 스캔에서 서명 조각이 보이면 패드 측정값 대신 이 값을 씁니다.
    pub fn on_paper() -> Self {
        Self { stroke_count: 2, bbox_area_ratio: 1.0, single_line: false }
    }
}

/// 현장 단말이 보낸 문서의 종류
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum FieldKind {
    /// 현장 계기 점검표 (아날로그 게이지 실측)
    GaugeCard,
    /// Red-line 수기 수정 도면 (변경관리)
    Redline,
    /// 스캔 없이 손으로 넣은 현장 수치
    Manual,
}

impl FieldKind {
    pub fn label(self) -> &'static str {
        match self {
            FieldKind::GaugeCard => "현장 계기 점검표",
            FieldKind::Redline => "설비변경 Red-line",
            FieldKind::Manual => "현장 수치 입력",
        }
    }
}

/// 현장 운전원이 고른 편차 소명 (현장 단말 5단계)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub enum DeviationExcuse {
    /// 현장 아날로그 게이지는 정상이나 전자식 전송기 영점이 흔들리는 것으로 보임
    TransmitterDrift,
    /// 계획예방정비 · 부품 교체(MOC)로 운전 사양이 바뀜 (Red-line 첨부)
    MocChange,
    /// 현장 수동 바이패스 밸브 조작 중
    ManualBypass,
    /// 자유 서술 (Zoro-1 이 표준 코드로 바꿉니다)
    FreeText(String),
}

impl DeviationExcuse {
    pub fn label(&self) -> &'static str {
        match self {
            DeviationExcuse::TransmitterDrift => "전송기 영점 흔들림 의심",
            DeviationExcuse::MocChange => "설비 변경(MOC)으로 사양 변경",
            DeviationExcuse::ManualBypass => "현장 수동 바이패스 조작 중",
            DeviationExcuse::FreeText(_) => "기타 현장 특이사항",
        }
    }
}

/// KKS 오독 교정 한 건 (Stage 2 상세 화면)
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Correction {
    /// 항목 이름 (`설비 KKS`, `센서 태그`, `MOC 번호`)
    pub field: String,
    pub raw: String,
    pub fixed: String,
    pub distance: usize,
}

/// 문서를 읽는 데 걸린 시간 (Stage 1 · 2)
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize)]
pub struct ReadTimings {
    pub layout_ms: f64,
    pub siglip2_ms: f64,
    pub ocr_ms: f64,
    /// 읽은 조각 수와 버린 안내문 조각 수
    pub crops: usize,
    pub dropped: usize,
}

/// 현장 단말이 보낸 점검 · 변경 보고 하나
#[derive(Clone, PartialEq, Serialize)]
pub struct FieldReport {
    pub kind: FieldKind,
    pub sector: Option<Sector>,
    pub kks: Option<String>,
    pub tag: Option<String>,
    /// 현장 실측치 (게이지) 또는 Red-line 의 새 값
    pub reading: Option<f64>,
    pub unit: Option<String>,
    /// 실측 · 스캔 시각 (시점 동기화 기준 `t_scan`)
    pub taken_at: DateTime<Utc>,
    pub drawing: Option<String>,
    pub drawing_rev: Option<String>,
    pub moc_no: Option<String>,
    /// Red-line 의 기존 값
    pub old_value: Option<f64>,
    pub worker_id: Option<String>,
    pub worker_name: Option<String>,
    pub signature: Option<Signature>,
    /// 안전작업허가(PTW) · 잠금표지(LOTO) 절차 확인
    pub permit_confirmed: bool,
    pub excuse: Option<DeviationExcuse>,
    /// 글자 신뢰도가 낮거나 못 읽은 항목 (사람이 확인)
    pub uncertain: Vec<String>,
    pub corrections: Vec<Correction>,
    /// 스캔 이미지 해시 (같은 장 중복 확인)
    pub content_hash: Option<String>,
    pub timings: Option<ReadTimings>,
}

impl fmt::Debug for FieldReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FieldReport")
            .field("kind", &self.kind)
            .field("kks", &self.kks)
            .field("tag", &self.tag)
            .field("reading", &self.reading)
            .field("moc_no", &self.moc_no)
            .field("worker_id", &self.worker_id.as_deref().map(|_| "***"))
            .field("worker_name", &self.worker_name.as_deref().map(crate::privacy::mask_name))
            .finish()
    }
}

impl FieldReport {
    /// 빈 보고 (항목은 채우는 쪽이 넣습니다)
    pub fn new(kind: FieldKind, taken_at: DateTime<Utc>) -> Self {
        Self {
            kind,
            sector: None,
            kks: None,
            tag: None,
            reading: None,
            unit: None,
            taken_at,
            drawing: None,
            drawing_rev: None,
            moc_no: None,
            old_value: None,
            worker_id: None,
            worker_name: None,
            signature: None,
            permit_confirmed: false,
            excuse: None,
            uncertain: Vec::new(),
            corrections: Vec::new(),
            content_hash: None,
            timings: None,
        }
    }
}

/// 사건의 출처
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum IncidentSource {
    /// DCS 알람 · SDS 트리거
    Dcs,
    /// 현장 단말이 먼저 보낸 보고 (Red-line 등)
    Field,
}

pub struct Incident {
    pub id: String,
    pub source: IncidentSource,
    pub alarm: Option<AlarmEvent>,
    pub telemetry: Telemetry,
    pub reports: Vec<FieldReport>,
    /// 분석 시각 ("지금"). 계측은 이 시각까지만 봅니다.
    pub received_at: DateTime<Utc>,
    /// 합성 시나리오 번호 (시연 · 시험용)
    pub scenario: Option<String>,
}

impl Drop for Incident {
    fn drop(&mut self) {
        for r in self.reports.iter_mut() {
            if let Some(name) = r.worker_name.as_mut() {
                name.zeroize();
            }
            if let Some(DeviationExcuse::FreeText(text)) = r.excuse.as_mut() {
                text.zeroize();
            }
        }
    }
}

impl Incident {
    /// 분석 시각 (epoch ms)
    pub fn now_ms(&self) -> i64 {
        self.received_at.timestamp_millis()
    }

    /// 알람 태그. 알람이 없으면 현장 보고의 태그입니다.
    pub fn focus_tag(&self) -> Option<&str> {
        self.alarm.as_ref().map(|a| a.tag.as_str()).or_else(|| self.reports.iter().find_map(|r| r.tag.as_deref()))
    }
}
