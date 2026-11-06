import { useEffect, useRef } from "react";
import { createPortal } from "react-dom";
import { shortTag } from "../domain";
import { useT } from "../i18n";
import type { Context } from "../PlantTopologyView";
import { equipmentLevel, fmt, hopDepths, MEDIUM_COLOR, sdsStateAt, sensorsOf, signedSecs, valueAt, type Level } from "../plant";

interface Node {
  kks: string;
  x: number;
  y: number;
  vx: number;
  vy: number;
}

const LEVEL_COLOR: Record<Level, string> = { normal: "#4fd1c5", warn: "#f6c453", alarm: "#ff9f43", trip: "#ff4d4f" };
/** 섹터마다 원 둘레의 자리 (원장의 섹터 순서: 발전소 보일러 · 터빈 · 급수 · 전기 · 환경, 데이터센터 컴퓨트 · 네트워크 · 냉각 · 전력 · 공조) */
const SECTOR_ANGLES = [270, 340, 200, 130, 40];

interface Label {
  x: number;
  y: number;
  r: number;
  head: string;
  info: string | null;
  bold: boolean;
  alpha: number;
  /** 작을수록 먼저 자리를 잡습니다 (선택 · 근본원인 · 경보 · ±1홉 · 나머지) */
  rank: number;
}

type Rect = [number, number, number, number];

const LINE = 14;
const overlaps = (a: Rect, b: Rect) => a[0] < b[2] + 2 && b[0] < a[2] + 2 && a[1] < b[3] + 2 && b[1] < a[3] + 2;

/** 이름표를 아래 · 위 · 오른쪽 · 왼쪽 순서로 놓아 보고 이미 놓인 이름표 · 큰 원과 겹치지 않는 첫 자리를 씁니다.
 *  자리가 없으면 밝은 노드는 이름 줄만 아래에 두고, 흐린 노드는 이름표를 생략합니다. */
function drawLabels(g: CanvasRenderingContext2D, labels: Label[], obstacles: Rect[]) {
  const placed: Rect[] = [...obstacles];
  g.textAlign = "left";
  for (const l of labels.sort((a, b) => a.rank - b.rank)) {
    g.font = l.bold ? "bold 13px Malgun Gothic, sans-serif" : "11px Malgun Gothic, sans-serif";
    const lines = l.info ? [l.head, l.info] : [l.head];
    const w = Math.max(...lines.map((s) => g.measureText(s).width)) + 6;
    const h = lines.length * LINE + 4;
    const spots: Rect[] = [
      [l.x - w / 2, l.y + l.r + 3, l.x + w / 2, l.y + l.r + 3 + h],
      [l.x - w / 2, l.y - l.r - 3 - h, l.x + w / 2, l.y - l.r - 3],
      [l.x + l.r + 6, l.y - h / 2, l.x + l.r + 6 + w, l.y + h / 2],
      [l.x - l.r - 6 - w, l.y - h / 2, l.x - l.r - 6, l.y + h / 2],
    ];
    let spot = spots.find((s) => !placed.some((p) => overlaps(s, p)));
    let shown = lines;
    if (!spot) {
      if (l.rank >= 4) continue;
      shown = [l.head];
      spot = spots[0];
    }
    placed.push(spot);
    g.globalAlpha = l.alpha;
    if (l.rank < 4) {
      g.fillStyle = "rgba(22,25,34,0.78)";
      g.fillRect(spot[0], spot[1], spot[2] - spot[0], shown.length * LINE + 4);
    }
    shown.forEach((s, i) => {
      g.fillStyle = i === 0 ? "#e6ebf5" : "#9fb0cc";
      g.fillText(s, spot![0] + 3, spot![1] + 12 + i * LINE);
    });
  }
}

/** 뷰 ③: 옵시디언 스타일 마인드맵 (힘 기반 그래프). 누르면 그 설비가 중심(Depth 0)이 되고 ±1 홉만 밝게 남습니다.
 *  2차 개선안: 캔버스가 무대 전체를 쓰고, 그래프 중심은 결재 HUD 를 뺀 빈 자리 가운데로 옮겨 갑니다 (noto-sandbox `.stage.has-unit`).
 *  변곡을 지난 노드 옆에는 유리 핀(`t=0 발원` · `Δt +4.3s` · 지금 값)이 붙고, CCTV 말풍선은 노드 좌표에서 지시선을 긋습니다. */
export function MindMapView({ ctx }: { ctx: Context }) {
  const canvas = useRef<HTMLCanvasElement>(null);
  const say = useT();
  const nodes = useRef<Node[]>([]);
  const live = useRef(ctx);
  live.current = ctx;
  const drag = useRef<{ kks: string; moved: boolean } | null>(null);
  /** 화면 좌표의 그래프 중심 (빈 자리 가운데로 부드럽게 옮겨 갑니다) */
  const centerAt = useRef<{ x: number; y: number } | null>(null);
  /** 노드의 무대 좌표 · 반지름 (말풍선 · 핀 자리) */
  const screen = useRef(new Map<string, { x: number; y: number; r: number }>());
  const pins = useRef(new Map<string, HTMLElement>());

  // 처음 자리: 섹터마다 원 둘레의 한쪽에 모읍니다.
  useEffect(() => {
    if (nodes.current.length) return;
    nodes.current = ctx.plant.equipment.map((e, i) => {
      const slot = ctx.plant.sectors.findIndex((s) => s.key === e.sector);
      const a = ((SECTOR_ANGLES[slot] ?? 0) * Math.PI) / 180;
      return { kks: e.kks, x: Math.cos(a) * 260 + ((i * 37) % 60) - 30, y: Math.sin(a) * 200 + ((i * 53) % 60) - 30, vx: 0, vy: 0 };
    });
  }, [ctx.plant]);

  // 말풍선 자리 찾기를 캔버스 노드 좌표로 바꿉니다 (캔버스는 무대 왼쪽 위에 붙어 있어 캔버스 좌표 = 무대 좌표)
  useEffect(() => {
    ctx.setLocator({
      find(kks) {
        const p = screen.current.get(kks);
        return p ? { x: p.x - p.r, y: p.y - p.r, w: p.r * 2, h: p.r * 2 } : null;
      },
      others() {
        return [...screen.current.values()].map((p) => ({ x: p.x - p.r, y: p.y - p.r, w: p.r * 2, h: p.r * 2 }));
      },
    });
    return () => ctx.setLocator(null);
  }, [ctx.setLocator]); // eslint-disable-line react-hooks/exhaustive-deps

  useEffect(() => {
    const el = canvas.current!;
    let raf = 0;
    let w = 0;
    let h = 0;
    const dpr = window.devicePixelRatio || 1;
    const resize = () => {
      w = el.clientWidth;
      h = el.clientHeight;
      el.width = w * dpr;
      el.height = h * dpr;
    };
    resize();
    const ro = new ResizeObserver(resize);
    ro.observe(el);
    const started = performance.now();

    const tick = (now: number) => {
      const c = live.current;
      const g = el.getContext("2d")!;
      g.setTransform(dpr, 0, 0, dpr, 0, 0);
      const ns = nodes.current;
      const byKks = new Map(ns.map((n) => [n.kks, n]));
      const depths = c.selected ? hopDepths(c.plant.edges, c.selected, 3) : new Map<string, number>();
      // 빈 자리(결재 HUD · 코너 알약을 뺀 곳)의 가운데. HUD 를 접고 펴면 천천히 따라갑니다.
      const want = { x: c.inset.left + (w - c.inset.left - c.inset.right) / 2, y: c.inset.top + (h - c.inset.top - c.inset.bottom) / 2 };
      const cur = centerAt.current ?? want;
      centerAt.current = { x: cur.x + (want.x - cur.x) * 0.15, y: cur.y + (want.y - cur.y) * 0.15 };
      const { x: ox, y: oy } = centerAt.current;

      // 힘: 반발 · 간선 탄성 · 중심 · 선택 설비를 가운데로
      for (const a of ns) {
        for (const b of ns) {
          if (a === b) continue;
          const dx = a.x - b.x;
          const dy = a.y - b.y;
          const d2 = Math.max(dx * dx + dy * dy, 25);
          const f = 2600 / d2;
          a.vx += (dx / Math.sqrt(d2)) * f;
          a.vy += (dy / Math.sqrt(d2)) * f;
        }
      }
      for (const e of c.plant.edges) {
        const a = byKks.get(e.from);
        const b = byKks.get(e.to);
        if (!a || !b) continue;
        const dx = b.x - a.x;
        const dy = b.y - a.y;
        const d = Math.max(Math.hypot(dx, dy), 1);
        const f = (d - 95) * 0.012;
        a.vx += (dx / d) * f;
        a.vy += (dy / d) * f;
        b.vx -= (dx / d) * f;
        b.vy -= (dy / d) * f;
      }
      for (const n of ns) {
        const pull = n.kks === c.selected ? 0.08 : 0.004;
        n.vx -= n.x * pull;
        n.vy -= n.y * pull;
        if (drag.current?.kks === n.kks) continue;
        n.vx *= 0.82;
        n.vy *= 0.82;
        n.x += Math.max(-12, Math.min(12, n.vx));
        n.y += Math.max(-12, Math.min(12, n.vy));
      }

      // 그리기
      g.fillStyle = "#161922";
      g.fillRect(0, 0, w, h);
      g.save();
      g.translate(ox, oy);
      const bright = (k: string) => !c.selected || (depths.has(k) && Math.abs(depths.get(k)!) <= 1);
      const near = (k: string) => depths.has(k) && Math.abs(depths.get(k)!) <= 2;
      const phase = (now - started) / 1000;
      for (const e of c.plant.edges) {
        const a = byKks.get(e.from);
        const b = byKks.get(e.to);
        if (!a || !b) continue;
        const flow = c.flows.find((f) => f.from === e.from && f.to === e.to);
        const onPath = c.path.includes(e.from) && c.path.includes(e.to);
        const lit = bright(e.from) && bright(e.to);
        g.globalAlpha = lit ? 0.95 : near(e.from) && near(e.to) ? 0.4 : 0.12;
        g.strokeStyle = flow?.bottleneck && c.mining ? "#ff4d4f" : onPath ? "#f0a020" : MEDIUM_COLOR[e.medium] ?? "#6b7280";
        g.lineWidth = (onPath ? 3 : 1.4) * (c.mining && flow ? Math.max(0.5, Math.min(2.2, flow.ratio * 1.6)) : 1);
        g.setLineDash(flow?.bottleneck && c.mining ? [6, 6] : e.medium === "power" ? [4, 4] : []);
        g.beginPath();
        g.moveTo(a.x, a.y);
        g.lineTo(b.x, b.y);
        g.stroke();
        g.setLineDash([]);
        // 화살촉
        const ang = Math.atan2(b.y - a.y, b.x - a.x);
        const tx = b.x - Math.cos(ang) * 16;
        const ty = b.y - Math.sin(ang) * 16;
        g.beginPath();
        g.moveTo(tx, ty);
        g.lineTo(tx - Math.cos(ang - 0.4) * 8, ty - Math.sin(ang - 0.4) * 8);
        g.lineTo(tx - Math.cos(ang + 0.4) * 8, ty - Math.sin(ang + 0.4) * 8);
        g.closePath();
        g.fillStyle = g.strokeStyle;
        g.fill();
        // 마이닝: 흐름 입자 (흐름이 줄면 느리고 듬성듬성)
        if (c.mining && lit) {
          const speed = 0.35 * (flow ? Math.max(0.15, flow.ratio) : 1);
          for (let k = 0; k < 3; k++) {
            const p = (phase * speed + k / 3) % 1;
            g.beginPath();
            g.arc(a.x + (b.x - a.x) * p, a.y + (b.y - a.y) * p, 2.2, 0, Math.PI * 2);
            g.fillStyle = flow?.bottleneck ? "#ff8080" : "#d6f6ff";
            g.fill();
          }
        }
        if (c.mining && flow?.lag_secs != null && lit) {
          g.globalAlpha = 1;
          g.fillStyle = "#ffcf70";
          g.font = "11px Malgun Gothic, sans-serif";
          g.fillText(`Δt +${flow.lag_secs.toFixed(1)}s`, (a.x + b.x) / 2 + 6, (a.y + b.y) / 2 - 6);
        }
      }
      const labels: Label[] = [];
      const obstacles: Rect[] = [];
      for (const n of ns) {
        const eq = c.plant.equipment.find((e) => e.kks === n.kks)!;
        const lv = c.detail ? equipmentLevel(c.plant, n.kks, c.detail.timeline, c.t) : "normal";
        const center = n.kks === c.selected;
        const isRoot = n.kks === c.rootKks;
        const lit = c.departures.has(n.kks) && c.departures.get(n.kks)! <= c.t;
        let r = center ? 20 : 12;
        if (c.mining && isRoot) r *= 2;
        screen.current.set(n.kks, { x: ox + n.x, y: oy + n.y, r: center ? r + 18 : r });
        g.globalAlpha = bright(n.kks) ? 1 : near(n.kks) ? 0.45 : 0.18;
        if (c.mining && isRoot) {
          const ripple = (phase % 1.6) / 1.6;
          g.beginPath();
          g.arc(n.x, n.y, r + ripple * 40, 0, Math.PI * 2);
          g.strokeStyle = `rgba(255,77,79,${1 - ripple})`;
          g.lineWidth = 3;
          g.stroke();
        }
        // 개선안 2-3: SDS 변곡 링. 재생 시각이 그 설비의 첫 변곡을 지나면 켜집니다 (발원지는 적색 이중 링, 파급은 황색 링).
        // 시차 글은 캔버스가 아니라 노드 옆 유리 핀(.mm-pin)에 씁니다.
        const dep = c.departures.get(n.kks);
        const pin = pins.current.get(n.kks);
        if (pin) pin.style.transform = `translate(${Math.round(ox + n.x)}px, ${Math.round(oy + n.y - r - 12)}px) translate(-50%, -100%)`;
        if (dep !== undefined && dep <= c.t) {
          const rings = isRoot ? [r + 6, r + 11] : [r + 6];
          for (const rad of rings) {
            g.beginPath();
            g.arc(n.x, n.y, rad, 0, Math.PI * 2);
            g.strokeStyle = isRoot ? "#ff4d4f" : "#f6c453";
            g.lineWidth = 2.5;
            g.stroke();
          }
        }
        if (center) {
          for (const [rad, a] of [
            [r + 10, 0.5],
            [r + 18, 0.25],
          ] as const) {
            g.beginPath();
            g.arc(n.x, n.y, rad, 0, Math.PI * 2);
            g.strokeStyle = `rgba(58,160,255,${a})`;
            g.lineWidth = 3;
            g.stroke();
          }
        }
        g.beginPath();
        g.arc(n.x, n.y, r, 0, Math.PI * 2);
        g.fillStyle = lit ? "#ff6b3d" : LEVEL_COLOR[lv];
        g.shadowColor = lit || center ? "#ffb070" : "transparent";
        g.shadowBlur = lit || center ? 18 : 0;
        g.fill();
        g.shadowBlur = 0;
        const d = depths.get(n.kks);
        const isAlarm = n.kks === c.alarmKks;
        let info: string | null = null;
        if (bright(n.kks) || center) {
          const s = sensorsOf(c.plant, n.kks)[0];
          const v = s ? (c.detail ? valueAt(c.detail.timeline, s.tag, c.t) : s.normal) : null;
          const sds = sdsStateAt(c.tracks.get(n.kks)?.[0]?.transitions, c.t);
          info = `${eq.name}${s ? ` · ${fmt(v)} ${s.unit}` : ""}${d !== undefined && c.selected ? ` · D${d > 0 ? "+" : ""}${d}` : ""}${sds && sds !== "STABLE" ? ` · ${sds}` : ""}`;
        }
        labels.push({
          x: n.x,
          y: n.y,
          r: center ? r + 18 : r,
          head: `${isRoot ? "🔥 " : isAlarm ? "⚠️ " : ""}${n.kks}`,
          info,
          bold: center,
          alpha: g.globalAlpha,
          rank: center ? 0 : isRoot ? 1 : isAlarm ? 2 : bright(n.kks) ? 3 : 4,
        });
        if (center || (c.mining && isRoot)) obstacles.push([n.x - r, n.y - r, n.x + r, n.y + r]);
        // 유리 핀 자리는 이름표가 비켜 갑니다
        if (pin) obstacles.push([n.x - pin.offsetWidth / 2, n.y - r - 12 - pin.offsetHeight, n.x + pin.offsetWidth / 2, n.y - r - 12]);
      }
      drawLabels(g, labels, obstacles);
      g.restore();
      g.globalAlpha = 1;
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => {
      cancelAnimationFrame(raf);
      ro.disconnect();
    };
  }, []);

  const hit = (e: React.PointerEvent<HTMLCanvasElement>) => {
    const r = e.currentTarget.getBoundingClientRect();
    const o = centerAt.current ?? { x: r.width / 2, y: r.height / 2 };
    const x = e.clientX - r.left - o.x;
    const y = e.clientY - r.top - o.y;
    return { x, y, node: nodes.current.find((n) => Math.hypot(n.x - x, n.y - y) < 22) };
  };

  // 변곡을 지난 노드의 유리 핀: 발원은 t=0, 파급은 Δt, 그리고 재생 시각의 대표 계측값
  const departed = [...ctx.departures.entries()].filter(([, at]) => at <= ctx.t);
  const pinText = (kks: string, at: number) => {
    const s = ctx.tracks.get(kks)?.[0] ? ctx.plant.sensors.find((x) => x.tag === ctx.tracks.get(kks)![0].tag) : sensorsOf(ctx.plant, kks)[0];
    const v = s && ctx.detail ? valueAt(ctx.detail.timeline, s.tag, ctx.t) : null;
    const value = s ? ` · ${shortTag(s.tag)} ${fmt(v)} ${s.unit}` : "";
    return kks === ctx.rootKks ? say(`🔴 t=0 발원${value}`, `🔴 t=0 origin${value}`) : `🟡 Δt ${signedSecs(at, ctx.origin)}${value}`;
  };

  return (
    <>
      <canvas
        ref={canvas}
        className="mindmap"
        onPointerDown={(e) => {
          const h = hit(e);
          if (h.node) {
            e.currentTarget.setPointerCapture(e.pointerId);
            drag.current = { kks: h.node.kks, moved: false };
          }
        }}
        onPointerMove={(e) => {
          if (!drag.current) return;
          const h = hit(e);
          const n = nodes.current.find((x) => x.kks === drag.current!.kks);
          if (n) {
            n.x = h.x;
            n.y = h.y;
            n.vx = 0;
            n.vy = 0;
            drag.current.moved = true;
          }
        }}
        onPointerUp={() => {
          if (drag.current) ctx.onSelect(drag.current.kks);
          drag.current = null;
        }}
      />
      <div className="mm-pins" aria-hidden="true">
        {departed.map(([kks, at]) => (
          <span
            key={kks}
            className={`mm-pin ${kks === ctx.rootKks ? "root" : ""}`}
            ref={(el) => {
              if (el) pins.current.set(kks, el);
              else pins.current.delete(kks);
            }}
          >
            {pinText(kks, at)}
          </span>
        ))}
      </div>
      {ctx.bar &&
        createPortal(
          <div className="view-bar ov-bar mm-legend">
            <span className="bar-label">{say("🕸️ 마인드맵", "🕸️ Mind map")}</span>
            <span>{say("(◎) 선택 중심 Depth 0 · (○) 1홉 ±1 · (.) 2홉 이상 흐림 · 🔴 이중 링 최초 변곡 발원지(t=0) · 🟡 링 시차 매칭 파급 · 🔥 근본원인 · ⚠️ 경보 · 드래그로 이동", "(◎) selected center Depth 0 · (○) 1 hop ±1 · (.) 2+ hops dimmed · 🔴 double ring = first inflection origin (t=0) · 🟡 ring = lag-matched propagation · 🔥 root cause · ⚠️ alarm · drag to move")}</span>
          </div>,
          ctx.bar,
        )}
    </>
  );
}
