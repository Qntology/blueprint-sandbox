import { useMemo, useState } from "react";
import { useTerms } from "./domain";
import { valueAt } from "./plant";
import type { Detail, EdgeFlow, Metrics, Plant, SdsTrack } from "./types";
import { DrawingView } from "./views/DrawingView";
import { MermaidView } from "./views/MermaidView";
import { MindMapView } from "./views/MindMapView";

/** 세 뷰가 함께 쓰는 사건 맥락: 역할 · 시각 t 의 흐름 · 변곡 시각 */
export interface Context {
  plant: Plant;
  detail: Detail | null;
  selected: string | null;
  onSelect: (kks: string) => void;
  mining: boolean;
  t: number;
  incident: string | null;
  alarmKks: string | null;
  rootKks: string | null;
  path: string[];
  flows: EdgeFlow[];
  /** 설비별 첫 변곡 시각 (재생 때 불이 켜지는 순간) */
  departures: Map<string, number>;
  /** 설비별 SDS 신호 상태 전이 (개선안 Part 2: 도면 배지 · 마인드맵 링) */
  tracks: Map<string, SdsTrack[]>;
  /** 센서별 60초 링버퍼 지표 (기울기 배지) */
  metrics: Map<string, Metrics>;
  /** 사건의 t = 0 (근본 원인 변곡) */
  origin: number;
  /** 원인 경로 간선의 관측 Δt 와 배관 허용 범위 (`from>to`) */
  edgeLags: Map<string, EdgeLag>;
}

/** 변곡이 있는 두 설비 사이 구간의 관측 시차. 구간의 간선들이 같은 값을 나눠 가집니다 (`hops` 간선 수). */
export interface EdgeLag {
  lag: number;
  range: [number, number];
  ok: boolean;
  hops: number;
}

type Tab = "drawing" | "mermaid" | "mindmap";

/** 3-Way 인터랙티브 관제 뷰 (기획 Part 8 6.1절). 어느 뷰에서 설비를 클릭하든 `onSelect` 하나로 세 곳이 함께 움직입니다. */
export function PlantTopologyView(props: {
  plant: Plant;
  detail: Detail | null;
  selected: string | null;
  onSelect: (kks: string) => void;
  mining: boolean;
  onMining: (on: boolean) => void;
  t: number;
  incident: string | null;
}) {
  const terms = useTerms();
  const [tab, setTab] = useState<Tab>("drawing");
  const { plant, detail, t } = props;
  const TABS: [Tab, string][] = [
    ["drawing", `① ${terms.drawing} 도면 뷰`],
    ["mermaid", "② Mermaid 계통 뷰"],
    ["mindmap", "③ 옵시디언 마인드맵 뷰"],
  ];
  const root = detail?.analysis.substantive?.causal?.root ?? null;

  // 시각 t 의 간선 흐름 (재생 슬라이더를 움직이면 다시 계산합니다)
  const flows = useMemo<EdgeFlow[]>(() => {
    const tl = detail?.timeline ?? null;
    return plant.edges.map((e) => {
      const base = detail?.flows.find((f) => f.from === e.from && f.to === e.to);
      const sensor = e.flow_tag ? plant.sensors.find((s) => s.tag === e.flow_tag) : undefined;
      const current = e.flow_tag && tl ? valueAt(tl, e.flow_tag, t) : null;
      const nominal = sensor?.normal ?? null;
      const ratio = current !== null && nominal ? current / nominal : 1;
      return {
        from: e.from,
        to: e.to,
        medium: e.medium,
        label: e.label,
        flow_tag: e.flow_tag ?? null,
        nominal,
        current,
        unit: sensor?.unit ?? null,
        ratio,
        bottleneck: ratio < 0.8,
        lag_secs: base?.lag_secs ?? null,
        on_path: base?.on_path ?? false,
      };
    });
  }, [plant, detail, t]);

  const departures = useMemo(() => {
    const m = new Map<string, number>();
    for (const ev of detail?.analysis.chain ?? []) {
      if (ev.kind === "departure" && !m.has(ev.kks)) m.set(ev.kks, ev.at_ms);
    }
    return m;
  }, [detail]);

  const tracks = useMemo(() => {
    const m = new Map<string, SdsTrack[]>();
    for (const tr of detail?.signals ?? []) m.set(tr.kks, [...(m.get(tr.kks) ?? []), tr]);
    return m;
  }, [detail]);
  const metrics = useMemo(() => new Map((detail?.analysis.metrics ?? []).map((m) => [m.sensor_tag, m])), [detail]);
  const origin = root?.at_ms ?? detail?.analysis.chain[0]?.at_ms ?? detail?.analysis.sds.center_ms ?? 0;
  // 원인 경로의 첫 변곡 시차와 배관 허용 지연 (개선안 2-1: 배관 위 `Δt +4.3s (허용 1.5~8.0s ✓)`). 계측이 없는 설비(헤더 등)는
  // 변곡이 없으니, 변곡이 있는 두 설비 사이 구간의 시차를 그 구간 간선 모두에 걸고 허용 범위는 간선 지연을 더합니다.
  const edgeLags = useMemo(() => {
    const m = new Map<string, EdgeLag>();
    const path = root?.path ?? [];
    let anchor = -1;
    for (let i = 0; i < path.length; i++) {
      const ti = departures.get(path[i]);
      if (ti === undefined) continue;
      if (anchor >= 0) {
        const keys = path.slice(anchor, i).map((a, k) => `${a}>${path[anchor + k + 1]}`);
        const edges = keys.map((key) => plant.edges.find((x) => `${x.from}>${x.to}` === key));
        if (edges.every((e) => e)) {
          const range: [number, number] = [edges.reduce((s, e) => s + e!.lag[0], 0), edges.reduce((s, e) => s + e!.lag[1], 0)];
          const lag = (ti - departures.get(path[anchor])!) / 1000;
          const v: EdgeLag = { lag, range, ok: lag >= range[0] - 0.5 && lag <= range[1] + 0.5, hops: i - anchor };
          for (const key of keys) m.set(key, v);
        }
      }
      anchor = i;
    }
    return m;
  }, [plant, root, departures]);

  const ctx: Context = {
    ...props,
    alarmKks: detail?.analysis.route.kks ?? null,
    rootKks: root?.kks ?? null,
    path: root?.path ?? [],
    flows,
    departures,
    tracks,
    metrics,
    origin,
    edgeLags,
  };

  return (
    <div className="topology">
      <div className="tabs">
        {TABS.map(([key, label]) => (
          <button key={key} className={tab === key ? "tab active" : "tab"} onClick={() => setTab(key)}>
            {tab === key ? "● " : "○ "}
            {label}
          </button>
        ))}
        <span className="grow" />
        <label className={`mining-toggle ${props.mining ? "on" : ""}`}>
          <input type="checkbox" checked={props.mining} onChange={(e) => props.onMining(e.target.checked)} />⚡ 흐름 · 병목 오버레이 {props.mining ? "ON" : "OFF"}
        </label>
      </div>
      <div className="canvas-area">
        {tab === "drawing" && <DrawingView ctx={ctx} />}
        {tab === "mermaid" && <MermaidView ctx={ctx} />}
        {tab === "mindmap" && <MindMapView ctx={ctx} />}
      </div>
    </div>
  );
}
