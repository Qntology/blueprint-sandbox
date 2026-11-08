# blueprint — 1.2GW 화력발전소 · 100MW AI 데이터센터 도면 · 신호 기반 이상 위치 탐색 및 원인 진단

물리적 폐쇄망의 1.2GW 초초임계압(USC) 석탄화력발전소에서, 알람이 나면 **P&ID 도면 위에 위치를 찍고**, **계통 마인드맵을 상류로 거슬러 근본 원인을 찾고**, **Zoro-1 로 원인 판정과 비정상 운전절차서(AOP) 초안을 만드는** 온디바이스 관제 파이프라인입니다. 기획은 [docs/powerplant-control-plan.md](docs/powerplant-control-plan.md)(Part 8 · 8-1 · 8-1B · 8-2), [docs/powerplant-timeseries-sds-plan.md](docs/powerplant-timeseries-sds-plan.md)(Part 8-3), 개선안 [docs/powerplant-a0-interlock-plan.md](docs/powerplant-a0-interlock-plan.md)(Part 8-4) · [docs/powerplant-physics-auto-plan.md](docs/powerplant-physics-auto-plan.md)(Part 8-5) · [docs/powerplant-multi-issue-plan.md](docs/powerplant-multi-issue-plan.md)(Part 8-6) 입니다. 자연어 검색은 logis-center 의 방식을 옮겼습니다 ([docs/search.md](docs/search.md)). 글자 없는 설비 심볼 · KKS 없는 판독 · 시계열 / SDS 고도화는 [docs/powerplant-symbol-plan.md](docs/powerplant-symbol-plan.md) 입니다.

콘솔 머리의 스위치 하나로 **100MW 하이퍼스케일 AI 데이터센터**(DLC 수냉 GPU 클러스터: CDU 냉각수 유량 → GPU 정션 온도)로 바꿔 같은 엔진 · 같은 모델로 관제합니다 ([docs/aidc-control-plan.md](docs/aidc-control-plan.md), 합성 데이터 [docs/synthetic-aidc-data.md](docs/synthetic-aidc-data.md)). 콘솔은 풀스크린 오버레이 관제판입니다: 고른 3-Way 도면이 화면 전체에 깔리고, 그 위에 근본 원인 · 경보 · 선택 설비의 CCTV 말풍선(60초 파형 · 도면 조각)이 지시선으로 붙고, 병목 · 인터록 · 결재 HUD 와 파이프라인 · 타임머신 · 자연어 질의 허브가 반투명 유리 판으로 떠 있습니다 ([docs/overlay-console-plan.md](docs/overlay-console-plan.md), 앞 단계 [docs/cctv-console-plan.md](docs/cctv-console-plan.md)).

- **바탕**: [zoro-gravity](https://github.com/Qntology/zoro-gravity) 의 구조를 그대로 따릅니다. 5-Gate 파이프라인 · 자동화 정책(보조 모드 기본, 불리한 판정 자동 확정 금지) · 감사 로그 · 사실 대조(`factcheck`) · Tauri 두 창 앱.
- **AI 엔진 · 모델**: zoro-gravity 의 `core/src/ai` 와 **같은 모델**(SigLIP2 크롭 · gocr 문서 인식 · Zoro-1 판단 · 글쓰기)을 씁니다. 모델은 zoro-gravity 의 앱 데이터 폴더에서 **가져옵니다**(큰 가중치는 하드 링크). [docs/engine.md](docs/engine.md)
- **데이터**: 발전소 DCS · CMMS 연계가 없어서 합성 시드 원장(KKS 설비 35 · 센서 23 · 간선 41 · 도면 5 + A0 종합 1 · MOC · AOP · 선행 절차 인터록 · 도면 기호 범례 10종)과 합성 시계열 시나리오 8개로 진행합니다. [docs/synthetic-plant-data.md](docs/synthetic-plant-data.md)
  AI 데이터센터도 같은 스키마의 합성 원장(DCIM 자산 32 · 센서 28 · 간선 36 · MEP · SLD 도면 5 + A0 · MOP · EOP · 인터록 · 물리엔진 계통)과 시나리오 8개(D1 ~ D7 · D3F)입니다. [docs/synthetic-aidc-data.md](docs/synthetic-aidc-data.md)
- **AI 단독 제어 금지**: 이 시스템은 실제 설비를 조작하지 않습니다. Part 8-5 의 자율 대응(Tier 1 우회 개방) 명령은 내장 물리엔진(시뮬레이터)에만 들어갑니다. 확정은 진단과 조치 지시서를 기록하는 일이고, 계통 이상 · 고위험 조치(감발 · 정지)는 교대감독 파트장 공동 결재가 있어야 확정됩니다.

## 기획 대응 (P11 ~ P14)

| 단계 | 기획 산출물 | 구현 | 검증 |
| --- | --- | --- | --- |
| **P11** 시드 DB · 합성 생성기 | `synthetic_db.json`, `plant_scenarios.json`, `gen_pid_drawings.py`, `gen_plant_cases.py` | 구현. 마스터 P&ID 5장 · Red-line 2장 · 계기 점검표 1장과 정답 JSON | 원장 로딩 시 참조 무결성 검사 (도면을 넘는 간선 13개 모두 연결점, 인터록 단계 깊이) |
| **P12** KKS · 물리량 추출, 원장 4대 상호작용 | `lattice.rs`, `extract.rs`, `evidence`(→ `gate2_consistency.rs`), `case.rs` | 구현. ① KKS 오독 자리별 교정 + 편집 거리 2 (유일할 때만) ② 도면 점프 연결점 ③ 2oo3 보팅 · 현장 게이지 **촬영 시각** 대조 ④ Red-line ↔ `moc_ledger` (번호 · 값 · 서명자 관할 · 설계 한계 · 도면 개정) | `10LABI0AP001 → 10LAB10AP001`, **P1** 14.2 vs 26.0 MPa 드리프트 분리, **P3** 31.0 통과 · 35.0 잠금 |
| **P13** Gate 3 역추적 · Zoro-1 판정 · AOP | `gate2_consistency.rs`, `gate3_substantive.rs`, `drafting.rs` | 구현. 인과 시차(Δt) 그래프 탐색, SDS 3축 분할 질문, AOP 초안 + 숫자 · 코드 1:1 대조, 고위험 공동 결재 강제 | **P2** 2홉 상류 밸브(개도 18%) · 도면 점프 `OPC-103-101`, **P4** 근거 없는 승인 · 반려 차단 |
| **P14** 3-Way 관제 UI · E2E | `PlantTopologyView.tsx`, `OfficerView.tsx`, `KioskView.tsx` | 구현. 도면 오버레이 · Mermaid · 옵시디언 마인드맵이 선택 설비 하나로 동기화, 공간 뎁스 · 홉 뎁스 · Stage 1~5 패널, 프로세스 마이닝 오버레이 · 타임머신 · SDS 분석기, 현장 단말 5단계 | `scripts/e2e/console.js` (실제 앱 두 창 · ROCm · 실제 모델): P2 · P1 · P3 · P3F · P4 통과, 현장 단말 화면마다 금지어 검사 |
| Part 8-3 시계열 · SDS | `timeseries.rs`, `sds.rs` | 구현. 60초 링버퍼(×1000 정수) · 변곡점 · 기울기 · 가속도 · EWMA · TTF, 동결 · 잡음 검출, 데드밴드 트리거 · 기울기 선행 경보, ±30초 동결 · 블랙박스 봉인 해시 | **P5** 잡음 · **P6** 동결 · **P7** 선행 경보 |
| Part 8-4 A0 · 시계열 질의 · 인터록 | `tiling.rs`, `tsdb.rs`, `interlock.rs`, `A0View.tsx` | 구현. A0 4×4 타일 판독 · 전역 좌표 · 중복 병합, 정적 시드 + 60초 링버퍼 질의(촬영 시각 값 · 구간 요약 · 변곡 시차), 절차서 선행 절차 3단계(깊이 순서 강제 · 미완료 확정 차단) | A0 실측: 타일 35/35 · 251 s, 원장 상자 빠른 확인 35/35 · 33 s (300 DPI: 타일 566 s · 상자 30 s), 상자 하나 재확인 2.2 s, 인터록 순서 위반 차단 |
| Part 8-5 물리엔진 · 자율 대응 | `physics.rs`, `mitigation.rs`, `PhysicsPanel.tsx` | 구현. 급수 계통 100 ms 물리엔진 · 고장 주입 · SDS 상태 추적 · Tier 1 우회 개방(자동화 모드 ∧ 규칙 ∧ 네 기준 ≥ 0.95, 명령은 시뮬레이터에만) | 폐루프 시험: 밸브 고착 → 사건 → 자동 우회 → 과열기 고점 트립 전 |
| Part 8-6 이슈 분해 | `issues.rs`, `SidePanel.tsx` | 구현. 단일 · 절차 · 복합 이슈 카탈로그(A-1 ~ C-3)와 하위 이슈 분해 트리 | P2 = 복합(C-1 + B-3), P1 = 단일(A-1), P4 = 단일(A-2) |
| 자연어 검색 (logis-center 방식, 한국어 · 영어) | `search/` (`lexicon` · `analyze` · `nms` · `models`), `nlp/stanza.rs`, `ai/embed.rs`, `cctv/CommandBar.tsx` | 구현. 유형 먼저(머리 낱말 규칙) · 조건 대조 후 힌트로 내림 · 전체 질의 경로 RRF · 구제 · 식별자 교정 · 질적 상태(높음 · 고착 · 비정상 …, 지금 계측이 있을 때만 필터) · "왜 찾았나". 영어 질의를 엔진이 그대로 읽음(영어 낱말 목록 · en.json 영어 이름). 선택 모델: Stanza 품사 · 원형(ONNX, 다시 검증한 변환본) → 조사 · 어미를 뗀 줄기 · 원형, granite 임베딩 → 질의 벡터 경로 · NMS 조각(낱말 창 → 원장 설비 · 센서) | 한국어 17개 MRR 0.941, 영어 17개 0.971, 데이터센터 한 · 영 0.900, 뜻풀이 질의 0.31 → 0.48 (한국어) · 0.61 → 0.68 (영어) 임베딩으로 ([docs/search.md](docs/search.md)) |
| 개선안: 설비 심볼 · KKS 없는 판독 | `symbols.rs`, `identify.rs`, `gen_pid_drawings.py` 심볼 범례 | 구현. 상자 오른쪽 위 심볼 10종, 잉크 연결 요소 후보 + SigLIP2 그림 사전(`symbol_dict.bin`, `crop_probe.bin` 과 별도), 이름 · 위치 · 계기 태그 · 항목 · 좌표 · 심볼 단서로 KKS 없이 설비 판독(애매하면 사람) | 시험 표본 191/200 맞음 · 틀림 0, 마스터 심볼 32/32 찾음 · 31 분류, 명판 훼손 L1 35/35 · L2 34/35 · L3 32/35 · 틀림 0 ([docs/powerplant-symbol-plan.md](docs/powerplant-symbol-plan.md)) |
| 시계열 · SDS 고도화 | `timeseries.rs`, `sds.rs`, `physics.rs` | 구현. 신호 모양(SDS 상태 코드) · 고장 코드(`VALVE_STUCK_LOW`) · CUSUM · 2차 TTF · 한 표본 튐 막기 · 변화 시작 시차 · 회복 히스테리시스 | CUSUM 1.5σ 탐지 98.8 % (띠 51.5 %) · 같은 오경보 수준, P5 순간 잡음은 열지 않음 |
| 개선안: CCTV형 통합 관제판 | `OfficerView.tsx`, `cctv/` (`CamPanel` · `Docks` · `RightRail`), `views/*` 오버레이, `mermaid.rs` 배지, `ai/siglip2/delta.rs` · `ink.rs` 전처리 | 구현. 가운데 6:4 (CAM-01 3-Way 도면 / CAM-02 시계열 · CAM-03 SDS 로직 차트 상시 도킹), 우측 병목 · 인터록 신호등 · 결재 통합, 단계 바 + 마이닝 레인 한 줄, 도면 설비마다 SDS 상태 칩 · 기울기 · 미니 파형, 원인 배관 Δt, 마인드맵 변곡 링, 현장 단말 스캔 조각 대조 카드 · 장갑 스테퍼, 델타 판독 전처리(고정 안내문 자리 잘라 내기 · 유채색 잉크 · 도각 평행 이동 맞춤) | 단위 시험 (도각 7 · 5 px 밀림 맞춤, 흑백 선 위 붉은 수정선, 안내문 자리 잘라 내기), 앱 E2E `CCTV` · [docs/cctv-console-plan.md](docs/cctv-console-plan.md) |
| 2차 개선안: 풀스크린 오버레이 콘솔 | `OfficerView.tsx`, `PlantTopologyView.tsx`, `cctv/` (`stage` · `CctvOverlay` · `CctvZoom` · `CommandBar` · `RightRail` · `ZoomCtl` · `traces`), `views/*`, `TimeScrubber.tsx`, `drawings.ts` | 구현. 6 : 4 격자 분할을 걷어 내고 고른 뷰 하나를 무대 전체(`inset: 0`)에 깔고 빈 자리 맞춤 · 휠 확대 · 끌어 이동, 근본 원인 · 경보 · 선택 · 검색 1위 설비에 CCTV 말풍선 + 지시선(판 · 서로 · 설비를 피하는 자리 찾기, 다른 도면 설비는 점프 커넥터에), 좌상단 · 우상단 알약, 사건 대기열 · 설정 팝오버, 접는 결재 HUD, 하단 허브(파이프라인 · 3줄 미니 파형 리플레이 · 자연어 커맨드 바 · CAM-02 · 03 Drop-up), ESC 판 닫기 순서 · Enter · / 포커스 · 질의 기록, 선택 설비 문맥 질의. Rust 는 검색 식별자 우선순위 하나만 고침 | 앱 E2E 17개 (CCTV: 도면 높이 · 말풍선 · 지시선 · 판과 겹침 없음 · 확대 창 · HUD 접기 · 마인드맵 핀 · Mermaid 말풍선, SEARCH: "왜" 리본 · 문맥 질의) · [docs/overlay-console-plan.md](docs/overlay-console-plan.md) |
| 추가 개선안: AI 데이터센터 모드 | `domain.rs` (`DomainMode` · `Terms` · `PhysicsConfig`), `plant.rs` `CodeScheme` (KKS · DCIM), `aidc_db.json` · `aidc_scenarios.json` · MEP · NET · SLD 도면, `switch_domain_mode` | 구현. 원장 · 시나리오 · 도면 · 용어 · 물리 계통을 도메인마다 갈아 끼우고 엔진 · 모델은 공유. DCIM 자리 꼴 오독 교정, 냉각수 · 냉수 · 냉각탑 매체 방향 규칙, CDU → GPU 열수력 물리엔진 | 시나리오 E2E 8개 (D1 ~ D7 · D3F · 폐루프 · 전환 뒤 사건 번호), 실제 모델 판정 8/8 · 자산 코드 32/32 · 스캔 3/3 ([docs/verification.md](docs/verification.md) 9절) |
| 개선안: 3대 병목 | `state_machine/staged.rs`, `ai/residency.rs` (`Coexist`), `ai/siglip2/delta.rs`, `ink::glyphs` | 구현 (일부는 실측으로 방식을 바꿈). 엔진 잠금을 준비 · 반영에만 쥐고 모델은 잠금 밖에서, SigLIP2 비전 타워 공존(선택, 기본 꺼짐), 마스터 대비 잉크가 바뀐 곳만 다시 읽는 델타 판독(Red-line · A0), 글자 없는 띠 건너뛰기 | 대역 모델 시험: 조회 대기 475 → 0.0 ms. Red-line P3 36.6 → 11.2 s · P3F 33.9 → 7.0 s (값 같음), 바뀌지 않은 A0 타일 17 ms. [docs/powerplant-bottleneck-plan.md](docs/powerplant-bottleneck-plan.md) |

기획과 다르게 만든 곳은 [docs/powerplant-control-plan.md](docs/powerplant-control-plan.md) 0절에 **확인** 으로 적었습니다.

## 시험

```bat
:: 단위 365개 + 엔진 잠금 4개 + 시나리오 E2E 발전소 14개 · AI 데이터센터 8개 (CPU, 모델 없이 규칙 · 대역 모델)
cargo test -p blueprint-core --no-default-features
cargo test -p blueprint-models

:: 실제 모델로 시나리오 전체 · 도면 · 현장 스캔 (ROCm, 모델 폴더 필요). AI 데이터센터는 생성기에 --domain aidc, 실측에 set BLUEPRINT_DOMAIN=aidc
python scripts\gen_plant_cases.py
python scripts\gen_pid_drawings.py
run\windows\rocm.bat plant_probe out\field out\plant_probe.json

:: 설비 심볼 그림 사전 만들기 · 재기, 명판 훼손 도면의 KKS 없는 판독 (ROCm)
python scripts\gen_pid_drawings.py --symbols
run\windows\rocm.bat symbol_probe --save

:: SDS 델타 판독: Red-line · A0 재스캔을 마스터와 견주어 바뀐 곳만 읽기 (ROCm)
python scripts\gen_pid_drawings.py --a0-delta
run\windows\rocm.bat delta_probe out\delta_probe.json

:: 자연어 검색 평가 (한국어 · 영어 · 뜻풀이를 규칙만 · +Stanza · +임베딩 세 가지로. 검색 모델이 없으면 규칙만), SDS 트리거 이전 · 지금 비교
cargo run -p blueprint-core --release --example search_eval
cargo run -p blueprint-core --release --example sds_eval
```

실제 모델 측정 결과(판정 8/8 · AOP 초안 6/6 모델 글 · 도면 KKS 35/35 · 도면번호 5/5 · 현장 스캔 3/3)와 앱 E2E 는 [docs/verification.md](docs/verification.md) 에 있습니다. 합성 데이터에서 잰 값이라 제품 성능으로 말하지 않습니다.

앱 E2E 는 한국어 · 영어 화면 모두 돕니다. 실행기 한 줄이 앱(ROCm)을 띄우고, 엔진 준비를 기다려 흐름을 돌린 뒤 앱을 내립니다 ([docs/ui-simplify-plan.md](docs/ui-simplify-plan.md) D1).

```bat
node scripts\e2e\run.js                       :: console.js 17 흐름 (한국어), --lang en · --only P2,P1 은 그대로 넘김
node scripts\e2e\run.js --i18n                :: 한국어 --audit → build_en.py → 영어 E2E
node scripts\e2e\run.js --record --lang en    :: 시연 영상 촬영 (record.js, 모델 기록 포함)
node scripts\e2e\run.js --share-gpu 2200      :: 다른 프로세스가 GPU 를 쓰는 동안: 이 앱 VRAM 상한 · 지킴 · out/e2e/vram.csv
node scripts\e2e\run.js --rules --app-only    :: 모델 없이 화면만 (GPU 안 씀), 내릴 때 --stop
```

시연 영상(한국어 · 영어)을 찍고 편집하는 방법은 [docs/video.md](docs/video.md) 에 있습니다 (`scripts/e2e/record.js` · `edit_video.py`). 합성 데이터는 `python scripts\gen_all.py` 가 순서대로 만듭니다 (`--en` 영어 그림, `--dry-run`).

## 실행

| OS | 앱 | 예제 (기본 `plant_probe`) |
| --- | --- | --- |
| Windows | `run\windows\app.bat` (Vulkan) · `run\windows\app-rocm.bat` (AMD ROCm) | `run\windows\rocm.bat` · `vulkan.bat` · `cuda.bat` |
| Linux | `run/linux/app.sh` | `run/linux/rocm.sh` · `vulkan.sh` · `cuda.sh` |
| macOS | `run/macos/app.command` (Apple Silicon Metal) | `run/macos/metal.command` · `vulkan.command` |

- 앱은 두 창입니다: **주제어실 관제 콘솔**(`console`)과 **현장 순시 · 정비 단말**(`field`). 현장 단말에는 내부 판정 코드 · 확신도 · 마인드맵이 가지 않습니다. 단말 첫 화면에는 주제어실이 현장 확인을 기다리는 경보 사건(설비 · 위치 · 요청 메모)이 뜨고, 누르면 설비 코드를 채웁니다. 사번은 원장에 있는 것만 받습니다 (모르는 사번을 이름으로 바꿔 받지 않음).
- 설정 팝오버(`[⚙️ 사번 · 모드]`)의 **도메인**(`🏭 1.2GW 화력발전소 | 🖥️ 100MW AI 데이터센터`)에서 도메인을 바꿉니다. 바꾸면 사건 · 감사 기록 · 자동화 모드 · 물리엔진을 비우고 모델은 그대로 둡니다. `BLUEPRINT_DOMAIN=aidc` 면 데이터센터로 시작합니다.
- 우상단 **🧪 시연 · 시험** 판의 시뮬레이터(발전소 DCS · 데이터센터 BMC)로 시나리오(P1 ~ P7 · D1 ~ D7)를 넣습니다. 같은 판에 물리엔진 시뮬레이터와 **세션 초기화**(앱을 다시 띄우지 않고 사건 · 감사 · 물리 · 단말을 비움)가 있습니다. Red-line 시나리오(P3 · P3F · D3 · D3F)는 계측만 바꾸고, 현장 단말에서 Red-line 을 스캔해 보내면 사건이 열립니다.
- 운영 화면처럼 쓰려면 설정의 **시연 · 시험 도구 → 숨기기**: 시연 판 · 결재 HUD 의 가드레일 시험 단추가 사라집니다 (이 PC 에 기억).
- 설정의 **엔지니어 보기 (코드 이름)** 를 켜면 판단 · 결재 화면에 코드 이름(`jev_head` · `PointerPrompt` · `verdict.rs` · `G4_…` · 검색 모델 이름)이 함께 보입니다. 기본은 끔이고 시연 영상은 켜고 찍습니다.
- 스캐너는 `BLUEPRINT_SCANNER_DIR` 폴더(기본 `%LOCALAPPDATA%\blueprint\scanner`)의 이미지로 대신합니다. `out/field/P1_gauge.png`, `P3_redline.png` 를 넣고 단말에서 스캔하세요.
- **화면 언어**: 설정 팝오버(`[⚙️ 사번 · 모드]`)의 `화면 언어 · Language` 에서 `🇰🇷 한국어` / `🇺🇸 English` 를 고릅니다. 두 창이 함께 바뀌고 다음 실행에도 남습니다 (`settings.json`, 시작 때 `BLUEPRINT_LANG=en` 으로 정할 수도 있음). 영어 화면에서는 도면 그림 · 원장 이름 · 판정 · 결재 · 검색 글이 영어이고, Zoro-1 이 절차서 원인 요약을 영어로 씁니다. 자연어 질의도 영어로 칩니다. 엔진 · 판정 · 현장 서류는 그대로입니다 ([docs/i18n.md](docs/i18n.md)).
- `BLUEPRINT_ENGINE=rules` 면 모델 없이 규칙만으로 돕니다. `BLUEPRINT_TRACE=<파일>` 이면 모델 사용 기록을 남깁니다.
- 장치는 `BLUEPRINT_DEVICE=auto|cuda|rocm|metal|vulkan|cpu` (기본 CUDA → ROCm → Metal → Vulkan → CPU).

### 모델

모델은 zoro-gravity 와 같습니다. 앱을 켜면 없는 모델을 `%LOCALAPPDATA%\zorogravity\models` 에서 가져오고(큰 가중치는 하드 링크, 작은 파일은 복사), 거기에도 없으면 Hugging Face 에서 내려받습니다.
검색용 두 모델은 선택입니다 (없으면 검색이 규칙 · 글자 경로만 씁니다). 임베딩은 logis-center 가 받아 둔 것을 가져오고, Stanza 는 공개 저장소가 없어 [stanza-onnx-export](https://github.com/Qntology/stanza-onnx-export) 로 en · ko 를 변환한 폴더에서 가져옵니다.

```bat
cargo run -p blueprint-models -- status
cargo run -p blueprint-models -- import
cargo run -p blueprint-models -- import stanza ..\stanza-onnx-export\onnx_final
```

| 모델 | 내려받는 곳 | 크기 |
| --- | --- | --- |
| `zoro` | [Qntology/zoro-1-gguf](https://huggingface.co/Qntology/zoro-1-gguf) | 5.2 GB |
| `gocr` | [Qntology/GravityOCR-KO-gguf](https://huggingface.co/Qntology/GravityOCR-KO-gguf) | 1.6 GB |
| `siglip2` | [Qntology/siglip2-so400m-patch16-naflex-gguf](https://huggingface.co/Qntology/siglip2-so400m-patch16-naflex-gguf) | 1.7 GB |
| `embed` (검색, 선택) | [ibm-granite/granite-embedding-97m-multilingual-r2](https://huggingface.co/ibm-granite/granite-embedding-97m-multilingual-r2) (logis-center 에서 가져옴) | 0.22 GB |
| `stanza` (검색, 선택) | 없음 — stanza-onnx-export 변환본 `onnx_final/{en,ko}` | 0.33 GB |

## 콘솔 화면 (기획 Part 8-1 · 8-1B · 8-2, 개선안 CCTV형 통합 관제판 → 2차 개선안 풀스크린 오버레이)

고른 3-Way 뷰 하나가 화면 전체에 깔린 배경 월드이고, 나머지는 모두 그 위에 뜬 반투명 유리 판입니다. 선택 설비 하나 · 재생 시각 하나로 함께 움직입니다 ([docs/overlay-console-plan.md](docs/overlay-console-plan.md)).

- **무대 (3-Way 뷰)**: ① P&ID 도면 위 `norm_bbox` 상자 · 홉 뎁스 · 근본 원인 🔥 · 원인 배관 Δt · 점프 커넥터(누르면 연결 도면으로), 결재 HUD 를 뺀 빈 자리에 맞춤 · 휠 확대 · 끌어 이동 · 🌓 어둡게 ② Mermaid 섹터 서브그래프 · 원인 경로 굵은 선 · SDS 배지 ③ 옵시디언 힘 기반 그래프(중심 Depth 0 후광, ±1 밝게, 변곡 노드 유리 핀 `t=0 발원` · `Δt`), A0 종합 도면은 타일 확대 · 왼쪽 아래 미니맵
- **CCTV 말풍선**: 근본 원인 · 경보 · 선택 · 검색 1위 설비에서 지시선으로 뜨는 카드(SDS 상태 · dx/dt · 60초 파형 · 도면 조각, `[🔍 파형 확대]` `[📋 인터록 이동]`). 다른 도면에 있는 설비는 그 도면으로 가는 점프 커넥터에 붙습니다
- **좌상단**: 브랜드 · `[🚨 사건 대기열]` 팝오버(현장 대기 ⏳ · 결재 대기 · 공동 결재 🔒 · 처리 완료), 뷰 도구줄(도면 선택 · 전체 계통 · A0 정보), 공간 Depth(설비를 고르면 Lv1 호기 ▸ Lv2 동 ▸ Lv3 층 ▸ Lv4 도면 ▸ Lv5 KKS)
- **우상단**: 3-Way 뷰 전환 · 🌊 흐름 · 병목 오버레이 · `[🧪 시연 · 시험]`(시나리오 주입 · 물리엔진 · 세션 초기화) · `[⚙️ 사번 · 모드]` 설정 팝오버(화면 언어 · 도메인 · 교대감독 사번 · 자동화 모드 · 시연 도구 보이기 · 엔지니어 보기 · 모델 관리)
- **오른쪽 결재 HUD (접기 ▸)**: 감지된 핵심 병목 → 절차서 인터록 신호등 게이트 → 결재(표준 조치 요약 · 공동 결재 · 근거 절차서 번호(미리 채우지 않음: 초안의 번호를 사람이 침) · 재점검 지시 · 조치확정, `[더 보기]` 에 반려 · 오경보 예외 · 증빙 · 메모, 시연 모드의 가드레일 시험 단추. 주입한 사건의 초안은 사건이 열린 뒤 뒤에서 채워지고 그동안 확정이 잠깁니다) → 선택 설비 계측 · 기준치 · 이슈 트리 · 홉 뎁스. 결재는 이 한 곳에서만 합니다
- **하단 허브**: 알림 한 줄(결재 결과 · 확정 뒤 `[🗄️ 보관 문서 열기]` · 현장 보고 도착 `[열기]` · 검색 요약), 파이프라인(Stage 1·2 · 3 · 4 · 5 카드 · 마이닝 레인 건수 · 최대 대기 → 누르면 단계 상세 서랍, Stage 5 는 초안 · Factcheck 읽기 전용), 트립(셧다운) 잔여 TTF(머리 숫자는 여기 한 곳) · 타임머신 리플레이(바탕에 D-1 · D0 · D+n 3줄 미니 파형) · `[📈 시계열 · SDS]` Drop-up(상류 → 원인 → 하류 파형 · 링버퍼 지표, SDS 상태 전이 · 3축 분할 · 게이트 벡터), 자연어 커맨드 바(Enter · / · ↑↓ 기록 · `[🛠 KKS ✕]` 문맥 · 결과 Drop-up)
- 팝오버 · Drop-up · 서랍은 종류마다 한 번에 하나만 열리고, **ESC** 는 위에 뜬 판부터 하나씩 닫습니다(확대 창 → 팝오버 → Drop-up → 서랍 → 검색 표시 → 설비 선택). 글을 쓰던 질의 줄에서는 글만 지웁니다. 단순화 내역은 [docs/ui-simplify-plan.md](docs/ui-simplify-plan.md)

## 디렉토리

```
blueprint-sandbox/
├── Cargo.toml              워크스페이스 · candle 포크 패치 (zoro-gravity 와 같음)
├── core/                   blueprint-core (Tauri 없이 시험 가능)
│   ├── src/ai/             zoro-gravity 에서 가져온 엔진 (SigLIP2 · gocr · Zoro-1 · 상주 관리)
│   ├── src/plant.rs        섹터 · 측정 종류 · 매체 · 코드 체계(KKS · DCIM) 꼴과 오독 자리 교정
│   ├── src/domain.rs       도메인 모드 (발전소 · AI 데이터센터) · 화면 · 글쓰기 용어 · 물리엔진 계통
│   ├── src/lattice.rs      합성 시드 원장 · KKS 매칭 · 공간 주소 · MOC · AOP
│   ├── src/graph.rs        홉 뎁스 · 상류 경로(누적 지연) · 도면 점프
│   ├── src/timeseries.rs   시계열 · 링버퍼 · 변곡점 · 기울기 · TTF · 동결 · 잡음
│   ├── src/causal.rs       인과 시차 근본 원인 역추적 · 역류 여파 · 전파 체인
│   ├── src/sds.rs          SDS 트리거 · 스냅샷 · 블랙박스 봉인
│   ├── src/symbols.rs      설비 심볼 후보 추출 · SigLIP2 그림 사전
│   ├── src/identify.rs     KKS 없는 설비 판독 (이름 · 위치 · 태그 · 항목 · 좌표 · 심볼)
│   ├── src/extract.rs      현장 스캔 판독 (점검표 · Red-line · 마스터 도면)
│   ├── src/state_machine/  gate0 ~ gate4_5 · analysis · pipeline (Engine)
│   ├── src/verdict.rs      판정 코드 · judge_by_criteria · judge_by_axes (SDS 축)
│   ├── src/drafting.rs     AOP 초안 · 소명 정규화 (TextWriter)
│   ├── src/factcheck.rs    숫자 · 설비 코드 1:1 대조
│   ├── src/field.rs        현장 단말 접수 (편차 소명 · 대기 사건 연결)
│   ├── src/mining.rs       간선 흐름 · 병목 · 의사결정 레인
│   ├── src/mermaid.rs      Mermaid 원본
│   ├── src/i18n.rs         화면 언어 (한국어 · English) · 영어 절차서 글쓰기 지시문
│   ├── src/search/         자연어 검색 (파서 · 경로 · RRF, 한 · 영 낱말 목록, 품사 · 원형 분석, NMS 조각, 모델 묶음)
│   ├── src/nlp/stanza.rs   Stanza 품사 · 원형 ONNX 실행기 (tract), src/ai/embed.rs 검색 임베딩 (granite, candle CPU)
│   ├── fixtures/           synthetic_db.json · plant_scenarios.json · aidc_db.json · aidc_scenarios.json · drawings/ (PID · MEP · NET · SLD, en/ 영어 그림)
│   ├── examples/           실제 모델 측정 (plant_probe · symbol_probe · a0_probe), 모델 없는 평가 (search_eval · sds_eval)
│   └── tests/plant_scenarios.rs P1 ~ P7 E2E
├── tools/model-settings/   blueprint-models: 모델 폴더 · 가져오기 (zoro-gravity · logis-center · stanza 변환본) · 다운로드
├── app/                    Tauri + React: 관제 콘솔 · 현장 단말 (src/i18n: 화면 언어 · 영어 번역 표)
├── scripts/                생성기 · ROCm/CUDA 환경 · 실행 공용 · e2e/ (console.js E2E · record.js 촬영 · edit_video.py 편집) · i18n/ (번역 표 만들기)
├── run/                    백엔드별 실행 스크립트
└── docs/                   기획 · 합성 데이터 · 엔진 · 검증
```

## 라이선스

Apache-2.0 ([LICENSE](LICENSE)). 모델 라이선스는 zoro-gravity 와 같습니다 (zoro · siglip2 Apache-2.0, gocr MIT). 검색 임베딩 granite-embedding-97m-multilingual-r2 · Stanza 모델은 Apache-2.0 입니다.
