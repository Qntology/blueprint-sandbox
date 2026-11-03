// 백엔드(app/src-tauri/src/lib.rs, core 의 Serialize) 와 같은 모양입니다.

export type Dir = "Up" | "Down";

export interface Sector {
  key: string;
  code: string;
  building: string;
  label: string;
  kks: string;
  drawing: string;
  blurb: string;
}

export interface Floor {
  code: string;
  label: string;
  elevation_m: number;
}

export interface Drawing {
  id: string;
  sector: string;
  title: string;
  rev: string;
  issued: string;
  signer: string;
}

export interface Equipment {
  kks: string;
  name: string;
  sector: string;
  floor: string;
  drawing: string;
  kind: string;
  bbox: [number, number, number, number];
  breadcrumb: string[];
  place: string;
}

export interface Sensor {
  tag: string;
  kks: string;
  label: string;
  unit: string;
  normal: number;
  alarm_low: number | null;
  alarm_high: number | null;
  trip_low: number | null;
  trip_high: number | null;
  overlay: string | null;
}

export interface Edge {
  from: string;
  to: string;
  medium: string;
  label: string;
  lag: [number, number];
  flow_tag?: string | null;
}

export interface Connector {
  id: string;
  from: string;
  to: string;
  from_drawing: string;
  to_drawing: string;
  out_bbox: [number, number, number, number];
  in_bbox: [number, number, number, number];
  label: string;
}

export interface Aop {
  id: string;
  title: string;
  steps: string[];
  high_risk: boolean;
}

export interface Plant {
  name: string;
  kind: string;
  sectors: Sector[];
  floors: Floor[];
  drawings: Drawing[];
  equipment: Equipment[];
  sensors: Sensor[];
  edges: Edge[];
  connectors: Connector[];
  aops: Aop[];
  /** 도면별 배관 꺾은선 (도면 생성기가 그린 그대로, 정규화 좌표). `via` 는 도면 간 점프 커넥터 */
  pipes: Record<string, Pipe[]>;
  /** A0 종합 도면 (Part 8-4): 마스터 도면 배치와 격자 타일 */
  a0: A0Info[];
}

export interface A0Tile {
  id: string;
  row: number;
  col: number;
  x0: number;
  y0: number;
  x1: number;
  y1: number;
}

export interface A0Info {
  id: string;
  title: string;
  rev: string;
  pixel_size: [number, number];
  sheet_px: [number, number];
  grid: { rows: number; cols: number; overlap: number };
  sheets: { drawing: string; x: number; y: number }[];
  tiles: A0Tile[];
  equipment: Record<string, { bbox: [number, number, number, number]; tile: string | null }>;
}

export interface Pipe {
  from: string;
  to: string;
  points: [number, number][];
  via?: string;
}

export interface VerdictInfo {
  code: string;
  label: string;
  class: "favorable" | "adverse" | "procedural";
}

export interface ScenarioInfo {
  id: string;
  title: string;
  story: string;
  has_alarm: boolean;
  scans: string[];
}

export interface IncidentRow {
  id: string;
  title: string;
  building: string;
  kks: string;
  state: "awaiting_field" | "review" | "done";
  verdict: string | null;
  verdict_label: string | null;
  verdict_class: string | null;
  reason: string;
  high_risk: boolean;
  opened_at: string | null;
  waiting_secs: number | null;
  archive_id: string | null;
  final_code: string | null;
  auto: boolean;
  closed_at: string | null;
  scenario: string | null;
}

export interface StageRecord {
  stage: number;
  name: string;
  actor: string;
  ms: number;
  status: "Done" | "Skipped" | "Corrected" | "Alert" | "Waiting" | "Blocked";
  summary: string;
}

export interface Address {
  unit_code: string;
  unit_label: string;
  sector: string;
  building: string;
  sector_label: string;
  floor: string;
  floor_label: string;
  elevation_m: number;
  drawing: string;
  drawing_rev: string;
  kks: string;
  name: string;
  bbox: [number, number, number, number];
}

export interface TruthRow {
  item: string;
  field: string | null;
  ledger: string | null;
  single: string | null;
  agree: boolean;
}

export interface Asked {
  title: string;
  facts: [string, string][];
  options: string[];
  probabilities: number[];
}

export interface RootCause {
  kks: string;
  tag: string;
  hops: number;
  path: string[];
  connectors: string[];
  lag_secs: number;
  lag_range: [number, number];
  direction: Dir;
  physics: boolean | null;
  at_ms: number;
}

export interface Propagation {
  upstream_kks: string;
  downstream_kks: string;
  upstream_tag: string;
  downstream_tag: string;
  observed_lag_secs: number;
  expected_lag_range: [number, number];
  causal_match: boolean;
  backflow: boolean;
  hops: number;
  path: string[];
  connectors: string[];
  up_direction: Dir;
  down_direction: Dir;
  physics: boolean | null;
}

export interface ChainEvent {
  at_ms: number;
  tag: string;
  kks: string;
  label: string;
  direction: Dir;
  kind: "departure" | "limit";
}

export interface AopDraft {
  aop_id: string;
  title: string;
  summary: string;
  steps: string[];
  by_model: boolean;
  verified: boolean;
  problems: string[];
  numbers: [string, boolean][];
  high_risk: boolean;
  risk_actions: string[];
  facts: string[];
  ms: number;
  attempts: number;
}

export interface Metrics {
  sensor_tag: string;
  inflection_at_ms: number | null;
  window_milli: number[];
  slope_per_sec_milli: number;
  accel_milli: number;
  ewma_milli: number;
  estimated_ttf_secs: number | null;
}

export interface VoteResult {
  tag: string;
  logic: string;
  at_ms: number;
  channels: [string, number][];
  voted: number;
  tolerance: number;
  outliers: string[];
  state_code: string;
}

export interface FieldCheck {
  tag: string;
  taken_at_ms: number;
  field_value: number;
  dcs_value: number | null;
  channels: [string, number][];
  tolerance: number;
  matches_dcs: boolean;
  matching_channels: string[];
}

export interface Analysis {
  route: { sector: string; verdict: string; tag: string | null; kks: string; address: Address };
  trigger: { kind: string; tag: string | null; at_ms: number; value: number | null; slope: number | null; ttf_secs: number | null };
  signal: { tag: string; state: unknown; channels: [string, unknown][] } | null;
  vote: VoteResult | null;
  field_checks: FieldCheck[];
  drift: unknown;
  moc: { moc_no: string | null; outcome: unknown; stale_drawing: string | null; rows: TruthRow[] } | null;
  truth: TruthRow[];
  substantive: {
    decided: { judgement: { verdict: string; confidence: number | null }; axes: [string, number][]; asked: Asked[] };
    causal: { alarm_kks: string; alarm_tag: string; alarm_departure_ms: number | null; alarm_direction: Dir | null; propagation: Propagation[]; root: RootCause | null } | null;
    facts: [string, string][];
    ttf_secs: number | null;
  } | null;
  chain: ChainEvent[];
  draft: AopDraft | null;
  interlock: Interlock | null;
  metrics: Metrics[];
  stages: StageRecord[];
  sds: { sds_id: string; trigger_kind: string; gate_state_vector: string[]; decomposed_confidences: [string, number][]; min_confidence: number | null; center_ms: number };
  model_trace: { t: number; model: string; step: string; ms: number; detail: Record<string, unknown> }[];
  candidate: string;
  next: string;
  reason: string;
  notes: string[];
}

/** 절차서에 딸린 선행 절차 인터록 (Part 8-4). 단계가 모두 passed 여야 확정할 수 있습니다. */
export interface Interlock {
  aop_id: string;
  target: string;
  fault: string;
  steps: InterlockStep[];
}

export type InterlockCheck = { kind: "below"; tag: string; value: number } | { kind: "at_least"; tag: string; value: number } | { kind: "loto" };

export interface InterlockStep {
  no: number;
  req_depth: number;
  kks: string;
  text: string;
  check: InterlockCheck;
  status: "passed" | "pending";
  observed: number | null;
  unit: string | null;
  source: "dcs" | "field" | "supervisor" | null;
  evidence: string | null;
}

export interface Report {
  kind: string;
  kks: string | null;
  tag: string | null;
  reading: number | null;
  unit: string | null;
  taken_at: string;
  moc_no: string | null;
  old_value: number | null;
  drawing: string | null;
  drawing_rev: string | null;
  worker: string;
  worker_id: string;
  signed: boolean;
  permit_confirmed: boolean;
  excuse: string | null;
  corrections: { field: string; raw: string; fixed: string; distance: number }[];
  uncertain: string[];
}

export interface Timeline {
  t0_ms: number;
  period_ms: number;
  now_ms: number;
  series: Record<string, number[]>;
  channels: Record<string, Record<string, number[]>>;
}

export interface EdgeFlow {
  from: string;
  to: string;
  medium: string;
  label: string;
  flow_tag: string | null;
  nominal: number | null;
  current: number | null;
  unit: string | null;
  ratio: number;
  bottleneck: boolean;
  lag_secs: number | null;
  on_path: boolean;
}

export interface Detail {
  id: string;
  state: "awaiting_field" | "review";
  reason: string;
  notes: string[];
  field_request: string | null;
  judgement: { verdict: string; confidence: number | null } | null;
  alarm: { tag: string; channel: string | null; at: string; value: number; kind: string; message: string } | null;
  received_at: string;
  opened_at: string;
  reports: Report[];
  analysis: Analysis;
  timeline: Timeline;
  flows: EdgeFlow[];
  issues: IssueReport;
  scenario: string | null;
  sim_time: string;
}

export interface Bottleneck {
  kind: "physical" | "decision";
  title: string;
  incident: string;
  kks: string | null;
  lines: string[];
  waiting_secs: number | null;
}

export interface Lane {
  key: string;
  label: string;
  count: number;
  max_wait_secs: number;
  mean_ms: number;
  incidents: string[];
}

export interface ConsoleStatus {
  automation: boolean;
  engine: string;
  pending: number;
  archived: number;
  graph_nodes: number;
  audit_count: number;
  audit_last: string[];
  overlays: { moc_no: string; tag: string; param: string; old: number; new: number }[];
  live_scenario: string | null;
}

export interface ArchiveDoc {
  archive_id: string;
  incident_id: string;
  title: string;
  final_code: string;
  mcr_doc: string;
  field_notice: string;
  special_audit_flag: boolean;
  seal: string;
  auto: boolean;
  closed_at: string;
  retention_until: string;
}

/** 단일 · 절차 · 복합 이슈 분해 (Part 8-6) */
export interface IssueReport {
  kind: "single" | "procedural" | "compound";
  codes: string[];
  label: string;
  subs: SubIssue[];
  open: number;
  resolved: number;
}

export interface SubIssue {
  n: number;
  kind: "equipment" | "cascade" | "sensor" | "procedure" | "moc";
  kks: string | null;
  depth: number | null;
  title: string;
  detail: string;
  resolution: string;
  resolved: boolean;
}
