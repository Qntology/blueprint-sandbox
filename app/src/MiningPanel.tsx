import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { mmss, sensorsOf, signedSecs } from "./plant";
import type { Bottleneck, Detail, Lane, Plant } from "./types";
import { WaveChart } from "./WaveChart";

/** 우측 병목(Bottleneck) & 신호 분석기 (기획 Part 8-1B 2절) */
export function MiningPanel({ plant, detail, selected, onSelect, onPick, t }: { plant: Plant | null; detail: Detail | null; selected: string | null; onSelect: (kks: string) => void; onPick: (id: string) => void; t: number | null }) {
  const [bottlenecks, setBottlenecks] = useState<Bottleneck[]>([]);
  useEffect(() => {
    const load = () => invoke<{ bottlenecks: Bottleneck[] }>("mining_overview").then((m) => setBottlenecks(m.bottlenecks));
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [detail?.id]);
  const sensor = plant && selected ? sensorsOf(plant, selected)[0] : undefined;
  const origin = detail?.analysis.substantive?.causal?.root?.at_ms ?? detail?.analysis.chain[0]?.at_ms ?? detail?.timeline.now_ms ?? 0;
  return (
    <section className="side mining">
      <h4>🚨 감지된 병목 ({bottlenecks.length}건)</h4>
      {bottlenecks.length === 0 && <p className="muted">진행 중인 병목이 없습니다.</p>}
      {bottlenecks.map((b, i) => (
        <button
          key={i}
          className={`bottleneck ${b.kind}`}
          onClick={() => {
            onPick(b.incident);
            if (b.kks) onSelect(b.kks);
          }}
        >
          <strong>
            {i + 1}. [{b.title}]
          </strong>
          {b.lines.map((l, k) => (
            <span key={k}>• {l}</span>
          ))}
          {b.waiting_secs !== null && <em>⏳ {mmss(b.waiting_secs)}</em>}
        </button>
      ))}
      {detail && sensor && plant && (
        <>
          <h4>[선택 노드 신호 파형] {sensor.label}</h4>
          <WaveChart
            timeline={detail.timeline}
            traces={[{ tag: sensor.tag, title: `${sensor.tag} (${sensor.unit})`, color: "#4fd1c5" }]}
            sensors={plant.sensors}
            from={Math.max(detail.timeline.t0_ms, origin - 10_000)}
            to={Math.min(detail.timeline.now_ms + 10_000, detail.timeline.t0_ms + 150_000)}
            markers={[{ at_ms: origin, label: "t=0", color: "#ffffff" }]}
            cursor={t}
            height={110}
          />
        </>
      )}
      {detail && (
        <>
          <h4>[신호 전파 체인 (Event Trace)]</h4>
          <ul className="facts">
            {detail.analysis.chain.map((ev, i) => (
              <li key={i}>
                • {signedSecs(ev.at_ms, origin)} : {ev.tag} {ev.label} {ev.kind === "limit" ? "한계 돌파" : ev.direction === "Up" ? "상승" : "급락"}
              </li>
            ))}
            {detail.analysis.stages
              .filter((s) => s.stage >= 4 && s.ms > 0)
              .map((s) => (
                <li key={s.stage}>
                  • {s.name}: {s.summary} ({(s.ms / 1000).toFixed(2)}초)
                </li>
              ))}
          </ul>
        </>
      )}
    </section>
  );
}

/** 하단 5-Gate & AI 의사결정 프로세스 마이닝 레인 */
export function MiningLanes({ refreshKey }: { refreshKey: number }) {
  const [lanes, setLanes] = useState<Lane[]>([]);
  useEffect(() => {
    const load = () => invoke<{ lanes: Lane[] }>("mining_overview").then((m) => setLanes(m.lanes));
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [refreshKey]);
  return (
    <div className="lanes">
      <span className="lanes-title">[5-Gate & AI 의사결정 프로세스 마이닝 레인]</span>
      {lanes.map((l, i) => (
        <span key={l.key} className={`lane ${l.max_wait_secs > 0 && l.count > 0 ? "hold" : ""}`}>
          {i > 0 && <b className="arrow">══►</b>}[{l.label} ({l.count}건)]
          {l.mean_ms > 0 && <i>{l.mean_ms < 1000 ? `${l.mean_ms.toFixed(1)}ms` : `${(l.mean_ms / 1000).toFixed(2)}s`}</i>}
          {l.max_wait_secs > 0 && l.count > 0 && <em>🚨 대기 {mmss(l.max_wait_secs)}</em>}
        </span>
      ))}
    </div>
  );
}
