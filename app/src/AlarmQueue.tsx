import { useT } from "./i18n";
import { mmss } from "./plant";
import type { IncidentRow, VerdictInfo } from "./types";

/** 알람 · 사건 대기열: 현장 대기 · 결재 대기 · 처리 완료 (2차 개선안: 좌상단 [🚨 사건 대기열] 알약의 팝오버 안) */
export function AlarmQueue({ rows, selected, onPick, catalog }: { rows: IncidentRow[]; selected: string | null; onPick: (id: string) => void; catalog: Record<string, VerdictInfo> }) {
  const t = useT();
  const waiting = rows.filter((r) => r.state !== "done");
  const done = rows.filter((r) => r.state === "done");
  return (
    <aside className="queue">
      <h3>{t(`확인 대기 (${waiting.length})`, `Pending (${waiting.length})`)}</h3>
      {waiting.length === 0 && <p className="muted">{t("대기 중인 사건이 없습니다. 위의 시뮬레이터로 시나리오를 넣어 보세요.", "No incidents waiting. Inject a scenario with the simulator above.")}</p>}
      {waiting.map((r) => (
        <button key={r.id} className={`queue-item ${r.verdict_class ?? ""} ${selected === r.id ? "selected" : ""}`} onClick={() => onPick(r.id)}>
          <strong>
            {r.scenario ? `${r.scenario} · ` : ""}
            {r.title}
          </strong>
          <span>
            {r.building} · {r.kks}
          </span>
          <span className="verdict">{r.verdict ? catalog[r.verdict]?.label ?? r.verdict : t("판정 전", "No verdict yet")}</span>
          <span className="queue-foot">
            {r.state === "awaiting_field" ? (
              <em className="hold">{t("⏳ 현장 대기", "⏳ Awaiting Field Check")}</em>
            ) : r.high_risk ? (
              <em className="risk">{t("🔒 공동결재", "🔒 Co-sign")}</em>
            ) : (
              <em>{t("결재 대기", "Awaiting Approval")}</em>
            )}
            <i>{mmss(r.waiting_secs)}</i>
          </span>
        </button>
      ))}
      <h3>{t(`처리 완료 (${done.length})`, `Closed (${done.length})`)}</h3>
      {done.map((r) => (
        <button key={r.id} className={`queue-item done ${selected === r.id ? "selected" : ""}`} onClick={() => onPick(r.id)}>
          <strong>
            {r.scenario ? `${r.scenario} · ` : ""}
            {r.title}
          </strong>
          <span>
            {r.auto ? t("[자동확정] ", "[Auto-confirmed] ") : ""}
            {r.final_code}
          </span>
          <span className="queue-foot">
            <i>{r.archive_id}</i>
            <i>{r.closed_at?.slice(11)}</i>
          </span>
        </button>
      ))}
    </aside>
  );
}
