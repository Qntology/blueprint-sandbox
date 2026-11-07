import { useMemo } from "react";
import { shortTag, useTerms } from "./domain";
import { useT } from "./i18n";
import { tr } from "./i18n/lang";
import { fmt, hopDepths, level, sensorsOf, valueAt } from "./plant";
import type { Detail, IssueReport, Plant, VerdictInfo } from "./types";

const LEVEL_TEXT = () => ({ normal: tr("정상", "Normal"), warn: tr("주의", "Warning"), alarm: tr("경보", "Alarm"), trip: tr("보호 정지 한계", "Trip limit") });

/** 우측 속성 패널: 선택 설비의 계측 · 기준치와 마인드맵 홉 뎁스 목록 (기획 Part 8 6.2절) */
const ISSUE_ICON: Record<string, string> = { equipment: "🔥", cascade: "⚠️", sensor: "📟", procedure: "🛑", moc: "📝" };

/** 다중 이슈 분해 트리 (Part 8-6): 단일 · 절차 · 설비 이슈를 쪼개 하나씩 해결 여부를 보여 줍니다. */
function IssueTree({ issues, onSelect }: { issues: IssueReport; onSelect: (kks: string) => void }) {
  const t = useT();
  if (!issues.subs.length) return null;
  return (
    <div className={`issues ${issues.kind}`}>
      <div className="issues-head">
        <b>{t("이슈 분류", "Issue Type")}</b> {issues.label}
        <span className="muted">
          {" "}
          · {issues.codes.join(" · ")}{" "}
          {t(
            `· 하위 ${issues.subs.length}건 (해결 ${issues.resolved} / 미해결 ${issues.open})`,
            `· ${issues.subs.length} sub-issues (${issues.resolved} resolved / ${issues.open} open)`,
          )}
        </span>
      </div>
      {issues.subs.map((s) => (
        <button key={s.n} className={`issue ${s.kind} ${s.resolved ? "resolved" : ""}`} onClick={() => s.kks && onSelect(s.kks)} disabled={!s.kks}>
          <span className="issue-title">
            {ISSUE_ICON[s.kind]} {t(`[이슈 #${s.n}: ${s.title}]`, `[Issue #${s.n}: ${s.title}]`)}
            {s.depth != null ? ` (Depth ${s.depth > 0 ? "+" : ""}${s.depth})` : ""} {s.resolved ? "✓" : ""}
          </span>
          <span>{s.detail}</span>
          <span className="muted">➔ {s.resolution}</span>
        </button>
      ))}
    </div>
  );
}

export function SidePanel({ plant, detail, selected, onSelect, t: time, catalog }: { plant: Plant | null; detail: Detail | null; selected: string | null; onSelect: (kks: string) => void; t: number | null; catalog: Record<string, VerdictInfo> }) {
  const terms = useTerms();
  const t = useT();
  const eq = plant?.equipment.find((e) => e.kks === selected);
  const depths = useMemo(() => (plant && selected ? [...hopDepths(plant.edges, selected, 3)].sort((a, b) => a[1] - b[1]) : []), [plant, selected]);
  if (!plant || !eq) return <section className="side empty">{t("설비를 고르세요 (도면 · Mermaid · 마인드맵 어디서든)", "Select equipment (from the Drawing, Mermaid or Mind Map)")}</section>;
  const tl = detail?.timeline ?? null;
  const at = time ?? tl?.now_ms ?? 0;
  const root = detail?.analysis.substantive?.causal?.root;
  const isRoot = root?.kks === eq.kks;
  const isAlarm = detail?.analysis.route.kks === eq.kks;
  const verdict = detail?.judgement?.verdict;
  return (
    <section className="side">
      {detail?.issues && <IssueTree issues={detail.issues} onSelect={onSelect} />}
      {isRoot && <div className="callout adverse">{t(`🔥 [근본 원인 후보] ${eq.name}`, `🔥 [Root Cause Candidate] ${eq.name}`)}</div>}
      {isAlarm && !isRoot && <div className="callout procedural">{t(`⚠️ [경보 설비] ${eq.name}`, `⚠️ [Alarm Equipment] ${eq.name}`)}</div>}
      {!isRoot && !isAlarm && <div className="callout">{t(`[선택 설비] ${eq.name}`, `[Selected Equipment] ${eq.name}`)}</div>}
      <dl className="props">
        <dt>{terms.code}</dt>
        <dd className="mono">{eq.kks}</dd>
        <dt>{t("섹터 · 층", "Sector · Floor")}</dt>
        <dd>
          {plant.sectors.find((s) => s.key === eq.sector)?.building} {plant.sectors.find((s) => s.key === eq.sector)?.label} · {eq.place.split(" ").slice(1).join(" ")}
        </dd>
        <dt>{t("도면", "Drawing")}</dt>
        <dd>{eq.drawing}</dd>
        {sensorsOf(plant, eq.kks).map((s) => {
          const v = tl ? valueAt(tl, s.tag, at) : s.normal;
          const lv = level(s, v);
          const limit = [s.alarm_high ?? s.alarm_low, s.trip_high ?? s.trip_low];
          return (
            <div key={s.tag} className="sensor-line">
              <dt>
                {s.label} <span className="mono muted">{shortTag(s.tag)}</span>
              </dt>
              <dd>
                <b className={`lv-text-${lv}`}>
                  {fmt(v)} {s.unit}
                </b>{" "}
                <span className="muted">
                  {t(
                    `(정상 ${fmt(s.normal)} · 경보 ${fmt(limit[0])} · ${terms.trip} ${fmt(limit[1])})`,
                    `(normal ${fmt(s.normal)} · alarm ${fmt(limit[0])} · ${terms.trip} ${fmt(limit[1])})`,
                  )}
                </span>{" "}
                <em className={`lv-tag lv-${lv}`}>{LEVEL_TEXT()[lv]}</em>
                {s.overlay && <em className="overlay-tag">{t(`${s.overlay} 반영`, `${s.overlay} applied`)}</em>}
              </dd>
            </div>
          );
        })}
      </dl>
      {isRoot && root && (
        <p className="muted">
          {t(
            `${root.hops}홉 상류 · 관측 시차 ${root.lag_secs.toFixed(1)}초 (허용 ${root.lag_range[0]} ~ ${root.lag_range[1]}초)${root.connectors.length ? ` · 도면 점프 ${root.connectors.join(", ")}` : ""}`,
            `${root.hops}-hop upstream · observed lag ${root.lag_secs.toFixed(1)}s (allowed ${root.lag_range[0]} ~ ${root.lag_range[1]}s)${root.connectors.length ? ` · drawing jump ${root.connectors.join(", ")}` : ""}`,
          )}
        </p>
      )}
      {verdict && (
        <p className="muted">
          {t("사건 판정 후보: ", "Incident verdict candidate: ")}
          <b>{catalog[verdict]?.label ?? verdict}</b>
        </p>
      )}
      <h4>{t("[마인드맵 연결 Hop Depth] (누르면 그 설비로 이동)", "[Mind Map Hop Depth] (click to jump to that equipment)")}</h4>
      <ol className="hops">
        {depths.map(([kks, d]) => {
          const e = plant.equipment.find((x) => x.kks === kks);
          if (!e) return null;
          const building = plant.sectors.find((s) => s.key === e.sector)?.building;
          return (
            <li key={kks}>
              <button className={`hop ${d === 0 ? "center" : ""} ${kks === root?.kks ? "root" : ""}`} onClick={() => onSelect(kks)}>
                <span className="mono">[D {d > 0 ? `+${d}` : d === 0 ? " 0" : d}]</span> {d === 0 ? "★ " : ""}
                {building} {e.name}
              </button>
            </li>
          );
        })}
      </ol>
    </section>
  );
}
