import { useMemo } from "react";
import { shortTag, useTerms } from "../domain";
import { clock, fixed, fmt, SDS_COLOR, sdsStateAt, signedSecs } from "../plant";
import type { Detail, Plant } from "../types";
import { WaveChart, type Trace } from "../WaveChart";
import { CamPanel } from "./CamPanel";

/** 사건의 t = 0 (근본 원인 변곡, 없으면 첫 변곡 · SDS 중심) */
export function originOf(detail: Detail): number {
  return detail.analysis.substantive?.causal?.root?.at_ms ?? detail.analysis.chain[0]?.at_ms ?? detail.analysis.sds.center_ms;
}

const CHANNEL_COLORS = ["#ff6b3d", "#4fd1c5", "#7aa2ff"];

/** 개선안 Part 3 좌측: 상류(역류) → 원인(D 0) → 하류(경보) 파형을 한 시간축에 겹쳐 그리고, 60초 링버퍼 지표(변곡 · 기울기 · 가속도 · TTF)를 붙입니다.
 *  하단 타임 스크러버의 시각 t 가 커서로 따라옵니다. */
export function TimeSeriesDock({ detail, plant, t }: { detail: Detail; plant: Plant; t: number }) {
  const terms = useTerms();
  const a = detail.analysis;
  const root = a.substantive?.causal?.root ?? null;
  const origin = originOf(detail);
  const traces = useMemo<Trace[]>(() => {
    const out: Trace[] = [];
    const label = (tag: string) => {
      const s = plant.sensors.find((x) => x.tag === tag);
      const e = plant.equipment.find((x) => x.kks === s?.kks);
      return `${e?.name ?? ""} ${s?.label ?? ""} (${shortTag(tag)}, ${s?.unit ?? ""})`;
    };
    const add = (tag: string, title: string, color: string, channel?: string) => {
      if (!out.some((x) => x.tag === tag && x.channel === channel)) out.push({ tag, title, color, channel });
    };
    const prop = a.substantive?.causal?.propagation ?? [];
    // 원인 기준 깊이: 원인보다 상류면 음수 (펌프 역류 여파 = D-1)
    const depth = (hops: number) => (root ? root.hops - hops : -hops);
    for (const p of prop.filter((x) => x.backflow)) add(p.upstream_tag, `[D${depth(p.hops)} 역류 여파] ${label(p.upstream_tag)}`, "#7aa2ff");
    if (root) add(root.tag, `[D 0 근본 원인] ${label(root.tag)}`, "#ff6b3d");
    for (const p of prop.filter((x) => x.causal_match && !x.backflow && x.upstream_tag !== root?.tag)) add(p.upstream_tag, `[D${depth(p.hops)} 파급] ${label(p.upstream_tag)}`, "#c084fc");
    if (a.route.tag) {
      const chs = detail.timeline.channels[a.route.tag];
      if (chs) Object.keys(chs).forEach((c, i) => add(a.route.tag!, `[채널 ${c}] ${label(a.route.tag!)}`, CHANNEL_COLORS[i % 3], c));
      else add(a.route.tag, `[${root ? `D+${root.hops}` : "D 0"} 경보] ${label(a.route.tag)}`, "#f6c453");
    }
    return out.slice(0, 4);
  }, [a, detail, plant, root]);
  const markers = a.chain.map((ev) => ({
    at_ms: ev.at_ms,
    label: `${signedSecs(ev.at_ms, origin)} ${shortTag(ev.tag)} ${ev.kind === "limit" ? "한계" : ev.direction === "Up" ? "↑" : "↓"}`,
    color: ev.kind === "limit" ? "#ff4d4f" : "#f6c453",
  }));
  const from = Math.max(detail.timeline.t0_ms, origin - 12_000);
  const to = Math.min(Math.max(detail.timeline.now_ms, origin + 10_000) + 8_000, detail.timeline.t0_ms + 150_000);
  const ttf = a.trigger.ttf_secs ?? a.substantive?.ttf_secs ?? null;
  return (
    <CamPanel cam="CAM-02" title="📈 시계열 로직 차트 (상류 → 원인 → 하류 · Δt · 기울기 · TTF)" info={`t ${signedSecs(t, origin)}${ttf !== null ? ` · ${terms.trip} 잔여 ${ttf.toFixed(0)}s` : ""}`}>
      <div className="dock-split">
        <div className="dock-chart">
          {traces.length ? (
            <WaveChart timeline={detail.timeline} traces={traces} sensors={plant.sensors} from={from} to={to} markers={[{ at_ms: origin, label: "t=0 발원", color: "#ffffff" }, ...markers]} cursor={t} height={62} />
          ) : (
            <p className="muted">계측 파형이 없는 사건입니다 (현장 보고만).</p>
          )}
          {root && (
            <p className="dock-note">
              관측 Δt = <b>+{root.lag_secs.toFixed(1)}s</b> (배관 허용 {root.lag_range[0]} ~ {root.lag_range[1]}s {root.lag_secs >= root.lag_range[0] - 0.5 && root.lag_secs <= root.lag_range[1] + 0.5 ? "✓" : "✗"}) · {root.hops}홉 상류 {plant.equipment.find((e) => e.kks === root.kks)?.name}
            </p>
          )}
        </div>
        <table className="grid dock-table">
          <thead>
            <tr>
              <th>센서</th>
              <th>변곡</th>
              <th>기울기/s</th>
              <th>가속도</th>
              <th>TTF</th>
            </tr>
          </thead>
          <tbody>
            {a.metrics.map((m) => (
              <tr key={m.sensor_tag} className={m.estimated_ttf_secs !== null && m.estimated_ttf_secs <= 30 ? "hot" : ""}>
                <td className="mono">{shortTag(m.sensor_tag)}</td>
                <td>{m.inflection_at_ms ? signedSecs(m.inflection_at_ms, origin) : "-"}</td>
                <td>{fixed(m.slope_per_sec_milli / 1000, 2)}</td>
                <td>{fixed(m.accel_milli / 1000, 2)}</td>
                <td>{m.estimated_ttf_secs !== null ? `${m.estimated_ttf_secs}s` : "-"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
    </CamPanel>
  );
}

/** 개선안 Part 3 우측: ① SDS 트리거 · 스냅샷 봉인 머리 ② 설비별 SDS 상태 전이 ③ 3축 직교 분할 검증 (최솟값 채택) ④ 게이트 SDS 벡터 */
export function SdsDock({ detail, plant, t, onSelect }: { detail: Detail; plant: Plant; t: number; onSelect: (kks: string) => void }) {
  const a = detail.analysis;
  const origin = originOf(detail);
  const first = (tr: Detail["signals"][number]) => tr.transitions.find(([, c]) => c !== "STABLE")?.[0] ?? Infinity;
  const tracks = [...detail.signals].sort((x, y) => first(x) - first(y)).slice(0, 5);
  const axes = a.sds.decomposed_confidences;
  const min = a.sds.min_confidence;
  return (
    <CamPanel cam="CAM-03" title="🧠 SDS 로직 차트 (상태 전이 · 3축 분할 검증)" info={a.sds.sds_id}>
      <div className="sds-head">
        <span>
          트리거 <b className="mono">{a.sds.trigger_kind}</b>
        </span>
        <span>동결 구간 {clock(a.sds.center_ms)} ±30s</span>
        {a.trigger.slope !== null && <span>기울기 {a.trigger.slope?.toFixed(2)}/s</span>}
      </div>
      <h5>① 설비별 SDS 상태 전이 (재생 시각 t 까지 켜짐)</h5>
      {tracks.length === 0 && <p className="muted">안정에서 벗어난 센서가 없습니다.</p>}
      {tracks.map((tr) => {
        const s = plant.sensors.find((x) => x.tag === tr.tag);
        const now = sdsStateAt(tr.transitions, t);
        return (
          <button key={tr.tag} className="sds-track" onClick={() => onSelect(tr.kks)} title={`${s?.label ?? ""} · 누르면 설비 선택`}>
            <span className="mono sds-tag">{shortTag(tr.tag)}</span>
            {tr.transitions.map(([at, code], i) => (
              <span key={i} className="sds-step">
                {i > 0 && <b className="sds-arrow">━►</b>}
                <span className={`sds-state ${at <= t ? "on" : "off"} ${now === code && at <= t ? "now" : ""}`} style={{ borderColor: SDS_COLOR[code] ?? "#59606e", color: at <= t ? SDS_COLOR[code] : undefined }}>
                  {code}
                  {i > 0 && <i>{signedSecs(at, origin)}</i>}
                </span>
              </span>
            ))}
          </button>
        );
      })}
      <h5>② 시공간 3축 직교 분할 검증 (최솟값 채택 = 환각 차단)</h5>
      {axes.length === 0 && <p className="muted">축별 질문 기록이 없습니다 (모델 없음 또는 절차 판정).</p>}
      {axes.map(([axis, p]) => (
        <div key={axis} className="axis-row">
          <span>{axis}</span>
          <div className="bar-track">
            <div className="bar-fill" style={{ width: `${p * 100}%` }} />
            <i className="bar-gate" style={{ left: "95%" }} title="자동 확정 기준 0.95" />
          </div>
          <b className="mono">
            {p.toFixed(3)}
            {min !== null && Math.abs(p - min) < 1e-9 ? " ◄" : ""}
          </b>
        </div>
      ))}
      <p className={`dock-note ${min !== null && min >= 0.95 ? "good" : ""}`}>➔ 최종 채택 확신도 (Min): {min !== null ? `${min.toFixed(3)} ${min >= 0.95 ? "(≥ 0.95 충족 ✓)" : "(기준 미달 · 교대감독 확인)"}` : "없음"}</p>
      <h5>③ 게이트 SDS 벡터</h5>
      <div className="chips">
        {a.sds.gate_state_vector.map((c, i) => (
          <span key={i} className="chip mono">
            {i > 0 ? "▸ " : ""}
            {c}
          </span>
        ))}
      </div>
      {a.chain.length > 0 && (
        <p className="muted small-text">
          전파 체인: {a.chain.slice(0, 6).map((ev) => `${signedSecs(ev.at_ms, origin)} ${shortTag(ev.tag)}${ev.kind === "limit" ? " 한계" : ev.direction === "Up" ? "↑" : "↓"}`).join(" → ")}
          {a.chain.length > 6 ? " …" : ""} · 지금 {fmt((t - origin) / 1000)}s
        </p>
      )}
    </CamPanel>
  );
}
