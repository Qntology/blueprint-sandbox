// 계통 계산 도우미. 코어의 graph.rs (홉 뎁스) · timeseries.rs (시각 값) 과 같은 규칙입니다.
import type { Edge, Plant, Sensor, Timeline } from "./types";

/** 소수 자리 고정. 반올림하면 0 이 되는 음수를 "-0.0" 으로 쓰지 않습니다. */
export function fixed(v: number, digits: number): string {
  const text = v.toFixed(digits);
  return /^-0(\.0*)?$/.test(text) ? text.slice(1) : text;
}

/** 값 표시 (도면 · 코어와 같은 꼴): 1000 이상 천 단위 쉼표, 1 미만 소수 둘째 자리 */
export function fmt(v: number | null | undefined): string {
  if (v === null || v === undefined || Number.isNaN(v)) return "-";
  if (Math.abs(v) >= 1000) return v.toLocaleString("ko-KR", { minimumFractionDigits: 1, maximumFractionDigits: 1 });
  if (Math.abs(v) >= 1 || v === 0) return fixed(v, 1);
  return fixed(v, 2);
}

export function short(kks: string): string {
  return kks.slice(0, 9);
}

/** 중심 설비에서 방향을 지켜 maxHops 안에 닿는 설비의 부호 있는 홉 (음수 상류, 양수 하류) */
export function hopDepths(edges: Edge[], center: string, maxHops: number): Map<string, number> {
  const bfs = (up: boolean) => {
    const seen = new Map<string, number>([[center, 0]]);
    const queue: string[] = [center];
    while (queue.length) {
      const cur = queue.shift()!;
      const d = seen.get(cur)!;
      if (d >= maxHops) continue;
      for (const e of edges) {
        const next = up ? (e.to === cur ? e.from : null) : e.from === cur ? e.to : null;
        if (next && !seen.has(next)) {
          seen.set(next, d + 1);
          queue.push(next);
        }
      }
    }
    seen.delete(center);
    return seen;
  };
  const out = new Map<string, number>([[center, 0]]);
  for (const [k, d] of bfs(true)) out.set(k, -d);
  for (const [k, d] of bfs(false)) {
    const cur = out.get(k);
    if (cur === undefined || Math.abs(cur) > d) out.set(k, d);
  }
  return out;
}

/** 방향을 보지 않은 홉 거리 */
export function distances(edges: Edge[], center: string): Map<string, number> {
  const seen = new Map<string, number>([[center, 0]]);
  const queue = [center];
  while (queue.length) {
    const cur = queue.shift()!;
    for (const e of edges) {
      const next = e.from === cur ? e.to : e.to === cur ? e.from : null;
      if (next && !seen.has(next)) {
        seen.set(next, seen.get(cur)! + 1);
        queue.push(next);
      }
    }
  }
  return seen;
}

/** 시각 t 의 값 (표본 사이 선형 보간, 밖이면 끝값) */
export function valueAt(tl: Timeline | null, tag: string, t: number): number | null {
  const v = tl?.series[tag];
  if (!tl || !v || v.length === 0) return null;
  const pos = (t - tl.t0_ms) / tl.period_ms;
  if (pos <= 0) return v[0];
  if (pos >= v.length - 1) return v[v.length - 1];
  const i = Math.floor(pos);
  return v[i] + (v[i + 1] - v[i]) * (pos - i);
}

export type Level = "normal" | "warn" | "alarm" | "trip";

/** 기준치 대비 상태 */
export function level(s: Sensor, v: number | null): Level {
  if (v === null) return "normal";
  if ((s.trip_high !== null && v >= s.trip_high) || (s.trip_low !== null && v <= s.trip_low)) return "trip";
  if ((s.alarm_high !== null && v >= s.alarm_high) || (s.alarm_low !== null && v <= s.alarm_low)) return "alarm";
  const dev = Math.abs(v - s.normal) / Math.max(Math.abs(s.normal), 1e-6);
  return dev > 0.03 ? "warn" : "normal";
}

const RANK: Record<Level, number> = { normal: 0, warn: 1, alarm: 2, trip: 3 };

/** 설비 상태 = 센서 중 가장 나쁜 것 */
export function equipmentLevel(plant: Plant, kks: string, tl: Timeline | null, t: number): Level {
  let worst: Level = "normal";
  for (const s of plant.sensors.filter((x) => x.kks === kks)) {
    const l = level(s, tl ? valueAt(tl, s.tag, t) : s.normal);
    if (RANK[l] > RANK[worst]) worst = l;
  }
  return worst;
}

export function sensorsOf(plant: Plant, kks: string): Sensor[] {
  return plant.sensors.filter((s) => s.kks === kks);
}

export function clock(ms: number): string {
  const d = new Date(ms);
  return d.toLocaleTimeString("ko-KR", { hour12: false });
}

export function signedSecs(ms: number, base: number): string {
  const text = fixed((ms - base) / 1000, 1);
  return `${text.startsWith("-") ? "" : "+"}${text}s`;
}

export function mmss(secs: number | null | undefined): string {
  if (secs === null || secs === undefined) return "-";
  const m = Math.floor(secs / 60);
  return `${String(m).padStart(2, "0")}m ${String(secs % 60).padStart(2, "0")}s`;
}

export const MEDIUM_COLOR: Record<string, string> = {
  power: "#c9a227",
  condensate: "#4aa3df",
  feedwater: "#2fb4c9",
  steam: "#e07b53",
  shaft: "#9aa3b5",
  oil: "#b48ead",
  seawater: "#3c7dd9",
  fuel: "#8d6e63",
  air: "#88c0d0",
  gas: "#a3a3a3",
  draft: "#7fb069",
};

export const STAGE_LABEL: Record<string, string> = {
  Done: "정상완료",
  Skipped: "건너뜀",
  Corrected: "교정완료",
  Alert: "경보발령",
  Waiting: "대기",
  Blocked: "차단",
};
