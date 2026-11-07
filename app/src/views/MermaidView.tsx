import { invoke } from "../ipc";
import mermaid from "mermaid";
import { useEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { fitRect, usePanZoom, type Size } from "../cctv/stage";
import { ZoomCtl } from "../cctv/ZoomCtl";
import { useLang, useT } from "../i18n";
import type { Context } from "../PlantTopologyView";

let ready = false;
let seq = 0;

function init() {
  if (ready) return;
  mermaid.initialize({ startOnLoad: false, theme: "dark", securityLevel: "loose", flowchart: { htmlLabels: true, curve: "basis", nodeSpacing: 36, rankSpacing: 56 }, fontFamily: "Malgun Gothic, system-ui, sans-serif" });
  ready = true;
}

/** 작은 다이어그램(선택 설비 2홉)을 화면 가득 키우면 글자가 지나치게 커집니다: 원래 크기의 1.6 배까지만 */
const MAX_FIT = 1.6;

/** 뷰 ②: Mermaid 계통 다이어그램 (2차 개선안: 무대 전체로 넓혀 스크롤 없이 한눈에). 원본은 코어(`mermaid.rs`)가 만들고,
 *  블록을 누르면 선택 설비가 바뀝니다. 노드마다 `data-kks` 를 달아 CCTV 말풍선이 그 노드에서 지시선을 긋습니다. */
export function MermaidView({ ctx }: { ctx: Context }) {
  const host = useRef<HTMLDivElement>(null);
  const say = useT();
  const lang = useLang();
  const [whole, setWhole] = useState(false);
  const [source, setSource] = useState("");
  const [showSource, setShowSource] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [natural, setNatural] = useState<Size>({ w: 800, h: 500 });
  const fit = fitRect(ctx.stage, ctx.inset, natural.w / natural.h, natural, MAX_FIT);
  const pz = usePanZoom(fit, source);

  useEffect(() => {
    invoke<string>("mermaid_source", { input: { incident: ctx.incident, selected: ctx.selected, mining: ctx.mining, whole } })
      .then(setSource)
      .catch((e) => setError(String(e)));
  }, [ctx.incident, ctx.selected, ctx.mining, whole, lang]);

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
        const el = host.current.querySelector("svg");
        if (el) {
          // 판 크기는 viewBox 에서 읽고, 그림은 판에 맞춰 늘입니다 (확대 · 이동은 판에 겁니다)
          const vb = el.viewBox.baseVal;
          if (vb && vb.width > 0 && vb.height > 0) setNatural({ w: vb.width, h: vb.height });
          el.removeAttribute("style");
          el.setAttribute("width", "100%");
          el.setAttribute("height", "100%");
        }
        host.current.querySelectorAll<SVGGElement>("g.node").forEach((g) => {
          const m = /N_([0-9A-Z]{12})/.exec(g.id);
          if (!m) return;
          g.dataset.kks = m[1];
          g.style.cursor = "pointer";
          g.addEventListener("click", () => ctx.onSelect(m[1]));
        });
        setError(null);
      })
      .catch((e) => setError(say(`다이어그램을 그리지 못했습니다: ${e}`, `Could not draw the diagram: ${e}`)));
    return () => {
      alive = false;
    };
  }, [source]); // eslint-disable-line react-hooks/exhaustive-deps

  const bar = ctx.bar
    ? createPortal(
        <div className="view-bar ov-bar mermaid-head">
          <span className="bar-label">{say("🧭 Mermaid 계통", "🧭 Mermaid diagram")}</span>
          <span className="muted">{say("섹터(Lv2)별 서브그래프 · 상 · 하류 파급 경로 (노드 클릭 시 연동)", "Subgraph per sector (Lv2) · upstream/downstream propagation (click a node to sync)")}</span>
          <label className="chip">
            <input type="checkbox" checked={whole} onChange={(e) => setWhole(e.target.checked)} />
            {say("전체 계통 (끄면 선택 설비 2홉 + 원인 경로)", "Whole system (off: 2 hops around the selection + cause path)")}
          </label>
          <button className={`chip ${showSource ? "active" : ""}`} onClick={() => setShowSource((s) => !s)}>
            {showSource ? "▲" : "▼"} {say("Mermaid.js 원본", "Mermaid.js source")}
          </button>
          {error && <span className="error">{error}</span>}
          {showSource && <pre className="mermaid-source vp-glass">{source}</pre>}
        </div>,
        ctx.bar,
      )
    : null;

  return (
    <>
      {bar}
      <div className="sheet-viewport mermaid-view" ref={pz.bind}>
        <div className="mermaid-host" ref={host} style={pz.style} />
      </div>
      <ZoomCtl scale={pz.view.s} onZoom={(f) => pz.zoomAt(f)} onReset={pz.reset} />
    </>
  );
}
