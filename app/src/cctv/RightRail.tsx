import { useEffect, useRef, useState, type ReactNode } from "react";
import { useT } from "../i18n";
import { mmss } from "../plant";
import { SidePanel } from "../SidePanel";
import { InterlockPanel, QuickApproval } from "../StagePanel";
import type { Bottleneck, Detail, Plant, VerdictInfo } from "../types";

/** 접을 수 있는 HUD 한 칸 (아코디언) */
function Section({ title, open, onToggle, children, sectionRef }: { title: ReactNode; open: boolean; onToggle: () => void; children: ReactNode; sectionRef?: React.Ref<HTMLElement> }) {
  return (
    <section className={`hud-sec ${open ? "open" : ""}`} ref={sectionRef}>
      <button className="hud-sec-head" onClick={onToggle} aria-expanded={open}>
        <span>{title}</span>
        <i>{open ? "▾" : "▸"}</i>
      </button>
      {open && <div className="hud-sec-body">{children}</div>}
    </section>
  );
}

/** 우측 통합 병목 · 결재 HUD (개선안 3단계 2번 → 2차 개선안: noto-sandbox `.unit-hud` 처럼 도면 위에 뜬 반투명 유리 판).
 *  병목 배지 → 인터록 신호등 게이트 → 표준 조치 · 결재 → 선택 설비가 아코디언으로 이어지고, [접기 ▸] 로 제목 줄만 남깁니다.
 *  CCTV 말풍선의 [📋 인터록 이동] 은 `focus` 를 올려 인터록 칸을 펴고 그 자리로 옮깁니다. */
export function RightRail(props: {
  plant: Plant | null;
  detail: Detail | null;
  selected: string | null;
  onSelect: (kks: string) => void;
  onPick: (id: string) => void;
  t: number | null;
  catalog: Record<string, VerdictInfo>;
  supervisor: string;
  onDecided: (m: string) => void;
  onOpenApproval: () => void;
  bottlenecks: Bottleneck[];
  folded: boolean;
  onFold: (folded: boolean) => void;
  focus: number;
  ttf: string | null;
}) {
  const { plant, detail, selected, onSelect, onPick, t, catalog, supervisor, onDecided, onOpenApproval, bottlenecks, folded, onFold, focus, ttf } = props;
  const say = useT();
  const [open, setOpen] = useState({ bottleneck: true, approval: true, equipment: true });
  const [flash, setFlash] = useState(false);
  const interlock = useRef<HTMLElement>(null);
  const toggle = (k: keyof typeof open) => setOpen((o) => ({ ...o, [k]: !o[k] }));

  useEffect(() => {
    if (!focus) return;
    setOpen((o) => ({ ...o, approval: true }));
    setFlash(true);
    const raf = requestAnimationFrame(() => interlock.current?.scrollIntoView({ block: "start", behavior: "smooth" }));
    const timer = window.setTimeout(() => setFlash(false), 1600);
    return () => {
      cancelAnimationFrame(raf);
      window.clearTimeout(timer);
    };
  }, [focus]);

  const pending = detail?.analysis.interlock?.steps.find((s) => s.status !== "passed") ?? null;
  return (
    <aside className={`approval-hud vp-glass ov-avoid ${folded ? "folded" : ""}`} aria-label={say("통합 병목 · 인터록 · 결재", "Bottlenecks · interlock · approval")}>
      <header className="hud-head">
        <b>{say("🛡️ 통합 병목 · 결재", "🛡️ Bottlenecks · Approval")}</b>
        <span className="hud-sum">
          {bottlenecks.length ? say(`🚨 병목 ${bottlenecks.length}건`, `🚨 ${bottlenecks.length} bottlenecks`) : say("병목 없음", "No bottlenecks")}
          {pending ? ` · 🔒 Step ${pending.no}` : ""}
          {ttf ? ` · ${ttf}` : ""}
        </span>
        <button className="bh-toggle" onClick={() => onFold(!folded)} aria-expanded={!folded} title={folded ? say("결재 HUD 펼치기", "Expand the approval HUD") : say("결재 HUD 접기 (도면 오른쪽을 넓게)", "Fold the approval HUD (widen the drawing)")}>
          {folded ? say("◂ 펼치기", "◂ Expand") : say("접기 ▸", "Fold ▸")}
        </button>
      </header>
      {!folded && (
        <div className="hud-body">
          <Section title={say(`🚨 감지된 핵심 병목 (${bottlenecks.length}건)`, `🚨 Detected Key Bottlenecks (${bottlenecks.length})`)} open={open.bottleneck} onToggle={() => toggle("bottleneck")}>
            {bottlenecks.length === 0 && <p className="muted">{say("진행 중인 병목이 없습니다.", "No active bottlenecks.")}</p>}
            {bottlenecks.slice(0, 4).map((b, i) => (
              <button
                key={i}
                className={`bottleneck ${b.kind}`}
                onClick={() => {
                  onPick(b.incident);
                  if (b.kks) onSelect(b.kks);
                }}
              >
                <strong>
                  {i + 1}. [{b.kind === "physical" ? say("물리", "Physical") : say("절차", "Procedural")}] {b.title}
                </strong>
                {b.lines.slice(0, 2).map((l, k) => (
                  <span key={k}>• {l}</span>
                ))}
                {b.waiting_secs !== null && <em>⏳ {mmss(b.waiting_secs)}</em>}
              </button>
            ))}
          </Section>
          {detail && (
            <Section title={say(`🚦 인터록 게이트 · 결재${pending ? ` (🔒 Step ${pending.no})` : ""}`, `🚦 Interlock Gate · Approval${pending ? ` (🔒 Step ${pending.no})` : ""}`)} open={open.approval} onToggle={() => toggle("approval")} sectionRef={interlock}>
              <div className={flash ? "flash" : ""}>
                <InterlockPanel detail={detail} supervisor={supervisor} onChanged={onDecided} compact />
                <QuickApproval detail={detail} supervisor={supervisor} onDecided={onDecided} onOpenDetail={onOpenApproval} />
              </div>
            </Section>
          )}
          <Section title={say("🔎 선택 설비 · 이슈 트리 · 홉 뎁스", "🔎 Selected Equipment · Issue Tree · Hop Depth")} open={open.equipment} onToggle={() => toggle("equipment")}>
            <SidePanel plant={plant} detail={detail} selected={selected} onSelect={onSelect} t={t} catalog={catalog} />
          </Section>
        </div>
      )}
    </aside>
  );
}
