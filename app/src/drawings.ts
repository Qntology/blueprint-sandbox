// 마스터 도면 그림 (data URL) 캐시. 도면 뷰 · A0 종합 뷰 · CCTV 카드의 도면 조각이 같은 그림을 한 번만 받습니다.
import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";

/** 마스터 도면 한 장의 픽셀 크기 (도면 생성기 `gen_pid_drawings.py` 의 A3 가로 서식) */
export const SHEET_PX: [number, number] = [2000, 1400];

const cache = new Map<string, string>();
const pending = new Map<string, Promise<string>>();

export function drawingImage(id: string): Promise<string> {
  const hit = cache.get(id);
  if (hit) return Promise.resolve(hit);
  let p = pending.get(id);
  if (!p) {
    p = invoke<string>("drawing_image", { id })
      .then((url) => {
        cache.set(id, url);
        return url;
      })
      .finally(() => pending.delete(id));
    pending.set(id, p);
  }
  return p;
}

/** 도면 그림 URL. 아직 받지 않았으면 null 이고, 받으면 다시 그립니다. */
export function useDrawingImage(id: string | null | undefined): string | null {
  const [url, setUrl] = useState<string | null>(id ? cache.get(id) ?? null : null);
  useEffect(() => {
    if (!id) {
      setUrl(null);
      return;
    }
    const hit = cache.get(id);
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
  }, [id]);
  return url;
}
