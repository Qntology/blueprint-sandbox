import { useEffect, useMemo, useRef, useState, type ReactNode } from "react";
import type { CausalTrace } from "./cctv/traces";
import { clock, signedSecs } from "./plant";
import type { Detail } from "./types";

const STRIP_ROWS = 3;
const SAMPLES = 320;

/** 하단 타임머신 리플레이 (기획 Part 8-1B 4절). 슬라이더를 움직이면 세 뷰의 색 · 흐름 · 병목이 그 시각으로 바뀝니다.
 *  2차 개선안: 하단 허브 안의 한 줄로 줄이고, 슬라이더 바탕에 상류(D-1) · 원인(D 0) · 경보(D+n) 3줄 미니 파형을 깔아
 *  CAM-02 를 펼치지 않아도 변곡이 어디서 먼저 났는지 보입니다. */
export function TimeScrubber({ detail, t, onT, onSds, traces, lead }: { detail: Detail; t: number; onT: (t: number) => void; onSds: () => void; traces: CausalTrace[]; lead?: ReactNode }) {
  const tl = detail.timeline;
  const length = Math.max(...Object.values(tl.series).map((v) => v.length), 1);
  const start = tl.t0_ms;
  const end = tl.t0_ms + (length - 1) * tl.period_ms;
  const origin = detail.analysis.substantive?.causal?.root?.at_ms ?? detail.analysis.chain[0]?.at_ms ?? detail.analysis.sds.center_ms;
  const [playing, setPlaying] = useState(false);
  const [speed, setSpeed] = useState(1);
  const tRef = useRef(t);
  tRef.current = t;

  useEffect(() => {
    if (!playing) return;
    let last = performance.now();
    let raf = 0;
    const step = (now: number) => {
      const next = tRef.current + (now - last) * speed;
      last = now;
      if (next >= tl.now_ms) {
        onT(tl.now_ms);
        setPlaying(false);
        return;
      }
      onT(next);
      raf = requestAnimationFrame(step);
    };
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [playing, speed, tl.now_ms, onT]);

  // 3줄 미니 스트립: 줄마다 자기 최소 ~ 최대로 맞춰 띠 하나(12 단위)에 그립니다.
  const bands = useMemo(
    () =>
      traces.slice(0, STRIP_ROWS).map((tr, row) => {
        const v = tr.channel ? tl.channels[tr.tag]?.[tr.channel] : tl.series[tr.tag];
        if (!v?.length) return { tr, points: "" };
        const step = Math.max(1, Math.floor(v.length / SAMPLES));
        const picked: [number, number][] = [];
        for (let i = 0; i < v.length; i += step) picked.push([tl.t0_ms + i * tl.period_ms, v[i]]);
        const lo = Math.min(...picked.map((p) => p[1]));
        const hi = Math.max(...picked.map((p) => p[1]));
        const span = Math.max(hi - lo, 1e-9);
        const points = picked.map(([at, x]) => `${(((at - start) / Math.max(end - start, 1)) * 1000).toFixed(1)},${(row * 12 + 11 - ((x - lo) / span) * 10).toFixed(1)}`).join(" ");
        return { tr, points };
      }),
    [traces, tl, start, end],
  );

  const pos = (ms: number) => `${((ms - start) / Math.max(end - start, 1)) * 100}%`;
  return (
    <div className="scrubber">
      {lead}
      <button
        className="plain"
        onClick={() => {
          if (!playing) onT(Math.max(start, origin - 10_000));
          setPlaying((p) => !p);
        }}
      >
        {playing ? "⏸ 멈춤" : "▶ 재생"}
      </button>
      <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))} aria-label="재생 속도">
        <option value={1}>1x</option>
        <option value={4}>4x</option>
        <option value={0.25}>0.25x</option>
      </select>
      <button className="plain" onClick={() => onT(origin)}>
        발원점
      </button>
      <button className="plain" onClick={() => onT(tl.now_ms)}>
        분석 시각
      </button>
      <div className="scrub-track" title={bands.map((b) => b.tr.title).join("\n")}>
        <svg className="scrub-waves" viewBox={`0 0 1000 ${STRIP_ROWS * 12}`} preserveAspectRatio="none" aria-hidden="true">
          {bands.map((b, i) => (b.points ? <polyline key={i} points={b.points} fill="none" stroke={b.tr.color} strokeWidth={1.3} vectorEffect="non-scaling-stroke" /> : null))}
        </svg>
        {bands.map((b, i) => (
          <span key={i} className="strip-label" style={{ top: `${(i * 100) / STRIP_ROWS}%`, color: b.tr.color }}>
            {b.tr.short}
          </span>
        ))}
        <input type="range" min={start} max={end} step={100} value={t} onChange={(e) => onT(Number(e.target.value))} aria-label="재생 시각" />
        {detail.analysis.chain.map((ev, i) => (
          <span key={i} className={`scrub-mark ${ev.kind}`} style={{ left: pos(ev.at_ms) }} title={`${signedSecs(ev.at_ms, origin)} ${ev.tag} ${ev.label}`} />
        ))}
        <span className="scrub-origin" style={{ left: pos(origin) }} title="t = 0 발원" />
        <span className="scrub-now" style={{ left: pos(tl.now_ms) }} title="분석 시각" />
      </div>
      <span className="mono scrub-time">
        {clock(t)} ({signedSecs(t, origin)})
      </span>
      <button className="teal" onClick={onSds}>
        ⏱️ 시계열 · SDS 분석기
      </button>
    </div>
  );
}
