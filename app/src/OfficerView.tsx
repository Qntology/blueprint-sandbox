import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import { AlarmQueue } from "./AlarmQueue";
import { CctvZoom } from "./cctv/CctvZoom";
import { CommandBar, contextual, SearchDropup, searchMarks, searchSummary, type SearchResult, type SearchRun } from "./cctv/CommandBar";
import { SdsDock, TimeSeriesDock } from "./cctv/Docks";
import { RightRail } from "./cctv/RightRail";
import { HUD_W, TOP_INSET, useElementSize, type Insets } from "./cctv/stage";
import { causalTraces } from "./cctv/traces";
import { DEFAULT_SUPERVISOR, DOMAIN_ICON, PLANT_TERMS, TermsContext, type DomainInfo, type DomainMode } from "./domain";
import { ModelSettings } from "./ModelSettings";
import { PhysicsPanel } from "./PhysicsPanel";
import { fmt, mmss } from "./plant";
import { PlantTopologyView, type Tab } from "./PlantTopologyView";
import { StageBar, StageDrawer } from "./StagePanel";
import { TimeScrubber } from "./TimeScrubber";
import type { ArchiveDoc, Bottleneck, ConsoleStatus, Detail, IncidentRow, Lane, Plant, ScenarioInfo, VerdictInfo } from "./types";

export type DrawerKey = 1 | 2 | 3 | 4 | 5 | "sds" | "archive" | "physics" | null;

const DARK_KEY = "blueprint.drawing-dark";

function readPref(key: string): string | null {
  try {
    return localStorage.getItem(key);
  } catch {
    return null;
  }
}

/** 관제 콘솔 (기획 Part 8-1 · 개선안 CCTV형 통합 관제판 · 2차 개선안 풀스크린 오버레이 · 추가 개선안 도메인 모드).
 *  고른 3-Way 도면 뷰 하나가 무대 전체에 깔리고(noto-sandbox 의 `inset: 0` 월드), 그 위에 반투명 유리 판이 뜹니다:
 *  좌상단 브랜드 · 도메인 · 사건 대기열 알약과 도면 도구줄, 우상단 뷰 전환 · 흐름 오버레이 · 물리엔진 · 설정 알약,
 *  오른쪽 병목 · 인터록 · 결재 HUD, 아래 가운데 파이프라인 · 타임머신(3줄 미니 파형) · 자연어 질의 허브.
 *  근본 원인 · 경보 · 선택 · 검색 1위 설비에는 CCTV 말풍선이 지시선으로 붙습니다. 선택 설비(`selectedKks`)와 재생 시각(`t`) 하나로 모두 맞물립니다. */
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
  const [supervisor, setSupervisor] = useState("OP-201");
  const [settingsOpen, setSettingsOpen] = useState(false);
  const [opening, setOpening] = useState(false);
  const [basis, setBasis] = useState("");
  const [busy, setBusy] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [message, setMessage] = useState<string | null>(null);
  // 2차 개선안: 떠 있는 판들의 여닫기
  const [tab, setTab] = useState<Tab>("drawing");
  const [queueOpen, setQueueOpen] = useState(true);
  const [hudFolded, setHudFolded] = useState(false);
  const [hudFocus, setHudFocus] = useState(0);
  const [chartsOpen, setChartsOpen] = useState(false);
  const [search, setSearch] = useState<SearchRun | null>(null);
  const [searchOpen, setSearchOpen] = useState(false);
  const [context, setContext] = useState(true);
  const [zoomKks, setZoomKks] = useState<string | null>(null);
  const [lanes, setLanes] = useState<Lane[]>([]);
  const [bottlenecks, setBottlenecks] = useState<Bottleneck[]>([]);
  const [dark, setDark] = useState(() => readPref(DARK_KEY) === "1");
  const [barSlot, setBarSlot] = useState<HTMLDivElement | null>(null);
  const stageRef = useRef<HTMLDivElement>(null);
  const hubCard = useRef<HTMLDivElement>(null);
  const inputRef = useRef<HTMLInputElement>(null);
  const stage = useElementSize(stageRef);
  const terms = plant?.terms ?? domain?.terms ?? PLANT_TERMS;

  const refresh = useCallback(() => {
    invoke<ConsoleStatus>("console_status").then(setStatus).catch((e) => setError(String(e)));
    invoke<IncidentRow[]>("incident_list").then(setRows).catch((e) => setError(String(e)));
    invoke<Plant>("plant_topology").then(setPlant).catch((e) => setError(String(e)));
  }, []);

  // 도메인마다 달라지는 것: 판정 이름(구역) · 시나리오 목록 · 용어
  const loadDomain = useCallback(() => {
    invoke<VerdictInfo[]>("verdict_catalog").then((list) => setCatalog(Object.fromEntries(list.map((v) => [v.code, v]))));
    invoke<ScenarioInfo[]>("scenario_list").then(setScenarios);
    invoke<DomainInfo>("domain_info").then((d) => {
      setDomain(d);
      setSupervisor((s) => (Object.values(DEFAULT_SUPERVISOR).includes(s) ? DEFAULT_SUPERVISOR[d.mode] : s));
    });
  }, []);

  // 확정 직후 보관 문서를 자동으로 연 사건 (한 번만 엽니다)
  const archiveShown = useRef<string | null>(null);

  const resetCase = useCallback(() => {
    archiveShown.current = null;
    setIncident(null);
    setDetail(null);
    setArchive(null);
    setSelectedKks(null);
    setT(null);
    setDrawer(null);
    setMessage(null);
    setSearch(null);
    setSearchOpen(false);
    setZoomKks(null);
  }, []);

  useEffect(() => {
    refresh();
    loadDomain();
    const stop = listen("incident_changed", () => refresh());
    const stopDomain = listen("domain_changed", () => {
      resetCase();
      refresh();
      loadDomain();
    });
    const timer = window.setInterval(() => invoke<IncidentRow[]>("incident_list").then(setRows).catch(() => {}), 5000);
    return () => {
      void stop.then((f) => f());
      void stopDomain.then((f) => f());
      window.clearInterval(timer);
    };
  }, [refresh, loadDomain, resetCase]);

  // 프로세스 마이닝 (레인 · 병목): 파이프라인 줄 · 사건 대기열 · 결재 HUD 가 같은 값을 씁니다.
  useEffect(() => {
    const load = () =>
      invoke<{ lanes: Lane[]; bottlenecks: Bottleneck[] }>("mining_overview")
        .then((m) => {
          setLanes(m.lanes);
          setBottlenecks(m.bottlenecks);
        })
        .catch(() => {});
    load();
    const timer = window.setInterval(load, 3000);
    return () => window.clearInterval(timer);
  }, [rows.length, status?.audit_count, detail?.id]);

  // 사건을 고르면 상세를 받고, 선택 설비를 근본 원인(없으면 경보 설비)으로 옮깁니다.
  const loadDetail = useCallback((id: string, keepSelection = false) => {
    setArchive(null);
    invoke<Detail>("incident_detail", { id })
      .then((d) => {
        setDetail(d);
        // 확정 뒤 자동으로 연 보관 문서 서랍은 진행 중인 사건을 고르면 닫습니다 (빈 서랍이 화면을 가리지 않게).
        setDrawer((open) => (open === "archive" ? null : open));
        if (!keepSelection) {
          setT(d.timeline.now_ms);
          setSelectedKks(d.analysis.substantive?.causal?.root?.kks ?? d.analysis.route.kks);
        }
      })
      .catch(() => {
        setDetail(null);
        invoke<ArchiveDoc>("archive_detail", { id }).then((a) => {
          setArchive(a);
          // 확정 직후 한 번만 보관 문서를 엽니다. 목록이 새로 올 때마다 다시 열면 닫아도 계속 화면을 가립니다.
          if (archiveShown.current !== id) {
            archiveShown.current = id;
            setDrawer("archive");
          }
        });
      });
  }, []);

  useEffect(() => {
    if (!incident) return;
    loadDetail(incident, true);
  }, [rows, incident, loadDetail]);

  // 새 설비를 고르면 질의 문맥을 다시 켭니다 (문맥 칩 ✕ 는 그 설비에서만 끕니다)
  useEffect(() => setContext(true), [selectedKks]);

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
    setMessage(null);
    setQueueOpen(false);
    loadDetail(id);
  }

  async function inject(id: string) {
    setBusy(`시나리오 ${id} 주입 중 (Zoro-1 판단 · 글쓰기)`);
    setError(null);
    try {
      const created = await invoke<string | null>("inject_scenario", { id });
      if (created) pick(created);
      else setMessage(`${id}: 알람 없이 계측만 바뀌었습니다. 현장 단말에서 Red-line 을 스캔해 보내세요 (스캐너 폴더에 ${scenarios.find((s) => s.id === id)?.scans.join(", ")}).`);
      refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function switchDomain(mode: DomainMode) {
    if (mode === domain?.mode) return;
    setBusy(`${mode === "aidc" ? "AI 데이터센터" : "화력발전소"} 모드로 바꾸는 중 (원장 · 시나리오 · 도면 색인)`);
    setError(null);
    try {
      const d = await invoke<DomainInfo>("switch_domain_mode", { mode });
      setDomain(d);
      setSupervisor(DEFAULT_SUPERVISOR[d.mode]);
      resetCase();
      refresh();
      loadDomain();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(null);
    }
  }

  async function toggleAuto(on: boolean) {
    setError(null);
    try {
      await invoke("set_automation", { input: { on, approver_id: supervisor, legal_basis: basis } });
      setOpening(false);
      setBasis("");
      refresh();
    } catch (e) {
      setError(String(e));
    }
  }

  // 자연어 질의: 선택 설비 문맥을 붙일지 정하고, 1위가 조건을 통과한 설비면 세 뷰를 그 설비로 옮깁니다 (2차 개선안 2-②).
  const runSearch = useCallback(
    async (text: string) => {
      if (!plant) return false;
      const kks = context ? selectedKks : null;
      const sent = kks ? contextual(text, kks, plant) : text;
      try {
        const result = await invoke<SearchResult>("search", { query: sent });
        setSearch({ typed: text, sent, context: sent !== text ? kks : null, result });
        setSearchOpen(true);
        setChartsOpen(false);
        const top = result.hits[0];
        if (top?.passed && top.doc.kks && top.doc.kks !== selectedKks) setSelectedKks(top.doc.kks);
        return true;
      } catch (e) {
        setError(String(e));
        return false;
      }
    },
    [plant, context, selectedKks],
  );

  // ESC 는 맨 위 판부터 하나씩 닫고(noto-sandbox overlays.js 의 STACK), 닫을 판이 없으면 검색 표시 → 설비 선택을 풉니다.
  // Enter · / 는 어디서든 질의 줄로 (다른 입력 칸 · 단추에 초점이 있을 때는 그대로).
  const keys = useRef({ zoomKks, drawer, settingsOpen, queueOpen, searchOpen, chartsOpen, search, selectedKks });
  keys.current = { zoomKks, drawer, settingsOpen, queueOpen, searchOpen, chartsOpen, search, selectedKks };
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.isComposing) return;
      const s = keys.current;
      if (e.key === "Escape") {
        // 화면에 겹친 순서대로: 확대 창(100) → 팝오버(25) → 허브 Drop-up(열리면 22, 서랍 위) → 서랍(20) → 검색 표시 → 설비 선택
        if (s.zoomKks) setZoomKks(null);
        else if (s.settingsOpen) setSettingsOpen(false);
        else if (s.queueOpen) setQueueOpen(false);
        else if (s.searchOpen) setSearchOpen(false);
        else if (s.chartsOpen) setChartsOpen(false);
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
  // 트립(셧다운) 잔여: 경보 센서의 TTF 와 보호 정지 한계
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
  const toast = search ? searchSummary(search) : null;
  const TABS: [Tab, string][] = [
    ["drawing", `① ${terms.drawing} 도면`],
    ["mermaid", "② Mermaid 계통"],
    ["mindmap", "③ 마인드맵"],
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
                try {
                  localStorage.setItem(DARK_KEY, on ? "1" : "0");
                } catch {
                  // 저장소가 막혀 있으면 이번 실행에만
                }
              }}
              onZoom={setZoomKks}
              onInterlock={onInterlock}
            />
          ) : (
            <div className="world-empty">계통을 읽는 중입니다</div>
          )}

          {/* 좌상단: 브랜드 · 도메인 · 사건 대기열, 뷰 도구줄(도면 선택 · 전체 계통 · A0), 공간 Depth */}
          <header className="corner-tl">
            <div className="corner-row ov-avoid">
              <span className="brand-mark ov-pill">
                ⚡ blueprint <span className="brand-title">{terms.console_title}</span>
              </span>
              <div className="domain-switch ov-pill" role="group" aria-label="도메인 모드">
                {(domain?.modes ?? []).map((m) => (
                  <button key={m.mode} className={m.mode === domain?.mode ? "seg active" : "seg"} disabled={!!busy} onClick={() => void switchDomain(m.mode)} title={`${m.label} 모드`}>
                    {DOMAIN_ICON[m.mode]} {m.label}
                  </button>
                ))}
              </div>
              <button className={`ov-pill queue-pill ${waiting ? "hot" : ""}`} aria-expanded={queueOpen} aria-controls="incident-popover" onClick={() => setQueueOpen((o) => !o)}>
                🚨 사건 대기열 · 대기 {waiting}건 {queueOpen ? "▴" : "▾"}
              </button>
            </div>
            <div className="corner-slot ov-avoid" ref={setBarSlot} />
            <nav className="breadcrumb ov-bar ov-avoid" aria-label="공간 Depth">
              <span className="crumb-title">공간 Depth</span>
              {(crumb.length ? crumb : [plant?.domain === "aidc" ? "Lv1 센터" : "Lv1 호기", "Lv2 섹터", "Lv3 층", `Lv4 ${terms.drawing} 도면`, `Lv5 ${terms.code}`]).map((c, i) => (
                <span key={i} className="crumb">
                  <em>Lv{i + 1}.</em> {c}
                  {i < 4 && <b>▸</b>}
                </span>
              ))}
            </nav>
          </header>

          {/* 우상단: 3-Way 뷰 · 흐름 오버레이 · 물리엔진 · 설정 */}
          <div className="corner-tr">
            <div className="corner-row ov-avoid">
              <div className="view-pill ov-pill" role="tablist" aria-label="3-Way 도면 뷰">
                {TABS.map(([key, label]) => (
                  <button key={key} role="tab" aria-selected={tab === key} className={tab === key ? "tab active" : "tab"} onClick={() => setTab(key)}>
                    {label}
                  </button>
                ))}
              </div>
              <label className={`mining-toggle ov-pill ${mining ? "on" : ""}`} title="배관 흐름 · 병목 · 근본 원인 물결을 도면 · 마인드맵 위에 겹칩니다">
                <input type="checkbox" checked={mining} onChange={(e) => setMining(e.target.checked)} />⚡ 흐름 · 병목 {mining ? "ON" : "OFF"}
              </label>
              <button className="ov-pill physics-open" onClick={() => setDrawer("physics")}>
                ⚡ 물리엔진 · 자율 대응
              </button>
              <button className={`ov-pill settings-pill ${status?.automation ? "auto" : ""}`} aria-expanded={settingsOpen} onClick={() => setSettingsOpen((o) => !o)}>
                ⚙️ {supervisor} · {status?.automation ? "자동화 모드" : "보조 모드"} ▾
              </button>
            </div>
          </div>

          {/* 사건 대기열 팝오버: 시뮬레이터 · 마이닝 레인 · 확인 대기 · 처리 완료 */}
          <section className="incident-popover ov-panel vp-glass ov-avoid" id="incident-popover" hidden={!queueOpen} aria-label="사건 대기열">
            <div className="ov-head">
              <b>🚨 사건 대기열</b>
              <span className="muted">
                대기 {waiting} · 처리 완료 {rows.length - waiting}
              </span>
              <button className="ov-close" onClick={() => setQueueOpen(false)} aria-label="닫기">
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
              <option value="">{terms.telemetry} 시뮬레이터 · 시나리오 주입</option>
              {scenarios.map((s) => (
                <option key={s.id} value={s.id}>
                  {s.id} · {s.title}
                </option>
              ))}
            </select>
            {lanes.length > 0 && (
              <div className="lane-line" title="프로세스 마이닝 레인 (Stage 1 ~ 5 대기)">
                {lanes.map((l) => (
                  <span key={l.key} className={`lane-chip ${l.count > 0 && l.max_wait_secs > 0 && l.key !== "done" ? "hold" : ""}`}>
                    {l.label} <b>{l.count}</b>
                    {l.count > 0 && l.max_wait_secs > 0 && l.key !== "done" ? ` ⏳ ${mmss(l.max_wait_secs)}` : ""}
                  </span>
                ))}
              </div>
            )}
            <AlarmQueue rows={rows} selected={incident} onPick={pick} catalog={catalog} />
          </section>

          {/* 설정 팝오버: 교대감독 사번 · 자동화 모드 · 모델 관리 (모델 관리는 늘 붙어 있어야 시작 때 모델을 확인합니다) */}
          <section className="settings-popover ov-panel vp-glass" hidden={!settingsOpen} aria-label="관제 설정">
            <div className="ov-head">
              <b>⚙️ 관제 설정</b>
              <button className="ov-close" onClick={() => setSettingsOpen(false)} aria-label="닫기">
                ✕
              </button>
            </div>
            <label className="inline">
              {terms.supervisor} 사번
              <input value={supervisor} onChange={(e) => setSupervisor(e.target.value)} placeholder={`예: ${DEFAULT_SUPERVISOR[domain?.mode ?? "power_plant"]}`} />
            </label>
            <div className="auto-row">
              {status?.automation ? (
                <button className="danger" onClick={() => toggleAuto(false)}>
                  자동화 모드 끄기
                </button>
              ) : (
                <button className="plain" disabled={!supervisor.trim()} onClick={() => setOpening((o) => !o)}>
                  자동화 모드 켜기
                </button>
              )}
              <em className={`tag ${status?.automation ? "auto" : ""}`}>{status?.automation ? "자동화 모드" : "보조 모드"}</em>
            </div>
            {opening && (
              <div className="auto-form">
                <label className="inline wide">
                  근거 규정
                  <input value={basis} placeholder={`자동 확정의 근거 (예: ${terms.site} 운영 지침 제12조)`} onChange={(e) => setBasis(e.target.value)} />
                </label>
                <span className="muted">
                  정상 유지 · 승인된 {terms.change} 반영만 자동 확정됩니다. 계통 이상 · 고위험은 언제나 {terms.supervisor}이 확정합니다.
                </span>
                <button className="green" disabled={!basis.trim()} onClick={() => toggleAuto(true)}>
                  켜기
                </button>
              </div>
            )}
            <ModelSettings open={settingsOpen} onOpen={() => setSettingsOpen(true)} />
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
            onDecided={(msg) => {
              setMessage(msg);
              refresh();
            }}
            onOpenApproval={() => setDrawer(5)}
            bottlenecks={bottlenecks}
            folded={hudFolded}
            onFold={setHudFolded}
            focus={hudFocus}
            ttf={ttf !== null ? `⏱ ${terms.trip} ${ttf.toFixed(0)}초` : null}
          />

          {/* 하단 가운데 허브: [Drop-up] · 알림 · 파이프라인 · 타임머신 · 자연어 질의 */}
          <section className="hub ov-avoid" aria-label="관제 허브">
            {searchOpen && search && (
              <SearchDropup
                run={search}
                onClose={() => setSearchOpen(false)}
                onSelect={setSelectedKks}
                onPick={pick}
                onInterlock={() => onInterlock?.()}
                interlockKks={interlockKks}
              />
            )}
            {chartsOpen && detail && plant && (
              <section className="telemetry-dropup dropup vp-glass ov-avoid" aria-label="시계열 · SDS 로직 차트">
                <div className="ov-head">
                  <b>📈 CAM-02 시계열 · 🧠 CAM-03 SDS 로직 차트</b>
                  <span className="muted">재생 시각 t 와 선택 설비에 맞물려 움직입니다</span>
                  <button className="ov-close" onClick={() => setChartsOpen(false)} aria-label="닫기">
                    ✕
                  </button>
                </div>
                <div className="dock-row">
                  <TimeSeriesDock detail={detail} plant={plant} t={at} />
                  <SdsDock detail={detail} plant={plant} t={at} onSelect={setSelectedKks} />
                </div>
              </section>
            )}
            {(busy || error || message) && (
              <div className={`cmd-toast notice ${error ? "err" : busy ? "busy" : "ok"}`} role="status">
                {busy && <span className="spin" aria-hidden="true" />}
                <span className="toast-text">{busy ?? error ?? message}</span>
                {!busy && (
                  <button
                    className="ov-close"
                    onClick={() => {
                      setError(null);
                      setMessage(null);
                    }}
                    aria-label="알림 닫기"
                  >
                    ✕
                  </button>
                )}
              </div>
            )}
            {toast && (
              <div className={`cmd-toast ${toast.kind}`} role="status">
                <span className="toast-text" title={search?.sent}>
                  {toast.text}
                </span>
                <button
                  className="ov-close"
                  onClick={() => {
                    setSearch(null);
                    setSearchOpen(false);
                  }}
                  aria-label="검색 표시 지우기"
                >
                  ✕
                </button>
              </div>
            )}
            <div className="hub-card vp-glass" ref={hubCard}>
              <StageBar
                detail={detail}
                onOpen={(k) => setDrawer(k)}
                lanes={lanes}
                tail={
                  <button className={`plain charts-toggle ${chartsOpen ? "active" : ""}`} disabled={!detail} aria-expanded={chartsOpen} onClick={() => setChartsOpen((o) => !o)} title="CAM-02 시계열 · CAM-03 SDS 로직 차트를 허브 위로 펼칩니다">
                    📈 CAM-02·03 {chartsOpen ? "▼" : "▲"}
                  </button>
                }
              />
              {detail && plant && (
                <TimeScrubber
                  detail={detail}
                  t={at}
                  onT={setT}
                  onSds={() => setDrawer("sds")}
                  traces={traces}
                  lead={
                    ttf !== null && (
                      <span className={`ttf-badge ${ttf <= 30 ? "hot" : ""}`} title="경보 센서가 지금 기울기로 보호 정지 한계에 닿기까지 남은 시간">
                        🚨 {terms.trip} 잔여(TTF) {ttf.toFixed(0)}초{tripLimit !== null && alarmSensor ? ` (${fmt(tripLimit)}${alarmSensor.unit} 한계)` : ""}
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
                open={searchOpen}
                onToggle={() => {
                  setSearchOpen((o) => !o);
                  setChartsOpen(false);
                }}
                onSubmit={runSearch}
              />
            </div>
          </section>

          {drawer === "physics" && plant && (
            <div className="drawer">
              <header>
                <strong>⚡ [물리엔진 이벤트 시뮬레이터 & Zoro-1 자율 대응 (Part 8-5)] 명령은 시뮬레이터에만 들어갑니다</strong>
                <span className="grow" />
                <button className="plain" onClick={() => setDrawer(null)}>
                  [X 닫기]
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
          {drawer !== null && drawer !== "physics" && (
            <StageDrawer
              which={drawer}
              detail={detail}
              archive={archive}
              plant={plant}
              catalog={catalog}
              supervisor={supervisor}
              onClose={() => setDrawer(null)}
              onDecided={(msg) => {
                setMessage(msg);
                refresh();
              }}
            />
          )}
          {zoomKks && plant && <CctvZoom kks={zoomKks} plant={plant} detail={detail} t={at} onClose={() => setZoomKks(null)} onT={setT} />}
        </div>
        <footer className="status-bar">
          <span>엔진: {status?.engine ?? "-"}</span>
          <span>
            도메인: {domain ? `${DOMAIN_ICON[domain.mode]} ${domain.label}` : "-"} · {plant?.kind}
          </span>
          <span>확인 대기 {status?.pending ?? 0}건</span>
          <span>보관 {status?.archived ?? 0}건</span>
          <span>설비 관계망 {status?.graph_nodes ?? 0}노드</span>
          <span>
            {terms.change} 오버레이 {status?.overlays.length ?? 0}건
          </span>
          <span>감사 기록 {status?.audit_count ?? 0}건</span>
          <span className="sb-keys">Enter · / 질의 · ESC 판 닫기</span>
          {status?.audit_last[0] && <span className="audit-tail">{status.audit_last[0]}</span>}
        </footer>
      </main>
    </TermsContext.Provider>
  );
}
