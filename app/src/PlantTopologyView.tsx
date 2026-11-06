import { useCallback, useEffect, useMemo, useRef, useState, type RefObject } from "react";
import { CctvOverlay, type CardTarget, type SearchMark } from "./cctv/CctvOverlay";
import { domLocator, type Insets, type Locator, type Size } from "./cctv/stage";
import { valueAt } from "./plant";
import type { Detail, EdgeFlow, Metrics, Plant, SdsTrack } from "./types";
import { DrawingView } from "./views/DrawingView";
import { MermaidView } from "./views/MermaidView";
import { MindMapView } from "./views/MindMapView";

export type Tab = "drawing" | "mermaid" | "mindmap";

/** 세 뷰가 함께 쓰는 사건 맥락: 역할 · 시각 t 의 흐름 · 변곡 시각, 그리고 풀스크린 무대의 크기 · 빈 자리 */
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
  /** 무대 크기와 떠 있는 판(코너 알약 · 결재 HUD)을 뺀 빈 자리. 도면은 빈 자리에 맞춰 그립니다 (2차 개선안: 풀스크린 무대) */
  stage: Size;
  inset: Insets;
  /** CCTV 말풍선이 붙은 설비. 도면의 작은 SDS 칩은 말풍선이 없는 설비에만 붙입니다 */
  carded: Set<string>;
  /** 검색 적중 (설비 → 순위 · 왜) */
  marks: Map<string, SearchMark>;
  /** 도면 어둡게 (관제실 톤) */
  dark: boolean;
  onDark: (on: boolean) => void;
  /** 좌상단 도구줄 자리: 뷰마다 도면 선택 · 전체 계통 · A0 정보를 여기에 그립니다 */
  bar: HTMLElement | null;
  /** 지금 보는 마스터 도면 (`A0:…` 면 A0 종합) */
  drawing: string;
  setDrawing: (id: string) => void;
  /** 노드 자리를 DOM 이 아닌 곳에서 읽는 뷰(마인드맵)가 말풍선 자리 찾기를 바꿉니다 */
  setLocator: (l: Locator | null) => void;
}

/** 변곡이 있는 두 설비 사이 구간의 관측 시차. 구간의 간선들이 같은 값을 나눠 가집니다 (`hops` 간선 수). */
export interface EdgeLag {
  lag: number;
  range: [number, number];
  ok: boolean;
  hops: number;
}

/** 3-Way 인터랙티브 관제 뷰 (기획 Part 8 6.1절 · 2차 개선안 풀스크린 무대). 고른 뷰 하나가 무대 전체에 깔리고,
 *  그 위에 근본 원인 · 경보 · 선택 · 검색 1위 설비의 CCTV 말풍선이 지시선으로 떠 있습니다. 어느 뷰에서 설비를 누르든 `onSelect` 하나로 함께 움직입니다. */
export function PlantTopologyView(props: {
  plant: Plant;
  detail: Detail | null;
  selected: string | null;
  onSelect: (kks: string) => void;
  mining: boolean;
  t: number;
  incident: string | null;
  tab: Tab;
  stageRef: RefObject<HTMLDivElement>;
  stage: Size;
  inset: Insets;
  bar: HTMLElement | null;
  marks: Map<string, SearchMark>;
  dark: boolean;
  onDark: (on: boolean) => void;
  onZoom: (kks: string) => void;
  onInterlock: (() => void) | null;
}) {
  const { plant, detail, t, selected, tab, stageRef } = props;
  const root = detail?.analysis.substantive?.causal?.root ?? null;
  const [drawing, setDrawing] = useState(() => plant.equipment.find((e) => e.kks === selected)?.drawing ?? plant.drawings[0]?.id ?? "");

  // 도메인을 바꾸면 도면 목록이 바뀝니다. 다른 뷰 · 검색에서 다른 도면의 설비를 고르면 그 도면으로 넘어갑니다 (A0 종합에서는 그대로).
  useEffect(() => {
    if (!plant.drawings.some((d) => d.id === drawing) && !plant.a0.some((a) => `A0:${a.id}` === drawing)) setDrawing(plant.drawings[0]?.id ?? "");
  }, [plant]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    const d = plant.equipment.find((e) => e.kks === selected)?.drawing;
    if (d && d !== drawing && !drawing.startsWith("A0:")) setDrawing(d);
  }, [selected, plant]); // eslint-disable-line react-hooks/exhaustive-deps

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

  const alarmKks = detail?.analysis.route.kks ?? null;
  const rootKks = root?.kks ?? null;
  const top = [...props.marks.entries()].find(([, m]) => m.rank === 1)?.[0] ?? null;
  // 말풍선 순서 = 좋은 자리를 먼저 차지하는 순서: 선택 설비 → 근본 원인 → 경보 → 검색 1위
  const targets = useMemo<CardTarget[]>(() => {
    const list: CardTarget[] = [];
    const add = (kks: string | null, role: CardTarget["roles"][number]) => {
      if (!kks || !plant.equipment.some((e) => e.kks === kks)) return;
      const hit = list.find((x) => x.kks === kks);
      if (hit) hit.roles.push(role);
      else list.push({ kks, roles: [role] });
    };
    add(selected, "selected");
    add(rootKks, "root");
    add(alarmKks, "alarm");
    add(top, "hit");
    return list;
  }, [plant, selected, rootKks, alarmKks, top]);
  const carded = useMemo(() => new Set(targets.map((x) => x.kks)), [targets]);

  // 말풍선 자리 찾기: 기본은 DOM 의 `data-kks` (도면 · A0 · Mermaid), 마인드맵은 캔버스 노드 좌표를 줍니다.
  const custom = useRef<Locator | null>(null);
  const dom = useRef<Locator | null>(null);
  const plantRef = useRef(plant);
  plantRef.current = plant;
  const locator = useCallback(() => {
    if (custom.current) return custom.current;
    const st = stageRef.current;
    if (!st) return null;
    dom.current ??= domLocator(st, (k) => plantRef.current.equipment.find((e) => e.kks === k)?.drawing);
    return dom.current;
  }, [stageRef]);
  const setLocator = useCallback((l: Locator | null) => {
    custom.current = l;
  }, []);
  // 도면 뷰에서 다른 도면의 설비는 점프 커넥터를 가리킵니다
  const remoteOf = useCallback(
    (kks: string) => {
      if (tab !== "drawing" || drawing.startsWith("A0:")) return null;
      const d = plant.equipment.find((e) => e.kks === kks)?.drawing;
      return d && d !== drawing ? d : null;
    },
    [tab, drawing, plant],
  );

  const ctx: Context = {
    plant,
    detail,
    selected,
    onSelect: props.onSelect,
    mining: props.mining,
    t,
    incident: props.incident,
    alarmKks,
    rootKks,
    path: root?.path ?? [],
    flows,
    departures,
    tracks,
    metrics,
    origin,
    edgeLags,
    stage: props.stage,
    inset: props.inset,
    carded,
    marks: props.marks,
    dark: props.dark,
    onDark: props.onDark,
    bar: props.bar,
    drawing,
    setDrawing,
    setLocator,
  };

  return (
    <>
      {/* 도메인을 바꾸면 뷰를 새로 붙입니다 (마인드맵 노드 · 도면 확대가 앞 도메인 것으로 남지 않게) */}
      <div className="canvas-full" key={plant.domain}>
        {tab === "drawing" && <DrawingView ctx={ctx} />}
        {tab === "mermaid" && <MermaidView ctx={ctx} />}
        {tab === "mindmap" && <MindMapView ctx={ctx} />}
      </div>
      <CctvOverlay
        stage={stageRef}
        locator={locator}
        targets={targets}
        plant={plant}
        detail={detail}
        t={t}
        tracks={tracks}
        metrics={metrics}
        marks={props.marks}
        remoteOf={remoteOf}
        dark={props.dark}
        onSelect={props.onSelect}
        onZoom={props.onZoom}
        onInterlock={props.onInterlock}
      />
    </>
  );
}
