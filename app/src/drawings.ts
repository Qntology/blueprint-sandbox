// 마스터 도면 그림 (data URL) 캐시. 도면 뷰 · A0 종합 뷰 · CCTV 카드의 도면 조각이 같은 그림을 한 번만 받습니다.
// 영어 화면이면 백엔드가 같은 배치로 글만 영어인 도면을 주므로 언어마다 따로 담습니다.
import { useEffect, useState } from "react";
import { useLang } from "./i18n";
import { getLang } from "./i18n/lang";
import { invoke } from "./ipc";

/** 마스터 도면 한 장의 픽셀 크기 (도면 생성기 `gen_pid_drawings.py` 의 A3 가로 서식) */
export const SHEET_PX: [number, number] = [2000, 1400];

const cache = new Map<string, string>();
const pending = new Map<string, Promise<string>>();

export function drawingImage(id: string): Promise<string> {
  const key = `${getLang()}:${id}`;
  const hit = cache.get(key);
  if (hit) return Promise.resolve(hit);
  let p = pending.get(key);
  if (!p) {
    p = invoke<string>("drawing_image", { id })
      .then((url) => {
        cache.set(key, url);
        return url;
      })
      .finally(() => pending.delete(key));
    pending.set(key, p);
  }
  return p;
}

/** 도면 그림 URL. 아직 받지 않았으면 null 이고, 받으면 다시 그립니다. */
export function useDrawingImage(id: string | null | undefined): string | null {
  const lang = useLang();
  const [url, setUrl] = useState<string | null>(id ? cache.get(`${lang}:${id}`) ?? null : null);
  useEffect(() => {
    if (!id) {
      setUrl(null);
      return;
    }
    const hit = cache.get(`${lang}:${id}`);
    if (hit) {
      setUrl(hit);
      return;
    }
    setUrl(null);
    let alive = true;
    drawingImage(id)
      .then((u) => alive && setUrl(u))
      .catch(() => {});
    return () => {
      alive = false;
    };
  }, [id, lang]);
  return url;
}
