import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useMemo, useState } from "react";
import { shortTag, useTerms } from "./domain";
import { fmt } from "./plant";
import type { Plant, Timeline } from "./types";
import { WaveChart, type Trace } from "./WaveChart";

interface Mitigation {
  label: string;
  tier: string;
  verdict: string;
  gates: [string, boolean][];
  decided: { judgement: { verdict: string; confidence: number | null }; axes: [string, number][] };
  auto: boolean;
  reason: string;
  post_audit: boolean;
  executed_at: string | null;
  executed_by: string | null;
}

interface PhysicsStatus {
  /** 그리는 태그 (원인 밸브 · 우회 밸브 · 유량 · 펌프 토출압 · 하류 온도)와 하류 온도 태그. 지금 도메인 원장의 `physics` 입니다. */
  shown: string[];
  hot_tag: string;
  running: boolean;
  busy: boolean;
  t_secs: number;
  t0_ms: number;
  now_ms: number;
  period_ms: number;
  series: Record<string, number[]>;
  latest: [string, number][];
  events: { t: number; tag: string; from: string; to: string; note: string }[];
  /** 센서 신호 모양 기록 (SDS 상태 코드: STABLE → SLEW_DROP → STUCK_LOW …) */
  signals: { t: number; tag: string; from: string; to: string; note: string }[];
  /** 추적기가 사건을 연 차례 (고장 코드는 원장 인터록의 fault 와 같은 꼴) */
  call: { t: number; alarm_tag: string; upstream_tag: string | null; lag_secs: number | null; onset_lag_secs: number | null; fault: string | null; trigger: string } | null;
  faults: { kind: string; open_pct?: number; tag?: string; bias?: number }[];
  commands: { t: number; command: { kind: string; open_pct: number } }[];
  incident: string | null;
  mitigation: Mitigation | null;
  proposed_t: number | null;
  peak_sh: number;
  note: string | null;
}

const TRACE_COLORS = ["#ff6b3d", "#9ae66e", "#4fd1c5", "#7aa2ff", "#f6c453"];

const STATE_LABEL: Record<string, string> = {
  STABLE: "STABLE",
  INFLECTION_DETECTED: "SLEW (변곡)",
  CAUSAL_CHAIN_LOCKED: "인과 사슬 잠금",
  MITIGATING: "완화 조치",
  RECOVERING: "회복 중",
};

/** 물리엔진 이벤트 시뮬레이터 & Zoro-1 자율 대응 (Part 8-5). 고장을 주입하면 식으로 연쇄가 생기고, SDS 추적기가 사건을 열고,
 *  정책이 허락하면 Tier 1 우회 개방을 자동 실행합니다. 명령은 시뮬레이터에만 들어갑니다. */
export function PhysicsPanel({ plant, supervisor, automation, onOpenIncident }: { plant: Plant; supervisor: string; automation: boolean; onOpenIncident: (id: string) => void }) {
  const terms = useTerms();
  const [st, setSt] = useState<PhysicsStatus | null>(null);
  const [msg, setMsg] = useState<string | null>(null);

  const load = () =>
    invoke<PhysicsStatus>("physics_status")
      .then(setSt)
      .catch(() => {});
  useEffect(() => {
    void load();
    const timer = window.setInterval(load, 1000);
    const stop = listen("physics_update", () => void load());
    return () => {
      window.clearInterval(timer);
      void stop.then((f) => f());
    };
  }, []);

  const call = async (cmd: string, args: Record<string, unknown> = {}) => {
    try {
      const r = await invoke<string | null>(cmd, args);
      setMsg(typeof r === "string" ? r : null);
      void load();
    } catch (e) {
      setMsg(String(e));
    }
  };

  const timeline: Timeline | null = st && Object.keys(st.series).length ? { t0_ms: st.t0_ms, period_ms: st.period_ms, now_ms: st.now_ms, series: st.series, channels: {} } : null;
  const m = st?.mitigation;
  // 그리는 태그 (펌프 토출압은 빼고 넷): 이름은 원장의 설비 · 센서 이름
  const traces = useMemo<Trace[]>(
    () =>
      (st?.shown ?? [])
        .filter((tag) => !shortTag(tag).startsWith("CP"))
        .map((tag, i) => {
          const s = plant.sensors.find((x) => x.tag === tag);
          const e = plant.equipment.find((x) => x.kks === s?.kks);
          const sector = plant.sectors.find((x) => x.key === e?.sector);
          return { tag, title: `${"①②③④⑤"[i]} ${sector?.building ?? ""} ${e?.name ?? ""} ${s?.label ?? ""} (${tag}, ${s?.unit ?? ""})`, color: TRACE_COLORS[i % TRACE_COLORS.length] };
        }),
    [st?.shown, plant],
  );
  const hot = plant.sensors.find((s) => s.tag === st?.hot_tag);
  const hotEq = plant.equipment.find((e) => e.kks === hot?.kks);
  const valve = plant.equipment.find((e) => e.kks === plant.sensors.find((s) => s.tag === st?.shown[0])?.kks);
  const trip = hot?.trip_high ?? 0;

  return (
    <div className="physics">
      <div className="physics-controls">
        <span className={`tag ${automation ? "auto" : ""}`}>{automation ? "🟢 자동화 모드" : `보조 모드 (${terms.supervisor} 실행)`}</span>
        <button className="plain" onClick={() => call("physics_start")}>
          {st?.running ? "↺ 정상 운전으로 리셋" : "▶ 시뮬레이터 시작"}
        </button>
        <button className="danger-fill" disabled={!st?.running} onClick={() => call("physics_inject", { kind: "valve_stuck" })}>
          🔥 {valve?.name ?? "조절밸브"} 18% 고착 주입
        </button>
        <button className="plain" disabled={!st?.running} onClick={() => call("physics_inject", { kind: "sensor_drift" })}>
          📉 압력 전송기 A 드리프트
        </button>
        <button className="plain" disabled={!st?.running} onClick={() => call("physics_stop")}>
          ■ 정지
        </button>
        <span className="muted">
          물리 틱 100 ms · t = {st ? st.t_secs.toFixed(1) : "-"}s · {hotEq?.name ?? "하류"} 고점 {st ? fmt(st.peak_sh) : "-"} ℃ ({terms.trip} {fmt(trip)} ℃)
        </span>
      </div>
      {msg && <p className="note">{msg}</p>}
      <div className="two-col">
        <section>
          <h4>[실시간 물리엔진 → 시계열 · SDS 트래킹 → 자율 복구 파형] (최근 60초)</h4>
          {timeline ? (
            <WaveChart timeline={timeline} traces={traces} sensors={plant.sensors} from={timeline.t0_ms} to={timeline.now_ms} markers={[{ at_ms: timeline.now_ms, label: "지금", color: "#ffffff" }]} height={96} />
          ) : (
            <p className="muted">시뮬레이터를 시작하세요. 처음 25초는 정상 운전 기준 구간입니다.</p>
          )}
          <div className="physics-latest">
            {st?.latest.map(([tag, v]) => (
              <span key={tag}>
                <b className="mono">{tag}</b> {fmt(v)} {plant.sensors.find((s) => s.tag === tag)?.unit}
              </span>
            ))}
          </div>
        </section>
        <section>
          <h4>[SDS 속성 트래킹 로그]</h4>
          <ul className="facts physics-log">
            {st?.events.map((e, i) => (
              <li key={i}>
                ⏱ t = {e.t.toFixed(1)}s · <span className="mono">{e.tag}</span> {STATE_LABEL[e.from] ?? e.from} → <b>{STATE_LABEL[e.to] ?? e.to}</b> · {e.note}
              </li>
            ))}
            {st?.commands.map((c, i) => (
              <li key={`c${i}`} className="good">
                ⚡ t = {c.t.toFixed(1)}s · 시뮬레이터 명령: 바이패스 {fmt(c.command.open_pct)}% 개방
              </li>
            ))}
          </ul>
          {st?.call && (
            <p className="callout">
              🔒 t = {st.call.t.toFixed(1)}s 사건 열림 · {st.call.trigger} · 고장 코드 <b className="mono">{st.call.fault ?? "-"}</b>
              {st.call.lag_secs != null && <> · 변곡 시차 {st.call.lag_secs.toFixed(1)}s</>}
              {st.call.onset_lag_secs != null && <> · 변화 시작(CUSUM) 시차 {st.call.onset_lag_secs.toFixed(1)}s</>}
            </p>
          )}
          <h4>[센서 신호 모양 (SDS 상태 코드)]</h4>
          <ul className="facts physics-log physics-signals">
            {st?.signals.map((e, i) => (
              <li key={`s${i}`}>
                📈 t = {e.t.toFixed(1)}s · <span className="mono">{e.tag}</span> {e.from} → <b>{e.to}</b> · {e.note}
              </li>
            ))}
          </ul>
          {st?.busy && <p className="note">🧠 Zoro-1 판단 중 (공정은 계속 움직입니다)</p>}
          {st?.note && <p className="callout">{st.note}</p>}
          {st?.incident && (
            <button className="plain" onClick={() => onOpenIncident(st.incident!)}>
              사건 {st.incident} 열기 (판정 · 인터록 · 결재)
            </button>
          )}
          {m && (
            <div className={`mitigation ${m.executed_at ? "done" : ""}`}>
              <h4>
                [Tier 1 자율 대응] {m.label} {st?.proposed_t != null && <span className="muted">· 제안 t = {st.proposed_t.toFixed(1)}s</span>} {m.post_audit && <span className="tag">사후 샘플 감사 대상</span>}
              </h4>
              <ul className="facts">
                {m.gates.map(([g, ok], i) => (
                  <li key={i} className={ok ? "good" : "bad"}>
                    {ok ? "✓" : "✗"} {g}
                  </li>
                ))}
              </ul>
              {m.decided.axes.map(([name, p]) => (
                <div key={name} className="bar-line">
                  <span className="bar-name">{name}</span>
                  <span className="bar">
                    <i style={{ width: `${p * 100}%` }} />
                  </span>
                  <b>{p.toFixed(3)}</b>
                </div>
              ))}
              <p>
                최솟값(확신도) <b>{m.decided.judgement.confidence != null ? m.decided.judgement.confidence.toFixed(3) : "없음"}</b> · {m.auto ? "자동 실행" : `${terms.supervisor} 확인: ${m.reason}`}
              </p>
              {m.executed_at ? (
                <p className="good">
                  ✓ 실행 · {m.executed_by}
                </p>
              ) : (
                <button className="danger-fill" disabled={!supervisor.trim()} onClick={() => call("physics_execute", { supervisorId: supervisor })}>
                  ⚡ 완화 조치 실행 ({terms.supervisor} {supervisor || "사번 필요"})
                </button>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
