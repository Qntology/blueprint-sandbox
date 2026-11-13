//! 모델 폴더의 `tokenizer.json` (바이트 수준 BPE) 을 쓰는 토크나이저. zoro 와 gocr 가 함께 씁니다.
//! zoro 는 판단 프롬프트의 바이트 위치(`PointerPrompt`)를 토큰 번호로 바꾸는 데도 씁니다.

use std::path::Path;

use anyhow::{anyhow, Context, Result};
use tokenizers::Tokenizer;

pub struct ModelTokenizer {
    inner: Tokenizer,
    /// 추가 · 특수 토큰 번호 (`<|im_start|>`, `[gMASK]` …)
    added: std::collections::HashSet<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Encoded {
    pub ids: Vec<u32>,
    /// 토큰마다 원문에서 차지하는 바이트 범위 `[시작, 끝)`
    pub offsets: Vec<(usize, usize)>,
}

impl ModelTokenizer {
    pub fn from_file(path: &Path) -> Result<Self> {
        let inner = Tokenizer::from_file(path)
            .map_err(|e| anyhow!("{e}"))
            .with_context(|| format!("토크나이저를 읽을 수 없습니다: {}", path.display()))?;
        Ok(Self::from_tokenizer(inner))
    }

    pub fn from_tokenizer(inner: Tokenizer) -> Self {
        let added = inner.get_added_tokens_decoder().keys().copied().collect();
        Self { inner, added }
    }

    /// 어휘 크기 (추가 토큰 포함)
    pub fn vocab_size(&self) -> usize {
        self.inner.get_vocab_size(true)
    }

    /// 추가 · 특수 토큰인가 (출력 투영을 잘라 둘 때 늘 남기는 행, `ai::pyramid`)
    pub fn is_added(&self, id: u32) -> bool {
        self.added.contains(&id)
    }

    /// 시작·끝 토큰을 붙이지 않고 인코딩합니다 (채팅 형식은 프롬프트 문자열에 이미 들어 있습니다).
    /// `<|im_start|>` 같은 특수 토큰 문자열은 하나의 토큰으로 인식됩니다.
    pub fn encode(&self, text: &str) -> Result<Encoded> {
        let e = self.inner.encode(text, false).map_err(|e| anyhow!("{e}"))?;
        Ok(Encoded { ids: e.get_ids().to_vec(), offsets: e.get_offsets().to_vec() })
    }

    pub fn decode(&self, ids: &[u32]) -> Result<String> {
        self.inner.decode(ids, false).map_err(|e| anyhow!("{e}"))
    }

    pub fn token_to_id(&self, token: &str) -> Option<u32> {
        self.inner.token_to_id(token)
    }

    /// 토큰 하나의 원래 바이트. 바이트 수준 BPE 는 한글 한 글자를 토큰 둘 셋으로 쪼개므로(`입` = `Ġìŀ` + `ħ`), 토큰 하나만
    /// `decode` 하면 깨진 글자(U+FFFD)가 됩니다. 어휘 글자를 GPT-2 바이트 표로 되돌리고, 표에 없는 글자가 있는 토큰(추가 · 특수 토큰)은 글 그대로입니다.
    pub fn token_bytes(&self, id: u32) -> Option<Vec<u8>> {
        let piece = self.inner.id_to_token(id)?;
        let table = byte_level_table();
        let bytes: Option<Vec<u8>> = piece.chars().map(|c| table.get(&c).copied()).collect();
        bytes.or_else(|| Some(piece.into_bytes()))
    }
}

/// GPT-2 바이트 수준 BPE 의 글자 → 바이트 표 (`bytes_to_unicode` 의 역)
fn byte_level_table() -> &'static std::collections::HashMap<char, u8> {
    static TABLE: std::sync::OnceLock<std::collections::HashMap<char, u8>> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut printable: Vec<u32> = (b'!' as u32..=b'~' as u32).chain(0xA1..=0xAC).chain(0xAE..=0xFF).collect();
        let mut chars = printable.clone();
        let mut n = 0;
        for b in 0..256u32 {
            if !printable.contains(&b) {
                printable.push(b);
                chars.push(256 + n);
                n += 1;
            }
        }
        printable.iter().zip(&chars).map(|(&b, &c)| (char::from_u32(c).unwrap(), b as u8)).collect()
    })
}

/// 원문의 `byte` 번째 바이트를 덮는 토큰의 번호. 같은 바이트를 덮는 토큰이 여럿이면 마지막 것입니다
/// (여러 바이트짜리 글자가 바이트 단위 토큰으로 쪼개진 경우, 글자를 끝까지 읽은 토큰).
pub fn token_covering(offsets: &[(usize, usize)], byte: usize) -> Option<usize> {
    offsets.iter().rposition(|&(s, e)| s <= byte && byte < e)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokenizers::models::bpe::{Vocab, BPE};
    use tokenizers::pre_tokenizers::byte_level::ByteLevel;
    use tokenizers::AddedToken;

    /// 병합 규칙이 없는 바이트 수준 BPE: 바이트마다 토큰 하나. 특수 토큰은 통째로 하나.
    fn byte_tokenizer() -> ModelTokenizer {
        let mut chars: Vec<char> = ByteLevel::alphabet().into_iter().collect();
        chars.sort();
        let vocab: Vocab = chars.iter().enumerate().map(|(i, c)| (c.to_string(), i as u32)).collect();
        let bpe = BPE::builder().vocab_and_merges(vocab, vec![]).build().expect("BPE");
        let mut tk = Tokenizer::new(bpe);
        tk.with_pre_tokenizer(Some(ByteLevel::default().add_prefix_space(false)));
        tk.with_decoder(Some(tokenizers::decoders::byte_level::ByteLevel::default()));
        tk.add_special_tokens([AddedToken::from("<|im_start|>", true), AddedToken::from("<|im_end|>", true)])
            .expect("특수 토큰");
        ModelTokenizer::from_tokenizer(tk)
    }

    #[test]
    fn offsets_are_byte_offsets_even_with_multibyte_text() -> Result<()> {
        let tk = byte_tokenizer();
        let text = "<|im_start|>user\n안녕 ABC_1<|im_end|>\n";
        let enc = tk.encode(text)?;
        assert_eq!(enc.ids.len(), enc.offsets.len());
        // 특수 토큰은 하나의 토큰이고, 그 범위가 문자열 전체를 덮습니다.
        assert_eq!(enc.offsets[0], (0, "<|im_start|>".len()));
        let start_id = tk.token_to_id("<|im_start|>").expect("특수 토큰 번호");
        assert_eq!(enc.ids[0], start_id);

        // ASCII 글자는 자기 바이트 위치를 정확히 덮는 토큰 하나입니다.
        for (i, ch) in text.char_indices().filter(|(_, c)| c.is_ascii() && !"<|>_".contains(*c)) {
            if text[..i].ends_with("<|im_start") || i < "<|im_start|>".len() {
                continue;
            }
            let tok = token_covering(&enc.offsets, i).expect("모든 바이트는 어떤 토큰에 덮입니다");
            if enc.offsets[tok] == (i, i + 1) {
                assert_eq!(tk.decode(&[enc.ids[tok]])?, ch.to_string());
            }
        }
        // 뒤쪽 ASCII 구간 "ABC_1" 의 마지막 글자 '1'
        let one = text.find("_1").unwrap() + 1;
        let tok = token_covering(&enc.offsets, one).unwrap();
        assert_eq!(enc.offsets[tok], (one, one + 1));
        assert_eq!(tk.decode(&[enc.ids[tok]])?, "1");
        // 한글은 3바이트: 글자 안의 어느 바이트도 어떤 토큰에는 덮이고, 글자 끝까지 읽은 토큰(마지막)을 돌려줍니다.
        let ko = text.find('안').unwrap();
        let first = token_covering(&enc.offsets, ko).unwrap();
        let last = token_covering(&enc.offsets, ko + 2).unwrap();
        assert!(last >= first);
        // 원문 밖은 None
        assert_eq!(token_covering(&enc.offsets, text.len()), None);
        Ok(())
    }

    #[test]
    fn the_last_covering_token_wins_when_spans_overlap() {
        let offsets = [(0, 3), (0, 3), (0, 3), (3, 4)];
        assert_eq!(token_covering(&offsets, 1), Some(2));
        assert_eq!(token_covering(&offsets, 3), Some(3));
        assert_eq!(token_covering(&offsets, 4), None);
        assert_eq!(token_covering(&[], 0), None);
    }

    #[test]
    fn missing_files_are_reported() {
        assert!(ModelTokenizer::from_file(Path::new("does-not-exist.json")).is_err());
    }

    /// 바이트 토큰 하나는 decode 하면 깨지지만(U+FFFD) token_bytes 는 원래 바이트를 돌려주고, 이어 붙이면 원문입니다.
    #[test]
    fn token_bytes_give_the_raw_bytes_of_split_hangul() -> Result<()> {
        let tk = byte_tokenizer();
        let text = "2026-11-04 입래현";
        let enc = tk.encode(text)?;
        let joined: Vec<u8> = enc.ids.iter().flat_map(|&id| tk.token_bytes(id).unwrap()).collect();
        assert_eq!(joined, text.as_bytes());
        let space = enc.ids[text.find(' ').unwrap()];
        assert_eq!(tk.token_bytes(space), Some(vec![b' ']));
        let lead = enc.ids[text.find('입').unwrap()];
        assert_eq!(tk.token_bytes(lead), Some(vec![0xEC]));
        assert!(tk.decode(&[lead])?.contains('\u{FFFD}'));
        // 특수 토큰은 글 그대로
        assert_eq!(tk.token_bytes(tk.token_to_id("<|im_end|>").unwrap()), Some(b"<|im_end|>".to_vec()));
        Ok(())
    }
}
