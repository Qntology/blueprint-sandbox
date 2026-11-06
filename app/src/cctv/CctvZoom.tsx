import { useMemo } from "react";
import { shortTag, useTerms } from "../domain";
import { fixed, fmt, level, SDS_COLOR, sdsStateAt, sensorsOf, signedSecs, valueAt } from "../plant";
import type { Detail, Plant } from "../types";
import { WaveChart, type Trace } from "../WaveChart";
import { originOf } from "./traces";

const COLORS = ["#ff6b3d", "#4fd1c5", "#7aa2ff", "#f6c453", "#c084fc"];
const LEVEL_TEXT = { normal: "정상", warn: "주의", alarm: "경보", trip: "보호 정지 한계" };

/** CCTV 말풍선의 [🔍 파형 확대] (noto-sandbox `.cz-card` 확대 비교 창): 그 설비의 모든 계측 파형 · SDS 전이 · 링버퍼 지표를 크게 봅니다. */
export function CctvZoom({ kks, plant, detail, t, onClose, onT }: { kks: string; plant: Plant; detail: Detail | null; t: number; onClose: () => void; onT: (t: number) => void }) {
  const terms = useTerms();
  const eq = plant.equipment.find((e) => e.kks === kks);
  const sensors = sensorsOf(plant, kks);
  const tl = detail?.timeline ?? null;
  const origin = detail ? originOf(detail) : 0;
  const traces = useMemo<Trace[]>(() => {
    const out: Trace[] = [];
    for (const s of sensors) {
      const chs = tl?.channels[s.tag];
      if (chs) Object.keys(chs).forEach((c) => out.push({ tag: s.tag, channel: c, title: `[채널 ${c}] ${s.label} (${shortTag(s.tag)}, ${s.unit})`, color: COLORS[out.length % COLORS.length] }));
      else out.push({ tag: s.tag, title: `${s.label} (${shortTag(s.tag)}, ${s.unit})`, color: COLORS[out.length % COLORS.length] });
    }
    return out;
  }, [sensors, tl]);
  if (!eq) return null;
  const tags = new Set(sensors.map((s) => s.tag));
  const markers = (detail?.analysis.chain ?? [])
    .filter((ev) => tags.has(ev.tag))
    .map((ev) => ({ at_ms: ev.at_ms, label: `${signedSecs(ev.at_ms, origin)} ${shortTag(ev.tag)} ${ev.kind === "limit" ? "한계" : ev.direction === "Up" ? "↑" : "↓"}`, color: ev.kind === "limit" ? "#ff4d4f" : "#f6c453" }));
  const from = tl ? Math.max(tl.t0_ms, origin - 20_000) : 0;
  const to = tl ? Math.min(tl.now_ms + 15_000, tl.t0_ms + 150_000) : 0;
  const tracks = detail?.signals.filter((s) => s.kks === kks) ?? [];
  const metrics = detail?.analysis.metrics.filter((m) => tags.has(m.sensor_tag)) ?? [];
  return (
    <div className="modal-back" role="dialog" aria-modal="true" aria-label={`${kks} 파형 확대`} onClick={(e) => e.target === e.currentTarget && onClose()}>
      <div className="modal-card cz-card">
        <header className="cz-head">
          <span className={tl ? "rec on" : "rec"} />
          <h3>
            🔍 CAM-EQ {eq.kks} · {eq.name} <span className="muted">· {eq.place} · 도면 {eq.drawing}</span>
          </h3>
          <button className="ov-close" onClick={onClose} aria-label="닫기">
            ✕
          </button>
        </header>
        <div className="cz-body">
          {tl && traces.length ? (
            <WaveChart timeline={tl} traces={traces} sensors={plant.sensors} from={from} to={to} markers={[{ at_ms: origin, label: "t=0 발원", color: "#ffffff" }, ...markers]} cursor={t} height={86} />
          ) : (
            <p className="muted">진행 중인 사건이 없어 {terms.telemetry} 파형이 없습니다. 기준치만 보입니다.</p>
          )}
          <table className="grid">
            <thead>
              <tr>
                <th>센서</th>
                <th>지금 (t {detail ? signedSecs(t, origin) : "-"})</th>
                <th>정상 · 경보 · {terms.trip}</th>
                <th>SDS 상태 전이</th>
                <th>변곡</th>
                <th>기울기/s</th>
                <th>TTF</th>
              </tr>
            </thead>
            <tbody>
              {sensors.map((s) => {
                const v = tl ? valueAt(tl, s.tag, t) : s.normal;
                const lv = level(s, v);
                const tr = tracks.find((x) => x.tag === s.tag);
                const m = metrics.find((x) => x.sensor_tag === s.tag);
                const now = tr ? sdsStateAt(tr.transitions, t) : null;
                return (
                  <tr key={s.tag}>
                    <td className="mono">
                      {shortTag(s.tag)} <span className="muted">{s.label}</span>
                    </td>
                    <td>
                      <b className={`lv-text-${lv}`}>
                        {fmt(v)} {s.unit}
                      </b>{" "}
                      <em className={`lv-tag lv-${lv}`}>{LEVEL_TEXT[lv]}</em>
                    </td>
                    <td>
                      {fmt(s.normal)} · {fmt(s.alarm_high ?? s.alarm_low)} · {fmt(s.trip_high ?? s.trip_low)}
                    </td>
                    <td>
                      {tr
                        ? tr.transitions.map(([at, code], i) => (
                            <span key={i} className={`sds-state ${at <= t ? "on" : "off"} ${now === code && at <= t ? "now" : ""}`} style={{ borderColor: SDS_COLOR[code] ?? "#59606e", color: at <= t ? SDS_COLOR[code] : undefined }}>
                              {code}
                              {i > 0 && <i>{signedSecs(at, origin)}</i>}
                            </span>
                          ))
                        : "-"}
                    </td>
                    <td>{m?.inflection_at_ms ? signedSecs(m.inflection_at_ms, origin) : "-"}</td>
                    <td>{m ? fixed(m.slope_per_sec_milli / 1000, 2) : "-"}</td>
                    <td>{m?.estimated_ttf_secs != null ? `${m.estimated_ttf_secs}s` : "-"}</td>
                  </tr>
                );
              })}
            </tbody>
          </table>
          {detail && (
            <div className="cz-actions">
              <button className="plain small" onClick={() => onT(origin)}>
                발원점으로 (t = 0)
              </button>
              <button className="plain small" onClick={() => onT(detail.timeline.now_ms)}>
                분석 시각으로
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
