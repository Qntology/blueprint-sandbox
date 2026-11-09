import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AlarmQueue } from "./AlarmQueue";
import { chooseLang, useLang, useLanguageSync, useT, type Lang } from "./i18n";
import { invoke, listen } from "./ipc";
import { CctvZoom } from "./cctv/CctvZoom";
import { CommandBar, contextual, SearchDropup, searchMarks, searchSummary, type SearchResult, type SearchRun } from "./cctv/CommandBar";
import { SdsDock, TimeSeriesDock } from "./cctv/Docks";
import { RightRail } from "./cctv/RightRail";
import { HUD_W, TOP_INSET, useElementSize, type Insets } from "./cctv/stage";
import { causalTraces } from "./cctv/traces";
import { DEFAULT_SUPERVISOR, DOMAIN_ICON, PLANT_TERMS, PLANT_TERMS_EN, TermsContext, type DomainInfo, type DomainMode } from "./domain";
import { setEngineer, useEngineer } from "./engineer";
import { ModelSettings } from "./ModelSettings";
import { PhysicsPanel } from "./PhysicsPanel";
import { fmt } from "./plant";
import { PlantTopologyView, type Tab } from "./PlantTopologyView";
import { StageBar, StageDrawer } from "./StagePanel";
import { TimeScrubber } from "./TimeScrubber";
import type { ArchiveDoc, Bottleneck, ConsoleStatus, Detail, IncidentRow, Lane, Plant, ScenarioInfo, VerdictInfo } from "./types";

export type DrawerKey = 1 | 2 | 3 | 4 | 5 | "archive" | "physics" | null;
/** 떠 있는 팝오버 (한 번에 하나): 좌상단 사건 대기열 · 우상단 시연 도구 · 관제 설정 */
type Popover = "queue" | "demo" | "settings" | null;
/** 허브 위로 펼치는 판 (한 번에 하나): 검색 결과 · 시계열 · SDS 차트 */
type Dropup = "search" | "charts" | null;
/** 허브 위 한 줄 알림. `action` 은 알림에서 바로 하는 일 (보관 문서 열기 · 도착한 현장 보고 열기) */
interface Notice {
  kind: "ok" | "err" | "info";
  text: string;
  action?: { label: string; run: () => void };
}

const DARK_KEY = "blueprint.drawing-dark";
const DEMO_KEY = "blueprint.demo-tools";

function readPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

function writePref(key: string, value: string) {
  try {
    localStorage.setItem(key, value);
  } catch {
    // 저장소가 막혀 있으면 이번 실행에만
  }
}

/** 관제 콘솔 (기획 Part 8-1 · 개선안 CCTV형 통합 관제판 · 2차 개선안 풀스크린 오버레이 · 추가 개선안 도메인 모드 · UI 단순화 기획안).
 *  고른 3-Way 도면 뷰 하나가 무대 전체에 깔리고, 그 위에 반투명 유리 판이 뜹니다:
 *  좌상단 브랜드 · 사건 대기열 알약과 도면 도구줄, 우상단 뷰 전환 · 흐름 오버레이 · 시연 도구 · 설정 알약,
 *  오른쪽 병목 · 인터록 · 결재 HUD(결재는 여기 한 곳), 아래 가운데 파이프라인 · 타임머신 · 자연어 질의 허브.
 *  팝오버 · Drop-up · 서랍은 종류마다 한 번에 하나만 열리고, ESC 는 맨 위의 것부터 닫습니다. */
export function OfficerView() {
  const [plant, setPlant] = useState<Plant | null>(null);
  const [domain, setDomain] = useState<DomainInfo | null>(null);
  const [catalog, setCatalog] = useState<Record<string, VerdictInfo>>({});
  const [scenarios, setScenarios] = useState<ScenarioInfo[]>([]);
  const [status, setStatus] = useState<ConsoleStatus | null>(null);
  const [rows, setRows] = useState<IncidentRow[]>([]);
  const [incident, setIncident] = useState<string | null>(null);
  const [detail, setDetail] = useState<Detail | null>(null);
  const [archive, setArchive] = useState<ArchiveDoc | null>(null);
  const [selectedKks, setSelectedKks] = useState<string | null>(null);
  const [mining, setMining] = useState(true);
  const [t, setT] = useState<number | null>(null);
  const [drawer, setDrawer] = useState<DrawerKey>(null);
  const [popover, setPopover] = useState<Popover>("queue");
  const [dropup, setDropup] = useState<Dropup>(null);
  const [supervisor, setSupervisor] = useState("OP-201");
  const [opening, setOpening] = useState(false);
  const [basis, setBasis] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [tab, setTab] = useState<Tab>("drawing");
  const [hudFolded, setHudFolded] = useState(false);
  const [hudFocus, setHudFocus] = useState(0);
  const [search, setSearch] = useState<SearchRun | null>(null);
  const [context, setContext] = useState(true);
  const [zoomKks, setZoomKks] = useState<string | null>(null);
  const [lanes, setLanes] = useState<Lane[]>([]);
  const [bottlenecks, setBottlenecks] = useState<Bottleneck[]>([]);
  const [dark, setDark] = useState(() => readPref(DARK_KEY) === "1");
  // 시연 · 시험 도구 (시나리오 주입 · 물리엔진 · 가드레일 시험 단추): 운영 화면에서는 설정에서 숨깁니다
  const [demo, setDemo] = useState(() => readPref(DEMO_KEY) !== "0");
  // 엔지니어 보기 (코드 이름 함께 보이기, 기본 끔): 모듈 변수라 상세 서랍 제목 같은 React 밖 글도 따라갑니다
  const engineer = useEngineer();
  const [resetArmed, setResetArmed] = useState(false);
  const [barSlot, setBarSlot] = useState<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const hubCard = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const stage = useElementSize(stageRef);
  const lang = useLang();
  const say = useT();
  const sayRef = useRef(say);
  sayRef.current = say;
  const terms = plant?.terms ?? domain?.terms ?? (lang === "en" ? PLANT_TERMS_EN : PLANT_TERMS);
  // 창 제목도 화면 언어를 따릅니다 (용어는 백엔드 원장의 것을 번역 표가 바꿈)
  useLanguageSync(useCallback(() => `blueprint ${terms.console_title} (${terms.control_room})`, [terms]));

  const fail = useCallback((e: unknown) => setNotice({ kind: "err", text: String(e) }), []);
  const toggle = (p: Exclude<Popover, null>) => setPopover((o) => (o === p ? null : p));
  const openSettings = useCallback(() => setPopover("settings"), []);

  // 원장(큰 응답)은 처음 · 도메인 · 언어 · 변경 오버레이 수가 바뀔 때만 받습니다
  const loadPlant = useCallback(() => {
    invoke<Plant>("plant_topology").then(setPlant).catch(fail);
  }, [fail]);

  // 도메인마다 달라지는 것: 판정 이름(구역) · 시나리오 목록 · 용어
  const loadDomain = useCallback(() => {
    invoke<VerdictInfo[]>("verdict_catalog").then((list) => setCatalog(Object.fromEntries(list.map((v) => [v.code, v]))));
    invoke<ScenarioInfo[]>("scenario_list").then(setScenarios);
    invoke<DomainInfo>("domain_info").then((d) => {
      setDomain(d);
      setSupervisor((s) => (Object.values(DEFAULT_SUPERVISOR).includes(s) ? DEFAULT_SUPERVISOR[d.mode] : s));
    });
  }, []);

  // 프로세스 마이닝 (레인 · 병목): 파이프라인 카드 · 결재 HUD 가 같은 값을 씁니다.
  const loadMining = useCallback(() => {
    invoke<{ lanes: Lane[]; bottlenecks: Bottleneck[] }>("mining_overview")
      .then((m) => {
        setLanes(m.lanes);
        setBottlenecks(m.bottlenecks);
      })
      .catch(() => {});
  }, []);

  const incidentRef = useRef<string | null>(null);
  incidentRef.current = incident;
  /** 마지막으로 받은 사건 목록 (null = 처음 · 도메인 전환 직후라 도착 알림을 내지 않음) */
  const rowsRef = useRef<IncidentRow[] | null>(null);
  const pickRef = useRef<(id: string) => void>(() => {});

  /** 목록을 바꾸고, 현장 단말에서 새로 온 보고(FLD)나 현장 회신을 받아 결재로 넘어간 사건을 알립니다 */
  const applyRows = useCallback((list: IncidentRow[]) => {
    const prev = rowsRef.current;
    rowsRef.current = list;
    setRows(list);
    if (!prev) return;
    const before = new Map(prev.map((r) => [r.id, r]));
    const arrived = list.find((r) => r.state !== "done" && ((!before.has(r.id) && r.id.startsWith("FLD")) || (before.get(r.id)?.state === "awaiting_field" && r.state === "review")));
    if (!arrived) return;
    const s = sayRef.current;
    setNotice({
      kind: "info",
      text: s(`📨 현장 보고 도착 · ${arrived.building} ${arrived.kks} · ${arrived.id}`, `📨 Field report received · ${arrived.building} ${arrived.kks} · ${arrived.id}`),
      action: { label: s("열기", "Open"), run: () => pickRef.current(arrived.id) },
    });
  }, []);

  /** 사건 하나를 엽니다. 처리가 끝난 사건(목록 상태 done)은 보관 문서를, 진행 중이면 상세를 받습니다. */
  const loadCase = useCallback(
    (id: string, list: IncidentRow[], keepSelection: boolean, showArchive = false) => {
      if (list.find((r) => r.id === id)?.state === "done") {
        setDetail(null);
        invoke<ArchiveDoc>("archive_detail", { id })
          .then((a) => {
            setArchive(a);
            if (showArchive) setDrawer("archive");
          })
          .catch(fail);
        return;
      }
      setArchive(null);
      invoke<Detail>("incident_detail", { id })
        .then((d) => {
          setDetail(d);
          // 보관 문서 서랍은 진행 중인 사건을 고르면 닫습니다 (빈 서랍이 화면을 가리지 않게)
          setDrawer((open) => (open === "archive" ? null : open));
          if (!keepSelection) {
            setT(d.timeline.now_ms);
            setSelectedKks(d.analysis.substantive?.causal?.root?.kks ?? d.analysis.route.kks);
          }
        })
        .catch(() => setDetail(null));
    },
    [fail],
  );

  /** 백엔드가 사건이 바뀌었다고 알릴 때 (incident_changed) 한 번: 상태 줄 · 목록 · 고른 사건 · 레인 */
  const reload = useCallback(() => {
    invoke<ConsoleStatus>("console_status").then(setStatus).catch(fail);
    invoke<IncidentRow[]>("incident_list")
      .then((list) => {
        applyRows(list);
        const id = incidentRef.current;
        if (id) loadCase(id, list, true);
      })
      .catch(fail);
    loadMining();
  }, [applyRows, fail, loadCase, loadMining]);
  const reloadRef = useRef(reload);
  reloadRef.current = reload;

  const resetCase = useCallback(() => {
    setIncident(null);
    setDetail(null);
    setArchive(null);
    setSelectedKks(null);
    setT(null);
    setDrawer(null);
    setDropup(null);
    setNotice(null);
    setSearch(null);
    setZoomKks(null);
  }, []);

  useEffect(() => {
    loadPlant();
    loadDomain();
    reloadRef.current();
    const stop = listen("incident_changed", () => reloadRef.current());
    const stopDomain = listen("domain_changed", () => {
      resetCase();
      rowsRef.current = null;
      loadPlant();
      loadDomain();
      reloadRef.current();
    });
    // 대기 시간(⏳ mm:ss)만 흐르게 하는 느린 예비: 목록 · 레인만 받고, 고른 사건의 상세는 다시 받지 않습니다
    const timer = window.setInterval(() => {
      invoke<IncidentRow[]>("incident_list")
        .then((list) => applyRows(list))
        .catch(() => {});
      loadMining();
    }, 5000);
    return () => {
      void stop.then((f) => f());
      void stopDomain.then((f) => f());
      window.clearInterval(timer);
    };
  }, [applyRows, loadDomain, loadMining, loadPlant, resetCase]);

  // 승인된 변경(MOC)이 반영되면 원장 오버레이 수가 바뀝니다. 그때만 원장을 다시 받습니다.
  const lastOverlays = useRef<number | null>(null);
  useEffect(() => {
    if (!status) return;
    const n = status.overlays.length;
    if (lastOverlays.current !== null && lastOverlays.current !== n) loadPlant();
    lastOverlays.current = n;
  }, [status, loadPlant]);

  // 새 설비를 고르면 질의 문맥을 다시 켭니다 (문맥 칩 ✕ 는 그 설비에서만 끕니다)
  useEffect(() => setContext(true), [selectedKks]);

  // 화면 언어를 바꾸면 받아 둔 글을 그 언어로 다시 받습니다 (번역은 invoke 감싸기가 함)
  const first = useRef(true);
  useEffect(() => {
    if (first.current) {
      first.current = false;
      return;
    }
    loadPlant();
    loadDomain();
    reloadRef.current();
    setSearch(null);
    setDropup((d) => (d === "search" ? null : d));
  }, [lang]); // eslint-disable-line react-hooks/exhaustive-deps

  // 하단 허브 높이: 서랍 · 차트 Drop-up 이 허브 위에서 멈춥니다
  useLayoutEffect(() => {
    const card = hubCard.current;
    const st = stageRef.current;
    if (!card || !st) return;
    const write = () => st.style.setProperty("--hub-h", `${Math.ceil(card.parentElement?.offsetHeight ?? card.offsetHeight)}px`);
    write();
    const ro = new ResizeObserver(write);
    ro.observe(card.parentElement ?? card);
    return () => ro.disconnect();
  }, []);

  function pick(id: string) {
    setIncident(id);
    setNotice(null);
    setPopover((p) => (p === "queue" ? null : p));
    loadCase(id, rowsRef.current ?? [], false, true);
  }
  pickRef.current = pick;

  const openArchive = useCallback(
    (id: string) => {
      invoke<ArchiveDoc>("archive_detail", { id })
        .then((a) => {
          setArchive(a);
          setDrawer("archive");
        })
        .catch(fail);
    },
    [fail],
  );

  /** 결재 결과: 막히면 사유, 확정되면 [보관 문서 열기] 가 붙은 알림 (보관 문서 서랍은 저절로 열리지 않습니다) */
  const onDecided = useCallback(
    (msg: string, ok: boolean, finalized: boolean) => {
      const id = incidentRef.current;
      setNotice({
        kind: !ok ? "err" : finalized ? "ok" : "info",
        text: msg,
        action: finalized && id ? { label: sayRef.current("🗄️ 보관 문서 열기", "🗄️ Open Archived Record"), run: () => openArchive(id) } : undefined,
      });
    },
    [openArchive],
  );

  async function inject(id: string) {
    // 주입은 판단까지만 기다립니다. 모델 초안은 사건이 열린 뒤 뒤에서 채워집니다 (P6)
    setBusy(say(`시나리오 ${id} 주입 중 (Zoro-1 판단)`, `Injecting scenario ${id} (Zoro-1 judgment)`));
    setNotice(null);
    setPopover(null);
    try {
      const created = await invoke<string | null>("inject_scenario", { id });
      if (created) pick(created);
      else {
        const scans = scenarios.find((s) => s.id === id)?.scans.join(", ");
        setNotice({
          kind: "info",
          text: say(`${id}: 알람 없이 계측만 바뀌었습니다. 현장 단말에서 Red-line 을 스캔해 보내세요 (스캐너 폴더에 ${scans}).`, `${id}: measurements changed without an alarm. Scan the Red-line on the field terminal and send it (scanner folder: ${scans}).`),
        });
      }
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
    }
  }

  async function switchDomain(mode: DomainMode) {
    if (mode === domain?.mode) return;
    setBusy(
      say(
        `${mode === "aidc" ? "AI 데이터센터" : "화력발전소"} 모드로 바꾸는 중 (원장 · 시나리오 · 도면 색인)`,
        `Switching to ${mode === "aidc" ? "AI Data Center" : "Thermal Power Plant"} mode (ledger · scenarios · drawing index)`,
      ),
    );
    setNotice(null);
    try {
      // 화면 비우기 · 다시 받기는 백엔드의 domain_changed 알림이 한 번에 합니다
      const d = await invoke<DomainInfo>("switch_domain_mode", { mode });
      setDomain(d);
      setSupervisor(DEFAULT_SUPERVISOR[d.mode]);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
    }
  }

  /** 같은 도메인을 처음 상태로 (사건 · 감사 · 물리엔진 · 현장 단말). 앱을 다시 띄우지 않고 E2E · 시연을 새로 시작합니다. */
  async function resetSession() {
    if (!resetArmed) {
      setResetArmed(true);
      window.setTimeout(() => setResetArmed(false), 4000);
      return;
    }
    setResetArmed(false);
    setBusy(say("세션을 처음 상태로 되돌리는 중 (사건 · 감사 · 물리엔진 · 현장 단말)", "Resetting the session (incidents · audit · physics · field terminal)"));
    try {
      await invoke("reset_session");
      setPopover(null);
    } catch (e) {
      fail(e);
    } finally {
      setBusy(null);
    }
  }

  async function toggleAuto(on: boolean) {
    setNotice(null);
    try {
      await invoke("set_automation", { input: { on, approver_id: supervisor, legal_basis: basis } });
      setOpening(false);
      setBasis("");
    } catch (e) {
      fail(e);
    }
  }

  function setDemoPref(on: boolean) {
    setDemo(on);
    writePref(DEMO_KEY, on ? "1" : "0");
    if (!on) {
      setPopover((p) => (p === "demo" ? null : p));
      setDrawer((d) => (d === "physics" ? null : d));
    }
  }

  // 자연어 질의: 선택 설비 문맥을 붙일지 정하고, 1위가 조건을 통과한 설비면 세 뷰를 그 설비로 옮깁니다 (2차 개선안 2-②).
  const runSearch = useCallback(
    async (text: string) => {
      if (!plant) return false;
      const kks = context ? selectedKks : null;
      const sent = kks ? contextual(text, kks, plant) : text;
      try {
        // 엔진이 한국어 · 영어 질의를 그대로 읽습니다 (예전 i18n/query.ts 의 영어 → 한국어 낱말 옮기기는 엔진 lexicon 으로 옮겼습니다).
        const engine = sent;
        const result = await invoke<SearchResult>("search", { query: engine });
        setSearch({ typed: text, sent, engine, context: sent !== text ? kks : null, result });
        // 알림 줄은 하나: 새 질의의 요약이 앞 알림을 대신합니다
        setNotice(null);
        setDropup("search");
        const top = result.hits[0];
        if (top?.passed && top.doc.kks && top.doc.kks !== selectedKks) setSelectedKks(top.doc.kks);
        return true;
      } catch (e) {
        fail(e);
        return false;
      }
    },
    [plant, context, selectedKks, fail],
  );

  // ESC 는 맨 위 판부터 하나씩 닫고(noto-sandbox overlays.js 의 STACK), 닫을 판이 없으면 검색 표시 → 설비 선택을 풉니다.
  // Enter · / 는 어디서든 질의 줄로 (다른 입력 칸 · 단추에 초점이 있을 때는 그대로).
  const keys = useRef({ zoomKks, popover, dropup, drawer, search, selectedKks });
  keys.current = { zoomKks, popover, dropup, drawer, search, selectedKks };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const s = keys.current;
      if (e.key === "Escape") {
        // 화면에 겹친 순서대로: 확대 창(100) → 팝오버(25) → 허브 Drop-up(22) → 서랍(20) → 검색 표시 → 설비 선택
        if (s.zoomKks) setZoomKks(null);
        else if (s.popover) setPopover(null);
        else if (s.dropup) setDropup(null);
        else if (s.drawer !== null) setDrawer(null);
        else if (s.search) setSearch(null);
        else if (s.selectedKks) setSelectedKks(null);
        return;
      }
      if ((e.key === "/" || e.key === "Enter") && !e.ctrlKey && !e.metaKey && !e.altKey) {
        const a = document.activeElement as HTMLElement | null;
        const idle = !a || a === document.body || a.matches("canvas");
        // / 는 글 칸이 아니면 어디서든(단추를 누른 뒤에도), Enter 는 단추를 누르는 키이기도 하니 빈 곳에서만
        if (e.key === "/" ? a?.matches("input, textarea, select, [contenteditable]") : !idle) return;
        e.preventDefault();
        inputRef.current?.focus();
      }
    };
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, []);

  const selected = useMemo(() => plant?.equipment.find((e) => e.kks === selectedKks) ?? null, [plant, selectedKks]);
  const crumb = selected?.breadcrumb ?? [];
  const at = t ?? detail?.timeline.now_ms ?? 0;
  // 트립(셧다운) 잔여: 경보 센서의 TTF 와 보호 정지 한계. 머리 숫자는 타임머신의 배지 한 곳에만 보입니다.
  const ttf = detail ? (detail.analysis.trigger.ttf_secs ?? detail.analysis.substantive?.ttf_secs ?? null) : null;
  const alarmSensor = detail?.analysis.route.tag ? plant?.sensors.find((s) => s.tag === detail.analysis.route.tag) : undefined;
  const tripLimit = alarmSensor ? (alarmSensor.trip_high ?? alarmSensor.trip_low) : null;
  const traces = useMemo(() => (detail && plant ? causalTraces(detail, plant) : []), [detail, plant]);
  const marks = useMemo(() => searchMarks(search), [search]);
  const interlockKks = useMemo(() => new Set((detail?.analysis.interlock?.steps ?? []).map((s) => s.kks).concat(detail?.analysis.interlock ? [detail.analysis.route.kks] : [])), [detail]);
  const hudOpen = !hudFolded;
  const inset: Insets = { top: TOP_INSET, left: 12, bottom: 12, right: hudOpen ? HUD_W + 24 : 12 };
  const waiting = rows.filter((r) => r.state !== "done").length;
  const onInterlock = detail?.analysis.interlock
    ? () => {
        setHudFolded(false);
        setHudFocus((n) => n + 1);
      }
    : null;
  const searchToast = search ? searchSummary(search, say) : null;
  const TABS: [Tab, string][] = [
    ["drawing", say(`① ${terms.drawing} 도면`, `① ${terms.drawing} Drawing`)],
    ["mermaid", say("② Mermaid 계통", "② Mermaid Diagram")],
    ["mindmap", say("③ 마인드맵", "③ Mind Map")],
  ];

  return (
    <TermsContext.Provider value={terms}>
      <main className={`console ${hudOpen ? "has-hud" : ""}`} data-scenario={detail?.scenario ?? ""} data-incident={incident ?? ""}>
        <div className="stage" ref={stageRef}>
          {plant ? (
            <PlantTopologyView
              plant={plant}
              detail={detail}
              selected={selectedKks}
              onSelect={setSelectedKks}
              mining={mining}
              t={at}
              incident={incident}
              tab={tab}
              stageRef={stageRef}
              stage={stage}
              inset={inset}
              bar={barSlot}
              marks={marks}
              dark={dark}
              onDark={(on) => {
                setDark(on);
                writePref(DARK_KEY, on ? "1" : "0");
              }}
              onZoom={setZoomKks}
              onInterlock={onInterlock}
            />
          ) : (
            <div className="world-empty">{say("계통을 읽는 중입니다", "Loading the plant topology")}</div>
          )}

          {/* 좌상단: 브랜드 · 사건 대기열, 뷰 도구줄(도면 선택 · 전체 계통 · A0), 공간 Depth(설비를 고를 때만) */}
          <header className="corner-tl">
            <div className="corner-row ov-avoid">
              <span className="brand-mark ov-pill">
                ◆ blueprint <span className="brand-title" title={terms.console_title}>{terms.console_title}</span>
              </span>
              <button className={`ov-pill queue-pill ${waiting ? "hot" : ""}`} aria-expanded={popover === "queue"} aria-controls="incident-popover" onClick={() => toggle("queue")}>
                {say(`🚨 사건 대기열 · 대기 ${waiting}건`, `🚨 Incident Queue · ${waiting} pending`)} {popover === "queue" ? "▴" : "▾"}
              </button>
            </div>
            <div className="corner-slot ov-avoid" ref={setBarSlot} />
            {crumb.length > 0 && (
              <nav className="breadcrumb ov-bar ov-avoid" aria-label={say("공간 Depth", "Spatial Depth")}>
                <span className="crumb-title">{say("공간 Depth", "Spatial Depth")}</span>
                {crumb.map((c, i) => (
                  <span key={i} className="crumb">
                    <em>Lv{i + 1}.</em> {c}
                    {i < crumb.length - 1 && <b>▸</b>}
                  </span>
                ))}
              </nav>
            )}
          </header>

          {/* 우상단: 3-Way 뷰 · 흐름 오버레이 · 시연 도구 · 설정 */}
          <div className="corner-tr">
            <div className="corner-row ov-avoid">
              <div className="view-pill ov-pill" role="tablist" aria-label={say("3-Way 도면 뷰", "3-Way drawing views")}>
                {TABS.map(([key, label]) => (
                  <button key={key} role="tab" aria-selected={tab === key} className={tab === key ? "tab active" : "tab"} onClick={() => setTab(key)}>
                    {label}
                  </button>
                ))}
              </div>
              <label className={`mining-toggle ov-pill ${mining ? "on" : ""}`} title={say("배관 흐름 · 병목 · 근본 원인 물결을 도면 · 마인드맵 위에 겹칩니다", "Overlay pipe flow, bottlenecks and root-cause ripples on the drawing and mind map")}>
                <input type="checkbox" checked={mining} onChange={(e) => setMining(e.target.checked)} />
                {say("🌊 흐름 · 병목", "🌊 Flow · Bottleneck")} {mining ? "ON" : "OFF"}
              </label>
              {demo && (
                <button className="ov-pill demo-pill" aria-expanded={popover === "demo"} aria-controls="demo-popover" onClick={() => toggle("demo")} title={say("시나리오 주입 · 물리엔진 · 세션 초기화 (설정에서 숨김)", "Scenario injection · physics engine · session reset (hide in Settings)")}>
                  {say("🧪 시연 · 시험", "🧪 Demo · Test")} {popover === "demo" ? "▴" : "▾"}
                </button>
              )}
              <button className={`ov-pill settings-pill ${status?.automation ? "auto" : ""}`} aria-expanded={popover === "settings"} onClick={() => toggle("settings")}>
                ⚙️ {supervisor} · {status?.automation ? say("자동화 모드", "Automation Mode") : say("보조 모드", "Assist Mode")} ▾
              </button>
            </div>
          </div>

          {/* 사건 대기열 팝오버: 확인 대기 · 처리 완료 (시나리오 주입은 시연 판으로 옮겼습니다) */}
          <section className="incident-popover ov-panel vp-glass ov-avoid" id="incident-popover" hidden={popover !== "queue"} aria-label={say("사건 대기열", "Incident Queue")}>
            <div className="ov-head">
              <b>{say("🚨 사건 대기열", "🚨 Incident Queue")}</b>
              <button className="ov-close" onClick={() => setPopover(null)} aria-label={say("닫기", "Close")}>
                ✕
              </button>
            </div>
            <AlarmQueue rows={rows} selected={incident} onPick={pick} catalog={catalog} demo={demo} />
          </section>

          {/* 시연 · 시험 도구 팝오버: 시나리오 주입 · 물리엔진 시뮬레이터 · 세션 초기화 */}
          {demo && (
            <section className="demo-popover ov-panel vp-glass ov-avoid" id="demo-popover" hidden={popover !== "demo"} aria-label={say("시연 · 시험 도구", "Demo · test tools")}>
              <div className="ov-head">
                <b>{say("🧪 시연 · 시험 도구", "🧪 Demo · Test Tools")}</b>
                <span className="muted">{say("명령은 시뮬레이터 · 합성 원장에만 들어갑니다", "Commands go to the simulator and synthetic ledger only")}</span>
                <button className="ov-close" onClick={() => setPopover(null)} aria-label={say("닫기", "Close")}>
                  ✕
                </button>
              </div>
              <select
                className="sim"
                value=""
                disabled={!!busy}
                onChange={(e) => {
                  if (e.target.value) void inject(e.target.value);
                }}
              >
                <option value="">{say(`${terms.telemetry} 시뮬레이터 · 시나리오 주입`, `${terms.telemetry} simulator · inject scenario`)}</option>
                {scenarios.map((s) => (
                  <option key={s.id} value={s.id}>
                    {s.id} · {s.title}
                  </option>
                ))}
              </select>
              <div className="demo-actions">
                <button
                  className="plain demo-physics"
                  onClick={() => {
                    setPopover(null);
                    setDrawer("physics");
                  }}
                >
                  {say("⚡ 물리엔진 · 자율 대응", "⚡ Physics Engine · Autonomous Response")}
                </button>
                <button className={`plain demo-reset ${resetArmed ? "armed" : ""}`} disabled={!!busy} onClick={() => void resetSession()}>
                  {resetArmed ? say("한 번 더 누르면 초기화", "Press again to reset") : say("↺ 세션 초기화", "↺ Reset session")}
                </button>
              </div>
            </section>
          )}

          {/* 설정 팝오버: 화면 언어 · 도메인 · 교대감독 사번 · 자동화 모드 · 시연 도구 · 모델 관리 (모델 관리는 늘 붙어 있어야 시작 때 모델을 확인합니다) */}
          <section className="settings-popover ov-panel vp-glass" hidden={popover !== "settings"} aria-label={say("관제 설정", "Console settings")}>
            <div className="ov-head">
              <b>{say("⚙️ 관제 설정", "⚙️ Console Settings")}</b>
              <button className="ov-close" onClick={() => setPopover(null)} aria-label={say("닫기", "Close")}>
                ✕
              </button>
            </div>
            <div className="lang-row" role="group" aria-label="언어 · Language">
              <span>{say("화면 언어", "Display language")} · Language</span>
              <div className="domain-switch ov-pill">
                {(["ko", "en"] as Lang[]).map((l) => (
                  <button key={l} className={lang === l ? "seg active" : "seg"} data-lang={l} onClick={() => void chooseLang(l).catch(fail)}>
                    {l === "ko" ? "🇰🇷 한국어" : "🇺🇸 English"}
                  </button>
                ))}
              </div>
            </div>
            <div className="lang-row domain-row" role="group" aria-label={say("도메인 모드", "Domain mode")}>
              <span title={say("바꾸면 사건 · 감사 기록 · 자동화 모드를 비웁니다", "Switching clears incidents, the audit log and automation mode")}>{say("도메인 (바꾸면 사건을 비움)", "Domain (switching clears incidents)")}</span>
              <div className="domain-switch ov-pill">
                {(domain?.modes ?? []).map((m) => (
                  <button key={m.mode} className={m.mode === domain?.mode ? "seg active" : "seg"} disabled={!!busy} onClick={() => void switchDomain(m.mode)} title={say(`${m.label} 모드`, `${m.label} mode`)}>
                    {DOMAIN_ICON[m.mode]} {m.label}
                  </button>
                ))}
              </div>
            </div>
            <label className="inline">
              {say(`${terms.supervisor} 사번`, `${terms.supervisor} ID`)}
              <input value={supervisor} onChange={(e) => setSupervisor(e.target.value)} placeholder={say(`예: ${DEFAULT_SUPERVISOR[domain?.mode ?? "power_plant"]}`, `e.g. ${DEFAULT_SUPERVISOR[domain?.mode ?? "power_plant"]}`)} />
            </label>
            <div className="auto-row">
              {status?.automation ? (
                <button className="danger" onClick={() => toggleAuto(false)}>
                  {say("자동화 모드 끄기", "Turn off Automation Mode")}
                </button>
              ) : (
                <button className="plain" disabled={!supervisor.trim()} onClick={() => setOpening((o) => !o)}>
                  {say("자동화 모드 켜기", "Turn on Automation Mode")}
                </button>
              )}
              <em className={`tag ${status?.automation ? "auto" : ""}`}>{status?.automation ? say("자동화 모드", "Automation Mode") : say("보조 모드", "Assist Mode")}</em>
            </div>
            {opening && (
              <div className="auto-form">
                <label className="inline wide">
                  {say("근거 규정", "Legal basis")}
                  <input value={basis} placeholder={say(`자동 확정의 근거 (예: ${terms.site} 운영 지침 제12조)`, `Basis for auto-confirmation (e.g. ${terms.site} operating guideline Art. 12)`)} onChange={(e) => setBasis(e.target.value)} />
                </label>
                <span className="muted">
                  {say(`정상 유지 · 승인된 ${terms.change} 반영만 자동 확정됩니다. 계통 이상 · 고위험은 언제나 ${terms.supervisor}이 확정합니다.`, `Only normal-hold and approved ${terms.change} updates are auto-confirmed. System faults and high-risk actions are always confirmed by the ${terms.supervisor}.`)}
                </span>
                <button className="green" disabled={!basis.trim()} onClick={() => toggleAuto(true)}>
                  {say("켜기", "Turn on")}
                </button>
              </div>
            )}
            <div className="lang-row demo-row" role="group" aria-label={say("시연 · 시험 도구", "Demo · test tools")}>
              <span title={say("시나리오 주입 · 물리엔진 시뮬레이터 · 가드레일 시험 단추 · 세션 초기화", "Scenario injection · physics simulator · guardrail test buttons · session reset")}>{say("시연 · 시험 도구", "Demo · test tools")}</span>
              <div className="domain-switch ov-pill">
                <button className={demo ? "seg active" : "seg"} onClick={() => setDemoPref(true)}>
                  {say("보이기", "Show")}
                </button>
                <button className={demo ? "seg" : "seg active"} onClick={() => setDemoPref(false)}>
                  {say("숨기기", "Hide")}
                </button>
              </div>
            </div>
            <div className="lang-row engineer-row" role="group" aria-label={say("엔지니어 보기", "Engineer view")}>
              <span title={say("결재 · 판단 화면에 코드 이름(jev_head · verdict.rs · G4_… 등)을 함께 보입니다", "Shows code names (jev_head · verdict.rs · G4_… etc.) on the approval and verdict screens")}>{say("엔지니어 보기 (코드 이름)", "Engineer view (code names)")}</span>
              <div className="domain-switch ov-pill">
                <button className={engineer ? "seg active" : "seg"} data-engineer="on" onClick={() => setEngineer(true)}>
                  {say("켜기", "On")}
                </button>
                <button className={engineer ? "seg" : "seg active"} data-engineer="off" onClick={() => setEngineer(false)}>
                  {say("끄기", "Off")}
                </button>
              </div>
            </div>
            <ModelSettings open={popover === "settings"} onOpen={openSettings} />
          </section>

          <RightRail
            plant={plant}
            detail={detail}
            selected={selectedKks}
            onSelect={setSelectedKks}
            onPick={pick}
            t={t}
            catalog={catalog}
            supervisor={supervisor}
            demo={demo}
            onDecided={onDecided}
            onOpenDraft={() => setDrawer(5)}
            bottlenecks={bottlenecks}
            folded={hudFolded}
            onFold={setHudFolded}
            focus={hudFocus}
          />

          {/* 하단 가운데 허브: [Drop-up] · 알림 한 줄 · 파이프라인 · 타임머신 · 자연어 질의 */}
          <section className="hub ov-avoid" aria-label={say("관제 허브", "Control hub")}>
            {dropup === "search" && search && (
              <SearchDropup
                run={search}
                onClose={() => setDropup(null)}
                onSelect={setSelectedKks}
                onPick={pick}
                onInterlock={() => onInterlock?.()}
                interlockKks={interlockKks}
              />
            )}
            {dropup === "charts" && detail && plant && (
              <section className="telemetry-dropup dropup vp-glass ov-avoid" aria-label={say("시계열 · SDS 로직 차트", "Time-series · SDS logic charts")}>
                <div className="ov-head">
                  <b>{say("📈 CAM-02 시계열 · 🧠 CAM-03 SDS 로직 차트", "📈 CAM-02 Time Series · 🧠 CAM-03 SDS Logic Chart")}</b>
                  <span className="muted">{say("재생 시각 t 와 선택 설비에 맞물려 움직입니다", "Follows the replay time t and the selected equipment")}</span>
                  <button className="ov-close" onClick={() => setDropup(null)} aria-label={say("닫기", "Close")}>
                    ✕
                  </button>
                </div>
                <div className="dock-row">
                  <TimeSeriesDock detail={detail} plant={plant} t={at} />
                  <SdsDock detail={detail} plant={plant} t={at} onSelect={setSelectedKks} />
                </div>
              </section>
            )}
            {busy ? (
              <div className="cmd-toast notice busy" role="status">
                <span className="spin" aria-hidden="true" />
                <span className="toast-text">{busy}</span>
              </div>
            ) : notice ? (
              <div className={`cmd-toast notice ${notice.kind}`} role="status">
                <span className="toast-text" title={notice.text}>
                  {notice.text}
                </span>
                {notice.action && (
                  <button className="plain small toast-action" onClick={notice.action.run}>
                    {notice.action.label}
                  </button>
                )}
                <button className="ov-close" onClick={() => setNotice(null)} aria-label={say("알림 닫기", "Dismiss notice")}>
                  ✕
                </button>
              </div>
            ) : searchToast ? (
              <div className={`cmd-toast ${searchToast.kind}`} role="status">
                <span className="toast-text" title={search?.sent}>
                  {searchToast.text}
                </span>
                <button
                  className="ov-close"
                  onClick={() => {
                    setSearch(null);
                    setDropup((d) => (d === "search" ? null : d));
                  }}
                  aria-label={say("검색 표시 지우기", "Clear search highlights")}
                >
                  ✕
                </button>
              </div>
            ) : null}
            <div className="hub-card vp-glass" ref={hubCard}>
              <StageBar detail={detail} onOpen={(k) => setDrawer(k)} lanes={lanes} />
              {detail && plant && (
                <TimeScrubber
                  detail={detail}
                  t={at}
                  onT={setT}
                  charts={dropup === "charts"}
                  onCharts={() => setDropup((d) => (d === "charts" ? null : "charts"))}
                  traces={traces}
                  lead={
                    ttf !== null && (
                      <span className={`ttf-badge ${ttf <= 30 ? "hot" : ""}`} title={say("경보 센서가 지금 기울기로 보호 정지 한계에 닿기까지 남은 시간", "Time until the alarm sensor reaches its protective trip limit at the current slope")}>
                        {say(`🚨 ${terms.trip} 잔여(TTF) ${ttf.toFixed(0)}초`, `🚨 Time to ${terms.trip} (TTF) ${ttf.toFixed(0)} s`)}
                        {tripLimit !== null && alarmSensor ? say(` (${fmt(tripLimit)}${alarmSensor.unit} 한계)`, ` (limit ${fmt(tripLimit)}${alarmSensor.unit})`) : ""}
                      </span>
                    )
                  }
                />
              )}
              <CommandBar
                inputRef={inputRef}
                selected={selectedKks}
                context={context}
                onContext={setContext}
                run={search}
                open={dropup === "search"}
                onToggle={() => setDropup((d) => (d === "search" ? null : "search"))}
                onSubmit={runSearch}
              />
            </div>
          </section>

          {drawer === "physics" && plant && (
            <div className="drawer">
              <header>
                <strong>{say("⚡ [물리엔진 이벤트 시뮬레이터 & Zoro-1 자율 대응 (Part 8-5)] 명령은 시뮬레이터에만 들어갑니다", "⚡ [Physics Engine Event Simulator & Zoro-1 Autonomous Response (Part 8-5)] Commands go to the simulator only")}</strong>
                <span className="grow" />
                <button className="plain" onClick={() => setDrawer(null)}>
                  {say("[X 닫기]", "[X Close]")}
                </button>
              </header>
              <div className="drawer-body">
                <PhysicsPanel
                  plant={plant}
                  supervisor={supervisor}
                  automation={!!status?.automation}
                  onOpenIncident={(id) => {
                    setDrawer(null);
                    pick(id);
                  }}
                />
              </div>
            </div>
          )}
          {drawer !== null && drawer !== "physics" && <StageDrawer which={drawer} detail={detail} archive={archive} catalog={catalog} supervisor={supervisor} onClose={() => setDrawer(null)} />}
          {zoomKks && plant && <CctvZoom kks={zoomKks} plant={plant} detail={detail} t={at} onClose={() => setZoomKks(null)} onT={setT} />}
        </div>
        <footer className="status-bar">
          <span>{say("엔진", "Engine")}: {status?.engine ? (engineer ? status.engine : status.engine.replace(/\s*\(SharedResidency\)/, "")) : "-"}</span>
          <span>
            {say("도메인", "Domain")}: {domain ? `${DOMAIN_ICON[domain.mode]} ${domain.label}` : "-"} · {plant?.kind}
          </span>
          <span>{say(`설비 관계망 ${status?.graph_nodes ?? 0}노드`, `Equipment graph ${status?.graph_nodes ?? 0} nodes`)}</span>
          <span>
            {say(`${terms.change} 오버레이 ${status?.overlays.length ?? 0}건`, `${terms.change} overlays ${status?.overlays.length ?? 0}`)}
          </span>
          <span>{say(`감사 기록 ${status?.audit_count ?? 0}건`, `Audit log ${status?.audit_count ?? 0}`)}</span>
          <span className="sb-keys">{say("Enter · / 질의 · ESC 판 닫기", "Enter · / query · ESC close panels")}</span>
          {status?.audit_last[0] && <span className="audit-tail">{status.audit_last[0]}</span>}
        </footer>
      </main>
    </TermsContext.Provider>
  );
}
