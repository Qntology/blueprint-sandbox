import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AlarmQueue } from "./AlarmQueue";
import { CamPanel } from "./cctv/CamPanel";
import { originOf, SdsDock, TimeSeriesDock } from "./cctv/Docks";
import { RightRail } from "./cctv/RightRail";
import { DEFAULT_SUPERVISOR, DOMAIN_ICON, PLANT_TERMS, TermsContext, type DomainInfo, type DomainMode } from "./domain";
import { ModelSettings } from "./ModelSettings";
import { PhysicsPanel } from "./PhysicsPanel";
import { fmt, signedSecs } from "./plant";
import { PlantTopologyView } from "./PlantTopologyView";
import { SearchBox } from "./SearchBox";
import { StageBar, StageDrawer } from "./StagePanel";
import { TimeScrubber } from "./TimeScrubber";
import type { ArchiveDoc, ConsoleStatus, Detail, IncidentRow, Plant, ScenarioInfo, VerdictInfo } from "./types";

export type DrawerKey = 1 | 2 | 3 | 4 | 5 | "sds" | "archive" | "physics" | null;

/** 관제 콘솔 (기획 Part 8-1 · 개선안 Part 1 CCTV형 통합 관제판 · 추가 개선안 도메인 모드).
 *  가운데 위(CAM-01)는 3-Way 도면, 가운데 아래(CAM-02 · 03)는 시계열 · SDS 로직 차트가 늘 떠 있고, 오른쪽은 병목 · 인터록 · 결재가 한 줄입니다.
 *  어느 칸에서 설비를 누르든 선택 설비(`selectedKks`)와 재생 시각(`t`) 하나로 모든 칸이 맞물립니다. */
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

  function pick(id: string) {
    setIncident(id);
    setMessage(null);
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

  const selected = useMemo(() => plant?.equipment.find((e) => e.kks === selectedKks) ?? null, [plant, selectedKks]);
  const crumb = selected?.breadcrumb ?? [];
  const at = t ?? detail?.timeline.now_ms ?? 0;
  const origin = detail ? originOf(detail) : 0;
  // 머리의 트립(셧다운) 잔여: 경보 센서의 TTF 와 보호 정지 한계
  const ttf = detail ? (detail.analysis.trigger.ttf_secs ?? detail.analysis.substantive?.ttf_secs ?? null) : null;
  const alarmSensor = detail?.analysis.route.tag ? plant?.sensors.find((s) => s.tag === detail.analysis.route.tag) : undefined;
  const tripLimit = alarmSensor ? (alarmSensor.trip_high ?? alarmSensor.trip_low) : null;

  return (
    <TermsContext.Provider value={terms}>
      <main className="console">
        <header className="console-header">
          <strong className="brand">⚡ blueprint {terms.console_title}</strong>
          <div className="domain-switch" role="group" aria-label="도메인 모드">
            {(domain?.modes ?? []).map((m) => (
              <button key={m.mode} className={m.mode === domain?.mode ? "seg active" : "seg"} disabled={!!busy} onClick={() => void switchDomain(m.mode)} title={`${m.label} 모드`}>
                {DOMAIN_ICON[m.mode]} {m.label}
              </button>
            ))}
          </div>
          <SearchBox onSelect={(kks) => setSelectedKks(kks)} onPick={(id) => pick(id)} />
          <span className="grow" />
          {ttf !== null && (
            <span className={`ttf-badge ${ttf <= 30 ? "hot" : ""}`} title="경보 센서가 지금 기울기로 보호 정지 한계에 닿기까지 남은 시간">
              🚨 {terms.trip} 잔여(TTF) {ttf.toFixed(0)}초{tripLimit !== null && alarmSensor ? ` (${fmt(tripLimit)}${alarmSensor.unit} 한계)` : ""}
            </span>
          )}
          <label className="inline">
            {terms.supervisor} 사번
            <input value={supervisor} onChange={(e) => setSupervisor(e.target.value)} placeholder={`예: ${DEFAULT_SUPERVISOR[domain?.mode ?? "power_plant"]}`} />
          </label>
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
          <button className="plain physics-open" onClick={() => setDrawer("physics")}>
            ⚡ 물리엔진 · 자율 대응
          </button>
          <button className="plain" onClick={() => setSettingsOpen((o) => !o)}>
            {settingsOpen ? "모델 설정 닫기" : "모델 설정"}
          </button>
        </header>
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
        <nav className="breadcrumb">
          <span className="crumb-title">공간 Depth</span>
          {(crumb.length ? crumb : [plant?.domain === "aidc" ? "Lv1 센터" : "Lv1 호기", "Lv2 섹터", "Lv3 층", `Lv4 ${terms.drawing} 도면`, `Lv5 ${terms.code}`]).map((c, i) => (
            <span key={i} className="crumb">
              <em>Lv{i + 1}.</em> {c}
              {i < 4 && <b>▸</b>}
            </span>
          ))}
          {busy && <span className="busy">{busy}</span>}
          {error && <span className="error">{error}</span>}
          {message && <span className="note">{message}</span>}
        </nav>
        <div className="console-body">
          <AlarmQueue rows={rows} selected={incident} onPick={pick} catalog={catalog} />
          <section className="center-grid">
            <CamPanel
              cam="CAM-01"
              className="cam-top"
              live={!!detail}
              title={`3-Way 프로세스 마이닝 도면 모니터 (${terms.drawing} · Mermaid · 마인드맵, 클릭 동기화)`}
              info={detail ? `${detail.sim_time} · t ${signedSecs(at, origin)} · ${detail.scenario ?? ""}` : plant?.name}
            >
              {plant ? (
                <PlantTopologyView plant={plant} detail={detail} selected={selectedKks} onSelect={setSelectedKks} mining={mining} onMining={setMining} t={at} incident={incident} />
              ) : (
                <div className="empty">계통을 읽는 중입니다</div>
              )}
            </CamPanel>
            <div className="dock-row">
              {detail && plant ? (
                <>
                  <TimeSeriesDock detail={detail} plant={plant} t={at} />
                  <SdsDock detail={detail} plant={plant} t={at} onSelect={setSelectedKks} />
                </>
              ) : (
                <>
                  <CamPanel cam="CAM-02" title="📈 시계열 로직 차트" live={false}>
                    <p className="muted">왼쪽 대기열에서 사건을 고르면 상류 → 원인 → 하류 파형과 Δt · 기울기 · TTF 가 여기 고정됩니다.</p>
                  </CamPanel>
                  <CamPanel cam="CAM-03" title="🧠 SDS 로직 차트" live={false}>
                    <p className="muted">사건을 고르면 SDS 상태 전이 · 3축 직교 분할 검증 · 게이트 벡터가 여기 고정됩니다.</p>
                  </CamPanel>
                </>
              )}
            </div>
          </section>
          <aside className="right">
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
            />
            <ModelSettings open={settingsOpen} onOpen={() => setSettingsOpen(true)} />
          </aside>
        </div>
        <StageBar detail={detail} onOpen={(k) => setDrawer(k)} refreshKey={rows.length + (status?.audit_count ?? 0)} />
        {detail && <TimeScrubber detail={detail} t={at} onT={setT} onSds={() => setDrawer("sds")} />}
        <footer className="tables">
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
          {status?.audit_last[0] && <span className="audit-tail">{status.audit_last[0]}</span>}
        </footer>
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
      </main>
    </TermsContext.Provider>
  );
}
