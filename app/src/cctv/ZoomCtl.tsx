import { useT } from "../i18n";

/** 왼쪽 아래 세로 확대 단추 (noto-sandbox 미니맵 자리의 유리 카드). 휠 · 끌기 · 두 번 누르기와 같은 일을 합니다. */
export function ZoomCtl({ scale, onZoom, onReset }: { scale: number; onZoom: (factor: number) => void; onReset: () => void }) {
  const t = useT();
  return (
    <div className="zoom-ctl vp-glass ov-avoid" role="group" aria-label={t("도면 확대", "Drawing zoom")} title={t("휠: 확대 · 끌기: 이동 · 두 번 누르기: 맞춤", "Wheel: zoom · drag: pan · double-click: fit")}>
      <button className="vt-btn" onClick={() => onZoom(1.25)} aria-label={t("확대", "Zoom in")}>
        ＋
      </button>
      <span className="mono zoom-pct">{Math.round(scale * 100)}%</span>
      <button className="vt-btn" onClick={() => onZoom(1 / 1.25)} aria-label={t("축소", "Zoom out")}>
        －
      </button>
      <button className="vt-btn" onClick={onReset} aria-label={t("빈 자리에 맞춤", "Fit to free area")}>
        ⤢
      </button>
    </div>
  );
}
