import { invoke } from "@tauri-apps/api/core";
import mermaid from "mermaid";
import { useEffect, useRef, useState } from "react";
import type { Context } from "../PlantTopologyView";

let ready = false;
let seq = 0;

function init() {
  if (ready) return;
  mermaid.initialize({ startOnLoad: false, theme: "dark", securityLevel: "loose", flowchart: { htmlLabels: true, curve: "basis", nodeSpacing: 36, rankSpacing: 56 }, fontFamily: "Malgun Gothic, system-ui, sans-serif" });
  ready = true;
}

/** 뷰 ②: Mermaid 계통 다이어그램. 원본은 코어(`mermaid.rs`)가 만들고, 블록을 누르면 선택 설비가 바뀝니다. */
export function MermaidView({ ctx }: { ctx: Context }) {
  const host = useRef<HTMLDivElement>(null);
  const [whole, setWhole] = useState(false);
  const [source, setSource] = useState("");
  const [showSource, setShowSource] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    invoke<string>("mermaid_source", { input: { incident: ctx.incident, selected: ctx.selected, mining: ctx.mining, whole } })
      .then(setSource)
      .catch((e) => setError(String(e)));
  }, [ctx.incident, ctx.selected, ctx.mining, whole]);

  useEffect(() => {
    if (!source || !host.current) return;
    init();
    let alive = true;
    const id = `mmd-${++seq}`;
    mermaid
      .render(id, source)
      .then(({ svg }) => {
        if (!alive || !host.current) return;
        host.current.innerHTML = svg;
        host.current.querySelectorAll<SVGGElement>("g.node").forEach((g) => {
          const m = /N_([0-9A-Z]{12})/.exec(g.id);
          if (!m) return;
          g.style.cursor = "pointer";
          g.addEventListener("click", () => ctx.onSelect(m[1]));
        });
        setError(null);
      })
      .catch((e) => setError(`다이어그램을 그리지 못했습니다: ${e}`));
    return () => {
      alive = false;
    };
  }, [source]); // eslint-disable-line react-hooks/exhaustive-deps

  return (
    <div className="mermaid-view">
      <div className="mermaid-head">
        <span>[Mermaid 계통 다이어그램] 섹터(Lv2)별 서브그래프 · 상 · 하류 파급 경로 (노드 클릭 시 연동)</span>
        <span className="grow" />
        <label className="check">
          <input type="checkbox" checked={whole} onChange={(e) => setWhole(e.target.checked)} />
          전체 계통 (끄면 선택 설비 2홉 + 원인 경로)
        </label>
      </div>
      {error && <p className="error">{error}</p>}
      <div className="mermaid-host" ref={host} />
      <details className="mermaid-source" open={showSource} onToggle={(e) => setShowSource((e.target as HTMLDetailsElement).open)}>
        <summary>▼ 자동 생성된 Mermaid.js 원본 스크립트 펼치기/접기</summary>
        <pre>{source}</pre>
      </details>
    </div>
  );
}
