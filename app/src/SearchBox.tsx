import { invoke } from "@tauri-apps/api/core";
import { useState } from "react";

interface SearchDoc {
  kind: "equipment" | "sensor" | "incident" | "archive" | "aop" | "moc" | "drawing";
  id: string;
  title: string;
  subtitle: string;
  kks: string | null;
}

interface SearchResult {
  query: string;
  types: SearchDoc["kind"][];
  conditions: { kind: string; text: string; applied: boolean; note: string }[];
  notes: string[];
  hits: { doc: SearchDoc; score: number; why: string[]; passed: boolean; rescued: boolean }[];
}

const KIND_LABEL: Record<SearchDoc["kind"], string> = { equipment: "설비", sensor: "계측", incident: "사건", archive: "보관", aop: "절차서", moc: "MOC", drawing: "도면" };

/** 자연어 검색 (logis-center 방식): 유형 먼저 · 조건은 원장과 대조해 맞는 게 없으면 힌트로 · 전체 질의 경로와 순위 합침 · 구제.
 *  결과마다 "왜 찾았나" 를 보여 주고, 누르면 그 설비로 세 뷰를 맞추거나 사건 · 보관 문서를 엽니다. */
export function SearchBox({ onSelect, onPick }: { onSelect: (kks: string) => void; onPick: (id: string) => void }) {
  const [q, setQ] = useState("");
  const [r, setR] = useState<SearchResult | null>(null);
  const [open, setOpen] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const run = async () => {
    if (!q.trim()) return;
    try {
      setR(await invoke<SearchResult>("search", { query: q }));
      setError(null);
      setOpen(true);
    } catch (e) {
      setError(String(e));
    }
  };

  return (
    <div className="search">
      <input
        className="search-input"
        placeholder="자연어 검색 (예: 103동 2층 밸브 · 과열기 온도 600도 이상 · 급수조절밸브 상류 2홉 · 승인 대기 MOC)"
        value={q}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          if (e.key === "Enter") void run();
          if (e.key === "Escape") setOpen(false);
        }}
        onFocus={() => r && setOpen(true)}
      />
      {error && <span className="bad">{error}</span>}
      {/* 결과는 지금 입력한 질의의 것일 때만 보입니다 (질의를 고치면 Enter 전까지 옛 결과를 감춤). */}
      {open && r && r.query === q && (
        <div className="search-results">
          <div className="search-head">
            <span>
              유형 <b>{r.types.map((t) => KIND_LABEL[t]).join(" · ")}</b>
            </span>
            {r.conditions.map((c, i) => (
              <span key={i} className={c.applied ? "cond on" : "cond hint"} title={c.note}>
                {c.kind} {c.text}
                {c.applied ? "" : " (힌트)"}
              </span>
            ))}
            <span className="grow" />
            <button className="plain small" onClick={() => setOpen(false)}>
              닫기
            </button>
          </div>
          {r.notes.map((n, i) => (
            <div key={i} className="search-note">
              {n}
            </div>
          ))}
          {r.hits.length === 0 && <p className="muted">찾은 것이 없습니다.</p>}
          {r.hits.map((h) => (
            <button
              key={`${h.doc.kind}:${h.doc.id}`}
              className={`search-hit ${h.passed ? "" : "weak"}`}
              onClick={() => {
                if (h.doc.kind === "incident" || h.doc.kind === "archive") onPick(h.doc.id);
                else if (h.doc.kks) onSelect(h.doc.kks);
                setOpen(false);
              }}
            >
              <span className="hit-line">
                <span className={`kind ${h.doc.kind}`}>{KIND_LABEL[h.doc.kind]}</span> <b>{h.doc.title}</b> <span className="mono">{h.doc.id}</span>
                {h.rescued && <em> · 전체 표에서 구제</em>}
                {!h.passed && <em> · 조건 밖 (전체 순위)</em>}
              </span>
              <span className="muted">{h.doc.subtitle}</span>
              {h.why.length > 0 && <span className="why">왜: {h.why.slice(0, 6).join(" · ")}</span>}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
