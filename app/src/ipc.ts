// 백엔드 호출 · 이벤트 감싸기. 영어 화면이면 응답 · 이벤트 · 오류의 한글 글을 번역 표로 바꿉니다 (i18n/translate.ts).
// 컴포넌트는 `@tauri-apps/api` 대신 이 모듈의 invoke · listen 을 씁니다.
import { invoke as rawInvoke } from "@tauri-apps/api/core";
import { listen as rawListen, type EventCallback, type UnlistenFn } from "@tauri-apps/api/event";
import { getLang } from "./i18n/lang";
import { audit, translateDeep, translateText } from "./i18n/translate";

/** 번역하지 않는 명령 (이미지 · 언어 설정) */
const RAW = new Set(["drawing_image", "get_language", "set_language"]);
/** 응답에서 번역하지 않고 그대로 두는 칸: 현장 서류를 읽은 값(점검자 이름 등)은 그대로 다시 백엔드로 가고, 스캔 원문은 서류 그대로 보여야 합니다 */
const KEEP: Record<string, string[]> = { scan_field_documents: ["prefill", "raw"] };

export async function invoke<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    const r = await rawInvoke<T>(cmd, args);
    if (RAW.has(cmd)) return r;
    audit(r);
    if (getLang() !== "en") return r;
    const out = translateDeep(r);
    for (const k of KEEP[cmd] ?? []) (out as Record<string, unknown>)[k] = (r as Record<string, unknown>)[k];
    return out;
  } catch (e) {
    throw getLang() === "en" ? translateText(String(e)) : e;
  }
}

export function listen<T>(event: string, handler: EventCallback<T>): Promise<UnlistenFn> {
  return rawListen<T>(event, (e) => {
    audit(e.payload);
    handler(getLang() === "en" ? { ...e, payload: translateDeep(e.payload) } : e);
  });
}
