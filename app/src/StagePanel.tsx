import { invoke } from "@tauri-apps/api/core";
import { useMemo, useState } from "react";
import type { DrawerKey } from "./OfficerView";
import { clock, fixed, fmt, signedSecs, STAGE_LABEL } from "./plant";
import type { ArchiveDoc, Asked, Detail, Plant, VerdictInfo } from "./types";
import { WaveChart, type Trace } from "./WaveChart";

const STAGE_CLASS: Record<string, string> = { Done: "ok", Skipped: "skip", Corrected: "fix", Alert: "alert", Waiting: "wait", Blocked: "block" };

function duration(ms: number): string {
  if (ms <= 0) return "-";
  if (ms < 1) return `${(ms * 1000).toFixed(0)}µs`;
  return ms < 1000 ? `${ms.toFixed(1)}ms` : `${(ms / 1000).toFixed(2)}초`;
}

/** 하단 파이프라인 바 (Stage 1 ~ 5). 누르면 단계 상세가 열립니다. */
export function StageBar({ detail, onOpen }: { detail: Detail | null; onOpen: (k: DrawerKey) => void }) {
  const stages = detail?.analysis.stages ?? [];
  return (
    <div className="stagebar">
      <span className="stagebar-title">[{detail ? `${detail.analysis.route.kks} ` : ""}파이프라인 실행 단계 · 누르면 상세]</span>
      {[1, 2, 3, 4, 5].map((n) => {
        const s = stages.find((x) => x.stage === n);
        return (
          <button key={n} className={`stage-card ${s ? STAGE_CLASS[s.status] : "skip"}`} disabled={!detail} onClick={() => onOpen(n as DrawerKey)}>
            <strong>
              Stage {n}. {s?.name ?? ["SigLIP2 크롭", "gocr 판독 + 대장 교정", "Gate 1~2 원장 대조", "Gate 3 + Zoro-1 판단", "Zoro-1 AOP 초안"][n - 1]}
            </strong>
            <span>{s?.summary ?? "-"}</span>
            <em>
              ({duration(s?.ms ?? 0)}) [{s ? STAGE_LABEL[s.status] : "대기"}]
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
  const stages = detail.analysis.stages;
  if (detail.reports.length === 0) {
    return (
      <p className="callout">
        이 사건은 현장 스캔이 없는 신호 알람이라 문서 모델(SigLIP2 · gocr)을 GPU 에 올리지 않고 바로 Zoro-1 판단으로 갔습니다 (SharedResidency: 한 번에 한 모델).
      </p>
    );
  }
  return (
    <>
      <div className="timeline-strip">
        [GPU 상주 타임라인] <b>규칙 · SigLIP2 비전</b> {duration(stages[0]?.ms ?? 0)} ➔ <b>gocr 비전 → 언어</b> {duration(stages[1]?.ms ?? 0)}
      </div>
      {detail.reports.map((r, i) => (
        <section key={i} className="card">
          <h4>
            {r.kind} · {r.taken_at} · {r.worker} ({r.worker_id}) {r.signed ? "· 서명 검출" : "· 서명 없음"} {r.permit_confirmed ? "· PTW/LOTO 확인" : ""}
          </h4>
          <dl className="props">
            <dt>설비 KKS</dt>
            <dd className="mono">{r.kks}</dd>
            <dt>센서 태그</dt>
            <dd className="mono">{r.tag}</dd>
            {r.drawing && (
              <>
                <dt>도면</dt>
                <dd>
                  {r.drawing} {r.drawing_rev}
                </dd>
              </>
            )}
            {r.moc_no && (
              <>
                <dt>변경번호</dt>
                <dd className="mono">{r.moc_no}</dd>
                <dt>변경 사양</dt>
                <dd>
                  {fmt(r.old_value)} → {fmt(r.reading)} {r.unit}
                </dd>
              </>
            )}
            {!r.moc_no && (
              <>
                <dt>현장 실측치</dt>
                <dd>
                  {fmt(r.reading)} {r.unit}
                </dd>
              </>
            )}
            {r.excuse && (
              <>
                <dt>현장 소명</dt>
                <dd>{r.excuse}</dd>
              </>
            )}
          </dl>
          {r.corrections.length > 0 && (
            <table className="grid">
              <thead>
                <tr>
                  <th>항목</th>
                  <th>gocr 원문</th>
                  <th>대장 교정</th>
                  <th>편집 거리</th>
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
          {r.uncertain.length > 0 && <p className="note">확인할 항목: {r.uncertain.join(", ")}</p>}
        </section>
      ))}
    </>
  );
}

function stateText(v: unknown): string {
  if (typeof v === "string") return v === "Valid" ? "정상" : v === "NoData" ? "계측 없음" : v;
  if (v && typeof v === "object") {
    const [k, inner] = Object.entries(v as Record<string, Record<string, unknown>>)[0];
    if (k === "Flatline") return `값 고정 ${inner.secs}초 (${inner.value})`;
    if (k === "SpikeFiltered") return `순간 잡음 ${(inner.spikes as unknown[]).length}건 걸러 냄`;
    if (k === "Isolated") return `채널 ${inner.channel} 단독 편차${inner.field_confirmed ? " · 현장 확인" : " · 현장 대기"}`;
    if (k === "Contradiction" || k === "Hold") return String(inner.reason);
    return k;
  }
  return "-";
}

function Truth({ detail, supervisor }: { detail: Detail; supervisor: string }) {
  const a = detail.analysis;
  const [checked, setChecked] = useState<Record<number, boolean>>({});
  const [names, setNames] = useState<string | null>(null);
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">[3중 진실 우선순위] ① 현장 스캔 증빙(게이지 · Red-line) &gt; ② DCS 다중 보팅 · MOC 승인 원장 &gt; ③ 단일 센서 신고값</div>
        <table className="grid">
          <thead>
            <tr>
              <th>대조 항목</th>
              <th>① 현장 스캔본</th>
              <th>② 시드 원장</th>
              <th>③ 단일 알람</th>
              <th>일치</th>
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
          <dt>Gate 1 신호</dt>
          <dd>{a.signal ? stateText(a.signal.state) : "계측 없음"}</dd>
          {a.signal?.channels.map(([c, s]) => (
            <div key={c} className="sensor-line">
              <dt>채널 {c}</dt>
              <dd>{stateText(s)}</dd>
            </div>
          ))}
          {a.vote && (
            <>
              <dt>{a.vote.logic} 보팅</dt>
              <dd>
                {a.vote.channels.map(([c, v]) => `${c} ${fmt(v)}`).join(" · ")} → 대표값 {fmt(a.vote.voted)} · <span className="mono">{a.vote.state_code}</span>
              </dd>
            </>
          )}
          {a.drift !== null && a.drift !== undefined && (
            <>
              <dt>편차 판정</dt>
              <dd>{stateText(a.drift)}</dd>
            </>
          )}
          {a.field_checks.map((f, i) => (
            <div key={i} className="sensor-line">
              <dt>시점 동기화 대조</dt>
              <dd>
                촬영 {clock(f.taken_at_ms)} · 현장 {fmt(f.field_value)} vs 같은 시각 DCS {fmt(f.dcs_value)} (허용 ±{fmt(f.tolerance)}) {f.matches_dcs ? "✓ 일치" : "✗ 불일치"}
              </dd>
            </div>
          ))}
          {a.moc && (
            <>
              <dt>MOC 대조</dt>
              <dd>
                {a.moc.moc_no} · {stateText(a.moc.outcome)}
              </dd>
              {a.moc.stale_drawing && (
                <>
                  <dt>도면 개정</dt>
                  <dd className="bad">{a.moc.stale_drawing}</dd>
                </>
              )}
            </>
          )}
        </dl>
        <h4>[Gate 2 검출 사항 · 교대감독 확인 체크리스트]</h4>
        {(detail.notes.length ? detail.notes : [a.reason]).map((n, i) => (
          <label key={i} className="check">
            <input type="checkbox" checked={!!checked[i]} onChange={(e) => setChecked((c) => ({ ...c, [i]: e.target.checked }))} />
            {n}
          </label>
        ))}
      </section>
      <section>
        <h4>[작업자 권한 및 계통 연결망 (가림 처리)]</h4>
        {detail.reports.length === 0 && <p className="muted">현장 보고가 없습니다.</p>}
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
              .then((n) => setNames(n ? `열람 허용 (감사 기록됨): ${n.join(", ")}` : "열람 거부: 위험도 70% 미만 (감사 기록됨)"))
              .catch((e) => setNames(String(e)))
          }
        >
          🔓 작업자 실명 열람 요청
        </button>
        {names && <p className="note">{names}</p>}
        <h4>[계통 파급 경로 (마인드맵 3홉 이내)]</h4>
        {(a.substantive?.causal?.propagation ?? []).map((p, i) => (
          <p key={i} className={p.causal_match && !p.backflow ? "good" : "muted"}>
            [{p.hops}홉 상류] {p.upstream_kks} · Δt {p.observed_lag_secs.toFixed(1)}s ({p.expected_lag_range[0]}~{p.expected_lag_range[1]}s){p.backflow ? " · 역류 여파" : p.causal_match ? " · 인과 일치" : " · 시차 불일치"}
          </p>
        ))}
      </section>
    </div>
  );
}

function Decision({ detail, catalog }: { detail: Detail; catalog: Record<string, VerdictInfo> }) {
  const a = detail.analysis;
  const sub = a.substantive;
  const asked = sub?.decided.asked ?? [];
  const rule = sub?.decided.judgement.verdict;
  const trace = a.model_trace;
  return (
    <div className="two-col">
      <section>
        <div className="timeline-strip">[동작 원리] 백본이 정제된 사실(근무자 정보 제외)만 읽고, jev_head 가 선택지 위치(닫는 따옴표)의 은닉 상태로 점수를 매깁니다 (0-Generation)</div>
        <h4>[모델에 입력된 표준 JSON 사실 (PointerPrompt)]</h4>
        <dl className="props">
          {(sub?.facts ?? []).map(([k, v]) => (
            <div key={k} className="sensor-line">
              <dt>{k}</dt>
              <dd>{v}</dd>
            </div>
          ))}
        </dl>
        <h4>[GPU 상주 · 모델 실행 기록]</h4>
        {trace.length === 0 && <p className="muted">모델 기록 없음 (규칙만 실행)</p>}
        <ol className="trace">
          {trace.map((r, i) => (
            <li key={i}>
              <span className="mono">{r.model}</span> {r.step} <b>{r.ms.toFixed(1)}ms</b> {r.detail?.group ? <i className="muted">{String(r.detail.group)}</i> : null}
            </li>
          ))}
        </ol>
      </section>
      <section>
        {asked.length === 0 && <p className="callout">Zoro-1 이 꽂혀 있지 않아 규칙 판정만 있습니다: {rule ? catalog[rule]?.label : "-"}</p>}
        {asked[0] && (
          <>
            <h4>질문 1. {asked[0].title}</h4>
            <Bars asked={asked[0]} catalog={catalog} highlight={rule} />
          </>
        )}
        {asked.slice(1).map((q, i) => (
          <div key={i} className="axis">
            <h4>
              [SDS 축 {i + 1}] {q.title.split(" 중 ").pop()?.replace(" 만 판단", "")}
            </h4>
            <p className="muted">{q.facts.map(([k, v]) => `${k}: ${v}`).join(" · ")}</p>
            <Bars asked={q} catalog={catalog} highlight={rule} />
          </div>
        ))}
        <div className="callout">
          📌 [판정 결합 규칙 (verdict.rs)] 판정은 규칙이 정합니다 ({rule ? catalog[rule]?.label : "-"}). 모델이 같은 쪽을 고르면 축별 최솟값이 확신도가 됩니다:{" "}
          <b>{sub?.decided.judgement.confidence != null ? sub.decided.judgement.confidence.toFixed(3) : "없음"}</b>. 정상 유지는 최솟값 0.95 이상일 때만 자동 확정 후보이고, 계통 이상 · 고위험은 교대감독이 확정합니다.
        </div>
      </section>
    </div>
  );
}

function Approval({ detail, supervisor, onDecided }: { detail: Detail; supervisor: string; onDecided: (m: string) => void }) {
  const d = detail.analysis.draft;
  const [ref, setRef] = useState(d?.aop_id ?? "");
  const [cosign, setCosign] = useState(false);
  const [scans, setScans] = useState(0);
  const [note, setNote] = useState("");
  const [reply, setReply] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
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
          [듀얼 헤드] 판단과 같은 몸통에 임베딩 표를 더 올려 글을 씁니다 {d ? `(${d.by_model ? `모델 · ${duration(d.ms)} · ${d.attempts}회` : "템플릿"})` : ""}
        </div>
        {d ? (
          <pre className="aop">
            {`[원인 진단 요약]\n${d.summary}\n\n[표준 조치 지시 (${d.aop_id} ${d.title})]\n${d.steps.map((s, i) => `${i + 1}. ${s}`).join("\n")}`}
          </pre>
        ) : (
          <p className="callout">이 판정에는 조치 절차서가 없습니다 ({detail.analysis.moc ? "Red-line 변경 반영 심사" : "절차서 없음"}).</p>
        )}
        {detail.state === "awaiting_field" && <p className="note">⏳ 현장 확인 대기 중입니다. 현장 게이지 실측이 오기 전에는 확정할 수 없습니다.</p>}
      </section>
      <section>
        <h4>[Factcheck 원문 숫자 · 코드 1:1 대조 검증기]</h4>
        {d && (
          <>
            <p className={d.problems.length ? "bad" : "good"}>검증 상태: {d.problems.length ? `모델 초안 불일치 ${d.problems.length}건 → 사실 줄 템플릿으로 대체 (G4_INTERCEPT_DOC_CORRUPTION)` : "✓ 원문 수치 100% 일치"}</p>
            <ul className="facts">
              {d.numbers.map(([n, ok], i) => (
                <li key={i} className={ok ? "good" : "bad"}>
                  [{n}] {ok ? "사실 줄과 일치 ✓" : "원문에 없음 ✗"}
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
        <h4>[{d?.high_risk ? "고위험 계통 조작 승인 (policy.rs 안전 가드레일)" : "교대감독 확정"}]</h4>
        {d?.high_risk && (
          <>
            <p className="muted">고위험 조치: {d.risk_actions.join(", ")}</p>
            <label className="field">
              근거 절차서 번호
              <input value={ref} onChange={(e) => setRef(e.target.value)} />
            </label>
            <label className="check">
              <input type="checkbox" checked={cosign} onChange={(e) => setCosign(e.target.checked)} />
              교대감독(파트장) 공동 결재를 받았습니다 (team_lead_cosign)
            </label>
            <label className="field">
              오경보 입증 현장 증빙 스캔 (장)
              <input type="number" min={0} value={scans} onChange={(e) => setScans(Number(e.target.value))} />
            </label>
          </>
        )}
        <label className="field">
          현장 재점검 요청 메모
          <input value={note} onChange={(e) => setNote(e.target.value)} placeholder="예: 2층 급수조절밸브 포지셔너 현장 확인" />
        </label>
        <div className="actions">
          {d?.high_risk && (
            <button className="plain" disabled={busy} onClick={() => act("approve_false_positive")}>
              고위험 예외승인 (오경보)
            </button>
          )}
          <button className="plain" disabled={busy} onClick={() => act("recheck")}>
            현장 재점검요구
          </button>
          <button className="plain" disabled={busy} onClick={() => act("reject")}>
            반려
          </button>
          <button className="danger-fill" disabled={busy} onClick={() => act(d?.high_risk ? "approve_procedure" : "confirm")}>
            🚨 조치확정
          </button>
        </div>
        {d?.high_risk && (
          <button className="plain small" disabled={busy} onClick={() => act("confirm")}>
            (시험) 공동 결재 없이 확정 시도
          </button>
        )}
        {reply && <p className="note">{reply}</p>}
      </section>
    </div>
  );
}

const COLORS = ["#ff6b3d", "#f6c453", "#4fd1c5", "#7aa2ff", "#c084fc", "#9ae66e"];

function Sds({ detail, plant }: { detail: Detail; plant: Plant }) {
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
      if (p.causal_match || p.backflow) add(p.upstream_tag, `[${p.backflow ? "역류" : `D -${p.hops}`}] ${label(p.upstream_tag)}`);
    }
    if (a.route.tag) {
      const chs = detail.timeline.channels[a.route.tag];
      if (chs) Object.keys(chs).forEach((c) => add(a.route.tag!, `[채널 ${c}] ${label(a.route.tag!)}`, c));
      else add(a.route.tag, `[D 0] ${label(a.route.tag)}`);
    }
    return out.slice(0, 5);
  }, [a, detail, plant]);
  const markers = a.chain.map((ev) => ({ at_ms: ev.at_ms, label: `${signedSecs(ev.at_ms, origin)} ${ev.tag.slice(7)} ${ev.kind === "limit" ? "한계" : ev.direction === "Up" ? "↑" : "↓"}`, color: ev.kind === "limit" ? "#ff4d4f" : "#f6c453" }));
  const from = Math.max(detail.timeline.t0_ms, origin - 20_000);
  const to = Math.min(detail.timeline.now_ms + 15_000, detail.timeline.t0_ms + 150_000);
  return (
    <div className="two-col wide-left">
      <section>
        <div className="timeline-strip">
          [SDS 스냅샷 ID: {a.sds.sds_id}] 트리거: {a.sds.trigger_kind} {a.trigger.slope !== null ? `· 기울기 ${a.trigger.slope?.toFixed(2)}/s` : ""} {a.trigger.ttf_secs !== null ? `· 트립 도달 예상 잔여 ${a.trigger.ttf_secs?.toFixed(0)}초` : ""}
        </div>
        <WaveChart timeline={detail.timeline} traces={traces} sensors={plant.sensors} from={from} to={to} markers={[{ at_ms: origin, label: "t=0 발원", color: "#ffffff" }, ...markers]} cursor={detail.timeline.now_ms} />
        {root && (
          <div className="callout adverse">
            📌 [시계열 인과 시차(Δt) 판정] 관측 시차 Δt = +{root.lag_secs.toFixed(1)}초 (배관 허용 {root.lag_range[0]} ~ {root.lag_range[1]}초 안) → {plant.equipment.find((e) => e.kks === a.route.kks)?.name} 이상은 {root.lag_secs.toFixed(1)}초 전 {plant.equipment.find((e) => e.kks === root.kks)?.name} 변화가 원인
          </div>
        )}
        <h4>[60초 링버퍼 지표]</h4>
        <table className="grid">
          <thead>
            <tr>
              <th>센서</th>
              <th>변곡 시각</th>
              <th>기울기 /s</th>
              <th>가속도</th>
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
                <td>{m.estimated_ttf_secs !== null ? `${m.estimated_ttf_secs}초` : "-"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>
      <section>
        <h4>[SDS 시공간 직교 분할 검증 (Zoro-1 판단)]</h4>
        <p className="muted">복합 알람을 직교 기준으로 쪼개어 묻고, 가장 낮은 확률을 최종 확신도로 씁니다.</p>
        {a.sds.decomposed_confidences.length === 0 && <p className="callout">축별 질문 기록이 없습니다 (모델 없음 또는 절차 판정).</p>}
        {a.sds.decomposed_confidences.map(([axis, p]) => (
          <div key={axis} className="bar-row">
            <span>{axis}</span>
            <div className="bar-track">
              <div className="bar-fill" style={{ width: `${p * 100}%` }} />
            </div>
            <b className="mono">{p.toFixed(3)}</b>
          </div>
        ))}
        <p className="callout">➔ SDS 최종 채택 확신도 (Min): {a.sds.min_confidence !== null ? a.sds.min_confidence.toFixed(3) : "없음"}</p>
        <h4>[게이트별 SDS 상태 전이 코드]</h4>
        <div className="chips">
          {a.sds.gate_state_vector.map((c, i) => (
            <span key={i} className="chip mono">
              {c}
            </span>
          ))}
        </div>
        <h4>[신호 전파 체인 (Event Trace)]</h4>
        <ul className="facts">
          {a.chain.map((ev, i) => (
            <li key={i}>
              • {signedSecs(ev.at_ms, origin)} : {ev.tag} {ev.label} {ev.kind === "limit" ? "한계 돌파" : ev.direction === "Up" ? "상승" : "하락"}
            </li>
          ))}
        </ul>
      </section>
    </div>
  );
}

function ArchiveView({ doc }: { doc: ArchiveDoc }) {
  return (
    <div className="two-col">
      <section>
        <h4>
          {doc.archive_id} · {doc.final_code} {doc.auto ? "[자동확정]" : ""}
        </h4>
        <pre className="aop">{doc.mcr_doc}</pre>
      </section>
      <section>
        <h4>현장 단말 안내</h4>
        <pre className="aop">{doc.field_notice || "(현장 보고 없음)"}</pre>
        <dl className="props">
          <dt>SDS 블랙박스 봉인</dt>
          <dd className="mono">{doc.seal}</dd>
          <dt>보존 기한</dt>
          <dd>{doc.retention_until}</dd>
          <dt>특별 감사 표시</dt>
          <dd>{doc.special_audit_flag ? "예 (감사실 보고)" : "아니오"}</dd>
        </dl>
      </section>
    </div>
  );
}

const TITLES: Record<string, string> = {
  "1": "🔍 [Stage 1 & 2 상세] SigLIP2 시각 앵커 조각 계획 & gocr + Lattice 대장 교정 결과",
  "2": "🔍 [Stage 1 & 2 상세] SigLIP2 시각 앵커 조각 계획 & gocr + Lattice 대장 교정 결과",
  "3": "⚖️ [Stage 3 상세] Gate 1 형식 심사 & Gate 2 3중 진실 정합성 교차 대조",
  "4": "🧠 [Stage 4 상세] Gate 3 마인드맵 파급 분석 & Zoro-1 포인터 판단 (jev_head)",
  "5": "✍️ [Stage 5 상세] Zoro-1 글쓰기 AOP 조치 절차서 초안 · Factcheck 수치 검증 · 최종 결재",
  sds: "⏱️ [시계열 & SDS 심층 분석기] 다중 계통 시계열 파형 동기화 (Δt) 및 SDS 직교 분할 검증",
  archive: "🗄️ 보관 문서 (Gate 5)",
};

export function StageDrawer({ which, detail, archive, plant, catalog, supervisor, onClose, onDecided }: { which: DrawerKey; detail: Detail | null; archive: ArchiveDoc | null; plant: Plant | null; catalog: Record<string, VerdictInfo>; supervisor: string; onClose: () => void; onDecided: (m: string) => void }) {
  return (
    <div className="drawer">
      <header>
        <strong>{TITLES[String(which)]}</strong>
        <span className="grow" />
        <button className="plain" onClick={onClose}>
          [X 닫기]
        </button>
      </header>
      <div className="drawer-body">
        {which === "archive" && archive && <ArchiveView doc={archive} />}
        {which !== "archive" && !detail && <p className="muted">진행 중인 사건을 고르세요.</p>}
        {detail && (which === 1 || which === 2) && <Scan detail={detail} />}
        {detail && which === 3 && <Truth detail={detail} supervisor={supervisor} />}
        {detail && which === 4 && <Decision detail={detail} catalog={catalog} />}
        {detail && which === 5 && <Approval detail={detail} supervisor={supervisor} onDecided={onDecided} />}
        {detail && plant && which === "sds" && <Sds detail={detail} plant={plant} />}
      </div>
    </div>
  );
}
