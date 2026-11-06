import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useEffect, useRef, useState } from "react";
import { PLANT_TERMS, type DomainInfo, type Terms } from "./domain";
import { SignaturePad, type SignatureStats } from "./SignaturePad";

const RESET_SECONDS = 40;
const IDLE_RESET_SECONDS = 180;

type Phase = "start" | "scanning" | "pick" | "form" | "excuse" | "sent" | "finished";
type Kind = "gauge_card" | "redline" | "manual";
type Values = Record<string, string>;

interface SectorChoice {
  key: string;
  building: string;
  label: string;
  kks: string;
  blurb: string;
}

interface ScanReply {
  docs: string[];
  kind: Kind | null;
  prefill: Record<string, string | number>;
  scanned: string[];
  corrected: string[];
  signed: boolean;
  unknown_pages: number;
  /** 원터치 대조 카드 (개선안 4단계): 줄인 스캔 그림 · 항목별 조각 상자 · 교정 전 원문 */
  image: string | null;
  image_size: [number, number] | null;
  boxes: Record<string, [number, number, number, number]>;
  raw: Record<string, string>;
}

/** 스캔 그림에서 그 항목을 읽은 조각만 잘라 보여 줍니다 (값과 1:1 눈대조). 아주 긴 띠는 왼쪽부터 잘라 보입니다. */
function Crop({ src, size, box, raw }: { src: string; size: [number, number]; box: [number, number, number, number]; raw?: string }) {
  const [x0, y0, x1, y1] = box;
  const ratio = Math.min(((x1 - x0) * size[0]) / Math.max((y1 - y0) * size[1], 1), 9);
  return (
    <div className="crop" title={raw ? `스캔 원문: ${raw}` : undefined}>
      <div className="crop-view" style={{ aspectRatio: `${ratio}` }}>
        <img src={src} alt="" draggable={false} style={{ width: `${100 / (x1 - x0)}%`, transform: `translate(${-x0 * 100}%, ${-y0 * 100}%)` }} />
      </div>
      {raw && <em className="mono">스캔 원문 {raw}</em>}
    </div>
  );
}

/** 단위별 스테퍼 간격 (장갑을 끼고 누르는 큰 버튼): bar 는 0.1 · 0.5, 그 밖은 0.1 · 1.0 */
function steps(unit: string | undefined): [number, number] {
  return unit?.trim().toLowerCase() === "bar" ? [0.1, 0.5] : [0.1, 1.0];
}

const PLANT_EXCUSES: [string, string][] = [
  ["transmitter_drift", "현장 아날로그 게이지는 정상이나 전자식 전송기(Transmitter) 영점 흔들림 의심"],
  ["moc_change", "계획예방정비 · 부품 교체(MOC)로 운전 압력 사양이 변경됨 (Red-line 도면 첨부)"],
  ["manual_bypass", "현장 수동 바이패스(Bypass) 밸브 조작 중으로 일시적 유량 · 압력 편차 발생"],
  ["free_text", "기타 현장 특이사항 직접 입력 (자유 서술 → Zoro-1이 표준 코드로 자동 분류)"],
];

const AIDC_EXCUSES: [string, string][] = [
  ["transmitter_drift", "현장 아날로그 압력계 · 유량계는 정상이나 BMC 텔레메트리(전송기) 값 흔들림 의심"],
  ["moc_change", "MOP(변경 작업)로 CDU · 랙 운전 사양이 변경됨 (Red-line 변경도 첨부)"],
  ["manual_bypass", "현장 CDU 우회밸브 수동 조작 중으로 일시적 유량 · 압력 편차 발생"],
  ["free_text", "기타 현장 특이사항 직접 입력 (자유 서술 → Zoro-1이 표준 코드로 자동 분류)"],
];

interface FieldReply {
  state: "sent" | "finished" | "hold" | "invalid" | "sign_again" | "excuse";
  receipt_no: string | null;
  notice: string | null;
  message: string | null;
}

const STEP_OF: Record<Phase, number> = { start: 0, scanning: 0, pick: 1, form: 2, excuse: 2, sent: 3, finished: 3 };

/** 현장 순시 · 정비 운전원 단말 (기획 Part 8-2 2절). 내부 판정 코드 · 확신도 · 마인드맵은 이 화면에 오지 않습니다. */
export function KioskView() {
  const [domain, setDomain] = useState<DomainInfo | null>(null);
  const terms: Terms = domain?.terms ?? PLANT_TERMS;
  const aidc = domain?.mode === "aidc";
  const EXCUSES = aidc ? AIDC_EXCUSES : PLANT_EXCUSES;
  // "대상 KKS 코드" · "대상 자산코드" (용어가 이미 "코드" 로 끝나면 덧붙이지 않습니다)
  const codeLabel = terms.code.endsWith("코드") ? `대상 ${terms.code}` : `대상 ${terms.code} 코드`;
  const STEPS = ["도면/계기 스캔", "관할 섹터 선택", `판독값 · ${aidc ? "LoTo" : "LOTO"} 서명`, `${terms.control_room} 전송`];
  const [scanImage, setScanImage] = useState<string | null>(null);
  const [scanSize, setScanSize] = useState<[number, number] | null>(null);
  const [boxes, setBoxes] = useState<Record<string, [number, number, number, number]>>({});
  const [raw, setRaw] = useState<Record<string, string>>({});
  const [ready, setReady] = useState<boolean | null>(null);
  const [phase, setPhase] = useState<Phase>("start");
  const [kind, setKind] = useState<Kind>("gauge_card");
  const [sectors, setSectors] = useState<SectorChoice[]>([]);
  const [docs, setDocs] = useState<string[]>([]);
  const [values, setValues] = useState<Values>({});
  const [prefill, setPrefill] = useState<Values>({});
  const [scanned, setScanned] = useState<string[]>([]);
  const [corrected, setCorrected] = useState<string[]>([]);
  const [paperSigned, setPaperSigned] = useState(false);
  const [usePaper, setUsePaper] = useState(false);
  const [permit, setPermit] = useState(false);
  const [sig, setSig] = useState<SignatureStats | null>(null);
  const [padKey, setPadKey] = useState(0);
  const [signFails, setSignFails] = useState(0);
  const [excuse, setExcuse] = useState("");
  const [excuseText, setExcuseText] = useState("");
  const [excuseMessage, setExcuseMessage] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [receipt, setReceipt] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [hold, setHold] = useState(false);
  const receiptRef = useRef<string | null>(null);

  useEffect(() => {
    invoke<boolean>("field_ready").then(setReady);
    invoke<SectorChoice[]>("field_sectors").then(setSectors);
    invoke<DomainInfo>("domain_info").then(setDomain);
    // 관제 콘솔이 도메인을 바꾸면 처음 화면으로 돌아가 구역 · 용어를 다시 받습니다.
    const stopDomain = listen("domain_changed", () => {
      reset();
      invoke<SectorChoice[]>("field_sectors").then(setSectors);
      invoke<DomainInfo>("domain_info").then(setDomain);
    });
    const stopState = listen<{ ready: boolean }>("field_state", (e) => setReady(e.payload.ready));
    const stopCase = listen<{ receipt_no: string; notice: string; done: boolean }>("field_update", (e) => {
      if (e.payload.receipt_no !== receiptRef.current) return;
      setNotice(e.payload.notice);
      if (e.payload.done) setPhase("finished");
    });
    return () => {
      void stopDomain.then((f) => f());
      void stopState.then((f) => f());
      void stopCase.then((f) => f());
    };
  }, []); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    if (phase !== "finished") return;
    const timer = window.setTimeout(reset, RESET_SECONDS * 1000);
    return () => window.clearTimeout(timer);
  }, [phase]);

  useEffect(() => {
    if (phase !== "pick" && phase !== "form" && phase !== "excuse") return;
    let timer = window.setTimeout(reset, IDLE_RESET_SECONDS * 1000);
    const touched = () => {
      window.clearTimeout(timer);
      timer = window.setTimeout(reset, IDLE_RESET_SECONDS * 1000);
    };
    window.addEventListener("pointerdown", touched);
    window.addEventListener("keydown", touched);
    return () => {
      window.clearTimeout(timer);
      window.removeEventListener("pointerdown", touched);
      window.removeEventListener("keydown", touched);
    };
  }, [phase]);

  function reset() {
    void invoke("field_reset");
    receiptRef.current = null;
    setPhase("start");
    setKind("gauge_card");
    setDocs([]);
    setValues({});
    setPrefill({});
    setScanned([]);
    setCorrected([]);
    setPaperSigned(false);
    setUsePaper(false);
    setPermit(false);
    setSig(null);
    setSignFails(0);
    setExcuse("");
    setExcuseText("");
    setError(null);
    setReceipt(null);
    setNotice(null);
    setHold(false);
    setScanImage(null);
    setScanSize(null);
    setBoxes({});
    setRaw({});
  }

  async function scan() {
    setPhase("scanning");
    setError(null);
    try {
      const r = await invoke<ScanReply>("scan_field_documents");
      const filled: Values = Object.fromEntries(Object.entries(r.prefill).map(([k, v]) => [k, String(v)]));
      setDocs((d) => [...d, ...r.docs]);
      setPrefill((p) => ({ ...p, ...filled }));
      setValues((v) => ({ ...filled, ...v }));
      setScanned((s) => [...s, ...r.scanned]);
      setCorrected((c) => [...c, ...r.corrected]);
      if (r.image && r.image_size) {
        setScanImage(r.image);
        setScanSize(r.image_size);
        setBoxes(r.boxes);
        setRaw(r.raw);
      }
      if (r.signed) {
        setPaperSigned(true);
        setUsePaper(true);
      }
      if (r.kind) setKind(r.kind);
      // 표제란에서 섹터를 알아냈으면 섹터 선택을 건너뜁니다.
      setPhase(filled.sector ? "form" : "pick");
    } catch (e) {
      setError(String(e));
      setPhase("start");
    }
  }

  async function submit(chosenExcuse: string | null) {
    setBusy(true);
    setError(null);
    const edited = Object.keys(prefill).filter((k) => String(prefill[k]) !== String(values[k] ?? ""));
    try {
      const reply = await invoke<FieldReply>("submit_field_report", {
        input: {
          kind,
          sector: values.sector ?? null,
          kks: values.kks ?? "",
          tag: values.tag ?? null,
          reading: kind === "redline" ? values.reading ?? null : values.reading ?? null,
          unit: values.unit ?? null,
          moc_no: values.moc_no ?? null,
          old_value: values.old_value ?? null,
          worker_name: values.worker_name ?? "",
          worker_id: values.worker_id ?? "",
          signature: usePaper ? null : sig,
          paper_signed: usePaper && paperSigned,
          permit_confirmed: permit,
          excuse: chosenExcuse,
          excuse_text: excuseText,
          edited,
        },
      });
      switch (reply.state) {
        case "sent":
          receiptRef.current = reply.receipt_no;
          setReceipt(reply.receipt_no);
          setPhase("sent");
          break;
        case "finished":
        case "hold":
          receiptRef.current = reply.receipt_no;
          setReceipt(reply.receipt_no);
          setNotice(reply.notice);
          setHold(reply.state === "hold");
          setPhase("finished");
          break;
        case "excuse":
          setExcuseMessage(reply.message ?? "");
          setExcuse("");
          setPhase("excuse");
          break;
        case "sign_again":
          setError(reply.message);
          setSig(null);
          setUsePaper(false);
          setPadKey((k) => k + 1);
          setSignFails((n) => n + 1);
          setPhase("form");
          break;
        default:
          setError(reply.message);
          setPhase("form");
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  const sector = sectors.find((s) => s.key === values.sector);
  const missing = !values.worker_name?.trim()
    ? "점검자 성함"
    : !values.worker_id?.trim()
      ? "사번"
      : !values.kks?.trim()
        ? codeLabel
        : kind === "redline"
          ? !values.moc_no?.trim()
            ? `${terms.change} 변경번호`
            : !values.reading?.trim()
              ? "변경 한계"
              : !permit
                ? `${terms.permit} 확인`
                : null
          : !values.reading?.trim()
            ? "현장 실측치"
            : null;
  const signMissing = !usePaper && !sig;

  // 숫자 칸 스테퍼: 장갑을 끼고도 누를 수 있게 키보드 대신 큰 버튼 (개선안 4단계)
  const bump = (key: string, d: number) =>
    setValues((v) => {
      const now = Number((v[key] ?? "").replace(/,/g, ""));
      const base = Number.isFinite(now) ? now : 0;
      return { ...v, [key]: (Math.round((base + d) * 100) / 100).toFixed(1) };
    });
  const field = (key: string, label: string, hint = "", numeric = false) => {
    const box = boxes[key];
    const [small, big] = steps(values.unit);
    return (
      <div className={`field-card ${box ? "with-crop" : ""}`}>
        <span className="field-label">
          {label}
          {corrected.includes(key) ? <em className="from-scan fixed">교정됨</em> : scanned.includes(key) ? <em className="from-scan">스캔됨</em> : null}
        </span>
        {box && scanImage && scanSize && <Crop src={scanImage} size={scanSize} box={box} raw={raw[key] !== values[key] ? raw[key] : undefined} />}
        <div className="value-row">
          {numeric && (
            <>
              <button type="button" className="step" onClick={() => bump(key, -big)}>
                -{big.toFixed(1)}
              </button>
              <button type="button" className="step" onClick={() => bump(key, -small)}>
                -{small.toFixed(1)}
              </button>
            </>
          )}
          <input value={values[key] ?? ""} placeholder={hint} inputMode={numeric ? "decimal" : undefined} onChange={(e) => setValues((v) => ({ ...v, [key]: e.target.value }))} />
          {numeric && (
            <>
              <button type="button" className="step" onClick={() => bump(key, small)}>
                +{small.toFixed(1)}
              </button>
              <button type="button" className="step" onClick={() => bump(key, big)}>
                +{big.toFixed(1)}
              </button>
            </>
          )}
        </div>
      </div>
    );
  };
  const eg = aidc
    ? { id: "FE-205", kks: "DC1MNF10MV01", tag: "DC1MNF10CP01 (모르면 비워 두세요)", moc: "MOP-2026-1104", old: "3.0", now: "3.5", unit: "bar" }
    : { id: "E205", kks: "10MAA10ST001", tag: "10MAA10CP001 (모르면 비워 두세요)", moc: "MOC-2026-1104", old: "29.0", now: "31.0", unit: "MPa" };

  return (
    <main className="kiosk">
      <header className="kiosk-header">
        🛠️ {terms.kiosk_title} (Field Patrol Kiosk)
        {domain && <span className="kiosk-domain">{domain.site}</span>}
      </header>
      <ol className="steps">
        {STEPS.map((s, i) => (
          <li key={s} className={ready && i === STEP_OF[phase] ? "active" : undefined}>
            {i + 1}. {s}
          </li>
        ))}
      </ol>
      {ready !== true ? (
        <section className="bot-card">
          <h1>안내</h1>
          <p>단말을 준비하고 있습니다. 잠시만 기다려 주세요.</p>
        </section>
      ) : phase === "start" ? (
        <section className="kiosk-card start">
          <h2>{aidc ? "현장 점검표, 매니폴드 압력계 사진, 또는 Red-line 변경도(MEP · MOP)를 스캐너에 넣어 주세요" : "현장 점검표, 아날로그 게이지 사진, 또는 수기 수정 도면(Red-line P&ID)을 스캐너에 넣어 주세요"}</h2>
          <p className="lead">도면과 계기판 수치를 자동으로 읽어 칸을 채워 드립니다. 스캔할 도면이 없어도 현장 수치를 입력할 수 있습니다.</p>
          {error && <p className="error">{error}</p>}
          <div className="start-actions">
            <button className="big green" onClick={scan}>
              📷 도면/계기 스캔
            </button>
            <button
              className="big plain"
              onClick={() => {
                setKind("manual");
                setPhase("pick");
              }}
            >
              스캔 없이 수치입력
            </button>
          </div>
        </section>
      ) : phase === "scanning" ? (
        <section className="bot-card">
          <div className="spinner" />
          <p>도면 · 점검표를 읽고 있습니다</p>
          <h1>SigLIP2 로 읽을 곳을 고르고 gocr 로 글자를 읽습니다</h1>
        </section>
      ) : phase === "pick" ? (
        <section className="kiosk-card">
          <h2>어느 섹터(건물)의 설비 점검인가요?</h2>
          <div className="choices">
            {sectors.map((s) => (
              <button
                key={s.key}
                className="choice"
                onClick={() => {
                  setValues((v) => ({ ...v, sector: s.key }));
                  setPhase("form");
                }}
              >
                <strong>
                  {s.building} {s.label} ({s.kks})
                </strong>
                <span>{s.blurb}</span>
              </button>
            ))}
          </div>
          <div className="actions">
            <button className="plain" onClick={reset}>
              처음으로
            </button>
          </div>
        </section>
      ) : phase === "form" ? (
        <section className="kiosk-card">
          <h2>
            {sector ? `${sector.building} ${sector.label}` : ""} {kind === "redline" ? "설비 변경(Red-line) 내용" : "점검 내용"}을 확인해 주세요
          </h2>
          {docs.length > 0 && (
            <div className="chips">
              {docs.map((d, i) => (
                <span key={i} className="chip">
                  {d}
                </span>
              ))}
            </div>
          )}
          <div className="kind-switch">
            {(["gauge_card", "redline", "manual"] as Kind[]).map((k) => (
              <button key={k} className={kind === k ? "chip active" : "chip"} onClick={() => setKind(k)}>
                {k === "gauge_card" ? "현장계기점검표" : k === "redline" ? "설비변경 Red-line" : "현장 수치 직접 입력"}
              </button>
            ))}
          </div>
          <div className="form-grid">
            {field("worker_name", "점검자 성함", "예: 이승민")}
            {field("worker_id", "사번", `예: ${eg.id}`)}
            {field("kks", codeLabel, `예: ${eg.kks}`)}
            {field("tag", "센서 태그", `예: ${eg.tag}`)}
            {kind === "redline" ? (
              <>
                {field("moc_no", `${terms.change} 변경번호`, `예: ${eg.moc}`)}
                {field("old_value", "기존 한계", `예: ${eg.old}`, true)}
                {field("reading", "변경 한계", `예: ${eg.now}`, true)}
              </>
            ) : (
              field("reading", "현장 실측치", "숫자만", true)
            )}
            {field("unit", "단위", `예: ${eg.unit}`)}
          </div>
          <label className="check">
            <input type="checkbox" checked={permit} onChange={(e) => setPermit(e.target.checked)} />
            {aidc ? "현장 작업허가(MOP/Hot-Work) 및 수냉 밸브 잠금표지(LoTo) 체결을 확인합니다" : "현장 안전작업허가(PTW) 및 잠금표지(LOTO) 절차를 준수했음을 확인합니다"} {kind === "redline" ? "(설비 변경은 필수)" : ""}
          </label>
          {paperSigned && (
            <label className="check">
              <input type="checkbox" checked={usePaper} onChange={(e) => setUsePaper(e.target.checked)} />
              스캔한 문서의 서명 · 직인을 씁니다
            </label>
          )}
          {!usePaper && (
            <div className="signature-field">
              <span className="field-label">작업자 서명 (획수 2회 이상 · 면적 15% 이상 · 직선 아님)</span>
              <SignaturePad key={padKey} onChange={setSig} />
            </div>
          )}
          {signFails >= 2 && <p className="muted">장갑 착용 등으로 서명이 어려우면 무전으로 {terms.control_room}에 사번 확인을 요청하세요.</p>}
          {error && <p className="error">{error}</p>}
          <div className="actions">
            <button className="plain" disabled={busy} onClick={reset}>
              처음으로
            </button>
            <button className="plain" disabled={busy} onClick={scan}>
              도면/사진 더 넣기
            </button>
            <button className="green" disabled={busy || missing !== null || signMissing} onClick={() => submit(null)}>
              {busy ? "전송 중입니다" : missing ? `${missing} 칸을 채워 주세요` : signMissing ? "서명해 주세요" : `${terms.control_room}로 전송하기`}
            </button>
          </div>
        </section>
      ) : phase === "excuse" ? (
        <section className="kiosk-card">
          <h2>⚠️ {excuseMessage}</h2>
          <p className="lead">(보안 원칙에 따라 중앙 제어실 내부 원장 수치 및 판정 코드는 화면에 표시되지 않습니다)</p>
          <div className="excuses">
            {EXCUSES.map(([value, label]) => (
              <label key={value} className="check">
                <input type="radio" name="excuse" value={value} checked={excuse === value} onChange={() => setExcuse(value)} />
                {label}
              </label>
            ))}
          </div>
          {excuse === "free_text" && <textarea rows={3} maxLength={300} value={excuseText} placeholder={aidc ? "예: 2층 매니폴드 퀵커플러 교체 뒤 누수 점검 중" : "예: 1층 윤활유 냉각기 스트레이너 차압 청소 중"} onChange={(e) => setExcuseText(e.target.value)} />}
          {error && <p className="error">{error}</p>}
          <div className="actions">
            <button className="plain" disabled={busy} onClick={() => setPhase("form")}>
              이전 화면
            </button>
            <button className="green" disabled={busy || !excuse || (excuse === "free_text" && !excuseText.trim())} onClick={() => submit(excuse)}>
              {busy ? "전송 중입니다" : "소명 포함 전송하기"}
            </button>
          </div>
        </section>
      ) : phase === "sent" ? (
        <section className="kiosk-card done">
          <h2>{terms.control_room}로 전송했습니다</h2>
          <p className="receipt">
            접수 번호 <strong>{receipt}</strong>
          </p>
          <p className="lead">{terms.supervisor}이 확인하고 있습니다. 결과나 재점검 요청이 오면 이 화면에 나옵니다.</p>
          {notice && <pre className="notice-doc">{notice}</pre>}
          <div className="spinner" />
          <div className="actions">
            <button className="plain" onClick={reset}>
              처음으로
            </button>
          </div>
        </section>
      ) : (
        <section className={`kiosk-card done ${hold ? "hold" : ""}`}>
          <h2>{hold ? "확인이 필요합니다" : "처리가 끝났습니다"}</h2>
          <p className="receipt">
            접수 번호 <strong>{receipt}</strong>
          </p>
          <pre className="notice-doc">{notice}</pre>
          <p className="muted">{RESET_SECONDS}초 뒤 처음 화면으로 돌아갑니다.</p>
          <div className="actions">
            <button className="green" onClick={reset}>
              처음으로
            </button>
          </div>
        </section>
      )}
    </main>
  );
}
