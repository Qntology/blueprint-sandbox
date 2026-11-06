// 화면 언어 훅. 컴포넌트는 `const t = useT()` 로 `t("한국어", "English")` 처럼 두 언어 글을 나란히 씁니다.
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen as rawListen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { useCallback, useEffect, useState } from "react";
import { applyLang, getLang, onLang, type Lang } from "./lang";

export type { Lang } from "./lang";

export function useLang(): Lang {
  const [lang, setLang] = useState(getLang());
  useEffect(() => onLang(setLang), []);
  return lang;
}

export type T = (ko: string, en: string) => string;

export function useT(): T {
  const lang = useLang();
  return useCallback((ko: string, en: string) => (lang === "en" ? en : ko), [lang]);
}

/** 백엔드의 화면 언어를 읽습니다 (첫 화면을 그리기 전에 한 번) */
export async function loadLang(): Promise<Lang> {
  try {
    const r = await rawInvoke<{ language: Lang }>("get_language");
    applyLang(r.language);
  } catch {
    // 백엔드가 아직 언어를 모르면 한국어
  }
  return getLang();
}

/** 다른 창(설정)에서 언어를 바꾸면 따라가고, 창 제목을 그 언어로 맞춥니다. */
export function useLanguageSync(title: (lang: Lang) => string) {
  const lang = useLang();
  useEffect(() => {
    const stop = rawListen<{ language: Lang }>("language_changed", (e) => applyLang(e.payload.language));
    return () => void stop.then((f) => f());
  }, []);
  useEffect(() => {
    getCurrentWindow()
      .setTitle(title(lang))
      .catch(() => {});
  }, [lang, title]);
  return lang;
}

/** 설정에서 언어를 고릅니다 (백엔드 설정 파일에 남고 두 창이 함께 바뀝니다) */
export async function chooseLang(l: Lang): Promise<void> {
  const r = await rawInvoke<{ language: Lang }>("set_language", { language: l });
  applyLang(r.language);
}
