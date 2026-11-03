# AI 엔진과 모델 (zoro-gravity 에서 가져온 것)

blueprint 는 zoro-gravity 와 **같은 모델 · 같은 엔진**을 씁니다. 새 모델을 들이지 않았고, 모델 코드도 새로 짜지 않았습니다.

## 1. 가져온 것

| 대상 | 가져온 곳 | 바꾼 것 |
| --- | --- | --- |
| `core/src/ai/` (accel · verify · weights · residency · ocr · document · siglip2 · gocr · zoro · jev_head · pointer · kv_turbo · speculative · tokenizer · trace · runtime) | zoro-gravity `core/src/ai` (main 748854a) | 환경 변수 접두어 `ZOROGRAVITY_` → `BLUEPRINT_`, 모델 폴더 크레이트 이름, 포인터 지시문을 발전소 진단으로, `trace` 에 메모리 수집(`capture`) 추가 |
| `tools/model-settings/` | zoro-gravity `tools/model-settings` | 앱 폴더 `blueprint`, zoro-gravity 모델 폴더에서 **가져오기** 추가, 명령줄 도구 `blueprint-models` |
| `scripts/rocm-env.bat` · `cuda-env.bat` · `run-*.{bat,sh}` · `run/` | zoro-gravity | 패키지 · 변수 이름, 기본 예제 `plant_probe` |
| `.github/` cross-verify 워크플로 | zoro-gravity | 패키지 이름(`blueprint-*`) · 체크아웃 경로 · 장치 변수(`BLUEPRINT_DEVICE`) · 로그 접두어, rhwp 단계 삭제. Actions 러너에서는 아직 돌려 보지 않았습니다 (아래 5절) |
| `app/src/SignaturePad.tsx` · `ModelSettings.tsx` | zoro-gravity | 가져오기 단추 |

GGUF 메타데이터 키(`zorogravity.source_config_json`, `zorogravity.block_diffusion.*`)는 모델 파일 형식이라 이름을 그대로 둡니다.
엔진 자체의 설계 · 실측(가중치 상주, 양자화, GPU 백엔드, 크롭, jev_head 형식)은 zoro-gravity 의 `docs/weight-residency.md`, `docs/gpu-backends.md`, `docs/document-crop.md`, `docs/jev-head.md`, `docs/zoro-backbone.md` 에 있습니다. 코드 주석의 `zoro-gravity docs/…` 는 그 문서를 가리킵니다.

## 2. 모델 폴더와 가져오기

| OS | blueprint 모델 폴더 | 가져올 곳 (zoro-gravity) |
| --- | --- | --- |
| Windows | `%LOCALAPPDATA%\blueprint\models` | `%LOCALAPPDATA%\zorogravity\models` |
| macOS | `~/Library/Application Support/blueprint/models` | `~/Library/Application Support/zorogravity/models` |
| Linux | `$XDG_DATA_HOME/blueprint/models` | `$XDG_DATA_HOME/zorogravity/models` |

`BLUEPRINT_MODELS_DIR` 로 모델 폴더를, `BLUEPRINT_IMPORT_FROM` 으로 가져올 곳을 바꿀 수 있습니다.

모델 목록(`zoro` · `gocr` · `siglip2`)과 Hugging Face 저장소는 zoro-gravity 와 같습니다. 가져오기 규칙:

- **큰 가중치(`*.gguf`, `*.safetensors`)는 하드 링크**입니다. 디스크를 두 번 쓰지 않습니다 (이 PC 에서 8.0 GB 가 추가 사용 없이 들어왔습니다, 링크 수 2).
- **작은 파일(`active.json`, `tokenizer.json`, `block_diffusion.json`, 크롭 앵커 `crop_probe.bin`, `precision-q8.json`)은 복사**합니다. 앱이 고쳐 쓸 수 있는 파일이라 두 앱이 같은 파일을 보면 안 됩니다 (예: 도면용 크롭 앵커를 다시 학습해도 zoro-gravity 의 앵커는 그대로).
- 변환 전 원본(`model.safetensors` 4.3 GB)은 가져오지 않습니다.
- `active.json` 을 맨 마지막에 둬서, 가져오다 멈춘 모델이 열리지 않습니다. 같은 크기의 파일은 다시 가져오지 않습니다.
- 하드 링크를 걸 수 없으면 (다른 드라이브) 복사합니다.
- blueprint 에서 모델을 지우면 링크만 지워지고 zoro-gravity 쪽 파일은 남습니다.
- 주의: 두 앱 중 하나가 같은 이름의 GGUF 를 **제자리에서** 다시 쓰면 다른 쪽도 바뀝니다. 다운로드는 `.tmp` 로 받아 이름을 바꾸므로 해당하지 않고, blueprint 에는 변환 도구가 없습니다.

```bat
:: 상태 보기 · 없는 모델 가져오기 · 내려받기
cargo run -p blueprint-models -- status
cargo run -p blueprint-models -- import
cargo run -p blueprint-models -- download zoro
```

앱은 켜질 때 없는 모델을 먼저 가져오고(`ensure_models`), 가져올 곳에도 없을 때만 설정 화면을 열어 내려받습니다.

## 3. 발전소 쪽에서 모델을 쓰는 곳

| 단계 | 모델 | 무엇을 | 코드 |
| --- | --- | --- | --- |
| Stage 1 | SigLIP2 비전 타워 + 크롭 앵커 | 현장 스캔 · 도면에서 제목 · 입력란 · 표 · 서명만 고르고 안내문(범례)을 버림 | `ai::document`, `ai::siglip2::plan` |
| Stage 2 | gocr 비전 → 언어 | 고른 조각의 글자 (KKS · 수치 · MOC 번호 · 서명자) | `ai::gocr`, `extract.rs` |
| Stage 4 | Zoro-1 백본 + `jev_head` (0-Generation 포인터) | 원인 판정 선택지 확률, SDS 축별 질문 | `ai::pointer`, `verdict::judge_by_axes`, `gate3_substantive.rs` |
| Stage 5 | Zoro-1 백본 + 임베딩 표 (글쓰기) | AOP 원인 진단 요약 | `drafting::draft_aop` |
| 현장 소명 | Zoro-1 글쓰기 | 자유 서술 편차 소명을 표준 코드로 | `drafting::normalize_excuse` |

`SharedResidency` 규칙은 그대로입니다: GPU 에는 한 번에 한 모델, 모델 안에서도 지금 단계의 묶음만 올라갑니다. 현장 스캔이 없는 신호 알람은 SigLIP2 · gocr 를 올리지 않고 바로 Zoro-1 로 갑니다 (Stage 1 · 2 `건너뜀`).

## 4. 판단 프롬프트

zoro-gravity 의 포인터 프롬프트(표준 JSON, 선택지는 닫는 따옴표 위치, 스케일 1/32)를 그대로 쓰고 지시문만 `발전소 계통 이상 진단 보조입니다` 로 바꿨습니다. zoro-gravity 의 형식 탐색에서 지시문 · 채팅 틀은 판단을 거의 바꾸지 않았습니다. 다만 **jev_head 의 확률 보정(스케일)은 민원 사례로 한 것**이고 발전소 사례로는 보정하지 않았습니다. 그래서 확신도는 방향 참고용이고, 자동 확정 기준 0.95 는 그림자 운영 데이터 없이 낮추지 않습니다.

## 5. 아직 하지 않은 것

- `.github/workflows/cross-verify.yml` 은 이 저장소의 패키지 이름 · 경로로 맞췄지만 **GitHub Actions 러너에서 돌려 보지 않았습니다.** CI 는 로컬 빌드로 갈음하지 않고 푸시해서 실제 러너로 확인합니다. 장치 시험 수(`DEVICE_CHECKS: 7`)는 zoro-gravity 에서 가져온 장치 시험 그대로입니다.
- 판단 프리필 상태 재사용(Part 8-3 의 `GdnState` 프리픽스 스냅샷): zoro 모델에 접두 상태 저장 · 복원이 없습니다. 판단 1회는 ROCm 에서 약 0.6 초입니다.
- 도면 전용 크롭 앵커 학습(P12 "도면 시각 프로브 학습"): 지금 앵커는 zoro-gravity 가 민원 서식으로 학습한 것입니다. 도면에서의 측정은 `docs/verification.md` 에 있습니다.
