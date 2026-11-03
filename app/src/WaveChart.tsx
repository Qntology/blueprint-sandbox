import { fmt } from "./plant";
import type { Sensor, Timeline } from "./types";

export interface Trace {
  tag: string;
  title: string;
  color: string;
  /** 다중 채널이면 채널 이름 (A · B · C) */
  channel?: string;
}

export interface Marker {
  at_ms: number;
  label: string;
  color?: string;
}

const MARKER_ROW = 11;
const MAX_MARKER_ROWS = 8;

/** 글자 폭 어림 (10px 글꼴: 한글 · 기호는 넓고 숫자 · 영문은 좁음) */
function textWidth(text: string): number {
  let w = 0;
  for (const ch of text) w += ch.charCodeAt(0) > 0x2000 ? 10 : 5.8;
  return w;
}

/** 표지 글을 위 띠에 줄을 나눠 놓습니다. 앞 글과 겹치지 않는 첫 줄에 넣고, 줄이 모자라면 마지막 줄에 겹쳐 둡니다. */
function placeMarkers(items: { x: number; width: number; right: boolean }[]): number[] {
  const ends: number[] = [];
  return items.map(({ x, width, right }) => {
    const start = right ? x - 3 - width : x + 3;
    const end = start + width + 6;
    let row = ends.findIndex((e) => e <= start);
    if (row < 0) {
      row = ends.length < MAX_MARKER_ROWS ? ends.length : MAX_MARKER_ROWS - 1;
      if (row === ends.length) ends.push(end);
    }
    ends[row] = Math.max(ends[row] ?? 0, end);
    return row;
  });
}

/** 상 · 하류 다중 설비 시계열 파형 동기화 차트 (기획 Part 8-3 5절). 한 줄에 한 센서, 같은 시간축입니다. */
export function WaveChart({ timeline, traces, sensors, from, to, markers, cursor, height = 92 }: { timeline: Timeline; traces: Trace[]; sensors: Sensor[]; from: number; to: number; markers: Marker[]; cursor?: number | null; height?: number }) {
  const W = 760;
  const left = 70;
  const right = 10;
  const x = (ms: number) => left + ((ms - from) / Math.max(to - from, 1)) * (W - left - right);
  const ticks: number[] = [];
  const step = (to - from) / 6;
  for (let i = 0; i <= 6; i++) ticks.push(from + i * step);

  const shown = markers.filter((m) => m.at_ms >= from && m.at_ms <= to).sort((a, b) => a.at_ms - b.at_ms);
  const rows = placeMarkers(
    shown.map((m) => {
      const width = textWidth(m.label);
      return { x: x(m.at_ms), width, right: x(m.at_ms) + 3 + width > W - right };
    }),
  );
  const band = shown.length ? (Math.max(...rows) + 1) * MARKER_ROW + 6 : 4;
  const totalH = band + traces.length * height + 18;

  return (
    <svg className="wave" viewBox={`0 0 ${W} ${totalH}`} preserveAspectRatio="xMinYMin meet">
      {traces.map((tr, row) => {
        const values = tr.channel ? timeline.channels[tr.tag]?.[tr.channel] : timeline.series[tr.tag];
        if (!values) return null;
        const pts: [number, number][] = [];
        values.forEach((v, i) => {
          const t = timeline.t0_ms + i * timeline.period_ms;
          if (t >= from && t <= to) pts.push([t, v]);
        });
        const s = sensors.find((x) => x.tag === tr.tag);
        const lims = [s?.alarm_high, s?.alarm_low, s?.trip_high, s?.trip_low].filter((v): v is number => v !== null && v !== undefined);
        const all = pts.map((p) => p[1]);
        let lo = Math.min(...all);
        let hi = Math.max(...all);
        for (const l of lims) {
          if (l > lo - (hi - lo) * 0.6 && l < hi + (hi - lo) * 0.6) {
            lo = Math.min(lo, l);
            hi = Math.max(hi, l);
          }
        }
        if (hi - lo < 1e-6) {
          hi += 1;
          lo -= 1;
        }
        // 한 줄 = 제목 띠(16) + 그림 + 축(12). 제목이 눈금 · 선과 겹치지 않게 그림 위에 따로 둡니다.
        const rowTop = band + row * height;
        const plotTop = rowTop + 18;
        const plotBottom = rowTop + height - 12;
        const y = (v: number) => plotTop + (1 - (v - lo) / (hi - lo)) * (plotBottom - plotTop);
        return (
          <g key={`${tr.tag}-${tr.channel ?? ""}`}>
            <text x={4} y={rowTop + 11} className="wave-title" fill={tr.color}>
              {tr.title}
            </text>
            <line x1={left} x2={W - right} y1={rowTop + height - 8} y2={rowTop + height - 8} className="wave-axis" />
            <text x={left - 6} y={y(hi) + 4} className="wave-tick" textAnchor="end">
              {fmt(hi)}
            </text>
            <text x={left - 6} y={y(lo) + 4} className="wave-tick" textAnchor="end">
              {fmt(lo)}
            </text>
            {lims
              .filter((l) => l >= lo && l <= hi)
              .map((l) => (
                <line key={l} x1={left} x2={W - right} y1={y(l)} y2={y(l)} className="wave-limit" />
              ))}
            <polyline points={pts.map(([t, v]) => `${x(t)},${y(v)}`).join(" ")} fill="none" stroke={tr.color} strokeWidth={2} />
          </g>
        );
      })}
      {shown.map((m, i) => {
        const mx = x(m.at_ms);
        const width = textWidth(m.label);
        const toLeft = mx + 3 + width > W - right;
        return (
          <g key={i}>
            <line x1={mx} x2={mx} y1={band - 4} y2={totalH - 20} className="wave-marker" style={{ stroke: m.color ?? "#ff6b3d" }} />
            <text x={toLeft ? mx - 3 : mx + 3} y={10 + rows[i] * MARKER_ROW} className="wave-marker-text" textAnchor={toLeft ? "end" : "start"} fill={m.color ?? "#ff9b70"}>
              {m.label}
            </text>
          </g>
        );
      })}
      {cursor !== null && cursor !== undefined && cursor >= from && cursor <= to && <line x1={x(cursor)} x2={x(cursor)} y1={band - 4} y2={totalH - 18} className="wave-cursor" />}
      {ticks.map((t, i) => (
        <text key={i} x={x(t)} y={totalH - 4} className="wave-tick" textAnchor="middle">
          {`${((t - (markers[0]?.at_ms ?? from)) / 1000).toFixed(0)}s`}
        </text>
      ))}
    </svg>
  );
}
