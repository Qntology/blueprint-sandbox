// 영어 화면의 번역 표. 엔진 · 원장 · 모델의 안쪽 글은 한국어이고, 화면에 나가는 글만 여기서 바꿉니다.
//
//   exact     글 전체가 같을 때 (메시지 · 상태 이름처럼 짧은 낱말은 여기에만: 다른 글 속에서 바뀌면 안 됨)
//   spans     틀보다 먼저 글 속 어디서든 통째로 바꾸는 것 (원장 이름 · 긴 조치 문장). 숫자 틀이 이름 속 "103동" 같은 조각을 먼저 바꿔 이름이 깨지지 않게 합니다.
//   patterns  정규식 [찾기, 바꾸기] — 숫자 · 코드가 들어간 문장 틀. 순서대로 모두 적용합니다.
//   phrases   글 속 어디서든 바꿔도 되는 낱말 (용어). 긴 것부터 바꿉니다.
//   names     원장 이름 (설비 · 센서 · 구역 · 도면 · 사람). phrases 처럼 바꾸고, 엔진의 검색이 영어 이름으로도 찾는 데 씁니다 (core/src/search/lexicon.rs).
//
// 바꾼 뒤에도 한글이 남으면 `misses` 에 모읍니다 (E2E 가 읽어 번역 표를 채웁니다).
// `drawing_image` 같은 큰 글은 한글이 없어 그대로 지나갑니다.
import data from "./en.json";

interface Table {
  exact: Record<string, string>;
  patterns: [string, string][];
  phrases: Record<string, string>;
  names?: Record<string, string>;
  spans?: Record<string, string>;
}

const T = data as unknown as Table;
const HANGUL = /[가-힣]/;
const escape = (s: string) => s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const exact = new Map(Object.entries(T.exact));
const patterns = T.patterns.map(([re, rep]) => [new RegExp(re, "g"), rep] as const);
const phrases: Record<string, string> = { ...T.names, ...T.phrases };
const alternation = (keys: string[]) => (keys.length ? new RegExp([...keys].sort((a, b) => b.length - a.length).map(escape).join("|"), "g") : null);
const phraseRe = alternation(Object.keys(phrases));
const spans = T.spans ?? {};
const spanRe = alternation(Object.keys(spans));
const memo = new Map<string, string>();

/** 영어로 바꾼 뒤에도 한글이 남은 글 (번역 표에 없는 것) */
export const misses = new Set<string>();
/** 감사 모드에서 백엔드가 보낸 한글 글 전부 (번역 표를 만들 때) */
export const seen = new Set<string>();

declare global {
  interface Window {
    __i18nAudit?: boolean;
    __i18nMisses?: () => string[];
    __i18nSeen?: () => string[];
  }
}
window.__i18nMisses = () => [...misses];
window.__i18nSeen = () => [...seen];

function line(s: string): string {
  const core = s.trim();
  const hit = exact.get(core);
  if (hit !== undefined) return s.replace(core, hit);
  let out = spanRe ? s.replace(spanRe, (k) => spans[k]) : s;
  for (const [re, rep] of patterns) {
    re.lastIndex = 0;
    out = out.replace(re, rep);
  }
  // 틀로 바꾼 뒤 남은 조각이 통째로 표에 있으면 그것을 씁니다 (틀이 이름 자리를 남긴 경우)
  const rest = exact.get(out.trim());
  if (rest !== undefined) return rest;
  if (phraseRe) out = out.replace(phraseRe, (k) => phrases[k]);
  return out.replace(/ {2,}/g, " ");
}

/** 글 하나를 영어로. 한글이 없으면 그대로입니다. 여러 줄은 줄마다 바꿉니다. */
export function translateText(s: string): string {
  if (!HANGUL.test(s)) return s;
  const hit = memo.get(s);
  if (hit !== undefined) return hit;
  const out = s.includes("\n") ? s.split("\n").map((l) => (HANGUL.test(l) ? line(l) : l)).join("\n") : line(s);
  if (HANGUL.test(out)) misses.add(s);
  memo.set(s, out);
  return out;
}

/** 감사 모드면 백엔드 응답의 한글 글을 모읍니다 (언어와 상관없이) */
export function audit(v: unknown) {
  if (!window.__i18nAudit) return;
  const walk = (x: unknown) => {
    if (typeof x === "string") {
      if (HANGUL.test(x)) for (const l of x.split("\n")) if (HANGUL.test(l)) seen.add(l);
    } else if (Array.isArray(x)) x.forEach(walk);
    else if (x && typeof x === "object") Object.values(x).forEach(walk);
  };
  walk(v);
}

/** 응답 전체의 글을 영어로 (객체 · 배열은 새로 만듭니다) */
export function translateDeep<T>(v: T): T {
  if (typeof v === "string") return translateText(v) as T;
  if (Array.isArray(v)) return v.map(translateDeep) as T;
  if (v && typeof v === "object") {
    const out: Record<string, unknown> = {};
    for (const [k, x] of Object.entries(v as Record<string, unknown>)) out[k] = translateDeep(x);
    return out as T;
  }
  return v;
}
