# 영어 화면 번역 표 app/src/i18n/en.json 을 en_fixtures.py(원장 · 시나리오) · en_engine.py(엔진 글)에서 만들고,
# 주어진 글 목록을 translate.ts 와 같은 순서로 바꿔 남은 한글을 보여줍니다 (docs/i18n.md).
#   python build_en.py --write [check-file ...]   en.json 을 씁니다 (앱이 돌고 있으면 vite 가 다시 읽으니 촬영 · E2E 중에는 쓰지 않습니다)
#   python build_en.py [check-file ...]           쓰지 않고 확인만
# check-file: 한 줄에 글 하나("## " 로 시작하는 줄은 건너뜀) 또는 JSON 글 배열
import importlib, json, pathlib, re, sys

HERE = pathlib.Path(__file__).parent
sys.path.insert(0, str(HERE))
OUT = HERE.parent.parent / "app" / "src" / "i18n" / "en.json"

names, exact, phrases, patterns = {}, {}, {}, []
# 틀은 엔진 문장(구체적) → 원장 낱말 단위(동 · 층 · 호기) 차례
for mod in ["en_engine", "en_fixtures"]:
    if not (HERE / f"{mod}.py").exists():
        continue
    m = importlib.import_module(mod)
    names.update(getattr(m, "NAMES", {}))
    exact.update(getattr(m, "EXACT", {}))
    phrases.update(getattr(m, "PHRASES", {}))
    patterns += getattr(m, "PATTERNS", [])
# 통째 글(exact)도 다른 줄 안에서 바꿀 수 있게 낱말 표에 넣습니다 (길이 4 이상, 긴 것부터 바뀌므로 낱말보다 앞섭니다)
for k, v in exact.items():
    if len(k) >= 4 and k not in phrases and k not in names:
        phrases[k] = v

for k, v in list(names.items()) + list(exact.items()) + list(phrases.items()):
    if re.search("[가-힣]", v):
        print("HANGUL IN EN:", k, "->", v)

# 틀보다 먼저 바꾸는 것: 원장 이름 전부 + 긴 통째 글(10자 이상). 숫자 틀이 이름 속 "103동" 같은 조각을 먼저 바꿔 이름이 깨지지 않게 합니다.
spans = {**{k: v for k, v in exact.items() if len(k) >= 10}, **names}
TABLE = {"exact": exact, "spans": spans, "patterns": patterns, "phrases": phrases, "names": names}

# ── translate.ts 흉내 ──
HANGUL = re.compile("[가-힣]")
allph = {**names, **phrases}
keys = sorted(allph, key=len, reverse=True)
phrase_re = re.compile("|".join(re.escape(k) for k in keys)) if keys else None
span_re = re.compile("|".join(re.escape(k) for k in sorted(spans, key=len, reverse=True))) if spans else None
comp = []
for p, r in patterns:
    try:
        comp.append((re.compile(p), r))
    except re.error as e:
        print("BAD PATTERN", p, e)


def js_rep(rep):
    def f(m):
        out, i = [], 0
        while i < len(rep):
            c = rep[i]
            if c == "$" and i + 1 < len(rep):
                n = rep[i + 1]
                if n == "$":
                    out.append("$"); i += 2; continue
                if n == "&":
                    out.append(m.group(0)); i += 2; continue
                if n.isdigit():
                    # 자바스크립트처럼 두 자리 번호는 그 모둠이 있을 때만
                    if i + 2 < len(rep) and rep[i + 2].isdigit() and int(rep[i + 1 : i + 3]) <= m.re.groups:
                        out.append(m.group(int(rep[i + 1 : i + 3])) or ""); i += 3; continue
                    if int(n) <= m.re.groups:
                        out.append(m.group(int(n)) or ""); i += 2; continue
            out.append(c); i += 1
        return "".join(out)
    return f


def line(s):
    core = s.strip()
    if core in exact:
        return s.replace(core, exact[core], 1)
    out = span_re.sub(lambda m: spans[m.group(0)], s) if span_re else s
    for r, rep in comp:
        out = r.sub(js_rep(rep), out)
    if out.strip() in exact:
        return exact[out.strip()]
    if phrase_re:
        out = phrase_re.sub(lambda m: allph[m.group(0)], out)
    return re.sub(" {2,}", " ", out)


def tr(s):
    if not HANGUL.search(s):
        return s
    return "\n".join(line(l) if HANGUL.search(l) else l for l in s.split("\n"))


def read_list(f):
    text = pathlib.Path(f).read_text(encoding="utf-8")
    if f.endswith(".json"):
        v = json.loads(text)
        return v if isinstance(v, list) else v.get("misses", [])
    return [l for l in text.splitlines() if l.strip() and not l.startswith("## ")]


if __name__ == "__main__":
    write = "--write" in sys.argv
    files = [a for a in sys.argv[1:] if a != "--write"]
    if write:
        OUT.write_text(json.dumps(TABLE, ensure_ascii=False, indent=1) + "\n", encoding="utf-8")
        print(f"en.json 씀: exact {len(exact)} · patterns {len(patterns)} · phrases {len(phrases)} · names {len(names)}")
    else:
        print(f"(쓰지 않음) exact {len(exact)} · patterns {len(patterns)} · phrases {len(phrases)} · names {len(names)}")
    shown, miss = [], 0
    for f in files:
        for raw in read_list(f):
            out = tr(raw)
            if HANGUL.search(out):
                miss += 1
                shown.append(f"MISS  {raw}\n   => {out}")
    (pathlib.Path.cwd() / "i18n_misses.txt").write_text("\n".join(shown), encoding="utf-8")
    print("misses:", miss, "(i18n_misses.txt)")
