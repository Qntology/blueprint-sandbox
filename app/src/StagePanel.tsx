import { invoke } from "./ipc";
import { useEffect, useState } from "react";
import { useTerms } from "./domain";
import { eng, useEngineer } from "./engineer";
import { useT } from "./i18n";
import { tr } from "./i18n/lang";
import type { DrawerKey } from "./OfficerView";
import { clock, fmt, mmss, STAGE_LABEL } from "./plant";
import type { ArchiveDoc, Asked, Detail, InterlockStep, Lane, VerdictInfo } from "./types";

const STAGE_CLASS: Record<string, string> = { Done: "ok", Skipped: "skip", Corrected: "fix", Alert: "alert", Waiting: "wait", Blocked: "block" };

function duration(ms: number): string {
  if (ms <= 0) return "-";
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
  return ms < 1000 ? `${ms.toFixed(1)}ms` : tr(`${(ms / 1000).toFixed(2)}초`, `${(ms / 1000).toFixed(2)}s`);
}

/** 단계 카드에 붙일 프로세스 마이닝 레인 (개선안 3단계 3번: 단계 바 + 마이닝 레인 두 줄을 한 줄로) */
const STAGE_LANES: Record<number, string[]> = { 1: ["gate01"], 2: ["gate01"], 3: ["gate2"], 4: ["gate3"], 5: ["gate45", "approval"] };

/** 단계 카드: Stage 1 · 2 (크롭 · 판독)는 같은 상세를 여는 한 칸에 두 상태를 함께 보입니다 */
const STAGE_CARDS: number[][] = [[1, 2], [3], [4], [5]];
/** 두 단계 중 더 눈여겨볼 상태 (막힘 > 경보 > 대기 > 교정 > 완료 > 건너뜀) */
const STATUS_RANK: Record<string, number> = { Blocked: 5, Alert: 4, Waiting: 3, Corrected: 2, Done: 1, Skipped: 0 };

/** 파이프라인 줄 (Stage 1 ~ 5, 카드 넷). 카드마다 그 단계 레인의 건수 · 최대 대기를 함께 적습니다. 누르면 단계 상세가 열립니다.
 *  2차 개선안: 하단 허브 맨 윗줄(noto-sandbox 의 퀘스트 줄 자리)에 한 줄로 들어가고, 단계 요약은 마우스를 올리면 보입니다. */
export function StageBar({ detail, onOpen, lanes }: { detail: Detail | null; onOpen: (k: DrawerKey) => void; lanes: Lane[] }) {
  const terms = useTerms();
  const t = useT();
  const stages = detail?.analysis.stages ?? [];
  const lane = (ns: number[]) => {
    const ls = lanes.filter((l) => ns.some((n) => STAGE_LANES[n].includes(l.key)));
    return { count: Math.max(0, ...ls.map((l) => l.count)), wait: Math.max(0, ...ls.map((l) => (l.count > 0 ? l.max_wait_secs : 0))) };
  };
  const fallback = (n: number) =>
    [
      t("SigLIP2 크롭", "SigLIP2 Crop"),
      t("gocr 판독 + 대장 교정", "gocr Read + Register Fix"),
      t("Gate 1~2 원장 대조", "Gate 1~2 Ledger Check"),
      t("Gate 3 + Zoro-1 판단", "Gate 3 + Zoro-1 Verdict"),
      t(`Zoro-1 ${terms.procedure} 초안`, `Zoro-1 ${terms.procedure} Draft`),
    ][n - 1];
  return (
    <div className="stagebar">
      <span
        className="stagebar-title"
        title={detail ? t(`${detail.analysis.route.kks} 파이프라인 · 마이닝 레인`, `${detail.analysis.route.kks} Pipeline · Mining Lanes`) : t("파이프라인 · 마이닝 레인", "Pipeline · Mining Lanes")}
      >
        {t("🧩 파이프라인", "🧩 Pipeline")}
      </span>
      {STAGE_CARDS.map((ns) => {
        const ss = ns.map((n) => stages.find((x) => x.stage === n));
        const shown = ss.filter((s) => s) as NonNullable<(typeof ss)[number]>[];
        const worst = shown.reduce<(typeof shown)[number] | null>((w, s) => (!w || (STATUS_RANK[s.status] ?? 0) > (STATUS_RANK[w.status] ?? 0) ? s : w), null);
        const l = lane(ns);
        const name = ns.length > 1 ? t("크롭 · 판독", "Crop · Read") : (ss[0]?.name ?? fallback(ns[0]));
        return (
          <button
            key={ns[0]}
            data-stage={ns[0]}
            className={`stage-card ${worst ? STAGE_CLASS[worst.status] : "skip"}`}
            disabled={!detail}
            onClick={() => onOpen(ns[0] as DrawerKey)}
            title={ns
              .map((n, i) =>
                n === 5 && detail?.drafting
                  ? t(`Stage 5. ${ss[i]?.name ?? fallback(n)}: Zoro-1 이 초안을 쓰는 중 (지금은 사실 줄 템플릿)`, `Stage 5. ${ss[i]?.name ?? fallback(n)}: Zoro-1 is drafting (fact-line template for now)`)
                  : `Stage ${n}. ${ss[i]?.name ?? fallback(n)}: ${ss[i]?.summary ?? "-"} (${duration(ss[i]?.ms ?? 0)})`,
              )
              .concat(t(`레인 ${l.count}건${l.wait > 0 ? ` · 최대 대기 ${mmss(l.wait)}` : ""}`, `Lane: ${l.count}${l.wait > 0 ? ` · Max wait ${mmss(l.wait)}` : ""}`))
              .join("\n")}
          >
            <strong>
              Stage {ns.join("·")}. {name}
            </strong>
            <em>
              <span>{ss.map((s, i) => `[${ns[i] === 5 && detail?.drafting ? t("✍️ 작성 중", "✍️ Drafting") : s ? STAGE_LABEL[s.status] : t("대기", "Waiting")}]`).join(" ")}</span>
              <span className={`lane-badge ${l.wait > 0 ? "hold" : ""}`}>
                {t(`${l.count}건`, `${l.count}`)}
                {l.wait > 0 ? ` · ⏳ ${mmss(l.wait)}` : ""}
              </span>
            </em>
          </button>
        );
      })}
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
  const engineer = useEngineer();
  const stages = detail.analysis.stages;
  if (detail.reports.length === 0) {
    return (
      <p className="callout">
        {t(
          `이 사건은 현장 스캔이 없는 신호 알람이라 문서 모델(SigLIP2 · gocr)을 GPU 에 올리지 않고 바로 Zoro-1 판단으로 갔습니다 (${engineer ? "SharedResidency: " : ""}한 번에 한 모델).`,
          `This incident is a signal alarm with no field scan, so the document models (SigLIP2 · gocr) were not loaded onto the GPU and it went straight to the Zoro-1 verdict (${engineer ? "SharedResidency: " : ""}one model at a time).`,
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
        {/* 예전 체크리스트는 어디에도 보내지 않고 닫으면 사라져(확정 조건도 아님) 목록으로 바꿨습니다 (UI 단순화 U9).
            검출 사항을 본 확인은 결재 HUD 의 확정 · 반려 결정 하나로 남습니다 (감사 기록). */}
        <h4>{t("[Gate 2 검출 사항]", "[Gate 2 Findings]")}</h4>
        <ul className="findings">
          {(detail.notes.length ? detail.notes : [a.reason]).map((n, i) => (
            <li key={i}>{n}</li>
          ))}
        </ul>
        <p className="muted">{t(`검출 사항을 본 뒤 ${terms.supervisor} 결정은 오른쪽 결재 HUD 에서 합니다.`, `After reviewing the findings, the ${terms.supervisor} decides in the approval HUD on the right.`)}</p>
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
  const engineer = useEngineer();
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
          {engineer
            ? t(
                "[동작 원리] 백본이 정제된 사실(근무자 정보 제외)만 읽고, jev_head 가 선택지 위치(닫는 따옴표)의 은닉 상태로 점수를 매깁니다 (0-Generation)",
                "[How it works] The backbone reads only cleaned facts (worker info excluded), and jev_head scores each option from the hidden state at its position (the closing quote) (0-Generation)",
              )
            : t(
                "[동작 원리] 모델은 정제된 사실(근무자 정보 제외)만 읽고 선택지마다 점수를 매겨 고릅니다 (글을 새로 쓰지 않음)",
                "[How it works] The model reads only cleaned facts (worker info excluded) and scores each option to choose one (it writes no new text)",
              )}
        </div>
        <h4>{t(`[모델에 입력된 표준 JSON 사실${engineer ? " (PointerPrompt)" : ""}]`, `[Standard JSON Facts Given to the Model${engineer ? " (PointerPrompt)" : ""}]`)}</h4>
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
            `📌 [판정 결합 규칙${engineer ? " (verdict.rs)" : ""}] 판정은 규칙이 정합니다 (${ruleLabel}). 모델이 같은 쪽을 고르면 축별 최솟값이 확신도가 됩니다:`,
            `📌 [Verdict Combination Rule${engineer ? " (verdict.rs)" : ""}] The rules set the verdict (${ruleLabel}). If the model picks the same side, the lowest per-axis score becomes the confidence:`,
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
 *  `compact` 는 우측 결재창의 신호등 게이트 (개선안 3단계 2번: 마이닝 · 인터록 · 결재를 한 화면에).
 *  `readOnly` 는 Stage 5 서랍의 상태 보기 (값 입력 · 무전 확인은 결재 HUD 한 곳에서만 합니다). */
export function InterlockPanel({ detail, supervisor, onChanged, compact = false, readOnly = false }: { detail: Detail; supervisor: string; onChanged: (m: string) => void; compact?: boolean; readOnly?: boolean }) {
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
          {!readOnly && s.status !== "passed" && s.check.kind !== "loto" && (
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
          {!readOnly && s.status !== "passed" && s.check.kind === "loto" && (
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

/** Stage 5 상세: Zoro-1 조치 절차서 초안 · Factcheck 대조 · 인터록 상태 (읽기 전용). 결재는 오른쪽 결재 HUD 한 곳에서 합니다 (UI 단순화 U1). */
function Draft({ detail, supervisor }: { detail: Detail; supervisor: string }) {
  const terms = useTerms();
  const t = useT();
  useEngineer(); // 코드 이름 꼬리(eng)가 설정을 따라 다시 그려지게
  const d = detail.analysis.draft;
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">
          {t("[듀얼 헤드] 판단과 같은 몸통에 임베딩 표를 더 올려 글을 씁니다", "[Dual Head] Writes text by adding an embedding table to the same trunk as the verdict")}{" "}
          {d
            ? `(${
                detail.drafting
                  ? t("✍️ 모델이 쓰는 중 · 지금은 템플릿", "✍️ model drafting · template for now")
                  : d.by_model
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
                    `모델 초안 불일치 ${d.problems.length}건 → 사실 줄 템플릿으로 대체${eng(" (G4_INTERCEPT_DOC_CORRUPTION)")}`,
                    `${d.problems.length} model draft ${d.problems.length === 1 ? "mismatch" : "mismatches"} → replaced with fact-line template${eng(" (G4_INTERCEPT_DOC_CORRUPTION)")}`,
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
        <InterlockPanel detail={detail} supervisor={supervisor} onChanged={() => {}} readOnly />
        {d?.high_risk && <p className="muted">{t(`고위험 조치: ${d.risk_actions.join(", ")}`, `High-risk actions: ${d.risk_actions.join(", ")}`)}</p>}
        <p className="callout">
          {t(
            `결재(${terms.supervisor} 확정 · 공동 결재 · 재점검 · 반려)는 오른쪽 결재 HUD 에서 합니다.`,
            `Approval (${terms.supervisor} confirmation · co-sign · recheck · reject) is done in the approval HUD on the right.`,
          )}
        </p>
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
  "4": tr(`🧠 [Stage 4 상세] Gate 3 마인드맵 파급 분석 & Zoro-1 포인터 판단${eng(" (jev_head)")}`, `🧠 [Stage 4 Detail] Gate 3 Mind Map Propagation Analysis & Zoro-1 Pointer Verdict${eng(" (jev_head)")}`),
  "5": tr("✍️ [Stage 5 상세] Zoro-1 글쓰기 조치 절차서 초안 · Factcheck 수치 검증 (결재는 오른쪽 HUD)", "✍️ [Stage 5 Detail] Zoro-1 Action Procedure Draft · Factcheck Figure Check (approve in the right HUD)"),
  archive: tr("🗄️ 보관 문서 (Gate 5)", "🗄️ Archived Record (Gate 5)"),
});

/** 결재 (오른쪽 결재 HUD 한 곳, UI 단순화 U1): 표준 조치 요약 · 공동 결재 · 근거 절차서 번호 · [재점검 지시] [조치확정],
 *  [더 보기] 에 반려 · 고위험 예외승인(오경보) · 증빙 장수 · 재점검 메모.
 *  확정은 의도 하나(`confirm`)만 보내고 고위험 절차 승인 갈래는 백엔드가 정합니다 (P1). 근거 절차서 번호는 도장 찍기 방지 대조라
 *  미리 채우지 않습니다: 초안 전문에서 번호를 보고 사람이 칩니다 (Gate 3 이 초안 번호와 대조).
 *  확정 막힘(현장 대기 · 선행 절차 · 공동 결재 · 번호)은 단추가 미리 막고 백엔드 가드레일이 한 번 더 막습니다.
 *  시연 모드에는 가드레일을 일부러 건드려 보는 시험 단추가 붙습니다 (영상 · E2E). */
export function QuickApproval({
  detail,
  supervisor,
  demo,
  onDecided,
  onOpenDraft,
}: {
  detail: Detail;
  supervisor: string;
  demo: boolean;
  onDecided: (m: string, ok: boolean, finalized: boolean) => void;
  onOpenDraft: () => void;
}) {
  const terms = useTerms();
  const t = useT();
  const d = detail.analysis.draft;
  const pending = detail.analysis.interlock?.steps.find((s) => s.status !== "passed") ?? null;
  const awaiting = detail.state === "awaiting_field";
  const [cosign, setCosign] = useState(false);
  const [more, setMore] = useState(false);
  const [ref, setRef] = useState("");
  const [scans, setScans] = useState(0);
  const [note, setNote] = useState("");
  const [busy, setBusy] = useState(false);
  const [reply, setReply] = useState<string | null>(null);
  useEffect(() => {
    setCosign(false);
    setMore(false);
    setRef("");
    setScans(0);
    setNote("");
    setReply(null);
  }, [detail.id]);
  /** `over.cosign` 은 시험 단추(공동 결재 없이 확정 시도)가 체크 상태와 상관없이 보낼 때 */
  const act = async (action: string, over: { cosign?: boolean } = {}) => {
    setBusy(true);
    try {
      const r = await invoke<{ ok: boolean; message: string; finalized: boolean }>("decide_incident", {
        input: { id: detail.id, supervisor_id: supervisor, action, team_lead_cosign: over.cosign ?? cosign, procedure_ref: ref.trim(), evidence_scans: scans, note },
      });
      setReply(r.message);
      onDecided(r.message, r.ok, r.finalized);
    } catch (e) {
      setReply(String(e));
      onDecided(String(e), false, false);
    } finally {
      setBusy(false);
    }
  };
  const needCosign = !!d?.high_risk && !cosign;
  const needRef = !!d?.high_risk && !ref.trim();
  // 모델 초안을 뒤에서 쓰는 중이면 (P6) 지금 보이는 것은 사실 줄 템플릿이고, 확정은 초안이 나온 뒤에 (보관 문서에 들어갈 글)
  const drafting = !!detail.drafting;
  return (
    <div className="quick-approval">
      <h4 className="qa-head">
        <span>
          {drafting
            ? t("[표준 조치 요약 (✍️ Zoro-1 초안 작성 중 · 템플릿 표시)]", "[Standard Action Summary (✍️ Zoro-1 drafting · showing template)]")
            : t(
                `[표준 조치 요약 ${d ? (d.problems.length ? "(Fact ✗ 템플릿)" : "(Fact ✓)") : ""}]`,
                `[Standard Action Summary${d ? (d.problems.length ? " (Fact ✗ template)" : " (Fact ✓)") : ""}]`,
              )}
        </span>
        <button className="plain small qa-draft" onClick={onOpenDraft}>
          {t("📄 초안 전문", "📄 Full draft")}
        </button>
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
        <>
          <label className="check cosign">
            <input type="checkbox" checked={cosign} onChange={(e) => setCosign(e.target.checked)} />
            {t(`파트장 공동 결재 · 고위험: ${d.risk_actions.join(", ")}`, `Team Lead co-sign · High-risk: ${d.risk_actions.join(", ")}`)}
          </label>
          <label className="field qa-ref-row">
            {t("근거 절차서 번호 (초안의 번호를 직접 입력)", "Procedure no. (type the draft's number)")}
            <input className="qa-ref" value={ref} spellCheck={false} placeholder={t("초안의 절차서 번호", "Procedure no. from the draft")} onChange={(e) => setRef(e.target.value)} />
          </label>
        </>
      )}
      <div className="actions">
        <button className="plain recheck" disabled={busy} onClick={() => act("recheck")}>
          {t("재점검 지시", "Order Recheck")}
        </button>
        <button className="plain qa-more-toggle" aria-expanded={more} onClick={() => setMore((m) => !m)}>
          {more ? t("접기 ▴", "Less ▴") : t("더 보기 ▾", "More ▾")}
        </button>
        <button className="danger-fill confirm" disabled={busy || !!pending || awaiting || drafting || needCosign || needRef} onClick={() => act("confirm")}>
          {awaiting
            ? t("⏳ 현장 대기", "⏳ Awaiting Field Check")
            : pending
              ? t(`🔒 Step ${pending.no} 필요`, `🔒 Step ${pending.no} required`)
              : drafting
                ? t("✍️ 초안 작성 중", "✍️ Drafting")
                : needCosign
                  ? t("🔒 공동 결재 필요", "🔒 Co-sign required")
                  : needRef
                    ? t("🔒 절차서 번호 필요", "🔒 Procedure no. required")
                    : t("🚨 조치확정", "🚨 Confirm Action")}
        </button>
      </div>
      {more && (
        <div className="qa-more">
          {d?.high_risk && (
            <label className="field">
              {t("오경보 입증 현장 증빙 스캔 (장)", "False-alarm field evidence scans (pages)")}
              <input className="qa-scans" type="number" min={0} value={scans} onChange={(e) => setScans(Number(e.target.value))} />
            </label>
          )}
          <label className="field">
            {t("현장 재점검 요청 메모", "Field recheck request note")}
            <input
              className="qa-note"
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
            <button className="plain qa-reject" disabled={busy} onClick={() => act("reject")}>
              {t("반려", "Reject")}
            </button>
            {d?.high_risk && (
              <button className="plain qa-false-positive" disabled={busy} onClick={() => act("approve_false_positive")}>
                {t("고위험 예외승인 (오경보)", "High-risk exception approval (false alarm)")}
              </button>
            )}
          </div>
        </div>
      )}
      {demo && (d?.high_risk || pending || awaiting) && (
        <div className="qa-tests">
          <span className="muted">{t("가드레일 시험", "Guardrail tests")}</span>
          {d?.high_risk && (
            <button className="plain small qa-test-cosign" disabled={busy} onClick={() => act("confirm", { cosign: false })}>
              {t("(시험) 공동 결재 없이 확정 시도", "(Test) Try confirming without co-sign")}
            </button>
          )}
          {pending && (
            <button className="plain small qa-test-order" disabled={busy} onClick={() => act("confirm")}>
              {t("(시험) 선행 절차를 건너뛰고 확정 시도", "(Test) Try confirming without prerequisite steps")}
            </button>
          )}
          {awaiting && (
            <button className="plain small qa-test-field" disabled={busy} onClick={() => act("confirm")}>
              {t("(시험) 현장 확인 전 확정 시도", "(Test) Try confirming before the field check")}
            </button>
          )}
        </div>
      )}
      {reply && <p className="note qa-reply">{reply}</p>}
    </div>
  );
}

export function StageDrawer({ which, detail, archive, catalog, supervisor, onClose }: { which: DrawerKey; detail: Detail | null; archive: ArchiveDoc | null; catalog: Record<string, VerdictInfo>; supervisor: string; onClose: () => void }) {
  const t = useT();
  useEngineer(); // 제목의 코드 이름 꼬리(eng)
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
        {detail && which === 5 && <Draft detail={detail} supervisor={supervisor} />}
      </div>
    </div>
  );
}
