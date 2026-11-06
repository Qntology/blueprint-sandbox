//! 발전소 · AI 데이터센터 공간 · 설비 식별 체계 (기획 Part 8 2절, 추가 개선안 Part 1).
//!
//! 설비는 아파트 단지처럼 5단계 공간 뎁스로 찾습니다: Lv1 호기 · 센터(단지) → Lv2 섹터 · 건물(동) → Lv3 층 · 고도(층)
//! → Lv4 계통 도면(라인) → Lv5 개별 설비 · 센서(호수). Lv5 는 발전소에서 KKS(발전소 설비 식별 표준) 코드,
//! AI 데이터센터에서 DCIM 자산 코드입니다 ([`CodeScheme`]).
//!
//! KKS 코드는 이 저장소에서 12글자로 씁니다: `10` 호기 · `LAB` 계통 · `10` 계통 번호 · `AP` 설비 종류 · `001` 설비 번호.
//! 센서 태그도 같은 꼴이고 설비 종류 자리가 `C` 로 시작하는 측정 코드입니다 (`10LAB10CP001` = 주급수펌프 A 의 압력).
//! DCIM 자산 코드도 12글자입니다: `DC` 사이트 · `1` 센터 · `CDU` 계통 · `20` 계통 번호 · `AA` 설비 종류 · `02` 설비 번호
//! (`DC1CDU20CG02` = CDU 2차측 유량조절밸브의 개도).

use chrono::{DateTime, FixedOffset, Utc};
use serde::{Deserialize, Serialize};

/// 발전소 현지 시각(KST, UTC+9). 사건 · 보관 번호의 날짜와 보존 기한은 이 시각으로 셉니다 (UTC 로 세면 자정~09시 사건이 전날 번호가 됩니다).
pub fn kst() -> FixedOffset {
    FixedOffset::east_opt(9 * 3600).expect("UTC+9 는 올바른 시간대입니다")
}

pub fn local(at: DateTime<Utc>) -> DateTime<FixedOffset> {
    at.with_timezone(&kst())
}

/// Lv2 섹터. Gate 0 라우터가 알람을 이 다섯 건물 중 하나로 보냅니다.
///
/// 다섯 자리는 도메인과 무관한 라우팅 자리입니다. AI 데이터센터 원장의 다섯 구역도 같은 자리로 갑니다
/// (`compute` → 1, `network` → 2, `cooling` → 3, `power` → 4, `facility` → 5). 화면에 쓰는 이름은 원장의 섹터 행이 정합니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize)]
pub enum Sector {
    Boiler,
    Turbine,
    Feedwater,
    Electric,
    EnvSafety,
}

impl Sector {
    pub const ALL: [Sector; 5] = [Sector::Boiler, Sector::Turbine, Sector::Feedwater, Sector::Electric, Sector::EnvSafety];

    /// 발전소 시드 대장의 섹터 키 (다른 도메인 원장의 키는 [`crate::lattice::Lattice::sector_for`] 로 찾습니다)
    pub fn key(self) -> &'static str {
        match self {
            Sector::Boiler => "boiler",
            Sector::Turbine => "turbine",
            Sector::Feedwater => "feedwater",
            Sector::Electric => "electric",
            Sector::EnvSafety => "env",
        }
    }

    /// 원장의 섹터 키 → 라우팅 자리. 발전소 키와 AI 데이터센터 키를 함께 받습니다.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "boiler" | "compute" => Some(Sector::Boiler),
            "turbine" | "network" => Some(Sector::Turbine),
            "feedwater" | "cooling" => Some(Sector::Feedwater),
            "electric" | "power" => Some(Sector::Electric),
            "env" | "facility" => Some(Sector::EnvSafety),
            _ => None,
        }
    }

    /// 라우팅 자리 번호 (1 ~ 5)
    pub fn slot(self) -> u8 {
        match self {
            Sector::Boiler => 1,
            Sector::Turbine => 2,
            Sector::Feedwater => 3,
            Sector::Electric => 4,
            Sector::EnvSafety => 5,
        }
    }

    /// 기획안의 분류 이름 (`Category`)
    pub fn category(self) -> &'static str {
        match self {
            Sector::Boiler => "Sector01Boiler",
            Sector::Turbine => "Sector02Turbine",
            Sector::Feedwater => "Sector03Feedwater",
            Sector::Electric => "Sector04Electric",
            Sector::EnvSafety => "Sector05EnvSafety",
        }
    }
}

/// 측정 종류. 센서 태그의 설비 종류 자리 두 글자(`CP`, `CT` …)입니다 (KKS 는 여덟째 · 아홉째, DCIM 은 아홉째 · 열째 글자).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Measure {
    /// CP 압력
    Pressure,
    /// CT 온도
    Temperature,
    /// CY 진동
    Vibration,
    /// CF 유량
    Flow,
    /// CG 개도율
    Opening,
    /// CE 전기량 (전류 · 출력)
    Electric,
    /// CL 수위
    Level,
    /// CQ 분석 (가스 농도)
    Analysis,
    Other,
}

impl Measure {
    pub fn from_code(code: &str) -> Self {
        match code {
            "CP" => Measure::Pressure,
            "CT" => Measure::Temperature,
            "CY" => Measure::Vibration,
            "CF" => Measure::Flow,
            "CG" => Measure::Opening,
            "CE" => Measure::Electric,
            "CL" => Measure::Level,
            "CQ" => Measure::Analysis,
            _ => Measure::Other,
        }
    }

    /// 센서 태그에서 측정 종류를 읽습니다. 코드 체계는 태그 모양으로 가립니다 (첫 글자가 숫자면 KKS, 영문이면 DCIM).
    pub fn of_tag(tag: &str) -> Self {
        let at = CodeScheme::of_code(tag).unwrap_or_default().kind_at();
        tag.get(at..at + 2).map(Self::from_code).unwrap_or(Measure::Other)
    }

    pub fn label(self) -> &'static str {
        match self {
            Measure::Pressure => "압력",
            Measure::Temperature => "온도",
            Measure::Vibration => "진동",
            Measure::Flow => "유량",
            Measure::Opening => "개도율",
            Measure::Electric => "전기량",
            Measure::Level => "수위",
            Measure::Analysis => "농도",
            Measure::Other => "계측",
        }
    }
}

/// 설비 사이를 잇는 물리 매체 (마인드맵 간선의 종류). 뒤의 다섯은 AI 데이터센터의 냉각 · 네트워크 매체입니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Medium {
    Power,
    Condensate,
    Feedwater,
    Steam,
    Shaft,
    Oil,
    Seawater,
    Fuel,
    Air,
    Gas,
    Draft,
    /// CDU 2차측 냉각수 (TCS 루프, 랙 매니폴드 · 콜드플레이트로 가는 물)
    Coolant,
    /// 칠러 → CDU 열교환기 1차측 냉수 (FWS)
    ChilledWater,
    /// 냉각탑 ↔ 칠러 응축기 냉각수
    CondenserWater,
    /// 항온항습기(CRAH) 냉기
    Airflow,
    /// 광 패브릭 · 스토리지 네트워크
    Network,
}

impl Medium {
    pub const ALL: [Medium; 16] = [
        Medium::Power,
        Medium::Condensate,
        Medium::Feedwater,
        Medium::Steam,
        Medium::Shaft,
        Medium::Oil,
        Medium::Seawater,
        Medium::Fuel,
        Medium::Air,
        Medium::Gas,
        Medium::Draft,
        Medium::Coolant,
        Medium::ChilledWater,
        Medium::CondenserWater,
        Medium::Airflow,
        Medium::Network,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Medium::Power => "power",
            Medium::Condensate => "condensate",
            Medium::Feedwater => "feedwater",
            Medium::Steam => "steam",
            Medium::Shaft => "shaft",
            Medium::Oil => "oil",
            Medium::Seawater => "seawater",
            Medium::Fuel => "fuel",
            Medium::Air => "air",
            Medium::Gas => "gas",
            Medium::Draft => "draft",
            Medium::Coolant => "coolant",
            Medium::ChilledWater => "chilled_water",
            Medium::CondenserWater => "condenser_water",
            Medium::Airflow => "airflow",
            Medium::Network => "network",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Medium::Power => "전력",
            Medium::Condensate => "복수",
            Medium::Feedwater => "급수",
            Medium::Steam => "증기",
            Medium::Shaft => "회전축",
            Medium::Oil => "윤활 · 제어유",
            Medium::Seawater => "냉각 해수",
            Medium::Fuel => "연료",
            Medium::Air => "연소 공기",
            Medium::Gas => "연소가스",
            Medium::Draft => "통풍",
            Medium::Coolant => "2차 냉각수",
            Medium::ChilledWater => "1차 냉수",
            Medium::CondenserWater => "냉각탑 냉각수",
            Medium::Airflow => "공조 냉기",
            Medium::Network => "광 네트워크",
        }
    }
}

/// 값이 움직인 방향
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Direction {
    Up,
    Down,
}

impl Direction {
    pub fn arrow(self) -> &'static str {
        match self {
            Direction::Up => "상승",
            Direction::Down => "하락",
        }
    }
}

/// 상류 변화와 하류 변화의 방향이 물리적으로 맞는지 봅니다 (SDS 축 2: 열역학 · 유체 보존 방향성).
/// 표에 없는 조합은 모른다(None)고 답합니다. 모르는 것은 원인 판정을 막지도, 돕지도 않습니다.
pub fn physics_consistent(medium: Medium, up: (Measure, Direction), down: (Measure, Direction)) -> Option<bool> {
    use Measure::*;
    let ((um, ud), (dm, dd)) = (up, down);
    match medium {
        // 급수가 줄면 (개도 · 유량 하락) 증발관 냉각이 줄어 하류 온도가 오르고 압력은 내려갑니다.
        Medium::Feedwater | Medium::Condensate if matches!(um, Opening | Flow) => match dm {
            Temperature => Some(ud != dd),
            Flow | Level => Some(ud == dd),
            Pressure => Some(ud == dd),
            _ => None,
        },
        // 유인통풍기 부하(전류)가 떨어지면 흡인이 약해져 노내 압력이 오릅니다.
        Medium::Draft if um == Electric || um == Flow => match dm {
            Pressure => Some(ud != dd),
            _ => None,
        },
        // 윤활유 온도가 오르면 유막이 얇아져 진동이 오릅니다.
        Medium::Oil if um == Temperature => match dm {
            Vibration | Temperature => Some(ud == dd),
            _ => None,
        },
        // 전력 공급이 줄면 하류 설비의 전류 · 유량 · 압력이 함께 내려갑니다.
        Medium::Power if um == Electric => match dm {
            Electric | Flow | Pressure => Some(ud == dd),
            _ => None,
        },
        Medium::Steam if um == Temperature || um == Pressure => match dm {
            Temperature | Pressure | Electric => Some(ud == dd),
            _ => None,
        },
        Medium::Shaft => match dm {
            Vibration if um == Vibration => Some(ud == dd),
            _ => None,
        },
        // 수냉 루프: 냉각수가 줄면 (개도 · 유량 하락) 콜드플레이트 · 열교환기 하류 온도가 오르고 유량 · 압력은 내려갑니다 (급수와 같은 열수력).
        Medium::Coolant | Medium::ChilledWater if matches!(um, Opening | Flow) => match dm {
            Temperature => Some(ud != dd),
            Flow | Level | Pressure => Some(ud == dd),
            _ => None,
        },
        // 공급 냉수 · 냉각수 온도가 오르면 하류 온도도 오릅니다.
        Medium::Coolant | Medium::ChilledWater | Medium::CondenserWater | Medium::Airflow if um == Temperature => match dm {
            Temperature => Some(ud == dd),
            _ => None,
        },
        // 냉각탑 팬 · 냉각수 펌프 부하(전류)가 떨어지면 열을 덜 버려 칠러 냉수 공급 온도가 오릅니다.
        Medium::CondenserWater | Medium::Airflow if um == Electric || um == Flow => match dm {
            Temperature => Some(ud != dd),
            _ => None,
        },
        _ => None,
    }
}

/// 설비 코드 체계. 둘 다 12글자이고 자리마다 숫자 · 영문이 정해져 있어 OCR 오독(1↔I, 0↔O)을 같은 방식으로 고칩니다.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CodeScheme {
    /// 발전소 KKS `10LAB10AP001`: 숫자 2 · 영문 3 · 숫자 2 · 영문 2 · 숫자 3
    #[default]
    Kks,
    /// AI 데이터센터 DCIM 자산 코드 `DC1CDU20AA02`: 영문 2 · 숫자 1 · 영문 3 · 숫자 2 · 영문 2 · 숫자 2
    Dcim,
}

impl CodeScheme {
    /// 자리마다 `9` 는 숫자, `A` 는 영문 대문자
    pub fn mask(self) -> &'static [u8; 12] {
        match self {
            CodeScheme::Kks => b"99AAA99AA999",
            CodeScheme::Dcim => b"AA9AAA99AA99",
        }
    }

    /// 설비 종류 · 측정 코드 두 글자가 시작하는 자리. 센서 태그는 이 자리가 `C` 로 시작합니다.
    /// 앞부분(계통 머리)은 센서 태그와 그 설비의 코드가 같습니다.
    pub fn kind_at(self) -> usize {
        match self {
            CodeScheme::Kks => 7,
            CodeScheme::Dcim => 8,
        }
    }

    /// 이 체계의 12글자 꼴인지
    pub fn shape(self, code: &str) -> bool {
        let b = code.as_bytes();
        b.len() == 12 && b.iter().zip(self.mask()).all(|(c, m)| if *m == b'9' { c.is_ascii_digit() } else { c.is_ascii_uppercase() })
    }

    /// 코드 모양으로 체계를 가립니다 (어느 꼴도 아니면 None)
    pub fn of_code(code: &str) -> Option<Self> {
        [CodeScheme::Kks, CodeScheme::Dcim].into_iter().find(|s| s.shape(code))
    }

    /// 화면 · 문서에 쓰는 이름
    pub fn label(self) -> &'static str {
        match self {
            CodeScheme::Kks => "KKS",
            CodeScheme::Dcim => "DCIM 자산코드",
        }
    }
}

/// KKS 12글자 꼴인지: 숫자 2 · 영문 3 · 숫자 2 · 영문 2 · 숫자 3
pub fn kks_shape(code: &str) -> bool {
    CodeScheme::Kks.shape(code)
}

/// 숫자 자리에 잘못 읽힌 영문 → 숫자 (OCR 이 자주 헷갈리는 글자)
fn as_digit(c: char) -> Option<char> {
    match c {
        '0'..='9' => Some(c),
        'O' | 'o' | 'D' | 'Q' => Some('0'),
        'I' | 'l' | 'i' | '|' | 'L' | 'J' | '!' => Some('1'),
        'Z' | 'z' => Some('2'),
        'S' | 's' => Some('5'),
        'G' | 'b' => Some('6'),
        'T' => Some('7'),
        'B' => Some('8'),
        'g' | 'q' => Some('9'),
        _ => None,
    }
}

/// 영문 자리에 잘못 읽힌 숫자 → 영문
fn as_letter(c: char) -> Option<char> {
    match c {
        'A'..='Z' => Some(c),
        'a'..='z' => Some(c.to_ascii_uppercase()),
        '0' => Some('O'),
        '1' => Some('I'),
        '2' => Some('Z'),
        '5' => Some('S'),
        '6' => Some('G'),
        '8' => Some('B'),
        _ => None,
    }
}

/// OCR 글자에서 KKS 후보를 만듭니다. 띄어쓰기 · 하이픈 · 점을 빼고, 자리마다 숫자 · 영문 혼동을 고칩니다.
/// 12글자가 아니면 대문자로만 바꿔 돌려줍니다 (편집 거리로 맞춥니다).
pub fn repair_kks(raw: &str) -> String {
    repair_code(raw, CodeScheme::Kks)
}

/// [`repair_kks`] 를 코드 체계의 자리 꼴(`mask`)로 합니다. DCIM 은 `DC1CDUI0AP01` → `DC1CDU10AP01`.
pub fn repair_code(raw: &str, scheme: CodeScheme) -> String {
    let squeezed: Vec<char> = raw.chars().filter(|c| !c.is_whitespace() && !matches!(c, '-' | '.' | '_' | ':' | '·')).collect();
    if squeezed.len() != 12 {
        return squeezed.iter().map(|c| c.to_ascii_uppercase()).collect();
    }
    squeezed
        .iter()
        .zip(scheme.mask())
        .map(|(&c, m)| {
            let fixed = if *m == b'9' { as_digit(c) } else { as_letter(c) };
            fixed.unwrap_or(c.to_ascii_uppercase())
        })
        .collect()
}

/// 센서 태그의 측정 코드부터 끝까지 (`10LAB20CG102` → `CG102`, `DC1CDU20CG02` → `CG02`). 도면 상자에 적힌 짧은 이름입니다.
pub fn short_tag(tag: &str) -> &str {
    let at = CodeScheme::of_code(tag).unwrap_or_default().kind_at();
    tag.get(at..).unwrap_or(tag)
}

/// 값을 화면 · 사실 줄에 쓰는 꼴. 1000 이상은 천 단위 쉼표, 1 미만은 소수 둘째 자리까지 씁니다.
pub fn fmt_value(v: f64) -> String {
    if v.abs() >= 1000.0 {
        let whole = format!("{:.1}", v.abs());
        let (int, frac) = whole.split_once('.').unwrap_or((&whole, "0"));
        let mut grouped = String::new();
        for (i, ch) in int.chars().enumerate() {
            if i > 0 && (int.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(ch);
        }
        format!("{}{grouped}.{frac}", if v < 0.0 { "-" } else { "" })
    } else if v.abs() >= 1.0 || v == 0.0 {
        format!("{v:.1}")
    } else {
        format!("{v:.2}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn plant_dates_are_counted_in_kst() {
        // UTC 11-03 17:30 은 발전소 시각 11-04 02:30 입니다. 사건 번호는 11-04 로 세야 합니다.
        let at = Utc.with_ymd_and_hms(2026, 11, 3, 17, 30, 0).unwrap();
        assert_eq!(local(at).format("%Y%m%d %H:%M").to_string(), "20261104 02:30");
        let mut engine = crate::state_machine::pipeline::Engine::new(b"salt", crate::policy::AutomationPolicy::default(), None);
        assert_eq!(engine.next_id("INC", at), "INC-20261104-0001");
    }

    #[test]
    fn sectors_round_trip_through_their_keys() {
        for s in Sector::ALL {
            assert_eq!(Sector::from_key(s.key()), Some(s));
        }
        assert_eq!(Sector::Feedwater.category(), "Sector03Feedwater");
        assert_eq!(Sector::from_key("nope"), None);
        // AI 데이터센터 구역은 같은 다섯 라우팅 자리로 갑니다.
        let aidc = ["compute", "network", "cooling", "power", "facility"].map(|k| Sector::from_key(k).unwrap().slot());
        assert_eq!(aidc, [1, 2, 3, 4, 5]);
    }

    #[test]
    fn the_measure_comes_from_the_tag() {
        assert_eq!(Measure::of_tag("10LAB20CG102"), Measure::Opening);
        assert_eq!(Measure::of_tag("10HAH10CT001"), Measure::Temperature);
        assert_eq!(Measure::of_tag("10MAD10CY001"), Measure::Vibration);
        assert_eq!(Measure::of_tag("짧음"), Measure::Other);
        // DCIM 태그는 측정 코드가 한 자리 뒤에 있습니다.
        assert_eq!(Measure::of_tag("DC1CDU20CG02"), Measure::Opening);
        assert_eq!(Measure::of_tag("DC1GPU10CT01"), Measure::Temperature);
        assert_eq!(Measure::of_tag("DC1CTW10CE01"), Measure::Electric);
        assert_eq!(short_tag("10LAB20CG102"), "CG102");
        assert_eq!(short_tag("DC1CDU20CG02"), "CG02");
    }

    #[test]
    fn kks_shape_is_checked_by_position() {
        assert!(kks_shape("10LAB10AP001"));
        assert!(kks_shape("10LAB20CG102"));
        assert!(!kks_shape("10LABI0AP001"));
        assert!(!kks_shape("10LAB10AP01"));
        assert!(!kks_shape("10lab10ap001"));
        assert!(CodeScheme::Dcim.shape("DC1CDU20AA02"));
        assert!(!CodeScheme::Dcim.shape("DC1CDUI0AP01"));
        assert!(!kks_shape("DC1CDU20AA02"));
        assert_eq!(CodeScheme::of_code("DC1GPU10RK01"), Some(CodeScheme::Dcim));
        assert_eq!(CodeScheme::of_code("10LAB10AP001"), Some(CodeScheme::Kks));
        assert_eq!(CodeScheme::of_code("10LABI0AP001"), None);
    }

    #[test]
    fn dcim_codes_are_repaired_by_their_own_mask() {
        // 추가 개선안 3.3 의 현장 단말 예: `I` 를 `1` 로
        assert_eq!(repair_code("DC1CDUI0AP01", CodeScheme::Dcim), "DC1CDU10AP01");
        assert_eq!(repair_code("DCI CDU 20 AA O2", CodeScheme::Dcim), "DC1CDU20AA02");
        assert_eq!(repair_code("0C1GPU10RK01", CodeScheme::Dcim), "OC1GPU10RK01", "영문 자리의 0 은 O 로 (사이트 코드는 편집 거리로 맞춤)");
        // 같은 글자를 KKS 꼴로 고치면 엉뚱해집니다: 체계를 원장이 알려 줘야 하는 까닭
        assert_ne!(repair_code("DC1CDUI0AP01", CodeScheme::Kks), "DC1CDU10AP01");
    }

    #[test]
    fn ocr_confusions_are_repaired_only_where_they_cannot_belong() {
        // 기획안의 예: `1` 을 `I` 로 읽음
        assert_eq!(repair_kks("10LABI0AP001"), "10LAB10AP001");
        assert_eq!(repair_kks("1O LAB 10 AP 0O1"), "10LAB10AP001");
        assert_eq!(repair_kks("10MAAl0ST001"), "10MAA10ST001");
        // 영문 자리의 숫자도 고칩니다 (`HAH` 의 H 는 그대로, `0` → `O`)
        assert_eq!(repair_kks("10LAB10AP0O1"), "10LAB10AP001");
        assert_eq!(repair_kks("1OL4B10AP001"), "10L4B10AP001", "4 는 영문으로 바꿀 수 없어 그대로 둡니다");
        assert_eq!(repair_kks("10LAB10AP001X"), "10LAB10AP001X", "12글자가 아니면 자리를 고치지 않습니다");
    }

    #[test]
    fn physics_directions_follow_the_medium() {
        let down = (Measure::Opening, Direction::Down);
        assert_eq!(physics_consistent(Medium::Feedwater, down, (Measure::Temperature, Direction::Up)), Some(true));
        assert_eq!(physics_consistent(Medium::Feedwater, down, (Measure::Temperature, Direction::Down)), Some(false));
        assert_eq!(physics_consistent(Medium::Draft, (Measure::Electric, Direction::Down), (Measure::Pressure, Direction::Up)), Some(true));
        assert_eq!(physics_consistent(Medium::Oil, (Measure::Temperature, Direction::Up), (Measure::Vibration, Direction::Up)), Some(true));
        assert_eq!(physics_consistent(Medium::Fuel, (Measure::Flow, Direction::Down), (Measure::Temperature, Direction::Up)), None);
        // AI 데이터센터: CDU 밸브가 닫히면 GPU 온도가 오르고, 냉각탑 팬 전류가 떨어지면 냉수 공급 온도가 오릅니다.
        assert_eq!(physics_consistent(Medium::Coolant, down, (Measure::Temperature, Direction::Up)), Some(true));
        assert_eq!(physics_consistent(Medium::Coolant, down, (Measure::Pressure, Direction::Up)), Some(false));
        assert_eq!(physics_consistent(Medium::CondenserWater, (Measure::Electric, Direction::Down), (Measure::Temperature, Direction::Up)), Some(true));
        assert_eq!(physics_consistent(Medium::Network, (Measure::Temperature, Direction::Up), (Measure::Temperature, Direction::Up)), None);
    }

    #[test]
    fn values_are_written_like_the_drawings() {
        assert_eq!(fmt_value(1850.0), "1,850.0");
        assert_eq!(fmt_value(1120.4), "1,120.4");
        assert_eq!(fmt_value(18.0), "18.0");
        assert_eq!(fmt_value(-0.1), "-0.10");
        assert_eq!(fmt_value(0.0), "0.0");
        assert_eq!(fmt_value(-1234.5), "-1,234.5");
    }
}
