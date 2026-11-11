"""발전소 원장(core/fixtures/synthetic_db.json)을 영어 · 일본어 · 독일어 원장으로 옮깁니다 (다국어 N:N 검색 평가의 데이터 언어).

검색이 읽는 칸만 바꿉니다: 동 이름 · 동 번호, 층 이름, 설비 이름, 센서 라벨, 도면 제목, 절차서 제목 · 단계 · 고위험 조치, MOC 사유.
KKS · 태그 · 도면 번호 · 단위 · 수치는 언어와 상관없으므로 그대로입니다 (정답 id 가 데이터 언어마다 같음).
번역 표(ledger_tr.json)에 없는 글이 하나라도 있으면 멈춥니다 — 한국어가 섞인 원장으로 재지 않게.

    python scripts/search_ml/gen_ledgers.py            # core/fixtures/search_ml/synthetic_db.{en,ja,de}.json
    python scripts/search_ml/gen_ledgers.py --check    # 다시 만들었을 때 바이트가 같은지
"""

import copy
import json
import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SRC = ROOT / "core" / "fixtures" / "synthetic_db.json"
TABLE = Path(__file__).with_name("ledger_tr.json")
OUT = ROOT / "core" / "fixtures" / "search_ml"
LANGS = ("en", "ja", "de")


def translate(db: dict, table: dict, lang: str) -> dict:
    strings = table["strings"]
    missing = []

    def tr(s: str) -> str:
        hit = strings.get(s, {}).get(lang)
        if hit is None:
            missing.append(s)
            return s
        return hit

    out = copy.deepcopy(db)
    out["note"] = f"{db['note']} ({lang} 원장: scripts/search_ml/gen_ledgers.py 가 synthetic_db.json 을 옮김)"
    for s in out["sectors"]:
        n = re.search(r"\d+", s["building"]).group(0)
        s["building"] = table["building"][lang].format(n=n)
        s["label"] = tr(s["label"])
    for f in out["floors"]:
        f["label"] = tr(f["label"])
    for e in out["equipment"]:
        e["name"] = tr(e["name"])
    for s in out["sensors"]:
        s["label"] = tr(s["label"])
    for d in out["drawings"]:
        d["title"] = tr(d["title"])
    for a in out["aop"]:
        a["title"] = tr(a["title"])
        a["steps"] = [tr(x) for x in a["steps"]]
        a["risk_actions"] = [tr(x) for x in a.get("risk_actions", [])]
    for m in out["moc"]:
        m["reason"] = tr(m["reason"])
    if missing:
        raise SystemExit(f"{lang}: 번역 표에 없는 글 {len(missing)}개: {sorted(set(missing))[:10]}")
    return out


def main() -> None:
    check = "--check" in sys.argv[1:]
    db = json.loads(SRC.read_text(encoding="utf-8"))
    table = json.loads(TABLE.read_text(encoding="utf-8"))
    OUT.mkdir(parents=True, exist_ok=True)
    for lang in LANGS:
        text = json.dumps(translate(db, table, lang), ensure_ascii=False, indent=1) + "\n"
        path = OUT / f"synthetic_db.{lang}.json"
        if check:
            same = path.exists() and path.read_text(encoding="utf-8") == text
            print(f"{path.name}: {'같음' if same else '다름'}")
            if not same:
                sys.exit(1)
        else:
            path.write_text(text, encoding="utf-8", newline="\n")
            print(f"{path.name}: {len(text):,} 바이트")


if __name__ == "__main__":
    main()
