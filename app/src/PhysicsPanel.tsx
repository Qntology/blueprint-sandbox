import { invoke } from "./ipc";
import { listen } from "./ipc";
import { useEffect, useMemo, useState } from "react";
import { shortTag, useTerms } from "./domain";
import { useT } from "./i18n";
import { tr } from "./i18n/lang";
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

/** 추적 상태 이름 (그릴 때의 화면 언어로) */
const stateLabels = (): Record<string, string> => ({
  STABLE: "STABLE",
  INFLECTION_DETECTED: tr("SLEW (변곡)", "SLEW (inflection)"),
  CAUSAL_CHAIN_LOCKED: tr("인과 사슬 잠금", "Causal chain locked"),
  MITIGATING: tr("완화 조치", "Mitigating"),
  RECOVERING: tr("회복 중", "Recovering"),
});

/** 물리엔진 이벤트 시뮬레이터 & Zoro-1 자율 대응 (Part 8-5). 고장을 주입하면 식으로 연쇄가 생기고, SDS 추적기가 사건을 열고,
 *  정책이 허락하면 Tier 1 우회 개방을 자동 실행합니다. 명령은 시뮬레이터에만 들어갑니다. */
export function PhysicsPanel({ plant, supervisor, automation, onOpenIncident }: { plant: Plant; supervisor: string; automation: boolean; onOpenIncident: (id: string) => void }) {
  const terms = useTerms();
  const t = useT();
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
  const stateLabel = stateLabels();

  return (
    <div className="physics">
      <div className="physics-controls">
        <span className={`tag ${automation ? "auto" : ""}`}>
          {automation ? t("🟢 자동화 모드", "🟢 Automation Mode") : t(`보조 모드 (${terms.supervisor} 실행)`, `Assist Mode (run by ${terms.supervisor})`)}
        </span>
        <button className="plain" onClick={() => call("physics_start")}>
          {st?.running ? t("↺ 정상 운전으로 리셋", "↺ Reset to normal operation") : t("▶ 시뮬레이터 시작", "▶ Start simulator")}
        </button>
        <button className="danger-fill" disabled={!st?.running} onClick={() => call("physics_inject", { kind: "valve_stuck" })}>
          🔥 {t(`${valve?.name ?? "조절밸브"} 18% 고착 주입`, `Inject ${valve?.name ?? "control valve"} stuck at 18%`)}
        </button>
        <button className="plain" disabled={!st?.running} onClick={() => call("physics_inject", { kind: "sensor_drift" })}>
          {t("📉 압력 전송기 A 드리프트", "📉 Pressure transmitter A drift")}
        </button>
        <button className="plain" disabled={!st?.running} onClick={() => call("physics_stop")}>
          {t("■ 정지", "■ Stop")}
        </button>
        <span className="muted">
          {t("물리 틱", "Physics tick")} 100 ms · t = {st ? st.t_secs.toFixed(1) : "-"}s · {hotEq?.name ?? t("하류", "downstream")} {t("고점", "peak")} {st ? fmt(st.peak_sh) : "-"} ℃ ({terms.trip} {fmt(trip)} ℃)
        </span>
      </div>
      {msg && <p className="note">{msg}</p>}
      <div className="two-col">
        <section>
          <h4>{t("[실시간 물리엔진 → 시계열 · SDS 트래킹 → 자율 복구 파형] (최근 60초)", "[Live physics engine → time series · SDS tracking → autonomous recovery waveform] (last 60 s)")}</h4>
          {timeline ? (
            <WaveChart timeline={timeline} traces={traces} sensors={plant.sensors} from={timeline.t0_ms} to={timeline.now_ms} markers={[{ at_ms: timeline.now_ms, label: t("지금", "Now"), color: "#ffffff" }]} height={96} />
          ) : (
            <p className="muted">{t("시뮬레이터를 시작하세요. 처음 25초는 정상 운전 기준 구간입니다.", "Start the simulator. The first 25 s are the normal-operation baseline.")}</p>
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
          <h4>{t("[SDS 속성 트래킹 로그]", "[SDS attribute tracking log]")}</h4>
          <ul className="facts physics-log">
            {st?.events.map((e, i) => (
              <li key={i}>
                ⏱ t = {e.t.toFixed(1)}s · <span className="mono">{e.tag}</span> {stateLabel[e.from] ?? e.from} → <b>{stateLabel[e.to] ?? e.to}</b> · {e.note}
              </li>
            ))}
            {st?.commands.map((c, i) => (
              <li key={`c${i}`} className="good">
                ⚡ t = {c.t.toFixed(1)}s · {t(`시뮬레이터 명령: 바이패스 ${fmt(c.command.open_pct)}% 개방`, `Simulator command: open bypass ${fmt(c.command.open_pct)}%`)}
              </li>
            ))}
          </ul>
          {st?.call && (
            <p className="callout">
              🔒 t = {st.call.t.toFixed(1)}s {t("사건 열림", "Incident opened")} · {st.call.trigger} · {t("고장 코드", "fault code")} <b className="mono">{st.call.fault ?? "-"}</b>
              {st.call.lag_secs != null && <> · {t("변곡 시차", "inflection lag")} {st.call.lag_secs.toFixed(1)}s</>}
              {st.call.onset_lag_secs != null && <> · {t("변화 시작(CUSUM) 시차", "onset (CUSUM) lag")} {st.call.onset_lag_secs.toFixed(1)}s</>}
            </p>
          )}
          <h4>{t("[센서 신호 모양 (SDS 상태 코드)]", "[Sensor signal shape (SDS state codes)]")}</h4>
          <ul className="facts physics-log physics-signals">
            {st?.signals.map((e, i) => (
              <li key={`s${i}`}>
                📈 t = {e.t.toFixed(1)}s · <span className="mono">{e.tag}</span> {e.from} → <b>{e.to}</b> · {e.note}
              </li>
            ))}
          </ul>
          {st?.busy && <p className="note">{t("🧠 Zoro-1 판단 중 (공정은 계속 움직입니다)", "🧠 Zoro-1 is deciding (the process keeps running)")}</p>}
          {st?.note && <p className="callout">{st.note}</p>}
          {st?.incident && (
            <button className="plain" onClick={() => onOpenIncident(st.incident!)}>
              {t(`사건 ${st.incident} 열기 (판정 · 인터록 · 결재)`, `Open incident ${st.incident} (verdict · interlock · approval)`)}
            </button>
          )}
          {m && (
            <div className={`mitigation ${m.executed_at ? "done" : ""}`}>
              <h4>
                {t("[Tier 1 자율 대응]", "[Tier 1 Autonomous Response]")} {m.label} {st?.proposed_t != null && <span className="muted">· {t("제안", "proposed")} t = {st.proposed_t.toFixed(1)}s</span>} {m.post_audit && <span className="tag">{t("사후 샘플 감사 대상", "Post-hoc sample audit")}</span>}
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
                {t("최솟값(확신도)", "min (confidence)")} <b>{m.decided.judgement.confidence != null ? m.decided.judgement.confidence.toFixed(3) : t("없음", "none")}</b> · {m.auto ? t("자동 실행", "Auto-run") : t(`${terms.supervisor} 확인: ${m.reason}`, `${terms.supervisor} check: ${m.reason}`)}
              </p>
              {m.executed_at ? (
                <p className="good">
                  ✓ {t("실행", "Executed")} · {m.executed_by}
                </p>
              ) : (
                <button className="danger-fill" disabled={!supervisor.trim()} onClick={() => call("physics_execute", { supervisorId: supervisor })}>
                  ⚡ {t("완화 조치 실행", "Run mitigation")} ({terms.supervisor} {supervisor || t("사번 필요", "ID required")})
                </button>
              )}
            </div>
          )}
        </section>
      </div>
    </div>
  );
}
