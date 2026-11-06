# 시연 영상 (한국어 · English)

`영상.txt` 의 단일 마스터 기획(6파트)을 실제 앱 · 실제 모델로 찍고 편집한 영상입니다. 한국어 화면과 영어 화면을 따로 찍었습니다.

| 파일 | 화면 언어 | 녹화 폴더 |
| --- | --- | --- |
| `out/video/blueprint_master_e2e_ko.mp4` | 한국어 | `out/rec/master_ko` (+ `master_ko_p1`: P1 다시 찍음) |
| `out/video/blueprint_master_e2e_en.mp4` | English | `out/rec/master_en` |

`out/` 은 저장소에 넣지 않습니다 (영상 · 프레임이 커서). 아래 명령으로 다시 만듭니다.

## 구성

| 파트 | 내용 | 실제 화면 | 모델 장면 (실제 기록에서 그림) |
| --- | --- | --- | --- |
| 1 듀얼 도메인 · 3-Way | 발전소 ↔ AI 데이터센터 전환, 도면 · Mermaid · 마인드맵, A0 종합도, 현장 단말 | INTRO | — |
| 2 자연어 질의 | 동 · 층 · 종류, 조건 강등(105동 밸브 유인통풍기), 부정어, 코드 오타(10LABI0AA102), 유형 질의, 1위 → 세 뷰 동시 이동 | SEARCH | — |
| 3 도면 전처리 | 계기 점검표 · Red-line 스캔, Stage 1 & 2, 위조 잠금(P3F · D3F) | P1 · P3 · P3F · D3 · D3F | SigLIP2 영역 경쟁(제목 · 입력란 · 표 · 서명 / 안내문 · 빈 띠 버림), 델타 패치(마스터 식별 · 건너뜀), 전처리 Off · On 판독 시간, 심볼 그림 사전 |
| 4 N:N 인과 역추적 | P2 · D2 원인 경로와 Δt 라벨 · 타임머신 재생 · 차트 · 마이닝 · 고장 중 질의, 3중 진실 대조표(P1 · D1), 물리엔진 폐루프(PHYS · DPHYS), 잡음 · 동결 · 선행 경보(P5 ~ P7 · D5 ~ D7) | P2 · D2 · P1 · D1 · PHYS · DPHYS · P567 · D567 | 자율 완화 네 기준(최솟값) |
| 5 Zoro-1 판단 | Stage 4 상세 | P2 · D2 · P4 · D4 | 0-Generation 포인터: 사실 · 질문 · 축별 확률 · 최솟값(모델 이견 포함), P2 한 건의 모델 사용 순서 |
| 6 가드레일 · 봉인 | 현장 확인 전 확정 차단 · 재점검, 장갑 스테퍼 · 무효 서명 재요청(G1) · 자유 서술 소명, 실명 열람, 공동 결재 없이 확정 · 증빙 없는 반려 · 선행 절차 건너뛰기 차단, 인터록 3단계, 보관 · SDS 봉인 | P1 · P2 · P4 · D2 · D4 | 조치 절차서 초안(AOP · EOP)과 숫자 1:1 대조 |

영상의 숫자와 글은 모두 그 실행의 기록에서 옵니다.

- 화면: CDP 화면 캐스트 프레임 (`frames/c` 콘솔, `frames/f` 현장 단말)
- 장면 표시와 그때 화면에서 읽은 값(질의 1위 · "왜" · 차단 문구 · 보관 번호 …): `timeline.json`
- 백엔드 값(판단 질문 · 축별 확률 · 초안 · 대조 결과): `snaps/*.json` (`incident_detail` · `verdict_catalog`)
- 모델 기록(조각 계획 · 판독 글 · 판단 · 글쓰기 · 가중치 올림 · 내림): 앱을 `BLUEPRINT_TRACE=<녹화 폴더>/trace.jsonl BLUEPRINT_TRACE_TEXT=1` 로 띄워 남김
- 전처리 Off · On 시간과 심볼 사전은 측정 파일: `out/delta_pre_*.json` · `delta_base_*.json` · `delta_shift_*.json` · `symbol_probe.json` · `plant_probe*.json`

영어 영상의 모델 장면은 모델에 들어간 한국어 사실을 앱과 같은 번역 표(`app/src/i18n/en.json`)로 영어로 보여 주고, 그렇다고 적습니다.
현장 서류(스캔)와 gocr 가 읽은 원문은 두 영상 모두 한국어입니다 ([i18n.md](i18n.md)).

## 다시 만들기

```bash
# 1) 앱 (ROCm) — 디버그 포트 · 스캐너 폴더 · 모델 기록
scripts\rocm-env.bat
set CARGO_TARGET_DIR=target-rocm
set WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=9222
set BLUEPRINT_SCANNER_DIR=<스캐너 폴더>
set BLUEPRINT_TRACE=%CD%\out\rec\master_ko\trace.jsonl
set BLUEPRINT_TRACE_TEXT=1
cd app && npx tauri dev --release --no-watch --features rocm -- --no-default-features

# 2) 촬영 (앱 상태는 메모리라 언어마다 앱을 새로 띄웁니다)
node scripts/e2e/record.js --out out/rec/master_ko
node scripts/e2e/record.js --out out/rec/master_en --lang en
#    한 장면만 다시: --only P1 --out out/rec/master_ko_p1 (편집기가 실패한 장면을 뒤 폴더의 것으로 바꿉니다)

# 3) 편집
python scripts/e2e/edit_video.py --lang ko --rec out/rec/master_ko --rec out/rec/master_ko_p1 --out out/video/blueprint_master_e2e_ko.mp4
python scripts/e2e/edit_video.py --lang en --rec out/rec/master_en --out out/video/blueprint_master_e2e_en.mp4
#    STILLS=<폴더> 를 주면 구간마다 한 장씩만 저장합니다 (검토용)
```

현장 스캔 그림은 `python scripts/gen_pid_drawings.py` (+ `--domain aidc`)가 `out/field` 에 만듭니다.
