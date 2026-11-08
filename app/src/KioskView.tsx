import { invoke } from "./ipc";
import { listen } from "./ipc";
import { useEffect, useRef, useState } from "react";
import { PLANT_TERMS, PLANT_TERMS_EN, type DomainInfo, type Terms } from "./domain";
import { useLanguageSync, useT, type Lang } from "./i18n";
import { tr } from "./i18n/lang";
import { SignaturePad, type SignatureStats } from "./SignaturePad";

const RESET_SECONDS = 40;
const IDLE_RESET_SECONDS = 180;
/** 서명 규칙 (core/src/state_machine/gate1_formal.rs 의 SIGN_MIN_STROKES · SIGN_MIN_AREA 와 같은 값). 판정은 코어가 하고,
 *  단말은 같은 기준으로 보내기 단추를 미리 막아 "보냄 → 서명 다시 → 소명 다시" 왕복을 없앱니다. */
const SIGN_MIN_STROKES = 2;
const SIGN_MIN_AREA = 0.15;

// 관할 섹터 고르기 화면은 없앴습니다: 엔진은 고른 섹터를 쓰지 않고 설비 코드의 원장 섹터로 라우팅 · 관할을 봅니다 (UI 단순화 K1).
type Phase = "start" | "scanning" | "form" | "excuse" | "sent" | "finished";
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
  const t = useT();
  const [x0, y0, x1, y1] = box;
  const ratio = Math.min(((x1 - x0) * size[0]) / Math.max((y1 - y0) * size[1], 1), 9);
  return (
    <div className="crop" title={raw ? t(`스캔 원문: ${raw}`, `As scanned: ${raw}`) : undefined}>
      <div className="crop-view" style={{ aspectRatio: `${ratio}` }}>
        <img src={src} alt="" draggable={false} style={{ width: `${100 / (x1 - x0)}%`, transform: `translate(${-x0 * 100}%, ${-y0 * 100}%)` }} />
      </div>
      {raw && <em className="mono">{t(`스캔 원문 ${raw}`, `As scanned: ${raw}`)}</em>}
    </div>
  );
}

/** 단위별 스테퍼 간격 (장갑을 끼고 누르는 큰 버튼): bar 는 0.1 · 0.5, 그 밖은 0.1 · 1.0 */
function steps(unit: string | undefined): [number, number] {
  return unit?.trim().toLowerCase() === "bar" ? [0.1, 0.5] : [0.1, 1.0];
}

// 소명 고르기 [백엔드로 가는 코드, 화면 글]. 글은 그릴 때의 화면 언어로 만듭니다.
const plantExcuses = (): [string, string][] => [
  ["transmitter_drift", tr("현장 아날로그 게이지는 정상이나 전자식 전송기(Transmitter) 영점 흔들림 의심", "Local analog gauge reads normal; suspect zero drift in the electronic transmitter")],
  ["moc_change", tr("계획예방정비 · 부품 교체(MOC)로 운전 압력 사양이 변경됨 (Red-line 도면 첨부)", "Operating pressure spec changed by planned maintenance / part replacement (MOC) (Red-line drawing attached)")],
  ["manual_bypass", tr("현장 수동 바이패스(Bypass) 밸브 조작 중으로 일시적 유량 · 압력 편차 발생", "Temporary flow/pressure deviation while the bypass valve is operated by hand on site")],
  ["free_text", tr("기타 현장 특이사항 직접 입력 (자유 서술 → Zoro-1이 표준 코드로 자동 분류)", "Other field condition (type it in → Zoro-1 sorts it into a standard code)")],
];

const aidcExcuses = (): [string, string][] => [
  ["transmitter_drift", tr("현장 아날로그 압력계 · 유량계는 정상이나 BMC 텔레메트리(전송기) 값 흔들림 의심", "Local analog pressure/flow gauges read normal; suspect unstable BMC telemetry (transmitter) values")],
  ["moc_change", tr("MOP(변경 작업)로 CDU · 랙 운전 사양이 변경됨 (Red-line 변경도 첨부)", "CDU/rack operating spec changed by MOP (change work) (Red-line change drawing attached)")],
  ["manual_bypass", tr("현장 CDU 우회밸브 수동 조작 중으로 일시적 유량 · 압력 편차 발생", "Temporary flow/pressure deviation while the CDU bypass valve is operated by hand on site")],
  ["free_text", tr("기타 현장 특이사항 직접 입력 (자유 서술 → Zoro-1이 표준 코드로 자동 분류)", "Other field condition (type it in → Zoro-1 sorts it into a standard code)")],
];

/** 창 제목 (한국어는 tauri.conf 의 제목 그대로) */
const kioskTitle = (l: Lang) => (l === "en" ? "blueprint Field Patrol & Maintenance Terminal" : "blueprint 현장 순시 · 정비 단말");

interface FieldReply {
  state: "sent" | "finished" | "hold" | "invalid" | "sign_again" | "excuse";
  receipt_no: string | null;
  notice: string | null;
  message: string | null;
}

const STEP_OF: Record<Phase, number> = { start: 0, scanning: 0, form: 1, excuse: 1, sent: 2, finished: 2 };

/** 현장 순시 · 정비 운전원 단말 (기획 Part 8-2 2절). 내부 판정 코드 · 확신도 · 마인드맵은 이 화면에 오지 않습니다. */
export function KioskView() {
  const t = useT();
  // 콘솔 설정에서 언어를 바꾸면 이 창도 따라가고 창 제목을 그 언어로 맞춥니다.
  const lang = useLanguageSync(kioskTitle);
  const [domain, setDomain] = useState<DomainInfo | null>(null);
  const terms: Terms = domain?.terms ?? (lang === "en" ? PLANT_TERMS_EN : PLANT_TERMS);
  const aidc = domain?.mode === "aidc";
  const EXCUSES = aidc ? aidcExcuses() : plantExcuses();
  // "대상 KKS 코드" · "대상 자산코드" (용어가 이미 "코드" 로 끝나면 덧붙이지 않습니다). 영어는 "KKS Code" · "Asset Code".
  const codeLabel = t(
    terms.code.endsWith("코드") ? `대상 ${terms.code}` : `대상 ${terms.code} 코드`,
    /(code|코드)$/i.test(terms.code) ? terms.code.charAt(0).toUpperCase() + terms.code.slice(1) : `${terms.code} Code`,
  );
  const kindLabel = (k: Kind) => (k === "gauge_card" ? t("현장계기점검표", "Gauge Inspection Sheet") : k === "redline" ? t("설비변경 Red-line", "Equipment Change Red-line") : t("현장 수치 직접 입력", "Manual Field Entry"));
  const STEPS = [
    t("도면/계기 스캔", "Scan Drawing/Gauge"),
    t(`판독값 · ${aidc ? "LoTo" : "LOTO"} 서명`, `Readings · ${aidc ? "LoTo" : "LOTO"} Sign-off`),
    t(`${terms.control_room} 전송`, `Send to ${terms.control_room}`),
  ];
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
  // 스캔이 서류 종류를 정했으면 종류 칩은 접어 두고, 비워 둬도 되는 센서 태그 칸은 [더 보기] 로 (UI 단순화 K3)
  const [kindOpen, setKindOpen] = useState(false);
  const [showTag, setShowTag] = useState(false);
  const receiptRef = useRef<string | null>(null);

  // 구역 이름 · 용어는 백엔드 글이라 받을 때의 화면 언어로 옵니다 (번역은 invoke 감싸기). 언어가 바뀌면 다시 받습니다.
  useEffect(() => {
    invoke<SectorChoice[]>("field_sectors").then(setSectors);
    invoke<DomainInfo>("domain_info").then(setDomain);
  }, [lang]);

  useEffect(() => {
    invoke<boolean>("field_ready").then(setReady);
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
    if (phase !== "form" && phase !== "excuse") return;
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
    setKindOpen(false);
    setShowTag(false);
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
      // 섹터는 묻지 않습니다 (표제란에서 읽었으면 제목에만 보이고, 없으면 엔진이 설비 코드로 정함)
      setPhase("form");
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
    ? t("점검자 성함", "Inspector Name")
    : !values.worker_id?.trim()
      ? t("사번", "Employee ID")
      : !values.kks?.trim()
        ? codeLabel
        : kind === "redline"
          ? !values.moc_no?.trim()
            ? t(`${terms.change} 변경번호`, `${terms.change} No.`)
            : !values.reading?.trim()
              ? t("변경 한계", "New Limit")
              : !permit
                ? t(`${terms.permit} 확인`, `${terms.permit} Confirmation`)
                : null
          : !values.reading?.trim()
            ? t("현장 실측치", "Field Reading")
            : null;
  const signMissing = !usePaper && !sig;
  // 패드가 이미 재는 획수 · 면적 · 직선 여부로 코어와 같은 규칙을 미리 봅니다 (코어 판정은 그대로 남음)
  const signWeak = !usePaper && !!sig && (sig.stroke_count < SIGN_MIN_STROKES || sig.bbox_area_ratio < SIGN_MIN_AREA || sig.single_line);

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
          {corrected.includes(key) ? <em className="from-scan fixed">{t("교정됨", "Corrected")}</em> : scanned.includes(key) ? <em className="from-scan">{t("스캔됨", "Scanned")}</em> : null}
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
    ? { id: "FE-205", kks: "DC1MNF10MV01", tag: t("DC1MNF10CP01 (모르면 비워 두세요)", "DC1MNF10CP01 (leave blank if unknown)"), moc: "MOP-2026-1104", old: "3.0", now: "3.5", unit: "bar" }
    : { id: "E205", kks: "10MAA10ST001", tag: t("10MAA10CP001 (모르면 비워 두세요)", "10MAA10CP001 (leave blank if unknown)"), moc: "MOC-2026-1104", old: "29.0", now: "31.0", unit: "MPa" };
  const example = (s: string) => t(`예: ${s}`, `e.g. ${s}`);

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
          <h1>{t("안내", "Notice")}</h1>
          <p>{t("단말을 준비하고 있습니다. 잠시만 기다려 주세요.", "Getting the terminal ready. Please wait a moment.")}</p>
        </section>
      ) : phase === "start" ? (
        <section className="kiosk-card start">
          <h2>{aidc ? t("현장 점검표, 매니폴드 압력계 사진, 또는 Red-line 변경도(MEP · MOP)를 스캐너에 넣어 주세요", "Place a field inspection sheet, a manifold pressure gauge photo, or a Red-line change drawing (MEP · MOP) in the scanner") : t("현장 점검표, 아날로그 게이지 사진, 또는 수기 수정 도면(Red-line P&ID)을 스캐너에 넣어 주세요", "Place a field inspection sheet, an analog gauge photo, or a hand-marked drawing (Red-line P&ID) in the scanner")}</h2>
          <p className="lead">{t("도면과 계기판 수치를 자동으로 읽어 칸을 채워 드립니다. 스캔할 도면이 없어도 현장 수치를 입력할 수 있습니다.", "We read the drawing and gauge values and fill in the form for you. No drawing to scan? You can still enter field values.")}</p>
          {error && <p className="error">{error}</p>}
          <div className="start-actions">
            <button className="big green" onClick={scan}>
              📷 {t("도면/계기 스캔", "Scan Drawing/Gauge")}
            </button>
            <button
              className="big plain"
              onClick={() => {
                setKind("manual");
                setPhase("form");
              }}
            >
              {t("스캔 없이 수치입력", "Enter Values Without Scanning")}
            </button>
          </div>
        </section>
      ) : phase === "scanning" ? (
        <section className="bot-card">
          <div className="spinner" />
          <p>{t("도면 · 점검표를 읽고 있습니다", "Reading the drawing and inspection sheet")}</p>
          <h1>{t("SigLIP2 로 읽을 곳을 고르고 gocr 로 글자를 읽습니다", "SigLIP2 picks where to read, then gocr reads the text")}</h1>
        </section>
      ) : phase === "form" ? (
        <section className="kiosk-card">
          <h2>
            {t(
              `${sector ? `${sector.building} ${sector.label}` : ""} ${kind === "redline" ? "설비 변경(Red-line) 내용" : "점검 내용"}을 확인해 주세요`,
              `Please check the ${kind === "redline" ? "equipment change (Red-line)" : "inspection details"}${sector ? ` for ${sector.building} ${sector.label}` : ""}`,
            )}
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
            {docs.length > 0 && !kindOpen ? (
              <>
                <span className="chip active">{kindLabel(kind)}</span>
                <button className="plain small" onClick={() => setKindOpen(true)}>
                  {t("종류 바꾸기", "Change type")}
                </button>
              </>
            ) : (
              (["gauge_card", "redline", "manual"] as Kind[]).map((k) => (
                <button key={k} className={kind === k ? "chip active" : "chip"} onClick={() => setKind(k)}>
                  {kindLabel(k)}
                </button>
              ))
            )}
          </div>
          <div className="form-grid">
            {field("worker_name", t("점검자 성함", "Inspector Name"), t("예: 이승민", "e.g. Lee Seung-min"))}
            {field("worker_id", t("사번", "Employee ID"), example(eg.id))}
            {field("kks", codeLabel, example(eg.kks))}
            {showTag || scanned.includes("tag") || values.tag?.trim() ? (
              field("tag", t("센서 태그 (선택)", "Sensor Tag (optional)"), example(eg.tag))
            ) : (
              <button type="button" className="plain more-field" onClick={() => setShowTag(true)}>
                {t("＋ 센서 태그 (선택 · 비우면 설비의 첫 센서)", "＋ Sensor tag (optional · blank = the equipment's first sensor)")}
              </button>
            )}
            {kind === "redline" ? (
              <>
                {field("moc_no", t(`${terms.change} 변경번호`, `${terms.change} No.`), example(eg.moc))}
                {field("old_value", t("기존 한계", "Old Limit"), example(eg.old), true)}
                {field("reading", t("변경 한계", "New Limit"), example(eg.now), true)}
              </>
            ) : (
              field("reading", t("현장 실측치", "Field Reading"), t("숫자만", "Numbers only"), true)
            )}
            {field("unit", t("단위", "Unit"), example(eg.unit))}
          </div>
          <label className="check">
            <input type="checkbox" checked={permit} onChange={(e) => setPermit(e.target.checked)} />
            {aidc
              ? t("현장 작업허가(MOP/Hot-Work) 및 수냉 밸브 잠금표지(LoTo) 체결을 확인합니다", "I confirm the field work permit (MOP/Hot-Work) and the coolant valve lockout/tagout (LoTo) are in place")
              : t("현장 안전작업허가(PTW) 및 잠금표지(LOTO) 절차를 준수했음을 확인합니다", "I confirm I followed the field safe work permit (PTW) and lockout/tagout (LOTO) procedures")}{" "}
            {kind === "redline" ? t("(설비 변경은 필수)", "(required for equipment changes)") : ""}
          </label>
          {paperSigned && (
            <label className="check">
              <input type="checkbox" checked={usePaper} onChange={(e) => setUsePaper(e.target.checked)} />
              {t("스캔한 문서의 서명 · 직인을 씁니다", "Use the signature/seal on the scanned document")}
            </label>
          )}
          {!usePaper && (
            <div className="signature-field">
              <span className="field-label">{t("작업자 서명 (획수 2회 이상 · 면적 15% 이상 · 직선 아님)", "Worker Signature (2+ strokes · 15%+ of the pad · not a straight line)")}</span>
              <SignaturePad key={padKey} onChange={setSig} />
            </div>
          )}
          {signFails >= 2 && <p className="muted">{t(`장갑 착용 등으로 서명이 어려우면 무전으로 ${terms.control_room}에 사번 확인을 요청하세요.`, `If gloves make signing hard, radio the ${terms.control_room} to confirm your employee ID.`)}</p>}
          {error && <p className="error">{error}</p>}
          <div className="actions">
            <button className="plain" disabled={busy} onClick={reset}>
              {t("처음으로", "Start Over")}
            </button>
            <button className="plain" disabled={busy} onClick={scan}>
              {t("도면/사진 더 넣기", "Add Drawing/Photo")}
            </button>
            <button className="green" disabled={busy || missing !== null || signMissing || signWeak} onClick={() => submit(null)}>
              {busy
                ? t("전송 중입니다", "Sending…")
                : missing
                  ? t(`${missing} 칸을 채워 주세요`, `Required: ${missing}`)
                  : signMissing
                    ? t("서명해 주세요", "Please sign")
                    : signWeak
                      ? t("서명을 다시 해 주세요 (획 2회 이상 · 면적 15% 이상 · 직선 아님)", "Please sign again (2+ strokes · 15%+ of the pad · not a straight line)")
                      : t(`${terms.control_room}로 전송하기`, `Send to ${terms.control_room}`)}
            </button>
          </div>
        </section>
      ) : phase === "excuse" ? (
        <section className="kiosk-card">
          <h2>⚠️ {excuseMessage}</h2>
          <p className="lead">{t("(보안 원칙에 따라 중앙 제어실 내부 원장 수치 및 판정 코드는 화면에 표시되지 않습니다)", "(For security, internal control room data is not shown on this screen)")}</p>
          <div className="excuses">
            {EXCUSES.map(([value, label]) => (
              <label key={value} className="check">
                <input type="radio" name="excuse" value={value} checked={excuse === value} onChange={() => setExcuse(value)} />
                {label}
              </label>
            ))}
          </div>
          {excuse === "free_text" && (
            <textarea
              rows={3}
              maxLength={300}
              value={excuseText}
              placeholder={aidc ? t("예: 2층 매니폴드 퀵커플러 교체 뒤 누수 점검 중", "e.g. Leak check after replacing a 2F manifold quick coupler") : t("예: 1층 윤활유 냉각기 스트레이너 차압 청소 중", "e.g. Cleaning the 1F lube oil cooler strainer (differential pressure)")}
              onChange={(e) => setExcuseText(e.target.value)}
            />
          )}
          {error && <p className="error">{error}</p>}
          <div className="actions">
            <button className="plain" disabled={busy} onClick={reset}>
              {t("처음으로", "Start Over")}
            </button>
            <button className="plain" disabled={busy} onClick={() => setPhase("form")}>
              {t("이전 화면", "Back")}
            </button>
            <button className="green" disabled={busy || !excuse || (excuse === "free_text" && !excuseText.trim())} onClick={() => submit(excuse)}>
              {busy ? t("전송 중입니다", "Sending…") : t("소명 포함 전송하기", "Send with Justification")}
            </button>
          </div>
        </section>
      ) : phase === "sent" ? (
        <section className="kiosk-card done">
          <h2>{t(`${terms.control_room}로 전송했습니다`, `Sent to the ${terms.control_room}`)}</h2>
          <p className="receipt">
            {t("접수 번호", "Receipt No.")} <strong>{receipt}</strong>
          </p>
          <p className="lead">{t(`${terms.supervisor}이 확인하고 있습니다. 결과나 재점검 요청이 오면 이 화면에 나옵니다.`, `The ${terms.supervisor} is reviewing it. Any result or recheck request will show up on this screen.`)}</p>
          {notice && <pre className="notice-doc">{notice}</pre>}
          <div className="spinner" />
          <div className="actions">
            <button className="plain" onClick={reset}>
              {t("처음으로", "Start Over")}
            </button>
          </div>
        </section>
      ) : (
        <section className={`kiosk-card done ${hold ? "hold" : ""}`}>
          <h2>{hold ? t("확인이 필요합니다", "Further check needed") : t("처리가 끝났습니다", "Processing complete")}</h2>
          <p className="receipt">
            {t("접수 번호", "Receipt No.")} <strong>{receipt}</strong>
          </p>
          <pre className="notice-doc">{notice}</pre>
          <p className="muted">{t(`${RESET_SECONDS}초 뒤 처음 화면으로 돌아갑니다.`, `Returning to the start screen in ${RESET_SECONDS} seconds.`)}</p>
          <div className="actions">
            <button className="green" onClick={reset}>
              {t("처음으로", "Start Over")}
            </button>
          </div>
        </section>
      )}
    </main>
  );
}
