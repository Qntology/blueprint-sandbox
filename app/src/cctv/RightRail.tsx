import { invoke } from "@tauri-apps/api/core";
import { useEffect, useState } from "react";
import { mmss } from "../plant";
import { SidePanel } from "../SidePanel";
import { InterlockPanel, QuickApproval } from "../StagePanel";
import type { Bottleneck, Detail, Plant, VerdictInfo } from "../types";

/** 우측 통합 병목 · 결재창 (개선안 3단계 2번). 마이닝 토글과 상관없이 병목 배지 → 인터록 신호등 게이트 → 표준 조치 · 결재 → 선택 설비가 한 줄로 늘 보입니다. */
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
}) {
  const { plant, detail, selected, onSelect, onPick, t, catalog, supervisor, onDecided, onOpenApproval } = props;
  const [bottlenecks, setBottlenecks] = useState<Bottleneck[]>([]);
  useEffect(() => {
    const load = () => invoke<{ bottlenecks: Bottleneck[] }>("mining_overview").then((m) => setBottlenecks(m.bottlenecks)).catch(() => {});
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [detail?.id]);
  return (
    <div className="rail">
      <section className="side rail-block">
        <h4>🚨 감지된 핵심 병목 ({bottlenecks.length}건)</h4>
        {bottlenecks.length === 0 && <p className="muted">진행 중인 병목이 없습니다.</p>}
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
              {i + 1}. [{b.kind === "physical" ? "물리" : "절차"}] {b.title}
            </strong>
            {b.lines.slice(0, 2).map((l, k) => (
              <span key={k}>• {l}</span>
            ))}
            {b.waiting_secs !== null && <em>⏳ {mmss(b.waiting_secs)}</em>}
          </button>
        ))}
      </section>
      {detail && (
        <section className="side rail-block">
          <InterlockPanel detail={detail} supervisor={supervisor} onChanged={onDecided} compact />
          <QuickApproval detail={detail} supervisor={supervisor} onDecided={onDecided} onOpenDetail={onOpenApproval} />
        </section>
      )}
      <SidePanel plant={plant} detail={detail} selected={selected} onSelect={onSelect} t={t} catalog={catalog} />
    </div>
  );
}
