import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { fitRect, usePanZoom } from "../cctv/stage";
import { ZoomCtl } from "../cctv/ZoomCtl";
import { drawingImage } from "../drawings";
import { useLang, useT } from "../i18n";
import type { Context } from "../PlantTopologyView";
import type { A0Info, A0Tile } from "../types";

const pct = (v: number, total: number) => `${(v / total) * 100}%`;

/** A0 종합 도면 뷰 (Part 8-4 1 · 5절). 마스터 도면 다섯 장을 원장 배치대로 앉히고 격자 타일을 겹칩니다.
 *  판독은 이 타일 단위로 하고(A0 한 장은 글자가 너무 작아 엔진 가정 밖), 타일을 누르면 그 타일로 확대합니다.
 *  2차 개선안: 종합 도면은 무대의 빈 자리에 맞춰 크게 그리고, 타일 미니맵은 왼쪽 아래 유리 카드로 늘 떠 있습니다.
 *  UI 단순화 U10: 도면 · Mermaid 와 같은 휠 확대 · 끌기 · 확대 단추를 쓰고, 타일 누르기는 그 타일에 맞추기만 합니다. */
export function A0View({ ctx, a0 }: { ctx: Context; a0: A0Info }) {
  const [W, H] = a0.pixel_size;
  const say = useT();
  const lang = useLang();
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [zoom, setZoom] = useState<A0Tile | null>(null);
  const fit = fitRect(ctx.stage, ctx.inset, W / H);
  const pz = usePanZoom(fit, a0.id);

  useEffect(() => {
    for (const s of a0.sheets) drawingImage(s.drawing).then((url) => setUrls((u) => ({ ...u, [s.drawing]: url }))).catch(() => {});
  }, [a0, lang]);

  const tileOf = (kks: string | null | undefined) => (kks ? a0.equipment[kks]?.tile ?? null : null);
  const marks = (id: string) => [id === tileOf(ctx.selected) ? "selected" : "", id === tileOf(ctx.rootKks) ? "root" : "", id === tileOf(ctx.alarmKks) ? "alarm" : "", id === zoom?.id ? "zoomed" : ""].join(" ");
  /** 타일(A0 픽셀)을 판 좌표(배율 1)로 옮겨 그 자리에 맞춥니다 */
  const focusTile = (tile: A0Tile) => {
    setZoom(tile);
    pz.zoomTo({ x: (tile.x0 / W) * fit.w, y: (tile.y0 / H) * fit.h, w: ((tile.x1 - tile.x0) / W) * fit.w, h: ((tile.y1 - tile.y0) / H) * fit.h });
  };
  const showAll = () => {
    setZoom(null);
    pz.reset();
  };

  const bar = ctx.bar
    ? createPortal(
        <div className="view-bar ov-bar a0-bar">
          <b>
            {say("[A0 종합]", "[A0 composite]")} {a0.id} ({a0.rev})
          </b>
          <span className="muted">
            {W.toLocaleString()} × {H.toLocaleString()} px · {a0.grid.rows}×{a0.grid.cols} {say("타일", "tiles")} · {say("겹침", "overlap")} {Math.round(a0.grid.overlap * 100)}%
          </span>
          {ctx.selected && tileOf(ctx.selected) && (
            <span>
              {say("선택 설비 타일", "Selected equipment tile")} <b>{tileOf(ctx.selected)}</b>
            </span>
          )}
          {zoom ? (
            <>
              <span>
                {say("확대", "Zoomed")} <b>{zoom.id}</b>
              </span>
              <button className="plain small" onClick={showAll}>
                {say("전체 보기", "Show all")}
              </button>
            </>
          ) : (
            <span className="muted">{say("타일을 누르면 그 구역으로 확대합니다 (휠 확대 · 끌어 이동)", "Click a tile to zoom into that area (wheel to zoom · drag to pan)")}</span>
          )}
        </div>,
        ctx.bar,
      )
    : null;

  return (
    <>
      {bar}
      <div className="sheet-viewport a0-view" ref={pz.bind}>
        <div className={`a0-sheet ${ctx.dark ? "dark" : ""}`} style={pz.style}>
          {a0.sheets.map((s) =>
            urls[s.drawing] ? <img key={s.drawing} src={urls[s.drawing]} alt={s.drawing} draggable={false} style={{ left: pct(s.x, W), top: pct(s.y, H), width: pct(a0.sheet_px[0], W) }} /> : null,
          )}
          {a0.tiles.map((t) => (
            <div key={t.id} className={`a0-tile ${marks(t.id)}`} style={{ left: pct(t.x0, W), top: pct(t.y0, H), width: pct(t.x1 - t.x0, W), height: pct(t.y1 - t.y0, H) }} onClick={() => focusTile(t)}>
              <span>
                {t.id}
                {t.id === tileOf(ctx.rootKks) ? " 🔥" : ""}
                {t.id === tileOf(ctx.alarmKks) ? " ⚠️" : ""}
              </span>
            </div>
          ))}
          {Object.entries(a0.equipment).map(([kks, e]) => (
            <button
              key={kks}
              data-kks={kks}
              className={`a0-eq ${kks === ctx.selected ? "selected" : ""} ${kks === ctx.rootKks ? "root" : ""} ${kks === ctx.alarmKks ? "alarm" : ""} ${ctx.marks.has(kks) ? "hit" : ""}`}
              style={{ left: `${e.bbox[0] * 100}%`, top: `${e.bbox[1] * 100}%`, width: `${(e.bbox[2] - e.bbox[0]) * 100}%`, height: `${(e.bbox[3] - e.bbox[1]) * 100}%` }}
              title={`${kks} · ${say("타일", "tile")} ${e.tile ?? "-"}`}
              aria-label={kks}
              onClick={() => ctx.onSelect(kks)}
            />
          ))}
        </div>
      </div>
      <div className="a0-minimap vp-glass ov-avoid" style={{ gridTemplateColumns: `repeat(${a0.grid.cols}, 1fr)` }} aria-label={say("A0 타일 미니맵", "A0 tile minimap")}>
        {a0.tiles.map((t) => (
          <button key={t.id} className={`a0-mini ${marks(t.id)}`} onClick={() => (zoom?.id === t.id ? showAll() : focusTile(t))}>
            {t.id}
          </button>
        ))}
      </div>
      <ZoomCtl scale={pz.view.s} onZoom={(f) => pz.zoomAt(f)} onReset={showAll} />
    </>
  );
}
