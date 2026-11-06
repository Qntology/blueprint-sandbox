import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import { shortTag, useTerms } from "../domain";
import type { Context, EdgeLag } from "../PlantTopologyView";
import { equipmentLevel, fixed, fmt, hopDepths, MEDIUM_COLOR, SDS_COLOR, sdsStateAt, sensorsOf, signedSecs, valueAt } from "../plant";
import type { Timeline } from "../types";
import { A0View } from "./A0View";

const images = new Map<string, string>();

type Box = [number, number, number, number];

const pct = (b: Box) => ({ left: `${b[0] * 100}%`, top: `${b[1] * 100}%`, width: `${(b[2] - b[0]) * 100}%`, height: `${(b[3] - b[1]) * 100}%` });

/** 미니 파형 (개선안 2-1 `[미니파형] ──┐______`): 발원 10초 앞 ~ 분석 시각, 재생 커서 */
function Spark({ tl, tag, from, to, t, color }: { tl: Timeline; tag: string; from: number; to: number; t: number; color: string }) {
  const v = tl.series[tag];
  if (!v?.length) return null;
  const pts: [number, number][] = [];
  v.forEach((x, i) => {
    const at = tl.t0_ms + i * tl.period_ms;
    if (at >= from && at <= to) pts.push([at, x]);
  });
  if (pts.length < 2) return null;
  const ys = pts.map((p) => p[1]);
  const lo = Math.min(...ys);
  const hi = Math.max(...ys);
  const span = Math.max(hi - lo, 1e-9);
  const X = (at: number) => ((at - from) / Math.max(to - from, 1)) * 64;
  const Y = (x: number) => 13 - ((x - lo) / span) * 12;
  return (
    <svg className="spark" viewBox="0 0 64 14" preserveAspectRatio="none">
      <polyline points={pts.map(([a, x]) => `${X(a).toFixed(1)},${Y(x).toFixed(1)}`).join(" ")} fill="none" stroke={color} strokeWidth={1.4} />
      {t >= from && t <= to && <line x1={X(t)} x2={X(t)} y1={0} y2={14} stroke="#3aa0ff" strokeWidth={1} />}
    </svg>
  );
}

/** 꺾은선의 길이 가운데 점 (배관 Δt 라벨 자리) */
function midpoint(points: [number, number][]): [number, number] {
  let total = 0;
  for (let i = 1; i < points.length; i++) total += Math.hypot(points[i][0] - points[i - 1][0], points[i][1] - points[i - 1][1]);
  let left = total / 2;
  for (let i = 1; i < points.length; i++) {
    const [a, b] = [points[i - 1], points[i]];
    const seg = Math.hypot(b[0] - a[0], b[1] - a[1]);
    if (seg >= left && seg > 0) return [a[0] + ((b[0] - a[0]) * left) / seg, a[1] + ((b[1] - a[1]) * left) / seg];
    left -= seg;
  }
  return points[0];
}

/** 뷰 ①: P&ID 도면 오버레이 캔버스. 설비 상자를 누르면 선택, 가장자리 점프 커넥터(OPC)를 누르면 연결된 도면으로 넘어갑니다. */
export function DrawingView({ ctx }: { ctx: Context }) {
  const terms = useTerms();
  const { plant, detail, selected, onSelect, mining, t } = ctx;
  const initial = plant.equipment.find((e) => e.kks === selected)?.drawing ?? plant.drawings[0].id;
  const [drawing, setDrawing] = useState(initial);
  const [src, setSrc] = useState<string | null>(images.get(initial) ?? null);

  // 다른 뷰에서 다른 도면의 설비를 고르면 그 도면으로 넘어갑니다.
  useEffect(() => {
    const d = plant.equipment.find((e) => e.kks === selected)?.drawing;
    // A0 종합 뷰에서는 설비를 골라도 마스터 도면으로 넘어가지 않습니다.
    if (d && d !== drawing && !drawing.startsWith("A0:")) setDrawing(d);
  }, [selected, plant]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (drawing.startsWith("A0:")) return;
    const cached = images.get(drawing);
    if (cached) {
      setSrc(cached);
      return;
    }
    setSrc(null);
    invoke<string>("drawing_image", { id: drawing }).then((url) => {
      images.set(drawing, url);
      setSrc(url);
    });
  }, [drawing]);

  const tl = detail?.timeline ?? null;
  const info = plant.drawings.find((d) => d.id === drawing);
  const onSheet = plant.equipment.filter((e) => e.drawing === drawing);
  // 배관은 도면 생성기가 그린 꺾은선을 그대로 씁니다 (도면 이미지의 선과 겹칩니다).
  const pipes = useMemo(() => new Map((plant.pipes[drawing] ?? []).map((p) => [`${p.from}>${p.to}`, p.points])), [plant, drawing]);
  const depths = useMemo(() => (selected ? hopDepths(plant.edges, selected, 3) : new Map<string, number>()), [plant, selected]);
  const opcs = plant.connectors.filter((c) => c.from_drawing === drawing || c.to_drawing === drawing);
  // 원인 경로 배관의 Δt 라벨 (같은 도면 안 배관 · 점프 커넥터로 가는 배관 모두). 여러 간선이 한 구간이면 이 도면의 첫 배관에 한 번만.
  const seen = new Set<EdgeLag>();
  const lagLabels = [...ctx.edgeLags.entries()].flatMap(([key, v]) => {
    const pts = pipes.get(key);
    if (!pts || seen.has(v)) return [];
    seen.add(v);
    return [{ key, at: midpoint(pts), ...v }];
  });
  const sparkFrom = ctx.origin - 10_000;
  const sparkTo = Math.max(detail?.timeline.now_ms ?? 0, ctx.origin + 15_000);

  return (
    <div className="drawing-view">
      <div className="drawing-select">
        <span className="muted">도면 선택</span>
        {plant.drawings.map((d) => {
          const sector = plant.sectors.find((s) => s.key === d.sector);
          return (
            <button key={d.id} className={d.id === drawing ? "chip active" : "chip"} onClick={() => setDrawing(d.id)}>
              {d.id === drawing ? "● " : "○ "}
              {d.id} · {sector?.building}
            </button>
          );
        })}
        {plant.a0.map((a) => (
          <button key={a.id} className={drawing === `A0:${a.id}` ? "chip active a0-chip" : "chip a0-chip"} onClick={() => setDrawing(`A0:${a.id}`)}>
            {drawing === `A0:${a.id}` ? "● " : "○ "}A0 종합 · {a.grid.rows}×{a.grid.cols} 타일
          </button>
        ))}
      </div>
      {drawing.startsWith("A0:") && plant.a0.some((a) => `A0:${a.id}` === drawing) ? (
        <A0View ctx={ctx} a0={plant.a0.find((a) => `A0:${a.id}` === drawing)!} />
      ) : (
        <>
      <div className="drawing-title">
        [표제란] {terms.drawing} 도면번호 <b>{info?.id}</b> ({info?.rev}) · {info?.title} · 발행 {info?.issued}
      </div>
      <div className="sheet">
        {src ? <img src={src} alt={drawing} draggable={false} /> : <div className="sheet-loading">도면을 불러오는 중</div>}
        {mining && (
          <svg className="pipes" viewBox="0 0 1 1" preserveAspectRatio="none">
            {ctx.flows
              .filter((f) => pipes.has(`${f.from}>${f.to}`))
              .map((f) => {
                const pts = pipes.get(`${f.from}>${f.to}`)!;
                const color = f.bottleneck ? "#ff4d4f" : f.on_path ? "#f0a020" : MEDIUM_COLOR[f.medium] ?? "#2fb4c9";
                return (
                  <polyline
                    key={`${f.from}-${f.to}`}
                    points={pts.map((p) => p.join(",")).join(" ")}
                    className={f.bottleneck ? "flow starve" : "flow"}
                    style={{ stroke: color, strokeWidth: (f.bottleneck ? 0.004 : 0.006) * Math.max(0.4, Math.min(1.4, f.ratio)) }}
                  />
                );
              })}
          </svg>
        )}
        {lagLabels.map((l) => (
          <span key={l.key} className={`lag-label ${l.ok ? "ok" : "bad"}`} style={{ left: `${l.at[0] * 100}%`, top: `${l.at[1] * 100}%` }}>
            ⏱️ Δt {l.lag >= 0 ? "+" : ""}
            {l.lag.toFixed(1)}s{l.hops > 1 ? ` · ${l.hops}홉` : ""} (허용 {fmt(l.range[0])}~{fmt(l.range[1])}s {l.ok ? "✓" : "✗"})
          </span>
        ))}
        {onSheet.map((e) => {
          const lv = tl ? equipmentLevel(plant, e.kks, tl, t) : "normal";
          const depth = depths.get(e.kks);
          const track = ctx.tracks.get(e.kks)?.[0];
          const sds = track ? sdsStateAt(track.transitions, t) : null;
          const metric = track ? ctx.metrics.get(track.tag) : undefined;
          const lit = ctx.departures.has(e.kks) && ctx.departures.get(e.kks)! <= t;
          const roles = [e.kks === ctx.rootKks ? "root" : "", e.kks === ctx.alarmKks ? "alarm" : "", ctx.path.includes(e.kks) ? "path" : ""].join(" ");
          const flow = ctx.flows.find((f) => f.from === e.kks && f.bottleneck);
          return (
            <button key={e.kks} className={`eq-box lv-${lv} ${roles} ${e.kks === selected ? "selected" : ""} ${lit ? "lit" : ""}`} style={pct(e.bbox as Box)} onClick={() => onSelect(e.kks)} title={`${e.name} · ${e.place}`}>
              {depth !== undefined && <span className="depth">D {depth > 0 ? `+${depth}` : depth}</span>}
              {track && sds && (
                <span className={`sds-chip ${(e.bbox as Box)[0] > 0.55 ? "flip" : ""}`} style={{ borderColor: SDS_COLOR[sds] ?? "#59606e" }} title={`${track.tag} SDS 상태 전이: ${track.transitions.map(([, c]) => c).join(" → ")}`}>
                  <b style={{ color: SDS_COLOR[sds] }}>{sds}</b>
                  {metric && (
                    <i>
                      {" "}
                      {shortTag(track.tag)} {fixed(metric.slope_per_sec_milli / 1000, 2)}/s
                    </i>
                  )}
                  {tl && <Spark tl={tl} tag={track.tag} from={sparkFrom} to={sparkTo} t={t} color={SDS_COLOR[sds] ?? "#4fd1c5"} />}
                </span>
              )}
              {e.kks === ctx.rootKks && <span className="badge root">🔥 근본원인</span>}
              {e.kks === ctx.alarmKks && <span className="badge alarm">⚠️ 경보</span>}
              <span className="eq-values">
                {sensorsOf(plant, e.kks)
                  .slice(0, 2)
                  .map((s) => (
                    <i key={s.tag}>
                      {shortTag(s.tag)} {fmt(tl ? valueAt(tl, s.tag, t) : s.normal)} {s.unit}
                    </i>
                  ))}
              </span>
              {mining && flow && (
                <span className="pin">
                  🚨 {flow.label} {((flow.ratio - 1) * 100).toFixed(1)}%{flow.lag_secs !== null ? ` · Δt +${flow.lag_secs.toFixed(1)}s` : ""}
                </span>
              )}
            </button>
          );
        })}
        {opcs.map((c) => {
          const out = c.from_drawing === drawing;
          const box = (out ? c.out_bbox : c.in_bbox) as Box;
          const target = out ? c.to_drawing : c.from_drawing;
          const other = plant.equipment.find((e) => e.kks === (out ? c.to : c.from));
          const lag = ctx.edgeLags.get(`${c.from}>${c.to}`);
          const arrive = ctx.departures.get(c.to);
          return (
            <button
              key={c.id + (out ? "o" : "i")}
              className={`opc ${ctx.path.includes(c.from) && ctx.path.includes(c.to) ? "path" : ""}`}
              style={pct(box)}
              title={`${c.id} · ${c.label} · 누르면 ${target} 로 이동`}
              aria-label={out ? `점프 ${target}` : `${target} 인입`}
              onClick={() => {
                setDrawing(target);
                if (other) onSelect(other.kks);
              }}
            >
              <span className="opc-icon">
                {out ? "⤴" : "⤵"}
                {lag && ` ${c.id}`}
              </span>
              {lag && (
                <span className="opc-osd">
                  {out ? "하류" : "상류"} 전파 Δt {lag.lag >= 0 ? "+" : ""}
                  {lag.lag.toFixed(1)}s {lag.ok ? "✓" : "✗"}
                  {arrive !== undefined ? ` · 도착 ${signedSecs(arrive, ctx.origin)}` : ""}
                </span>
              )}
            </button>
          );
        })}
      </div>
        </>
      )}
    </div>
  );
}
