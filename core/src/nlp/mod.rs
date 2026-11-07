//! 자연어 처리 (검색 질의 분석용). 모두 CPU 에서 돕니다 — GPU 상주 관리([`crate::ai::residency`])와 겹치지 않습니다.
//!
//! - `stanza`: Stanza 품사(upos · xpos) · 원형. `stanza-onnx-export` 가 만든 ONNX(format 2)를 순수 Rust 실행기(tract)로 돌립니다.
//!   PyTorch Stanza 와 끝에서 끝까지 같은 결과인지는 그 저장소의 `verify_onnx.py` 가 잽니다 (en · ko 30문장 100 %).
//!   한국어 원형은 형태소를 `+` 로 이은 꼴(`급수펌프+의`, `높+ㄴ`)이고 xpos 는 세종 품사(`nq+jcm`)라 조사 · 어미를 정확히 떼는 데 씁니다.
//!
//! 모델 폴더: `<models>/stanza/<언어>/` (vocab.json · pos.onnx · pretrain_vocab.json · lemma_encoder.onnx · lemma_decoder.onnx · lemma_dict.json).

pub mod stanza;

pub use stanza::{StanzaLang, Tagged};
