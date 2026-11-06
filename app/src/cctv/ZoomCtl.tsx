/** 왼쪽 아래 세로 확대 단추 (noto-sandbox 미니맵 자리의 유리 카드). 휠 · 끌기 · 두 번 누르기와 같은 일을 합니다. */
export function ZoomCtl({ scale, onZoom, onReset }: { scale: number; onZoom: (factor: number) => void; onReset: () => void }) {
  return (
    <div className="zoom-ctl vp-glass ov-avoid" role="group" aria-label="도면 확대" title="휠: 확대 · 끌기: 이동 · 두 번 누르기: 맞춤">
      <button className="vt-btn" onClick={() => onZoom(1.25)} aria-label="확대">
        ＋
      </button>
      <span className="mono zoom-pct">{Math.round(scale * 100)}%</span>
      <button className="vt-btn" onClick={() => onZoom(1 / 1.25)} aria-label="축소">
        －
      </button>
      <button className="vt-btn" onClick={onReset} aria-label="빈 자리에 맞춤">
        ⤢
      </button>
    </div>
  );
}
