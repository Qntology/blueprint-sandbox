import { useEffect, useRef, useState } from "react";
import { clock, signedSecs } from "./plant";
import type { Detail } from "./types";

/** 하단 타임머신 리플레이 (기획 Part 8-1B 4절). 슬라이더를 움직이면 세 뷰의 색 · 흐름 · 병목이 그 시각으로 바뀝니다. */
export function TimeScrubber({ detail, t, onT, onSds }: { detail: Detail; t: number; onT: (t: number) => void; onSds: () => void }) {
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

  const pos = (ms: number) => `${((ms - start) / (end - start)) * 100}%`;
  return (
    <div className="scrubber">
      <span className="scrub-title">⏪ 타임머신 리플레이</span>
      <button
        className="plain"
        onClick={() => {
          if (!playing) onT(Math.max(start, origin - 10_000));
          setPlaying((p) => !p);
        }}
      >
        {playing ? "⏸ 멈춤" : "▶ 재생"}
      </button>
      <select value={speed} onChange={(e) => setSpeed(Number(e.target.value))}>
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
      <div className="scrub-track">
        <input type="range" min={start} max={end} step={100} value={t} onChange={(e) => onT(Number(e.target.value))} />
        {detail.analysis.chain.map((ev, i) => (
          <span key={i} className={`scrub-mark ${ev.kind}`} style={{ left: pos(ev.at_ms) }} title={`${signedSecs(ev.at_ms, origin)} ${ev.tag} ${ev.label}`} />
        ))}
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
