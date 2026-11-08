"""합성 데이터 한 번에 만들기: 시나리오 → 도면(마스터 · 현장 스캔 · A0) 순서로, 두 도메인을 차례로 만듭니다.

    python scripts/gen_all.py                  발전소 · 데이터센터 시나리오와 도면 (한국어)
    python scripts/gen_all.py --en             영어 화면용 마스터 도면도 함께 (--lang en)
    python scripts/gen_all.py --domain aidc    한 도메인만 (plant · aidc · both)
    python scripts/gen_all.py --dry-run        돌릴 명령만 보입니다

도면 생성기는 core/fixtures 의 시나리오를 읽으므로 시나리오를 먼저 만들어야 합니다. 만든 뒤에는 core/fixtures 아래 파일의
내용 해시를 전후로 견줘 바뀐 파일을 알립니다 (JSON 의 줄바꿈 CRLF · LF 차이는 같은 것으로 봄). core/fixtures 는 앱 · 예제에
컴파일되어 들어가므로(include_str! · include_bytes!), 바뀌었으면 앱과 probe 를 다시 빌드해야 합니다.
"""
import argparse
import hashlib
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
FIXTURES = os.path.join(ROOT, "core", "fixtures")


def digest_tree(folder):
    out = {}
    for base, _, files in os.walk(folder):
        for name in files:
            path = os.path.join(base, name)
            data = open(path, "rb").read()
            if name.endswith((".json", ".txt", ".md")):
                data = data.replace(b"\r\n", b"\n")
            out[os.path.relpath(path, ROOT)] = hashlib.sha256(data).hexdigest()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--domain", choices=["plant", "aidc", "both"], default="both")
    ap.add_argument("--en", action="store_true", help="영어 화면용 마스터 도면도 만듭니다")
    ap.add_argument("--dry-run", action="store_true")
    a = ap.parse_args()
    domains = ["plant", "aidc"] if a.domain == "both" else [a.domain]
    steps = []
    for d in domains:
        flag = [] if d == "plant" else ["--domain", d]
        steps.append([os.path.join("scripts", "gen_plant_cases.py"), *flag])
    for d in domains:
        flag = [] if d == "plant" else ["--domain", d]
        steps.append([os.path.join("scripts", "gen_pid_drawings.py"), *flag])
        if a.en:
            steps.append([os.path.join("scripts", "gen_pid_drawings.py"), *flag, "--lang", "en"])
    for s in steps:
        print("[gen]", "python", " ".join(s))
    if a.dry_run:
        return
    before = digest_tree(FIXTURES)
    for s in steps:
        r = subprocess.run([sys.executable, *s], cwd=ROOT)
        if r.returncode != 0:
            sys.exit(f"[gen] 실패: {' '.join(s)} (코드 {r.returncode})")
    after = digest_tree(FIXTURES)
    changed = sorted(k for k in set(before) | set(after) if before.get(k) != after.get(k))
    if changed:
        print(f"[gen] core/fixtures 에서 바뀐 파일 {len(changed)}개 (앱 · probe 를 다시 빌드하세요):")
        for k in changed:
            print("   ", k)
    else:
        print("[gen] core/fixtures 는 전과 같습니다 (현장 스캔 · A0 는 out/ 에 새로 썼습니다)")


if __name__ == "__main__":
    main()
