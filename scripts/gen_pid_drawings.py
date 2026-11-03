"""합성 P&ID 마스터 도면 · Red-line 수기 수정 도면 · 현장 계기 점검표 생성기.

    python scripts/gen_pid_drawings.py [마스터 폴더] [현장 스캔 폴더] [시드]

기본값은 core/fixtures/drawings (마스터 도면, 저장소에 들어갑니다) 와 out/field (현장 스캔, 저장소 밖) 입니다.
설비 상자의 위치는 시드 대장(core/fixtures/synthetic_db.json)의 `bbox`(정규화 좌표)를 그대로 씁니다. 그래서 대장이 곧 정답 좌표입니다.

출력
  <마스터>/PID-10x-XXX.png · .json   마스터 도면과 정답 영역 (스캔 잡음 없음, 앱의 도면 뷰가 씁니다)
  <현장>/P3_redline.png · .json        Red-line: PID-103-LAB 에 붉은 펜으로 MOC 번호 · 새 경보 한계 · 서명
  <현장>/P3F_redline.png · .json       같은 Red-line 의 위조 수치 (35.0 MPa)
  <현장>/P1_gauge.png · .json          현장 계기 점검표 (102동 주증기 압력 26.0 MPa, 손글씨)

정답 영역의 kind 는 zoro-gravity 의 합성 서식과 같은 이름을 씁니다.
  title  표제란, value  읽어야 하는 값(설비 상자 · 도면 간 연결 · 수기 기입), table  운전 기준치 일람표,
  label  배관 이름처럼 값 옆의 고정 인쇄, guide  고정 범례(읽지 않아도 됨), sign  서명 · 직인
손글씨 글꼴이 없어서 붓 글꼴(궁서)을 글자마다 흔들어 찍습니다. 실제 사람 · 설비가 아닙니다.
"""

import json
import math
import os
import random
import sys

import numpy as np
from PIL import Image, ImageDraw, ImageFilter, ImageFont

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SEED = os.path.join(ROOT, "core", "fixtures", "synthetic_db.json")

FONT_DIR = r"C:\Windows\Fonts" if os.name == "nt" else "/usr/share/fonts"
GOTHIC = os.path.join(FONT_DIR, "malgun.ttf")
BOLD = os.path.join(FONT_DIR, "malgunbd.ttf")
SERIF = os.path.join(FONT_DIR, "batang.ttc")
GULIM = os.path.join(FONT_DIR, "gulim.ttc")

DW, DH = 2000, 1400  # 도면 (가로)
PW, PH = 1240, 1754  # 점검표 (A4 세로 150 DPI)
INK = (20, 20, 25)
BLUE = (25, 35, 120)
RED = (205, 25, 25)
GRAY = (130, 130, 135)
SHADE = (236, 238, 241)

TITLE_BOX = (0.030, 0.020, 0.970, 0.095)
TABLE_BOX = (0.035, 0.665, 0.660, 0.870)
GUIDE_BOX = (0.035, 0.895, 0.660, 0.975)
SIGN_BOX = (0.720, 0.860, 0.970, 0.975)

# 설비 상자 격자 (synthetic_db.json 의 bbox 와 같은 값). 배관은 상자를 가로지르지 않고 상자 사이 통로로만 지나갑니다.
COL_X = [0.140, 0.325, 0.510, 0.695]
COL_W = 0.165
ROW_Y = [0.130, 0.310, 0.490]
ROW_H = 0.125
OPC_IN_X1 = 0.118  # 왼쪽 인입 점프 상자의 오른쪽 끝
OPC_OUT_X0 = 0.882  # 오른쪽 점프 상자의 왼쪽 끝
# 열 통로 j 는 j 번째 열의 왼쪽 (j = 4 는 마지막 열의 오른쪽, 점프 상자 앞). 폭 0.02 (40 px)
COL_GUTTER = [(OPC_IN_X1 + COL_X[0]) / 2] + [(COL_X[j - 1] + COL_W + COL_X[j]) / 2 for j in range(1, 4)] + [(COL_X[3] + COL_W + OPC_OUT_X0) / 2]
# 행 통로 i 는 i 번째 행의 위 (i = 3 은 마지막 행의 아래, 일람표 앞). 높이 0.035 ~ 0.055
ROW_GUTTER = [(TITLE_BOX[3] + ROW_Y[0]) / 2] + [(ROW_Y[i - 1] + ROW_H + ROW_Y[i]) / 2 for i in range(1, 3)] + [(ROW_Y[2] + ROW_H + TABLE_BOX[1]) / 2]
LANE_X = 7 / DW  # 같은 열 통로를 지나는 선끼리 띄우는 간격
LANE_Y = 10 / DH

MEDIUM_STYLE = {
    "power": ("dash", 2),
    "gas": ("dashdot", 3),
    "draft": ("dashdot", 3),
    "oil": ("dot", 3),
    "shaft": ("solid", 2),
}

GUIDE_LINES = [
    "1. 모든 설비 식별 코드는 KKS 표준 규격을 따른다. 센서 태그의 C 다음 글자는 측정 종류를 뜻한다.",
    "2. 실선은 급수 · 증기, 점선은 전력, 일점쇄선은 연소가스 · 통풍, 짧은 점선은 윤활유 계통을 나타낸다.",
    "3. 이 도면은 합성 시험용이며 실제 발전소 설비와 관계가 없다. 무단 복제를 금한다.",
]


def font(path, size, index=0):
    return ImageFont.truetype(path, size, index=index)


class Canvas:
    def __init__(self, w, h, rng):
        self.w, self.h = w, h
        self.rng = rng
        self.img = Image.new("RGB", (w, h), (255, 255, 255))
        self.d = ImageDraw.Draw(self.img)
        self.gt = []

    def px(self, box):
        x0, y0, x1, y1 = box
        return (x0 * self.w, y0 * self.h, x1 * self.w, y1 * self.h)

    def add(self, kind, box, text=""):
        self.gt.append({"kind": kind, "box": [int(round(v)) for v in box], "text": text})

    def text(self, x, y, s, size=20, path=GOTHIC, fill=INK, kind=None):
        f = font(path, size)
        self.d.text((x, y), s, font=f, fill=fill)
        l, t, r, b = self.d.textbbox((x, y), s, font=f)
        if kind:
            self.add(kind, (l, t, r, b), s)
        return r, b

    def width(self, s, size, path=GOTHIC):
        return self.d.textlength(s, font=font(path, size))

    def hand(self, x, y, s, size=30, color=BLUE, kind="value"):
        """글자마다 크기 · 기울기 · 위치를 흔들어 찍는 손글씨"""
        rng = self.rng
        cx = x
        tilt = rng.uniform(-2.5, 2.5)
        x0, y0, x1, y1 = x, y, x, y
        for ch in s:
            if ch == " ":
                cx += size * 0.45
                continue
            sz = int(size * rng.uniform(0.92, 1.1))
            f = font(SERIF, sz, index=2)  # 궁서
            pad = sz // 2
            tile = Image.new("RGBA", (sz * 2, sz * 2), (0, 0, 0, 0))
            ImageDraw.Draw(tile).text((pad // 2, pad // 2), ch, font=f, fill=color + (235,))
            tile = tile.rotate(rng.uniform(-7, 7) + tilt, resample=Image.BICUBIC)
            bbox = tile.getbbox()
            if bbox is None:
                cx += sz * 0.6
                continue
            drop = bbox[1] - pad // 2
            tile = tile.crop(bbox)
            py = int(y + drop + rng.uniform(-3, 3))
            self.img.paste(tile, (int(cx), py), tile)
            x1 = max(x1, cx + tile.width)
            y0, y1 = min(y0, py), max(y1, py + tile.height)
            cx += tile.width + rng.uniform(1, 6)
        if kind:
            self.add(kind, (x0, y0, x1, y1), s)
        return x1, y1

    def scribble(self, x, y, w, h, color=INK, kind="sign", text=""):
        """곡선을 겹친 서명"""
        rng = self.rng
        pts = []
        for _ in range(rng.randint(3, 5)):
            a = (x + rng.uniform(0, w), y + rng.uniform(0, h))
            b = (x + rng.uniform(0, w), y + rng.uniform(0, h))
            c = (x + rng.uniform(0, w), y + rng.uniform(0, h))
            for t in np.linspace(0, 1, 40):
                pts.append((((1 - t) ** 2) * a[0] + 2 * (1 - t) * t * b[0] + t * t * c[0], ((1 - t) ** 2) * a[1] + 2 * (1 - t) * t * b[1] + t * t * c[1]))
        self.d.line(pts, fill=color, width=3)
        if kind:
            self.add(kind, (x, y, x + w, y + h), text)

    def stamp(self, cx, cy, name, r=40, kind="sign"):
        self.d.ellipse((cx - r, cy - r, cx + r, cy + r), outline=RED, width=4)
        f = font(BOLD, 24)
        tw = self.d.textlength(name[:3], font=f)
        self.d.text((cx - tw / 2, cy - 15), name[:3], font=f, fill=RED)
        if kind:
            self.add(kind, (cx - r, cy - r, cx + r, cy + r), name)

    def line(self, pts, style="solid", width=3, fill=INK):
        if style == "solid":
            self.d.line(pts, fill=fill, width=width)
            return
        pattern = {"dash": [18, 10], "dot": [4, 8], "dashdot": [22, 7, 4, 7]}[style]
        for (ax, ay), (bx, by) in zip(pts, pts[1:]):
            length = math.hypot(bx - ax, by - ay)
            if length == 0:
                continue
            ux, uy = (bx - ax) / length, (by - ay) / length
            pos, k, draw = 0.0, 0, True
            while pos < length:
                seg = min(pattern[k % len(pattern)], length - pos)
                if draw:
                    self.d.line([(ax + ux * pos, ay + uy * pos), (ax + ux * (pos + seg), ay + uy * (pos + seg))], fill=fill, width=width)
                pos += seg
                k += 1
                draw = not draw

    def arrow(self, x, y, direction, size=14, fill=INK):
        if direction == "right":
            pts = [(x, y), (x - size, y - size * 0.6), (x - size, y + size * 0.6)]
        elif direction == "left":
            pts = [(x, y), (x + size, y - size * 0.6), (x + size, y + size * 0.6)]
        elif direction == "down":
            pts = [(x, y), (x - size * 0.6, y - size), (x + size * 0.6, y - size)]
        else:
            pts = [(x, y), (x - size * 0.6, y + size), (x + size * 0.6, y + size)]
        self.d.polygon(pts, fill=fill)

    def tag(self, cx, cy, s, size=15, path=GOTHIC, fill=GRAY, kind=None):
        """배관 이름. 흰 바탕을 깔아 그 자리의 선을 끊고 씁니다 (P&ID 의 선 위 이름표). (cx, cy) 가 글 가운데입니다."""
        f = font(path, size)
        l, t, r, b = self.d.textbbox((0, 0), s, font=f)
        x, y = cx - (r - l) / 2 - l, cy - (b - t) / 2 - t
        self.d.rectangle((x + l - 3, y + t - 2, x + r + 3, y + b + 2), fill=(255, 255, 255))
        self.d.text((x, y), s, font=f, fill=fill)
        if kind:
            self.add(kind, (x + l, y + t, x + r, y + b), s)

    def vtext(self, cx, cy, s, size=13, path=GOTHIC, fill=GRAY, kind=None):
        """세로 통로에 90도 돌려 쓰는 이름표 (아래에서 위로 읽음, 흰 바탕). (cx, cy) 가 글 가운데입니다."""
        f = font(path, size)
        l, t, r, b = self.d.textbbox((0, 0), s, font=f)
        tile = Image.new("RGBA", (r - l + 4, b - t + 4), (255, 255, 255, 255))
        ImageDraw.Draw(tile).text((2 - l, 2 - t), s, font=f, fill=fill + (255,))
        tile = tile.rotate(90, expand=True)
        x, y = int(cx - tile.width / 2), int(cy - tile.height / 2)
        self.img.paste(tile, (x, y), tile)
        if kind:
            self.add(kind, (x, y, x + tile.width, y + tile.height), s)


def cell(box):
    """정규화 상자의 (행, 열). 왼쪽 인입 점프 상자는 열 -1, 오른쪽 점프 상자는 열 4 입니다."""
    row = min(range(3), key=lambda i: abs(box[1] - ROW_Y[i]))
    if box[2] <= OPC_IN_X1 + 1e-6:
        return row, -1
    if box[0] >= OPC_OUT_X0 - 1e-6:
        return row, 4
    return row, min(range(4), key=lambda j: abs(box[0] - COL_X[j]))


class Router:
    """상자 사이 통로로만 가는 직각 배관 경로. 같은 통로를 여러 선이 지나가면 차례로 옆 줄(0, +1, -1, +2, -2)로 띄웁니다."""

    def __init__(self):
        self.cols = {}
        self.rows = {}
        self.sides = {}

    def _side(self, box, side):
        step = 16 / DH if side in ("left", "right") else 16 / DW
        return self._lane(self.sides, (tuple(box), side), step)

    @staticmethod
    def _lane(use, key, step):
        n = use.get(key, 0)
        use[key] = n + 1
        k = ((n + 1) // 2) * (1 if n % 2 else -1)
        k = max(-2, min(2, k))
        return k * step

    def route(self, a, b):
        """정규화 상자 a → b 의 꺾은선. (점들, 화살 방향, 행 통로를 지나는 가로 구간 또는 None)"""
        (ra, ca), (rb, cb) = cell(a), cell(b)
        acx, acy = (a[0] + a[2]) / 2, (a[1] + a[3]) / 2
        bcy = (b[1] + b[3]) / 2
        if ca == cb and abs(ra - rb) == 1:  # 바로 위아래 칸: 행 통로를 곧게 건넙니다
            x = acx + self._side(a, "bottom" if rb > ra else "top")
            if rb > ra:
                return [(x, a[3]), (x, b[1])], "down", None
            return [(x, a[1]), (x, b[3])], "up", None
        if cb > ca:
            g1, g2 = ca + 1, cb
            sides, arrow = ("right", "left"), "right"
        elif cb < ca:
            g1, g2 = ca, cb + 1
            sides, arrow = ("left", "right"), "left"
        else:  # 같은 열, 한 칸 이상 떨어짐: 오른쪽 열 통로로 내려가 오른쪽에서 들어갑니다
            g1 = g2 = ca + 1
            sides, arrow = ("right", "right"), "left"
        # 한 상자의 같은 변에 선이 여럿 붙으면 붙는 자리를 위아래로 벌립니다 (들고 나는 화살이 겹치지 않게).
        start = (a[2] if sides[0] == "right" else a[0], acy + self._side(a, sides[0]))
        end = (b[2] if sides[1] == "right" else b[0], bcy + self._side(b, sides[1]))
        x1 = COL_GUTTER[g1] + self._lane(self.cols, g1, LANE_X)
        if g1 == g2:  # 한 열 통로 안에서 꺾습니다
            return [start, (x1, start[1]), (x1, end[1]), end], arrow, None
        x2 = COL_GUTTER[g2] + self._lane(self.cols, g2, LANE_X)
        gi = ra if rb < ra else ra + 1  # a 행 바로 아래 통로 (b 가 위 행이면 바로 위 통로)
        y = ROW_GUTTER[gi] + self._lane(self.rows, gi, LANE_Y)
        return [start, (x1, start[1]), (x1, y), (x2, y), (x2, end[1]), end], arrow, ((x1, y), (x2, y))


def short_tag(tag):
    """센서 태그의 끝 다섯 글자 (예: 10LAB20CG102 → CG102)"""
    return tag[7:]


def fmt(v):
    return f"{v:,.1f}" if abs(v) >= 1000 else f"{v:.1f}" if abs(v) >= 1 or v == 0 else f"{v:.2f}"


class Plant:
    def __init__(self, db):
        self.db = db
        self.eq = {e["kks"]: e for e in db["equipment"]}
        self.sectors = {s["key"]: s for s in db["sectors"]}
        self.floors = {f["code"]: f for f in db["floors"]}
        self.drawings = {d["id"]: d for d in db["drawings"]}
        self.people = {p["id"]: p for p in db["personnel"]}

    def location(self, e):
        s = self.sectors[e["sector"]]
        f = self.floors[e["floor"]]
        if e["floor"] == "OUT":
            return f"{s['building']} 옥외"
        el = f["elevation_m"]
        return f"{s['building']} {f['code']} (EL.{el:+.1f}m)"

    def sensors_of(self, kks):
        return [s for s in self.db["sensors"] if s["kks"] == kks]


def draw_master(plant, drawing_id, rng, value_overrides=None):
    """마스터 도면 한 장. `value_overrides` 는 태그별로 상자에 적을 현재값입니다 (없으면 정상치)."""
    c = Canvas(DW, DH, rng)
    d = plant.drawings[drawing_id]
    sector = plant.sectors[d["sector"]]
    values = value_overrides or {}
    # 테두리
    c.d.rectangle((14, 14, DW - 14, DH - 14), outline=INK, width=3)
    c.d.rectangle((22, 22, DW - 22, DH - 22), outline=INK, width=1)

    # 표제란
    x0, y0, x1, y1 = c.px(TITLE_BOX)
    c.d.rectangle((x0, y0, x1, y1), outline=INK, width=3)
    cells = [f"도면번호: {d['id']} ({d['rev']})", d["title"], f"{sector['building']} {sector['label']} ({sector['kks']})"]
    widths = [0.30, 0.42, 0.28]
    cx = x0
    for text, share in zip(cells, widths):
        w = (x1 - x0) * share
        size = 28
        while c.width(text, size, BOLD) > w - 30 and size > 16:
            size -= 1
        c.text(cx + (w - c.width(text, size, BOLD)) / 2, y0 + (y1 - y0 - size * 1.3) / 2, text, size, BOLD)
        cx += w
        if cx < x1 - 2:
            c.d.line((cx, y0, cx, y1), fill=INK, width=2)
    c.add("title", (x0, y0, x1, y1), " | ".join(cells))

    on_sheet = [e for e in plant.db["equipment"] if e["drawing"] == drawing_id]
    for e in on_sheet:
        bx0, by0, bx1, by1 = c.px(e["bbox"])
        c.d.rectangle((bx0, by0, bx1, by1), fill=(255, 255, 255), outline=INK, width=3)
        lines = [(f"KKS: {e['kks']}", 21, BOLD), (e["name"], 21, GOTHIC), (f"위치: {plant.location(e)}", 16, GOTHIC)]
        for s in plant.sensors_of(e["kks"])[:2]:
            v = values.get(s["tag"], s["normal"])
            lines.append((f"{short_tag(s['tag'])} {s['label']} {fmt(v)} {s['unit']}", 16, GOTHIC))
        ty = by0 + 10
        text_parts = []
        for text, size, path in lines:
            while c.width(text, size, path) > (bx1 - bx0) - 22 and size > 12:
                size -= 1
            c.text(bx0 + 11, ty, text, size, path)
            text_parts.append(text)
            ty += size * 1.45
        c.add("value", (bx0, by0, bx1, by1), "\n".join(text_parts))

    # 같은 도면 안의 배관 · 전력선. 상자 사이 통로로만 가고, 앱 오버레이가 같은 선을 쓰도록 경로를 JSON 에 남깁니다.
    router = Router()
    norm = {e["kks"]: e["bbox"] for e in on_sheet}
    c.pipes = []

    def draw_pipe(edge, a_box, b_box, via=None):
        style, width = MEDIUM_STYLE.get(edge["medium"], ("solid", 4))
        pts, arrow, run = router.route(a_box, b_box)
        px_pts = [(x * DW, y * DH) for x, y in pts]
        c.line(px_pts, style, width)
        c.arrow(*px_pts[-1], arrow)
        pipe = {"from": edge["from"], "to": edge["to"], "points": [[round(x, 4), round(y, 4)] for x, y in pts]}
        if via:
            pipe["via"] = via
        c.pipes.append(pipe)
        return px_pts, run

    placed = []

    def free(rect):
        """이미 쓴 배관 이름과 겹치면 쓰지 않습니다 (모선처럼 한 통로에 같은 이름이 여러 줄 지나갈 때)."""
        if any(r[0] < rect[2] and rect[0] < r[2] and r[1] < rect[3] and rect[1] < r[3] for r in placed):
            return False
        placed.append(rect)
        return True

    for edge in plant.db["edges"]:
        a, b = edge["from"], edge["to"]
        if a not in norm or b not in norm:
            continue
        px_pts, run = draw_pipe(edge, norm[a], norm[b])
        label = edge["label"]
        if run:  # 행 통로를 지나는 가로 구간 가운데 위
            (x1, y), (x2, _) = run
            w = c.width(label, 15, GOTHIC)
            mx, my = (x1 + x2) / 2 * DW, y * DH
            if free((mx - w / 2, my - 10, mx + w / 2, my + 10)):
                c.tag(mx, my, label, kind="label")
        elif len(px_pts) == 2:  # 바로 위아래 칸: 선 오른쪽, 행 통로 윗머리 (가운데는 가로 배관이 지나갑니다)
            lx, ly = px_pts[0][0] + 8, min(px_pts[0][1], px_pts[1][1]) + 3
            if free((lx, ly, lx + c.width(label, 15, GOTHIC), ly + 19)):
                c.text(lx, ly, label, 15, GOTHIC, fill=GRAY, kind="label")
        else:  # 열 통로 안에서 꺾음: 세로 구간 옆이나 위에 돌려 씁니다
            x = px_pts[1][0]
            top, bottom = sorted((px_pts[1][1], px_pts[2][1]))
            length = c.width(label, 13, GOTHIC)
            if bottom - top >= length + 16:
                gutter = min(COL_GUTTER, key=lambda g: abs(g * DW - x)) * DW
                cx, cy = (x - 10 if x >= gutter else x + 10), (top + bottom) / 2
            else:
                cx, cy = x, top - 8 - length / 2
            if free((cx - 9, cy - length / 2, cx + 9, cy + length / 2)):
                c.vtext(cx, cy, label, kind="label")

    # 도면 간 점프 커넥터 (OPC)
    for con in plant.db["connectors"]:
        src, dst = plant.eq[con["from"]], plant.eq[con["to"]]
        edge = next(e for e in plant.db["edges"] if e["from"] == con["from"] and e["to"] == con["to"])
        if src["drawing"] == drawing_id:
            box, other, kind = con["out_bbox"], dst, "out"
        elif dst["drawing"] == drawing_id:
            box, other, kind = con["in_bbox"], src, "in"
        else:
            continue
        ox0, oy0, ox1, oy1 = c.px(box)
        notch = 22
        if kind == "out":
            shape = [(ox0, oy0), (ox1 - notch, oy0), (ox1, (oy0 + oy1) / 2), (ox1 - notch, oy1), (ox0, oy1)]
            head = f"점프 → {other['drawing']}"
        else:
            shape = [(ox0 + notch, oy0), (ox1, oy0), (ox1, oy1), (ox0 + notch, oy1), (ox0, (oy0 + oy1) / 2)]
            head = f"{other['drawing']} → 인입"
        c.d.polygon(shape, fill=SHADE, outline=INK)
        lines = [(head, 15, BOLD), (plant.sectors[other["sector"]]["building"] + " " + other["name"], 14, GOTHIC), (f"({edge['label']})", 13, GOTHIC)]
        ty = oy0 + 8
        for text, size, path in lines:
            while c.width(text, size, path) > (ox1 - ox0) - 26 and size > 10:
                size -= 1
            c.text(ox0 + (14 if kind == "out" else notch + 4), ty, text, size, path)
            ty += size * 1.4
        c.add("value", (ox0, oy0, ox1, oy1), " ".join(t for t, _, _ in lines))
        # 상자와 잇는 선 (이름은 점프 상자에 적혀 있어 따로 쓰지 않습니다)
        if kind == "out":
            draw_pipe(edge, norm[con["from"]], box, via=con["id"])
        else:
            draw_pipe(edge, box, norm[con["to"]], via=con["id"])

    # 운전 기준치 일람표
    sensors = [s for s in plant.db["sensors"] if plant.eq[s["kks"]]["drawing"] == drawing_id]
    tx0, ty0, tx1, ty1 = c.px(TABLE_BOX)
    c.text(tx0, ty0 - 2, "운전 기준치 일람표", 20, BOLD)
    head_y = ty0 + 32
    header = ["설비코드", "센서태그", "항목", "정상치", "경보한계", "트립한계"]
    shares = [0.18, 0.18, 0.22, 0.13, 0.15, 0.14]
    rows = len(sensors) + 1
    row_h = min(34, (ty1 - head_y) / max(rows, 1))
    c.d.rectangle((tx0, head_y, tx1, head_y + row_h * rows), outline=INK, width=2)
    c.d.rectangle((tx0, head_y, tx1, head_y + row_h), fill=SHADE, outline=INK, width=2)
    xs = [tx0]
    for share in shares:
        xs.append(xs[-1] + (tx1 - tx0) * share)
    for x in xs[1:-1]:
        c.d.line((x, head_y, x, head_y + row_h * rows), fill=INK, width=1)
    table_text = [" | ".join(header)]
    for r in range(1, rows):
        c.d.line((tx0, head_y + row_h * r, tx1, head_y + row_h * r), fill=INK, width=1)
    for i, h in enumerate(header):
        c.text(xs[i] + 8, head_y + 6, h, 16, BOLD)
    for r, s in enumerate(sensors, start=1):
        def limit(*keys):
            for k in keys:
                if s.get(k) is not None:
                    return f"{fmt(s[k])} {s['unit']}"
            return "-"
        cells = [s["kks"], s["tag"], s["label"], f"{fmt(s['normal'])} {s['unit']}", limit("alarm_high", "alarm_low"), limit("trip_high", "trip_low")]
        for i, cell in enumerate(cells):
            size = 15
            while c.width(cell, size) > (xs[i + 1] - xs[i]) - 12 and size > 10:
                size -= 1
            c.text(xs[i] + 6, head_y + row_h * r + 7, cell, size)
        table_text.append(" | ".join(cells))
    c.add("table", (tx0, head_y, tx1, head_y + row_h * rows), "\n".join(table_text))

    # 고정 범례 (읽지 않아도 되는 안내문)
    gx0, gy0, gx1, gy1 = c.px(GUIDE_BOX)
    c.line([(gx0, gy0), (gx1, gy0), (gx1, gy1), (gx0, gy1), (gx0, gy0)], "dash", 1, fill=GRAY)
    gy = gy0 + 8
    for line_text in GUIDE_LINES:
        c.text(gx0 + 10, gy, line_text, 14, GOTHIC, fill=GRAY)
        gy += 20
    c.add("guide", (gx0, gy0, gx1, gy1), "\n".join(GUIDE_LINES))

    # 서명란
    sx0, sy0, sx1, sy1 = c.px(SIGN_BOX)
    signer = plant.people[d["signer"]]
    c.d.rectangle((sx0, sy0, sx1, sy1), outline=INK, width=2)
    c.text(sx0 + 12, sy0 + 10, f"기술책임자: {signer['name']} ({signer['id']})", 18, BOLD, kind="label")
    c.text(sx0 + 12, sy1 - 34, f"검토일 {d['issued']}", 16, GOTHIC, kind="label")
    c.scribble(sx0 + 250, sy0 + 40, 170, 60, text=signer["name"])
    return c


def redline(plant, rng, new_value):
    """PID-103-LAB 위에 붉은 펜으로 고친 Red-line 도면"""
    c = draw_master(plant, "PID-103-LAB", rng)
    moc = next(m for m in plant.db["moc"] if m["moc_no"] == "MOC-2026-1104")
    engineer = plant.people[moc["approved_by"]]
    # 일람표에서 토출 압력 줄의 경보한계를 긋고 새 값을 적습니다.
    table = next(g for g in c.gt if g["kind"] == "table")
    rows = table["text"].split("\n")
    row = next(i for i, r in enumerate(rows) if r.split(" | ")[1] == moc["tag"])
    tx0, ty0, tx1, ty1 = table["box"]
    row_h = (ty1 - ty0) / len(rows)
    xs = [tx0]
    for share in [0.18, 0.18, 0.22, 0.13, 0.15, 0.14]:
        xs.append(xs[-1] + (tx1 - tx0) * share)
    ry = ty0 + row_h * row + row_h / 2
    c.d.line((xs[4] + 4, ry + 2, xs[5] - 6, ry - 3), fill=RED, width=4)
    c.hand(xs[5] - 70, ty0 + row_h * row - 22, f"{new_value:.1f}", 26, RED)
    # 펌프 상자를 구름으로 두르고 빈 자리에 변경 내용을 적습니다.
    eq = plant.eq[moc["kks"]]
    bx0, by0, bx1, by1 = c.px(eq["bbox"])
    for k in range(2):
        c.d.rounded_rectangle((bx0 - 12 - k * 3, by0 - 12 - k * 3, bx1 + 12 + k * 3, by1 + 12 + k * 3), radius=26, outline=RED, width=3)
    # 메모는 3행 왼쪽의 빈 칸 안에 씁니다. 행 사이 통로에는 배관이 지나가서 글과 선이 겹칩니다.
    nx0, ny0 = c.px((COL_X[0], ROW_Y[2] + 0.012, 0.0, 0.0))[:2]
    note = [moc["moc_no"], f"{moc['kks']} 토출압력 경보", f"{moc['old']:.1f} MPa → {new_value:.1f} MPa", f"2026-11-04 {engineer['name']}"]
    y = ny0
    x_end = nx0
    start = len(c.gt)
    for line_text in note:
        x1, y1 = c.hand(nx0, y, line_text, 30, RED, kind=None)
        x_end = max(x_end, x1)
        y = y1 + 10
    c.add("value", (nx0 - 6, ny0 - 6, x_end + 6, y), "\n".join(note))
    del c.gt[start + 1 :]
    c.scribble(x_end + 30, ny0 + 30, 160, 60, color=RED, text=engineer["name"])
    c.stamp(x_end + 260, ny0 + 70, engineer["name"], r=42)
    # 수기 메모 근처로 화살표
    c.line([(x_end + 10, ny0 + 20), (bx0 - 20, by1 + 16)], "solid", 3, fill=RED)
    return c, {"moc_no": moc["moc_no"], "kks": moc["kks"], "tag": moc["tag"], "old": moc["old"], "new": new_value, "unit": moc["unit"], "worker": engineer["id"], "drawing": "PID-103-LAB", "rev": "Rev.3"}


def gauge_card(plant, rng, scan):
    """현장 계기 점검표 (손글씨 값 + 아날로그 게이지)"""
    c = Canvas(PW, PH, rng)
    worker = plant.people[scan["worker"]]
    eq = plant.eq[scan["kks"]]
    sector = plant.sectors[scan["sector"]]
    title = "현장 계기 점검표"
    f = font(BOLD, 44)
    tw = c.d.textlength(title, font=f)
    c.text((PW - tw) / 2, 90, title, 44, BOLD, kind="title")
    c.text(90, 165, "제1발전소 1호기 · 현장 순시 기록 (합성)", 20, GOTHIC, fill=GRAY, kind="label")
    rows = [
        ("점검 일시", "2026-11-04 14:21:05"),
        ("점검자", worker["name"]),
        ("사번", worker["id"]),
        ("관할 섹터", f"{sector['building']} {sector['label']}"),
        ("설비 KKS", scan["kks"]),
        ("센서 태그", scan["tag"]),
        ("현장 실측치", f"{scan['value']:.1f}"),
        ("단위", scan["unit"]),
    ]
    x0, y0, label_w, value_w, row_h = 90, 230, 260, 800, 78
    c.d.rectangle((x0, y0, x0 + label_w + value_w, y0 + row_h * len(rows)), outline=INK, width=3)
    for i, (label, value) in enumerate(rows):
        y = y0 + row_h * i
        c.d.rectangle((x0, y, x0 + label_w, y + row_h), fill=SHADE, outline=INK, width=1)
        c.d.rectangle((x0 + label_w, y, x0 + label_w + value_w, y + row_h), outline=INK, width=1)
        c.text(x0 + 18, y + 24, label, 24, BOLD, kind="label")
        c.hand(x0 + label_w + 24, y + 18, value, 34, BLUE)
    # 아날로그 게이지
    gx, gy, r = 380, 1080, 190
    c.d.ellipse((gx - r, gy - r, gx + r, gy + r), outline=INK, width=5)
    vmax = 40.0
    for k in range(0, 41, 5):
        a = math.radians(225 - 270 * k / vmax)
        c.d.line((gx + (r - 30) * math.cos(a), gy - (r - 30) * math.sin(a), gx + (r - 6) * math.cos(a), gy - (r - 6) * math.sin(a)), fill=INK, width=4)
        lx, ly = gx + (r - 58) * math.cos(a), gy - (r - 58) * math.sin(a)
        c.text(lx - 12, ly - 12, str(k), 20, GOTHIC)
    a = math.radians(225 - 270 * scan["value"] / vmax)
    c.d.line((gx, gy, gx + (r - 40) * math.cos(a), gy - (r - 40) * math.sin(a)), fill=RED, width=6)
    c.d.ellipse((gx - 12, gy - 12, gx + 12, gy + 12), fill=INK)
    c.text(gx - 30, gy + 60, "MPa", 24, BOLD)
    c.add("label", (gx - r, gy - r, gx + r, gy + r), "게이지")
    c.text(640, 930, "현장 판독 메모", 24, BOLD, kind="label")
    c.hand(640, 980, "아날로그 게이지 정상", 30, BLUE)
    c.hand(640, 1040, "전송기 표시와 다름", 30, BLUE)
    # 안내문
    guide = [
        "1. 점검표는 순시 직후 작성하고 현장 단말로 주제어실에 전송한다.",
        "2. 현장 실측치는 아날로그 게이지 바늘이 가리키는 값을 그대로 적는다.",
        "3. 정비 작업이 있으면 안전작업허가서(PTW)와 잠금표지(LOTO)를 확인한다.",
    ]
    gy0 = 1330
    for i, line_text in enumerate(guide):
        c.text(90, gy0 + i * 30, line_text, 18, GOTHIC, fill=GRAY)
    c.add("guide", (90, gy0, 1150, gy0 + 90), "\n".join(guide))
    c.d.rectangle((760, 1480, 1150, 1620), outline=INK, width=2)
    c.text(780, 1490, "점검자 서명", 20, BOLD, kind="label")
    c.scribble(820, 1525, 260, 70, color=BLUE, text=worker["name"])
    return c


def scan_noise(a, rng):
    h, w = a.shape[:2]
    if rng.random() < 0.4:
        edge = rng.randint(6, 26)
        fade = np.linspace(rng.uniform(0.6, 0.85), 1.0, edge, dtype=np.float32)
        a[:, :edge] *= fade[None, :, None]
    if rng.random() < 0.25:
        y = rng.randint(h // 3, 2 * h // 3)
        a[y : y + 2] *= 0.88
    return a


def rotate_box(box, deg, w, h):
    cx, cy = w / 2, h / 2
    a = math.radians(-deg)
    xs, ys = [], []
    for x, y in ((box[0], box[1]), (box[2], box[1]), (box[0], box[3]), (box[2], box[3])):
        dx, dy = x - cx, y - cy
        xs.append(cx + dx * math.cos(a) - dy * math.sin(a))
        ys.append(cy + dx * math.sin(a) + dy * math.cos(a))
    return [int(min(xs)), int(min(ys)), int(max(xs)) + 1, int(max(ys)) + 1]


def scanned(canvas, rng):
    deg = rng.uniform(-0.8, 0.8)
    out = canvas.img.rotate(deg, resample=Image.BICUBIC, fillcolor=(255, 255, 255))
    out = out.filter(ImageFilter.GaussianBlur(rng.uniform(0.2, 0.5)))
    a = np.asarray(out).astype(np.float32)
    tint = np.array([rng.uniform(246, 255), rng.uniform(245, 254), rng.uniform(240, 252)], dtype=np.float32) / 255.0
    noise = np.random.default_rng(rng.randint(0, 1 << 30)).normal(0, 2.0, a.shape)
    a = scan_noise(a * tint + noise, rng)
    img = Image.fromarray(np.clip(a, 0, 255).astype(np.uint8))
    gt = [{**g, "box": rotate_box(g["box"], deg, canvas.w, canvas.h)} for g in canvas.gt]
    return img, gt, deg


def save(path, img, gt, extra):
    img.save(path)
    with open(os.path.splitext(path)[0] + ".json", "w", encoding="utf-8", newline="\n") as f:
        json.dump({**extra, "width": img.width, "height": img.height, "gt": gt}, f, ensure_ascii=False, indent=1)
        f.write("\n")


def a0_sheet(plant, a0, masters_dir):
    """A0 종합 도면 (Part 8-4). 마스터 도면들을 원장 배치(`a0_drawings`)대로 한 장에 앉힙니다.
    글자 크기는 마스터 그대로라 A0 전체 높이의 0.4 % 쯤입니다 (실제 A0 도면의 KKS 글씨 2.5 ~ 3.5 mm 와 같은 비율)."""
    W, H = a0["pixel_size"]
    c = Canvas(W, H, random.Random(a0["id"]))
    c.d.rectangle((20, 20, W - 20, H - 20), outline=INK, width=6)
    c.d.rectangle((40, 40, W - 40, H - 40), outline=INK, width=2)
    c.text(160, 260, f"{a0['title']}  ·  A0 {a0['size_mm'][0]} × {a0['size_mm'][1]} mm  ·  {a0['dpi']} DPI", 64, BOLD, kind="label")
    c.text(160, 380, "마스터 계통도 5장을 한 장에 모은 종합도입니다. 각 칸의 표제란은 원 도면의 것입니다.", 40, GOTHIC, fill=GRAY, kind="label")
    gt = []
    for sh in a0["sheets"]:
        img = Image.open(os.path.join(masters_dir, f"{sh['drawing']}.png")).convert("RGB")
        c.img.paste(img, (sh["x"], sh["y"]))
        c.d.rectangle((sh["x"] - 4, sh["y"] - 4, sh["x"] + img.width + 4, sh["y"] + img.height + 4), outline=INK, width=3)
    for e in plant.db["equipment"]:
        sh = next((s for s in a0["sheets"] if s["drawing"] == e["drawing"]), None)
        if sh is None:
            continue
        sw, shh = a0["sheet_px"]
        x0, y0, x1, y1 = e["bbox"]
        gt.append({"kks": e["kks"], "bbox": [round((sh["x"] + x0 * sw) / W, 5), round((sh["y"] + y0 * shh) / H, 5), round((sh["x"] + x1 * sw) / W, 5), round((sh["y"] + y1 * shh) / H, 5)]})
    # A0 자체 표제란 (오른쪽 아래, 비어 있는 칸)
    tx0, ty0, tx1, ty1 = 4846, 2630, 6846, 4030
    c.d.rectangle((tx0, ty0, tx1, ty1), outline=INK, width=4)
    c.text(tx0 + 60, ty0 + 80, f"도면번호: {a0['id']} ({a0['rev']})", 60, BOLD, kind="title")
    c.text(tx0 + 60, ty0 + 200, a0["title"], 48, BOLD, kind="label")
    c.text(tx0 + 60, ty0 + 300, f"격자 {a0['grid']['rows']} × {a0['grid']['cols']} 타일 · 겹침 {int(a0['grid']['overlap'] * 100)}%", 40, GOTHIC, fill=GRAY, kind="label")
    for i, line in enumerate(GUIDE_LINES):
        c.text(tx0 + 60, ty0 + 520 + i * 70, line, 30, GOTHIC, fill=GRAY, kind="guide")
    return c, gt


def main():
    masters = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "core", "fixtures", "drawings")
    field = sys.argv[2] if len(sys.argv) > 2 else os.path.join(ROOT, "out", "field")
    seed = int(sys.argv[3]) if len(sys.argv) > 3 else 20261104
    os.makedirs(masters, exist_ok=True)
    os.makedirs(field, exist_ok=True)
    db = json.load(open(SEED, encoding="utf-8"))
    plant = Plant(db)
    for d in db["drawings"]:
        c = draw_master(plant, d["id"], random.Random(f"{seed}/{d['id']}"))
        # 앱에 들어가는 파일이라 팔레트로 줄여 저장합니다.
        img = c.img.convert("P", palette=Image.ADAPTIVE, colors=64)
        save(os.path.join(masters, f"{d['id']}.png"), img, c.gt, {"drawing": d["id"], "rev": d["rev"], "pipes": c.pipes})
    # A0 종합 도면은 크고(수 MB) 마스터에서 다시 만들 수 있어 저장소에 넣지 않습니다 (out/a0).
    a0_dir = os.path.join(os.path.dirname(field), "a0")
    os.makedirs(a0_dir, exist_ok=True)
    for a0 in db.get("a0_drawings", []):
        c, gt = a0_sheet(plant, a0, masters)
        img = c.img.convert("P", palette=Image.ADAPTIVE, colors=64)
        save(os.path.join(a0_dir, f"{a0['id']}.png"), img, c.gt, {"drawing": a0["id"], "rev": a0["rev"], "equipment": gt, "grid": a0["grid"]})
    scenarios = json.load(open(os.path.join(ROOT, "core", "fixtures", "plant_scenarios.json"), encoding="utf-8"))["scenarios"]
    for s in scenarios:
        for scan in s["scans"]:
            rng = random.Random(f"{seed}/{s['id']}/{scan['kind']}")
            if scan["kind"] == "redline":
                c, truth = redline(plant, rng, scan["new"])
            else:
                c = gauge_card(plant, rng, scan)
                truth = {k: scan[k] for k in ("kks", "tag", "value", "unit", "worker", "sector")}
            img, gt, deg = scanned(c, rng)
            save(os.path.join(field, scan["image"]), img, gt, {"scenario": s["id"], "kind": scan["kind"], "skew": round(deg, 3), "truth": truth})
    print(f"마스터 도면 {len(db['drawings'])}장: {masters}")
    print(f"현장 스캔: {field}")
    print(f"A0 종합 도면: {a0_dir}")


if __name__ == "__main__":
    main()
