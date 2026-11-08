// 자연어 질의 커맨드 바 (2차 개선안 2: logis-center 방식 검색을 noto-sandbox 하단 허브의 `.command-bar` · `.dropup` · `.cmd-toast` 로).
// 백엔드 `search` 명령이 한국어 · 영어 질의를 그대로 읽습니다 (Stanza 품사 · 원형, 임베딩 경로, NMS 조각: docs/search.md).
import { useRef, useState, type KeyboardEvent, type RefObject } from "react";
import { useT, type T } from "../i18n";
import { tr } from "../i18n/lang";
import type { Plant } from "../types";
import type { SearchMark } from "./CctvOverlay";

export interface SearchDoc {
  kind: "equipment" | "sensor" | "incident" | "archive" | "aop" | "moc" | "drawing";
  id: string;
  title: string;
  subtitle: string;
  kks: string | null;
}

export interface SearchHit {
  doc: SearchDoc;
  score: number;
  why: string[];
  passed: boolean;
  rescued: boolean;
}

export interface SearchResult {
  query: string;
  types: SearchDoc["kind"][];
  conditions: { kind: string; text: string; applied: boolean; note: string }[];
  notes: string[];
  hits: SearchHit[];
  /** 질의 언어 (엔진이 판단) */
  lang?: "ko" | "en";
  /** 쓴 모델 ("Stanza en · ko · 임베딩 granite-97m", 없으면 "규칙만") */
  engine?: string;
  /** 낱말마다 품사 · 원형 · 줄기 */
  tokens?: { text: string; upos: string | null; xpos: string | null; lemma: string | null; forms: string[]; function: boolean }[];
  /** NMS 에서 이긴 조각 (낱말 창 → 원장 설비 · 센서) */
  segments?: { text: string; label: string; cos: number; z: number }[];
}

/** 한 번의 질의: 입력한 글 · 문맥을 붙인 글 · 엔진에 보낸 글(문맥을 붙인 글 그대로, 영어도 옮기지 않음) · 결과 */
export interface SearchRun {
  typed: string;
  sent: string;
  engine: string;
  context: string | null;
  result: SearchResult;
}

const KIND_KO: Record<SearchDoc["kind"], string> = { equipment: "설비", sensor: "계측", incident: "사건", archive: "보관", aop: "절차서", moc: "MOC", drawing: "도면" };
const KIND_EN: Record<SearchDoc["kind"], string> = { equipment: "Equipment", sensor: "Sensor", incident: "Incident", archive: "Archive", aop: "Procedure", moc: "MOC", drawing: "Drawing" };
/** 결과 종류 이름 (화면 언어) */
export const kindLabel = (k: SearchDoc["kind"]) => tr(KIND_KO[k], KIND_EN[k]);

const HISTORY_KEY = "blueprint.search-history";
const RELATIVE = ["상류", "하류", "주변", "연결", "관련", "이 설비", "여기", "upstream", "downstream", "around", "near", "connected", "related", "this"];

const bigrams = (text: string) => {
  const chars = [...text.toLowerCase()].filter((c) => !/\s/.test(c) && !"·()[],.".includes(c));
  const out = new Set<string>();
  for (let i = 1; i < chars.length; i++) out.add(chars[i - 1] + chars[i]);
  return out;
};

/** 선택 설비 문맥 (noto-sandbox `#unit-tools` 의 `[🛠 유닛 ✕]`): 질의에 자기 기준이 없고(식별자 · 동 · 설비 이름이 없음)
 *  "상류 1홉" · "관련 MOC" · "절차서" 처럼 짧거나 관계를 묻는 글이면 앞에 선택 설비 코드를 붙입니다.
 *  코드는 엔진이 연결 기준 설비로 씁니다 (`search.rs`: 식별자 → 기준 설비). 자기 기준이 있는 질의는 그대로 보냅니다. */
export function contextual(query: string, kks: string, plant: Plant): string {
  const q = query.trim();
  const words = q.split(/\s+/).filter(Boolean);
  const ownId = words.some((w) => /^[0-9A-Z][0-9A-Z-]{7,}$/i.test(w.replace(/[^0-9A-Za-z-]/g, "")) && /[0-9]/.test(w));
  const building = plant.sectors.some((s) => q.includes(s.building)) || /\d+동/.test(q) || /\b(?:bldg\.?|building)\s*\d+/i.test(q);
  const qg = bigrams(q);
  // 한글 이름은 두 글자 조각 둘 이상이 겹치면 이름을 부른 것으로 봅니다. 영어는 글자 조각이 너무 흔해("upstream" ↔ "Main Steam")
  // 이름 전체가 들어 있거나 이름의 세 글자 이상 낱말이 둘 이상 들어 있을 때만 봅니다.
  const latin = !/[가-힣]/.test(q);
  const lower = q.toLowerCase();
  const named = plant.equipment.some((e) => {
    if (!latin) return [...bigrams(e.name)].filter((g) => qg.has(g)).length >= 2;
    const name = e.name.toLowerCase();
    const words = name.split(/[^a-z0-9]+/).filter((w) => w.length >= 3);
    return lower.includes(name) || words.filter((w) => new RegExp(`\\b${w}\\b`).test(lower)).length >= 2;
  });
  if (ownId || building || named) return q;
  const relative = RELATIVE.some((w) => q.toLowerCase().includes(w));
  // 짧은 질의: 한국어 "관련 MOC" · "절차서" 처럼 두 낱말까지. 영어는 관계 낱말(related · upstream …)을 따로 쓰므로 한 낱말만
  // ("pending MOC" 는 상태로 거른 유형 질의라 선택 설비 문맥을 붙이지 않습니다)
  return relative || words.length <= (latin ? 1 : 2) ? `${kks} ${q}` : q;
}

/** 결과에서 도면 위 Top-3 표시 (설비 코드가 있는 조건 통과 항목, 같은 설비는 한 번) */
export function searchMarks(run: SearchRun | null): Map<string, SearchMark> {
  const m = new Map<string, SearchMark>();
  for (const h of run?.result.hits ?? []) {
    if (m.size >= 3) break;
    if (!h.passed || !h.doc.kks || m.has(h.doc.kks)) continue;
    m.set(h.doc.kks, { rank: m.size + 1, why: h.why, kind: kindLabel(h.doc.kind), id: h.doc.id });
  }
  return m;
}

/** 입력줄 위 한 줄 요약 (noto-sandbox `.cmd-toast`): 유형 · 적중 수 · 1위 · 힌트로 내린 조건 · 문맥 */
export function searchSummary(run: SearchRun, t: T): { kind: "ok" | "info" | "err"; text: string } {
  const r = run.result;
  const passed = r.hits.filter((h) => h.passed);
  const top = r.hits[0];
  const demoted = r.conditions.filter((c) => !c.applied).map((c) => `'${c.text}'`);
  const parts = [`[${r.types.map(kindLabel).join("·")}] ${t(`${passed.length}건 적중`, `${passed.length} hits`)}`];
  if (top) parts.push(`${t("1위", "Top")}: ${top.doc.id} ${top.doc.title}${top.passed ? "" : t(" (조건 밖)", " (outside conditions)")}`);
  if (demoted.length) parts.push(t(`${demoted.join(" · ")} 조건 힌트로 내림`, `${demoted.join(" · ")} demoted to hints`));
  if (run.context) parts.push(t(`문맥 ${run.context} + '${run.typed}'`, `context ${run.context} + '${run.typed}'`));
  if (!r.hits.length) return { kind: "err", text: t(`⚪ 찾은 것이 없습니다${run.context ? ` (문맥 ${run.context})` : ""}`, `⚪ Nothing found${run.context ? ` (context ${run.context})` : ""}`) };
  return { kind: top?.passed ? "ok" : "info", text: `${top?.passed ? "🟢" : "🟡"} ${parts.join(" · ")}` };
}

function loadHistory(): string[] {
  try {
    const v = JSON.parse(localStorage.getItem(HISTORY_KEY) ?? "[]");
    return Array.isArray(v) ? v.filter((s) => typeof s === "string").slice(-50) : [];
  } catch {
    return [];
  }
}

/** 하단 허브 맨 아랫줄: `[🛠 KKS ✕]` 문맥 칩 · 입력 · 질의 · `[결과 N건 ▲]`.
 *  Enter 나 / 로 어디서든 이 줄로 들어오고(OfficerView), ↑↓ 로 지난 질의(최대 50, 이 PC 에 저장)를 불러옵니다.
 *  ESC 는 글이 있으면 지우기만 하고, 없으면 입력을 떠나 전역 ESC(판 닫기 → 설비 선택 풀기)로 넘깁니다. */
export function CommandBar(props: {
  inputRef: RefObject<HTMLInputElement>;
  selected: string | null;
  context: boolean;
  onContext: (on: boolean) => void;
  run: SearchRun | null;
  open: boolean;
  onToggle: () => void;
  onSubmit: (text: string) => Promise<boolean>;
}) {
  const { inputRef, selected, context, onContext, run, open, onToggle, onSubmit } = props;
  const t = useT();
  const [value, setValue] = useState("");
  const [busy, setBusy] = useState(false);
  // 기록은 처음 한 번만 읽습니다 (렌더마다 저장소를 읽지 않게)
  const [initial] = useState(loadHistory);
  const history = useRef<string[]>(initial);
  const idx = useRef(initial.length);
  const draft = useRef("");

  const push = (text: string) => {
    const h = history.current;
    if (h[h.length - 1] !== text) h.push(text);
    history.current = h.slice(-50);
    idx.current = history.current.length;
    draft.current = "";
    try {
      localStorage.setItem(HISTORY_KEY, JSON.stringify(history.current));
    } catch {
      // 저장소가 막혀 있어도 이번 실행 동안은 기록이 남습니다
    }
  };

  const submit = async () => {
    const text = value.trim();
    if (!text || busy) return;
    setBusy(true);
    try {
      if (await onSubmit(text)) push(text);
    } finally {
      setBusy(false);
    }
  };

  const onKeyDown = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.nativeEvent.isComposing) return;
    const h = history.current;
    if (e.key === "Enter") {
      e.preventDefault();
      void submit();
    } else if (e.key === "ArrowUp" && h.length) {
      e.preventDefault();
      if (idx.current === h.length) draft.current = value;
      idx.current = Math.max(0, idx.current - 1);
      setValue(h[idx.current]);
    } else if (e.key === "ArrowDown" && idx.current < h.length) {
      e.preventDefault();
      idx.current += 1;
      setValue(idx.current === h.length ? draft.current : h[idx.current]);
    } else if (e.key === "Escape") {
      if (value) {
        // 글만 지우고 판은 그대로 (전역 ESC 가 판을 닫지 않게 표시)
        e.preventDefault();
        setValue("");
      } else {
        e.currentTarget.blur();
      }
    }
  };

  const count = run?.result.hits.filter((h) => h.passed).length ?? 0;
  return (
    <form
      className="command-bar"
      autoComplete="off"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      {selected &&
        (context ? (
          <button type="button" className="unit-tag on" onClick={() => onContext(false)} title={t("선택 설비 문맥으로 묻는 중 (누르면 문맥 없이)", "Querying in the selected equipment context (click to query without it)")}>
            🛠 {selected} ✕
          </button>
        ) : (
          <button type="button" className="unit-tag" onClick={() => onContext(true)} title={t(`${selected} 기준으로 묻기 ("상류 1홉" · "관련 MOC" · "절차서")`, `Query relative to ${selected} ("1 hop upstream" · "related MOC" · "procedure")`)}>
            {t("🛠 문맥 ＋", "🛠 Context ＋")}
          </button>
        ))}
      <span className="cmd-prompt" aria-hidden="true">
        ›
      </span>
      <label className="sr-only" htmlFor="command-input">
        {t("자연어 질의", "Natural-language query")}
      </label>
      <input
        id="command-input"
        ref={inputRef}
        className="search-input"
        placeholder={
          selected && context
            ? t(`${selected} 기준: 상류 1홉 · 관련 MOC · 절차서 …  (다른 설비는 이름 · 동으로)`, `Relative to ${selected}: 1 hop upstream · related MOC · procedure …  (other equipment: by name or building)`)
            : t("자연어 질의 (Enter · /): 103동 2층 밸브 · 과열기 온도 600도 이상 · 급수조절밸브 상류 2홉 · 승인 대기 MOC", "Natural-language query (Enter · /): valves on floor 2 of building 103 · superheater temperature above 600 °C · 2 hops upstream of feedwater control valve · pending MOC")
        }
        value={value}
        maxLength={200}
        spellCheck={false}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <button type="submit" className="teal" disabled={busy || !value.trim()}>
        {busy ? t("찾는 중", "Searching") : t("🔍 질의", "🔍 Query")}
      </button>
      <button type="button" className={`plain results-toggle ${open ? "active" : ""}`} disabled={!run} onClick={onToggle} aria-expanded={open}>
        {t(`결과 ${count}건`, `${count} results`)} {open ? "▼" : "▲"}
      </button>
    </form>
  );
}

/** 입력줄 위로 펼치는 반투명 판 (noto-sandbox `#quest-dropup`): 유형 · 적용 조건(초록) · 힌트로 내린 조건(노란 점선) · 안내 · 결과 목록 */
export function SearchDropup({ run, onClose, onSelect, onPick, onInterlock, interlockKks }: { run: SearchRun; onClose: () => void; onSelect: (kks: string) => void; onPick: (id: string) => void; onInterlock: () => void; interlockKks: Set<string> }) {
  const r = run.result;
  const t = useT();
  return (
    <section className="search-results dropup vp-glass ov-avoid" aria-label={t("질의 분석 · 검색 결과", "Query analysis · results")}>
      <div className="search-head">
        <b>{t("🔍 질의 분석", "🔍 Query Analysis")}</b>
        <span>
          {t("유형", "Type")} <b>{r.types.map(kindLabel).join(" · ")}</b>
        </span>
        {r.conditions.map((c, i) => (
          <span key={i} className={c.applied ? "cond on" : "cond hint"} title={c.note}>
            {c.applied ? "🟢" : "🟡"} {c.kind} {c.text}
            {c.applied ? "" : t(" (힌트)", " (hint)")}
          </span>
        ))}
        {run.context && <span className="cond ctx">{t(`🛠 문맥 ${run.context}`, `🛠 Context ${run.context}`)}</span>}
        {r.engine && (
          <span className="cond ctx" title={t("이번 검색에 쓴 분석 모델 (모델 설정에서 가져오기)", "Analysis models used for this search (import them in Model Settings)")}>
            🧠 {r.engine}
          </span>
        )}
        <span className="grow" />
        <button className="ov-close" onClick={onClose} aria-label={t("닫기", "Close")} title={t("닫기 (ESC)", "Close (ESC)")}>
          ✕
        </button>
      </div>
      {r.notes.map((n, i) => (
        <div key={i} className="search-note">
          {n}
        </div>
      ))}
      {r.hits.length === 0 && <p className="muted">{t("찾은 것이 없습니다.", "Nothing found.")}</p>}
      <div className="search-list">
        {r.hits.map((h, i) => (
          <div key={`${h.doc.kind}:${h.doc.id}`} className={`search-hit ${h.passed ? "" : "weak"}`}>
            <button
              className="hit-main"
              onClick={() => {
                if (h.doc.kind === "incident" || h.doc.kind === "archive") onPick(h.doc.id);
                else if (h.doc.kks) onSelect(h.doc.kks);
              }}
            >
              <span className="hit-line">
                <span className="rank">{i + 1 <= 3 ? ["🥇", "🥈", "🥉"][i] : `${i + 1}.`}</span> <span className={`kind ${h.doc.kind}`}>{kindLabel(h.doc.kind)}</span> <b>{h.doc.title}</b> <span className="mono">{h.doc.id}</span>
                {h.rescued && <em>{t(" · 전체 표에서 구제", " · rescued from the full table")}</em>}
                {!h.passed && <em>{t(" · 조건 밖 (전체 순위)", " · outside conditions (overall rank)")}</em>}
              </span>
              <span className="muted">{h.doc.subtitle}</span>
              {h.why.length > 0 && <span className="why">{t("왜", "Why")}: {h.why.slice(0, 6).join(" · ")}</span>}
            </button>
            {/* 행을 누르면 설비를 고르거나(설비 · 계측 · 도면 · MOC) 사건을 엽니다(사건 · 보관). 단추는 그와 다른 일만: 사건 행의 설비 보기 · 인터록 보기 */}
            <span className="hit-actions">
              {h.doc.kks && (h.doc.kind === "incident" || h.doc.kind === "archive") && (
                <button className="vt-btn" onClick={() => onSelect(h.doc.kks!)} title={t("사건을 열지 않고 도면에서 그 설비만 비춥니다", "Show the equipment on the drawing without opening the incident")}>
                  {t("📍 설비 보기", "📍 Show equipment")}
                </button>
              )}
              {h.doc.kks && interlockKks.has(h.doc.kks) && (
                <button
                  className="vt-btn"
                  onClick={() => {
                    onSelect(h.doc.kks!);
                    onInterlock();
                  }}
                >
                  {t("📋 인터록 보기", "📋 View interlock")}
                </button>
              )}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}
