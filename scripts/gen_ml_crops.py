"""12개 언어 합성 문서 조각 (다국어 판독 실측용): 설비 카드 · 점검 메모 · 상태 줄을 언어마다 그려 정답과 함께 저장합니다.

    python scripts/gen_ml_crops.py [출력 폴더]      (기본 out/ml_crops)

그림은 Microsoft Edge 헤드리스로 그립니다 (아랍어 이어 쓰기 · 오른쪽에서 왼쪽, 일본어 · 중국어 글꼴을 시스템 글꼴 그대로).
출력: `<언어>_<번호>.png` 와 `crops.json` (조각마다 언어 · 정답 글). 표준 라이브러리만 씁니다.
설비 코드 · 숫자는 원장(합성)과 같은 꼴이고 사람 · 설비는 실제가 아닙니다.
"""

from __future__ import annotations

import html
import json
import shutil
import subprocess
import sys
from pathlib import Path

EDGE_PATHS = [
    r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
]

# 언어마다: (설비 이름, 위치 머리, 상태 문장, 점검 메모)
TEXTS = {
    "ko": ("주급수펌프 A", "위치: 103동 2F", "토출압력 정상 범위", "현장 게이지 확인 완료"),
    "en": ("Main feedwater pump A", "Location: Bldg 103 2F", "Discharge pressure within normal range", "Field gauge checked"),
    "de": ("Hauptspeisewasserpumpe A", "Standort: Gebäude 103, 2. OG", "Förderdruck im Normalbereich", "Vor-Ort-Anzeige geprüft"),
    "fr": ("Pompe alimentaire principale A", "Emplacement : bâtiment 103, 2e étage", "Pression de refoulement normale", "Manomètre local vérifié"),
    "es": ("Bomba principal de agua de alimentación A", "Ubicación: edificio 103, planta 2", "Presión de descarga en rango normal", "Manómetro local revisado"),
    "it": ("Pompa principale acqua di alimento A", "Posizione: edificio 103, piano 2", "Pressione di mandata nella norma", "Manometro locale verificato"),
    "pt": ("Bomba principal de água de alimentação A", "Localização: edifício 103, piso 2", "Pressão de descarga dentro do normal", "Manômetro local verificado"),
    "nl": ("Hoofdvoedingswaterpomp A", "Locatie: gebouw 103, 2e verdieping", "Persdruk binnen normaal bereik", "Lokale manometer gecontroleerd"),
    "cs": ("Hlavní napájecí čerpadlo A", "Umístění: budova 103, 2. patro", "Výtlačný tlak v normálním rozsahu", "Místní manometr zkontrolován"),
    "ar": ("مضخة مياه التغذية الرئيسية A", "الموقع: المبنى 103، الطابق 2", "ضغط التصريف ضمن المعدل الطبيعي", "تم فحص المقياس الميداني"),
    "ja": ("主給水ポンプA", "設置場所：103棟 2階", "吐出圧力は正常範囲内", "現場計器を確認済み"),
    "zh": ("主给水泵A", "位置：103号楼 2层", "出口压力在正常范围内", "已检查现场仪表"),
}

FONTS = {
    "ko": "'Malgun Gothic'",
    "ja": "'Yu Gothic','MS Gothic'",
    "zh": "'Microsoft YaHei','SimSun'",
    "ar": "'Segoe UI',Tahoma",
}

# 조각마다 고르는 줄 (코드 줄 + 언어 줄)
LAYOUTS = [
    lambda t: ["KKS: 10LAB10AP001", t[0], f"{t[1]} (EL.+9.0m)"],
    lambda t: ["KKS: 10LAB20AA102", t[2], "28.5 MPa"],
    lambda t: [t[3], "2026-11-04 10:30", "10LAB10CP001 = 26.0 MPa"],
    lambda t: [t[0]],
    lambda t: [t[1], t[2]],
    lambda t: ["MOC-2026-1104", t[3], "29.0 MPa → 31.0 MPa"],
]


def edge() -> str:
    for p in EDGE_PATHS:
        if Path(p).exists():
            return p
    found = shutil.which("msedge")
    if not found:
        sys.exit("Microsoft Edge 가 없습니다")
    return found


def render(exe: str, lines: list[str], lang: str, out: Path, size: int) -> None:
    font = FONTS.get(lang, "Arial,'Segoe UI'")
    rtl = ' dir="rtl"' if lang == "ar" else ""
    body = "<br>".join(html.escape(l) for l in lines)
    page = out.with_suffix(".html")
    page.write_text(
        f'<html><head><meta charset="utf-8"></head><body style="margin:0;background:#fff">'
        f'<div style="font:{size}px {font};padding:16px 20px;line-height:1.4;color:#111;width:max-content"{rtl}>{body}</div></body></html>',
        encoding="utf-8",
    )
    width = max(480, int(max(len(l) for l in lines) * size * (1.05 if lang in ("ja", "zh", "ko") else 0.62)) + 60)
    # 창이 너무 낮으면 헤드리스 Edge 의 스크린숏이 멈춥니다 (한 줄 조각에서 실측). 남는 여백은 판독 쪽에서 잘라 냅니다.
    height = max(240, int(len(lines) * size * 1.4) + 40)
    # 스크린숏 경로는 절대 경로여야 합니다 (상대 경로는 Edge 의 작업 폴더 기준이라 엉뚱한 곳에 저장됨).
    subprocess.run(
        [exe, "--headless=new", "--disable-gpu", "--hide-scrollbars", f"--screenshot={out.resolve()}", f"--window-size={width},{height}", page.resolve().as_uri()],
        check=True,
        capture_output=True,
        timeout=120,
    )
    page.unlink()
    if not out.exists():
        sys.exit(f"그림을 만들지 못했습니다: {out}")


def main() -> None:
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "out/ml_crops")
    out.mkdir(parents=True, exist_ok=True)
    exe = edge()
    rows = []
    for lang, texts in TEXTS.items():
        for i, layout in enumerate(LAYOUTS):
            lines = layout(texts)
            name = f"{lang}_{i:02d}.png"
            render(exe, lines, lang, out / name, 30 if i % 2 == 0 else 26)
            rows.append({"image": name, "lang": lang, "text": "\n".join(lines)})
        print(f"[조각] {lang} {len(LAYOUTS)}개")
    (out / "crops.json").write_text(json.dumps(rows, ensure_ascii=False, indent=1), encoding="utf-8")
    print(f"[조각] 모두 {len(rows)}개 → {out}")


if __name__ == "__main__":
    main()
