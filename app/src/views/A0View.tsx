import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import type { Context } from "../PlantTopologyView";
import type { A0Info, A0Tile } from "../types";

const images = new Map<string, string>();

const pct = (v: number, total: number) => `${(v / total) * 100}%`;

/** A0 종합 도면 뷰 (Part 8-4 1 · 5절). 마스터 도면 다섯 장을 원장 배치대로 앉히고 격자 타일을 겹칩니다.
 *  판독은 이 타일 단위로 하고(A0 한 장은 글자가 너무 작아 엔진 가정 밖), 타일을 누르면 그 타일로 확대합니다. */
export function A0View({ ctx, a0 }: { ctx: Context; a0: A0Info }) {
  const [W, H] = a0.pixel_size;
  const [urls, setUrls] = useState<Record<string, string>>({});
  const [zoom, setZoom] = useState<A0Tile | null>(null);

  useEffect(() => {
    for (const s of a0.sheets) {
      const hit = images.get(s.drawing);
      if (hit) {
        setUrls((u) => ({ ...u, [s.drawing]: hit }));
        continue;
      }
      invoke<string>("drawing_image", { id: s.drawing }).then((url) => {
        images.set(s.drawing, url);
        setUrls((u) => ({ ...u, [s.drawing]: url }));
      });
    }
  }, [a0]);

  const tileOf = (kks: string | null | undefined) => (kks ? a0.equipment[kks]?.tile ?? null : null);
  const marks = (id: string) => [id === tileOf(ctx.selected) ? "selected" : "", id === tileOf(ctx.rootKks) ? "root" : "", id === tileOf(ctx.alarmKks) ? "alarm" : "", id === zoom?.id ? "zoomed" : ""].join(" ");
  // 확대: 타일 왼쪽 위를 원점으로 옮기고 타일이 보기 창을 채우게 키웁니다 (transform-origin 0 0).
  const scale = zoom ? Math.min(W / (zoom.x1 - zoom.x0), H / (zoom.y1 - zoom.y0)) : 1;
  const transform = zoom ? `scale(${scale}) translate(${-(zoom.x0 / W) * 100}%, ${-(zoom.y0 / H) * 100}%)` : "none";

  return (
    <div className="a0-view">
      <div className="a0-bar">
        <b>
          [A0 종합] {a0.id} ({a0.rev})
        </b>
        <span className="muted">
          {W.toLocaleString()} × {H.toLocaleString()} px · {a0.grid.rows}×{a0.grid.cols} 타일 · 겹침 {Math.round(a0.grid.overlap * 100)}%
        </span>
        {ctx.selected && tileOf(ctx.selected) && (
          <span>
            선택 설비 타일 <b>{tileOf(ctx.selected)}</b>
          </span>
        )}
        {zoom ? (
          <>
            <span>
              확대 <b>{zoom.id}</b>
            </span>
            <button className="plain small" onClick={() => setZoom(null)}>
              전체 보기
            </button>
          </>
        ) : (
          <span className="muted">타일을 누르면 그 구역으로 확대합니다</span>
        )}
      </div>
      <div className="a0-sheet" style={{ aspectRatio: `${W} / ${H}` }}>
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
              className={`a0-eq ${kks === ctx.selected ? "selected" : ""} ${kks === ctx.rootKks ? "root" : ""} ${kks === ctx.alarmKks ? "alarm" : ""}`}
              style={{ left: `${e.bbox[0] * 100}%`, top: `${e.bbox[1] * 100}%`, width: `${(e.bbox[2] - e.bbox[0]) * 100}%`, height: `${(e.bbox[3] - e.bbox[1]) * 100}%` }}
              title={`${kks} · 타일 ${e.tile ?? "-"}`}
              aria-label={kks}
              onClick={() => ctx.onSelect(kks)}
            />
          ))}
        </div>
        {zoom && (
          <div className="a0-minimap" style={{ gridTemplateColumns: `repeat(${a0.grid.cols}, 1fr)` }}>
            {a0.tiles.map((t) => (
              <button key={t.id} className={`a0-mini ${marks(t.id)}`} onClick={() => setZoom(t)}>
                {t.id}
              </button>
            ))}
          </div>
        )}
      </div>
    </div>
  );
}
