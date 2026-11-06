// 풀스크린 무대 (2차 개선안: noto-sandbox 의 `inset: 0` 월드 + 떠 있는 유리 HUD).
// 세 도면 뷰가 무대 전체에 깔리고, 도면은 오른쪽 결재 HUD 를 뺀 빈 자리에 비율을 지켜 맞춥니다. 휠로 확대, 끌어서 이동합니다.
import { useCallback, useEffect, useLayoutEffect, useRef, useState, type RefObject } from "react";

export interface Size {
  w: number;
  h: number;
}

/** 무대 가장자리에서 떠 있는 판이 차지하는 폭 (위 · 오른쪽 · 아래 · 왼쪽) */
export interface Insets {
  top: number;
  right: number;
  bottom: number;
  left: number;
}

export interface Rect {
  x: number;
  y: number;
  w: number;
  h: number;
}

/** 오른쪽 결재 HUD 폭 (styles.css 의 --hud-w 와 같은 값) */
export const HUD_W = 340;
/** 좌상단 · 우상단 첫 줄 알약 아래 (도면은 이 아래부터 맞춥니다) */
export const TOP_INSET = 48;

export function useElementSize(ref: RefObject<HTMLElement>): Size {
  const [size, setSize] = useState<Size>({ w: 0, h: 0 });
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const read = () => setSize((s) => (s.w === el.clientWidth && s.h === el.clientHeight ? s : { w: el.clientWidth, h: el.clientHeight }));
    read();
    const ro = new ResizeObserver(read);
    ro.observe(el);
    return () => ro.disconnect();
  }, [ref]);
  return size;
}

/** 빈 자리 안에 가로 · 세로 비율 `aspect` 인 판을 가운데 맞춥니다. `maxScale` 는 원래 크기(natural) 대비 최대 배율. */
export function fitRect(stage: Size, inset: Insets, aspect: number, natural?: Size, maxScale = Infinity): Rect {
  const area = { x: inset.left, y: inset.top, w: Math.max(stage.w - inset.left - inset.right, 1), h: Math.max(stage.h - inset.top - inset.bottom, 1) };
  let w = Math.min(area.w, area.h * aspect);
  if (natural && Number.isFinite(maxScale)) w = Math.min(w, natural.w * maxScale);
  const h = w / aspect;
  return { x: area.x + (area.w - w) / 2, y: area.y + (area.h - h) / 2, w, h };
}

export interface View {
  s: number;
  x: number;
  y: number;
}

const MIN_S = 0.4;
const MAX_S = 8;

/** 휠 확대(커서 자리 고정) · 끌어 이동 · 두 번 누르면 맞춤. 판은 `fit` 자리에 놓이고 그 위에 `translate(x, y) scale(s)` 를 겁니다.
 *  끌다 놓은 클릭은 버립니다 (끌기 끝에 설비 상자가 눌리지 않게). `bind` 는 보기 창(무대와 같은 좌표) 요소에 거는 ref 입니다. */
export function usePanZoom(fit: Rect, resetKey: unknown) {
  const [view, setView] = useState<View>({ s: 1, x: 0, y: 0 });
  const [el, bind] = useState<HTMLElement | null>(null);
  const live = useRef({ view, fit });
  live.current = { view, fit };

  useEffect(() => setView({ s: 1, x: 0, y: 0 }), [resetKey]);

  const zoomAt = useCallback((factor: number, px?: number, py?: number) => {
    const { view: v, fit: f } = live.current;
    const s = Math.min(MAX_S, Math.max(MIN_S, v.s * factor));
    // 기준점(무대 좌표)은 판 위 같은 자리에 남습니다: X = f.x + x + s · p
    const X = px ?? f.x + f.w / 2;
    const Y = py ?? f.y + f.h / 2;
    const p = { x: (X - f.x - v.x) / v.s, y: (Y - f.y - v.y) / v.s };
    const next = { s, x: X - f.x - s * p.x, y: Y - f.y - s * p.y };
    // 한 프레임에 휠이 여러 번 와도 쌓이게 다음 렌더를 기다리지 않고 바로 적어 둡니다
    live.current = { ...live.current, view: next };
    setView(next);
  }, []);

  const reset = useCallback(() => setView({ s: 1, x: 0, y: 0 }), []);

  useEffect(() => {
    if (!el) return;
    const origin = () => el.getBoundingClientRect();
    // 휠은 무대에서 받습니다: 도면 위에 뜬 CCTV 말풍선 위에서 굴려도 도면이 확대되고, 떠 있는 판(HUD · 허브 · 팝오버) 위에서는 그 판이 스크롤됩니다.
    const wheelHost = el.closest(".stage") ?? el;
    const onWheel = (e: WheelEvent) => {
      if (!(e.target as HTMLElement).closest?.(".canvas-full, .cctv-layer")) return;
      e.preventDefault();
      const r = origin();
      zoomAt(e.deltaY < 0 ? 1.15 : 1 / 1.15, e.clientX - r.left, e.clientY - r.top);
    };
    let drag: { id: number; x: number; y: number; vx: number; vy: number; moved: boolean } | null = null;
    let swallow = false;
    const onDown = (e: PointerEvent) => {
      if (e.button !== 0 && e.button !== 1) return;
      if ((e.target as HTMLElement).closest("input, select, textarea, .no-pan")) return;
      const v = live.current.view;
      drag = { id: e.pointerId, x: e.clientX, y: e.clientY, vx: v.x, vy: v.y, moved: false };
    };
    const onMove = (e: PointerEvent) => {
      if (!drag || e.pointerId !== drag.id) return;
      const dx = e.clientX - drag.x;
      const dy = e.clientY - drag.y;
      if (!drag.moved && Math.hypot(dx, dy) < 5) return;
      if (!drag.moved) {
        drag.moved = true;
        el.setPointerCapture(e.pointerId);
        el.classList.add("panning");
      }
      const x = drag.vx + dx;
      const y = drag.vy + dy;
      setView((v) => ({ ...v, x, y }));
    };
    const onUp = (e: PointerEvent) => {
      if (!drag || e.pointerId !== drag.id) return;
      swallow = drag.moved;
      drag = null;
      el.classList.remove("panning");
    };
    // 끌기를 끝낸 클릭은 상자 · 노드까지 내려가지 않게 잡습니다 (캡처 단계)
    const onClick = (e: MouseEvent) => {
      if (!swallow) return;
      swallow = false;
      e.stopPropagation();
      e.preventDefault();
    };
    const onDouble = (e: MouseEvent) => {
      if ((e.target as HTMLElement).closest("button, a, .no-pan")) return;
      setView({ s: 1, x: 0, y: 0 });
    };
    wheelHost.addEventListener("wheel", onWheel as EventListener, { passive: false });
    el.addEventListener("pointerdown", onDown);
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerup", onUp);
    el.addEventListener("pointercancel", onUp);
    el.addEventListener("click", onClick, true);
    el.addEventListener("dblclick", onDouble);
    return () => {
      wheelHost.removeEventListener("wheel", onWheel as EventListener);
      el.removeEventListener("pointerdown", onDown);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerup", onUp);
      el.removeEventListener("pointercancel", onUp);
      el.removeEventListener("click", onClick, true);
      el.removeEventListener("dblclick", onDouble);
    };
  }, [el, zoomAt]);

  const style = { left: fit.x, top: fit.y, width: fit.w, height: fit.h, transform: `translate(${view.x}px, ${view.y}px) scale(${view.s})` };
  return { view, style, bind, zoomAt, reset };
}

/** 설비 자리 (무대 좌표). `remote` 는 설비가 다른 도면에 있어 이 도면의 점프 커넥터로 대신 가리킬 때 그 도면 번호 */
export interface Anchor extends Rect {
  remote?: string;
}

/** 지금 뷰에서 설비가 어디 그려졌는지 (CCTV 말풍선의 지시선 출발점). DOM 뷰는 `data-kks` 를, 마인드맵은 노드 좌표를 씁니다. */
export interface Locator {
  find(kks: string): Anchor | null;
  /** 말풍선이 되도록 덮지 않을 다른 설비 자리 */
  others(): Rect[];
}

export function relRect(el: Element, origin: DOMRect): Rect {
  const r = el.getBoundingClientRect();
  return { x: r.left - origin.left, y: r.top - origin.top, w: r.width, h: r.height };
}

/** DOM 뷰(도면 · A0 · Mermaid)의 설비 자리: `[data-kks]` 요소. 없고 다른 도면에 있으면 그 도면으로 가는 점프 커넥터(`[data-drawing]`). */
export function domLocator(stage: HTMLElement, drawingOf: (kks: string) => string | undefined): Locator {
  const canvas = () => stage.querySelector(".canvas-full");
  return {
    find(kks) {
      const host = canvas();
      if (!host) return null;
      const origin = stage.getBoundingClientRect();
      const el = host.querySelector(`[data-kks="${kks}"]`);
      if (el) {
        const r = relRect(el, origin);
        return r.w > 0 && r.h > 0 ? r : null;
      }
      const d = drawingOf(kks);
      const via = d ? host.querySelector(`.opc[data-other="${kks}"]`) ?? host.querySelector(`.opc[data-drawing="${d}"]`) : null;
      if (via) {
        const r = relRect(via, origin);
        return r.w > 0 ? { ...r, remote: d } : null;
      }
      return null;
    },
    others() {
      const host = canvas();
      if (!host) return [];
      const origin = stage.getBoundingClientRect();
      return [...host.querySelectorAll("[data-kks]")].slice(0, 80).map((el) => relRect(el, origin));
    },
  };
}
