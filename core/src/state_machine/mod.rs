//! 5-Gate 상태 머신 (기획 Part 8 5절). 게이트마다 순수 함수로 결과를 내고, 파이프라인이 순서와 확정 경로를 맡습니다.
//!
//! - `gate0_router`: 알람 라우팅 · 공간 주소 · 도면 좌표 핀포인팅
//! - `gate1_formal`: 신호 무결성(동결 · 순간 잡음) · 현장 문서 서명 · 작업허가
//! - `gate2_consistency`: DCS 다중 보팅 · 현장 게이지 시점 동기화 대조 · Red-line ↔ MOC 원장
//! - `gate3_substantive`: 마인드맵 상류 역추적 + Zoro-1 판단 (SDS 직교 분할) · 고위험 승인 통제
//! - `gate4_5_finalize`: AOP 절차서 고르기 · 템플릿 요약 · 현장 안내 누출 차단 · 보관 행
//! - `analysis`: 사건 하나를 Gate 0 ~ 4 로 분석한 결과 (화면과 결재가 함께 씁니다)
//! - `pipeline`: 자동 확정 · 교대감독 결정 · 현장 보고 재분석 · Gate 5 봉인 (`Engine`)
//! - `staged`: 모델을 부르는 일을 준비(엔진 잠금) → 모델(잠금 없음) → 반영(엔진 잠금)으로 나눠 엔진 잠금을 짧게 쥡니다

pub mod analysis;
pub mod gate0_router;
pub mod gate1_formal;
pub mod gate2_consistency;
pub mod gate3_substantive;
pub mod gate4_5_finalize;
pub mod pipeline;
pub mod staged;

pub use pipeline::{Engine, Minds, Outcome, SupervisorDecision};
pub use staged::EngineAccess;
