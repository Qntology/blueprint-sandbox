// 영어 자연어 질의 → 엔진(search.rs)이 읽는 한국어 낱말. 엔진의 파서(유형 · 동 · 층 · 종류 · 계측 · 질적 상태 · 비교어 · 연결)는
// 한국어 낱말을 보므로, 영어 화면에서는 질의를 그 낱말로 옮겨 보냅니다. 설비 이름은 번역 표의 원장 이름을 거꾸로 씁니다.
// 결과(제목 · "왜" · 조건)는 다시 invoke 감싸기가 영어로 바꿉니다.
import data from "./en.json";

const TABLE = data as unknown as { names?: Record<string, string>; phrases?: Record<string, string> };

/** 영어 → 한국어 낱말 규칙 (같은 영어가 둘이면 먼저 나온 것, 긴 것부터) */
function reverseRules(entries: [string, string][], skip: Set<string>): [RegExp, string][] {
  const reverse = new Map<string, string>();
  for (const [ko, en] of entries) {
    const key = en.toLowerCase();
    if (key.length >= 4 && !/[가-힣]/.test(key) && !reverse.has(key) && !skip.has(key)) reverse.set(key, ko);
  }
  return [...reverse.entries()]
    .sort((a, b) => b[0].length - a[0].length)
    .map(([en, ko]) => [new RegExp(`(?<![a-z0-9])${en.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}(?![a-z0-9])`, "gi"), ` ${ko} `]);
}
/** 원장 이름과 여러 낱말 용어("ID fan" · "feedwater pump") — 통째로 먼저 바뀌어야 아래 낱말 규칙에 쪼개지지 않습니다 */
const phrases = Object.entries(TABLE.phrases ?? {});
const nameRules = reverseRules([...Object.entries(TABLE.names ?? {}), ...phrases.filter(([, en]) => en.includes(" "))], new Set());
/** 한 낱말 용어 — 비교어 · 동 · 층 · 상태 규칙(RULES) 뒤에 씁니다. "floor 2" 는 RULES 가 "2층" 으로, "pending" 은 "승인 대기" 로 옮겨야 합니다 */
const phraseRules = reverseRules(
  phrases.filter(([, en]) => !en.includes(" ")),
  new Set(["floor", "pending", "state", "type", "space", "connected", "around", "upstream", "downstream", "high", "low", "approved", "stuck", "fault", "value", "check"]),
);

const UNIT: Record<string, string> = { "°c": "도", c: "도", degc: "도", degrees: "도", degree: "도", mpa: "MPa", kpa: "kPa", bar: "bar", "t/h": "t/h", "%": "%", lpm: "LPM", a: "A", kw: "kW", mw: "MW", ppm: "ppm", "μm": "μm", um: "μm" };
const UNIT_RE = "(°c|degc|degrees?|mpa|kpa|bar|t/h|%|lpm|kw|mw|ppm|μm|um|a|c)?";

/** 낱말 규칙 (순서대로). 앞쪽이 더 긴 표현입니다. */
const RULES: [RegExp, string | ((...m: string[]) => string)][] = [
  // 비교어 + 숫자 (+ 단위): "above 600 °C" → "600도 이상"
  [new RegExp(`(?:at least|no less than|>=)\\s*([\\d,.]+)\\s*${UNIT_RE}`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 이상 `],
  [new RegExp(`(?:above|over|more than|greater than|higher than|exceeding|>)\\s*([\\d,.]+)\\s*${UNIT_RE}`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 초과 `],
  [new RegExp(`(?:at most|no more than|<=|within)\\s*([\\d,.]+)\\s*${UNIT_RE}`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 이하 `],
  [new RegExp(`(?:below|under|less than|lower than|<)\\s*([\\d,.]+)\\s*${UNIT_RE}`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 미만 `],
  [new RegExp(`([\\d,.]+)\\s*${UNIT_RE}\\s*(?:or more|and above|or higher|\\+)`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 이상 `],
  [new RegExp(`([\\d,.]+)\\s*${UNIT_RE}\\s*(?:or less|and below|or lower)`, "gi"), (_, n, u) => ` ${n}${UNIT[(u ?? "").toLowerCase()] ?? ""} 이하 `],
  // 동 · 층 · 홉
  [/\b(?:bldg\.?|building)\s*(\d+)/gi, (_, n) => ` ${n}동 `],
  [/\b(\d+)\s*(?:bldg\.?|building)\b/gi, (_, n) => ` ${n}동 `],
  [/\b(\d+)(?:st|nd|rd|th)?\s*(?:floor|fl\.?)\b/gi, (_, n) => ` ${n}층 `],
  [/\b(?:floor|fl\.?)\s*(\d+)\b/gi, (_, n) => ` ${n}층 `],
  [/\b(\d)f\b/gi, (_, n) => ` ${n}층 `],
  [/\b(?:basement|underground|b1f?)\b/gi, " 지하 "],
  [/\b(?:outdoor|outside)\b/gi, " 옥외 "],
  [/\b(\d+)\s*-?\s*hops?\b/gi, (_, n) => ` ${n}홉 `],
  [/\bupstream\b/gi, " 상류 "],
  [/\bdownstream\b/gi, " 하류 "],
  [/\b(?:around|nearby|near|neighbou?rs? of|connected(?: to)?)\b/gi, " 주변 "],
  [/\brelated(?: to)?\b/gi, " 관련 "],
  // 부정: "not stuck" → "고착 아닌" 은 아래 낱말을 옮긴 뒤 처리합니다
  [/\b(?:pending approval|awaiting approval|pending)\b/gi, " 승인 대기 "],
  [/\bapproved\b/gi, " 승인 "],
  [/\bhigh[- ]risk\b/gi, " 고위험 "],
  // 문서 유형
  [/\b(?:procedures?|aops?|eops?)\b/gi, (m) => (/^(aop|eop)s?$/i.test(m) ? ` ${m.toUpperCase().replace(/S$/, "")} ` : " 절차서 ")],
  [/\b(?:mocs?|mops?|red-?lines?|management of change)\b/gi, (m) => (/^mop/i.test(m) ? " MOP " : /red/i.test(m) ? " 레드라인 " : " MOC ")],
  [/\b(?:incidents?|alarm list|in progress|open cases?)\b/gi, " 사건 "],
  [/\b(?:archived|archives?|closed|history)\b/gi, " 보관 "],
  [/\b(?:drawings?|p&id|diagrams?)\b/gi, (m) => (/p&id/i.test(m) ? " P&ID " : " 도면 ")],
  [/\b(?:sensors?|instruments?|transmitters?|tags?|gauges?)\b/gi, " 센서 "],
  [/\b(?:equipment|devices?|assets?)\b/gi, " 설비 "],
  // 질적 상태
  [/\bstuck\b/gi, " 고착 "],
  [/\b(?:frozen|flat-?line[sd]?)\b/gi, " 동결 "],
  [/\b(?:abnormal|anomalous|faulty|problem)\b/gi, " 비정상 "],
  [/\b(?:rising|increasing|going up)\b/gi, " 상승 "],
  [/\b(?:falling|dropping|decreasing|going down)\b/gi, " 하강 "],
  [/\b(?:high|higher|excessive)\b/gi, " 높은 "],
  [/\b(?:low|lower|insufficient)\b/gi, " 낮은 "],
  [/\b(?:in alarm|alarms?|alarming)\b/gi, " 경보 "],
  [/\btrips?\b/gi, " 트립 "],
  [/\bnormal\b/gi, " 정상 "],
  // 계측 종류
  [/\b(?:outlet temperature|outlet temp)\b/gi, " 출구온도 "],
  [/\b(?:temperature|temp)\b/gi, " 온도 "],
  [/\bpressure\b/gi, " 압력 "],
  [/\bflow(?: ?rate)?\b/gi, " 유량 "],
  [/\bvibration\b/gi, " 진동 "],
  [/\b(?:opening|position|valve position)\b/gi, " 개도 "],
  [/\bcurrent\b/gi, " 전류 "],
  [/\blevel\b/gi, " 수위 "],
  [/\boutput\b/gi, " 출력 "],
  [/\bpower\b/gi, " 전력 "],
  [/\bconcentration\b/gi, " 농도 "],
  // 설비 종류 (복수형 포함)
  [/\bvalves?\b/gi, " 밸브 "],
  [/\bpumps?\b/gi, " 펌프 "],
  [/\b(?:fans?|blowers?)\b/gi, " 팬 "],
  [/\bturbines?\b/gi, " 터빈 "],
  [/\bbearings?\b/gi, " 베어링 "],
  [/\bbreakers?\b/gi, " 차단기 "],
  [/\btransformers?\b/gi, " 변압기 "],
  [/\bsuperheaters?\b/gi, " 과열기 "],
  [/\breheaters?\b/gi, " 재열기 "],
  [/\bheaters?\b/gi, " 가열기 "],
  [/\bdeaerators?\b/gi, " 탈기기 "],
  [/\bcondensers?\b/gi, " 복수기 "],
  [/\bcoolers?\b/gi, " 냉각기 "],
  [/\bgenerators?\b/gi, " 발전기 "],
  [/\bburners?\b/gi, " 버너 "],
  [/\b(?:esp|electrostatic precipitators?)\b/gi, " 집진기 "],
  [/\bstacks?\b/gi, " 연돌 "],
  [/\bgpu(?: racks?)?\b(?!\s*랙)/gi, " GPU 랙 "],
  [/\bmanifolds?\b/gi, " 매니폴드 "],
  [/\b(?:heat )?exchangers?\b/gi, " 열교환기 "],
  [/\bchillers?\b/gi, " 칠러 "],
  [/\bcooling towers?\b/gi, " 냉각탑 "],
  [/\b(?:crahs?|air handlers?)\b/gi, " 항온항습기 "],
  [/\bups\b/gi, " UPS "],
  [/\bbatter(?:y|ies)\b/gi, " 배터리 "],
  [/\bpdus?\b/gi, " PDU "],
  [/\bswitch(?:es)?\b/gi, " 스위치 "],
  [/\btransceivers?\b/gi, " 트랜시버 "],
  [/\bstorage\b/gi, " 스토리지 "],
  [/\btanks?\b/gi, " 탱크 "],
];

/** 뜻 없는 영어 낱말 (빼고 보냅니다) */
const FILLER = new Set(["with", "the", "a", "an", "in", "on", "of", "at", "show", "find", "list", "all", "that", "is", "are", "which", "whose", "for", "to", "me", "and", "where", "has", "have", "its", "by", "from", "this", "these", "those", "please", "any", "some"]);
const KIND = new Set(["밸브", "펌프", "팬", "터빈", "베어링", "차단기", "변압기", "과열기", "재열기", "가열기", "탈기기", "복수기", "냉각기", "발전기", "버너", "집진기", "연돌", "매니폴드", "열교환기", "칠러", "냉각탑", "항온항습기", "배터리", "스위치", "트랜시버", "스토리지", "탱크", "설비"]);
const NEG = /^(?:not|non|except|excluding|without|no)$/i;
/** 머리 명사 뒤에서 꾸밈을 여는 낱말 */
const PREP = new Set(["on", "in", "of", "at", "with", "that", "which", "whose", "where", "for", "from", "near", "around"]);

/** 영어 질의를 엔진이 읽는 한국어 낱말로. 한글이 이미 있으면(한국어로 친 질의) 그대로 보냅니다. */
export function queryToKo(q: string): string {
  if (/[가-힣]/.test(q)) return q;
  let s = ` ${q} `;
  for (const [re, ko] of nameRules) s = s.replace(re, ko);
  for (const [re, rep] of RULES) s = s.replace(re, rep as never);
  for (const [re, ko] of phraseRules) s = s.replace(re, ko);
  const words = s.split(/\s+/).filter(Boolean);
  // "not (a) X" → "X 아닌" (한 낱말로 묶어 맨 앞에 둡니다: "밸브 아닌 103동 설비")
  const negated: string[] = [];
  const out: string[] = [];
  const heads: string[] = [];
  for (let i = 0; i < words.length; i++) {
    if (NEG.test(words[i])) {
      let j = i + 1;
      while (j < words.length && FILLER.has(words[j].toLowerCase())) j++;
      if (j < words.length) {
        negated.push(`${words[j]} 아닌`);
        i = j;
      }
      continue;
    }
    if (FILLER.has(words[i].toLowerCase())) continue;
    // 영어는 머리 명사 뒤에 꾸밈이 붙고("valves on floor 2"), 한국어는 머리 명사가 끝에 옵니다 ("2층 밸브").
    // 전치사가 뒤따르는 설비 종류 낱말만 맨 뒤로 옮깁니다 ("valve ID fan" 처럼 이미 머리가 끝이면 그대로).
    const next = words[i + 1]?.toLowerCase();
    if (KIND.has(words[i]) && next && PREP.has(next)) heads.push(words[i]);
    else out.push(words[i]);
  }
  return [...negated, ...out, ...heads].join(" ").trim();
}
