import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import { fitRect } from "../cctv/stage";
import { drawingImage } from "../drawings";
import { useLang, useT } from "../i18n";
import type { Context } from "../PlantTopologyView";
import type { A0Info, A0Tile } from "../types";

const pct = (v: number, total: number) => `${(v / total) * 100}%`;

/** A0 종합 도면 뷰 (Part 8-4 1 · 5절). 마스터 도면 다섯 장을 원장 배치대로 앉히고 격자 타일을 겹칩니다.
 *  판독은 이 타일 단위로 하고(A0 한 장은 글자가 너무 작아 엔진 가정 밖), 타일을 누르면 그 타일로 확대합니다.
 *  2차 개선안: 종합 도면은 무대의 빈 자리에 맞춰 크게 그리고, 타일 미니맵은 왼쪽 아래 유리 카드로 늘 떠 있습니다. */
export function A0View({ ctx, a0 }: { ctx: Context; a0: A0Info }) {
  const [W, H] = a0.pixel_size;
  const say = useT();
  const lang = useLang();
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [zoom, setZoom] = useState<A0Tile | null>(null);

  useEffect(() => {
    for (const s of a0.sheets) drawingImage(s.drawing).then((url) => setUrls((u) => ({ ...u, [s.drawing]: url }))).catch(() => {});
  }, [a0, lang]);

  const tileOf = (kks: string | null | undefined) => (kks ? a0.equipment[kks]?.tile ?? null : null);
  const marks = (id: string) => [id === tileOf(ctx.selected) ? "selected" : "", id === tileOf(ctx.rootKks) ? "root" : "", id === tileOf(ctx.alarmKks) ? "alarm" : "", id === zoom?.id ? "zoomed" : ""].join(" ");
  // 확대: 타일 왼쪽 위를 원점으로 옮기고 타일이 보기 창을 채우게 키웁니다 (transform-origin 0 0).
  const scale = zoom ? Math.min(W / (zoom.x1 - zoom.x0), H / (zoom.y1 - zoom.y0)) : 1;
  const transform = zoom ? `scale(${scale}) translate(${-(zoom.x0 / W) * 100}%, ${-(zoom.y0 / H) * 100}%)` : "none";
  const fit = fitRect(ctx.stage, ctx.inset, W / H);

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
              <button className="plain small" onClick={() => setZoom(null)}>
                {say("전체 보기", "Show all")}
              </button>
            </>
          ) : (
            <span className="muted">{say("타일을 누르면 그 구역으로 확대합니다", "Click a tile to zoom into that area")}</span>
          )}
        </div>,
        ctx.bar,
      )
    : null;

  return (
    <div className="a0-view">
      {bar}
      <div className={`a0-sheet ${ctx.dark ? "dark" : ""}`} style={{ left: fit.x, top: fit.y, width: fit.w, height: fit.h }}>
        <div className="a0-canvas" style={{ transform }}>
          {a0.sheets.map((s) =>
            urls[s.drawing] ? <img key={s.drawing} src={urls[s.drawing]} alt={s.drawing} draggable={false} style={{ left: pct(s.x, W), top: pct(s.y, H), width: pct(a0.sheet_px[0], W) }} /> : null,
          )}
          {a0.tiles.map((t) => (
            <div key={t.id} className={`a0-tile ${marks(t.id)}`} style={{ left: pct(t.x0, W), top: pct(t.y0, H), width: pct(t.x1 - t.x0, W), height: pct(t.y1 - t.y0, H) }} onClick={() => setZoom(t)}>
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
          <button key={t.id} className={`a0-mini ${marks(t.id)}`} onClick={() => setZoom(zoom?.id === t.id ? null : t)}>
            {t.id}
          </button>
        ))}
      </div>
    </div>
  );
}
