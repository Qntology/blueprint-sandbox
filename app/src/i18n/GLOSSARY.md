# 영어 화면 용어집 (English UI glossary)

화면 글은 `const t = useT()` (`import { useT } from "./i18n"`, 폴더 깊이에 맞게) 로 `t("한국어", "English")` 처럼 나란히 씁니다.
한국어 글은 바꾸지 않습니다 (E2E 가 그 글을 찾습니다). 영어는 짧고 관제실 용어답게. 백엔드가 보내는 글(원장 이름 · 엔진 메시지)은
`ipc.ts` 가 번역 표(`en.json`)로 바꾸므로 컴포넌트에서 따로 번역하지 않습니다.

| 한국어 | English |
| --- | --- |
| 관제 콘솔 | Control Console |
| 주제어실 | Main Control Room (MCR) |
| 교대감독 | Shift Supervisor |
| 파트장 | Team Lead |
| 현장 운전원 · 현장 엔지니어 | Field Operator · Field Engineer |
| 현장 단말 | Field Terminal |
| 사건 · 사건 대기열 | Incident · Incident Queue |
| 확인 대기 · 결재 대기 · 현장 대기 · 처리 완료 | Pending · Awaiting Approval · Awaiting Field Check · Closed |
| 공동 결재 | Co-sign (joint approval) |
| 확정 · 조치확정 · 반려 · 재점검 | Confirm · Confirm Action · Reject · Recheck |
| 인터록 · 선행 절차 | Interlock · Prerequisite Step |
| 병목 | Bottleneck |
| 근본 원인 · 경보 · 트립 · 셧다운 | Root Cause · Alarm · Trip · Shutdown |
| 표준 조치 | Standard Action |
| 절차서 · 초안 | Procedure · Draft |
| 변경관리 (MOC · MOP) | Management of Change (MOC · MOP) |
| 도면 · 계통도 | Drawing · System Diagram |
| 설비 · 계측 · 센서 · 태그 | Equipment · Measurement · Sensor · Tag |
| 마인드맵 | Mind Map |
| 파이프라인 · 마이닝 레인 | Pipeline · Mining Lanes |
| 타임머신 리플레이 · 재생 · 멈춤 · 발원점 · 분석 시각 | Time-Machine Replay · Play · Pause · Origin · Analysis Time |
| 시계열 · SDS 분석기 | Time Series · SDS Analyzer |
| 물리엔진 · 자율 대응 | Physics Engine · Autonomous Response |
| 자동화 모드 · 보조 모드 | Automation Mode · Assist Mode |
| 모델 관리 | Model Management |
| 공간 Depth | Spatial Depth |
| 현장 스캔 · 점검표 · 서명 | Field Scan · Inspection Sheet · Signature |
| 보관 문서 · 감사 기록 | Archived Record · Audit Log |
| 화력발전소 · AI 데이터센터 | Thermal Power Plant · AI Data Center |
| 질의 · 문맥 | Query · Context |
| 판정 · 확신도 | Verdict · Confidence |
| 실측치 · 편차 · 소명 | Field Reading · Deviation · Justification |
| 접수 번호 | Receipt No. |
