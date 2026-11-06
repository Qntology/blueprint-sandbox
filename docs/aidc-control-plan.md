# AI 데이터센터 모드 (추가 개선안: 1.2GW 화력발전소 ↔ 100MW AI 데이터센터)

> 상태: 받은 추가 개선안(Part 1 ~ 4)을 코드와 대조해 구현한 기록입니다. 데이터 정의는 [synthetic-aidc-data.md](synthetic-aidc-data.md), 실측은 [verification.md](verification.md) 9절입니다.
> 수치는 모두 합성 데이터 기준이고 실제 데이터센터 · 설비 · 사람이 아닙니다.

## 1. 한 줄 요약

콘솔 머리의 스위치 `[🏭 1.2GW 화력발전소 | 🖥️ 100MW AI 데이터센터]` 하나로, 같은 앱 · 같은 모델(SigLIP2 · gocr · Zoro-1) · 같은 5-Gate 엔진이 **DLC 수냉 GPU 클러스터**를 관제합니다. CDU 냉각수 유량이 줄면 수 초 뒤 GPU 정션 온도가 오르는 열수력 연쇄가 발전소의 급수 → 과열기 연쇄와 같은 식이라, 인과 시차(Δt) 역추적 · SDS 3축 분할 · 잉크 차분 · 100 ms 물리엔진 폐루프가 그대로 돕니다.

## 2. 도메인 1:1 대응

| 아키텍처 요소 | 🏭 화력발전소 | 🖥️ AI 데이터센터 | 엔진 쪽 |
| --- | --- | --- | --- |
| 관리 대상 | 1.2GW 초초임계압 1호기 | 100MW 센터A (DLC 수냉 GPU Pod) | 원장 `plant` |
| 설비 식별 코드 | KKS `10LAB20AA102` (`99AAA99AA999`) | DCIM 자산 코드 `DC1CDU20AA02` (`AA9AAA99AA99`) | `plant::CodeScheme` — 자리 꼴만 다르고 오독 교정 · 편집 거리 맞춤은 같음 |
| Lv2 구역 | 101 보일러 · 102 터빈 · 103 급수 · 104 전기 · 105 환경 | 101 GPU 컴퓨트 홀 · 102 패브릭·스토리지 · 103 수냉 CDU·칠러룸 · 104 전력·UPS · 105 열원·냉각탑·공조 | Gate 0 다섯 라우팅 자리 공유 |
| 계통 도면 (Lv4) | P&ID `PID-103-LAB` | MEP 수냉 배관도 `MEP-103-CDU` · 단선결선도 `SLD-104-PWR` · 패브릭 `NET-102-FAB` | 같은 서식 · 같은 판독 |
| 핵심 물리 흐름 | 급수 유량(t/h) → 과열기 증기 온도 | CDU 2차 냉각수 유량(LPM) → GPU 정션 온도 | `causal.rs` 같음, 매체 방향 규칙 추가 |
| 3중 진실 대조 | DCS 2oo3 보팅 vs 현장 게이지 | BMC 2oo3 보팅 vs 현장 매니폴드 압력계 | `gate2_consistency.rs` 같음 (촬영 시각 동기화) |
| 현장 수기 도면 | Red-line P&ID + MOC · LOTO | Red-line 변경도 + MOP · Hot-Work · LoTo | 델타 · 원장 대조 같음, 변경번호 머리는 원장에서 (`MOC` · `MOP`) |
| 표준 절차서 | AOP | EOP | `drafting.rs` · `factcheck.rs` 같음, 지시문만 도메인 용어 |
| Tier 1 자율 대응 | 급수 바이패스 65 % 개방 | CDU 우회밸브 65 % 개방 (+ 학습 잡 드레인은 절차서 단계) | `mitigation.rs` 같음 (인터록 우회 단계에서 찾음) |
| Tier 2 고위험 | 터빈 수동 정지 · 출력 감발 (2인 결재) | Pod EPO · 클러스터 강제 종료 (2인 결재) | `policy.rs` 같음 (`team_lead_cosign`) |

## 3. 시나리오 (D1 ~ D7 · D3F)

| ID | 발전소 짝 | 무엇 | 기대 판정 |
| --- | --- | --- | --- |
| D1 | P1 센서 드리프트 | 101동 Pod A 매니폴드 공급 압력 채널 A 만 0.8 bar (누수 의심), B · C · 현장 압력계 2.4 bar | `G2_SENSOR_DRIFT_ISOLATED` — EPO · 밸브 차단 보류, 현장 확인 전 확정 불가 (`EOP-101-IC01`) |
| D2 | P2 상류 밸브 고착 | CDU 2차측 유량조절밸브 18 % 고착 → 유량 725 LPM → 펌프 토출압 3.4 bar(역류 여파) → 4.2 s 뒤 GPU 정션 89 °C | `G3_ADVERSE_UPSTREAM_CAUSE` — 원인 `DC1CDU20AA02` 2홉 · `OPC-103-101` · `EOP-103-CDU02` (인터록 3단계 · 2인 결재) |
| D3 | P3 Red-line | CDU 펌프 경보 3.0 → 3.5 bar `MOP-2026-1104` | `G2_MOC_VERIFIED` — 기준치 오버레이 |
| D3F | P3F 위조 | 같은 도면에 4.5 bar (승인값 · 콜드플레이트 내압 4.0 bar 초과) | `G2_REJECT_CONTRADICTION_LOCK` |
| D4 | P4 고위험 | 104동 배터리 셀 78 °C (열폭주 전조), 상류 정상 | `G3_VIOLATION_HIGH_RISK` — EPO · 클러스터 종료는 2인 결재 (`EOP-104-BAT01`) |
| D5 | P5 순간 잡음 | GPU 정션 1초만 145 °C | `G1_SPIKE_FILTERED` → `G3_ELIGIBLE_NORMAL` |
| D6 | P6 동결 | 랙 냉각수 출구 온도 40초째 42.000 °C (BMC 데몬 행) | `G1_SENSOR_FLATLINE` (`EOP-COM-BMC02`) |
| D7 | P7 선행 경보 | 냉각탑 팬 인버터 전류 360 → 282 A, 2 s 뒤 냉수 공급 온도 상승 | `HIGH_SLOPE_PRE_ALARM` → `G3_ADVERSE_UPSTREAM_CAUSE` (`EOP-105-CTW01`) |

## 4. 물리엔진 (`physics.rs`, 원장 `physics`)

```
C · dT/dt = Qd · (T0 − Tin) − Q(t − τ) · (T − Tin)       (발전소 과열기 · 데이터센터 GPU 콜드플레이트 같은 식)
Q* = Qd · min(1.05, (f(u) + k_b · f(b)) / f(u0)),  f(u) = u^0.35,  1차 지연 0.6 s
P  = P0 + ΔP · (1 − Q/Qd) / 0.4                     (토출압은 유량을 1 s 늦게 따라감)
```

데이터센터 상수: Qd 1,200 LPM · u0 0.78 · k_b 0.42 · P0 2.8 bar · ΔP 0.6 bar · T0 68 °C · Tin 20 °C · C(=M) 14,400 · τ 4.2 s. 밸브 18 % 고착이면 720 LPM · 3.4 bar, GPU 정션은 τ 뒤 약 1.6 °C/s 로 올라 그대로 두면 약 37초 뒤 95 °C 셧다운 한계를 넘습니다. SDS 추적기가 밸브 → GPU 인과 사슬을 잠그고(`VALVE_STUCK_LOW`, 인터록 고장 코드와 같음) Tier 1 이 CDU 우회밸브를 65 % 열면 유량이 99 % 로 돌아와 고점 약 88 °C 에서 꺾입니다 (시험: `physics::tests` · `core/tests/aidc_scenarios.rs` 폐루프).

## 5. 화면

- **콘솔**: 발전소와 같은 CCTV형 통합 관제판 ([cctv-console-plan.md](cctv-console-plan.md)). 제목 · 용어(자산코드 · EOP · MOP · BMC·DCIM · NOC 관제실 · 셧다운) · 도면 칩(MEP · NET · SLD) · 시나리오 목록(D1 ~ D7) · 판정 구역 이름 · 교대감독 사번(NOC-201)이 도메인을 따라 바뀝니다. 바꾸면 진행 · 보관 사건 · 감사 기록 · 자동화 모드 · 물리엔진 · 현장 대기를 비우고, 모델은 그대로 둡니다. 델타 판독 마스터 색인은 새 도메인 다섯 장으로 뒤에서 다시 만듭니다 (두 도메인 도면이 같은 서식이라 열 장을 함께 견주면 1 · 2위 차이가 줄기 때문).
- **현장 단말**: 머리 `AI 데이터센터 현장 엔지니어 단말`, 자산코드 · MOP 변경번호 · bar 단위 예시, 소명 보기(BMC 텔레메트리 흔들림 · MOP 변경 · CDU 우회밸브 수동 조작), 작업허가 확인 `MOP/Hot-Work · LoTo`, 전송 버튼 `NOC 관제실로 전송하기`. 원터치 대조 카드 · 장갑 스테퍼는 두 도메인 공통입니다.
- 시작 도메인은 `BLUEPRINT_DOMAIN=aidc` 로 고를 수 있습니다 (기본 발전소).

## 6. 대조 · 구현

| 추가 개선안 | 구현 | 상태 |
| --- | --- | --- |
| Part 1 "코어 엔진(`state_machine` · `causal.rs` · `timeseries.rs` · `sds.rs`)은 단 한 줄도 바꿀 필요가 없다" | 알고리즘은 그대로지만 도메인이 코드에 박힌 곳이 있었습니다: KKS 자리 꼴(`kks_shape` · `repair_kks` · `Measure::of_tag` 의 7번째 글자), 섹터 키 다섯 개(`Sector::from_key`), 매체 열한 개와 방향 규칙, 물리엔진 태그 · 상수, 글쓰기 · 판단 지시문의 "발전소 주제어실", 공통 절차서 번호, 현장 안내 · 결재 문서의 "주제어실" · `MCR` 번호, 점검표 항목 `설비 KKS`, 변경번호 머리 `MOC` | **확인** (개선안이 틀림) |
| "DCIM 코드도 `Lattice::match_kks` 의 1↔I · 0↔O 교정이 그대로 작동" | 그대로는 작동하지 않습니다. `DC1CDU20AA02` 는 KKS 자리 꼴과 달라 KKS 규칙으로 고치면 `D` → `0` 처럼 망가집니다. `CodeScheme`(`Kks` · `Dcim` 자리 꼴)을 원장에 두고 교정 · 측정 코드 위치 · 짧은 태그를 체계마다 셉니다 (`DC1CDUI0AP01` → `DC1CDU10AP01`) | 구현 |
| `DomainProfile::PowerPlant \| AiDataCenter` | `domain::DomainMode` + 원장 시드가 도메인 · 코드 체계 · 용어(`Terms`) · 물리 계통(`PhysicsConfig`)을 들고 옵니다 | 구현 |
| 섹터 `label()` 을 모드로 분기하거나 원장 `sectors` 를 그대로 신뢰 | 원장 섹터 행을 그대로 쓰고, `Sector` 는 다섯 라우팅 자리로 남겼습니다 (발전소 키 · 데이터센터 키 모두 받음). 판정 목록의 구역 이름(`SEC_0x`)도 원장에서 | 구현 |
| `aidc_db.json` · `aidc_scenarios.json` 같은 스키마 | 같은 스키마 (선택 항목 `plant.domain` · `code_scheme` · `terms`, `physics`, 절차서 `role`, 도면 `ignore_zones` 만 더함). 설비는 개선안의 48개 대신 32개(시나리오에 필요한 계통 + 구역마다 대표 설비), 센서 28개 | 구현 |
| 개선안의 코드 `DC1BESS0BT01` · `DC1PDU10el01` · `DC1CLP10CP01` | 앞 둘은 12글자 꼴이 아니고(`BESS0` · 소문자), 셋째는 설비 종류 자리에 압력 측정 코드 `CP` 를 써서 센서 태그와 헷갈립니다. `DC1BES10BT01` · `DC1PDU10CB01` 로 고치고, 콜드플레이트는 GPU 랙에 포함했습니다 (D2 의 2홉: 밸브 → 랙 매니폴드 → GPU 랙) | **확인** (개선안이 틀림) |
| `scripts/gen_aidc_drawings.py` | 새 스크립트 대신 같은 생성기에 `--domain aidc` (발전소 출력 바이트 동일). 시나리오는 `gen_plant_cases.py --domain aidc` | 구현 (방식을 바꿈) |
| `Lattice::embedded_for` · `ScenarioSet::embedded_for` | 구현 | 구현 |
| `physics.rs` 태그 상수 → `SimConfig` | 원장 `physics` (`PhysicsConfig`). 발전소 원장에는 없어도 기본값이 발전소 급수 계통이라 그대로 | 구현 |
| `Desk.mode` · `switch_domain_mode` | `Desk.ledger` · `cases` (`RwLock<Arc<…>>`, 명령마다 같은 사본) · `domain`, `switch_domain_mode` · `domain_info`, 이벤트 `domain_changed` (콘솔 · 현장 단말 둘 다) | 구현 |
| 프론트 모드 스위치 · 용어 바인딩 | `domain.ts` (`TermsContext`), 콘솔 머리 세그먼트 버튼, 현장 단말 용어 | 구현 |
| D1 `EOP-102-IC01` | 매니폴드는 101동이라 `EOP-101-IC01` | 바꿈 |
| D2 "1.2s 뒤 펌프 토출압" · "TTF 38초" · "+4.8 °C/s" | 발전소 P2 와 같은 판정 경로가 되게 1.6 s · 초기 3.2 °C/s 로 맞췄습니다. TTF 는 분석 시각의 기울기로 계산되는 값입니다 | 바꿈 |
| 인터록 Step 1 "토출압 ≤ 4.0 bar" | 4.0 bar 는 셧다운(설계) 한계라, 발전소처럼 경보와 셧다운 사이 3.8 bar 로 두었습니다 | 바꿈 |

## 7. 남은 과제

- 두 도메인을 한 화면에 동시에 띄우지 않습니다 (한 번에 한 도메인, 바꾸면 사건을 비움). 여러 사이트를 함께 관제하려면 원장 · 엔진을 도메인마다 따로 두어야 합니다.
- 설비 심볼 그림 사전은 발전소 표본으로 만든 것을 그대로 씁니다 (같은 10종 모양). 데이터센터 도면의 KKS 없는 판독(명판 훼손) 실측은 하지 않았습니다.
- 네트워크(102동) 계통은 매체 방향 규칙이 없어(판정 표 없음) 인과 판정에 쓰이지 않습니다. 광트랜시버 과열 시나리오는 넣지 않았습니다.
