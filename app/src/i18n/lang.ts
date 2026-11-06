// 화면 언어 (한국어 · 영어). 백엔드(`get_language` · `set_language`)가 정하고 두 창이 `language_changed` 로 따라갑니다.
// React 밖(invoke 감싸기 · 숫자 · 시각 꼴)도 쓰므로 모듈 변수로 둡니다.
export type Lang = "ko" | "en";

let current: Lang = "ko";
const listeners = new Set<(l: Lang) => void>();

export function getLang(): Lang {
  return current;
}

/** 화면 언어만 바꿉니다 (백엔드에 남기는 것은 `useLangControl().choose`) */
export function applyLang(l: Lang) {
  if (l === current) return;
  current = l;
  document.documentElement.lang = l;
  listeners.forEach((f) => f(l));
}

export function onLang(f: (l: Lang) => void): () => void {
  listeners.add(f);
  return () => listeners.delete(f);
}

/** React 밖에서 쓰는 두 언어 글 (한국어, 영어) */
export function tr(ko: string, en: string): string {
  return current === "en" ? en : ko;
}

/** 시각 꼴의 지역 (한국어 `14시 20분 9초` · 영어 `14:20:09`) */
export function locale(): string {
  return current === "en" ? "en-GB" : "ko-KR";
}
