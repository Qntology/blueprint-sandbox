# AI 엔진과 모델 (zoro-gravity 에서 가져온 것)

blueprint 는 zoro-gravity 와 **같은 모델 · 같은 엔진**을 씁니다. 새 모델을 들이지 않았습니다 (logis-center 계보의 Qwen3.5 2B 는 쓰지 않고, 다국어 문서 · VLM · 추론은 모두 Zoro-1 이 맡습니다).
엔진에 더한 것은 5 · 6절에 있습니다: 원본 BF16 에서 한 번 양자화하는 피라미드 양자화와 출력 투영 잘라 두기, Zoro-1 비전(mmproj) 순전파와 다국어 VLM 재판독(선택). 마스터 블록 매칭 · 표제란 보호는 [verification.md](verification.md) 15절에 있습니다.

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
| Stage 1 | SigLIP2 비전 타워 + 크롭 앵커 | 현장 스캔 · 도면에서 제목 · 입력란 · 표 · 서명만 고르고 안내문(범례)을 버림. 글자 모양 덩어리가 둘 미만인 띠(상자 테두리 · 배관뿐)는 읽지 않음 | `ai::document`, `ai::siglip2::plan` · `ink::glyphs` |
| Stage 1 (Red-line · A0 재스캔) | SigLIP2 특징 격자(이미 뽑은 것) + 잉크 | 어느 마스터 도면인지(패치 코사인) 가리고, 마스터와 잉크가 달라진 곳에 걸친 조각만 그 둘레로 좁혀 읽음. 표제란 · 서명은 늘 읽음 | `ai::siglip2::delta`, `DocumentReader::read_pages_delta` |
| Stage 2 | gocr 비전 → 언어 | 고른 조각의 글자 (KKS · 수치 · MOC 번호 · 서명자) | `ai::gocr`, `extract.rs` |
| Stage 4 | Zoro-1 백본 + `jev_head` (0-Generation 포인터) | 원인 판정 선택지 확률, SDS 축별 질문 | `ai::pointer`, `verdict::judge_by_axes`, `gate3_substantive.rs` |
| Stage 5 | Zoro-1 백본 + 임베딩 표 (글쓰기) | AOP 원인 진단 요약 | `drafting::draft_aop` |
| 현장 소명 | Zoro-1 글쓰기 | 자유 서술 편차 소명을 표준 코드로 | `drafting::normalize_excuse` |

`SharedResidency` 규칙은 그대로입니다: GPU 에는 한 번에 한 모델, 모델 안에서도 지금 단계의 묶음만 올라갑니다. 현장 스캔이 없는 신호 알람은 SigLIP2 · gocr 를 올리지 않고 바로 Zoro-1 로 갑니다 (Stage 1 · 2 `건너뜀`).

예외 하나를 켤 수 있습니다: `BLUEPRINT_COEXIST=siglip2` 이면 SigLIP2 비전 타워(약 0.8 GB)를 모델이 바뀌어도 남겨 둡니다. 다음 단계를 올리고도 여유분 640 MB + 공존 여유 512 MB(`BLUEPRINT_COEXIST_MARGIN_MB`)가 남을 때만 남기고, 모자라면 다음 단계를 올리기 **전에** 내립니다. 실측 이득이 스캔 한 번에 0.3 초 남짓이라 기본은 꺼짐입니다 ([verification.md](verification.md) 8.2).

앱은 엔진(세션 · 감사 기록)을 잠그는 시간과 GPU 를 쓰는 시간을 나눕니다 (`state_machine::staged`): 준비(엔진 잠금) → 모델(GPU 잠금만) → 반영(엔진 잠금). 모델이 도는 동안에도 콘솔의 조회 · 결정이 멈추지 않습니다. GPU 잠금은 모델 계산 전체를 덮습니다 (상주 관리자는 쓰는 중인 묶음을 내리지 못하므로).

## 4. 판단 프롬프트

zoro-gravity 의 포인터 프롬프트(표준 JSON, 선택지는 닫는 따옴표 위치, 스케일 1/32)를 그대로 쓰고 지시문만 `발전소 계통 이상 진단 보조입니다` 로 바꿨습니다. zoro-gravity 의 형식 탐색에서 지시문 · 채팅 틀은 판단을 거의 바꾸지 않았습니다. 다만 **jev_head 의 확률 보정(스케일)은 민원 사례로 한 것**이고 발전소 사례로는 보정하지 않았습니다. 그래서 확신도는 방향 참고용이고, 자동 확정 기준 0.95 는 그림자 운영 데이터 없이 낮추지 않습니다.

## 5. 원본 BF16 과 피라미드 양자화 (`ai::original` · `ai::kquant` · `ai::pyramid`)

양자화는 **원본 BF16 가중치에서 한 번만** 합니다. 이미 Q8_0 으로 양자화된 q8 파일을 풀어 Q4_K · Q5_K 로 다시 깎지 않습니다 (두 번 양자화한 오차가 겹침).

- **원본**: 모델 폴더의 `bf16/` 에 Hugging Face 의 safetensors 를 그대로 둡니다. gocr 는 [trillionlabs/GravityOCR-KO](https://huggingface.co/trillionlabs/GravityOCR-KO) (MIT, 2.1 GB, gravity-ocr-qsa-ngram 의 `models/` 에 같은 파일이 있으면 하드 링크), zoro 는 [trillionlabs/Zoro-1](https://huggingface.co/trillionlabs/Zoro-1) (Apache-2.0, 조각 17개 8.5 GB). safetensors 는 머리말(JSON)만 해석하고 값은 메모리 매핑에서 행 단위로 읽습니다.
- **대응표 · 감사**: GGUF 텐서 이름 → 원본 이름을 모델마다 적고(`original::Family`), 쓰기 전에 지금 파일의 같은 텐서와 값을 견줍니다. `pyramid -- audit <모델>` 이 전체를 확인합니다: 두 모델 모두 mmproj(BF16)는 원본과 비트까지 같고, q8 의 Q8_0 은 원본을 GGML 기준(블록 최댓값 / 127)으로 양자화한 바이트와 같습니다 — 지금 쓰는 q8 은 원본에서 한 번 양자화한 파일입니다 ([verification.md](verification.md) 15절).
- **재료 규칙**(`pyramid::material_rows`): 원본이 있으면 원본에서, 없으면 저장 형식이 원본 그대로(BF16 · F16 · F32)인 텐서만 그 값에서 읽습니다. 저장된 Q8_0 만 있으면 거부합니다. 그래서 비전만 바꾸는 배분은 앱이 이미 받는 BF16 mmproj 만으로 만들 수 있고, 언어 몸통을 바꾸는 배분은 원본이 있어야 합니다.
- **인코더**(`kquant`): GGML 형식(Q8_0 · Q6_K · Q5_K · Q4_K)에 gravity-ocr `quant.py` 의 척도 탐색을 씁니다. 블록마다 중요도 `rms(블록) + |x|` 로 가중한 최소제곱 격자 탐색(대칭 후보 19개 · 비대칭 21개), 중요도 행렬(열마다 입력 제곱 평균)이 있으면 곱합니다. 비트 배치는 GGML 그대로라 candle 이 장치에서 곱합니다. Q3_K 는 만들지 않습니다.
- **배분**(`pyramid::gocr_plan` · `zoro_plan`): gocr 비전은 gravity-ocr 측정 배분에 보호를 더한 것(16 ~ 22 층 주의 Q8_0 · MLP Q6_K, downsample · merger 관문 Q8_0 또는 원본 BF16), 언어는 측정 배분 · 민감층 보호 혼합, Zoro-1 은 처음 · 마지막 4층과 주의 · 선형 주의 게이트 Q8_0 · Q6_K, 가운데 FFN Q4_K · Q5_K. 무엇을 쓰는지는 측정으로 정했습니다 ([verification.md](verification.md) 15절, `pyramid::DEFAULT_GOCR_PLAN` · `DEFAULT_ZORO_PLAN`):
  gocr 기본은 `vision-q8+head` (비전 선형 가중치 전부 Q8_0 + 출력 투영 잘라 두기, 언어는 q8 그대로). 비전 피라미드 · 언어 K 형식은 원본 대비 충실도를 잃고,
  gocr 최대 VRAM 은 비전 · 언어 중 큰 쪽이라 비전을 Q8_0 보다 더 줄여도 내려가지 않습니다. `BLUEPRINT_GOCR_PLAN` · `BLUEPRINT_ZORO_PLAN` 으로 다른 배분을 고릅니다.
- **사용할 때 양자화**(`pyramid::ensure`, `runtime::open`): 앱이 모델을 열기 전에 기본 배분 · 레시피 번호(`pyramid::RECIPE`)의 파일이 없으면 CPU 로 만들어 모델 폴더에 두고 `active.json` 의 `pyramid` 에 적습니다. 규칙을 바꾸면 레시피 번호를 올려 다시 만들게 합니다. 원본이 꼭 필요한 배분인데 원본이 없으면 만들지 않고 q8 로 엽니다.
- **쓰는 곳**: K 형식을 장치에서 곱하는 백엔드(ROCm · CUDA · Metal · CPU)만 피라미드 파일을 엽니다. 포크의 Vulkan 은 Q8_0 만 장치에서 곱하고 나머지는 호스트 CPU 로 곱하므로 q8 그대로입니다. `BLUEPRINT_PYRAMID=0` 이면 늘 q8, `1` 이면 장치와 상관없이 있으면 씁니다.
- **출력 투영 잘라 두기**(`ai::head`): 쓰는 글자의 행만 `output.weight` 로 남기고 나머지를 `output_rest.weight` 로 나눕니다. 초안은 남긴 행으로, 검증 · 프리필은 잘린 행의 최댓값까지 봅니다 (잘린 행 검사). 검증이 잘린 행의 토큰을 한 번 고르면(가나 · 아랍 문자 · 발음 기호 등) 그 조각의 남은 초안도 전체 어휘로 고릅니다 (`gocr::Session::full_draft`, 글은 같고 다른 문자에서 느려지지 않음). 두 부분 모두 저장과 같은 Q8_0 이면 행 바이트를 그대로 옮겨(다시 양자화 없음) 원본 없이 만듭니다. gocr 기본 배분에 넣었습니다 (디코드 −7.3 %, 출력 그대로, 15절).

- **정밀 모드(선택)**: RX 6600 (gfx1032) 는 BF16 행렬곱 하드웨어가 없어, BF16 으로 저장된 일반 가중치를 F32 로 넓혀 올리면(`BLUEPRINT_DENSE=f32`) 원본 그대로 계산하면서 더 빠릅니다.
  gocr 언어도 원본 F32 로 두면(`BLUEPRINT_GOCR_PLAN=f32`, 원본 필요) Q8_0 커널보다 빠릅니다 (문서 판독 −18.8 %, 정답 기준 손해 0). 대신 gocr 최대 VRAM 이 2.2 GB 라 기본은 아닙니다.
- **묶음 질의 주의 커널(선택)**: `ai::fused_attention` (HIP, `core/kernels/attend.hip`, `core/build.rs` 가 `rocm` 기능에서 hipcc 로 컴파일). 자가 검증은 통과하지만 이 GPU 에서는
  rocBLAS 행렬곱 경로보다 느려 기본은 꺼짐입니다 (`BLUEPRINT_FUSED_ATTN=1`).

```bat
:: 원본과 지금 파일의 대응 · 한 번 양자화 확인, 측정 기준(원본을 F32 로 둔 파일), 배분 파일 만들기
cargo run -p blueprint-core --release --example pyramid -- audit gocr
cargo run -p blueprint-core --release --example pyramid -- reference gocr out/quant/ref
cargo run -p blueprint-core --release --example pyramid -- gocr vision-q8+head
:: 앱처럼 기본 배분 파일이 없으면 만들기 (사용할 때 양자화)
cargo run -p blueprint-core --release --example pyramid -- ensure
```

## 6. Zoro-1 비전 (`ai::zoro::vision`)

Zoro-1 의 mmproj(`model.visual.*`)를 순전파해 이미지 토큰(폭 2560)을 만들고, 프롬프트의 `<|image_pad|>` 자리에 넣어 블록 0 부터 돌립니다.
텍스트만 쓰는 판단 · 글쓰기는 비전을 올리지 않습니다. 이미지가 있으면 비전 올림 → 인코딩 → 비전 내림 → 블록 올림 → 프리필 · 디코딩 순서라 GPU 에는 한 번에 한 묶음입니다.
위치는 M-RoPE (시간 · 높이 · 너비 섹션 [11, 11, 10], 주파수를 번갈아 배정)이고, 이미지 다음 글자의 위치는 `기준 + max(행, 열)` 입니다. transformers 와의 대조는 [verification.md](verification.md) 15절에 있습니다.
gocr 가 읽은 조각을 Zoro-1 비전으로 다시 읽는 경로는 `BLUEPRINT_VLM`(`off` 기본 · `all` · 확신 기준 숫자)로 켭니다. 아랍어처럼 gocr 가 읽지 못하는 문자의 문서에 씁니다:
12개 언어 실측에서 Zoro-1 은 합성 한국어 조각을 gocr 보다 잘 읽었지만(6/6 · 3/6) 다른 언어에서 그럴듯한 낱말로 바꾸거나 글자를 되풀이했고,
조각당 약 6 배 느립니다 ([verification.md](verification.md) 15.11).

## 7. 아직 하지 않은 것

- `.github/workflows/cross-verify.yml` 은 이 저장소의 패키지 이름 · 경로로 맞췄지만 **GitHub Actions 러너에서 돌려 보지 않았습니다.** CI 는 로컬 빌드로 갈음하지 않고 푸시해서 실제 러너로 확인합니다. 장치 시험 수(`DEVICE_CHECKS: 7`)는 zoro-gravity 에서 가져온 장치 시험 그대로입니다.
- 판단 프리필 상태 재사용(Part 8-3 의 `GdnState` 프리픽스 스냅샷): zoro 모델에 접두 상태 저장 · 복원이 없습니다. 판단 1회는 ROCm 에서 약 0.6 초입니다.
- 도면 전용 크롭 앵커 학습(P12 "도면 시각 프로브 학습"): 지금 앵커는 zoro-gravity 가 민원 서식으로 학습한 것입니다. 도면에서의 측정은 `docs/verification.md` 에 있습니다.
