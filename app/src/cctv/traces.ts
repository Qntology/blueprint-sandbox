// 사건의 인과 파형 줄: 상류(역류) → 근본 원인(D 0) → 하류(경보). CAM-02 시계열 로직 차트와 하단 허브의 3줄 미니 스트립이 같은 줄을 씁니다.
import { shortTag } from "../domain";
import type { Detail, Plant } from "../types";
import type { Trace } from "../WaveChart";

const CHANNEL_COLORS = ["#ff6b3d", "#4fd1c5", "#7aa2ff"];

export interface CausalTrace extends Trace {
  /** 미니 스트립 줄 머리 (D-1 · D 0 · D+2 · 채널 A) */
  short: string;
}

/** 사건의 t = 0 (근본 원인 변곡, 없으면 첫 변곡 · SDS 중심) */
export function originOf(detail: Detail): number {
  return detail.analysis.substantive?.causal?.root?.at_ms ?? detail.analysis.chain[0]?.at_ms ?? detail.analysis.sds.center_ms;
}

/** 원인 기준 깊이로 줄을 고릅니다: 원인보다 상류면 음수 (펌프 역류 여파 = D-1), 경보는 D+홉. 최대 네 줄. */
export function causalTraces(detail: Detail, plant: Plant): CausalTrace[] {
  const a = detail.analysis;
  const root = a.substantive?.causal?.root ?? null;
  const out: CausalTrace[] = [];
  const label = (tag: string) => {
    const s = plant.sensors.find((x) => x.tag === tag);
    const e = plant.equipment.find((x) => x.kks === s?.kks);
    return `${e?.name ?? ""} ${s?.label ?? ""} (${shortTag(tag)}, ${s?.unit ?? ""})`;
  };
  const add = (tag: string, short: string, title: string, color: string, channel?: string) => {
    if (!out.some((x) => x.tag === tag && x.channel === channel)) out.push({ tag, short, title, color, channel });
  };
  const prop = a.substantive?.causal?.propagation ?? [];
  const depth = (hops: number) => (root ? root.hops - hops : -hops);
  for (const p of prop.filter((x) => x.backflow)) add(p.upstream_tag, `D${depth(p.hops)}`, `[D${depth(p.hops)} 역류 여파] ${label(p.upstream_tag)}`, "#7aa2ff");
  if (root) add(root.tag, "D 0", `[D 0 근본 원인] ${label(root.tag)}`, "#ff6b3d");
  for (const p of prop.filter((x) => x.causal_match && !x.backflow && x.upstream_tag !== root?.tag)) add(p.upstream_tag, `D${depth(p.hops)}`, `[D${depth(p.hops)} 파급] ${label(p.upstream_tag)}`, "#c084fc");
  if (a.route.tag) {
    const chs = detail.timeline.channels[a.route.tag];
    if (chs) Object.keys(chs).forEach((c, i) => add(a.route.tag!, `채널 ${c}`, `[채널 ${c}] ${label(a.route.tag!)}`, CHANNEL_COLORS[i % 3], c));
    else add(a.route.tag, root ? `D+${root.hops}` : "D 0", `[${root ? `D+${root.hops}` : "D 0"} 경보] ${label(a.route.tag)}`, "#f6c453");
  }
  return out.slice(0, 4);
}
