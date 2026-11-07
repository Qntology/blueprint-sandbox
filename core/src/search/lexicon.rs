//! 검색 낱말 목록 (한국어 · 영어). 숫자 · 비교어 · 상태어 · 유형 지시어처럼 닫힌 낱말은 모델이 아니라 이 목록으로 정합니다
//! (logis-center 실측: "정확한 낱말 목록 먼저, 모델은 후보가 둘 이상일 때만").
//!
//! 영어는 예전에 화면(`app/src/i18n/query.ts`)이 한국어 낱말로 옮겨 보냈습니다. 이제 엔진이 영어 질의를 그대로 읽습니다.
//! 영어 낱말은 Stanza 원형(복수 → 단수, 진행형 → 원형)이나 규칙 단수화로 맞추므로 목록에는 원형만 둡니다.
//! 원장 이름의 영어는 화면 번역 표(`app/src/i18n/en.json` 의 names)를 그대로 씁니다 — 이름 표를 두 곳에 두지 않으려고.

use std::collections::HashMap;
use std::sync::OnceLock;

use super::{DocKind, Op, Quality};

/// 유형 지시어 (영어, 원형으로 같을 때). 한국어는 `TYPE_WORDS` (부분 일치).
/// "change" · "closed" · "event" 는 넣지 않습니다 ("temperature change", "closed valves" 가 MOC · 보관 문서로만 좁혀지면 정답이 지워짐).
pub const TYPE_WORDS_EN: [(DocKind, &[&str]); 6] = [
    (DocKind::Sensor, &["sensor", "instrument", "transmitter", "gauge", "gage", "tag", "meter"]),
    (DocKind::Incident, &["incident", "alarm list", "open case", "in progress", "inc-"]),
    (DocKind::Archive, &["archive", "archived", "history", "resolved", "mcr-", "noc-"]),
    (DocKind::Aop, &["procedure", "aop", "eop", "playbook", "runbook", "checklist"]),
    (DocKind::Moc, &["moc", "mop", "red-line", "redline", "management of change"]),
    (DocKind::Drawing, &["drawing", "p&id", "diagram", "schematic", "pid-", "mep-", "sld-", "net-"]),
];

/// 설비 종류 (영어 원형 → 원장 kind). 여러 낱말은 질의에 그대로 들어 있을 때만.
pub const KIND_WORDS_EN: [(&str, &[&str]); 36] = [
    ("valve", &["valve"]),
    ("pump", &["pump"]),
    ("fan", &["fan"]),
    ("blower", &["fan"]),
    ("turbine", &["turbine"]),
    ("bearing", &["bearing"]),
    ("breaker", &["breaker"]),
    ("transformer", &["transformer"]),
    ("superheater", &["superheater"]),
    ("reheater", &["reheater"]),
    ("heater", &["heater"]),
    ("deaerator", &["deaerator"]),
    ("condenser", &["condenser"]),
    ("cooler", &["cooler"]),
    ("generator", &["generator"]),
    ("burner", &["burner"]),
    ("precipitator", &["esp"]),
    ("esp", &["esp"]),
    ("stack", &["stack"]),
    ("chimney", &["stack"]),
    ("gpu rack", &["gpu_rack"]),
    ("gpu", &["gpu_rack"]),
    ("rack", &["gpu_rack"]),
    ("manifold", &["manifold"]),
    ("heat exchanger", &["exchanger"]),
    ("exchanger", &["exchanger"]),
    ("chiller", &["chiller"]),
    ("cooling tower", &["fan"]),
    ("crah", &["crah"]),
    ("air handler", &["crah"]),
    ("ups", &["ups"]),
    ("battery", &["battery"]),
    ("pdu", &["pdu"]),
    ("switch", &["switch"]),
    ("transceiver", &["transceiver"]),
    ("tank", &["tank"]),
];

/// 계측 종류 (영어 → 엔진의 한국어 계측어). 센서 라벨이 한국어라 조건은 한국어 낱말로 둡니다.
pub const MEASURE_WORDS_EN: [(&str, &str); 14] = [
    ("temperature", "온도"),
    ("temp", "온도"),
    ("pressure", "압력"),
    ("flow", "유량"),
    ("flowrate", "유량"),
    ("vibration", "진동"),
    ("opening", "개도"),
    ("position", "개도"),
    ("current", "전류"),
    ("level", "수위"),
    ("output", "출력"),
    ("power", "전력"),
    ("concentration", "농도"),
    ("amperage", "전류"),
];

/// 질적 상태 (영어 원형). Stanza 원형이 없을 때를 위해 활용형도 둡니다.
pub const QUALITY_WORDS_EN: [(&str, Quality); 30] = [
    ("high", Quality::High),
    ("higher", Quality::High),
    ("excessive", Quality::High),
    ("overheat", Quality::High),
    ("overheating", Quality::High),
    ("hot", Quality::High),
    ("low", Quality::Low),
    ("lower", Quality::Low),
    ("insufficient", Quality::Low),
    ("rise", Quality::Rising),
    ("rising", Quality::Rising),
    ("increase", Quality::Rising),
    ("increasing", Quality::Rising),
    ("climb", Quality::Rising),
    ("climbing", Quality::Rising),
    ("fall", Quality::Falling),
    ("falling", Quality::Falling),
    ("drop", Quality::Falling),
    ("dropping", Quality::Falling),
    ("decrease", Quality::Falling),
    ("decreasing", Quality::Falling),
    ("decline", Quality::Falling),
    ("stuck", Quality::Stuck),
    ("frozen", Quality::Frozen),
    ("flatline", Quality::Frozen),
    ("abnormal", Quality::Abnormal),
    ("anomalous", Quality::Abnormal),
    ("faulty", Quality::Abnormal),
    ("problem", Quality::Abnormal),
    ("unusual", Quality::Abnormal),
];

/// 비교어 (영어). 숫자 앞에 오는 것과 뒤에 오는 것을 나눕니다 ("above 600", "600 or more").
pub const COMPARATORS_EN_BEFORE: [(&str, Op); 16] = [
    ("at least", Op::Gte),
    ("no less than", Op::Gte),
    (">=", Op::Gte),
    ("more than", Op::Gt),
    ("greater than", Op::Gt),
    ("higher than", Op::Gt),
    ("above", Op::Gt),
    ("over", Op::Gt),
    ("exceeding", Op::Gt),
    (">", Op::Gt),
    ("at most", Op::Lte),
    ("no more than", Op::Lte),
    ("within", Op::Lte),
    ("<=", Op::Lte),
    ("less than", Op::Lt),
    ("below", Op::Lt),
];
pub const COMPARATORS_EN_BEFORE_MORE: [(&str, Op); 3] = [("under", Op::Lt), ("lower than", Op::Lt), ("<", Op::Lt)];
pub const COMPARATORS_EN_AFTER: [(&str, Op); 6] = [("or more", Op::Gte), ("and above", Op::Gte), ("or higher", Op::Gte), ("or less", Op::Lte), ("and below", Op::Lte), ("or lower", Op::Lte)];

/// 부정어 (영어는 앞에 옵니다: "not stuck", "valves that are not stuck")
pub const NEGATIONS_EN: [&str; 7] = ["not", "no", "non", "except", "excluding", "without", "never"];

/// 뜻 없는 영어 낱말 (품사 분석이 없을 때 기능어로 봄)
pub const FILLER_EN: [&str; 44] = [
    "with", "the", "a", "an", "in", "on", "of", "at", "show", "find", "list", "all", "that", "is", "are", "was", "were", "which", "whose", "for", "to", "me", "and",
    "where", "has", "have", "its", "by", "from", "this", "these", "those", "please", "any", "some", "what", "there", "be", "been", "it", "give", "get", "display", "search",
];

/// 명령 동사 (품사 분석이 VERB 로 봐도 검색 낱말이 아님)
pub const COMMAND_VERBS_EN: [&str; 9] = ["show", "find", "list", "give", "get", "display", "search", "open", "see"];
pub const COMMAND_VERBS_KO: [&str; 9] = ["찾", "보이", "보여", "알리", "알려", "열", "띄우", "검색하", "주"];

/// 단위 (영어 질의에 따로 쓰는 것)
pub fn unit_measure_en(unit: &str) -> Option<&'static str> {
    match unit.to_lowercase().trim_end_matches('.') {
        "degree" | "degrees" | "degc" | "°c" | "℃" | "c" | "deg" => Some("온도"),
        _ => None,
    }
}

/// en.json 의 원장 이름 · 낱말 표 (한국어 → 영어)
pub struct Names {
    pub names: HashMap<String, String>,
    pub phrases: HashMap<String, String>,
}

static NAMES: OnceLock<Names> = OnceLock::new();

pub fn names() -> &'static Names {
    NAMES.get_or_init(|| {
        let v: serde_json::Value = serde_json::from_str(include_str!("../../../app/src/i18n/en.json")).unwrap_or_default();
        let table = |k: &str| -> HashMap<String, String> {
            v.get(k).and_then(|t| t.as_object()).map(|o| o.iter().filter_map(|(a, b)| b.as_str().map(|b| (a.clone(), b.to_string()))).collect()).unwrap_or_default()
        };
        Names { names: table("names"), phrases: table("phrases") }
    })
}

impl Names {
    /// 원장 이름 · 낱말의 영어. 없으면 None
    pub fn en(&self, ko: &str) -> Option<&str> {
        self.names.get(ko).or_else(|| self.phrases.get(ko)).map(String::as_str)
    }
}

/// 규칙 단수화 (품사 분석이 없을 때): batteries → battery, switches → switch, valves → valve
pub fn singular_en(w: &str) -> String {
    let n = w.chars().count();
    if n > 4 && w.ends_with("ies") {
        format!("{}y", &w[..w.len() - 3])
    } else if n > 4 && (w.ends_with("ches") || w.ends_with("shes") || w.ends_with("xes") || w.ends_with("sses")) {
        w[..w.len() - 2].to_string()
    } else if n > 3 && w.ends_with('s') && !w.ends_with("ss") && !w.ends_with("us") {
        w[..w.len() - 1].to_string()
    } else {
        w.to_string()
    }
}

/// 한국어 조사 (긴 것부터). 품사 분석이 없을 때 낱말 끝에서 하나만 떼어 본 꼴을 더합니다 (원문도 함께 씀).
pub const PARTICLES_KO: [&str; 26] = [
    "에서는", "으로는", "에서", "으로", "에게", "까지", "부터", "보다", "처럼", "이랑", "하고", "에는", "와", "과", "을", "를", "이", "가", "은", "는", "의", "에", "로", "도", "만", "랑",
];

pub fn strip_particle_ko(w: &str) -> Option<String> {
    for p in PARTICLES_KO {
        if let Some(stem) = w.strip_suffix(p) {
            if stem.chars().count() >= 2 && stem.chars().all(|c| ('가'..='힣').contains(&c) || c.is_ascii_alphanumeric()) {
                return Some(stem.to_string());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn singular_and_particle_rules() {
        assert_eq!(singular_en("valves"), "valve");
        assert_eq!(singular_en("batteries"), "battery");
        assert_eq!(singular_en("switches"), "switch");
        assert_eq!(singular_en("pressure"), "pressure");
        assert_eq!(singular_en("bus"), "bus");
        assert_eq!(strip_particle_ko("급수펌프의").as_deref(), Some("급수펌프"));
        assert_eq!(strip_particle_ko("밸브를").as_deref(), Some("밸브"));
        assert_eq!(strip_particle_ko("밸브"), None, "두 글자 밑으로는 떼지 않습니다");
    }

    #[test]
    fn ledger_names_have_english() {
        let n = names();
        assert_eq!(n.en("주급수펌프 A"), Some("Main Feedwater Pump A"));
        assert_eq!(n.en("보일러동"), Some("Boiler Building"));
    }
}
