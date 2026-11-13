//! candle 기반 AI 엔진. zoro-gravity 의 `core/src/ai` 를 그대로 가져왔습니다 (같은 모델 파일, 같은 가중치 상주 관리).
//!
//! - `accel`: 장치 선택(ROCm → Vulkan → CPU) 과 dtype 정책
//! - `verify`: GPU 커널 자가 검증 (CPU 기준값과 비교해 틀리면 그 경로만 끔)
//! - `jev_head`: Zoro-1 판단 헤드 (질의·키 투영, 0-Generation 포인터 판단)
//! - `pointer`: 판단 질문을 프롬프트로 만들고 백본 은닉 상태를 `jev_head` 로 점수화
//! - `gguf_model`: 선택된 양자화 모델(`active.json` → GGUF) 열기
//! - `weights` · `residency`: GGUF 에서 필요한 텐서만 읽기, 모델 하나 · 단계별 묶음만 올리기
//! - `attention` · `tokenizer`: 모델들이 함께 쓰는 주의 연산과 토크나이저 (`fused_attention`: 디코딩 패스의 묶음 질의 주의 HIP 커널)
//! - `kv_turbo`: KV 캐시 FP8 압축 (장치 메모리가 모자랄 때)
//! - `speculative`: 자기 추측 디코딩 (같은 가중치의 블록 초안 + 자기회귀 검증)
//! - `zoro`: 판단 백본, `gocr`: 문서 인식, `siglip2`: 읽을 곳 고르기
//! - `ocr`: 비전 → 언어 단계 전환, `document`: 문서 한 장 읽기
//! - `pyramid`: 피라미드 양자화 (텐서마다 정밀도를 달리한 GGUF 와 잘라 둔 출력 투영을 원본 BF16 에서 한 번 양자화해 만듦)
//! - `original` · `kquant`: 원본 safetensors(BF16) 읽기 · GGML 블록 형식 인코더 (gravity-ocr `quant.py` 의 척도 탐색)
//! - `embed`: 검색용 문장 임베딩 (granite-embedding-97m-multilingual-r2, CPU)

pub mod accel;
pub mod attention;
pub mod document;
pub mod embed;
pub mod fused_attention;
pub mod gguf_model;
pub mod gocr;
pub mod head;
pub mod jev_head;
pub mod kquant;
pub mod kv_turbo;
pub mod ocr;
pub mod original;
pub mod pointer;
pub mod pyramid;
pub mod residency;
pub mod runtime;
pub mod siglip2;
pub mod speculative;
pub mod tokenizer;
pub mod trace;
pub mod verify;
pub mod weights;
pub mod zoro;

/// 모델 폴더 위치 (`%LOCALAPPDATA%\blueprint\models` 등). `blueprint-models` 크레이트입니다.
pub use blueprint_models as model_store;
