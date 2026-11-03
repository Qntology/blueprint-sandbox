//! 비식별과 누출 방지.
//!
//! - 근무자 실명은 주제어실 화면에서 가리고(`김○현`), 열람은 고위험 건에서만 허용하며 감사 로그에 남깁니다.
//! - 현장 단말(키오스크) 화면에는 내부 판정 코드 · 확신도 · 마인드맵 정보가 들어가면 안 됩니다 (기획 Part 8-2 보안 불변식).
//! - 감사 로그에는 사번 대신 솔트 해시를 씁니다.

use sha2::{Digest, Sha256};

/// 작업자 실명 열람 허용 기준 위험도 (zoro-gravity 의 제3자 열람과 같은 70%)
pub const UNMASK_RISK_THRESHOLD: f32 = 0.70;

/// 현장 단말에 보내면 안 되는 낱말 (zoro-gravity `l4.js` 의 FORBIDDEN 대응).
/// 모델 이름 자체는 막지 않습니다. 기획안의 소명 화면이 "자유 서술 → Zoro-1이 표준 코드로 자동 분류" 라고 알리기 때문이고,
/// 막는 것은 Zoro-1 이 무엇을 판단했는지(단계 이름 · 확신도 · 판정 코드)입니다.
pub const FIELD_FORBIDDEN: &[&str] = &[
    "G1_", "G2_", "G3_", "G4_", "SEC_", "SDS_", "확신도", "Zoro-1 판단", "Zoro-1 글쓰기", "jev_head", "Lattice", "마인드맵", "근본 원인", "고위험", "원장 수치", "보팅", "DCS 표시값",
];

/// 솔트 SHA-256 해시 (소문자 hex)
pub fn salted_hash(salt: &[u8], raw: &str) -> String {
    let mut h = Sha256::new();
    h.update(salt);
    h.update(raw.as_bytes());
    hex::encode(h.finalize())
}

/// 이름 가리기. 세 글자 `김태현` → `김○현`, 두 글자 `김태` → `김○`.
pub fn mask_name(name: &str) -> String {
    let chars: Vec<char> = name.chars().collect();
    match chars.len() {
        0 => String::new(),
        1 => "○".to_string(),
        2 => format!("{}○", chars[0]),
        n => format!("{}{}{}", chars[0], "○".repeat(n - 2), chars[n - 1]),
    }
}

/// 사번 가리기. `OP-102` → `OP-***`, `E205` → `E***`
pub fn mask_id(id: &str) -> String {
    let head: String = id.chars().take_while(|c| c.is_ascii_alphabetic() || *c == '-').collect();
    format!("{head}***")
}

/// 글에서 찾은 금지어. 비어 있으면 안전합니다.
pub fn find_leaks<'a, S: AsRef<str>>(text: &str, terms: &'a [S]) -> Vec<&'a str> {
    terms.iter().map(AsRef::as_ref).filter(|t| !t.is_empty() && text.contains(t)).collect()
}

/// 위험도가 기준 이상일 때만 실명 열람을 허용합니다.
pub fn unmask_allowed(risk: f32) -> bool {
    risk >= UNMASK_RISK_THRESHOLD
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_ids_are_masked_like_the_plan() {
        assert_eq!(mask_name("김태현"), "김○현");
        assert_eq!(mask_name("김태"), "김○");
        assert_eq!(mask_name("남궁민수"), "남○○수");
        assert_eq!(mask_id("OP-102"), "OP-***");
        assert_eq!(mask_id("E205"), "E***");
    }

    #[test]
    fn field_text_with_internal_codes_is_caught() {
        let text = "G3_ADVERSE_UPSTREAM_CAUSE 확신도 0.97";
        let found = find_leaks(text, FIELD_FORBIDDEN);
        assert!(found.contains(&"G3_") && found.contains(&"확신도"));
        assert!(find_leaks("주제어실에 전송되었습니다. 접수 번호 F-1104-0001", FIELD_FORBIDDEN).is_empty());
    }

    #[test]
    fn hashes_are_salted_and_stable() {
        let a = salted_hash(b"s1", "E102");
        assert_eq!(a, salted_hash(b"s1", "E102"));
        assert_ne!(a, salted_hash(b"s2", "E102"));
        assert_eq!(a.len(), 64);
    }

    #[test]
    fn unmask_needs_seventy_percent() {
        assert!(!unmask_allowed(0.3));
        assert!(unmask_allowed(0.974));
    }
}
