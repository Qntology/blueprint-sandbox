// 자연어 질의 커맨드 바 (2차 개선안 2: logis-center 방식 검색을 noto-sandbox 하단 허브의 `.command-bar` · `.dropup` · `.cmd-toast` 로).
// 백엔드 `search` 명령은 그대로 쓰고(Rust 변경 없음), 화면만 하단 허브로 옮겼습니다.
import { useRef, useState, type KeyboardEvent, type RefObject } from "react";
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
}

/** 한 번의 질의: 입력한 글 · 실제로 보낸 글(선택 설비 문맥을 붙였으면 다름) · 결과 */
export interface SearchRun {
  typed: string;
  sent: string;
  context: string | null;
  result: SearchResult;
}

export const KIND_LABEL: Record<SearchDoc["kind"], string> = { equipment: "설비", sensor: "계측", incident: "사건", archive: "보관", aop: "절차서", moc: "MOC", drawing: "도면" };

const HISTORY_KEY = "blueprint.search-history";
const RELATIVE = ["상류", "하류", "주변", "연결", "관련", "이 설비", "여기"];

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
  const building = plant.sectors.some((s) => q.includes(s.building)) || /\d+동/.test(q);
  const qg = bigrams(q);
  const named = plant.equipment.some((e) => [...bigrams(e.name)].filter((g) => qg.has(g)).length >= 2);
  if (ownId || building || named) return q;
  const relative = RELATIVE.some((w) => q.includes(w));
  return relative || words.length <= 2 ? `${kks} ${q}` : q;
}

/** 결과에서 도면 위 Top-3 표시 (설비 코드가 있는 조건 통과 항목, 같은 설비는 한 번) */
export function searchMarks(run: SearchRun | null): Map<string, SearchMark> {
  const m = new Map<string, SearchMark>();
  for (const h of run?.result.hits ?? []) {
    if (m.size >= 3) break;
    if (!h.passed || !h.doc.kks || m.has(h.doc.kks)) continue;
    m.set(h.doc.kks, { rank: m.size + 1, why: h.why, kind: KIND_LABEL[h.doc.kind], id: h.doc.id });
  }
  return m;
}

/** 입력줄 위 한 줄 요약 (noto-sandbox `.cmd-toast`): 유형 · 적중 수 · 1위 · 힌트로 내린 조건 · 문맥 */
export function searchSummary(run: SearchRun): { kind: "ok" | "info" | "err"; text: string } {
  const r = run.result;
  const passed = r.hits.filter((h) => h.passed);
  const top = r.hits[0];
  const demoted = r.conditions.filter((c) => !c.applied).map((c) => `'${c.text}'`);
  const parts = [`[${r.types.map((k) => KIND_LABEL[k]).join("·")}] ${passed.length}건 적중`];
  if (top) parts.push(`1위: ${top.doc.id} ${top.doc.title}${top.passed ? "" : " (조건 밖)"}`);
  if (demoted.length) parts.push(`${demoted.join(" · ")} 조건 힌트로 내림`);
  if (run.context) parts.push(`문맥 ${run.context} + '${run.typed}'`);
  if (!r.hits.length) return { kind: "err", text: `⚪ 찾은 것이 없습니다${run.context ? ` (문맥 ${run.context})` : ""}` };
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
          <button type="button" className="unit-tag on" onClick={() => onContext(false)} title="선택 설비 문맥으로 묻는 중 (누르면 문맥 없이)">
            🛠 {selected} ✕
          </button>
        ) : (
          <button type="button" className="unit-tag" onClick={() => onContext(true)} title={`${selected} 기준으로 묻기 ("상류 1홉" · "관련 MOC" · "절차서")`}>
            🛠 문맥 ＋
          </button>
        ))}
      <span className="cmd-prompt" aria-hidden="true">
        ›
      </span>
      <label className="sr-only" htmlFor="command-input">
        자연어 질의
      </label>
      <input
        id="command-input"
        ref={inputRef}
        className="search-input"
        placeholder={selected && context ? `${selected} 기준: 상류 1홉 · 관련 MOC · 절차서 …  (다른 설비는 이름 · 동으로)` : "자연어 질의 (Enter · /): 103동 2층 밸브 · 과열기 온도 600도 이상 · 급수조절밸브 상류 2홉 · 승인 대기 MOC"}
        value={value}
        maxLength={200}
        spellCheck={false}
        onChange={(e) => setValue(e.target.value)}
        onKeyDown={onKeyDown}
      />
      <button type="submit" className="teal" disabled={busy || !value.trim()}>
        {busy ? "찾는 중" : "🔍 질의"}
      </button>
      <button type="button" className={`plain results-toggle ${open ? "active" : ""}`} disabled={!run} onClick={onToggle} aria-expanded={open}>
        결과 {count}건 {open ? "▼" : "▲"}
      </button>
    </form>
  );
}

/** 입력줄 위로 펼치는 반투명 판 (noto-sandbox `#quest-dropup`): 유형 · 적용 조건(초록) · 힌트로 내린 조건(노란 점선) · 안내 · 결과 목록 */
export function SearchDropup({ run, onClose, onSelect, onPick, onInterlock, interlockKks }: { run: SearchRun; onClose: () => void; onSelect: (kks: string) => void; onPick: (id: string) => void; onInterlock: () => void; interlockKks: Set<string> }) {
  const r = run.result;
  return (
    <section className="search-results dropup vp-glass ov-avoid" aria-label="질의 분석 · 검색 결과">
      <div className="search-head">
        <b>🔍 질의 분석</b>
        <span>
          유형 <b>{r.types.map((t) => KIND_LABEL[t]).join(" · ")}</b>
        </span>
        {r.conditions.map((c, i) => (
          <span key={i} className={c.applied ? "cond on" : "cond hint"} title={c.note}>
            {c.applied ? "🟢" : "🟡"} {c.kind} {c.text}
            {c.applied ? "" : " (힌트)"}
          </span>
        ))}
        {run.context && <span className="cond ctx">🛠 문맥 {run.context}</span>}
        <span className="grow" />
        <button className="ov-close" onClick={onClose} aria-label="닫기" title="닫기 (ESC)">
          ✕
        </button>
      </div>
      {r.notes.map((n, i) => (
        <div key={i} className="search-note">
          {n}
        </div>
      ))}
      {r.hits.length === 0 && <p className="muted">찾은 것이 없습니다.</p>}
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
                <span className="rank">{i + 1 <= 3 ? ["🥇", "🥈", "🥉"][i] : `${i + 1}.`}</span> <span className={`kind ${h.doc.kind}`}>{KIND_LABEL[h.doc.kind]}</span> <b>{h.doc.title}</b> <span className="mono">{h.doc.id}</span>
                {h.rescued && <em> · 전체 표에서 구제</em>}
                {!h.passed && <em> · 조건 밖 (전체 순위)</em>}
              </span>
              <span className="muted">{h.doc.subtitle}</span>
              {h.why.length > 0 && <span className="why">왜: {h.why.slice(0, 6).join(" · ")}</span>}
            </button>
            <span className="hit-actions">
              {h.doc.kks && (
                <button className="vt-btn" onClick={() => onSelect(h.doc.kks!)} title="도면에서 이 설비를 비춥니다">
                  📍 도면 핀
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
                  📋 인터록 보기
                </button>
              )}
              {(h.doc.kind === "incident" || h.doc.kind === "archive") && (
                <button className="vt-btn" onClick={() => onPick(h.doc.id)}>
                  📂 열기
                </button>
              )}
            </span>
          </div>
        ))}
      </div>
    </section>
  );
}
