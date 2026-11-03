# blueprint — 1.2GW 화력발전소 P&ID · 신호 기반 이상 위치 탐색 및 원인 진단

물리적 폐쇄망의 1.2GW 초초임계압(USC) 석탄화력발전소에서, 알람이 나면 **P&ID 도면 위에 위치를 찍고**, **계통 마인드맵을 상류로 거슬러 근본 원인을 찾고**, **Zoro-1 로 원인 판정과 비정상 운전절차서(AOP) 초안을 만드는** 온디바이스 관제 파이프라인입니다. 기획은 [docs/powerplant-control-plan.md](docs/powerplant-control-plan.md)(Part 8 · 8-1 · 8-1B · 8-2)와 [docs/powerplant-timeseries-sds-plan.md](docs/powerplant-timeseries-sds-plan.md)(Part 8-3) 입니다.

- **바탕**: [zoro-gravity](https://github.com/Qntology/zoro-gravity) 의 구조를 그대로 따릅니다. 5-Gate 파이프라인 · 자동화 정책(보조 모드 기본, 불리한 판정 자동 확정 금지) · 감사 로그 · 사실 대조(`factcheck`) · Tauri 두 창 앱.
- **AI 엔진 · 모델**: zoro-gravity 의 `core/src/ai` 와 **같은 모델**(SigLIP2 크롭 · gocr 문서 인식 · Zoro-1 판단 · 글쓰기)을 씁니다. 모델은 zoro-gravity 의 앱 데이터 폴더에서 **가져옵니다**(큰 가중치는 하드 링크). [docs/engine.md](docs/engine.md)
- **데이터**: 발전소 DCS · CMMS 연계가 없어서 합성 시드 원장(KKS 설비 34 · 센서 22 · 간선 39 · 도면 5 · MOC · AOP)과 합성 시계열 시나리오 8개로 진행합니다. [docs/synthetic-plant-data.md](docs/synthetic-plant-data.md)
- **AI 단독 제어 금지**: 이 시스템은 설비를 조작하지 않습니다. 확정은 진단과 조치 지시서를 기록하는 일이고, 계통 이상 · 고위험 조치(감발 · 정지)는 교대감독 파트장 공동 결재가 있어야 확정됩니다.

## 기획 대응 (P11 ~ P14)

| 단계 | 기획 산출물 | 구현 | 검증 |
| --- | --- | --- | --- |
| **P11** 시드 DB · 합성 생성기 | `synthetic_db.json`, `plant_scenarios.json`, `gen_pid_drawings.py`, `gen_plant_cases.py` | 구현. 마스터 P&ID 5장 · Red-line 2장 · 계기 점검표 1장과 정답 JSON | 원장 로딩 시 참조 무결성 검사 (도면을 넘는 간선 12개 모두 연결점) |
| **P12** KKS · 물리량 추출, 원장 4대 상호작용 | `lattice.rs`, `extract.rs`, `evidence`(→ `gate2_consistency.rs`), `case.rs` | 구현. ① KKS 오독 자리별 교정 + 편집 거리 2 (유일할 때만) ② 도면 점프 연결점 ③ 2oo3 보팅 · 현장 게이지 **촬영 시각** 대조 ④ Red-line ↔ `moc_ledger` (번호 · 값 · 서명자 관할 · 설계 한계 · 도면 개정) | `10LABI0AP001 → 10LAB10AP001`, **P1** 14.2 vs 26.0 MPa 드리프트 분리, **P3** 31.0 통과 · 35.0 잠금 |
| **P13** Gate 3 역추적 · Zoro-1 판정 · AOP | `gate2_consistency.rs`, `gate3_substantive.rs`, `drafting.rs` | 구현. 인과 시차(Δt) 그래프 탐색, SDS 3축 분할 질문, AOP 초안 + 숫자 · 코드 1:1 대조, 고위험 공동 결재 강제 | **P2** 2홉 상류 밸브(개도 18%) · 도면 점프 `OPC-103-101`, **P4** 근거 없는 승인 · 반려 차단 |
| **P14** 3-Way 관제 UI · E2E | `PlantTopologyView.tsx`, `OfficerView.tsx`, `KioskView.tsx` | 구현. 도면 오버레이 · Mermaid · 옵시디언 마인드맵이 선택 설비 하나로 동기화, 공간 뎁스 · 홉 뎁스 · Stage 1~5 패널, 프로세스 마이닝 오버레이 · 타임머신 · SDS 분석기, 현장 단말 5단계 | `scripts/e2e/console.js` (실제 앱 두 창 · ROCm · 실제 모델): P2 · P1 · P3 · P3F · P4 통과, 현장 단말 화면마다 금지어 검사 |
| Part 8-3 시계열 · SDS | `timeseries.rs`, `sds.rs` | 구현. 60초 링버퍼(×1000 정수) · 변곡점 · 기울기 · 가속도 · EWMA · TTF, 동결 · 잡음 검출, 데드밴드 트리거 · 기울기 선행 경보, ±30초 동결 · 블랙박스 봉인 해시 | **P5** 잡음 · **P6** 동결 · **P7** 선행 경보 |

기획과 다르게 만든 곳은 [docs/powerplant-control-plan.md](docs/powerplant-control-plan.md) 0절에 **확인** 으로 적었습니다.

## 시험

```bat
:: 단위 292개 + 시나리오 E2E 13개 (CPU, 모델 없이 규칙 · 대역 모델)
cargo test -p blueprint-core --no-default-features
cargo test -p blueprint-models

:: 실제 모델로 시나리오 전체 · 도면 · 현장 스캔 (ROCm, 모델 폴더 필요)
python scripts\gen_plant_cases.py
python scripts\gen_pid_drawings.py
run\windows\rocm.bat plant_probe out\field out\plant_probe.json
```

실제 모델 측정 결과(판정 8/8 · AOP 초안 6/6 모델 글 · 도면 KKS 34/34 · 도면번호 5/5 · 현장 스캔 3/3)와 앱 E2E 는 [docs/verification.md](docs/verification.md) 에 있습니다. 합성 데이터에서 잰 값이라 제품 성능으로 말하지 않습니다.

## 실행

| OS | 앱 | 예제 (기본 `plant_probe`) |
| --- | --- | --- |
| Windows | `run\windows\app.bat` (Vulkan) · `run\windows\app-rocm.bat` (AMD ROCm) | `run\windows\rocm.bat` · `vulkan.bat` · `cuda.bat` |
| Linux | `run/linux/app.sh` | `run/linux/rocm.sh` · `vulkan.sh` · `cuda.sh` |
| macOS | `run/macos/app.command` (Apple Silicon Metal) | `run/macos/metal.command` · `vulkan.command` |

- 앱은 두 창입니다: **주제어실 관제 콘솔**(`console`)과 **현장 순시 · 정비 단말**(`field`). 현장 단말에는 내부 판정 코드 · 확신도 · 마인드맵이 가지 않습니다.
- 콘솔 머리의 **DCS 시뮬레이터**로 시나리오(P1 ~ P7)를 넣습니다. Red-line 시나리오(P3 · P3F)는 계측만 바꾸고, 현장 단말에서 Red-line 을 스캔해 보내면 사건이 열립니다.
- 스캐너는 `BLUEPRINT_SCANNER_DIR` 폴더(기본 `%LOCALAPPDATA%\blueprint\scanner`)의 이미지로 대신합니다. `out/field/P1_gauge.png`, `P3_redline.png` 를 넣고 단말에서 스캔하세요.
- `BLUEPRINT_ENGINE=rules` 면 모델 없이 규칙만으로 돕니다. `BLUEPRINT_TRACE=<파일>` 이면 모델 사용 기록을 남깁니다.
- 장치는 `BLUEPRINT_DEVICE=auto|cuda|rocm|metal|vulkan|cpu` (기본 CUDA → ROCm → Metal → Vulkan → CPU).

### 모델

모델은 zoro-gravity 와 같습니다. 앱을 켜면 없는 모델을 `%LOCALAPPDATA%\zorogravity\models` 에서 가져오고(큰 가중치는 하드 링크, 작은 파일은 복사), 거기에도 없으면 Hugging Face 에서 내려받습니다.

```bat
cargo run -p blueprint-models -- status
cargo run -p blueprint-models -- import
```

| 모델 | 내려받는 곳 | 크기 |
| --- | --- | --- |
| `zoro` | [Qntology/zoro-1-gguf](https://huggingface.co/Qntology/zoro-1-gguf) | 5.2 GB |
| `gocr` | [Qntology/GravityOCR-KO-gguf](https://huggingface.co/Qntology/GravityOCR-KO-gguf) | 1.6 GB |
| `siglip2` | [Qntology/siglip2-so400m-patch16-naflex-gguf](https://huggingface.co/Qntology/siglip2-so400m-patch16-naflex-gguf) | 1.7 GB |

## 콘솔 화면 (기획 Part 8-1 · 8-1B · 8-2)

- **머리**: 교대감독 사번, 자동화 모드(승인자 · 근거 규정), DCS 시뮬레이터, 모델 설정
- **공간 Depth 브레드크럼**: Lv1 호기 ▸ Lv2 동 ▸ Lv3 층(EL) ▸ Lv4 도면(개정) ▸ Lv5 KKS
- **좌측 대기열**: 현장 대기(⏳) · 결재 대기 · 공동 결재(🔒) · 처리 완료(자동 확정 표시)
- **중앙 3-Way 뷰**: ① P&ID 도면 위 `norm_bbox` 상자 · 홉 뎁스 · 근본 원인 🔥 · 점프 커넥터(누르면 연결 도면으로) ② Mermaid 섹터 서브그래프 · 원인 경로 굵은 선 · 원본 펼치기 ③ 옵시디언 힘 기반 그래프(중심 Depth 0 후광, ±1 밝게, 나머지 흐림, 드래그)
- **우측**: 선택 설비 계측 · 기준치(MOC 오버레이 표시) · 마인드맵 홉 뎁스 목록 / 마이닝을 켜면 병목 분석기(물리 병목 · 의사결정 병목 · 신호 전파 체인)
- **하단**: Stage 1~5 카드(실측 시간 · 상태) → 누르면 단계 상세(크롭 · 교정, 3중 진실 대조 · 실명 열람 요청, Zoro-1 사실 · 선택지 막대 · SDS 축, AOP 초안 · Factcheck · 공동 결재), 의사결정 레인, 타임머신 리플레이, 시계열 · SDS 분석기

## 디렉토리

```
blueprint-sandbox/
├── Cargo.toml              워크스페이스 · candle 포크 패치 (zoro-gravity 와 같음)
├── core/                   blueprint-core (Tauri 없이 시험 가능)
│   ├── src/ai/             zoro-gravity 에서 가져온 엔진 (SigLIP2 · gocr · Zoro-1 · 상주 관리)
│   ├── src/plant.rs        섹터 · 측정 종류 · 매체 · KKS 꼴과 오독 자리 교정
│   ├── src/lattice.rs      합성 시드 원장 · KKS 매칭 · 공간 주소 · MOC · AOP
│   ├── src/graph.rs        홉 뎁스 · 상류 경로(누적 지연) · 도면 점프
│   ├── src/timeseries.rs   시계열 · 링버퍼 · 변곡점 · 기울기 · TTF · 동결 · 잡음
│   ├── src/causal.rs       인과 시차 근본 원인 역추적 · 역류 여파 · 전파 체인
│   ├── src/sds.rs          SDS 트리거 · 스냅샷 · 블랙박스 봉인
│   ├── src/extract.rs      현장 스캔 판독 (점검표 · Red-line · 마스터 도면)
│   ├── src/state_machine/  gate0 ~ gate4_5 · analysis · pipeline (Engine)
│   ├── src/verdict.rs      판정 코드 · judge_by_criteria · judge_by_axes (SDS 축)
│   ├── src/drafting.rs     AOP 초안 · 소명 정규화 (TextWriter)
│   ├── src/factcheck.rs    숫자 · 설비 코드 1:1 대조
│   ├── src/field.rs        현장 단말 접수 (편차 소명 · 대기 사건 연결)
│   ├── src/mining.rs       간선 흐름 · 병목 · 의사결정 레인
│   ├── src/mermaid.rs      Mermaid 원본
│   ├── fixtures/           synthetic_db.json · plant_scenarios.json · drawings/
│   ├── examples/plant_probe.rs  실제 모델 측정
│   └── tests/plant_scenarios.rs P1 ~ P7 E2E
├── tools/model-settings/   blueprint-models: 모델 폴더 · zoro-gravity 가져오기 · 다운로드
├── app/                    Tauri + React: 관제 콘솔 · 현장 단말
├── scripts/                생성기 · ROCm/CUDA 환경 · 실행 공용 · e2e/console.js
├── run/                    백엔드별 실행 스크립트
└── docs/                   기획 · 합성 데이터 · 엔진 · 검증
```

## 라이선스

Apache-2.0 ([LICENSE](LICENSE)). 모델 라이선스는 zoro-gravity 와 같습니다 (zoro · siglip2 Apache-2.0, gocr MIT).
