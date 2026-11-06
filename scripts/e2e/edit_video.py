"""blueprint 시연 영상 편집 (한국어 · 영어). record.js 의 녹화(두 창 프레임 · 장면 표시 · 스냅숏)와 모델 기록(trace.jsonl)을 잇고,
모델이 쓰이는 자리에는 그 모델이 실제로 한 일(조각 계획 · 판독 · 판단 확률 · 초안 · 대조)을 그린 장면을 끼워 넣습니다.

    python scripts/e2e/edit_video.py --lang ko --rec out/rec/master_ko [--rec out/rec/master_ko_fix ...] --out out/video/blueprint_master_e2e_ko.mp4
    STILLS=<폴더> 를 주면 영상 대신 구간마다 한 장씩 저장합니다 (검토용).

녹화 폴더가 여럿이면 뒤 폴더에 있는(실패하지 않은) 장면이 앞 폴더의 같은 장면을 대신합니다 (실패한 장면만 다시 찍을 때).
화면의 숫자와 글은 그 실행의 기록(marks · snaps · trace)과 전처리 실측 파일(out/delta_*.json · plant_probe*.json · symbol_probe.json)에서 옵니다.
영어 영상은 앱 영어 화면과 같은 번역 표(app/src/i18n/en.json)로 모델 입력 사실 · 판단 질문을 영어로 보여 줍니다 (모델은 한국어 원문을 읽습니다).
"""
import argparse
import bisect
import json
import math
import os
import re
import subprocess
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
W, H = 1920, 1080
FPS = 30
TOP, BOTTOM = 56, 128
AREA = (40, TOP + 14, W - 40, H - BOTTOM - 14)
MAX_PLAIN = 6.5  # 이보다 짧은 구간은 실제 속도
MAX_SHOWN = 5.5  # 긴 구간을 줄인 길이
MIN_SHOWN = 3.4  # 자막을 읽을 시간 (짧은 구간은 마지막 프레임에서 멈춤)
LEAD = 2.0  # 장면 표시 앞에서 함께 보여 줄 동작 (초)

BG = (16, 20, 27)
PANEL = (27, 32, 41)
PANEL2 = (37, 43, 54)
TEXT = (240, 243, 247)
MUTED = (158, 168, 182)
DARK = (14, 17, 22)
GREEN = (72, 187, 120)
BLUE = (88, 166, 255)
ORANGE = (240, 160, 70)
RED = (240, 96, 96)
TEAL = (64, 196, 180)
PURPLE = (199, 146, 234)
YELLOW = (238, 204, 90)
GRAY = (120, 128, 140)

REG = "C:/Windows/Fonts/malgun.ttf"
BOLD = "C:/Windows/Fonts/malgunbd.ttf"
MONO = "C:/Windows/Fonts/consola.ttf"
_FONTS = {}


def font(size, bold=False, mono=False):
    key = (size, bold, mono)
    if key not in _FONTS:
        _FONTS[key] = ImageFont.truetype(MONO if mono else (BOLD if bold else REG), size)
    return _FONTS[key]


LANG = "ko"


def L(ko, en):
    return en if LANG == "en" else ko


# ───────── 번역 (영어 영상: app/src/i18n/en.json, translate.ts 와 같은 순서) ─────────


class Translator:
    def __init__(self, path):
        t = json.load(open(path, encoding="utf-8"))
        self.exact = t["exact"]
        self.spans = t.get("spans", {})
        self.phrases = {**t.get("names", {}), **t["phrases"]}
        js = lambda r: re.sub(r"\$(\$|&|\d+)", lambda m: "$" if m.group(1) == "$" else "\\g<0>" if m.group(1) == "&" else f"\\g<{m.group(1)}>", r.replace("\\", "\\\\"))
        self.pats = []
        for p, r in t["patterns"]:
            try:
                self.pats.append((re.compile(p), js(r)))
            except re.error:
                pass
        mk = lambda d: re.compile("|".join(re.escape(k) for k in sorted(d, key=len, reverse=True))) if d else None
        self.span_re, self.phrase_re = mk(self.spans), mk(self.phrases)
        self.memo = {}

    def line(self, s):
        core = s.strip()
        if core in self.exact:
            return s.replace(core, self.exact[core], 1)
        out = self.span_re.sub(lambda m: self.spans[m.group(0)], s) if self.span_re else s
        for p, r in self.pats:
            out = p.sub(r, out)
        if out.strip() in self.exact:
            return self.exact[out.strip()]
        if self.phrase_re:
            out = self.phrase_re.sub(lambda m: self.phrases[m.group(0)], out)
        return re.sub(" {2,}", " ", out)

    def __call__(self, s):
        if LANG != "en" or not isinstance(s, str) or not re.search("[가-힣]", s):
            return s
        if s not in self.memo:
            self.memo[s] = "\n".join(self.line(x) if re.search("[가-힣]", x) else x for x in s.split("\n"))
        return self.memo[s]


TR = None

# ───────── 글 그리기 ─────────


def wrap(d, text, f, width):
    lines = []
    for para in str(text).split("\n"):
        words = re.split(r"(\s+)", para)
        cur = ""
        for w in words:
            trial = cur + w
            if d.textlength(trial, font=f) <= width or not cur:
                cur = trial
                # 한국어처럼 띄어 쓰지 않은 긴 낱말은 글자 단위로 자릅니다
                while d.textlength(cur, font=f) > width and len(cur) > 1:
                    cut = len(cur)
                    while cut > 1 and d.textlength(cur[:cut], font=f) > width:
                        cut -= 1
                    lines.append(cur[:cut])
                    cur = cur[cut:]
            else:
                lines.append(cur.rstrip())
                cur = w.lstrip()
        lines.append(cur.rstrip())
    return lines


GLYPH = {"✓": "√", "✗": "×", "✕": "×", "⏱️": "", "⏱": "", "▸": "›", "▾": "", "▴": "", "⤢": ""}
EMOJI = re.compile("[\U0001F000-\U0001FAFF\u2600-\u26FF\u2700-\u27BF\uFE0F]")


def clean(text):
    """맑은 고딕에 없는 글자는 비슷한 글자로, 그림 문자는 뺍니다"""
    text = str(text)
    for k, v in GLYPH.items():
        text = text.replace(k, v)
    return re.sub(r" {2,}", " ", EMOJI.sub("", text)).strip()


def block(d, x, y, text, f, fill, width, gap=8, max_lines=None):
    lines = wrap(d, clean(text), f, width)
    if max_lines and len(lines) > max_lines:
        lines = lines[:max_lines]
        lines[-1] = lines[-1][: max(0, len(lines[-1]) - 2)] + " …"
    for line in lines:
        d.text((x, y), line, font=f, fill=fill)
        y += f.size + gap
    return y


def centered(d, cx, y, text, f, fill):
    d.text((cx - d.textlength(text, font=f) / 2, y), text, font=f, fill=fill)


def bar(d, x, y, width, value, color, height=20):
    d.rounded_rectangle((x, y, x + width, y + height), 5, fill=PANEL2)
    if value > 0:
        d.rounded_rectangle((x, y, x + max(8, width * min(1.0, value)), y + height), 5, fill=color)


def bullets(d, x, y, items, width, size=24, fill=TEXT, gap=6):
    for it in items:
        d.text((x, y), "•", font=font(size), fill=MUTED)
        y = block(d, x + 24, y, it, font(size), fill, width - 24, gap=gap) + 8
    return y


def fit(img, w, h):
    r = min(w / img.width, h / img.height)
    return img.resize((max(1, int(img.width * r)), max(1, int(img.height * r))), Image.LANCZOS if r < 1 else Image.BICUBIC)


# ───────── 녹화 · 기록 ─────────


class Screen:
    def __init__(self, folder, name, frames):
        self.dir = os.path.join(folder, "frames", name)
        frames = sorted(frames, key=lambda f: f["t"])
        self.times = [f["t"] for f in frames]
        self.files = [f["file"] for f in frames]
        self.cache = {}

    def at(self, t):
        if not self.times:
            return None
        i = max(0, bisect.bisect_right(self.times, t) - 1)
        if i not in self.cache:
            if len(self.cache) > 24:
                self.cache.pop(next(iter(self.cache)))
            self.cache[i] = Image.open(os.path.join(self.dir, self.files[i])).convert("RGB")
        return self.cache[i]


class Rec:
    def __init__(self, folder):
        self.dir = folder
        tl = json.load(open(os.path.join(folder, "timeline.json"), encoding="utf-8"))
        self.events = tl["events"]
        self.screens = {k: Screen(folder, k, v) for k, v in tl["frames"].items()}
        self.marks = {}
        self.order = []
        for e in self.events:
            if e["type"] == "mark":
                self.order.append(e)
                self.marks.setdefault(e["id"], e)
        self.srcs = [(e["t"], e["src"]) for e in self.events if e["type"] == "src"]
        self.clicks = [e for e in self.events if e["type"] == "click"]
        scenes = [e for e in self.events if e["type"] == "scene"]
        end = next((e["t"] for e in self.events if e["type"] == "end"), self.events[-1]["t"])
        self.scene_span = {}
        for i, e in enumerate(scenes):
            self.scene_span[e["id"]] = (e["t"], scenes[i + 1]["t"] if i + 1 < len(scenes) else end)
        self.failed = set()
        for f in (e for e in self.events if e["type"] == "fail"):
            for sid, (a, b) in self.scene_span.items():
                if a <= f["t"] <= b + 1:
                    self.failed.add(sid)

    def snap(self, name):
        p = os.path.join(self.dir, "snaps", f"{name}.json")
        return json.load(open(p, encoding="utf-8")) if os.path.exists(p) else None

    def src_at(self, t):
        cur = "c"
        for tt, s in self.srcs:
            if tt <= t:
                cur = s
            else:
                break
        return cur

    def next_mark_t(self, mark):
        i = self.order.index(mark)
        return self.order[i + 1]["t"] if i + 1 < len(self.order) else self.scene_span.get(mark["scene"], (0, mark["t"] + 6000))[1]

    def prev_mark_t(self, mark):
        i = self.order.index(mark)
        return self.order[i - 1]["t"] if i > 0 else self.scene_span.get(mark["scene"], (mark["t"] - 2000, 0))[0]


class Trace:
    STEP_KEY = {("siglip2", "패치 특징"): "siglip2", ("siglip2", "심볼 그림 사전"): "siglip2", ("gocr", "비전"): "gocr_v", ("gocr", "언어"): "gocr_l", ("zoro", "판단"): "zoro_j", ("zoro", "글쓰기"): "zoro_w"}

    def __init__(self, paths):
        seen = set()
        self.events = []
        for p in paths:
            if not os.path.exists(p):
                continue
            for line in open(p, encoding="utf-8"):
                try:
                    e = json.loads(line)
                except json.JSONDecodeError:
                    continue
                key = (e["t"], e["model"], e["step"])
                if key in seen:
                    continue
                seen.add(key)
                e["end"] = e["t"]
                e["start"] = e["t"] - e["ms"]
                e["key"] = self.STEP_KEY.get((e["model"], e["step"]))
                self.events.append(e)
        self.events.sort(key=lambda e: e["end"])
        self.ends = [e["end"] for e in self.events]
        self.work = [e for e in self.events if e["key"]]
        self.moves = [e for e in self.events if e["step"] in ("올림", "내림")]

    def active(self, t):
        return {e["key"] for e in self.work if e["start"] <= t <= e["end"] + 250}

    def resident(self, t):
        now = {}
        for e in self.moves:
            if e["end"] > t:
                break
            key = (e["model"], e["detail"].get("group", ""))
            if e["step"] == "올림":
                now[key] = e["detail"].get("bytes", 0)
            else:
                now.pop(key, None)
        return now

    def between(self, a, b, model=None, step=None):
        i = bisect.bisect_left(self.ends, a)
        out = []
        while i < len(self.events) and self.events[i]["end"] <= b:
            e = self.events[i]
            if (model is None or e["model"] == model) and (step is None or e["step"] == step):
                out.append(e)
            i += 1
        return out


# ───────── 바탕 (윗줄 · 아랫줄) ─────────

def chips():
    return [("siglip2", "SigLIP2", BLUE), ("gocr_v", L("gocr 비전", "gocr vision"), TEAL), ("gocr_l", L("gocr 언어", "gocr language"), GREEN), ("zoro_j", L("Zoro-1 판단", "Zoro-1 verdict"), ORANGE), ("zoro_w", L("Zoro-1 글쓰기", "Zoro-1 writer"), PURPLE)]


def frame_base(part, where, label, note, active=(), resident=None):
    canvas = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(canvas)
    d.rectangle((0, 0, W, TOP), fill=PANEL)
    if part:
        tw = d.textlength(part, font=font(20, True)) + 28
        d.rounded_rectangle((18, 12, 18 + tw, 44), 8, fill=BLUE)
        d.text((32, 14), part, font=font(20, True), fill=DARK)
        d.text((18 + tw + 16, 13), where, font=font(22, True), fill=TEXT)
    x = W - 18
    for key, name, color in reversed(chips()):
        tw = d.textlength(name, font=font(17, True)) + 22
        on = key in active
        d.rounded_rectangle((x - tw, 13, x, 43), 8, fill=color if on else None, outline=color, width=2)
        d.text((x - tw + 11, 16), name, font=font(17, True), fill=DARK if on else color)
        x -= tw + 8
    d.rectangle((0, H - BOTTOM, W, H), fill=PANEL)
    y = H - BOTTOM + 12
    y = block(d, 36, y, label, font(31, True), TEXT, W - 72, gap=4, max_lines=1)
    if note:
        block(d, 36, y + 2, note, font(23), MUTED, W - 72, gap=4, max_lines=2)
    if resident:
        held = " · ".join(sorted({f"{m}" for m, _ in resident}))
        s = L(f"GPU 상주: {held}", f"On GPU: {held}")
        d.text((W - 24 - d.textlength(s, font=font(16)), H - 26), s, font=font(16), fill=GRAY)
    tag = L("합성 데이터 · 실제 앱 · 실제 모델 (ROCm RX 6600)", "Synthetic data · real app · real models (ROCm RX 6600)")
    d.text((W - 24 - d.textlength(tag, font=font(16)), H - BOTTOM + 8), tag, font=font(16), fill=GRAY)
    return canvas, d


# ───────── 구간 ─────────


class Seg:
    """한 구간: 길이(초)와 프레임 그리기 함수"""

    def __init__(self, shown, draw, name):
        self.shown = shown
        self.draw = draw
        self.name = name


def card_seg(name, shown, painter):
    img = painter()
    return Seg(shown, lambda k, n: img, name)


def footage_seg(rec, trace, t0, t1, part, where, label, note, name, src=None, real_speed=False):
    real = max(0.2, (t1 - t0) / 1000)
    if real_speed or real <= MAX_PLAIN:
        speed = 1.0
    else:
        speed = real / MAX_SHOWN
    shown = max(real / speed, MIN_SHOWN)
    base_cache = {}

    def draw(k, n):
        p = k / max(1, n - 1)
        t = t0 + min(real, p * shown * speed) * 1000
        s = src or rec.src_at(t)
        screen = rec.screens.get(s) or rec.screens["c"]
        img = screen.at(t) or rec.screens["c"].at(t)
        act = frozenset(trace.active(t)) if trace else frozenset()
        res = tuple(sorted(trace.resident(t).items())) if trace else ()
        key = (act, res)
        if key not in base_cache:
            if len(base_cache) > 6:
                base_cache.clear()
            c, _ = frame_base(part, where if s == "c" else L("현장 단말", "Field terminal"), label, note, act, dict(res))
            base_cache[key] = c
        canvas = base_cache[key].copy()
        if img is not None:
            aw, ah = AREA[2] - AREA[0], AREA[3] - AREA[1] + 12
            shown_img = fit(img, aw, ah)
            x = AREA[0] + (aw - shown_img.width) // 2
            y = AREA[1] - 6 + (ah - shown_img.height) // 2
            canvas.paste(shown_img, (x, y))
            d = ImageDraw.Draw(canvas)
            d.rectangle((x - 1, y - 1, x + shown_img.width, y + shown_img.height), outline=(60, 68, 80))
            # 클릭 물결 (캐스트에는 마우스가 안 보입니다)
            r = shown_img.width / img.width
            for c in rec.clicks:
                age = (t - c["t"]) / 1000
                if c["page"] == s and 0 <= age <= 0.8:
                    cx, cy = x + c["x"] * r, y + c["y"] * r
                    rad = 10 + 34 * age
                    a = int(255 * (1 - age / 0.8))
                    d.ellipse((cx - rad, cy - rad, cx + rad, cy + rad), outline=(255, 214, 90), width=4)
                    d.ellipse((cx - 6, cy - 6, cx + 6, cy + 6), fill=(255, 214, 90))
        return canvas

    return Seg(shown, draw, name)


def model_seg(shown, part, label, note, painter, name, active=()):
    base, _ = frame_base(part, L("모델 동작 (실제 기록)", "Model at work (real trace)"), label, note, set(active))

    def draw(k, n):
        p = k / max(1, n - 1)
        canvas = base.copy()
        d = ImageDraw.Draw(canvas)
        d.rounded_rectangle(AREA, 12, fill=PANEL)
        painter(canvas, d, p)
        return canvas

    return Seg(shown, draw, name)


# ───────── 모델 장면 ─────────

KIND_COLOR = {"제목": BLUE, "입력란": GREEN, "표": PURPLE, "서명": ORANGE}
KIND_NAME = {"제목": ("Title", "제목 (표제란)"), "입력란": ("Form", "입력란 · 수기"), "표": ("Table", "일람표"), "서명": ("Sign", "서명 · 직인")}


def deskew(img, deg):
    """core/src/ai/siglip2/ink.rs::deskew 와 같은 회전 (가장 가까운 화소)"""
    if not deg:
        return img
    a = np.asarray(img)
    h, w = a.shape[:2]
    s, c = math.sin(math.radians(deg)), math.cos(math.radians(deg))
    cx, cy = w / 2, h / 2
    ys, xs = np.mgrid[0:h, 0:w].astype(np.float32)
    dx, dy = xs + 0.5 - cx, ys + 0.5 - cy
    sx = np.rint(cx + dx * c - dy * s - 0.5).astype(np.int64)
    sy = np.rint(cy + dx * s + dy * c - 0.5).astype(np.int64)
    ok = (sx >= 0) & (sy >= 0) & (sx < w) & (sy < h)
    out = np.full_like(a, 255)
    out[ok] = a[sy[ok], sx[ok]]
    return Image.fromarray(out)


def model_head(d, name, role, color):
    x, y = AREA[0] + 22, AREA[1] + 16
    tw = d.textlength(name, font=font(28, True)) + 30
    d.rounded_rectangle((x, y, x + tw, y + 46), 10, fill=color)
    d.text((x + 15, y + 5), name, font=font(28, True), fill=DARK)
    d.text((x + tw + 18, y + 8), role, font=font(26), fill=TEXT)


def crop_plan_painter(scan_png, plan_event, read_event, symbol_event, timing, title_note):
    page = plan_event["detail"]["pages"][0]
    img = deskew(Image.open(scan_png).convert("RGB"), page.get("skew", 0.0))
    ph = AREA[3] - AREA[1] - 110
    r = ph / img.height
    small = img.resize((int(img.width * r), ph), Image.LANCZOS)
    crops = page["crops"]
    dropped = page.get("dropped", [])
    blank = page.get("blank", [])
    delta = page.get("delta") or {}
    texts = [t["text"] for t in (read_event or {}).get("detail", {}).get("texts", [])] if read_event else []

    def paint(canvas, d, p):
        model_head(d, "SigLIP2 → gocr", L("어디를 읽을지 고르고 (영역 경쟁), 고른 조각만 읽습니다", "Choose where to read (region arena), then read only those crops"), BLUE)
        x0, y0 = AREA[0] + 24, AREA[1] + 82
        layer = Image.new("RGBA", small.size, (0, 0, 0, 0))
        ld = ImageDraw.Draw(layer)
        # 1) 패치 격자 (SigLIP2 비전 타워): 16 px 패치를 조각 점수로
        g = max(0.0, 1 - abs(p - 0.18) / 0.18)
        if g > 0:
            step = max(6, int(16 * r * 2.5))
            for xx in range(0, small.width, step):
                ld.line((xx, 0, xx, small.height), fill=(60, 120, 255, int(70 * g)))
            for yy in range(0, small.height, step):
                ld.line((0, yy, small.width, yy), fill=(60, 120, 255, int(70 * g)))
        # 2) 델타: 마스터와 같은 조각은 건너뜀, 바뀐 칸은 노랑
        if delta and p > 0.25:
            for b in delta.get("skipped_boxes", []):
                bx = [v * r for v in b]
                ld.rectangle(bx, fill=(120, 128, 140, 120), outline=(150, 156, 166, 255), width=2)
            for b in delta.get("changed_boxes", [])[:600]:
                bx = [v * r for v in b]
                ld.rectangle(bx, fill=(255, 214, 90, 90))
        # 3) 버림: 안내문(dropped) 회색 점선 + X, 글자 없는 띠(blank) 붉은 취소선
        if p > 0.32:
            for b in dropped:
                bx = [v * r for v in b]
                ld.rectangle(bx, outline=(170, 176, 186, 255), width=2)
                ld.line((bx[0], bx[1], bx[2], bx[3]), fill=(170, 176, 186, 220), width=2)
                ld.line((bx[0], bx[3], bx[2], bx[1]), fill=(170, 176, 186, 220), width=2)
            for b in blank:
                bx = [v * r for v in b]
                ld.rectangle(bx, outline=(240, 96, 96, 200), width=1)
                ld.line((bx[0], (bx[1] + bx[3]) / 2, bx[2], (bx[1] + bx[3]) / 2), fill=(240, 96, 96, 230), width=2)
        # 4) 남은 조각: 제목 → 입력란 → 표 → 서명 차례로
        order = ["제목", "입력란", "표", "서명"]
        shown_kinds = order[: max(0, min(4, int((p - 0.45) / 0.1) + 1))] if p > 0.45 else []
        for c in crops:
            if c["kind"] in shown_kinds:
                col = KIND_COLOR.get(c["kind"], MUTED)
                bx = [v * r for v in c["bbox"]]
                ld.rectangle(bx, fill=(*col, 50), outline=(*col, 255), width=3)
        canvas.paste(Image.alpha_composite(small.convert("RGBA"), layer).convert("RGB"), (x0, y0))
        d.rectangle((x0 - 1, y0 - 1, x0 + small.width, y0 + small.height), outline=(90, 100, 112))
        tx = x0 + small.width + 34
        width = AREA[2] - tx - 24
        y = y0
        for k in order:
            col = KIND_COLOR[k]
            n = sum(1 for c in crops if c["kind"] == k)
            d.rectangle((tx, y + 6, tx + 22, y + 26), outline=col, width=3)
            name = L(KIND_NAME[k][1], KIND_NAME[k][0])
            d.text((tx + 32, y), f"{name}  {n}", font=font(23, True), fill=col if k in shown_kinds or p > 0.9 else MUTED)
            y += 34
        d.rectangle((tx, y + 6, tx + 22, y + 26), outline=(170, 176, 186), width=2)
        d.text((tx + 32, y), L(f"안내문 버림 (dropped)  {len(dropped)}", f"Guide dropped  {len(dropped)}"), font=font(23), fill=TEXT if p > 0.32 else MUTED)
        y += 34
        d.line((tx, y + 16, tx + 22, y + 16), fill=RED, width=3)
        d.text((tx + 32, y), L(f"글자 없는 띠 (blank, MIN_GLYPHS 미만)  {len(blank)}", f"Blank band (< MIN_GLYPHS)  {len(blank)}"), font=font(23), fill=TEXT if p > 0.32 else MUTED)
        y += 44
        if delta:
            y = bullets(d, tx, y, [
                L(f"마스터 식별: {delta.get('master')} (SigLIP2 패치 코사인 {delta.get('similarity', 0):.3f}, 2위와 차이 {delta.get('margin', 0):.3f})", f"Master: {delta.get('master')} (SigLIP2 patch cosine {delta.get('similarity', 0):.3f}, margin {delta.get('margin', 0):.3f})"),
                L(f"잉크가 바뀐 칸 {delta.get('changed')}/{delta.get('blocks')} · 같은 조각 {delta.get('skipped')}개 건너뜀 · 바뀐 곳 둘레로 좁힘 {delta.get('clipped')} · 안내문 자리 잘라 냄 {delta.get('trimmed', 0)}", f"Changed ink cells {delta.get('changed')}/{delta.get('blocks')} · {delta.get('skipped')} unchanged crops skipped · {delta.get('clipped')} clipped to the change · {delta.get('trimmed', 0)} guide zones trimmed"),
            ], width, size=21)
        y = bullets(d, tx, y, timing, width, size=21)
        if texts and p > 0.75:
            y += 6
            d.text((tx, y), L("gocr 가 읽은 글 (일부)", "What gocr read (excerpt)"), font=font(21, True), fill=GREEN)
            y += 32
            for t in texts[:5]:
                t1 = re.sub(r"<[^>]+>", " ", t).replace("\n", " ")
                t1 = re.sub(r"\s+", " ", t1).strip()
                y = block(d, tx, y, "· " + t1, font(19), TEXT, width, gap=3, max_lines=2) + 4
                if y > AREA[3] - 40:
                    break
        if title_note:
            d.text((x0, AREA[3] - 30), title_note, font=font(18), fill=GRAY)

    return paint


def delta_compare_painter(rows, run_note):
    """전처리 Off · On 판독 시간 막대 (delta_probe 실측 파일)"""

    def paint(canvas, d, p):
        model_head(d, L("SDS 델타 패치", "SDS delta patch"), L("마스터와 같은 곳은 건너뛰고 바뀐 붉은 잉크 둘레만 읽습니다", "Skip what matches the master; read only around the changed red ink"), BLUE)
        x0, y0 = AREA[0] + 40, AREA[1] + 96
        label_w = 520
        width = AREA[2] - x0 - label_w - 160
        top = max(r[2] for r in rows)
        y = y0
        for name, sub, secs, color in rows:
            d.text((x0, y), name, font=font(25, True), fill=TEXT)
            d.text((x0, y + 34), sub, font=font(19), fill=MUTED)
            v = secs / top * min(1.0, p * 1.6)
            bar(d, x0 + label_w, y + 14, width, v, color, height=34)
            d.text((x0 + label_w + width + 16, y + 14), f"{secs:.1f} s", font=font(27, True), fill=color)
            y += 92
        block(d, x0, y + 10, run_note, font(22), MUTED, AREA[2] - x0 - 40)

    return paint


def symbols_painter(drawing_id, probe_data):
    """symbol_probe 실측 (마스터 도면의 기호마다 1위 종류 · 유사도 · 받아들임 여부). 상자는 도면 JSON 의 심볼 좌표"""
    sheet = json.load(open(os.path.join(ROOT, "core", "fixtures", "drawings", f"{drawing_id}.json"), encoding="utf-8"))
    img = Image.open(os.path.join(ROOT, "core", "fixtures", "drawings", f"{drawing_id}.png")).convert("RGB")
    master = next(m for m in probe_data["masters"] if m["drawing"] == drawing_id)
    meas = {x["kks"]: x for x in master["symbols"]}
    boxes = [(sym, meas.get(sym["kks"])) for sym in sheet["symbols"]]
    ph = AREA[3] - AREA[1] - 110
    r = min(ph / img.height, 1080 / img.width)
    small = img.resize((int(img.width * r), int(img.height * r)), Image.LANCZOS)
    names = {"pump": L("펌프", "pump"), "valve": L("밸브", "valve"), "exchanger": L("열교환기", "heat exchanger"), "fan": L("팬", "fan"), "vessel": L("용기", "vessel"), "breaker": L("차단기", "breaker"), "transformer": L("변압기", "transformer"), "generator": L("발전기", "generator"), "panel": L("전원반", "panel"), "turbine": L("터빈", "turbine"), "none": L("없음", "none")}
    chosen, test = probe_data["chosen"], probe_data["test"]

    def paint(canvas, d, p):
        model_head(d, L("SigLIP2 그림 사전", "SigLIP2 symbol dictionary"), L("글자가 없어도 기호 모양으로 설비 종류를 맞힙니다", "Identifies equipment by symbol shape, even without text"), BLUE)
        x0, y0 = AREA[0] + 24, AREA[1] + 82
        canvas.paste(small, (x0, y0))
        n = int(len(boxes) * min(1.0, p * 1.5))
        for sym, m in boxes[:n]:
            b = sym["box"]
            bx = [x0 + b[0] * r, y0 + b[1] * r, x0 + b[2] * r, y0 + b[3] * r]
            ok = m and m.get("got")
            col = BLUE if ok else ORANGE
            d.rectangle((bx[0] - 4, bx[1] - 4, bx[2] + 4, bx[3] + 4), outline=col, width=3)
            label = f"{names.get(m['nearest'], m['nearest'])} {m['sim']:.3f}" if m else "?"
            if m and not m.get("got"):
                label += L(" 보류", " abstain")
            tw = d.textlength(label, font=font(15, True)) + 8
            d.rectangle((bx[0] - 4, bx[1] - 26, bx[0] - 4 + tw, bx[1] - 6), fill=col)
            d.text((bx[0], bx[1] - 25), label, font=font(15, True), fill=DARK)
        tx = x0 + small.width + 34
        width = AREA[2] - tx - 24
        y = bullets(d, tx, y0, [
            L("검은 잉크 연결 요소에서 60 px 안팎 기호 후보를 찾습니다 (규칙)", "Rules find ~60 px symbol candidates among black-ink components"),
            L("SigLIP2 가 후보마다 64 패치 평균 벡터를 만들고 그림 사전(10종 원형 + none)과 코사인으로 댑니다", "SigLIP2 embeds each candidate (mean of 64 patches) and compares it with the dictionary (10 prototypes + none) by cosine"),
            L(f"받아들이는 기준: 유사도 ≥ {chosen['min_sim']:.3f} · 2위와 차이 ≥ {chosen['min_margin']:.3f}. 못 미치면 보류(주황)", f"Accepted when similarity ≥ {chosen['min_sim']:.3f} and margin ≥ {chosen['min_margin']:.3f}; otherwise it abstains (orange)"),
        ], width, size=21)
        y += 6
        tot = probe_data["master_totals"]
        y = block(d, tx, y, L(f"마스터 도면 10장: 기호 {tot['right']}/{tot['symbols']} 맞힘 · 잘못 받아들임 {tot['false_accepted']}", f"10 master drawings: {tot['right']}/{tot['symbols']} symbols right · {tot['false_accepted']} false accepts"), font(22, True), BLUE, width) + 6
        y = block(d, tx, y, L(f"시험 표본: 받아들여 맞힘 {test['accepted_right']} · 틀림 {test['accepted_wrong']} · 보류 {test['abstain']}", f"Test samples: {test['accepted_right']} accepted right · {test['accepted_wrong']} wrong · {test['abstain']} abstained"), font(21), TEXT, width) + 10
        for sym, m in boxes[:n]:
            if not m:
                continue
            s = f"{sym['kks']}  {names.get(m['nearest'], m['nearest'])}  {m['sim']:.3f}" + ("" if m.get("got") else L("  (마진 미달 → 보류)", "  (margin too small → abstain)"))
            d.text((tx, y), s, font=font(20), fill=TEXT if m.get("got") else ORANGE)
            y += 28
        d.text((x0, AREA[3] - 30), L(f"출처: out/symbol_probe.json ({probe_data['at'][:10]}, ROCm) · 측정한 한국어 마스터 도면 {drawing_id}", f"Source: out/symbol_probe.json ({probe_data['at'][:10]}, ROCm) · the Korean master drawing {drawing_id} that was measured"), font=font(18), fill=GRAY)

    return paint


def verdict_label(code, catalog):
    lab = catalog.get(code, code)
    return TR(lab) if TR else lab


def decision_painter(detail, catalog, rule_code, note):
    asked = detail["analysis"]["substantive"]["decided"]["asked"]
    sds = detail["analysis"]["sds"]
    trace = detail["analysis"].get("model_trace", [])
    judge_ms = [e["ms"] for e in trace if e["model"] == "zoro" and e["step"] == "판단"]

    def short(title):
        t = TR(title) if TR else title
        if " — judge only: " in t:
            return "[" + L("SDS 축", "SDS axis") + "] " + t.split(" — judge only: ", 1)[1]
        m = re.match(r".+ 중 (.+) 만 판단$", t)
        if m:
            return "[SDS 축] " + m.group(1)
        return L("질문 1 (전체)", "Question 1 (whole case)") + ": " + t

    def paint(canvas, d, p):
        model_head(d, "Zoro-1 " + L("판단", "verdict"), L("글을 쓰지 않고 선택지를 가리킵니다 (0-Generation 포인터 · jev_head)", "Points at an option instead of generating text (0-generation pointer · jev_head)"), ORANGE)
        x0, y0 = AREA[0] + 24, AREA[1] + 80
        left = 640
        d.text((x0, y0), L("모델에 들어간 사실 (PointerPrompt · 근무자 정보 없음)", "Facts given to the model (PointerPrompt · no worker data)"), font=font(22, True), fill=TEXT)
        y = y0 + 38
        for key, value in asked[0]["facts"]:
            d.text((x0, y), TR(key) if TR else key, font=font(19, True), fill=MUTED)
            y = block(d, x0, y + 26, TR(value) if TR else value, font(19), TEXT, left, gap=3, max_lines=3) + 8
            if y > AREA[3] - 150:
                break
        if LANG == "en":
            d.text((x0, AREA[3] - 112), "(translated for display; the model reads the Korean originals)", font=font(16), fill=GRAY)
        y = max(y, AREA[3] - 86)
        if judge_ms:
            d.text((x0, AREA[3] - 82), L(f"질문 {len(judge_ms)}개 · 질문당 {sum(judge_ms) / len(judge_ms) / 1000:.2f} s (GPU 실행 기록)", f"{len(judge_ms)} questions · {sum(judge_ms) / len(judge_ms) / 1000:.2f} s each (GPU trace)"), font=font(20, True), fill=ORANGE)
        tx = x0 + left + 40
        width = AREA[2] - tx - 24
        n = len(asked)
        upto = min(n, int(p * n * 1.25) + 1)
        yy = y0
        for q in asked[:upto]:
            d.text((tx, yy), short(q["title"]), font=font(21, True), fill=TEXT)
            yy += 32
            pairs = sorted(zip(q["options"], q["probabilities"]), key=lambda a: -a[1])[:2]
            for j, (code, prob) in enumerate(pairs):
                d.text((tx, yy - 2), verdict_label(code, catalog)[:34], font=font(18), fill=MUTED)
                bar(d, tx + 330, yy, width - 420, prob, ORANGE if code == rule_code else (120, 130, 142), height=18)
                d.text((tx + width - 80, yy - 3), f"{prob:.3f}", font=font(19, True), fill=TEXT)
                yy += 28
            yy += 10
        if upto == n:
            mn = sds.get("min_confidence")
            axes = sds.get("decomposed_confidences", [])
            if axes and mn is not None:
                parts = " · ".join(f"{(TR(a) if TR else a)} {v:.3f}" for a, v in axes)
                msg = L(f"축별 최솟값 = 최종 확신도 {mn:.3f}  ({parts})", f"Minimum over axes = final confidence {mn:.3f}  ({parts})")
                col = GREEN if mn >= 0.95 else ORANGE
            else:
                msg = L("모델 이견: 축 확률이 규칙 판정과 엇갈려 확신도 없음 → 규칙 판정 유지, 교대감독 확정", "Model dissent: axis probabilities disagree with the rule verdict → no confidence; the rule verdict stands and the shift supervisor confirms")
                col = RED
            yy = max(yy + 4, AREA[3] - 132)
            d.rounded_rectangle((tx - 10, yy - 8, AREA[2] - 20, AREA[3] - 14), 10, fill=PANEL2)
            yy = block(d, tx, yy, msg, font(21, True), col, width - 20, gap=4, max_lines=2)
            block(d, tx, yy + 2, note, font(18), MUTED, width - 20, gap=3, max_lines=2)

    return paint


def writer_painter(draft, write_event, system_note):
    summary = draft.get("summary", "")
    numbers = draft.get("numbers", [])
    problems = draft.get("problems", [])

    def paint(canvas, d, p):
        model_head(d, "Zoro-1 " + L("글쓰기", "writer"), L("같은 몸통에 임베딩 표를 얹어 조치 절차서 원인 요약을 씁니다", "Same backbone + embedding table writes the procedure's root-cause summary"), PURPLE)
        x0, y0 = AREA[0] + 24, AREA[1] + 86
        width = AREA[2] - x0 - 24
        d.text((x0, y0), system_note, font=font(21), fill=MUTED)
        y = y0 + 40
        d.text((x0, y), L(f"{draft.get('aop_id', '')} 원인 진단 요약 — 모델이 쓴 글", f"{draft.get('aop_id', '')} root-cause summary — written by the model"), font=font(22, True), fill=TEXT)
        y += 38
        shown = summary[: int(len(summary) * min(1.0, p * 1.7))]
        d.rounded_rectangle((x0, y, x0 + width, y + 210), 10, fill=PANEL2)
        block(d, x0 + 20, y + 16, shown, font(26), TEXT, width - 40, gap=8, max_lines=6)
        y += 232
        if p > 0.55:
            d.text((x0, y), L("factcheck.rs: 초안의 숫자 · 코드를 사실 줄과 1:1 대조", "factcheck.rs: every number and code in the draft is checked 1:1 against the fact lines"), font=font(22, True), fill=TEXT)
            y += 38
            x = x0
            for n, ok in numbers:
                s = f"[{n}] {'√' if ok else '×'}"
                tw = d.textlength(s, font=font(22, True)) + 24
                if x + tw > x0 + width:
                    x = x0
                    y += 44
                d.rounded_rectangle((x, y, x + tw, y + 36), 8, fill=(30, 70, 45) if ok else (90, 30, 30))
                d.text((x + 12, y + 4), s, font=font(22, True), fill=GREEN if ok else RED)
                x += tw + 10
            y += 56
            good = sum(1 for _, ok in numbers if ok)
            verdict = L(f"숫자 {good}/{len(numbers)} 일치 · 재작성 {max(0, draft.get('attempts', 1) - 1)}회 · {'모델 글 채택' if draft.get('by_model') else '사실 줄 템플릿으로 대체 (G4_INTERCEPT_DOC_CORRUPTION)'}", f"{good}/{len(numbers)} figures match · {max(0, draft.get('attempts', 1) - 1)} rewrites · {'model text kept' if draft.get('by_model') else 'replaced with the fact-line template (G4_INTERCEPT_DOC_CORRUPTION)'}")
            d.text((x0, y), verdict, font=font(23, True), fill=GREEN if not problems else ORANGE)
            y += 42
            if write_event:
                det = write_event["detail"]
                per = write_event["ms"] / max(1, det.get("tokens", 1))
                d.text((x0, y), L(f"실측 {write_event['ms'] / 1000:.1f} s · 토큰 {det.get('tokens')}개 · 토큰당 {per:.0f} ms · 자기 추측 초안 {det.get('drafted', 0)} / 채택 {det.get('accepted', 0)}", f"Measured {write_event['ms'] / 1000:.1f} s · {det.get('tokens')} tokens · {per:.0f} ms/token · self-speculation drafted {det.get('drafted', 0)} / accepted {det.get('accepted', 0)}"), font=font(21), fill=PURPLE)

    return paint


def gantt_painter(trace, a, b, title):
    rows = [("siglip2", "SigLIP2", BLUE), ("gocr_v", L("gocr 비전", "gocr vision"), TEAL), ("gocr_l", L("gocr 언어", "gocr language"), GREEN), ("zoro_j", L("Zoro-1 판단", "Zoro-1 verdict"), ORANGE), ("zoro_w", L("Zoro-1 글쓰기", "Zoro-1 writer"), PURPLE)]
    evs = [e for e in trace.events if a <= e["end"] <= b]
    moves = [e for e in evs if e["step"] in ("올림", "내림")]

    def paint(canvas, d, p):
        x0, y0 = AREA[0] + 24, AREA[1] + 20
        d.text((x0, y0), title, font=font(32, True), fill=TEXT)
        d.text((x0, y0 + 48), L(f"녹화 구간 {(b - a) / 1000:.0f}초 (화면 대기 포함). GPU 에는 단계마다 필요한 가중치 묶음만 올립니다 (SharedResidency)", f"{(b - a) / 1000:.0f} s of the recording (including on-screen waits). Only the weight groups each step needs are on the GPU (SharedResidency)"), font=font(22), fill=MUTED)
        left = 250
        cx, cw = x0 + left, AREA[2] - x0 - left - 30
        y = y0 + 110
        rh = 86
        scale = cw / (b - a)
        upto = a + (b - a) * min(1.0, p * 1.3)
        for key, name, color in rows:
            d.text((x0, y + 8), name, font=font(24, True), fill=color)
            d.rectangle((cx, y + 4, cx + cw, y + rh - 16), fill=PANEL2)
            tot = 0.0
            for e in evs:
                if e["key"] != key or e["start"] > upto:
                    continue
                xa = cx + (max(e["start"], a) - a) * scale
                xb = cx + (min(e["end"], upto) - a) * scale
                d.rectangle((xa, y + 8, max(xa + 3, xb), y + rh - 20), fill=color)
                tot += e["ms"]
            if tot:
                d.text((x0, y + 40), f"{tot / 1000:.1f} s", font=font(20), fill=MUTED)
            y += rh
        d.text((x0, y + 6), L("가중치 올림 · 내림", "Weight load · unload"), font=font(22, True), fill=TEXT)
        y += 40
        for e in moves:
            if e["end"] > upto:
                break
            xx = cx + (e["end"] - a) * scale
            col = GREEN if e["step"] == "올림" else RED
            d.line((xx, y, xx, y + 26), fill=col, width=3)
        d.text((cx, y + 34), L("초록: 올림 · 빨강: 내림 (모델 · 묶음 단위)", "Green: load · red: unload (per model weight group)"), font=font(18), fill=MUTED)

    return paint


def criteria_painter(events, note):
    def paint(canvas, d, p):
        model_head(d, "Zoro-1 " + L("판단", "verdict"), L("자율 완화 조치를 네 기준으로 따로 묻고 최솟값을 씁니다 (PLANT_MULTI_CRITERIA)", "Asks each of four criteria separately for auto-mitigation and takes the minimum (PLANT_MULTI_CRITERIA)"), ORANGE)
        x0, y0 = AREA[0] + 40, AREA[1] + 96
        width = AREA[2] - x0 - 60
        y = y0
        mins = []
        n = len(events)
        for e in events[: min(n, int(p * n * 1.3) + 1)]:
            det = e["detail"]
            title = det["task"]
            t = TR(title) if TR else title
            if " — judge only: " in t:
                t = t.split(" — judge only: ", 1)[1]
            else:
                m = re.match(r".+ 중 (.+) 만 판단$", title)
                t = m.group(1) if m else t
            prob = det["probabilities"][0]
            if " 만 판단" in title:
                mins.append(prob)
            y = block(d, x0, y, t, font(22, True), TEXT, width - 200, gap=4, max_lines=2)
            bar(d, x0, y + 4, width - 200, prob, ORANGE, height=22)
            d.text((x0 + width - 180, y), f"{prob:.3f} · {e['ms'] / 1000:.2f} s", font=font(22, True), fill=TEXT)
            y += 44
        if p > 0.8 and mins:
            mn = min(mins)
            ok = mn >= 0.95
            msg = L(f"최솟값 {mn:.3f} {'≥' if ok else '<'} 0.95 → {'자동 실행' if ok else '보조 모드: 교대감독이 실행합니다'}", f"Minimum {mn:.3f} {'≥' if ok else '<'} 0.95 → {'auto-run' if ok else 'Assist Mode: the shift supervisor runs it'}")
            d.text((x0, y + 10), msg, font=font(26, True), fill=GREEN if ok else ORANGE)
            block(d, x0, y + 56, note, font(20), MUTED, width)

    return paint


# ───────── 카드 ─────────


def title_card(lines):
    canvas = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(canvas)
    d.rectangle((0, 0, W, 8), fill=BLUE)
    total = sum(s + 26 for _, s, _ in lines)
    y = (H - total) // 2
    for text, size, color in lines:
        centered(d, W / 2, y, text, font(size, size >= 40), color)
        y += size + 26
    return canvas


def divider_card(num, title, sub):
    canvas = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(canvas)
    d.rectangle((0, 0, W, 8), fill=BLUE)
    d.text((120, 330), num, font=font(46, True), fill=BLUE)
    y = block(d, 120, 400, title, font(78, True), TEXT, W - 240, gap=16)
    block(d, 120, y + 30, sub, font(32), MUTED, W - 240, gap=10)
    return canvas


def models_card():
    canvas = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(canvas)
    centered(d, W / 2, 96, L("모델이 쓰이는 자리", "Where the models work"), font(54, True), TEXT)
    centered(d, W / 2, 172, L("판정은 규칙이 정합니다. 모델은 고르고, 읽고, 확률을 매기고, 초안을 씁니다. GPU(RX 6600 8 GB)에는 단계마다 필요한 가중치만 올립니다", "Rules decide the verdict. Models choose, read, score and draft. Only the weights each step needs sit on the GPU (RX 6600 8 GB)"), font(25), MUTED)
    rows = [
        ("SigLIP2", BLUE, L("어디를 읽을지 고릅니다", "Chooses where to read"), L("제목 · 입력란 · 표 · 서명 조각, 안내문 버림, 마스터 델타 패치, 기호 그림 사전", "Title · form · table · sign crops, drops guides, master delta patches, symbol dictionary")),
        ("gocr " + L("비전 → 언어", "vision → language"), TEAL, L("고른 조각의 글자를 읽습니다", "Reads the chosen crops"), L("비전 가중치로 특징을 만든 뒤 내리고 언어 가중치로 글자를 씁니다 · 원장으로 KKS/DCIM 오독 교정", "Vision weights make features, then unload; language weights write text · ledger corrects KKS/DCIM misreads")),
        ("Zoro-1 " + L("판단", "verdict"), ORANGE, L("규칙의 판정을 기준별로 다시 묻습니다", "Re-asks the rule verdict per criterion"), L("근무자 정보 없는 사실만 보고 선택지 확률을 냅니다 (0-Generation 포인터) · 축별 최솟값 = 확신도", "Sees only de-identified facts and scores options (0-generation pointer) · minimum over axes = confidence")),
        ("Zoro-1 " + L("글쓰기", "writer"), PURPLE, L("조치 절차서 원인 요약 · 소명 분류를 씁니다", "Writes procedure summaries · sorts justifications"), L("숫자와 코드를 원문과 1:1 대조합니다. 어긋나면 다시 쓰고, 그래도 어긋나면 사실 줄 템플릿으로 바꿉니다", "Numbers and codes are checked 1:1 against the source; rewritten once, then replaced by the fact-line template")),
    ]
    y = 270
    for name, color, what, how in rows:
        d.rounded_rectangle((160, y, 1760, y + 150), 12, fill=PANEL)
        d.rounded_rectangle((184, y + 48, 184 + 360, y + 100), 10, fill=color)
        centered(d, 184 + 180, y + 55, name, font(26, True), DARK)
        d.text((580, y + 26), what, font=font(31, True), fill=TEXT)
        block(d, 580, y + 76, how, font(23), MUTED, 1150, gap=4, max_lines=2)
        y += 170
    return canvas


def summary_card(rows, stats, lines):
    canvas = Image.new("RGB", (W, H), BG)
    d = ImageDraw.Draw(canvas)
    d.rectangle((0, 0, W, 8), fill=BLUE)
    d.text((100, 60), L("이번 촬영 실행의 결과", "Results of this recorded run"), font=font(46, True), fill=TEXT)
    y = 140
    col = [100, 230, 980]
    d.text((col[0], y), L("시나리오", "Scenario"), font=font(22, True), fill=MUTED)
    d.text((col[1], y), L("사건 · 판정 (실행 기록)", "Incident · verdict (from the run)"), font=font(22, True), fill=MUTED)
    y += 40
    for sid, text, color in rows:
        d.text((col[0], y), sid, font=font(23, True), fill=TEXT)
        block(d, col[1], y, text, font(21), color, 820, gap=2, max_lines=1)
        y += 36
    x2 = 1010
    y2 = 140
    d.text((x2, y2), L("모델 사용 (trace.jsonl)", "Model use (trace.jsonl)"), font=font(22, True), fill=MUTED)
    y2 += 40
    for name, s in stats:
        d.text((x2, y2), name, font=font(23, True), fill=TEXT)
        d.text((x2 + 260, y2), s, font=font(21), fill=MUTED)
        y2 += 36
    y2 += 20
    for line in lines:
        y2 = block(d, x2, y2, line, font(21), TEXT, 820, gap=4) + 10
    return canvas


# ───────── 자막 ─────────


def top_hit(text):
    lines = [l for l in (text or "").split("\n") if l.strip()]
    if not lines:
        return "", "", ""
    first = re.sub(r"^\S*[🥇🥈🥉]\S*\s*", "", lines[0])
    m = re.search(r"((?:MO[CP]|AOP|EOP)-[\w-]+|[0-9A-Z]{8,})\s*$", first)
    code = m.group(1) if m else ""
    name = first[: m.start()].strip() if m else first
    why = next((l for l in lines if l.startswith("왜:") or l.lower().startswith("why:")), "")
    return name, code, why


def hits_of(toast):
    m = re.search(r"(\d+)\s*(?:건 적중|hits?)", toast or "")
    return m.group(1) if m else "?"


# (제목 한국어, 설명 한국어, 제목 영어, 설명 영어). {…} 는 장면 표시 값으로 채웁니다.
CAP = {
    "intro.console": ("1.2GW 화력발전소 관제 콘솔 — 풀스크린 P&ID 무대", "도면이 화면 전체에 깔리고 결재 HUD · 사건 대기열 · 하단 허브(파이프라인 · 타임머신 · 질의)가 유리 판으로 떠 있습니다",
                      "1.2GW power plant control console — full-screen P&ID stage", "The drawing fills the screen; approval HUD, incident queue and the bottom hub (pipeline · time machine · query) float as glass panels"),
    "intro.mermaid": ("② Mermaid 계통 로직 뷰", "같은 원장에서 그린 계통 그래프입니다 (도면 · Mermaid · 마인드맵이 같은 설비 선택을 공유)",
                      "② Mermaid system-logic view", "A system graph drawn from the same ledger (drawing · Mermaid · mind map share the selected equipment)"),
    "intro.mindmap": ("③ 마인드맵 — 홉 Depth 인과 그래프", "선택 설비를 가운데 두고 상류 · 하류 홉 깊이로 펼칩니다",
                      "③ Mind map — hop-depth causal graph", "Centers the selected equipment and fans out upstream/downstream by hop depth"),
    "intro.a0": ("A0 종합 계통도 (7016 × 4961 px)", "마스터 계통도 5장을 원장 배치대로 한 장에 앉힌 종합도 · 4 × 4 타일 격자",
                 "A0 composite (7016 × 4961 px)", "The five master diagrams laid out on one sheet from the ledger · 4 × 4 tile grid"),
    "intro.a0tile": ("타일 확대", "타일을 누르면 그 칸을 크게 봅니다 (판독도 타일 단위로 합니다)", "Tile zoom", "Click a tile to enlarge it (reading also works per tile)"),
    "intro.kiosk": ("현장 순시 · 정비 단말 (두 번째 창)", "현장 작업자가 계기 점검표 · Red-line 도면을 스캔해 보내는 단말입니다",
                    "Field patrol & maintenance terminal (second window)", "Field workers scan gauge sheets and Red-line drawings here"),
    "intro.aidc": ("도메인 전환 → 100MW AI 데이터센터 ({sec}초)", "같은 5-Gate 엔진 · 같은 모델이 원장 · 용어(DCIM 자산코드 · MOP · EOP · NOC) · 도면(MEP · SLD · NET)만 바꿔 관제합니다",
                   "Domain switch → 100MW AI data center ({sec} s)", "Same 5-gate engine and models; only the ledger, terms (DCIM asset codes · MOP · EOP · NOC) and drawings (MEP · SLD · NET) change"),
    "intro.aidc_mermaid": ("데이터센터 Mermaid", "CDU 2차 냉각수 → 인랙 매니폴드 → GPU 랙 계통", "Data center Mermaid", "CDU secondary coolant → in-rack manifold → GPU racks"),
    "intro.aidc_mindmap": ("데이터센터 마인드맵", "발전소와 1:1 대칭인 홉 Depth 그래프", "Data center mind map", "Hop-depth graph mirroring the power plant 1:1"),
    "intro.plant": ("다시 발전소로 ({sec}초)", "도메인을 바꾸면 해당 도메인 마스터 도면 5장으로 델타 색인(MasterSet)을 다시 만듭니다",
                    "Back to the power plant ({sec} s)", "A domain switch rebuilds the delta index (MasterSet) from that domain's five master drawings"),
    "search.floor": ("질의 '{q}' → 1위 {code}", "{why}", "Query '{q}' → top {code}", "{why}"),
    "search.sync_drawing": ("1위를 누르면 도면이 그 설비로", "선택은 세 뷰가 함께 씁니다", "Clicking the top hit focuses the drawing", "All three views share the selection"),
    "search.sync_mermaid": ("Mermaid 도 같은 설비로", "", "Mermaid follows the same equipment", ""),
    "search.sync_mindmap": ("마인드맵도 같은 설비 가운데로", "", "The mind map centers on it too", ""),
    "search.demote": ("조건 강등 — '{q}' → 1위 {code}", "105동에는 밸브가 없어 '밸브' 조건을 지우지 않고 힌트로 내려 0건을 막습니다 · {why}",
                      "Condition demotion — '{q}' → top {code}", "Bldg 105 has no valves, so 'valve' is demoted to a hint instead of returning zero hits · {why}"),
    "search.negate": ("부정어 — '{q}' ({hits}건)", "'밸브 아닌' 을 제외 조건으로 읽습니다 · 1위 {code}", "Negation — '{q}' ({hits} hits)", "'not valves' becomes an exclusion · top {code}"),
    "search.typo": ("코드 오타 교정 — '{q}' → {code}", "1↔I · 0↔O 오타를 원장 편집 거리로 바로잡아 찾습니다", "Code typo correction — '{q}' → {code}", "1↔I · 0↔O typos are fixed by edit distance against the ledger"),
    "search.moc": ("유형 질의 — '{q}' → {code}", "{why}", "Type query — '{q}' → {code}", "{why}"),
    "p2.inject": ("P2 주입: 103동 급수조절밸브 18% 고착", "알람이 들어오면 5단계 파이프라인이 차례로 켜집니다 (스캔 없는 신호 알람이라 문서 모델은 올리지 않음)",
                  "P2 injected: Bldg 103 feedwater control valve stuck at 18%", "The 5-stage pipeline lights up in turn (a signal alarm with no scan, so no document models load)"),
    "p2.open": ("근본 원인 = 103동 주급수 유량조절밸브 · 다른 도면의 과열기까지 CCTV 말풍선", "원인 경로 배관에 {lag}",
                "Root cause = Bldg 103 main feedwater flow control valve · callouts reach the superheater on another drawing", "On the causal pipe: {lag}"),
    "p2.replay": ("타임머신 재생 — 발원점부터", "상류 밸브 → 펌프 토출압 → 과열기 순서로 변곡이 켜집니다 (SDS 상태 전이)",
                  "Time-machine replay from the origin", "Inflections light up upstream valve → pump discharge → superheater (SDS state transitions)"),
    "p2.charts": ("허브 Drop-up: CAM-02 시계열 · CAM-03 SDS 로직 차트", "재생 시각까지의 파형과 축별 SDS 판정", "Hub drop-up: CAM-02 time series · CAM-03 SDS logic chart", "Waveforms up to the replay time and per-axis SDS checks"),
    "p2.mining": ("프로세스 마이닝 — 흐름 · 병목 겹치기", "배관 유량 비율 · 병목 · 근본 원인 물결을 도면 위에", "Process mining — flow · bottleneck overlay", "Pipe flow ratios, bottlenecks and root-cause ripples on the drawing"),
    "p2.lanes": ("마이닝 레인 — 단계별 체류 · 병목", "사건 대기열의 Gate 별 레인", "Mining lanes — per-gate dwell and bottlenecks", "Gate lanes in the incident queue"),
    "p2.mermaid": ("Mermaid 의 Δt 라벨", "인과 경로 간선에 전파 시차와 허용 범위가 붙습니다", "Δt labels in Mermaid", "Causal edges carry the propagation lag and its allowance"),
    "p2.mindmap": ("마인드맵 유리 핀", "변곡을 지난 노드에 시각 핀이 꽂힙니다", "Glass pins on the mind map", "Nodes past their inflection get a time pin"),
    "p2.stage3": ("Stage 3 — Gate 1~2 원장 대조", "신호 · 원장 · 보팅을 맞춰 봅니다", "Stage 3 — Gate 1~2 ledger check", "Signal, ledger and voting are cross-checked"),
    "p2.stage4": ("Stage 4 — Gate 3 마인드맵 파급 + Zoro-1 판단", "PointerPrompt 사실 · GPU 실행 기록 · SDS 축별 질문 막대", "Stage 4 — Gate 3 propagation + Zoro-1 verdict", "PointerPrompt facts · GPU trace · per-axis SDS question bars"),
    "p2.q_pressure": ("고장 중 실시간 질의 — '{q}'", "{why}", "Live query during the fault — '{q}'", "{why}"),
    "p2.q_stuck": ("'{q}' → {code}", "{why}", "'{q}' → {code}", "{why}"),
    "p2.q_typo": ("오타 + 상태 — '{q}' → {code}", "{why}", "Typo + state — '{q}' → {code}", "{why}"),
    "p2.q_flow": ("수치 조건 — '{q}' → {code}", "{why}", "Numeric condition — '{q}' → {code}", "{why}"),
    "p2.stage5": ("Stage 5 — 절차서 초안 · Factcheck · 결재", "고위험 조치(감발)가 든 AOP-103-FW02 라 파트장 공동 결재가 필요합니다", "Stage 5 — draft · factcheck · approval", "AOP-103-FW02 contains a high-risk action (load reduction), so a team-lead co-sign is required"),
    "p2.g3_rubber": ("가드레일 ① 공동 결재 없이 확정 → 차단 (G3_INTERCEPT_RUBBER_STAMP)", "{text}", "Guardrail ① confirm without co-sign → blocked (G3_INTERCEPT_RUBBER_STAMP)", "{text}"),
    "p2.reject": ("가드레일 ② 증빙 없이 반려 → 차단", "{text}", "Guardrail ② reject without evidence → blocked", "{text}"),
    "p2.g3_order": ("가드레일 ③ 선행 절차를 건너뛰고 확정 → 차단 (G3_INTERCEPT_INTERLOCK_ORDER)", "{text}", "Guardrail ③ skip prerequisite steps → blocked (G3_INTERCEPT_INTERLOCK_ORDER)", "{text}"),
    "p2.step2": ("인터록 Step 2 — 바이패스 개도 65% 현장 값 확인", "Step 1 은 DCS 계측으로 이미 통과, Step 2 를 채우면 Step 3 만 남습니다", "Interlock Step 2 — bypass opening 65% field value", "Step 1 already passed on DCS data; after Step 2 only Step 3 remains"),
    "p2.step3": ("인터록 Step 3 — LOTO 무전 확인 (사번 날인)", "순서대로 모두 확인되어 확정 잠금이 풀립니다", "Interlock Step 3 — LOTO radio confirmation (ID stamp)", "All steps confirmed in order; the confirm lock opens"),
    "p2.archive": ("공동 결재 확정 → 보관 문서 {archive} · SDS 블랙박스 봉인", "60초 시계열 스냅숏과 판정 이력이 해시 체인으로 봉인됩니다", "Co-signed → archived record {archive} · SDS black-box seal", "The 60 s time-series snapshot and verdict history are sealed in a hash chain"),
    "p1.awaiting": ("P1: 주증기 압력 채널 A 만 14.2 MPa — 현장 대기", "2oo3 보팅에서 채널 A 만 벗어나 트립을 보류하고 현장 게이지 실측을 기다립니다", "P1: main steam pressure channel A alone at 14.2 MPa — awaiting field", "Only channel A deviates in 2oo3 voting, so the trip is held until the field gauge is read"),
    "p1.g2_block": ("가드레일 — 현장 확인 전 확정 → 차단 (G2_FIELD_RECHECK_REQUIRED)", "{text}", "Guardrail — confirm before the field check → blocked (G2_FIELD_RECHECK_REQUIRED)", "{text}"),
    "p1.recheck": ("현장 재점검 요청", "{text}", "Field recheck requested", "{text}"),
    "p1.kiosk": ("현장 단말 — 계기 점검표 스캔", "흔들린 손글씨(궁서) 점검표 · ±0.8° 기울기 · 잡음", "Field terminal — gauge sheet scan", "Shaky handwritten sheet · ±0.8° skew · scan noise"),
    "p1.scan_done": ("원터치 대조 카드 — 스캔 조각 {crops}개", "SigLIP2 가 고른 조각을 gocr 가 읽어 칸을 채웠습니다. 카드마다 스캔 원문 조각이 붙습니다", "One-touch check cards — {crops} scan crops", "gocr read the crops SigLIP2 chose and filled the fields; each card shows its source crop"),
    "p1.stepper": ("장갑 스테퍼", "장갑을 낀 채 ±0.1 단추로 실측치를 고칩니다", "Glove stepper", "Adjust the reading with ±0.1 buttons while wearing gloves"),
    "p1.sign_line": ("서명을 한 획 직선으로", "서명 규칙: 획 2회 이상 · 면적 15% 이상 · 직선 아님", "A one-stroke straight-line signature", "Signature rule: 2+ strokes · 15%+ of the pad · not a straight line"),
    "p1.g1_retry": ("가드레일 — 무효 서명 → 사건을 열지 않고 재서명 (G1_SIGN_INVALID_RETRY)", "{text}", "Guardrail — invalid signature → re-sign without opening a case (G1_SIGN_INVALID_RETRY)", "{text}"),
    "p1.excuse": ("편차 소명 화면", "중앙 계측값과 현장 실측치가 달라 소명을 받습니다 (내부 판정 코드 · 원장 수치는 보이지 않음)", "Deviation justification", "The central and field readings differ, so a justification is required (no internal codes or ledger values shown)"),
    "p1.free_text": ("자유 서술 소명 → Zoro-1 이 표준 코드로 분류", "현장 작업자가 쓴 글을 글쓰기 모델이 정규화합니다 (normalize_excuse)", "Free-text justification → Zoro-1 sorts it into a standard code", "The writer model normalizes what the field worker typed (normalize_excuse)"),
    "p1.sent": ("전송 → 접수 번호", "현장 보고가 콘솔의 현장 대기 사건에 붙고(attach_report) 다시 분석합니다", "Sent → receipt number", "The report attaches to the waiting incident (attach_report) and is re-analyzed"),
    "p1.truth": ("3중 진실 대조표 — DCS 2oo3 vs 현장 게이지 (촬영 시각 기준)", "채널 A 단독 드리프트로 분리 (G2_SENSOR_DRIFT_ISOLATED) · 트립 보류", "Triple-truth table — DCS 2oo3 vs field gauge (at scan time)", "Isolated as a channel-A drift (G2_SENSOR_DRIFT_ISOLATED) · trip held"),
    "p1.unmask": ("작업자 실명 열람 요청 (request_unmask)", "{text}", "Worker name unmask request (request_unmask)", "{text}"),
    "p1.stage5": ("확정", "", "Confirm", ""),
    "p1.archive": ("확정 → 보관", "", "Confirmed → archived", ""),
    "p1.kiosk_done": ("현장 단말에 처리 결과 안내", "타 근무자 실명 · 내부 정보 누출 검사(check_field_text)를 거친 글만 갑니다", "Result notice on the field terminal", "Only text that passed the leak check (check_field_text) is sent"),
    "p3.kiosk": ("P3: Red-line 수기 수정 도면 (MOC 승인 수치 31.0 MPa)", "붉은 펜으로 경보 한계를 고친 PID-103-LAB 스캔", "P3: hand-marked Red-line drawing (MOC-approved 31.0 MPa)", "PID-103-LAB scan with the alarm limit changed in red pen"),
    "p3.scan_done": ("SigLIP2 델타 패치로 읽음 — 조각 {crops}개", "마스터 도면을 찾고 바뀐 붉은 잉크 둘레 · 표제란 · 서명란만 읽었습니다", "Read with the SigLIP2 delta patch — {crops} crops", "Found the master drawing and read only around the changed red ink, the title block and the signature"),
    "p3.form": ("변경번호 {moc} · 새 한계 읽음", "", "Change no. {moc} · new limit read", ""),
    "p3.sent": ("전송", "", "Sent", ""),
    "p3.stage12": ("Stage 1 & 2 — SigLIP2 조각 계획 · gocr 판독 · 대장 교정", "버린 안내문 · 빈 조각 · 마스터와 같아 건너뛴 조각 수가 함께 나옵니다", "Stage 1 & 2 — SigLIP2 crop plan · gocr reading · register correction", "Shows the dropped guides, blank crops and crops skipped as identical to the master"),
    "p3.stage3": ("Gate 2 — 승인 원장 대조 (G2_MOC_VERIFIED)", "변경번호 · 설비 · 새 한계 · 서명자 권한 · 설계 한계를 모두 맞춥니다", "Gate 2 — approved-ledger check (G2_MOC_VERIFIED)", "Change no., equipment, new limit, signer authority and design limit all match"),
    "p3.overlay": ("도면 위 MOC 오버레이", "승인된 새 한계가 도면 기준치에 반영됩니다", "MOC overlay on the drawing", "The approved new limit is applied to the drawing's reference values"),
    "p3.archive": ("확정 → 보관", "", "Confirmed → archived", ""),
    "p3.kiosk_done": ("현장 단말: 기준치에 반영되었습니다", "", "Field terminal: applied to the reference values", ""),
    "p3f.kiosk": ("P3F: 같은 도면의 위조 수치 35.0 MPa", "", "P3F: the same drawing with a forged 35.0 MPa", ""),
    "p3f.scan_done": ("델타 판독 — 조각 {crops}개", "", "Delta reading — {crops} crops", ""),
    "p3f.stage12": ("Stage 1 & 2", "", "Stage 1 & 2", ""),
    "p3f.stage3": ("승인 원장 31.0 MPa 와 모순 → 잠금 (G2_REJECT_CONTRADICTION_LOCK)", "", "Contradicts the approved 31.0 MPa → locked (G2_REJECT_CONTRADICTION_LOCK)", ""),
    "p3f.locked": ("위조 Red-line 은 반영되지 않고 잠깁니다", "기술책임자 확인 전까지 기준치 변경 잠금", "A forged Red-line is locked, not applied", "Reference change locked until the technical lead checks"),
    "p4.stage4": ("P4: 고압터빈 베어링 진동 — 모델 이견", "상류 이상 없이 경보 설비만 변해 규칙은 단위 설비 고장(고위험), 모델 축 확률은 엇갈려 확신도 없음", "P4: HP turbine bearing vibration — model dissent", "Only the alarmed unit changed, so the rule says single-unit fault (high risk); axis probabilities disagree → no confidence"),
    "p4.stage5": ("고위험 → 공동 결재 잠금 창", "", "High risk → co-sign lock", ""),
    "p4.g3_rubber": ("공동 결재 없이 확정 → 차단", "{text}", "Confirm without co-sign → blocked", "{text}"),
    "p4.false_positive": ("오경보 예외 승인 → 현장 증빙 스캔 1장 이상 필요", "{text}", "False-alarm exception → needs at least one field evidence scan", "{text}"),
    "p4.cosign": ("파트장 공동 결재", "", "Team lead co-sign", ""),
    "p4.archive": ("APPROVED_HIGH_RISK → 보관 문서 {archive}", "", "APPROVED_HIGH_RISK → archived record {archive}", ""),
    "phys.start": ("물리엔진 100 ms 폐루프 — 정상 운전", "", "Physics engine 100 ms closed loop — normal operation", ""),
    "phys.inject": ("급수조절밸브 18% 고착 주입", "", "Feedwater control valve stuck at 18%", ""),
    "phys.chain": ("SDS 변곡 → 인과 사슬 잠금 → 사건 열림", "글쓰기를 기다리지 않고 run_quick 으로 먼저 판정합니다", "SDS inflection → causal chain locked → incident opened", "run_quick decides first without waiting for the writer"),
    "phys.tier1": ("Tier 1 자율 대응 제안 — 급수 바이패스 65% 개방", "네 기준 최솟값이 0.95 미만이라 보조 모드: 교대감독이 실행", "Tier 1 autonomous response — open the feedwater bypass 65%", "The minimum of four criteria is below 0.95, so Assist Mode: the shift supervisor runs it"),
    "phys.run": ("완화 조치 실행", "", "Mitigation executed", ""),
    "phys.recovering": ("유량 회복 → 과열기 고점 {peak} ℃ 에서 꺾임 (트립 645 ℃ 전)", "백그라운드에서 redraft 가 절차서 초안을 채웁니다", "Flow recovers → superheater peaks at {peak} ℃ (before the 645 ℃ trip)", "redraft fills in the procedure draft in the background"),
    "phys.hud": ("실제 계측으로 인터록 Step 2 자동 통과 (refresh_interlock)", "", "Interlock Step 2 auto-passed from live measurement (refresh_interlock)", ""),
    "aidc.switch": ("AI 데이터센터로 전환 ({sec}초)", "", "Switched to the AI data center ({sec} s)", ""),
    "d2.open": ("D2: CDU 2차측 유량조절밸브 18% 고착 → GPU Pod A 정션 온도", "원인 경로 배관에 {lag}", "D2: CDU secondary flow control valve stuck at 18% → GPU Pod A junction temperature", "On the causal pipe: {lag}"),
    "d2.replay": ("타임머신 재생", "CDU 밸브 → 2차 순환펌프 → 매니폴드 → GPU 랙", "Time-machine replay", "CDU valve → secondary pump → manifold → GPU rack"),
    "d2.stage4": ("D2 Stage 4 — SDS 축별 질문", "", "D2 Stage 4 — per-axis SDS questions", ""),
    "d2.q_stuck": ("'{q}' → {code}", "{why}", "'{q}' → {code}", "{why}"),
    "d2.q_gpu": ("'{q}' → {code}", "{why}", "'{q}' → {code}", "{why}"),
    "d2.q_typo": ("자산코드 오타 — '{q}' → {code}", "{why}", "Asset-code typo — '{q}' → {code}", "{why}"),
    "d2.q_mop": ("'{q}' → {code}", "{why}", "'{q}' → {code}", "{why}"),
    "d2.g3_order": ("선행 절차를 건너뛰고 확정 → 차단", "{text}", "Skip prerequisite steps → blocked", "{text}"),
    "d2.interlock": ("CDU 우회밸브 인터록 3단계 확인", "", "CDU bypass interlock: 3 steps confirmed", ""),
    "d2.archive": ("NOC 결재 → 보관 문서 {archive}", "", "NOC approval → archived record {archive}", ""),
    "d1.awaiting": ("D1: 매니폴드 공급 압력 채널 A 만 0.8 bar — 현장 대기", "", "D1: manifold supply pressure channel A alone at 0.8 bar — awaiting field", ""),
    "d1.scan_done": ("현장 엔지니어 단말 — 점검표 스캔 조각 {crops}개", "", "Field engineer terminal — {crops} scan crops", ""),
    "d1.form": ("자산코드 {kks} 읽음", "", "Asset code {kks} read", ""),
    "d1.sent": ("소명 포함 전송", "", "Sent with justification", ""),
    "d1.truth": ("3중 진실 대조표 — BMC 2oo3 vs 현장 매니폴드 압력계", "", "Triple-truth table — BMC 2oo3 vs field manifold gauge", ""),
    "d1.archive": ("확정 → 보관", "", "Confirmed → archived", ""),
    "d1.kiosk_done": ("현장 단말 처리 결과", "", "Field terminal result", ""),
    "d3.scan_done": ("D3: MEP Red-line 델타 판독 — 조각 {crops}개", "", "D3: MEP Red-line delta reading — {crops} crops", ""),
    "d3.form": ("변경번호 {moc} (3.5 bar)", "", "Change no. {moc} (3.5 bar)", ""),
    "d3.stage12": ("Stage 1 & 2 — 데이터센터 서식", "", "Stage 1 & 2 — data center sheet", ""),
    "d3.stage3": ("MOP 원장 일치", "", "Matches the MOP ledger", ""),
    "d3.archive": ("확정 → 기준치 반영", "", "Confirmed → reference updated", ""),
    "d3f.scan_done": ("D3F: 위조 4.5 bar", "", "D3F: forged 4.5 bar", ""),
    "d3f.stage3": ("승인 3.5 bar 와 모순 · 콜드플레이트 내압 4.0 bar 초과 → 잠금", "", "Contradicts the approved 3.5 bar and exceeds the 4.0 bar cold-plate limit → locked", ""),
    "d3f.locked": ("위조 MOP 잠금", "", "Forged MOP locked", ""),
    "d4.stage4": ("D4: UPS 배터리 셀 온도 급등 — 모델 이견", "", "D4: UPS battery cell temperature surge — model dissent", ""),
    "d4.g3_rubber": ("공동 결재 없이 확정 → 차단", "{text}", "Confirm without co-sign → blocked", "{text}"),
    "d4.archive": ("EPO 포함 EOP 공동 결재 → 보관 문서 {archive}", "", "EOP with EPO co-signed → archived record {archive}", ""),
    "dphys.chain": ("데이터센터 물리엔진 — 인과 사슬 잠금", "", "Data center physics engine — causal chain locked", ""),
    "dphys.tier1": ("Tier 1 — CDU 우회밸브 65% 개방 제안", "", "Tier 1 — open the CDU bypass valve 65%", ""),
    "dphys.recovering": ("GPU 정션 온도 고점 {peak} ℃ 에서 꺾임 (셧다운 95 ℃ 전)", "", "GPU junction temperature peaks at {peak} ℃ (before the 95 ℃ shutdown)", ""),
    "end.queue": ("처리 완료 대기열", "", "Completed queue", ""),
}

SUMMARY_CUT = {
    "P5": ("P5: 과열기 1초 스파이크 810 ℃ → 잡음으로 걸러 냄", "P5: 1 s superheater spike to 810 ℃ → filtered as noise"),
    "P6": ("P6: 윤활유 온도 40초 동결 → 계측값 고정 (SENSOR_FLATLINE)", "P6: lube oil temperature frozen for 40 s → frozen reading (SENSOR_FLATLINE)"),
    "P7": ("P7: 노내 압력 상승 기울기 → 트립 전 선행 경보 (TTF)", "P7: furnace pressure rising slope → early warning before trip (TTF)"),
    "D5": ("D5: GPU 정션 1초 스파이크 145 ℃ → 잡음", "D5: 1 s GPU junction spike to 145 ℃ → noise"),
    "D6": ("D6: 랙 냉각수 출구 온도 동결 (BMC 데몬 행)", "D6: rack coolant outlet temperature frozen (BMC daemon hang)"),
    "D7": ("D7: 냉수 공급 온도 상승 기울기 → 셧다운 전 선행 경보", "D7: chilled-water supply temperature rising slope → early warning before shutdown"),
}


def caption(mark_id, data):
    ko_t, ko_n, en_t, en_n = CAP.get(mark_id, (mark_id, "", mark_id, ""))
    t, n = (en_t, en_n) if LANG == "en" else (ko_t, ko_n)
    vals = dict(data)
    name, code, why = top_hit(data.get("top", ""))
    vals.setdefault("code", code or name)
    vals.setdefault("why", why)
    vals.setdefault("hits", hits_of(data.get("toast", "")))
    if "ms" in data:
        vals["sec"] = f"{data['ms'] / 1000:.1f}"
    lags = data.get("lags") or []
    vals["lag"] = re.sub(r"^\W+", "", lags[0]).strip() if lags else ""
    txt = data.get("text", "")
    if isinstance(txt, list):
        txt = " · ".join(txt[1:] or txt)
    m = re.search(r"((?:MCR|NOC)-\d{4}-\d{4})", txt)
    vals["archive"] = m.group(1) if m else ""
    m = re.search(r"(?:고점|peak) ([0-9.]+)", txt)
    vals["peak"] = m.group(1) if m else "?"
    vals["text"] = re.sub(r"\s*✕\s*$", "", txt.split(" | ")[-1]).replace("\n", " ").strip()
    out = []
    for s in (t, n):
        try:
            out.append(s.format(**vals))
        except (KeyError, IndexError, ValueError):
            out.append(re.sub(r"\{[^}]*\}", "", s))
    return out[0], out[1]


# ───────── 이야기 구성 ─────────


class Story:
    def __init__(self, recs, trace, catalog):
        self.recs = recs
        self.trace = trace
        self.catalog = catalog
        self.segs = []

    def find(self, mark_id):
        """장면이 성공한 마지막 녹화의 장면 표시"""
        hit = None
        for r in self.recs:
            m = r.marks.get(mark_id)
            if m and m["scene"] not in r.failed:
                hit = (r, m)
        if hit is None:
            for r in self.recs:
                if mark_id in r.marks:
                    hit = (r, r.marks[mark_id])
        return hit

    def snap(self, name):
        for r in reversed(self.recs):
            s = r.snap(name)
            if s is not None:
                return s
        return None

    def card(self, img, shown, name):
        self.segs.append(Seg(shown, lambda k, n, img=img: img, name))

    def beats(self, part, where, ids, lead=LEAD, full_lead=(), real=()):
        for mid in ids:
            hit = self.find(mid)
            if not hit:
                continue
            rec, m = hit
            prev = rec.prev_mark_t(m)
            nxt = rec.next_mark_t(m)
            start = m["t"] - lead * 1000
            if "q" in m:
                # 질의는 치기 시작한 때부터 (예전 녹화에는 t0 가 없어 친 시간을 어림합니다)
                start = m.get("t0") or m["t"] - (len(m["q"]) * 55 + 2700)
            if mid in full_lead:
                start = prev
            start = max(prev, start)
            # 장면 표시 바로 앞에서 창이 바뀌었으면 바뀐 때부터 (앞 창이 새 자막 아래 잠깐 보이지 않게)
            switches = [t for t, _ in rec.srcs if start < t <= m["t"]]
            if switches:
                start = switches[-1]
            end = min(nxt, m["t"] + (6500 if m.get("text") else 26000))
            label, note = caption(mid, m)
            self.segs.append(footage_seg(rec, self.trace, start, end, part, where, label, note, mid, real_speed=mid in real))

    def model(self, shown, part, label, note, painter, name, active=()):
        self.segs.append(model_seg(shown, part, label, note, painter, name, active))

    def window(self, a_id, b_id):
        a, b = self.find(a_id), self.find(b_id)
        if not a or not b:
            return None
        return a[1]["t"], b[1]["t"]


def probe(name):
    p = os.path.join(ROOT, "out", name)
    return json.load(open(p, encoding="utf-8")) if os.path.exists(p) else None


def scan_events(story, start_id, end_id):
    w = story.window(start_id, end_id)
    if not w:
        return None, None, None, []
    a, b = w
    tr = story.trace
    plan = next(iter(tr.between(a, b + 2000, "규칙", "조각 계획")), None)
    read = next(iter(tr.between(a, b + 2000, "gocr", "결과")), None)
    sym = next(iter(tr.between(a, b + 2000, "siglip2", "심볼 그림 사전")), None)
    timing = []
    for model, step, ko, en in (("siglip2", "패치 특징", "SigLIP2 패치 특징", "SigLIP2 patch features"), ("규칙", "조각 계획", "조각 계획 (규칙)", "crop plan (rules)"), ("gocr", "비전", "gocr 비전", "gocr vision"), ("gocr", "언어", "gocr 언어", "gocr language")):
        ev = tr.between(a, b + 2000, model, step)
        if ev:
            timing.append(f"{L(ko, en)} {sum(e['ms'] for e in ev) / 1000:.2f} s")
    if read:
        det = read["detail"]
        timing.append(L(f"gocr 조각 {det.get('crops')}개 · 글자 토큰 {det.get('tokens')} · 이미지 토큰 {det.get('image_tokens')}", f"gocr {det.get('crops')} crops · {det.get('tokens')} text tokens · {det.get('image_tokens')} image tokens"))
    return plan, read, sym, [" · ".join(timing[:2]), " · ".join(timing[2:])] if timing else []


def build(story):
    S = story
    P = lambda n, ko, en: f"PART {n} · {L(ko, en)}"
    S.card(title_card([
        ("blueprint", 40, BLUE),
        (L("1.2GW 화력발전소 ↔ 100MW AI 데이터센터", "1.2GW Thermal Power Plant ↔ 100MW AI Data Center"), 62, TEXT),
        (L("듀얼 도메인 관제 콘솔 · 실제 앱 E2E 시연", "Dual-domain control console · real-app E2E demo"), 40, TEXT),
        (L("SigLIP2 도면 전처리 → 자연어 질의 → N:N 인과 역추적 · 물리엔진 → Zoro-1 판단 → 현장 · 교대감독 가드레일 · 봉인", "SigLIP2 drawing preprocessing → natural-language search → N:N causal trace · physics → Zoro-1 verdict → field · supervisor guardrails · seal"), 26, MUTED),
        (L("합성 데이터 · 실제 모델 (ROCm, Radeon RX 6600 8 GB) · 영상의 숫자는 이 실행의 기록에서 가져왔습니다", "Synthetic data · real models (ROCm, Radeon RX 6600 8 GB) · every number on screen comes from this run's logs"), 22, GRAY),
    ]), 6.5, "title")
    S.card(models_card(), 9, "models")

    # PART 1
    S.card(divider_card("PART 1", L("듀얼 도메인 · 3-Way 도면 뷰", "Dual domain · 3-way drawing views"), L("같은 엔진 · 같은 모델로 발전소(KKS · DCS · P&ID · MOC · AOP)와 데이터센터(DCIM · BMC · MEP/SLD · MOP · EOP)를 1:1 대칭으로 관제합니다", "One engine and one model set run the power plant (KKS · DCS · P&ID · MOC · AOP) and the data center (DCIM · BMC · MEP/SLD · MOP · EOP) 1:1")), 4.5, "div1")
    p1 = P(1, "듀얼 도메인 · 3-Way 뷰", "Dual domain · 3-way views")
    S.beats(p1, L("관제 콘솔", "Control console"), ["intro.console", "intro.mermaid", "intro.mindmap", "intro.a0", "intro.a0tile", "intro.kiosk", "intro.aidc", "intro.aidc_mermaid", "intro.aidc_mindmap", "intro.plant"])

    # PART 2 자연어 질의
    S.card(divider_card("PART 2", L("운전원 자연어 질의", "Operator natural-language search"), L("유형 · 동 · 층 · 종류 · 계측 · 상태 · 비교어 · 연결 홉을 읽고, 0건이 될 조건은 힌트로 내리고, 코드 오타는 원장으로 고칩니다", "Reads type · building · floor · kind · measure · state · comparators · hops; demotes conditions that would zero out; fixes code typos against the ledger")), 4.5, "div2")
    p2 = P(2, "자연어 질의", "Natural-language search")
    S.beats(p2, L("관제 콘솔 · 하단 허브 질의", "Console · hub query"), ["search.floor", "search.sync_drawing", "search.sync_mermaid", "search.sync_mindmap", "search.demote", "search.negate", "search.typo", "search.moc"])

    # PART 3 전처리
    S.card(divider_card("PART 3", L("도면 전처리 — SigLIP2 크롭 · 델타 · 심볼", "Drawing preprocessing — SigLIP2 crops · delta · symbols"), L("현장 스캔이 들어오면 SigLIP2 가 어디를 읽을지 고르고(4종 채택 · 안내문 · 빈 띠 버림), 마스터와 견줘 바뀐 곳만 gocr 로 읽습니다", "For each field scan SigLIP2 picks where to read (4 kinds kept, guides and blank bands dropped) and gocr reads only what changed versus the master")), 5, "div3")
    p3 = P(3, "도면 전처리", "Preprocessing")
    S.beats(p3, L("현장 단말", "Field terminal"), ["p1.kiosk", "p1.scan_done"])
    plan, read, sym, timing = scan_events(S, "p1.scan_start", "p1.scan_done")
    if plan:
        S.model(12, p3, L("P1 계기 점검표 — SigLIP2 영역 경쟁 → gocr", "P1 gauge sheet — SigLIP2 region arena → gocr"), L("파랑 제목 · 초록 입력란 · 보라 표 · 주황 서명만 남기고, 안내문(회색 X)과 글자 없는 띠(붉은 줄)는 gocr 에 넣지 않습니다", "Only blue title, green form, purple table and orange sign crops remain; guides (gray X) and blank bands (red line) never reach gocr"),
                crop_plan_painter(os.path.join(ROOT, "out", "field", "P1_gauge.png"), plan, read, sym, timing, L("스캔: out/field/P1_gauge.png (합성 손글씨 · 기울기 · 잡음)", "Scan: out/field/P1_gauge.png (synthetic handwriting · skew · noise)")), "m.p1plan", active={"siglip2", "gocr_v", "gocr_l"})
    S.beats(p3, L("현장 단말", "Field terminal"), ["p3.kiosk", "p3.scan_done", "p3.form"])
    plan, read, sym, timing = scan_events(S, "p3.scan_start", "p3.scan_done")
    if plan:
        S.model(13, p3, L("P3 Red-line — 마스터 PID-103-LAB 식별 · 델타 패치", "P3 Red-line — master PID-103-LAB identified · delta patch"), L("회색 = 마스터와 같아 건너뛴 조각, 노랑 = 잉크가 바뀐 칸. 붉은 펜(chroma ≥ 45) · 바뀐 잉크 둘레 · 표제란 · 서명란만 읽습니다", "Gray = crops skipped as identical to the master, yellow = changed ink cells. Only red pen (chroma ≥ 45), changed ink, the title block and signature are read"),
                crop_plan_painter(os.path.join(ROOT, "out", "field", "P3_redline.png"), plan, read, sym, timing, L("스캔: out/field/P3_redline.png", "Scan: out/field/P3_redline.png")), "m.p3plan", active={"siglip2", "gocr_v", "gocr_l"})
        pre_p, base_p, pre_a, base_a, sh_no, sh_al = probe("delta_pre_plant.json"), probe("delta_base_plant.json"), probe("delta_pre_aidc.json"), probe("delta_base_aidc.json"), probe("delta_shift_noalign.json"), probe("delta_shift_align.json")
        if pre_p and base_p and pre_a and base_a:
            g = lambda d, scan, k: next(s for s in d["scans"] if s["scan"] == scan)[k]["ms"] / 1000
            rows = [
                (L("P3 Red-line · 전부 읽기", "P3 Red-line · read everything"), L("조각 14개", "14 crops"), g(pre_p, "P3_redline", "full"), GRAY),
                (L("P3 · 델타 (전처리 Off)", "P3 · delta (preprocessing off)"), L("도각 맞춤 · 유채색 분리 없이", "no frame alignment · no chroma split"), g(base_p, "P3_redline", "delta"), ORANGE),
                (L("P3 · 델타 (전처리 On)", "P3 · delta (preprocessing on)"), L("안내문 자르기 · 붉은 펜 · 도각 맞춤", "guide trim · red pen · frame alignment"), g(pre_p, "P3_redline", "delta"), GREEN),
                (L("D3 MEP Red-line · 델타 Off → On", "D3 MEP Red-line · delta off → on"), f"{g(base_a, 'D3_redline', 'delta'):.1f} s → {g(pre_a, 'D3_redline', 'delta'):.1f} s", g(pre_a, "D3_redline", "delta"), GREEN),
            ]
            if sh_no and sh_al:
                rows.append((L("밀린 스캔 (9 × 6 px) · 맞춤 없음", "Shifted scan (9 × 6 px) · no alignment"), L("바뀐 칸 4,240/11,000 → 전부 읽기로 물러남", "4,240/11,000 cells 'changed' → falls back to reading everything"), g(sh_no, "P3_redline", "delta"), RED))
                rows.append((L("밀린 스캔 · 도각 맞춤", "Shifted scan · frame alignment"), L("이동 (9, 6) 찾아 되돌림", "finds the (9, 6) shift and undoes it"), g(sh_al, "P3_redline", "delta"), GREEN))
            S.model(11, p3, L("전처리 Off · On 판독 시간 (delta_probe 실측)", "Reading time with preprocessing off vs on (delta_probe measurements)"), L("같은 스캔 · 같은 모델. 바뀐 곳 둘레만 좁혀 읽어 시간이 줄어듭니다", "Same scans, same models — only the area around the changes is read"),
                    delta_compare_painter(rows, L("출처: out/delta_pre_*.json · delta_base_*.json · delta_shift_*.json (ROCm RX 6600, 2026-11-06). 마스터 도면 판독은 KKS 35/35 · DCIM 자산코드 32/32 (plant_probe).", "Source: out/delta_pre_*.json · delta_base_*.json · delta_shift_*.json (ROCm RX 6600, 2026-11-06). Master drawings read KKS 35/35 · DCIM asset codes 32/32 (plant_probe).")), "m.delta")
        sp = probe("symbol_probe.json")
        if sp:
            S.model(10, p3, L("글자 없는 설비 심볼 — SigLIP2 그림 사전 (symbol_probe 실측)", "Text-free equipment symbols — SigLIP2 symbol dictionary (symbol_probe measurement)"), L("명판이 훼손돼도 기호 모양으로 펌프 · 밸브 · 열교환기 · 차단기를 맞힙니다. 애매하면 답하지 않고 보류합니다", "Pumps, valves, heat exchangers and breakers are recognized by shape even when nameplates are damaged; ambiguous ones abstain"),
                    symbols_painter("PID-103-LAB", sp), "m.symbols", active={"siglip2"})
    S.beats(p3, L("관제 콘솔", "Control console"), ["p3.stage12", "p3.stage3", "p3.overlay", "p3.kiosk_done", "p3f.kiosk", "p3f.scan_done", "p3f.stage3", "p3f.locked"])
    S.beats(p3, L("현장 단말 · 데이터센터", "Field terminal · data center"), ["d3.scan_done", "d3.form", "d3.stage12", "d3f.stage3", "d3f.locked"])

    # PART 4 N:N
    S.card(divider_card("PART 4", L("N:N 시계열 · 인과 역추적 · 물리엔진 폐루프", "N:N time series · causal back-trace · physics closed loop"), L("상류 고착 → 하류 과열이 도면 점프 커넥터를 건너 몇 초 뒤에 오는지, 다중 보팅과 현장 게이지가 무엇을 말하는지 같은 화면에서 봅니다", "See how an upstream stuck valve reaches a downstream overheat across drawing jump connectors, and what multi-channel voting and the field gauge say")), 5, "div4")
    p4 = P(4, "N:N 인과 역추적", "N:N causal trace")
    S.beats(p4, L("관제 콘솔", "Control console"), ["p2.inject"], lead=1.0)
    S.beats(p4, L("관제 콘솔", "Control console"), ["p2.open"], full_lead=("p2.open",))
    S.beats(p4, L("관제 콘솔", "Control console"), ["p2.replay", "p2.charts", "p2.mining", "p2.lanes", "p2.mermaid", "p2.mindmap", "p2.q_pressure", "p2.q_stuck", "p2.q_typo", "p2.q_flow", "p2.stage3"], real=("p2.replay",))
    S.beats(p4, L("관제 콘솔 · 현장 단말", "Console · field terminal"), ["p1.awaiting", "p1.truth"])
    S.beats(p4, L("데이터센터 콘솔", "Data center console"), ["aidc.switch", "d2.open", "d2.replay", "d2.q_stuck", "d2.q_gpu", "d2.q_typo", "d2.q_mop", "d1.awaiting", "d1.scan_done", "d1.form", "d1.truth"], real=("d2.replay",))
    S.beats(p4, L("물리엔진 · 자율 대응", "Physics engine · autonomous response"), ["phys.start", "phys.inject", "phys.chain", "phys.tier1"])
    w = S.window("phys.chain", "phys.tier1")
    if w:
        evs = [e for e in S.trace.between(w[0] - 20000, w[1] + 2000, "zoro", "판단") if "자동 완화 조치 심사" in e["detail"].get("task", "")]
        if evs:
            S.model(10, p4, L("물리엔진 run_quick — 자율 완화 네 기준", "Physics run_quick — four auto-mitigation criteria"), L("글쓰기(10~20초)를 기다리지 않고 템플릿 판정 · 완화 제안을 먼저 내고, 초안은 뒤에서 다시 씁니다 (redraft)", "Decides and proposes mitigation from the template first instead of waiting for the writer (10–20 s); the draft is rewritten in the background (redraft)"),
                    criteria_painter(evs, L("기준마다 따로 물어 가장 낮은 확률을 씁니다. 하나라도 낮으면 사람이 실행합니다", "Each criterion is asked separately and the lowest probability wins; if any is low, a person runs it")), "m.criteria", active={"zoro_j"})
    S.beats(p4, L("물리엔진 · 자율 대응", "Physics engine · autonomous response"), ["phys.run", "phys.recovering", "phys.hud", "dphys.chain", "dphys.tier1", "dphys.recovering"])
    for sid in ["P5", "P6", "P7", "D5", "D6", "D7"]:
        k = sid.lower()
        ids = [i for i in (f"{k}.open", f"{k}.stage4", f"{k}.closed") if S.find(i)]
        for i in ids:
            CAP.setdefault(i, (SUMMARY_CUT[sid][0], "", SUMMARY_CUT[sid][1], ""))
        S.beats(p4, L("요약 컷", "Summary cut"), ids, lead=1.2)

    # PART 5 Zoro-1 판단
    S.card(divider_card("PART 5", L("결정 모델 Zoro-1 — 0-Generation 포인터 · SDS 축별 질문", "Decision model Zoro-1 — 0-generation pointer · per-axis SDS questions"), L("판정은 규칙이 정하고, 모델은 같은 질문을 시간 · 물리 · 센서 축으로 쪼개 물어 가장 낮은 확률을 확신도로 씁니다", "Rules set the verdict; the model asks the same question split by time, physics and sensor axes and uses the lowest probability as confidence")), 5, "div5")
    p5 = P(5, "Zoro-1 판단", "Zoro-1 verdict")
    for sid, stage_id, note in (("P2", "p2.stage4", L("P2: 상류 설비 파급 원인 — 규칙과 모델이 같은 쪽", "P2: upstream cause — rule and model agree")), ("D2", "d2.stage4", L("D2: CDU 밸브 → GPU 랙 — 발전소와 같은 모양", "D2: CDU valve → GPU rack — same shape as the plant")), ("P4", "p4.stage4", L("P4: 베어링 단독 고장 — 모델 이견", "P4: bearing single-unit fault — model dissent")), ("D4", "d4.stage4", L("D4: 배터리 열폭주 전조 — 모델 이견", "D4: battery thermal-runaway precursor — model dissent"))):
        S.beats(p5, L("Stage 4 상세", "Stage 4 detail"), [stage_id], lead=1.5)
        det = S.snap(f"{sid}-detail")
        if det and det.get("analysis", {}).get("substantive", {}).get("decided"):
            rule = (det.get("judgement") or {}).get("verdict") or det["analysis"].get("candidate")
            S.model(11, p5, note, L("막대 = 선택지 확률. 주황 = 규칙 판정 쪽", "Bars = option probabilities. Orange = the rule's verdict"), decision_painter(det, S.catalog, rule, L("정상 유지만 최솟값 ≥ 0.95 일 때 자동 확정 후보이고, 계통 이상 · 고위험은 교대감독이 확정합니다", "Only normal-hold is an auto-confirm candidate at minimum ≥ 0.95; system faults and high risk are confirmed by the shift supervisor")), f"m.decide.{sid}", active={"zoro_j"})
    w = S.window("p2.inject", "p2.archive")
    if w and S.trace:
        S.model(9, p5, L("P2 한 건에서 모델이 쓰인 순서", "Model use across the P2 incident"), L("판단 질문 4개는 몸통 블록을 올린 채 차례로, 초안은 같은 몸통에 임베딩 · 출력 묶음을 더해 씁니다", "Four verdict questions run back to back on the resident backbone; the draft adds the embedding and output groups to the same backbone"),
                gantt_painter(S.trace, w[0], w[1], L("P2 — 주입부터 보관까지", "P2 — from injection to archive")), "m.gantt")

    # PART 6 가드레일 · 봉인
    S.card(divider_card("PART 6", L("현장 단말 ↔ 교대감독 가드레일 · Stage 5 봉인", "Field terminal ↔ shift supervisor guardrails · Stage 5 seal"), L("AI 가 판정과 초안을 낸 뒤 사람이 절차를 건너뛰거나, 현장 확인 없이 닫거나, 고위험을 혼자 결재하려 하면 규칙이 막습니다", "After the AI decides and drafts, rules stop people from skipping steps, closing without a field check, or approving high risk alone")), 5, "div6")
    p6 = P(6, "가드레일 · 봉인", "Guardrails · seal")
    S.beats(p6, L("관제 콘솔", "Control console"), ["p1.g2_block", "p1.recheck"])
    S.beats(p6, L("현장 단말", "Field terminal"), ["p1.stepper", "p1.sign_line", "p1.g1_retry", "p1.excuse", "p1.free_text", "p1.sent"])
    w = S.window("p1.free_text", "p1.sent")
    if w:
        ev = [e for e in S.trace.between(w[0] - 2000, w[1] + 30000, "zoro", "글쓰기") if "표준" in e["detail"].get("system", "") or "code" in e["detail"].get("system", "").lower()]
        if ev:
            e = ev[0]
            S.model(8, p6, L("자유 서술 소명 → 표준 코드 (Zoro-1 글쓰기)", "Free-text justification → standard code (Zoro-1 writer)"), L("현장 글을 정해진 소명 코드 중 하나로 분류합니다. 코드 밖의 답은 받지 않습니다", "Sorts the field text into one of the fixed justification codes; anything else is rejected"),
                    writer_painter({"aop_id": "", "summary": e["detail"].get("text", ""), "numbers": [], "problems": [], "attempts": 1, "by_model": True}, e, L("지시문: 현장 소명 → 표준 코드", "Instruction: field justification → standard code")), "m.excuse", active={"zoro_w"})
    S.beats(p6, L("관제 콘솔", "Control console"), ["p1.unmask", "p1.kiosk_done"])
    S.beats(p6, L("Stage 5 상세", "Stage 5 detail"), ["p2.stage5", "p2.g3_rubber", "p2.reject", "p2.g3_order", "p2.step2", "p2.step3", "p2.archive"])
    for sid, note in (("P2", L("AOP-103-FW02 원인 진단 요약 (발전소)", "AOP-103-FW02 root-cause summary (power plant)")), ("D2", L("EOP-103-CDU02 원인 진단 요약 (데이터센터)", "EOP-103-CDU02 root-cause summary (data center)"))):
        det = S.snap(f"{sid}-detail-final") or S.snap(f"{sid}-detail")
        if det and det.get("analysis", {}).get("draft"):
            dr = det["analysis"]["draft"]
            w = S.window(f"{sid.lower()}.inject" if sid == "P2" else "aidc.switch", f"{sid.lower()}.archive")
            ev = [e for e in S.trace.between(w[0], w[1], "zoro", "글쓰기") if "표준" not in e["detail"].get("system", "")] if w else []
            S.model(11, p6, note, L("같은 Zoro-1 몸통이 글쓰기로 바뀌어 도메인 용어로 씁니다", "The same Zoro-1 backbone switches to writing, in the domain's own terms"), writer_painter(dr, ev[-1] if ev else None, L("지시문: 도메인 전용 (발전소 AOP · 데이터센터 EOP)", "Instruction: domain-specific (power plant AOP · data center EOP)")), f"m.write.{sid}", active={"zoro_w"})
    S.beats(p6, L("Stage 5 상세", "Stage 5 detail"), ["p4.g3_rubber", "p4.false_positive", "p4.cosign", "p4.archive", "d2.g3_order", "d2.interlock", "d2.archive", "d4.g3_rubber", "d4.archive", "end.queue"])

    # 요약
    rows = []
    retake = False
    for sid in ["P1", "P2", "P3", "P3F", "P4", "P5", "P6", "P7", "D1", "D2", "D3", "D3F", "D4", "D5", "D6", "D7"]:
        det = S.snap(f"{sid}-detail") or S.snap(f"{sid}-detail-final")
        if det:
            v = (det.get("judgement") or {}).get("verdict") or det.get("analysis", {}).get("route", {}).get("verdict") or "-"
            c = (det.get("judgement") or {}).get("confidence")
            text = f"{det['id']} · {v}" + (f" · {L('확신도', 'confidence')} {c:.3f}" if isinstance(c, (int, float)) else "")
            # 다시 찍은 장면(뒤 녹화 폴더)은 앱을 새로 띄워 사건 번호가 처음부터입니다
            src_rec = next((r for r in reversed(S.recs) if r.snap(f"{sid}-detail") is not None or r.snap(f"{sid}-detail-final") is not None), None)
            if src_rec is not None and src_rec is not S.recs[0]:
                text += " *"
                retake = True
            rows.append((sid, text, TEXT))
        else:
            lst = S.snap(f"{sid}-list")
            if lst:
                rows.append((sid, L("사건을 열지 않고 처리 (대기열 기록)", "Handled without opening an incident (queue record)"), MUTED))
    stats = []
    for key, name in (("siglip2", "SigLIP2"), ("gocr_v", L("gocr 비전", "gocr vision")), ("gocr_l", L("gocr 언어", "gocr language")), ("zoro_j", L("Zoro-1 판단", "Zoro-1 verdict")), ("zoro_w", L("Zoro-1 글쓰기", "Zoro-1 writer"))):
        ev = [e for e in S.trace.work if e["key"] == key] if S.trace else []
        if ev:
            stats.append((name, L(f"{len(ev)}회 · 합계 {sum(e['ms'] for e in ev) / 1000:.1f} s", f"{len(ev)} calls · {sum(e['ms'] for e in ev) / 1000:.1f} s total")))
    notes = [
        L("판정은 규칙(Gate 1~3)이 정하고, 모델은 읽고 · 확률을 매기고 · 초안을 씁니다.", "Rules (Gates 1–3) decide; models read, score and draft."),
        L("고위험 · 계통 이상은 언제나 교대감독이 확정하고, 결재 · 가드레일 차단은 감사 기록과 SDS 봉인으로 남습니다.", "High risk and system faults are always confirmed by the shift supervisor; approvals and guardrail blocks stay in the audit log and SDS seal."),
    ]
    if retake:
        notes.append(L("* 다시 찍은 장면: 앱을 새로 띄워 찍어 사건 번호가 0001 부터입니다.", "* Re-taken scene: recorded in a fresh app session, so incident numbers restart at 0001."))
    S.card(summary_card(rows, stats, notes), 10, "summary")


# ───────── 출력 ─────────


def render(segs, out, stills):
    if stills:
        os.makedirs(stills, exist_ok=True)
        for i, s in enumerate(segs):
            n = max(2, int(s.shown * FPS))
            s.draw(int(n * 0.85), n).save(os.path.join(stills, f"{i:03d}-{s.name}.png"))
        print(f"stills: {len(segs)} → {stills}")
        return
    import imageio_ffmpeg

    ff = imageio_ffmpeg.get_ffmpeg_exe()
    os.makedirs(os.path.dirname(out), exist_ok=True)
    proc = subprocess.Popen([ff, "-y", "-loglevel", "error", "-f", "rawvideo", "-pix_fmt", "rgb24", "-s", f"{W}x{H}", "-r", str(FPS), "-i", "-", "-c:v", "libx264", "-preset", "veryfast", "-crf", "21", "-pix_fmt", "yuv420p", "-movflags", "+faststart", out], stdin=subprocess.PIPE)
    total = 0.0
    for i, s in enumerate(segs):
        n = max(2, int(round(s.shown * FPS)))
        for k in range(n):
            img = s.draw(k, n)
            proc.stdin.write(img.tobytes())
        total += n / FPS
        print(f"[{i + 1}/{len(segs)}] {s.name} {n / FPS:.1f}s (누적 {total:.0f}s)", flush=True)
    proc.stdin.close()
    proc.wait()
    print(f"written {out} · {total:.0f} s · {os.path.getsize(out) / 1e6:.1f} MB")


def main():
    global LANG, TR
    ap = argparse.ArgumentParser()
    ap.add_argument("--lang", default="ko")
    ap.add_argument("--rec", action="append", required=True)
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    LANG = a.lang
    TR = Translator(os.path.join(ROOT, "app", "src", "i18n", "en.json")) if LANG == "en" else (lambda s: s)
    recs = [Rec(os.path.join(ROOT, r) if not os.path.isabs(r) else r) for r in a.rec]
    trace = Trace([os.path.join(r.dir, "trace.jsonl") for r in recs])
    # 판정 코드 → 이름 (verdict_catalog 스냅숏)
    catalog = {}
    for r in recs:
        raw = r.snap("verdict_catalog")
        items = raw.items() if isinstance(raw, dict) else ((x.get("code"), x) for x in raw or [])
        for code, info in items:
            label = info.get("label") if isinstance(info, dict) else info
            if code and label:
                catalog[code] = label
    story = Story(recs, trace, catalog)
    build(story)
    render(story.segs, os.path.join(ROOT, a.out) if not os.path.isabs(a.out) else a.out, os.environ.get("STILLS"))


if __name__ == "__main__":
    main()
