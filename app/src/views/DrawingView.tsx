import { invoke } from "@tauri-apps/api/core";
import { useEffect, useMemo, useState } from "react";
import type { Context } from "../PlantTopologyView";
import { equipmentLevel, fmt, hopDepths, MEDIUM_COLOR, sensorsOf, valueAt } from "../plant";

const images = new Map<string, string>();

type Box = [number, number, number, number];

const pct = (b: Box) => ({ left: `${b[0] * 100}%`, top: `${b[1] * 100}%`, width: `${(b[2] - b[0]) * 100}%`, height: `${(b[3] - b[1]) * 100}%` });

/** 뷰 ①: P&ID 도면 오버레이 캔버스. 설비 상자를 누르면 선택, 가장자리 점프 커넥터(OPC)를 누르면 연결된 도면으로 넘어갑니다. */
export function DrawingView({ ctx }: { ctx: Context }) {
  const { plant, detail, selected, onSelect, mining, t } = ctx;
  const initial = plant.equipment.find((e) => e.kks === selected)?.drawing ?? plant.drawings[0].id;
  const [drawing, setDrawing] = useState(initial);
  const [src, setSrc] = useState<string | null>(images.get(initial) ?? null);

  // 다른 뷰에서 다른 도면의 설비를 고르면 그 도면으로 넘어갑니다.
  useEffect(() => {
    const d = plant.equipment.find((e) => e.kks === selected)?.drawing;
    if (d && d !== drawing) setDrawing(d);
  }, [selected, plant]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
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
      </div>
      <div className="drawing-title">
        [표제란] 도면번호 <b>{info?.id}</b> ({info?.rev}) · {info?.title} · 발행 {info?.issued}
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
        {onSheet.map((e) => {
          const lv = tl ? equipmentLevel(plant, e.kks, tl, t) : "normal";
          const depth = depths.get(e.kks);
          const lit = ctx.departures.has(e.kks) && ctx.departures.get(e.kks)! <= t;
          const roles = [e.kks === ctx.rootKks ? "root" : "", e.kks === ctx.alarmKks ? "alarm" : "", ctx.path.includes(e.kks) ? "path" : ""].join(" ");
          const flow = ctx.flows.find((f) => f.from === e.kks && f.bottleneck);
          return (
            <button key={e.kks} className={`eq-box lv-${lv} ${roles} ${e.kks === selected ? "selected" : ""} ${lit ? "lit" : ""}`} style={pct(e.bbox as Box)} onClick={() => onSelect(e.kks)} title={`${e.name} · ${e.place}`}>
              {depth !== undefined && <span className="depth">D {depth > 0 ? `+${depth}` : depth}</span>}
              {e.kks === ctx.rootKks && <span className="badge root">🔥 근본원인</span>}
              {e.kks === ctx.alarmKks && <span className="badge alarm">⚠️ 경보</span>}
              <span className="eq-values">
                {sensorsOf(plant, e.kks)
                  .slice(0, 2)
                  .map((s) => (
                    <i key={s.tag}>
                      {s.tag.slice(7)} {fmt(tl ? valueAt(tl, s.tag, t) : s.normal)} {s.unit}
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
              <span className="opc-icon">{out ? "⤴" : "⤵"}</span>
            </button>
          );
        })}
      </div>
    </div>
  );
}
