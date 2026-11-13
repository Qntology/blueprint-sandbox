"""두 실측 묶음(예: q8 기준 `out/base` · 피라미드 `out/pyramid`)을 견줘 바뀐 것만 표로 씁니다.

    python scripts/compare_runs.py out/base out/pyramid out/compare.md

`chain_measure` 순서의 파일을 봅니다: plant_ko · plant_aidc · plant_en · aidc_en (plant_probe), a0_fast (a0_probe), delta · delta_aidc (delta_probe).
판정 · 확신도 · SDS 축 확률 · 근본 원인 · 초안(모델 글 · 대조 결과 · 글) · 도면 KKS · 도면번호 · 개정 · 현장 스캔 칸 · A0 원장 상자 · 델타 판독 값을 봅니다.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path


def load(d: Path, name: str):
    p = d / f"{name}.json"
    return json.loads(p.read_text(encoding="utf-8")) if p.exists() else None


def fmt(x, nd=3):
    return "-" if x is None else (f"{x:.{nd}f}" if isinstance(x, float) else str(x))


def plant(a, b, title, out):
    out.append(f"\n### {title}\n")
    if a.get("drawings"):
        out.append("| 도면 | KKS (기준 → 새) | 도면번호 · 개정 | 시간 s (기준 → 새) |\n| --- | --- | --- | --- |")
        for x, y in zip(a["drawings"], b["drawings"]):
            same = (x["read_id"], x["rev"]) == (y["read_id"], y["rev"])
            title = "같음" if same else f"{x['read_id']} {x['rev']} → {y['read_id']} {y['rev']}"
            out.append(f"| {x['id']} | {x['found']}/{x['expected']} → {y['found']}/{y['expected']} | {title} | {x['ms'] / 1e3:.1f} → {y['ms'] / 1e3:.1f} |")
    if a.get("scans"):
        out.append("\n| 스캔 | 칸 (기준 → 새) | 시간 s |\n| --- | --- | --- |")
        for x, y in zip(a["scans"], b["scans"]):
            cx = all(v is True or isinstance(v, str) for v in x["checks"].values())
            cy = all(v is True or isinstance(v, str) for v in y["checks"].values())
            same = x["checks"] == y["checks"]
            out.append(f"| {x['image']} | {'모두 맞음' if cx else '틀림'} → {'모두 맞음' if cy else '틀림'}{'' if same else ' (다름)'} | {x['ms'] / 1e3:.1f} → {y['ms'] / 1e3:.1f} |")
    out.append("\n| 시나리오 | 판정 | 확신도 (기준 → 새) | SDS 축 확률 최대 차이 | 원인 | 초안 (기준 → 새) | 글 같음 | Stage 4 · 5 s (기준 → 새) |\n| --- | --- | --- | --- | --- | --- | --- | --- |")
    for x, y in zip(a["scenarios"], b["scenarios"]):
        ax = {k: v for k, v in (x.get("axes") or [])}
        ay = {k: v for k, v in (y.get("axes") or [])}
        dmax = max([abs(ax[k] - ay.get(k, 0)) for k in ax] or [0.0])
        dx, dy = x.get("draft") or {}, y.get("draft") or {}
        def dstate(d):
            if not d:
                return "-"
            return ("모델 글" if d.get("by_model") else "템플릿") + (" · 대조 통과" if not d.get("problems") else f" · 문제 {len(d['problems'])}")
        same_text = (dx.get("summary") == dy.get("summary")) if dx else True
        st = lambda s, n: next((g["ms"] for g in (s.get("stages") or []) if g["stage"] == n), None)
        root_same = (x.get("root") or {}).get("kks") == (y.get("root") or {}).get("kks")
        out.append(
            f"| {x['id']} | {'같음' if x.get('verdict') == y.get('verdict') else x.get('verdict') + ' → ' + str(y.get('verdict'))} | {fmt(x.get('confidence'))} → {fmt(y.get('confidence'))} | {dmax:.4f} | {'같음' if root_same else '다름'} | {dstate(dx)} → {dstate(dy)} | {'같음' if same_text else '다름'} | {fmt((st(x, 4) or 0) / 1e3, 2)} · {fmt((st(x, 5) or 0) / 1e3, 1)} → {fmt((st(y, 4) or 0) / 1e3, 2)} · {fmt((st(y, 5) or 0) / 1e3, 1)} |"
        )


def a0(a, b, out):
    out.append("\n### A0 원장 상자 빠른 확인\n\n| | 방식 | KKS 맞음 | 시간 s |\n| --- | --- | --- | --- |")
    for tag, r in (("기준", a), ("새", b)):
        for run in r["runs"]:
            checks = run.get("checks") or []
            ok = f"{sum(1 for c in checks if c.get('ok'))}/{len(checks)}" if checks else str(run.get("found", "-"))
            out.append(f"| {tag} | {run.get('mode', '')} | {ok} | {run.get('ms', 0) / 1e3:.1f} |")


# 델타는 바뀐 조각만 읽으므로 원장 KKS 개수(kks_hits)는 전부 읽기와 다릅니다. 나머지 값(MOC · 기존 · 새 값 · KKS · 태그 · 도면 · 개정 · 서명 · 작성자)을 견줍니다.
def values(v):
    return {k: x for k, x in (v or {}).items() if k not in ("kks_hits",)}


def delta(a, b, title, out):
    out.append(f"\n### {title}\n\n| 스캔 | 전부 읽기 값 (기준 = 새) | 델타 값 (기준 = 새) | 전부 · 델타 시간 s (기준 → 새) | 델타 조각 (기준 → 새) |\n| --- | --- | --- | --- | --- |")
    for x, y in zip(a["scans"], b["scans"]):
        fx, fy = x.get("full") or {}, y.get("full") or {}
        dx, dy = x.get("delta") or {}, y.get("delta") or {}
        out.append(
            f"| {x['scan']} | {'같음' if values(fx.get('values')) == values(fy.get('values')) else '다름'} | {'같음' if values(dx.get('values')) == values(dy.get('values')) else '다름'} | "
            f"{fmt(fx.get('ms', 0) / 1e3, 1)} · {fmt(dx.get('ms', 0) / 1e3, 1)} → {fmt(fy.get('ms', 0) / 1e3, 1)} · {fmt(dy.get('ms', 0) / 1e3, 1)} | {dx.get('crops')} → {dy.get('crops')} |"
        )


def main() -> None:
    base, new = Path(sys.argv[1]), Path(sys.argv[2])
    out_path = Path(sys.argv[3]) if len(sys.argv) > 3 else None
    out: list[str] = [f"# {base} → {new}"]
    for name, title in (("plant_ko", "발전소 (한국어)"), ("plant_aidc", "AI 데이터센터 (한국어)"), ("plant_en", "발전소 (영어 초안)"), ("aidc_en", "AI 데이터센터 (영어 초안)")):
        a, b = load(base, name), load(new, name)
        if a and b:
            plant(a, b, title, out)
    a, b = load(base, "a0_fast"), load(new, "a0_fast")
    if a and b:
        a0(a, b, out)
    for name, title in (("delta", "델타 판독 (발전소)"), ("delta_aidc", "델타 판독 (AI 데이터센터)")):
        a, b = load(base, name), load(new, name)
        if a and b:
            delta(a, b, title, out)
    text = "\n".join(out) + "\n"
    if out_path:
        out_path.write_text(text, encoding="utf-8")
    sys.stdout.buffer.write(text.encode("utf-8"))


if __name__ == "__main__":
    main()
