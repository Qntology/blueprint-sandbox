# UI · 절차 단순화 (개발 개선 기획안 0 ~ 3단계)

> 상태: 2026-11-09 개발 개선 기획안(UI 13개 · 절차 14개 후보, main d5a9ffd 기준)을 구현한 기록입니다. 0 ~ 2단계를 먼저 했고(커밋 35a40d2),
> 결정 칸이 비어 있던 3단계 항목(P1 · U9 체크리스트 · P4 · K2 · U7 · P6 · D4 장기)은 "전부 진행" 요청에 따라 기획안의 "하지 말 것"
> (인터록 · 공동 결재 · 증빙 조건을 풀지 않음, 현장에 내부 정보를 보내지 않음)을 지키는 쪽으로 정해 구현했습니다 (3절).
> 실측은 [verification.md](verification.md) 14 · 15절입니다. 앞 단계 기록: [overlay-console-plan.md](overlay-console-plan.md) (풀스크린 오버레이) · [i18n.md](i18n.md) · [search.md](search.md).

## 0. 한 줄 요약

같은 정보 · 같은 조작이 두세 곳에 있던 것을 한 곳으로 모았습니다. 결재는 오른쪽 결재 HUD 하나, 시계열 · SDS 는 허브 Drop-up 하나, 트립 잔여(TTF)는 타임머신 배지 하나입니다.
시연 조작은 🧪 시연 판으로 옮겼습니다. 현장 단말은 엔진이 쓰지 않는 섹터 화면을 없앴고, 서명 규칙은 보내기 전에 확인합니다.
화면이 백엔드를 부르는 횟수는 가만히 1분 96 → 24번, 타임머신 재생 10초 1,157 → 4번입니다.

## 1. 대조표

| # | 기획안 | 구현 | 파일 |
| --- | --- | --- | --- |
| U1 | 결재를 한 곳으로 | 결재 HUD 하나: 표준 조치 요약 · 공동 결재 · [재점검 지시] [조치확정], [더 보기] 에 반려 · 고위험 예외승인(오경보) · 근거 절차서 번호 · 증빙 장수 · 재점검 메모. Stage 5 서랍은 초안 · Factcheck · 인터록 상태 읽기 전용(값 입력 없음). 막힘 규칙은 HUD 한 벌 | `StagePanel.tsx` (`QuickApproval` · `Draft` · `InterlockPanel readOnly`), `RightRail.tsx` |
| U2 | 시계열 · SDS 판 하나로 | SDS 분석기 서랍(`Sds`)을 지우고 허브 Drop-up 하나로. 타임머신 끝 [📈 시계열 · SDS] 가 여닫고, 서랍에만 있던 EWMA 칸은 CAM-02 표로 옮김. 단계 줄의 CAM 단추도 이 하나로 합침. 설비 확대 창은 설비 범위라 그대로 | `StagePanel.tsx`, `TimeScrubber.tsx`, `cctv/Docks.tsx` |
| U3 | 같은 숫자는 한 번 | TTF 는 타임머신 배지에만 (HUD 머리 · CAM-02 머리 · 서랍에서 뺌, 서랍만 `trigger` 값 · "트립" 고정이던 불일치도 같이 사라짐). 대기 건수는 알약 · 대기열 제목, 팝오버 머리 · 상태 줄의 대기 · 보관 건수와 단계 줄의 "처리 완료" 를 뺌. 레인은 단계 카드에만 | `OfficerView.tsx`, `cctv/RightRail.tsx`, `StagePanel.tsx` |
| U4 | 시연 조작 분리 | 우상단 [🧪 시연 · 시험] 판: 시나리오 주입 · 물리엔진 · 세션 초기화. 결재 HUD 의 가드레일 시험 단추 3개(공동 결재 없이 · 선행 절차 건너뛰기 · 현장 확인 전)는 시연 모드에만. 설정의 "시연 · 시험 도구 보이기 · 숨기기" (이 PC 에 기억, 기본 보이기). 모델 관리는 관리 기능이라 설정에 그대로 | `OfficerView.tsx`, `AlarmQueue.tsx`, `StagePanel.tsx` |
| U5 | 판 여닫기 규칙 하나 | 여닫기 상태 8개 → 팝오버 1 (대기열 · 시연 · 설정) · Drop-up 1 (검색 · 차트) · 서랍 1 · 확대 창. ESC 는 확대 창 → 팝오버 → Drop-up → 서랍 → 검색 표시 → 설비 선택. 확정 뒤 보관 문서 서랍은 저절로 열지 않고 허브 알림의 [🗄️ 보관 문서 열기] 로 | `OfficerView.tsx` |
| U6 | 머리줄 정리 | 도메인 전환은 설정 팝오버로 (사건을 비우는 드문 조작). 공간 Depth 는 설비를 고를 때만 (자리표시 Lv1 ~ Lv5 없앰). ⚡ 는 물리엔진에만, 브랜드 ◆ · 흐름 🌊 | `OfficerView.tsx`, `styles.css` |
| U8 | 검색 결과 단추 | 행 누르기와 같은 일을 하던 "📍 도면 핀"(설비 행) · "📂 열기"(사건 행)를 뺌. 사건 행의 "📍 설비 보기"(다른 일)와 "📋 인터록 보기"만. 허브 알림은 한 줄 (바쁨 › 알림 › 검색 요약) | `cctv/CommandBar.tsx`, `OfficerView.tsx` |
| U9 | 의미 없는 조작 | Stage 1 · 2 카드를 한 칸(두 상태 표시, `data-stage`)으로. Gate 2 체크리스트는 결정이 필요해 그대로 | `StagePanel.tsx` |
| U10 | 확대 방식 하나 | A0 도 도면 · Mermaid 와 같은 휠 확대 · 끌기 · 확대 단추. 타일 누르기는 그 타일에 맞추기(`usePanZoom().zoomTo`), [전체 보기] 는 맞춤 되돌리기 | `views/A0View.tsx`, `cctv/stage.ts` |
| K1 | 현장: 섹터 화면 없애기 | 섹터 고르기 화면 삭제, 단계 4 → 3. 엔진은 고른 섹터를 읽지 않음 (라우팅 · 관할은 설비 코드의 원장 섹터, 비면 코드로 채움) | `KioskView.tsx` |
| K3 | 현장: 종류 · 선택 칸 | 스캔이 종류를 정했으면 종류는 글로만 + [종류 바꾸기]. 센서 태그 칸은 [＋ 센서 태그 (선택)] (비우면 설비의 첫 센서) | `KioskView.tsx` |
| P2 | 상세 · 보관을 오류로 가르지 않기 | 목록 행 상태(done)로 상세 · 보관을 고름. 결정 응답의 `finalized` 로 알림에 [보관 문서 열기] | `OfficerView.tsx`, `StagePanel.tsx` |
| P3 | 현장 보고 도착 알림 | 새 현장 보고(FLD) · 현장 회신으로 결재 대기가 된 사건을 허브 알림 "📨 현장 보고 도착 · 동 · 코드 · 번호 [열기]" 로 | `OfficerView.tsx` |
| P5 | 서명은 보내기 전에 | 패드가 재는 획수 · 면적 · 직선으로 코어(`gate1_formal.rs`)와 같은 규칙을 미리 보고 보내기 단추를 막음. 소명 화면에 [처음으로] | `KioskView.tsx` |
| P7 | 모델 확인은 한 번 | `ModelSettings` 효과가 렌더마다 다시 돌던 것(넘겨받은 `onOpen` 이 매번 새 함수)을 처음 한 번으로. `ensure_models` · `check_model_status` 는 비동기 명령으로 (메인 스레드 비움) | `ModelSettings.tsx`, `OfficerView.tsx`, `lib.rs` |
| P8 | 새로 고침은 알림 하나로 | `incident_changed` · `domain_changed` 알림 한 번에 한 번만 받음 (명령 뒤 직접 받기 · 도메인 전환의 3중 새로 고침 없앰). 5초 목록 폴링은 대기 시간용으로 남기되 고른 사건 상세는 다시 받지 않음. 원장(`plant_topology`)은 처음 · 도메인 · 언어 · 변경 오버레이 수가 바뀔 때만. 물리 판은 1초 폴링을 빼고 0.5초 알림만 | `OfficerView.tsx`, `PhysicsPanel.tsx` |
| P9 | 앱을 다시 띄우지 않는 초기화 | `reset_session` 명령: 같은 도메인의 사건 · 감사 · 자동화 · 물리엔진 · 단말 대기를 비움 (도메인 전환과 같은 함수, 마스터 도면 색인은 그대로). 시연 판의 [↺ 세션 초기화] (두 번 눌러 확인). 녹화 기록 파일 바꾸기는 하지 않음 (core 를 건드려야 함) | `lib.rs` (`reset_session` · `reset_domain`), `OfficerView.tsx` |
| D1 | E2E · 촬영 실행 한 줄 | `scripts/e2e/run.js`: rocm-env · 빌드 폴더 · 디버그 포트 · 스캐너 폴더 · 모델 기록을 맞춰 앱을 띄우고 스크립트를 돌린 뒤 앱을 내림. 다른 cargo · rustc 가 돌면 끝날 때까지 기다림, `--jobs`, `--rules`, `--app-only` / `--stop`, `--share-gpu` · `--gpu-guard` | `scripts/e2e/run.js`, `scripts/e2e/ipc_probe.js` |
| D2 | E2E 스크립트 공통 부분 | `scripts/e2e/lib.js` 하나로 (창은 화면 글이 아니라 `main.console` · `main.kiosk` 로 찾음, 스캐너 폴더는 늘 비우고 한 장만). record.js 는 마우스 자리를 남기는 click 을 넘겨 씀 | `scripts/e2e/lib.js`, `console.js`, `record.js` |
| D3 | 영상 편집 입력 확인 | 시작할 때 녹화 폴더 · 모델 기록 · 측정 파일을 보고, 빠진 것과 만드는 명령을 알리고 멈춤 (`--allow-missing` 으로만 통과) | `scripts/e2e/edit_video.py` |
| D4 (단기) | 번역 표 절차 | `run.js --i18n` 한 줄: 한국어 `--audit` → `build_en.py` → 영어 E2E. 문서의 결과 폴더도 `--shots out/shots_ko` 로 맞춤 | `scripts/e2e/run.js`, `docs/i18n.md` |
| D5 | 합성 데이터 만들기 | `scripts/gen_all.py` 가 시나리오 → 도면 순서로 두 도메인을 만들고 core/fixtures 의 바뀐 파일을 알림. 도면 생성기는 `--lang` · `--domain` 순서를 가리지 않음 | `scripts/gen_all.py`, `scripts/gen_pid_drawings.py` |
| 0단계 | E2E 기준 | 흐름 표시를 끄고 다시 켜지 않던 잔재를 고침 (P2 뒤 흐름이 기본 상태로). 모든 결재가 HUD 를 지나 HUD 결재가 E2E 기준이 됨. 흐름 전체의 명령 호출 수를 `<shots>/ipc_calls.json` 에 남김. CCTV 흐름에 "TTF 머리 숫자 한 곳" 검사 | `scripts/e2e/console.js`, `app/src/ipc.ts` (`window.__ipcCalls`) |
| P1 | 확정 의도 하나 | 화면은 `confirm` 하나만 보내고(공동 결재 · 근거 번호와 함께), 고위험 절차 승인 갈래(`ApplyProcedure`)는 백엔드가 정함. 공동 결재 없이 누른 확정 · 현장 대기 사건의 확정은 그대로 확정으로 보내 가드레일(도장 찍기 · 현장 확인 전)이 막음. 근거 절차서 번호는 **미리 채우지 않고** 공동 결재 아래에 늘 보이며 사람이 침 (Gate 3 이 초안 번호와 대조). 번호가 비면 [조치확정] 이 "🔒 절차서 번호 필요". 예전 `approve_procedure` 는 CDP 스크립트용으로 남김 | `StagePanel.tsx` (`QuickApproval`), `lib.rs` (`decide_incident`), `scripts/e2e/lib.js` (`cosign` 이 초안 전문에서 번호를 읽어 침) |
| U9 | Gate 2 체크리스트 | 어디에도 보내지 않고 닫으면 사라지던 체크 칸을 읽기 전용 목록 "[Gate 2 검출 사항]" 으로. 검출 사항은 이미 자동 확정을 막고(policy.rs) 감사에 남으며, 사람의 확인은 결재 HUD 의 결정 하나로 남김 | `StagePanel.tsx` (`Truth`) |
| P4 | 현장 확인 요청 목록 | 단말 첫 화면에 주제어실이 현장 확인을 기다리는 경보 사건(위치 · 설비 이름 · 코드 · 요청 메모 · 요청 시각 · 접수 번호)을 띄움. 누르면 설비 코드 · 센서 태그를 채움. 사건이 바뀔 때마다 `field_requests_changed` 로 다시 받음. 메모는 현장으로 가는 글이라 내부 정보 검사(`check_field_text`)를 거침 (재점검 알림도 같게). 현장 보고 사건(FLD)의 재점검은 목록에 넣지 않음: 계기 · 수치 보고는 알람 사건에만 붙고 FLD 에는 새 사건이 열림 | `lib.rs` (`field_requests` · `notify_console`), `KioskView.tsx` |
| K2 | 사번 | 접수 때 사번으로만 찾음 (모르는 사번을 이름으로 찾아 원장 사번으로 조용히 바꾸던 것을 막음, 소문자 사번은 같은 사번). 이름 칸은 남김 (3절). 스캔에서 사번을 잘못 읽고 이름으로 고친 경우는 "교정됨" · 스캔 원문으로 단말에 보임. 콘솔의 교정 표에서도 이름 · 사번은 머리줄처럼 가림 | `core/src/field.rs`, `core/src/extract.rs` (`resolve_worker`), `lib.rs` (교정 표 가림) |
| U7 | 엔지니어 보기 | 설정의 "엔지니어 보기 (코드 이름)", 기본 끔 (이 PC 에 기억). 끄면 판단 · 결재 화면이 `jev_head` · `PointerPrompt` · `verdict.rs` · `SharedResidency` · `G4_INTERCEPT_DOC_CORRUPTION` 없이 평이한 말, 검색 결과의 분석 모델 이름 · 상태 줄의 `(SharedResidency)` 도 숨김. 녹화(record.js)는 켜고 찍음 (기획 지정 문구 그대로). 판정 코드 · SDS 코드 · KKS · 절차서 번호는 운영자가 쓰는 표준 코드라 그대로 | `app/src/engineer.ts`, `StagePanel.tsx`, `cctv/CommandBar.tsx`, `OfficerView.tsx` |
| P6 | 알람 사건 초안은 뒤에 | 시나리오 주입은 판단까지 하고 사건을 엶 (AOP 초안은 사실 줄 템플릿). 모델 초안은 뒤의 스레드가 다시 써 채움 (물리엔진 모드와 같은 `staged::redraft`). 그동안 HUD · 서랍 · 단계 카드에 "✍️ 초안 작성 중", [조치확정] 잠금 (보관 문서에 들어갈 글이 정해진 뒤 확정). 원래 초안을 뒤에서 쓰던 물리엔진 사건도 같은 표시. 자동화 모드 · 글쓰기 모델이 없을 때는 예전처럼 한 번에. `BLUEPRINT_DEFER_DRAFT=0` 이면 늘 한 번에 (전후 측정) | `lib.rs` (`inject_scenario` · `redraft_later` · `drafting`), `StagePanel.tsx`, `scripts/e2e/p6_probe.js` |
| D4 (장기, 첫 조각) | 엔진 글을 키 + 값으로 | 단계 카드 요약(`StageRecord`)을 키 + 값(`StageNote`, 22가지)으로 바꾸고, 화면에 보낼 때 지금 화면 언어로 만듦 (이름 · 맡은 것도). 번역 틀 표에서 단계 문장 틀 19개 · 통째 글 11개를 지움 (en.json patterns 219 → 200, exact 277 → 268). 판정 이름은 다른 곳처럼 낱말 표가 바꿈 (이름 표를 두 벌 두지 않음) | `core/src/stages.rs`, `core/src/state_machine/analysis.rs`, `scripts/i18n/en_engine.py` |

## 2. 기획안과 다르게 한 것

- U2: 설비 확대 창(설비 범위)은 남겼습니다. 사건 범위 Drop-up 과 보여 주는 대상이 달라, 합치면 범위 전환 UI 가 하나 더 생깁니다.
- U4: 모델 관리는 시연 도구가 아니라 관리 기능이라 설정 팝오버에 그대로 두었습니다.
- U4: 기본은 "보이기" 입니다. 이 앱은 시연 · E2E 가 주 용도라, 기본으로 숨기면 처음 연 사람이 사건을 만들 길이 보이지 않습니다.
- P1 의 갈래 판단은 core 의 `Engine::confirm` 이 아니라 앱(`decide_incident`)에 두었습니다. core 의 확정 · 고위험 승인 계약과 시험(발전소 · 데이터센터 시나리오 E2E)은 그대로입니다.
- D4 는 기획안처럼 화면이 키를 받아 글을 만드는 대신, 엔진이 키 + 값을 들고 있다가 보낼 때 그 화면 언어로 만듭니다. 영어 · 한국어 문장 틀이 엔진 한 곳에 있고, 프런트 · 번역 감싸기(`ipc.ts`)는 바꾸지 않았습니다 (영어 글에는 한글이 없어 번역 표를 지나침).
- 기획 조사 중 나온 "단계 서랍이 결재 HUD 를 가린다" 는 틀린 주장이라 기획안에 넣지 않았습니다. `.console.has-hud .drawer` 는 HUD 앞에서 멈추고, 허브의 단계 카드로 서랍 내용을 바로 바꿀 수 있습니다.

## 3. 결정 칸이 비어 있어 이렇게 정한 것

기획안의 결정 칸은 비어 있었고, 사용자가 "전부 진행" 을 요청해 아래처럼 정했습니다. 되돌리는 방법을 함께 적습니다.

| # | 정한 것 | 까닭 | 되돌리려면 |
| --- | --- | --- | --- |
| P1 | 근거 절차서 번호는 사람이 침 (미리 채우지 않음) | core 가 이 대조를 도장 찍기 차단(`G3_INTERCEPT_RUBBER_STAMP`)으로 두었고, 기획 Part 8-6 이 "비상운전절차서(AOP) 조항 번호 기입" 을 입증 조건으로 적음. 미리 채우면 대조가 늘 통과해 장치가 없는 것과 같음 | `QuickApproval` 의 `useState("")` 를 초안 번호로 (한 줄) |
| U9 | 체크 칸을 지우고 목록으로 | 체크는 확정 조건도 감사 기록도 아니었음. 확정 조건으로 이으면 확정마다 Stage 3 서랍을 여는 마찰이 새로 생기고, 같은 검출 사항은 이미 자동 확정을 막음 | `Truth` 의 목록을 체크 칸으로 되돌리고, 잇는다면 `decide_incident` 에 확인 목록을 실어 감사에 남김 |
| P4 | 목록을 띄우되 위치 · 설비 이름 · 코드 · 요청 메모 · 시각 · 접수 번호만 | 현장 단말은 이미 접수 번호와 재점검 메모를 받음 (`field_update`). 내부 수치 · 판정 코드 · 확신도는 보내지 않고, 메모도 금지어 검사를 거침 | `KioskView` 의 목록 블록과 `field_requests` 명령을 지움 |
| K2 | 사번 · 이름 둘 다 받되 모르는 사번은 막음. 사번만 받고 원장 이름을 보이는 안은 하지 않음 | 콘솔은 근무자 이름을 가리고 실명 열람을 감사에 남김 (`request_unmask`). 단말이 사번을 치면 원장 이름을 보여 주면 사번만으로 이름을 알아낼 수 있어 그 원칙과 어긋남 | `field.rs` 의 `person(...)` 에 `.or_else(person_named)` 를 되돌림 |
| U7 | 기본 끔, 녹화는 켬 | 기획 화면 그림(Part 8, powerplant-control-plan.md)은 `jev_head` · `PointerPrompt` · `verdict.rs` 같은 코드 이름을 그대로 쓰지만, 운영자에게는 뜻 없는 식별자라 기획안 U7 대로 스위치로 나눔. 녹화는 켜서 기획 문구를 그대로 보임 | 설정의 엔지니어 보기 기본값 (`engineer.ts` 의 `read()`) |
| P6 | 미룸 (자동화 모드 · 글쓰기 모델 없음은 예외) | 콘솔이 사건을 판단 직후에 봄. 대신 뒤의 글쓰기가 GPU 를 쥐는 동안 다음 판단 · 스캔은 기다림 (실측 15절). 물리엔진 모드는 이미 같은 방식 | `BLUEPRINT_DEFER_DRAFT=0` (끄기만), 코드는 `inject_scenario` 의 `defer` |
| D4 (장기) | 첫 조각(단계 카드)만 | 기획 조사의 권장 조각: 화면 전용 글이고 한 곳에서 만들며 값이 언어와 무관함. 나머지(가드레일 · 검색 노트 · 상주 오류 등)는 같은 방식으로 이어 갈 수 있음 | `StageNote::text` 대신 한국어 `summary` 를 다시 보내고 지운 틀을 되살림 |

## 4. 이번에 밟은 것

- VRAM 상한(`BLUEPRINT_VRAM_BUDGET_MB`)은 가중치에만 걸립니다. 현장 스캔 판독(SigLIP2 · gocr) 중에는 활성값이 1.6 GB 까지 더 붙어, 가중치 상한 1,432 MB 에서 앱 최대 3,083 MB 를 썼습니다. 다른 프로세스와 GPU 를 나눠 쓸 때는 `run.js --share-gpu MB` (가중치 상한 = MB − 1200, 버퍼 캐시 0) 와 지킴(어댑터 총량 − 300 MB 를 넘으면 앱을 내림)을 함께 씁니다.
- 상한이 모자라면 Zoro-1 판단은 블록을 나눠 올려 돌지만, 초안 글쓰기는 모든 블록이 함께 있어야 해서 실패하고 템플릿 초안으로 대신합니다. 판단이 느려져 물리엔진 폐루프의 완화 제안이 29.6 → 48.2 초로 늦어지고 과열기 고점이 트립을 넘습니다 (공유 모드에서만).
- 앱은 지난번 고른 화면 언어(영어)로 뜹니다. 화면 글로 단추를 찾는 임시 스크립트는 `L()` 로 두 언어를 줘야 합니다.
- E2E · 촬영 중에 app/src 를 고치면 vite 가 화면을 다시 읽습니다. 기준 측정 · E2E 를 돌리는 동안은 스크립트 · 문서만 고쳤습니다.
- Git Bash 의 `sed -i` 는 CRLF 파일을 LF 로 바꿉니다 (`core/src/stages.rs` 에서 밟고 되돌림). Rust 소스 · 문서 대부분이 CRLF 라 고칠 때는 Edit 도구나 줄 끝을 보존하는 Python 으로.
- 현장 보고는 알람이 있는 사건에만 붙습니다 (`field.rs` `waiting_incident`). 현장 보고 사건(FLD)에 재점검을 지시하면 단말이 다시 보내도 새 사건이 열리므로, 요청 목록에는 알람 사건만 넣었습니다.
- Red-line 스캔은 사번을 읽지 않고 서명 이름으로 원장 사번을 채웁니다 (`extract.rs` 648-658). 그래서 K2 는 접수(`report_of`)만 사번으로 찾게 하고 스캔의 이름 → 사번 채움은 남겼습니다 (단말 화면에 보임).
