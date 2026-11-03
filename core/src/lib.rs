//! blueprint 코어: 1.2GW 화력발전소 P&ID · 신호 기반 이상 위치 탐색 및 원인 진단 (기획 Part 8).
//!
//! - `ai`: zoro-gravity 에서 가져온 candle 엔진 (SigLIP2 크롭 · gocr 인식 · Zoro-1 판단 · 글쓰기, 가중치 상주 관리)
//! - `plant` · `lattice`: 공간 뎁스(Lv1~Lv5) · KKS 코드 · 합성 시드 원장 (설비 · 센서 · 간선 · 도면 연결점 · MOC · AOP)
//! - `graph` · `causal`: 계통 마인드맵 (홉 뎁스 · 상류 경로) 과 인과 시차(Δt) 근본 원인 역추적
//! - `timeseries` · `sds`: 60초 링버퍼 · 변곡점 · 기울기 · TTF, SDS 트리거 · 스냅샷 · 블랙박스 봉인
//! - `extract` · `evidence`: 현장 스캔(계기 점검표 · Red-line) 판독과 원장 4대 상호작용
//! - `state_machine`: Gate 0 ~ 5 파이프라인 (`Engine`)
//! - `drafting` · `factcheck`: Zoro-1 이 쓰는 AOP 초안과 숫자 · 코드 1:1 대조
//! - `policy` · `audit` · `privacy` · `store`: 자동화 정책 · 감사 로그 · 비식별 · 세션과 보관
//! - `stages` · `mining` · `mermaid` · `scenario` · `field`: 콘솔 화면용 단계 기록 · 프로세스 마이닝 · Mermaid 원본 · 합성 시나리오 · 현장 단말 접수

pub mod ai;
pub mod audit;
pub mod case;
pub mod causal;
pub mod decision;
pub mod drafting;
pub mod extract;
pub mod factcheck;
pub mod field;
pub mod graph;
pub mod interlock;
pub mod issues;
pub mod lattice;
pub mod mermaid;
pub mod mining;
pub mod mitigation;
pub mod physics;
pub mod plant;
pub mod policy;
pub mod privacy;
pub mod scenario;
pub mod search;
pub mod sds;
pub mod stages;
pub mod state_machine;
pub mod store;
pub mod tiling;
pub mod timeseries;
pub mod tsdb;
pub mod verdict;
