import { useMemo, useState } from "react";
import { valueAt } from "./plant";
import type { Detail, EdgeFlow, Plant } from "./types";
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
}

type Tab = "drawing" | "mermaid" | "mindmap";

const TABS: [Tab, string][] = [
  ["drawing", "① P&ID 도면 뷰"],
  ["mermaid", "② Mermaid 계통 뷰"],
  ["mindmap", "③ 옵시디언 마인드맵 뷰"],
];

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
  const [tab, setTab] = useState<Tab>("drawing");
  const { plant, detail, t } = props;
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

  const ctx: Context = {
    ...props,
    alarmKks: detail?.analysis.route.kks ?? null,
    rootKks: root?.kks ?? null,
    path: root?.path ?? [],
    flows,
    departures,
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
          <input type="checkbox" checked={props.mining} onChange={(e) => props.onMining(e.target.checked)} />⚡ 프로세스 마이닝 오버레이 {props.mining ? "ON" : "OFF"}
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
