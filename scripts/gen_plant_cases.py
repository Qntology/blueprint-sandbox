"""합성 발전소 사건 시나리오 생성기. 시드 대장의 센서마다 1초 간격 시계열을 만들고 이상 징후를 얹습니다.

    python scripts/gen_plant_cases.py [출력 JSON] [시드]

기본 출력은 core/fixtures/plant_scenarios.json 입니다. 값은 모두 합성이며 실제 발전소 계측이 아닙니다.

시계열은 t = -60 ~ +90 초 (151 개 표본)이고, t = 0 은 사건의 기준 시각(`t0`)입니다.
이상 징후는 (시각, 값) 꼭짓점을 잇는 꺾은선으로 만들고, 꼭짓점 사이 기준값 구간에는 데드밴드보다 훨씬 작은 잡음을 얹습니다.
값 고정(동결)은 잡음까지 없앤 상수 구간입니다.

시나리오 (docs/powerplant-control-plan.md 7절 · Part 8-3)
  P1   터빈 주증기 압력 채널 A 드리프트 (2oo3 보팅 · 현장 게이지 26.0 MPa)
  P2   보일러 과열 · 2홉 상류 103동 급수조절밸브 18% 고착 (도면 점프)
  P3   주급수펌프 Red-line MOC (승인 원장 31.0 MPa 와 일치)
  P3F  같은 Red-line 의 위조 수치 35.0 MPa
  P4   고압터빈 베어링 진동 고 (상류 이상 없음, 고위험)
  P5   과열기 온도 순간 잡음 (슬루율 위반 스파이크)
  P6   윤활유 온도 계측값 고정 (40 초째 분산 0)
  P7   노내 압력 기울기 선행 경보 · 유인통풍기 실속
"""

import json
import os
import random
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SEED = os.path.join(ROOT, "core", "fixtures", "synthetic_db.json")
FROM_S = -60
LENGTH = 151  # -60 .. +90
# 처음 시나리오를 만든 뒤에 원장에 더한 센서. 같은 난수 흐름에서 뽑으면 뒤따르는 기존 센서 값이 모두 바뀌므로
# 센서마다 따로 시드를 잡은 난수로 뽑습니다 (기존 시나리오 값은 그대로).
LATE_TAGS = ["10LAB20CG103"]


def ramp(points, t):
    """꼭짓점 (시각, 값) 을 잇는 꺾은선의 t 에서의 값. 첫 꼭짓점 앞과 마지막 뒤는 끝값을 유지합니다."""
    if t <= points[0][0]:
        return points[0][1]
    for (t0, v0), (t1, v1) in zip(points, points[1:]):
        if t0 <= t <= t1:
            return v0 + (v1 - v0) * (t - t0) / (t1 - t0)
    return points[-1][1]


class Builder:
    def __init__(self, db, rng, late_seed=0):
        self.db = db
        self.rng = rng
        self.late_seed = late_seed
        self.sensors = {s["tag"]: s for s in db["sensors"]}

    def baseline(self, tag, value=None, rng=None):
        s = self.sensors[tag]
        v = s["normal"] if value is None else value
        sigma = s["deadband"] * 0.12
        rng = rng or self.rng
        return [round(v + rng.gauss(0.0, sigma), 3) for _ in range(LENGTH)]

    def all_normal(self):
        series = {tag: self.baseline(tag) for tag in self.sensors if tag not in LATE_TAGS}
        for tag in LATE_TAGS:
            if tag in self.sensors:
                series[tag] = self.baseline(tag, rng=random.Random(f"{self.late_seed}/{tag}"))
        return series

    def shape(self, tag, points, noise=True):
        """꼭짓점 꺾은선 + 작은 잡음. 꺾은선이 움직이는 구간에는 잡음을 줄입니다."""
        s = self.sensors[tag]
        sigma = s["deadband"] * 0.12 if noise else 0.0
        out = []
        for i in range(LENGTH):
            t = FROM_S + i
            out.append(round(ramp(points, t) + self.rng.gauss(0.0, sigma), 3))
        return out


def idx(t):
    return t - FROM_S


def scenario_p1(b):
    series = b.all_normal()
    tag = "10MAA10CP001"
    a = b.shape(tag, [(-2.0, 26.0), (4.0, 14.2), (90.0, 14.2)])
    ch_b = b.baseline(tag)
    ch_c = b.baseline(tag)
    # DCS 대표값은 2oo3 보팅의 가운데 값입니다.
    series[tag] = [round(sorted(x)[1], 3) for x in zip(a, ch_b, ch_c)]
    return {
        "id": "P1",
        "title": "터빈 주증기 압력 급락 (전송기 A 드리프트)",
        "story": "102동 고압터빈 주증기 압력 채널 A 만 14.2 MPa 로 떨어지고 B · C 와 현장 게이지는 26.0 MPa 입니다.",
        "t0": "2026-11-04T14:20:00+09:00",
        "now_s": 1,
        "alarm": {"tag": tag, "channel": "A", "at_s": 0.0, "value": 21.9, "kind": "low", "message": "주증기 압력 저 경보 (채널 A)"},
        "series": series,
        "channels": {tag: {"A": a, "B": ch_b, "C": ch_c}},
        "scans": [
            {"kind": "gauge_card", "at_s": 65, "sector": "turbine", "kks": "10MAA10ST001", "tag": tag, "value": 26.0, "unit": "MPa",
             "worker": "E205", "signed": True, "loto": False, "image": "P1_gauge.png"}
        ],
        "expect": {"verdict": "G2_SENSOR_DRIFT_ISOLATED", "drift_channel": "A", "awaits_field": True, "aop": "AOP-102-IC01"},
    }


def scenario_p2(b):
    series = b.all_normal()
    series["10LAB20CG102"] = b.shape("10LAB20CG102", [(0.0, 78.0), (1.0, 18.0), (90.0, 18.0)])
    series["10LAB20CF001"] = b.shape("10LAB20CF001", [(0.4, 1850.0), (3.0, 1180.0), (6.0, 1120.0), (90.0, 1118.0)])
    series["10LAB10CP001"] = b.shape("10LAB10CP001", [(1.6, 28.5), (2.6, 29.15), (5.0, 29.4), (90.0, 29.42)])
    series["10HAH10CT001"] = b.shape("10HAH10CT001", [(4.2, 610.0), (10.0, 631.0), (20.0, 637.5), (40.0, 638.0), (90.0, 638.2)])
    return {
        "id": "P2",
        "title": "보일러 과열 · 상류 급수조절밸브 고착",
        "story": "103동 2층 급수조절밸브가 18% 에서 고착되어 급수 유량이 줄고, 4초쯤 뒤 101동 과열기 출구 온도가 오릅니다.",
        "t0": "2026-11-04T14:20:00+09:00",
        "now_s": 9,
        "alarm": {"tag": "10HAH10CT001", "channel": None, "at_s": 8.3, "value": 625.4, "kind": "high", "message": "과열기 출구 증기 온도 고 경보"},
        "series": series,
        "channels": {},
        "scans": [],
        "expect": {"verdict": "G3_ADVERSE_UPSTREAM_CAUSE", "root_cause": "10LAB20AA102", "hops": 2, "connector": "OPC-103-101", "aop": "AOP-103-FW02"},
    }


def scenario_p3(b, forged):
    series = b.all_normal()
    # 임펠러 상향 교체 뒤 토출 압력이 29.4 MPa 로 올라 옛 경보 한계 29.0 을 넘은 상태입니다.
    series["10LAB10CP001"] = b.baseline("10LAB10CP001", 29.4)
    value = 35.0 if forged else 31.0
    return {
        "id": "P3F" if forged else "P3",
        "title": "주급수펌프 Red-line 변경 (위조 수치)" if forged else "주급수펌프 Red-line 변경 (MOC 승인)",
        "story": ("Red-line 에 적힌 새 경보 한계 35.0 MPa 가 승인 원장의 31.0 MPa 와 다릅니다." if forged
                  else "현장에서 수기로 고친 도면의 새 경보 한계 31.0 MPa 가 승인 원장과 같습니다."),
        "t0": "2026-11-04T14:30:00+09:00",
        "now_s": 0,
        "alarm": None,
        "series": series,
        "channels": {},
        "scans": [
            {"kind": "redline", "at_s": 0, "sector": "feedwater", "drawing": "PID-103-LAB", "rev": "Rev.3", "kks": "10LAB10AP001", "tag": "10LAB10CP001",
             "moc_no": "MOC-2026-1104", "old": 29.0, "new": value, "unit": "MPa", "worker": "E102", "signed": True, "loto": True,
             "image": "P3F_redline.png" if forged else "P3_redline.png"}
        ],
        "expect": ({"verdict": "G2_REJECT_CONTRADICTION_LOCK"} if forged else {"verdict": "G2_MOC_VERIFIED", "overlay": {"tag": "10LAB10CP001", "alarm_high": 31.0}}),
    }


def scenario_p4(b):
    series = b.all_normal()
    series["10MAD10CY001"] = b.shape("10MAD10CY001", [(0.0, 45.0), (3.0, 95.0), (6.0, 150.0), (9.0, 205.0), (14.0, 212.0), (90.0, 213.0)])
    return {
        "id": "P4",
        "title": "고압터빈 베어링 진동 고 (고위험)",
        "story": "102동 고압터빈 1번 베어링 진동이 212 µm 까지 오르고, 윤활유 · 주증기 등 상류에는 이상이 없습니다.",
        "t0": "2026-11-04T14:25:00+09:00",
        "now_s": 10,
        "alarm": {"tag": "10MAD10CY001", "channel": None, "at_s": 8.7, "value": 201.0, "kind": "high", "message": "터빈 베어링 진동 고 경보"},
        "series": series,
        "channels": {},
        "scans": [],
        "expect": {"verdict": "G3_VIOLATION_HIGH_RISK", "root_cause": None, "aop": "AOP-102-TB01", "high_risk": True},
    }


def scenario_p5(b):
    series = b.all_normal()
    spiked = list(series["10HAH10CT001"])
    spiked[idx(0)] = 810.0
    series["10HAH10CT001"] = spiked
    return {
        "id": "P5",
        "title": "과열기 온도 순간 잡음 (스파이크)",
        "story": "과열기 출구 온도가 1초 동안만 810 °C 로 튀었다가 돌아옵니다. 열역학적으로 불가능한 변화라 잡음으로 거릅니다.",
        "t0": "2026-11-04T14:40:00+09:00",
        "now_s": 2,
        "alarm": {"tag": "10HAH10CT001", "channel": None, "at_s": 0.0, "value": 810.0, "kind": "high", "message": "과열기 출구 증기 온도 고 경보"},
        "series": series,
        "channels": {},
        "scans": [],
        "expect": {"verdict": "G3_ELIGIBLE_NORMAL", "gate1": "G1_SPIKE_FILTERED", "aop": "AOP-COM-EMI01"},
    }


def scenario_p6(b):
    series = b.all_normal()
    frozen = list(series["10MAV10CT001"])
    for i in range(idx(-40), LENGTH):
        frozen[i] = 56.0
    series["10MAV10CT001"] = frozen
    return {
        "id": "P6",
        "title": "윤활유 온도 계측값 고정 (동결)",
        "story": "윤활유 출구 온도가 56.0 °C 에서 40초째 잡음조차 없이 멈춰 있고, 베어링 진동 등 하류는 정상입니다.",
        "t0": "2026-11-04T14:45:00+09:00",
        "now_s": 0,
        "alarm": {"tag": "10MAV10CT001", "channel": None, "at_s": -40.0, "value": 56.0, "kind": "high", "message": "윤활유 출구 온도 고 경보 (지속)"},
        "series": series,
        "channels": {},
        "scans": [],
        "expect": {"verdict": "G1_SENSOR_FLATLINE", "aop": "AOP-COM-IC02"},
    }


def scenario_p7(b):
    series = b.all_normal()
    series["10HLB10CE001"] = b.shape("10HLB10CE001", [(0.0, 360.0), (2.0, 300.0), (4.0, 285.0), (90.0, 282.0)])
    series["10HAD10CP101"] = b.shape("10HAD10CP101", [(2.0, -0.1), (10.0, 0.38), (30.0, 0.95), (60.0, 1.1), (90.0, 1.12)], noise=False)
    return {
        "id": "P7",
        "title": "노내 압력 선행 경보 · 유인통풍기 실속",
        "story": "105동 유인통풍기 전류가 떨어진 2초쯤 뒤 노내 압력이 빠르게 오릅니다. 경보 한계 전이지만 트립까지 30초 안이라 먼저 알립니다.",
        "t0": "2026-11-04T14:50:00+09:00",
        "now_s": 6,
        "alarm": {"tag": "10HAD10CP101", "channel": None, "at_s": 5.0, "value": 0.08, "kind": "pre_alarm", "message": "노내 압력 상승 기울기 선행 경보"},
        "series": series,
        "channels": {},
        "scans": [],
        "expect": {"verdict": "G3_ADVERSE_UPSTREAM_CAUSE", "trigger": "HIGH_SLOPE_PRE_ALARM", "root_cause": "10HLB10AN001", "aop": "AOP-105-DR01"},
    }


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else os.path.join(ROOT, "core", "fixtures", "plant_scenarios.json")
    seed = int(sys.argv[2]) if len(sys.argv) > 2 else 20261104
    db = json.load(open(SEED, encoding="utf-8"))
    scenarios = []
    for i, make in enumerate([scenario_p1, scenario_p2, lambda b: scenario_p3(b, False), lambda b: scenario_p3(b, True), scenario_p4, scenario_p5, scenario_p6, scenario_p7]):
        # 시나리오마다 따로 시드를 줘서 하나를 고쳐도 다른 시나리오 값이 바뀌지 않게 합니다.
        scenarios.append(make(Builder(db, random.Random(seed + i), late_seed=seed + i)))
    doc = {
        "version": 1,
        "note": "합성 사건 시나리오입니다. scripts/gen_plant_cases.py 가 만들었습니다. 실제 계측이 아닙니다.",
        "from_s": FROM_S,
        "period_s": 1,
        "length": LENGTH,
        "scenarios": scenarios,
    }
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        json.dump(doc, f, ensure_ascii=False, separators=(",", ":"))
        f.write("\n")
    print(f"{out}: 시나리오 {len(scenarios)}개, 센서 {len(db['sensors'])}개 x 표본 {LENGTH}개")


if __name__ == "__main__":
    main()
