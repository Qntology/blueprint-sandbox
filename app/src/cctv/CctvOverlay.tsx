import { useEffect, useRef } from "react";
import { shortTag } from "../domain";
import { SHEET_PX, useDrawingImage } from "../drawings";
import { fixed, fmt, SDS_COLOR, sdsStateAt, sensorsOf, valueAt } from "../plant";
import type { Detail, Equipment, Metrics, Plant, SdsTrack, Sensor, Timeline } from "../types";
import { relRect, type Anchor, type Locator, type Rect } from "./stage";

/** 말풍선을 띄우는 까닭: 근본 원인 · 경보 설비 · 선택 설비 · 검색 1위 */
export type CardRole = "root" | "alarm" | "selected" | "hit";

export interface CardTarget {
  kks: string;
  roles: CardRole[];
}

/** 검색 적중 표시 (2차 개선안 2-②: 도면 위 Top-K 핀과 "왜" 리본) */
export interface SearchMark {
  rank: number;
  why: string[];
  kind: string;
  id: string;
}

const ROLE_TAG: Record<CardRole, string> = { root: "🔥 근본원인", alarm: "⚠️ 경보", selected: "◎ 선택", hit: "🎯 검색" };
const TAIL = 18; // 설비 상자와 말풍선 사이
const GAP = 6;
const EDGE = 6;
const RINGS = 4;
const ANGLES = 12;

const clamp = (v: number, lo: number, hi: number) => Math.max(lo, Math.min(hi, v));
const hits = (a: Rect, b: Rect) => a.x < b.x + b.w + GAP && b.x < a.x + a.w + GAP && a.y < b.y + b.h + GAP && b.y < a.y + a.h + GAP;

/** 설비 상자 둘레의 자리 후보: 위 · 오른쪽 · 왼쪽 · 아래 · 네 모서리, 그다음 바깥 고리 (noto-sandbox cctv.js 와 같은 순서 매김) */
function spots(a: Rect, w: number, h: number): [number, number][] {
  const cx = a.x + a.w / 2;
  const cy = a.y + a.h / 2;
  const out: [number, number][] = [
    [cx - w / 2, a.y - h - TAIL],
    [a.x + a.w + TAIL, cy - h / 2],
    [a.x - w - TAIL, cy - h / 2],
    [cx - w / 2, a.y + a.h + TAIL],
    [a.x + a.w + TAIL, a.y - h - TAIL],
    [a.x - w - TAIL, a.y - h - TAIL],
    [a.x + a.w + TAIL, a.y + a.h + TAIL],
    [a.x - w - TAIL, a.y + a.h + TAIL],
  ];
  for (let ring = 1; ring <= RINGS; ring++) {
    const rx = a.w / 2 + w / 2 + TAIL + ring * (w * 0.5 + GAP);
    const ry = a.h / 2 + h / 2 + TAIL + ring * (h * 0.5 + GAP);
    for (let k = 0; k < ANGLES; k++) {
      const ang = -Math.PI / 2 + (k * 2 * Math.PI) / ANGLES;
      out.push([cx + rx * Math.cos(ang) - w / 2, cy + ry * Math.sin(ang) - h / 2]);
    }
  }
  return out;
}

/** 60초 파형 타일 (CAM-WAVE): 재생 시각 t 앞 50초 ~ 뒤 10초, 커서 · 경보 · 트립 한계선 */
function WaveTile({ tl, sensor, t, color }: { tl: Timeline | null; sensor: Sensor | undefined; t: number; color: string }) {
  const v = sensor && tl ? tl.series[sensor.tag] : undefined;
  if (!tl || !sensor || !v?.length) return <div className="tile-empty">{sensor ? `계측 대기 · 정상 ${fmt(sensor.normal)} ${sensor.unit}` : "계측 없음"}</div>;
  const end = tl.t0_ms + (v.length - 1) * tl.period_ms;
  let to = Math.min(end, t + 10_000);
  let from = to - 60_000;
  if (from < tl.t0_ms) {
    from = tl.t0_ms;
    to = Math.min(end, from + 60_000);
  }
  const pts: [number, number][] = [];
  v.forEach((x, i) => {
    const at = tl.t0_ms + i * tl.period_ms;
    if (at >= from && at <= to) pts.push([at, x]);
  });
  if (pts.length < 2) return <div className="tile-empty">계측 대기</div>;
  let lo = Math.min(...pts.map((p) => p[1]));
  let hi = Math.max(...pts.map((p) => p[1]));
  const limits = [sensor.alarm_high, sensor.alarm_low, sensor.trip_high, sensor.trip_low].filter((l): l is number => l !== null);
  for (const l of limits) {
    if (l > lo - (hi - lo) * 0.6 && l < hi + (hi - lo) * 0.6) {
      lo = Math.min(lo, l);
      hi = Math.max(hi, l);
    }
  }
  const span = Math.max(hi - lo, 1e-9);
  const X = (at: number) => ((at - from) / Math.max(to - from, 1)) * 120;
  const Y = (x: number) => 66 - ((x - lo) / span) * 60;
  return (
    <svg className="wave-tile" viewBox="0 0 120 72" preserveAspectRatio="none">
      {limits
        .filter((l) => l >= lo && l <= hi)
        .map((l) => (
          <line key={l} x1={0} x2={120} y1={Y(l)} y2={Y(l)} className="tile-limit" vectorEffect="non-scaling-stroke" />
        ))}
      <polyline points={pts.map(([a, x]) => `${X(a).toFixed(1)},${Y(x).toFixed(1)}`).join(" ")} fill="none" stroke={color} strokeWidth={1.6} vectorEffect="non-scaling-stroke" />
      {t >= from && t <= to && <line x1={X(t)} x2={X(t)} y1={0} y2={72} className="tile-cursor" vectorEffect="non-scaling-stroke" />}
    </svg>
  );
}

/** 도면 조각 타일 (CAM-SYM): 마스터 도면에서 설비 상자(심볼 · 이름 · KKS)를 타일 비율로 잘라 보입니다 */
function SymbolTile({ eq, dark }: { eq: Equipment; dark: boolean }) {
  const url = useDrawingImage(eq.drawing);
  if (!url) return <div className="tile-empty">도면을 불러오는 중</div>;
  const [W, H] = SHEET_PX;
  const aspect = 4 / 3;
  const pad = 0.012;
  let [x0, y0, x1, y1] = [eq.bbox[0] - pad, eq.bbox[1] - pad, eq.bbox[2] + pad, eq.bbox[3] + pad];
  // 타일 비율(4:3)에 맞춰 짧은 쪽을 늘립니다 (그림이 찌그러지지 않게)
  const wpx = (x1 - x0) * W;
  const hpx = (y1 - y0) * H;
  if (wpx / hpx > aspect) {
    const grow = (wpx / aspect / H - (y1 - y0)) / 2;
    y0 -= grow;
    y1 += grow;
  } else {
    const grow = ((hpx * aspect) / W - (x1 - x0)) / 2;
    x0 -= grow;
    x1 += grow;
  }
  const bw = Math.min(x1 - x0, 1);
  const bh = Math.min(y1 - y0, 1);
  x0 = clamp(x0, 0, 1 - bw);
  y0 = clamp(y0, 0, 1 - bh);
  const pos = (o: number, size: number) => (size >= 1 ? 0 : (o / (1 - size)) * 100);
  return (
    <div
      className={`sym-crop ${dark ? "dark" : ""}`}
      style={{ backgroundImage: `url(${url})`, backgroundSize: `${100 / bw}% ${100 / bh}%`, backgroundPosition: `${pos(x0, bw)}% ${pos(y0, bh)}%` }}
    />
  );
}

interface CardProps {
  n: number;
  target: CardTarget;
  plant: Plant;
  detail: Detail | null;
  t: number;
  tracks: Map<string, SdsTrack[]>;
  metrics: Map<string, Metrics>;
  mark: SearchMark | undefined;
  remote: string | null;
  dark: boolean;
  onSelect: (kks: string) => void;
  onZoom: (kks: string) => void;
  onInterlock: (() => void) | null;
  bind: (el: HTMLElement | null) => void;
}

/** 대표 계측: 근본 원인은 원인 태그, 경보 설비는 경보 태그, 나머지는 SDS 전이가 있는 첫 센서 → 첫 센서 */
function primarySensor(plant: Plant, detail: Detail | null, tracks: Map<string, SdsTrack[]>, kks: string): Sensor | undefined {
  const root = detail?.analysis.substantive?.causal?.root;
  const tag = root?.kks === kks ? root.tag : detail?.analysis.route.kks === kks ? detail.analysis.route.tag : tracks.get(kks)?.[0]?.tag;
  return plant.sensors.find((s) => s.tag === tag) ?? sensorsOf(plant, kks)[0];
}

function Card({ n, target, plant, detail, t, tracks, metrics, mark, remote, dark, onSelect, onZoom, onInterlock, bind }: CardProps) {
  const eq = plant.equipment.find((e) => e.kks === target.kks);
  if (!eq) return null;
  const tl = detail?.timeline ?? null;
  const sensor = primarySensor(plant, detail, tracks, eq.kks);
  const track = tracks.get(eq.kks)?.find((x) => x.tag === sensor?.tag) ?? tracks.get(eq.kks)?.[0];
  const sds = track ? sdsStateAt(track.transitions, t) : null;
  const metric = sensor ? metrics.get(sensor.tag) : undefined;
  const value = sensor ? (tl ? valueAt(tl, sensor.tag, t) : sensor.normal) : null;
  const color = SDS_COLOR[sds ?? "STABLE"] ?? "#4fd1c5";
  const stop = (fn: () => void) => (e: React.MouseEvent) => {
    e.stopPropagation();
    fn();
  };
  return (
    <article ref={bind} className={`cctv ${target.roles.join(" ")}`} data-card={eq.kks} aria-label={`${eq.kks} CCTV`} onClick={() => onSelect(eq.kks)}>
      <header className="cctv-head">
        <span className={tl ? "rec on" : "rec"} />
        <b className="mono">CAM-EQ{n}</b>
        <span className="mono cctv-kks">{eq.kks}</span>
        <span className="grow" />
        {target.roles.map((r) => (
          <em key={r} className={`cctv-role ${r}`}>
            {r === "hit" && mark ? `🎯 #${mark.rank}` : ROLE_TAG[r]}
          </em>
        ))}
      </header>
      <div className="cctv-status" title={`${eq.name} · ${eq.place}`}>
        <span className="cctv-name">{eq.name}</span>
        {sds && (
          <b className="mono" style={{ color }}>
            {sds}
          </b>
        )}
        {metric && <i className="mono">dx/dt {fixed(metric.slope_per_sec_milli / 1000, 2)}/s</i>}
      </div>
      {remote && <div className="cctv-remote">📍 {remote} 도면 · 점프 커넥터로 이어짐</div>}
      {mark && (
        <div className="cctv-why" title={mark.why.join(" · ")}>
          🎯 #{mark.rank} 적중 · 왜: {mark.why.length ? mark.why.slice(0, 4).join(" · ") : `${mark.kind} ${mark.id}`}
        </div>
      )}
      <div className="cctv-tiles">
        <figure className="cctv-tile">
          <WaveTile tl={tl} sensor={sensor} t={t} color={color} />
          <figcaption>
            CAM-WAVE{sensor ? ` · ${shortTag(sensor.tag)} ${fmt(value)} ${sensor.unit}` : ""}
          </figcaption>
        </figure>
        <figure className="cctv-tile">
          <SymbolTile eq={eq} dark={dark} />
          <figcaption>CAM-SYM · {eq.drawing}</figcaption>
        </figure>
      </div>
      <footer className="cctv-foot">
        <button className="vt-btn" onClick={stop(() => onZoom(eq.kks))}>
          🔍 파형 확대
        </button>
        <button className="vt-btn" disabled={!onInterlock} onClick={stop(() => onInterlock?.())} title={onInterlock ? "우측 결재 HUD 의 인터록 게이트로" : "인터록이 있는 사건이 아닙니다"}>
          📋 인터록 이동
        </button>
      </footer>
    </article>
  );
}

interface Props {
  stage: React.RefObject<HTMLDivElement>;
  locator: () => Locator | null;
  targets: CardTarget[];
  plant: Plant;
  detail: Detail | null;
  t: number;
  tracks: Map<string, SdsTrack[]>;
  metrics: Map<string, Metrics>;
  marks: Map<string, SearchMark>;
  remoteOf: (kks: string) => string | null;
  dark: boolean;
  onSelect: (kks: string) => void;
  onZoom: (kks: string) => void;
  onInterlock: (() => void) | null;
}

/** 도면 위 설비 CCTV 말풍선 층 (2차 개선안 1-①, noto-sandbox `.cctv-layer` 이식).
 *  근본 원인 · 경보 · 선택 · 검색 1위 설비마다 반투명 카드를 띄우고 설비 상자에서 점선 지시선을 긋습니다.
 *  카드는 코너 알약 · 결재 HUD · 하단 허브(`.ov-avoid`)와 서로, 그리고 말풍선이 가리키는 설비를 덮지 않는 자리를 고르고,
 *  될 수 있으면 다른 설비 상자도 피합니다. 자리는 매 프레임 DOM 에서 읽어 확대 · 이동 · 마인드맵 움직임을 따라갑니다. */
export function CctvOverlay(props: Props) {
  const { stage, locator, targets } = props;
  const nodes = useRef(new Map<string, HTMLElement>());
  const lines = useRef(new Map<string, SVGLineElement>());
  const slots = useRef(new Map<string, number>());
  const dirty = useRef(true);
  const live = useRef({ targets, locator });
  live.current = { targets, locator };
  // 설비마다 고정된 ref 함수 (렌더마다 새 함수를 주면 React 가 떼었다 다시 붙여 자리를 잃습니다)
  const binders = useRef(new Map<string, { node: (el: HTMLElement | null) => void; line: (el: SVGLineElement | null) => void }>());
  const binder = (kks: string) => {
    let b = binders.current.get(kks);
    if (!b) {
      b = {
        node: (el) => {
          dirty.current = true;
          if (el) {
            // 처음 자리를 잡기 전에는 숨깁니다 (왼쪽 위 구석에 한 프레임 그려지지 않게)
            el.style.visibility = "hidden";
            nodes.current.set(kks, el);
          } else {
            nodes.current.delete(kks);
            slots.current.delete(kks);
          }
        },
        line: (el) => {
          dirty.current = true;
          if (el) lines.current.set(kks, el);
          else lines.current.delete(kks);
        },
      };
      binders.current.set(kks, b);
    }
    return b;
  };

  useEffect(() => {
    let raf = 0;
    let last = "";
    const tick = () => {
      raf = requestAnimationFrame(tick);
      const st = stage.current;
      if (!st) return;
      const { targets: list, locator: getLocator } = live.current;
      const loc = getLocator();
      const W = st.clientWidth;
      const H = st.clientHeight;
      const origin = st.getBoundingClientRect();
      const hard = [...st.querySelectorAll(".ov-avoid")].map((el) => relRect(el, origin)).filter((r) => r.w > 0 && r.h > 0);
      const anchors = new Map<string, Anchor | null>(list.map((x) => [x.kks, loc ? loc.find(x.kks) : null]));
      const soft = loc ? loc.others() : [];
      const sizes = list.map((x) => {
        const node = nodes.current.get(x.kks);
        return node ? [node.offsetWidth, node.offsetHeight] : [0, 0];
      });
      const round = (r: Rect | null | undefined) => (r ? `${r.x | 0},${r.y | 0},${r.w | 0},${r.h | 0}` : "-");
      const sig = `${W}x${H}|${list.map((x) => x.kks).join(",")}|${[...anchors.values()].map(round).join(";")}|${hard.map(round).join(";")}|${soft.map(round).join(";")}|${sizes.join(";")}`;
      if (sig === last && !dirty.current) return;
      last = sig;
      dirty.current = false;
      const placed: Rect[] = [];
      const own = [...anchors.values()].filter((a): a is Anchor => !!a);
      list.forEach((target, i) => {
        const node = nodes.current.get(target.kks);
        const line = lines.current.get(target.kks);
        if (!node || !line) return;
        const a = anchors.get(target.kks);
        const cx = a ? a.x + a.w / 2 : -1;
        const cy = a ? a.y + a.h / 2 : -1;
        if (!a || cx < 0 || cy < 0 || cx > W || cy > H) {
          node.style.visibility = "hidden";
          line.style.display = "none";
          return;
        }
        const [w, h] = sizes[i];
        const cands = spots(a, w, h).map(([x, y]) => ({ x: clamp(x, EDGE, W - w - EDGE), y: clamp(y, EDGE, H - h - EDGE), w, h }));
        const panels = [...hard, ...placed];
        const free = (k: number, extra: Rect[]) => !panels.some((r) => hits(cands[k], r)) && !extra.some((r) => hits(cands[k], r));
        const prev = slots.current.get(target.kks) ?? 0;
        const order = [prev, ...cands.keys()];
        // 1) 판 · 다른 말풍선 · 가리키는 설비 · 다른 설비를 모두 피함 → 2) 다른 설비는 덮어도 됨 → 3) 가리키는 설비까지 덮어도 됨 (판 밑으로는 숨지 않게)
        const slot = order.find((k) => free(k, [...own, ...soft])) ?? order.find((k) => free(k, own)) ?? order.find((k) => free(k, [])) ?? prev;
        slots.current.set(target.kks, slot);
        const r = cands[slot];
        placed.push(r);
        node.style.visibility = "";
        node.style.transform = `translate(${Math.round(r.x)}px, ${Math.round(r.y)}px)`;
        // 지시선: 설비 상자에서 카드 쪽으로 가장 가까운 점 → 카드에서 상자 쪽으로 가장 가까운 점
        const ax = clamp(r.x + w / 2, a.x, a.x + a.w);
        const ay = clamp(r.y + h / 2, a.y, a.y + a.h);
        const bx = clamp(ax, r.x, r.x + w);
        const by = clamp(ay, r.y, r.y + h);
        line.setAttribute("x1", ax.toFixed(1));
        line.setAttribute("y1", ay.toFixed(1));
        line.setAttribute("x2", bx.toFixed(1));
        line.setAttribute("y2", by.toFixed(1));
        line.style.display = "";
      });
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [stage]);

  return (
    <div className="cctv-layer">
      <svg className="cctv-lines" aria-hidden="true">
        {targets.map((x) => (
          <line key={x.kks} className={x.roles.join(" ")} style={{ display: "none" }} ref={binder(x.kks).line} />
        ))}
      </svg>
      {targets.map((x, i) => (
        <Card
          key={x.kks}
          n={i + 1}
          target={x}
          plant={props.plant}
          detail={props.detail}
          t={props.t}
          tracks={props.tracks}
          metrics={props.metrics}
          mark={props.marks.get(x.kks)}
          remote={props.remoteOf(x.kks)}
          dark={props.dark}
          onSelect={props.onSelect}
          onZoom={props.onZoom}
          onInterlock={props.onInterlock}
          bind={binder(x.kks).node}
        />
      ))}
    </div>
  );
}
