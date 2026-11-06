import { invoke } from "./ipc";
import { useEffect, useMemo, useState, type ReactNode } from "react";
import { shortTag, useTerms } from "./domain";
import { useT } from "./i18n";
import { tr } from "./i18n/lang";
import type { DrawerKey } from "./OfficerView";
import { clock, fixed, fmt, mmss, signedSecs, STAGE_LABEL } from "./plant";
import type { ArchiveDoc, Asked, Detail, InterlockStep, Lane, Plant, VerdictInfo } from "./types";
import { WaveChart, type Trace } from "./WaveChart";

const STAGE_CLASS: Record<string, string> = { Done: "ok", Skipped: "skip", Corrected: "fix", Alert: "alert", Waiting: "wait", Blocked: "block" };

function duration(ms: number): string {
  if (ms <= 0) return "-";
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
  return ms < 1000 ? `${ms.toFixed(1)}ms` : tr(`${(ms / 1000).toFixed(2)}초`, `${(ms / 1000).toFixed(2)}s`);
}

/** 단계 카드에 붙일 프로세스 마이닝 레인 (개선안 3단계 3번: 단계 바 + 마이닝 레인 두 줄을 한 줄로) */
const STAGE_LANES: Record<number, string[]> = { 1: ["gate01"], 2: ["gate01"], 3: ["gate2"], 4: ["gate3"], 5: ["gate45", "approval"] };

/** 파이프라인 줄 (Stage 1 ~ 5). 카드마다 그 단계 레인의 건수 · 최대 대기를 함께 적습니다. 누르면 단계 상세가 열립니다.
 *  2차 개선안: 하단 허브 맨 윗줄(noto-sandbox 의 퀘스트 줄 자리)에 한 줄로 들어가고, 단계 요약은 마우스를 올리면 보입니다. */
export function StageBar({ detail, onOpen, lanes, tail }: { detail: Detail | null; onOpen: (k: DrawerKey) => void; lanes: Lane[]; tail?: ReactNode }) {
  const terms = useTerms();
  const t = useT();
  const stages = detail?.analysis.stages ?? [];
  const lane = (n: number) => {
    const ls = lanes.filter((l) => STAGE_LANES[n].includes(l.key));
    return { count: Math.max(0, ...ls.map((l) => l.count)), wait: Math.max(0, ...ls.map((l) => (l.count > 0 ? l.max_wait_secs : 0))) };
  };
  const done = lanes.find((l) => l.key === "done")?.count ?? 0;
  return (
    <div className="stagebar">
      <span
        className="stagebar-title"
        title={detail ? t(`${detail.analysis.route.kks} 파이프라인 · 마이닝 레인`, `${detail.analysis.route.kks} Pipeline · Mining Lanes`) : t("파이프라인 · 마이닝 레인", "Pipeline · Mining Lanes")}
      >
        {t("🧩 파이프라인", "🧩 Pipeline")}
        <i>{t(`처리 완료 ${done}건`, `${done} closed`)}</i>
      </span>
      {[1, 2, 3, 4, 5].map((n) => {
        const s = stages.find((x) => x.stage === n);
        const l = lane(n);
        return (
          <button
            key={n}
            className={`stage-card ${s ? STAGE_CLASS[s.status] : "skip"}`}
            disabled={!detail}
            onClick={() => onOpen(n as DrawerKey)}
            title={t(
              `${s?.summary ?? "-"} (${duration(s?.ms ?? 0)}) · 레인 ${l.count}건${l.wait > 0 ? ` · 최대 대기 ${mmss(l.wait)}` : ""}`,
              `${s?.summary ?? "-"} (${duration(s?.ms ?? 0)}) · Lane: ${l.count}${l.wait > 0 ? ` · Max wait ${mmss(l.wait)}` : ""}`,
            )}
          >
            <strong>
              Stage {n}.{" "}
              {s?.name ??
                [
                  t("SigLIP2 크롭", "SigLIP2 Crop"),
                  t("gocr 판독 + 대장 교정", "gocr Read + Register Fix"),
                  t("Gate 1~2 원장 대조", "Gate 1~2 Ledger Check"),
                  t("Gate 3 + Zoro-1 판단", "Gate 3 + Zoro-1 Verdict"),
                  t(`Zoro-1 ${terms.procedure} 초안`, `Zoro-1 ${terms.procedure} Draft`),
                ][n - 1]}
            </strong>
            <em>
              [{s ? STAGE_LABEL[s.status] : t("대기", "Waiting")}]
              <span className={`lane-badge ${l.wait > 0 ? "hold" : ""}`}>
                {t(`${l.count}건`, `${l.count}`)}
                {l.wait > 0 ? ` · ⏳ ${mmss(l.wait)}` : ""}
              </span>
            </em>
          </button>
        );
      })}
      {tail}
    </div>
  );
}

function Bars({ asked, catalog, highlight }: { asked: Asked; catalog: Record<string, VerdictInfo>; highlight?: string }) {
  return (
    <div className="bars">
      {asked.options.map((o, i) => (
        <div key={o} className={`bar-row ${o === highlight ? "rule" : ""}`}>
          <span>
            {catalog[o]?.label ?? o} <i className="mono muted">({o})</i>
          </span>
          <div className="bar-track">
            <div className="bar-fill" style={{ width: `${Math.max(1, asked.probabilities[i] * 100)}%` }} />
          </div>
          <b className="mono">{asked.probabilities[i].toFixed(3)}</b>
        </div>
      ))}
    </div>
  );
}

function Scan({ detail }: { detail: Detail }) {
  const terms = useTerms();
  const t = useT();
  const stages = detail.analysis.stages;
  if (detail.reports.length === 0) {
    return (
      <p className="callout">
        {t(
          "이 사건은 현장 스캔이 없는 신호 알람이라 문서 모델(SigLIP2 · gocr)을 GPU 에 올리지 않고 바로 Zoro-1 판단으로 갔습니다 (SharedResidency: 한 번에 한 모델).",
          "This incident is a signal alarm with no field scan, so the document models (SigLIP2 · gocr) were not loaded onto the GPU and it went straight to the Zoro-1 verdict (SharedResidency: one model at a time).",
        )}
      </p>
    );
  }
  return (
    <>
      <div className="timeline-strip">
        {t("[GPU 상주 타임라인]", "[GPU Residency Timeline]")} <b>{t("규칙 · SigLIP2 비전", "Rules · SigLIP2 Vision")}</b> {duration(stages[0]?.ms ?? 0)} ➔ <b>{t("gocr 비전 → 언어", "gocr Vision → Language")}</b>{" "}
        {duration(stages[1]?.ms ?? 0)}
      </div>
      {detail.reports.map((r, i) => (
        <section key={i} className="card">
          <h4>
            {r.kind} · {r.taken_at} · {r.worker} ({r.worker_id}) {r.signed ? t("· 서명 검출", "· Signature detected") : t("· 서명 없음", "· No signature")}{" "}
            {r.permit_confirmed ? t(`· ${terms.permit} 확인`, `· ${terms.permit} confirmed`) : ""}
          </h4>
          <dl className="props">
            <dt>{t(`설비 ${terms.code}`, `Equipment ${terms.code}`)}</dt>
            <dd className="mono">{r.kks}</dd>
            <dt>{t("센서 태그", "Sensor Tag")}</dt>
            <dd className="mono">{r.tag}</dd>
            {r.drawing && (
              <>
                <dt>{t("도면", "Drawing")}</dt>
                <dd>
                  {r.drawing} {r.drawing_rev}
                </dd>
              </>
            )}
            {r.moc_no && (
              <>
                <dt>{t("변경번호", "Change No.")}</dt>
                <dd className="mono">{r.moc_no}</dd>
                <dt>{t("변경 사양", "Change Spec")}</dt>
                <dd>
                  {fmt(r.old_value)} → {fmt(r.reading)} {r.unit}
                </dd>
              </>
            )}
            {!r.moc_no && (
              <>
                <dt>{t("현장 실측치", "Field Reading")}</dt>
                <dd>
                  {fmt(r.reading)} {r.unit}
                </dd>
              </>
            )}
            {r.excuse && (
              <>
                <dt>{t("현장 소명", "Field Justification")}</dt>
                <dd>{r.excuse}</dd>
              </>
            )}
          </dl>
          {r.corrections.length > 0 && (
            <table className="grid">
              <thead>
                <tr>
                  <th>{t("항목", "Item")}</th>
                  <th>{t("gocr 원문", "gocr Raw")}</th>
                  <th>{t("대장 교정", "Register Fix")}</th>
                  <th>{t("편집 거리", "Edit Distance")}</th>
                </tr>
              </thead>
              <tbody>
                {r.corrections.map((c, k) => (
                  <tr key={k}>
                    <td>{c.field}</td>
                    <td className="mono bad">{c.raw}</td>
                    <td className="mono good">{c.fixed}</td>
                    <td>{c.distance}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          )}
          {r.uncertain.length > 0 && <p className="note">{t(`확인할 항목: ${r.uncertain.join(", ")}`, `To verify: ${r.uncertain.join(", ")}`)}</p>}
        </section>
      ))}
    </>
  );
}

function stateText(v: unknown): string {
  if (typeof v === "string") return v === "Valid" ? tr("정상", "Normal") : v === "NoData" ? tr("계측 없음", "No data") : v;
  if (v && typeof v === "object") {
    const [k, inner] = Object.entries(v as Record<string, Record<string, unknown>>)[0];
    if (k === "Flatline") return tr(`값 고정 ${inner.secs}초 (${inner.value})`, `Flatline ${inner.secs}s (${inner.value})`);
    if (k === "SpikeFiltered") return tr(`순간 잡음 ${(inner.spikes as unknown[]).length}건 걸러 냄`, `Filtered ${(inner.spikes as unknown[]).length} spikes`);
    if (k === "Isolated")
      return tr(
        `채널 ${inner.channel} 단독 편차${inner.field_confirmed ? " · 현장 확인" : " · 현장 대기"}`,
        `Channel ${inner.channel} isolated deviation${inner.field_confirmed ? " · field confirmed" : " · awaiting field check"}`,
      );
    if (k === "Contradiction" || k === "Hold") return String(inner.reason);
    return k;
  }
  return "-";
}

function Truth({ detail, supervisor }: { detail: Detail; supervisor: string }) {
  const terms = useTerms();
  const t = useT();
  const a = detail.analysis;
  const [checked, setChecked] = useState<Record<number, boolean>>({});
  const [names, setNames] = useState<string | null>(null);
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">
          {t(
            `[3중 진실 우선순위] ① 현장 스캔 증빙(게이지 · Red-line) > ② ${terms.telemetry} 다중 보팅 · ${terms.change} 승인 원장 > ③ 단일 센서 신고값`,
            `[Truth Priority] ① Field scan evidence (gauge · Red-line) > ② ${terms.telemetry} multi-channel voting · ${terms.change} approval ledger > ③ Single-sensor reading`,
          )}
        </div>
        <table className="grid">
          <thead>
            <tr>
              <th>{t("대조 항목", "Check Item")}</th>
              <th>{t("① 현장 스캔본", "① Field Scan")}</th>
              <th>{t("② 시드 원장", "② Seed Ledger")}</th>
              <th>{t("③ 단일 알람", "③ Single Alarm")}</th>
              <th>{t("일치", "Match")}</th>
            </tr>
          </thead>
          <tbody>
            {a.truth.map((r, i) => (
              <tr key={i}>
                <td>{r.item}</td>
                <td>{r.field ?? "-"}</td>
                <td>{r.ledger ?? "-"}</td>
                <td>{r.single ?? "-"}</td>
                <td className={r.agree ? "good" : "bad"}>{r.agree ? "✓" : "✗"}</td>
              </tr>
            ))}
          </tbody>
        </table>
        <dl className="props">
          <dt>{t("Gate 1 신호", "Gate 1 Signal")}</dt>
          <dd>{a.signal ? stateText(a.signal.state) : t("계측 없음", "No data")}</dd>
          {a.signal?.channels.map(([c, s]) => (
            <div key={c} className="sensor-line">
              <dt>{t(`채널 ${c}`, `Channel ${c}`)}</dt>
              <dd>{stateText(s)}</dd>
            </div>
          ))}
          {a.vote && (
            <>
              <dt>{t(`${a.vote.logic} 보팅`, `${a.vote.logic} Voting`)}</dt>
              <dd>
                {a.vote.channels.map(([c, v]) => `${c} ${fmt(v)}`).join(" · ")} {t("→ 대표값", "→ voted")} {fmt(a.vote.voted)} · <span className="mono">{a.vote.state_code}</span>
              </dd>
            </>
          )}
          {a.drift !== null && a.drift !== undefined && (
            <>
              <dt>{t("편차 판정", "Deviation Verdict")}</dt>
              <dd>{stateText(a.drift)}</dd>
            </>
          )}
          {a.field_checks.map((f, i) => (
            <div key={i} className="sensor-line">
              <dt>{t("시점 동기화 대조", "Time-Synced Check")}</dt>
              <dd>
                {t(
                  `촬영 ${clock(f.taken_at_ms)} · 현장 ${fmt(f.field_value)} vs 같은 시각 ${terms.telemetry} ${fmt(f.dcs_value)} (허용 ±${fmt(f.tolerance)}) ${f.matches_dcs ? "✓ 일치" : "✗ 불일치"}`,
                  `Photo ${clock(f.taken_at_ms)} · field ${fmt(f.field_value)} vs ${terms.telemetry} at same time ${fmt(f.dcs_value)} (tol. ±${fmt(f.tolerance)}) ${f.matches_dcs ? "✓ match" : "✗ mismatch"}`,
                )}
              </dd>
            </div>
          ))}
          {a.moc && (
            <>
              <dt>{t(`${terms.change} 대조`, `${terms.change} Check`)}</dt>
              <dd>
                {a.moc.moc_no} · {stateText(a.moc.outcome)}
              </dd>
              {a.moc.stale_drawing && (
                <>
                  <dt>{t("도면 개정", "Drawing Revision")}</dt>
                  <dd className="bad">{a.moc.stale_drawing}</dd>
                </>
              )}
            </>
          )}
        </dl>
        <h4>{t(`[Gate 2 검출 사항 · ${terms.supervisor} 확인 체크리스트]`, `[Gate 2 Findings · ${terms.supervisor} Checklist]`)}</h4>
        {(detail.notes.length ? detail.notes : [a.reason]).map((n, i) => (
          <label key={i} className="check">
            <input type="checkbox" checked={!!checked[i]} onChange={(e) => setChecked((c) => ({ ...c, [i]: e.target.checked }))} />
            {n}
          </label>
        ))}
      </section>
      <section>
        <h4>{t("[작업자 권한 및 계통 연결망 (가림 처리)]", "[Worker Permissions & System Links (masked)]")}</h4>
        {detail.reports.length === 0 && <p className="muted">{t("현장 보고가 없습니다.", "No field reports.")}</p>}
        {detail.reports.map((r, i) => (
          <p key={i}>
            • {r.kind}: {r.worker} ({r.worker_id})
          </p>
        ))}
        <button
          className="plain"
          disabled={detail.reports.length === 0}
          onClick={() =>
            invoke<string[] | null>("request_unmask", { id: detail.id, supervisorId: supervisor })
              .then((n) =>
                setNames(
                  n
                    ? t(`열람 허용 (감사 기록됨): ${n.join(", ")}`, `Access granted (audit logged): ${n.join(", ")}`)
                    : t("열람 거부: 위험도 70% 미만 (감사 기록됨)", "Access denied: risk below 70% (audit logged)"),
                ),
              )
              .catch((e) => setNames(String(e)))
          }
        >
          {t("🔓 작업자 실명 열람 요청", "🔓 Request worker names")}
        </button>
        {names && <p className="note">{names}</p>}
        <h4>{t("[계통 파급 경로 (마인드맵 3홉 이내)]", "[System Propagation Path (Mind Map, within 3 hops)]")}</h4>
        {(a.substantive?.causal?.propagation ?? []).map((p, i) => (
          <p key={i} className={p.causal_match && !p.backflow ? "good" : "muted"}>
            {t(
              `[${p.hops}홉 상류] ${p.upstream_kks} · Δt ${p.observed_lag_secs.toFixed(1)}s (${p.expected_lag_range[0]}~${p.expected_lag_range[1]}s)${p.backflow ? " · 역류 여파" : p.causal_match ? " · 인과 일치" : " · 시차 불일치"}`,
              `[${p.hops}-hop upstream] ${p.upstream_kks} · Δt ${p.observed_lag_secs.toFixed(1)}s (${p.expected_lag_range[0]}~${p.expected_lag_range[1]}s)${p.backflow ? " · backflow effect" : p.causal_match ? " · causal match" : " · lag mismatch"}`,
            )}
          </p>
        ))}
      </section>
    </div>
  );
}

function Decision({ detail, catalog }: { detail: Detail; catalog: Record<string, VerdictInfo> }) {
  const t = useT();
  const a = detail.analysis;
  const sub = a.substantive;
  const asked = sub?.decided.asked ?? [];
  const rule = sub?.decided.judgement.verdict;
  const ruleLabel = rule ? (catalog[rule]?.label ?? "") : "-";
  const trace = a.model_trace;
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">
          {t(
            "[동작 원리] 백본이 정제된 사실(근무자 정보 제외)만 읽고, jev_head 가 선택지 위치(닫는 따옴표)의 은닉 상태로 점수를 매깁니다 (0-Generation)",
            "[How it works] The backbone reads only cleaned facts (worker info excluded), and jev_head scores each option from the hidden state at its position (the closing quote) (0-Generation)",
          )}
        </div>
        <h4>{t("[모델에 입력된 표준 JSON 사실 (PointerPrompt)]", "[Standard JSON Facts Given to the Model (PointerPrompt)]")}</h4>
        <dl className="props">
          {(sub?.facts ?? []).map(([k, v]) => (
            <div key={k} className="sensor-line">
              <dt>{k}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
        <h4>{t("[GPU 상주 · 모델 실행 기록]", "[GPU Residency · Model Run Log]")}</h4>
        {trace.length === 0 && <p className="muted">{t("모델 기록 없음 (규칙만 실행)", "No model log (rules only)")}</p>}
        <ol className="trace">
          {trace.map((r, i) => (
            <li key={i}>
              <span className="mono">{r.model}</span> {r.step} <b>{r.ms.toFixed(1)}ms</b> {r.detail?.group ? <i className="muted">{String(r.detail.group)}</i> : null}
            </li>
          ))}
        </ol>
      </section>
      <section>
        {asked.length === 0 && (
          <p className="callout">{t(`Zoro-1 이 꽂혀 있지 않아 규칙 판정만 있습니다: ${ruleLabel}`, `Zoro-1 is not loaded, so there is only the rule verdict: ${ruleLabel}`)}</p>
        )}
        {asked[0] && (
          <>
            <h4>{t(`질문 1. ${asked[0].title}`, `Question 1. ${asked[0].title}`)}</h4>
            <Bars asked={asked[0]} catalog={catalog} highlight={rule} />
          </>
        )}
        {asked.slice(1).map((q, i) => (
          <div key={i} className="axis">
            <h4>
              {/* 질문 제목 "<과제> 중 <기준> 만 판단" 의 기준만 (영어 화면은 번역 표가 "<task> — judge only: <criterion>" 으로 바꿉니다) */}
              {t(`[SDS 축 ${i + 1}]`, `[SDS Axis ${i + 1}]`)} {q.title.includes(" — judge only: ") ? q.title.split(" — judge only: ").pop() : q.title.split(" 중 ").pop()?.replace(" 만 판단", "")}
            </h4>
            <p className="muted">{q.facts.map(([k, v]) => `${k}: ${v}`).join(" · ")}</p>
            <Bars asked={q} catalog={catalog} highlight={rule} />
          </div>
        ))}
        <div className="callout">
          {t(
            `📌 [판정 결합 규칙 (verdict.rs)] 판정은 규칙이 정합니다 (${ruleLabel}). 모델이 같은 쪽을 고르면 축별 최솟값이 확신도가 됩니다:`,
            `📌 [Verdict Combination Rule (verdict.rs)] The rules set the verdict (${ruleLabel}). If the model picks the same side, the lowest per-axis score becomes the confidence:`,
          )}{" "}
          <b>{sub?.decided.judgement.confidence != null ? sub.decided.judgement.confidence.toFixed(3) : t("없음", "none")}</b>
          {t(
            ". 정상 유지는 최솟값 0.95 이상일 때만 자동 확정 후보이고, 계통 이상 · 고위험은 교대감독이 확정합니다.",
            ". Normal hold is an auto-confirm candidate only when the minimum is 0.95 or higher; system faults and high-risk cases are confirmed by the Shift Supervisor.",
          )}
        </div>
      </section>
    </div>
  );
}

const DEPTH = (d: number) => (d > 0 ? `+${d}` : `${d}`);

function requirement(s: InterlockStep): string {
  const unit = s.unit ?? "";
  if (s.check.kind === "below") return tr(`${fmt(s.check.value)} ${unit} 이하`, `≤ ${fmt(s.check.value)} ${unit}`);
  if (s.check.kind === "at_least") return tr(`${fmt(s.check.value)} ${unit} 이상`, `≥ ${fmt(s.check.value)} ${unit}`);
  return tr("현장 LOTO 서명", "Field LOTO signature");
}

/** 깊이별 선행 절차(Interlock) 체크 (Part 8-4). 순서대로 끝나야 아래 확정이 풀립니다.
 *  `compact` 는 우측 결재창의 신호등 게이트 (개선안 3단계 2번: 마이닝 · 인터록 · 결재를 한 화면에) */
export function InterlockPanel({ detail, supervisor, onChanged, compact = false }: { detail: Detail; supervisor: string; onChanged: (m: string) => void; compact?: boolean }) {
  const terms = useTerms();
  const t = useT();
  const il = detail.analysis.interlock;
  const [values, setValues] = useState<Record<number, string>>({});
  const [busy, setBusy] = useState(false);
  if (!il) return null;
  const next = il.steps.find((s) => s.status !== "passed");
  const submit = async (step: number, value: number | null, radio: boolean) => {
    setBusy(true);
    try {
      const r = await invoke<{ ok: boolean; message: string }>("record_interlock", { input: { id: detail.id, supervisor_id: supervisor, step, value, radio } });
      onChanged(r.message);
    } catch (e) {
      onChanged(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className={`interlock ${compact ? "compact" : ""}`}>
      <h4>
        {t(`[${il.aop_id} 인터록 게이트]`, `[${il.aop_id} Interlock Gate]`)}{" "}
        <span className="muted">{compact ? t("순서대로 통과해야 확정", "Pass in order to confirm") : t("순서대로 끝나야 확정이 풀립니다", "Confirm unlocks once every step is done in order")}</span>
      </h4>
      {il.steps.map((s) => (
        <div key={s.no} className={`il-step ${s.status}`}>
          <div className="il-head">
            <b>
              {s.status === "passed" ? "🟢" : next?.no === s.no ? "🛑" : "⚪"} [Step {s.no}]
            </b>{" "}
            Depth {DEPTH(s.req_depth)} · <span className="mono">{s.kks}</span>
          </div>
          <div>
            {s.status === "passed" ? "☑" : "☐"} {s.text}
          </div>
          {s.evidence && <div className="il-evidence">➔ {s.evidence}</div>}
          {s.status !== "passed" && (
            <div className="il-evidence warn">
              {s.observed != null ? t(`현재 ${fmt(s.observed)} ${s.unit ?? ""} · `, `Now ${fmt(s.observed)} ${s.unit ?? ""} · `) : ""}
              {t(`필요: ${requirement(s)}`, `Required: ${requirement(s)}`)}
              {next?.no !== s.no ? t(` · Step ${next?.no} 먼저`, ` · Step ${next?.no} first`) : ""}
            </div>
          )}
          {s.status !== "passed" && s.check.kind !== "loto" && (
            <div className="il-input">
              <input
                type="number"
                placeholder={t(`현장 값 (${s.unit ?? ""})`, `Field value (${s.unit ?? ""})`)}
                value={values[s.no] ?? ""}
                onChange={(e) => setValues({ ...values, [s.no]: e.target.value })}
              />
              <button className="plain" disabled={busy || next?.no !== s.no || !values[s.no]} onClick={() => submit(s.no, Number(values[s.no]), false)}>
                {t("현장 값 확인", "Check field value")}
              </button>
            </div>
          )}
          {s.status !== "passed" && s.check.kind === "loto" && (
            <div className="il-input">
              <button className="plain" disabled={busy || next?.no !== s.no} onClick={() => submit(s.no, null, true)}>
                {t(`📻 ${terms.supervisor} 무전 확인으로 갈음 (사번 날인)`, `📻 Use ${terms.supervisor} radio confirmation instead (ID stamp)`)}
              </button>
            </div>
          )}
        </div>
      ))}
    </div>
  );
}

function Approval({ detail, supervisor, onDecided }: { detail: Detail; supervisor: string; onDecided: (m: string) => void }) {
  const terms = useTerms();
  const t = useT();
  const d = detail.analysis.draft;
  const [ref, setRef] = useState(d?.aop_id ?? "");
  const [cosign, setCosign] = useState(false);
  const [scans, setScans] = useState(0);
  const [note, setNote] = useState("");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const pending = detail.analysis.interlock?.steps.find((s) => s.status !== "passed") ?? null;
  const act = async (action: string) => {
    setBusy(true);
    try {
      const r = await invoke<{ ok: boolean; message: string }>("decide_incident", { input: { id: detail.id, supervisor_id: supervisor, action, team_lead_cosign: cosign, procedure_ref: ref, evidence_scans: scans, note } });
      setReply(r.message);
      onDecided(r.message);
    } catch (e) {
      setReply(String(e));
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">
          {t("[듀얼 헤드] 판단과 같은 몸통에 임베딩 표를 더 올려 글을 씁니다", "[Dual Head] Writes text by adding an embedding table to the same trunk as the verdict")}{" "}
          {d
            ? `(${
                d.by_model
                  ? t(`모델 · ${duration(d.ms)} · ${d.attempts}회`, `model · ${duration(d.ms)} · ${d.attempts} ${d.attempts === 1 ? "attempt" : "attempts"}`)
                  : t("템플릿", "template")
              })`
            : ""}
        </div>
        {d ? (
          <pre className="aop">
            {t(
              `[원인 진단 요약]\n${d.summary}\n\n[표준 조치 지시 (${d.aop_id} ${d.title})]\n${d.steps.map((s, i) => `${i + 1}. ${s}`).join("\n")}`,
              `[Cause Diagnosis Summary]\n${d.summary}\n\n[Standard Action Orders (${d.aop_id} ${d.title})]\n${d.steps.map((s, i) => `${i + 1}. ${s}`).join("\n")}`,
            )}
          </pre>
        ) : (
          <p className="callout">
            {t(
              `이 판정에는 조치 절차서가 없습니다 (${detail.analysis.moc ? "Red-line 변경 반영 심사" : "절차서 없음"}).`,
              `This verdict has no action procedure (${detail.analysis.moc ? "Red-line change review" : "no procedure"}).`,
            )}
          </p>
        )}
        {detail.state === "awaiting_field" && (
          <p className="note">
            {t("⏳ 현장 확인 대기 중입니다. 현장 게이지 실측이 오기 전에는 확정할 수 없습니다.", "⏳ Awaiting field check. Cannot confirm until the field gauge reading arrives.")}
          </p>
        )}
      </section>
      <section>
        <h4>{t("[Factcheck 원문 숫자 · 코드 1:1 대조 검증기]", "[Factcheck: 1:1 Source Number · Code Verifier]")}</h4>
        {d && (
          <>
            <p className={d.problems.length ? "bad" : "good"}>
              {t("검증 상태: ", "Check status: ")}
              {d.problems.length
                ? t(
                    `모델 초안 불일치 ${d.problems.length}건 → 사실 줄 템플릿으로 대체 (G4_INTERCEPT_DOC_CORRUPTION)`,
                    `${d.problems.length} model draft ${d.problems.length === 1 ? "mismatch" : "mismatches"} → replaced with fact-line template (G4_INTERCEPT_DOC_CORRUPTION)`,
                  )
                : t("✓ 원문 수치 100% 일치", "✓ Source figures 100% match")}
            </p>
            <ul className="facts">
              {d.numbers.map(([n, ok], i) => (
                <li key={i} className={ok ? "good" : "bad"}>
                  [{n}] {ok ? t("사실 줄과 일치 ✓", "Matches fact line ✓") : t("원문에 없음 ✗", "Not in source ✗")}
                </li>
              ))}
              {d.problems.map((p, i) => (
                <li key={`p${i}`} className="bad">
                  {p}
                </li>
              ))}
            </ul>
          </>
        )}
        <InterlockPanel
          detail={detail}
          supervisor={supervisor}
          onChanged={(m) => {
            setReply(m);
            onDecided(m);
          }}
        />
        <h4>
          [{d?.high_risk ? t("고위험 계통 조작 승인 (policy.rs 안전 가드레일)", "High-Risk System Operation Approval (policy.rs safety guardrail)") : t(`${terms.supervisor} 확정`, `${terms.supervisor} confirmation`)}]
        </h4>
        {d?.high_risk && (
          <>
            <p className="muted">{t(`고위험 조치: ${d.risk_actions.join(", ")}`, `High-risk actions: ${d.risk_actions.join(", ")}`)}</p>
            <label className="field">
              {t("근거 절차서 번호", "Procedure ref. no.")}
              <input value={ref} onChange={(e) => setRef(e.target.value)} />
            </label>
            <label className="check">
              <input type="checkbox" checked={cosign} onChange={(e) => setCosign(e.target.checked)} />
              {t(`${terms.supervisor}(파트장) 공동 결재를 받았습니다 (team_lead_cosign)`, `${terms.supervisor} (Team Lead) co-sign obtained (team_lead_cosign)`)}
            </label>
            <label className="field">
              {t("오경보 입증 현장 증빙 스캔 (장)", "False-alarm field evidence scans (pages)")}
              <input type="number" min={0} value={scans} onChange={(e) => setScans(Number(e.target.value))} />
            </label>
          </>
        )}
        <label className="field">
          {t("현장 재점검 요청 메모", "Field recheck request note")}
          <input
            value={note}
            onChange={(e) => setNote(e.target.value)}
            placeholder={
              terms.code === "KKS"
                ? t("예: 2층 급수조절밸브 포지셔너 현장 확인", "e.g. Check feedwater control valve positioner on 2F")
                : t("예: 2층 CDU 유량조절밸브 액추에이터 현장 확인", "e.g. Check CDU flow control valve actuator on 2F")
            }
          />
        </label>
        <div className="actions">
          {d?.high_risk && (
            <button className="plain" disabled={busy} onClick={() => act("approve_false_positive")}>
              {t("고위험 예외승인 (오경보)", "High-risk exception approval (false alarm)")}
            </button>
          )}
          <button className="plain" disabled={busy} onClick={() => act("recheck")}>
            {t("현장 재점검요구", "Request field recheck")}
          </button>
          <button className="plain" disabled={busy} onClick={() => act("reject")}>
            {t("반려", "Reject")}
          </button>
          <button className="danger-fill" disabled={busy || !!pending} onClick={() => act(d?.high_risk ? "approve_procedure" : "confirm")}>
            {pending ? t(`🔒 조치확정 (Step ${pending.no} 필요)`, `🔒 Confirm Action (Step ${pending.no} required)`) : t("🚨 조치확정", "🚨 Confirm Action")}
          </button>
        </div>
        {d?.high_risk && (
          <button className="plain small" disabled={busy} onClick={() => act("confirm")}>
            {t("(시험) 공동 결재 없이 확정 시도", "(Test) Try confirming without co-sign")}
          </button>
        )}
        {pending && (
          <button className="plain small" disabled={busy} onClick={() => act(d?.high_risk ? "approve_procedure" : "confirm")}>
            {t("(시험) 선행 절차를 건너뛰고 확정 시도", "(Test) Try confirming without prerequisite steps")}
          </button>
        )}
        {reply && <p className="note">{reply}</p>}
      </section>
    </div>
  );
}

const COLORS = ["#ff6b3d", "#f6c453", "#4fd1c5", "#7aa2ff", "#c084fc", "#9ae66e"];

function Sds({ detail, plant }: { detail: Detail; plant: Plant }) {
  const t = useT();
  const a = detail.analysis;
  const root = a.substantive?.causal?.root;
  const origin = root?.at_ms ?? a.chain[0]?.at_ms ?? a.sds.center_ms;
  const traces = useMemo<Trace[]>(() => {
    const out: Trace[] = [];
    const add = (tag: string, title: string, channel?: string) => {
      if (!out.some((x) => x.tag === tag && x.channel === channel)) out.push({ tag, title, color: COLORS[out.length % COLORS.length], channel });
    };
    const label = (tag: string) => {
      const s = plant.sensors.find((x) => x.tag === tag);
      const e = plant.equipment.find((x) => x.kks === s?.kks);
      return `${e?.name ?? ""} ${s?.label ?? ""} (${tag}, ${s?.unit ?? ""})`;
    };
    for (const p of a.substantive?.causal?.propagation ?? []) {
      if (p.causal_match || p.backflow) add(p.upstream_tag, `[${p.backflow ? t("역류", "Backflow") : `D -${p.hops}`}] ${label(p.upstream_tag)}`);
    }
    if (a.route.tag) {
      const chs = detail.timeline.channels[a.route.tag];
      if (chs) Object.keys(chs).forEach((c) => add(a.route.tag!, `${t(`[채널 ${c}]`, `[Ch ${c}]`)} ${label(a.route.tag!)}`, c));
      else add(a.route.tag, `[D 0] ${label(a.route.tag)}`);
    }
    return out.slice(0, 5);
  }, [a, detail, plant, t]);
  const markers = a.chain.map((ev) => ({
    at_ms: ev.at_ms,
    label: `${signedSecs(ev.at_ms, origin)} ${shortTag(ev.tag)} ${ev.kind === "limit" ? t("한계", "limit") : ev.direction === "Up" ? "↑" : "↓"}`,
    color: ev.kind === "limit" ? "#ff4d4f" : "#f6c453",
  }));
  const from = Math.max(detail.timeline.t0_ms, origin - 20_000);
  const to = Math.min(detail.timeline.now_ms + 15_000, detail.timeline.t0_ms + 150_000);
  return (
    <div className="two-col wide-left">
      <section>
        <div className="timeline-strip">
          {t(`[SDS 스냅샷 ID: ${a.sds.sds_id}] 트리거:`, `[SDS Snapshot ID: ${a.sds.sds_id}] Trigger:`)} {a.sds.trigger_kind}{" "}
          {a.trigger.slope !== null ? t(`· 기울기 ${a.trigger.slope?.toFixed(2)}/s`, `· slope ${a.trigger.slope?.toFixed(2)}/s`) : ""}{" "}
          {a.trigger.ttf_secs !== null ? t(`· 트립 도달 예상 잔여 ${a.trigger.ttf_secs?.toFixed(0)}초`, `· est. ${a.trigger.ttf_secs?.toFixed(0)}s to trip`) : ""}
        </div>
        <WaveChart
          timeline={detail.timeline}
          traces={traces}
          sensors={plant.sensors}
          from={from}
          to={to}
          markers={[{ at_ms: origin, label: t("t=0 발원", "t=0 origin"), color: "#ffffff" }, ...markers]}
          cursor={detail.timeline.now_ms}
        />
        {root && (
          <div className="callout adverse">
            {t(
              `📌 [시계열 인과 시차(Δt) 판정] 관측 시차 Δt = +${root.lag_secs.toFixed(1)}초 (배관 허용 ${root.lag_range[0]} ~ ${root.lag_range[1]}초 안) → ${plant.equipment.find((e) => e.kks === a.route.kks)?.name ?? ""} 이상은 ${root.lag_secs.toFixed(1)}초 전 ${plant.equipment.find((e) => e.kks === root.kks)?.name ?? ""} 변화가 원인`,
              `📌 [Time-Series Causal Lag (Δt) Verdict] Observed lag Δt = +${root.lag_secs.toFixed(1)}s (within pipe tolerance ${root.lag_range[0]} ~ ${root.lag_range[1]}s) → the ${plant.equipment.find((e) => e.kks === a.route.kks)?.name ?? ""} anomaly is caused by the ${plant.equipment.find((e) => e.kks === root.kks)?.name ?? ""} change ${root.lag_secs.toFixed(1)}s earlier`,
            )}
          </div>
        )}
        <h4>{t("[60초 링버퍼 지표]", "[60 s Ring-Buffer Metrics]")}</h4>
        <table className="grid">
          <thead>
            <tr>
              <th>{t("센서", "Sensor")}</th>
              <th>{t("변곡 시각", "Inflection")}</th>
              <th>{t("기울기 /s", "Slope /s")}</th>
              <th>{t("가속도", "Accel.")}</th>
              <th>EWMA</th>
              <th>TTF</th>
            </tr>
          </thead>
          <tbody>
            {a.metrics.map((m) => (
              <tr key={m.sensor_tag}>
                <td className="mono">{m.sensor_tag}</td>
                <td>{m.inflection_at_ms ? signedSecs(m.inflection_at_ms, origin) : "-"}</td>
                <td>{fixed(m.slope_per_sec_milli / 1000, 2)}</td>
                <td>{fixed(m.accel_milli / 1000, 2)}</td>
                <td>{fmt(m.ewma_milli / 1000)}</td>
                <td>{m.estimated_ttf_secs !== null ? t(`${m.estimated_ttf_secs}초`, `${m.estimated_ttf_secs}s`) : "-"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
      <section>
        <h4>{t("[SDS 시공간 직교 분할 검증 (Zoro-1 판단)]", "[SDS Spatio-Temporal Orthogonal Split Check (Zoro-1 Verdict)]")}</h4>
        <p className="muted">{t("복합 알람을 직교 기준으로 쪼개어 묻고, 가장 낮은 확률을 최종 확신도로 씁니다.", "Splits a compound alarm into orthogonal questions and uses the lowest probability as the final confidence.")}</p>
        {a.sds.decomposed_confidences.length === 0 && <p className="callout">{t("축별 질문 기록이 없습니다 (모델 없음 또는 절차 판정).", "No per-axis question log (no model, or procedural verdict).")}</p>}
        {a.sds.decomposed_confidences.map(([axis, p]) => (
          <div key={axis} className="bar-row">
            <span>{axis}</span>
            <div className="bar-track">
              <div className="bar-fill" style={{ width: `${p * 100}%` }} />
            </div>
            <b className="mono">{p.toFixed(3)}</b>
          </div>
        ))}
        <p className="callout">
          {t("➔ SDS 최종 채택 확신도 (Min): ", "➔ SDS final confidence (Min): ")}
          {a.sds.min_confidence !== null ? a.sds.min_confidence.toFixed(3) : t("없음", "none")}
        </p>
        <h4>{t("[게이트별 SDS 상태 전이 코드]", "[SDS State Transition Codes by Gate]")}</h4>
        <div className="chips">
          {a.sds.gate_state_vector.map((c, i) => (
            <span key={i} className="chip mono">
              {c}
            </span>
          ))}
        </div>
        <h4>{t("[신호 전파 체인 (Event Trace)]", "[Signal Propagation Chain (Event Trace)]")}</h4>
        <ul className="facts">
          {a.chain.map((ev, i) => (
            <li key={i}>
              • {signedSecs(ev.at_ms, origin)} : {ev.tag} {ev.label}{" "}
              {ev.kind === "limit" ? t("한계 돌파", "limit breached") : ev.direction === "Up" ? t("상승", "rising") : t("하락", "falling")}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function ArchiveView({ doc }: { doc: ArchiveDoc }) {
  const t = useT();
  return (
    <div className="two-col">
      <section>
        <h4>
          {doc.archive_id} · {doc.final_code} {doc.auto ? t("[자동확정]", "[Auto-confirmed]") : ""}
        </h4>
        <pre className="aop">{doc.mcr_doc}</pre>
      </section>
      <section>
        <h4>{t("현장 단말 안내", "Field Terminal Notice")}</h4>
        <pre className="aop">{doc.field_notice || t("(현장 보고 없음)", "(no field report)")}</pre>
        <dl className="props">
          <dt>{t("SDS 블랙박스 봉인", "SDS Black-Box Seal")}</dt>
          <dd className="mono">{doc.seal}</dd>
          <dt>{t("보존 기한", "Retain Until")}</dt>
          <dd>{doc.retention_until}</dd>
          <dt>{t("특별 감사 표시", "Special Audit Flag")}</dt>
          <dd>{doc.special_audit_flag ? t("예 (감사실 보고)", "Yes (reported to audit office)") : t("아니오", "No")}</dd>
        </dl>
      </section>
    </div>
  );
}

/** 상세 서랍 제목 (그릴 때마다 불러 화면 언어를 따릅니다) */
const TITLES = (): Record<string, string> => ({
  "1": tr("🔍 [Stage 1 & 2 상세] SigLIP2 시각 앵커 조각 계획 & gocr + Lattice 대장 교정 결과", "🔍 [Stage 1 & 2 Detail] SigLIP2 Visual-Anchor Crop Plan & gocr + Lattice Register Correction"),
  "2": tr("🔍 [Stage 1 & 2 상세] SigLIP2 시각 앵커 조각 계획 & gocr + Lattice 대장 교정 결과", "🔍 [Stage 1 & 2 Detail] SigLIP2 Visual-Anchor Crop Plan & gocr + Lattice Register Correction"),
  "3": tr("⚖️ [Stage 3 상세] Gate 1 형식 심사 & Gate 2 3중 진실 정합성 교차 대조", "⚖️ [Stage 3 Detail] Gate 1 Format Review & Gate 2 Triple-Truth Cross-Check"),
  "4": tr("🧠 [Stage 4 상세] Gate 3 마인드맵 파급 분석 & Zoro-1 포인터 판단 (jev_head)", "🧠 [Stage 4 Detail] Gate 3 Mind Map Propagation Analysis & Zoro-1 Pointer Verdict (jev_head)"),
  "5": tr("✍️ [Stage 5 상세] Zoro-1 글쓰기 조치 절차서 초안 · Factcheck 수치 검증 · 최종 결재", "✍️ [Stage 5 Detail] Zoro-1 Action Procedure Draft · Factcheck Figure Check · Final Approval"),
  sds: tr("⏱️ [시계열 & SDS 심층 분석기] 다중 계통 시계열 파형 동기화 (Δt) 및 SDS 직교 분할 검증", "⏱️ [Time Series & SDS Deep Analyzer] Multi-System Waveform Sync (Δt) & SDS Orthogonal Split Check"),
  archive: tr("🗄️ 보관 문서 (Gate 5)", "🗄️ Archived Record (Gate 5)"),
});

/** 우측 통합 결재창의 짧은 결재 (개선안 Part 1 우측 아래: 표준 조치 요약 + [재점검 지시] [확정]).
 *  고위험이면 파트장 공동 결재와 절차서 번호(초안의 번호로 미리 채움)가 함께 갑니다. 오경보 처리 · 시험 버튼은 Stage 5 상세에 있습니다. */
export function QuickApproval({ detail, supervisor, onDecided, onOpenDetail }: { detail: Detail; supervisor: string; onDecided: (m: string) => void; onOpenDetail: () => void }) {
  const terms = useTerms();
  const t = useT();
  const d = detail.analysis.draft;
  const pending = detail.analysis.interlock?.steps.find((s) => s.status !== "passed") ?? null;
  const [cosign, setCosign] = useState(false);
  const [busy, setBusy] = useState(false);
  const [reply, setReply] = useState<string | null>(null);
  useEffect(() => {
    setCosign(false);
    setReply(null);
  }, [detail.id]);
  const act = async (action: string, note = "") => {
    setBusy(true);
    try {
      const r = await invoke<{ ok: boolean; message: string }>("decide_incident", { input: { id: detail.id, supervisor_id: supervisor, action, team_lead_cosign: cosign, procedure_ref: d?.aop_id ?? "", evidence_scans: 0, note } });
      setReply(r.message);
      onDecided(r.message);
    } catch (e) {
      setReply(String(e));
    } finally {
      setBusy(false);
    }
  };
  const awaiting = detail.state === "awaiting_field";
  return (
    <div className="quick-approval">
      <h4>
        {t(
          `[표준 조치 요약 ${d ? (d.problems.length ? "(Fact ✗ 템플릿)" : "(Fact ✓)") : ""}]`,
          `[Standard Action Summary${d ? (d.problems.length ? " (Fact ✗ template)" : " (Fact ✓)") : ""}]`,
        )}
      </h4>
      {d ? (
        <ol className="quick-steps">
          {d.steps.map((s, i) => (
            <li key={i}>
              {i + 1}. {s}
            </li>
          ))}
        </ol>
      ) : (
        <p className="muted">
          {t(
            `조치 절차서가 없는 판정입니다 (${detail.analysis.moc ? "Red-line 변경 반영 심사" : "기록만"}).`,
            `No action procedure for this verdict (${detail.analysis.moc ? "Red-line change review" : "record only"}).`,
          )}
        </p>
      )}
      {d?.high_risk && (
        <label className="check">
          <input type="checkbox" checked={cosign} onChange={(e) => setCosign(e.target.checked)} />
          {t(`파트장 공동 결재 (${d.aop_id}) · 고위험: ${d.risk_actions.join(", ")}`, `Team Lead co-sign (${d.aop_id}) · High-risk: ${d.risk_actions.join(", ")}`)}
        </label>
      )}
      <div className="actions">
        <button className="plain" disabled={busy} onClick={() => act("recheck", `${terms.control_room} 재점검 지시`)}>
          {t("재점검 지시", "Order Recheck")}
        </button>
        <button className="plain" disabled={busy} onClick={onOpenDetail}>
          {t("상세 결재", "Detailed Approval")}
        </button>
        <button className="danger-fill" disabled={busy || !!pending || awaiting || (!!d?.high_risk && !cosign)} onClick={() => act(d?.high_risk ? "approve_procedure" : "confirm")}>
          {awaiting
            ? t("⏳ 현장 대기", "⏳ Awaiting Field Check")
            : pending
              ? t(`🔒 Step ${pending.no} 필요`, `🔒 Step ${pending.no} required`)
              : d?.high_risk && !cosign
                ? t("🔒 공동 결재 필요", "🔒 Co-sign required")
                : t("🚨 조치확정", "🚨 Confirm Action")}
        </button>
      </div>
      {reply && <p className="note">{reply}</p>}
    </div>
  );
}

export function StageDrawer({ which, detail, archive, plant, catalog, supervisor, onClose, onDecided }: { which: DrawerKey; detail: Detail | null; archive: ArchiveDoc | null; plant: Plant | null; catalog: Record<string, VerdictInfo>; supervisor: string; onClose: () => void; onDecided: (m: string) => void }) {
  const t = useT();
  return (
    <div className="drawer">
      <header>
        <strong>{TITLES()[String(which)]}</strong>
        <span className="grow" />
        <button className="plain" onClick={onClose}>
          {t("[X 닫기]", "[X Close]")}
        </button>
      </header>
      <div className="drawer-body">
        {which === "archive" && archive && <ArchiveView doc={archive} />}
        {which !== "archive" && !detail && <p className="muted">{t("진행 중인 사건을 고르세요.", "Select an active incident.")}</p>}
        {detail && (which === 1 || which === 2) && <Scan detail={detail} />}
        {detail && which === 3 && <Truth detail={detail} supervisor={supervisor} />}
        {detail && which === 4 && <Decision detail={detail} catalog={catalog} />}
        {detail && which === 5 && <Approval detail={detail} supervisor={supervisor} onDecided={onDecided} />}
        {detail && plant && which === "sds" && <Sds detail={detail} plant={plant} />}
      </div>
    </div>
  );
}
