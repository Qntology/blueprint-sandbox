//! 질의 언어 판정 · 글자 정규화 · 따옴표 이름 (12개 언어: ko en de fr es it pt nl cs ar ja zh-hans).
//!
//! logis-center `stanza::detect_query_lang` (c0e801c1 · fcfad2ab) 의 실측 규칙을 옮겼습니다.
//! - 따옴표로 묶은 이름(데이터 값)은 문장 언어를 정할 때 뺍니다: 일본어 문장 속 「Hauptspeisewasserpumpe A」 가 독일어로,
//!   영어 문장 속 '주급수펌프 A' 가 한국어로 판정되지 않게 (logis-center 커머스 질의 120개 중 32개가 이것으로 틀렸음).
//! - 라틴 · 아랍 문장 속 한중일 이름 어절도 뺍니다 (문장 낱말이 셋 이상이고 그 어절 수의 두 배 이상일 때만).
//! - 문자 체계: 한글 → ko (가나 · 한자보다 적어도 어절 끝이 한글이면 ko), 가나 → ja (간체에만 쓰는 한자가 둘 이상이면 zh-hans),
//!   가나 없는 한자 → zh-hans, 아랍 문자 → ar.
//! - 라틴 문자: 그 언어에만 쓰는 글자(ě š č ř ž ů ť ď ň → cs, ã õ → pt, ñ ¿ ¡ → es, ß ä ö ü → de)를 먼저 보고, 다음으로 검색 사전
//!   (lexicon_ml.json 의 기능어 · 도메인 낱말 · 닫힌 낱말) 투표: 낱말이 든 언어마다 1 / (그 낱말이 든 언어 수). 투표가 없거나 비기면
//!   whatlang 을 8개 라틴 언어 안에서만 돌리고(넓게 두면 짧은 질의가 라틴어 · 슬로바키아어로 떨어짐), 그래도 모르면 영어.

use std::collections::HashMap;
use std::sync::OnceLock;

/// 판정할 수 있는 언어 (Stanza 변환본 12개와 같음)
pub const LANGS: [&str; 12] = ["ko", "en", "de", "fr", "es", "it", "pt", "nl", "cs", "ar", "ja", "zh-hans"];
/// 라틴 문자 언어
pub const LATIN: [&str; 8] = ["en", "de", "fr", "es", "it", "pt", "nl", "cs"];

/// 언어 코드를 정적 문자열로 (모르면 None)
pub fn code(lang: &str) -> Option<&'static str> {
    LANGS.iter().find(|l| **l == lang).copied()
}

/// 띄어쓰기 없이 붙여 쓰는 언어 (낱말을 공백으로 나눌 수 없음)
pub fn glued(lang: &str) -> bool {
    matches!(lang, "ja" | "zh-hans")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Script {
    Hangul,
    Kana,
    Han,
    Arabic,
    Latin,
    Digit,
    Other,
}

impl Script {
    pub fn of(c: char) -> Option<Script> {
        Some(match c as u32 {
            0xAC00..=0xD7A3 | 0x1100..=0x11FF | 0x3130..=0x318F => Script::Hangul,
            0x3040..=0x30FF | 0x31F0..=0x31FF | 0xFF66..=0xFF9F => Script::Kana,
            0x4E00..=0x9FFF | 0x3400..=0x4DBF | 0xF900..=0xFAFF | 0x3005 => Script::Han,
            0x0600..=0x06FF | 0x0750..=0x077F | 0xFB50..=0xFDFF | 0xFE70..=0xFEFF => Script::Arabic,
            _ if c.is_ascii_digit() => Script::Digit,
            _ if c.is_alphabetic() && (c.is_ascii() || (0x00C0..=0x024F).contains(&(c as u32))) => Script::Latin,
            _ if c.is_alphabetic() => Script::Other,
            _ => return None,
        })
    }

    pub fn cjk(self) -> bool {
        matches!(self, Script::Hangul | Script::Kana | Script::Han)
    }
}

/// 따옴표 짝: 여는 글자 → 닫는 글자들. `'` · `"` 는 낱말 경계에서만 짝으로 봅니다 (l'opération · dell'edificio 의 아포스트로피 제외).
const QUOTE_PAIRS: [(char, &str); 10] = [
    ('\'', "'"),
    ('"', "\""),
    ('‘', "’"),
    ('“', "”"),
    ('„', "“”"),
    ('‚', "‘’"),
    ('«', "»"),
    ('‹', "›"),
    ('「', "」"),
    ('『', "』"),
];

/// 따옴표로 묶은 구의 바이트 범위 (따옴표 글자 포함)
pub fn quoted_ranges(query: &str) -> Vec<std::ops::Range<usize>> {
    let chars: Vec<(usize, char)> = query.char_indices().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let (start, c) = chars[i];
        if let Some((_, closers)) = QUOTE_PAIRS.iter().find(|(o, _)| *o == c) {
            let ascii = c == '\'' || c == '"';
            if !ascii || i == 0 || !chars[i - 1].1.is_alphanumeric() {
                let close = (i + 2..chars.len()).find(|&j| closers.contains(chars[j].1) && (!ascii || j + 1 == chars.len() || !chars[j + 1].1.is_alphanumeric()));
                if let Some(j) = close {
                    out.push(start..chars[j].0 + chars[j].1.len_utf8());
                    i = j + 1;
                    continue;
                }
            }
        }
        i += 1;
    }
    out
}

/// 따옴표 안의 이름들 (따옴표 글자를 뗀 글)
pub fn quoted_names(query: &str) -> Vec<String> {
    quoted_ranges(query)
        .into_iter()
        .map(|r| {
            let inner = &query[r];
            let mut cs = inner.chars();
            cs.next();
            cs.next_back();
            cs.as_str().trim().to_string()
        })
        .filter(|s| !s.is_empty())
        .collect()
}

/// 따옴표로 묶은 구를 공백으로 바꿉니다. 따옴표 밖에 글자가 하나도 없으면(이름만 친 질의) 원문 그대로.
pub fn strip_quoted(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    let mut at = 0;
    for r in quoted_ranges(query) {
        out.push_str(&query[at..r.start]);
        out.push(' ');
        at = r.end;
    }
    out.push_str(&query[at..]);
    if out.chars().any(|c| c.is_alphabetic()) {
        out
    } else {
        query.to_string()
    }
}

/// 간체에만 쓰는 한자 (일본어 신자체와 모양이 다른 것만). 가나가 섞여도 이 글자가 둘 이상이면 중국어 문장입니다.
const SIMPLIFIED_ONLY: &str = "单发运购订货车录页选结进领优费详记关实东们这么吗呢给让说话时间门问题对应开买卖务产业场报价钱银账码张网认证书动华汇过还达递邮输转统计设备线级类样历现为处见观视规则质从项阳伞阀热压电变组层显块图纸异闸风轮轴传仪测";

/// 라틴 · 아랍 문장 속 한중일 이름 어절을 뺍니다 (문장 낱말이 셋 이상이고 한중일 어절의 두 배 이상일 때만)
fn drop_cjk_names(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let has_cjk = |t: &str| t.chars().filter_map(Script::of).any(Script::cjk);
    let (mut latin, mut arabic, mut cjk) = (0usize, 0usize, 0usize);
    for t in &tokens {
        let scripts: Vec<Script> = t.chars().filter_map(Script::of).collect();
        if scripts.iter().any(|s| s.cjk()) {
            cjk += 1;
        } else if scripts.contains(&Script::Digit) {
            continue;
        } else if !scripts.is_empty() && scripts.iter().all(|s| *s == Script::Latin) {
            let letters: Vec<char> = t.chars().filter(|c| c.is_alphabetic()).collect();
            if !(letters.len() >= 2 && letters.iter().all(|c| c.is_uppercase())) {
                latin += 1;
            }
        } else if !scripts.is_empty() && scripts.iter().all(|s| *s == Script::Arabic) {
            arabic += 1;
        }
    }
    let words = latin.max(arabic);
    if cjk > 0 && words >= 3 && words >= 2 * cjk {
        tokens.into_iter().filter(|t| !has_cjk(t)).collect::<Vec<_>>().join(" ")
    } else {
        text.to_string()
    }
}

/// 비교용 꼴: 소문자 + 라틴 문자 발음 구별 기호 떼기 (é → e, ß → ss, č → c) + 아랍 문자 정규화(알리프 꼴 · 모음 부호 · 늘임표).
/// 사용자가 악센트 없이 쳐도(presion · temperature) 사전 낱말과 맞게 합니다. 한글 · 가나 · 한자는 그대로.
pub fn fold(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars().flat_map(char::to_lowercase) {
        match c {
            'à' | 'á' | 'â' | 'ã' | 'ä' | 'å' | 'ā' | 'ą' => out.push('a'),
            'ç' | 'č' | 'ć' => out.push('c'),
            'ď' => out.push('d'),
            'è' | 'é' | 'ê' | 'ë' | 'ě' | 'ē' | 'ę' => out.push('e'),
            'ì' | 'í' | 'î' | 'ï' | 'ī' => out.push('i'),
            'ñ' | 'ň' | 'ń' => out.push('n'),
            'ò' | 'ó' | 'ô' | 'õ' | 'ö' | 'ø' | 'ō' => out.push('o'),
            'ř' => out.push('r'),
            'š' | 'ś' => out.push('s'),
            'ť' => out.push('t'),
            'ù' | 'ú' | 'û' | 'ü' | 'ů' | 'ū' => out.push('u'),
            'ý' | 'ÿ' => out.push('y'),
            'ž' | 'ź' | 'ż' => out.push('z'),
            'ß' => out.push_str("ss"),
            'œ' => out.push_str("oe"),
            'æ' => out.push_str("ae"),
            // 아랍 문자: 알리프 꼴(أ إ آ ٱ) → ا, 모음 부호 · 늘임표는 뗌
            'أ' | 'إ' | 'آ' | 'ٱ' => out.push('ا'),
            'ى' => out.push('ي'),
            '\u{064B}'..='\u{0652}' | '\u{0640}' => {}
            // 전각 영숫자 (ＡＢＣ１２３) → 반각
            '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}' => {
                out.push(char::from_u32(c as u32 - 0xFEE0).unwrap_or(c));
            }
            // 아랍 · 페르시아 숫자 → 0-9
            '\u{0660}'..='\u{0669}' => out.push(char::from_u32(c as u32 - 0x0660 + '0' as u32).unwrap_or(c)),
            '\u{06F0}'..='\u{06F9}' => out.push(char::from_u32(c as u32 - 0x06F0 + '0' as u32).unwrap_or(c)),
            // 마이크로 기호 → 그리스 뮤 (원장 단위 µm · μm 를 같게)
            'µ' => out.push('μ'),
            _ => out.push(c),
        }
    }
    out
}

/// 질의 글자를 미리 고릅니다: 전각 숫자 · 아랍 숫자를 반각으로 (대소문자 · 악센트는 그대로 — 언어 판정 · Stanza 가 씀)
pub fn normalize_query(q: &str) -> String {
    q.chars()
        .map(|c| match c {
            '\u{FF10}'..='\u{FF19}' | '\u{FF21}'..='\u{FF3A}' | '\u{FF41}'..='\u{FF5A}' => char::from_u32(c as u32 - 0xFEE0).unwrap_or(c),
            '\u{0660}'..='\u{0669}' => char::from_u32(c as u32 - 0x0660 + '0' as u32).unwrap_or(c),
            '\u{06F0}'..='\u{06F9}' => char::from_u32(c as u32 - 0x06F0 + '0' as u32).unwrap_or(c),
            '\u{3000}' => ' ',
            _ => c,
        })
        .collect()
}

/// 투표 낱말: 라틴 언어마다 그 언어 사전 낱말(기능어 · 도메인 · 닫힌 낱말을 낱말로 쪼갠 것) → 그 낱말이 든 언어들
fn vote_vocab() -> &'static HashMap<String, Vec<&'static str>> {
    static V: OnceLock<HashMap<String, Vec<&'static str>>> = OnceLock::new();
    V.get_or_init(|| {
        let mut map: HashMap<String, Vec<&'static str>> = HashMap::new();
        for lang in LATIN {
            let Some(lex) = super::ml::lex(lang) else { continue };
            let mut words: Vec<String> = lex.vocabulary().into_iter().flat_map(|p| p.split([' ', '-', '\'']).map(fold).collect::<Vec<_>>()).filter(|w| w.chars().count() >= 2).collect();
            words.sort();
            words.dedup();
            for w in words {
                map.entry(w).or_default().push(lang);
            }
        }
        map
    })
}

/// 투표 낱말의 주인 언어들: 같은 낱말 → 없으면 굴절 앞머리(festsitzende ↔ festsitzend, sensori ↔ sensore) →
/// 없으면 합성어 앞뒤(독일어 · 네덜란드어: hoofdstoom·druk, Lager·trilling)
fn vote_owners(w: &str) -> Vec<&'static str> {
    let vocab = vote_vocab();
    if let Some(owners) = vocab.get(w) {
        return owners.clone();
    }
    let wc: Vec<char> = w.chars().collect();
    let mut owners: Vec<&'static str> = Vec::new();
    if wc.len() >= 5 {
        for (v, langs) in vocab {
            let vc: Vec<char> = v.chars().collect();
            let n = vc.len();
            let common = wc.iter().zip(vc.iter()).take_while(|(a, b)| a == b).count();
            if n >= 4 && wc.len() + 2 >= n && wc.len() <= n + 4 && common >= 4.max(n.saturating_sub(2)) {
                for l in langs {
                    if !owners.contains(l) {
                        owners.push(l);
                    }
                }
            }
        }
    }
    if owners.is_empty() && wc.len() >= 7 {
        for (v, langs) in vocab {
            let vc: Vec<char> = v.chars().collect();
            if vc.len() >= 4 && wc.len() >= vc.len() + 3 && (wc.ends_with(&vc) || wc.starts_with(&vc)) {
                for l in langs.iter().filter(|l| matches!(**l, "de" | "nl")) {
                    if !owners.contains(l) {
                        owners.push(l);
                    }
                }
            }
        }
    }
    owners
}

/// 라틴 문자 질의의 사전 투표: (언어, 점수, 맞은 낱말 수) 점수 순
fn latin_vote(text: &str) -> Vec<(&'static str, f32, usize)> {
    let mut score: Vec<(&'static str, f32, usize)> = LATIN.iter().map(|l| (*l, 0.0, 0)).collect();
    let mut seen = std::collections::HashSet::new();
    for raw in text.split(|c: char| !(c.is_alphanumeric() || c == '\'' || c == '-')) {
        let w = fold(raw.trim_matches(|c| c == '\'' || c == '-'));
        if w.chars().count() < 2 || w.chars().any(|c| c.is_ascii_digit()) || !seen.insert(w.clone()) {
            continue;
        }
        // 하이픈 낱말은 조각마다도 (Speisewasser-Bypass)
        let parts: Vec<String> = if w.contains('-') { w.split('-').filter(|p| p.chars().count() >= 2).map(str::to_string).collect() } else { vec![w.clone()] };
        for part in parts {
            let owners = vote_owners(&part);
            if owners.is_empty() {
                continue;
            }
            let share = 1.0 / owners.len() as f32;
            for s in score.iter_mut().filter(|s| owners.contains(&s.0)) {
                s.1 += share;
                s.2 += 1;
            }
        }
    }
    score.sort_by(|a, b| b.1.total_cmp(&a.1));
    score
}

/// 일본어 신자체에만 쓰는 한자: 간체와도 번체와도 모양이 다른 것 (気 ↔ 气 · 氣, 圧 ↔ 压 · 壓, 図 ↔ 图 · 圖, 発 ↔ 发 · 發 …).
/// 가나 없이 한자만 쓴 질의가 일본어인지 중국어인지 가립니다 (主蒸気圧力 이 중국어로 판정되던 것). 번체와 같은 글자(風 · 機 · 熱)는
/// 넣지 않습니다 — 번체 중국어 질의가 일본어로 판정되지 않게. 이 글자가 없으면(誘引通風機) 두 사전 낱말 수로 정합니다.
const KANJI_NOT_SIMPLIFIED: &str = "気圧弁変発図転関検験続価売読様鉄総録駅険済戦単囲伝両歓観証豊軽辺銭帰広拡処歯恵団児浄渋猟獣稲穂穏粋継絵緑縁縄繊聴臓舎薬蔵覧訳謡譲賛酔醸釈鋳錬隠雑霊頼顕駆騒髄髪鶏黒黙齢";

fn whatlang_latin(text: &str) -> Option<&'static str> {
    use whatlang::{Detector, Lang};
    static DETECTOR: OnceLock<Detector> = OnceLock::new();
    const MAP: [(Lang, &str); 8] = [(Lang::Eng, "en"), (Lang::Deu, "de"), (Lang::Fra, "fr"), (Lang::Spa, "es"), (Lang::Ita, "it"), (Lang::Por, "pt"), (Lang::Nld, "nl"), (Lang::Ces, "cs")];
    let d = DETECTOR.get_or_init(|| Detector::with_allowlist(MAP.iter().map(|(l, _)| *l).collect()));
    let info = d.detect(text)?;
    MAP.iter().find(|(l, _)| *l == info.lang()).map(|x| x.1)
}

/// 판정 결과
#[derive(Debug, Clone, PartialEq)]
pub struct Detected {
    pub lang: &'static str,
    /// 근거 (화면 · 기록용): "문자 체계" · "고유 글자" · "사전 투표" · "whatlang" · "기본"
    pub how: &'static str,
    /// 사전 투표가 비겨 함께 볼 언어 (닫힌 낱말을 이 언어들의 사전으로도 맞춥니다)
    pub also: Vec<&'static str>,
}

/// 질의 언어를 정합니다 (12개 언어 밖이면 영어)
pub fn detect(query: &str) -> Detected {
    let text = drop_cjk_names(&strip_quoted(&normalize_query(query)));
    let (mut hangul, mut kana, mut han, mut arabic, mut latin, mut other) = (0usize, 0usize, 0usize, 0usize, 0usize, 0usize);
    // 어절 끝(조사 · 어미 자리)의 문자 체계: 한글이 한자 · 가나 이름 뒤에 붙는 한국어 문장 (株式会社ひなた工房에서)
    let (mut hangul_tail, mut kanji_tail) = (0usize, 0usize);
    for word in text.split_whitespace() {
        match word.chars().rev().filter_map(Script::of).find(|s| s.cjk()) {
            Some(Script::Hangul) => hangul_tail += 1,
            Some(_) => kanji_tail += 1,
            None => {}
        }
    }
    for word in text.split_whitespace() {
        // 숫자가 든 낱말(설비 코드 10LABI0AA102 · 태그)의 로마자는 문장 언어가 아님
        let id_like = word.chars().any(|c| c.is_ascii_digit());
        for c in word.chars() {
            match Script::of(c) {
                Some(Script::Hangul) => hangul += 1,
                Some(Script::Kana) => kana += 1,
                Some(Script::Han) => han += 1,
                Some(Script::Arabic) => arabic += 1,
                Some(Script::Latin) if !id_like => latin += 1,
                Some(Script::Other) => other += 1,
                _ => {}
            }
        }
    }
    let one = |lang: &'static str, how: &'static str| Detected { lang, how, also: Vec::new() };
    if hangul > 0 && (hangul >= kana + han || hangul_tail > kanji_tail) {
        return one("ko", "문자 체계");
    }
    if kana > 0 {
        let simplified = text.chars().filter(|c| SIMPLIFIED_ONLY.contains(*c)).count();
        return if simplified >= 2 { one("zh-hans", "문자 체계") } else { one("ja", "문자 체계") };
    }
    if han > 0 && han >= latin / 3 {
        // 가나 없는 한자: 일본어 신자체 글자 수 ↔ 간체 글자 수, 비기면 두 사전의 낱말(부분 일치) 수, 그래도 비기면 중국어
        let ja_sig = text.chars().filter(|c| KANJI_NOT_SIMPLIFIED.contains(*c)).count();
        let zh_sig = text.chars().filter(|c| SIMPLIFIED_ONLY.contains(*c)).count();
        if ja_sig != zh_sig {
            return one(if ja_sig > zh_sig { "ja" } else { "zh-hans" }, "문자 체계(신자체 · 간체)");
        }
        let f = fold(&text);
        let count = |l: &str| super::ml::lex(l).map(|x| x.vocabulary().into_iter().filter(|w| w.chars().count() >= 2 && !w.is_ascii() && f.contains(w.as_str())).count()).unwrap_or(0);
        let (ja, zh) = (count("ja"), count("zh-hans"));
        return one(if ja > zh { "ja" } else { "zh-hans" }, "문자 체계(사전 낱말)");
    }
    if arabic > 0 && arabic >= latin {
        return one("ar", "문자 체계");
    }
    if hangul > 0 {
        return one("ko", "문자 체계");
    }
    if latin == 0 {
        return one("en", "기본");
    }
    let _ = other;
    let lower = text.to_lowercase();
    let has = |set: &str| lower.chars().any(|c| set.contains(c));
    if has("ěščřžůťďň") {
        return one("cs", "고유 글자");
    }
    if has("ãõ") {
        return one("pt", "고유 글자");
    }
    if has("ñ¿¡") {
        return one("es", "고유 글자");
    }
    if has("ßäöü") {
        return one("de", "고유 글자");
    }
    let vote = latin_vote(&text);
    let (top, second) = (vote[0], vote[1]);
    if top.1 > 0.0 && top.1 >= second.1 + 0.5 {
        return one(top.0, "사전 투표");
    }
    // 비김: 영어가 비긴 언어 안에 있으면 영어 (이 앱의 라틴 문자 기본 언어 — 'temperature' 는 프랑스어 'température' 를 접은 꼴과 같아
    // "GPU junction temperature sensor" 가 en 0.81 · fr 0.61 로 비김), 아니면 whatlang 이 고른 것, 아니면 첫째
    let tied: Vec<&'static str> = vote.iter().filter(|v| v.1 > 0.0 && v.1 + 0.5 > top.1).map(|v| v.0).collect();
    if tied.contains(&"en") {
        return Detected { lang: "en", how: "사전 투표(비김 · 영어 우선)", also: tied.into_iter().filter(|l| *l != "en").collect() };
    }
    if let Some(w) = whatlang_latin(&text) {
        if tied.is_empty() || tied.contains(&w) {
            return Detected { lang: w, how: "whatlang", also: tied.into_iter().filter(|l| *l != w).collect() };
        }
    }
    if let Some(first) = tied.first() {
        return Detected { lang: first, how: "사전 투표(비김)", also: tied[1..].to_vec() };
    }
    one("en", "기본")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scripts_and_quotes_decide_the_sentence_language() {
        assert_eq!(detect("103동 2층 밸브").lang, "ko");
        assert_eq!(detect("103号棟2階のバルブ").lang, "ja");
        assert_eq!(detect("103号楼2层的阀门").lang, "zh-hans");
        assert_eq!(detect("الصمامات في المبنى 103").lang, "ar");
        // 따옴표 안 이름은 문장 언어가 아님
        assert_eq!(detect("「Hauptspeisewasserpumpe A」の下流1ホップ").lang, "ja");
        assert_eq!(detect("1 hop downstream of '주급수펌프 A'").lang, "en");
        assert_eq!(detect("“主給水ポンプA”下游1跳").lang, "zh-hans");
        assert_eq!(quoted_names("1 saut en aval de « Main Feedwater Pump A »"), vec!["Main Feedwater Pump A".to_string()]);
        // 아포스트로피는 따옴표가 아님
        assert!(quoted_ranges("valvole dell'edificio 103 al secondo piano").is_empty());
    }

    #[test]
    fn latin_languages_by_letters_and_vocabulary() {
        assert_eq!(detect("ventily v budově 103").lang, "cs");
        assert_eq!(detect("válvulas do edifício 103 no 2º andar").lang, "pt");
        assert_eq!(detect("bombas del sótano").lang, "es");
        assert_eq!(detect("Pumpen im Keller").lang, "de");
        assert_eq!(detect("vannes du bâtiment 103 au 2e étage").lang, "fr");
        assert_eq!(detect("kleppen in gebouw 103 op de 2e verdieping").lang, "nl");
        assert_eq!(detect("valvole dell'edificio 103 al secondo piano").lang, "it");
        assert_eq!(detect("valves on floor 2 of building 103").lang, "en");
    }

    #[test]
    fn folding_ignores_accents_and_digit_forms() {
        assert_eq!(fold("Présion Ventilöffnung ČERPADLO"), "presion ventiloffnung cerpadlo");
        assert_eq!(fold("Größe"), "grosse");
        assert_eq!(normalize_query("１０３号棟"), "103号棟");
        assert_eq!(fold("أعلى"), "اعلي", "알리프 꼴 → ا, 알리프 막수라 ى → ي");
    }
}
