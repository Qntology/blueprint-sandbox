# 3대 핵심 병목 개선 (개선안: 엔진 잠금 · 상주 공존 · SDS 델타 패치 · 도면 버림)

> 상태: 받은 개선안을 코드와 실측으로 대조한 기록입니다. 아래 "받은 개선안" 은 원문을 옮기고 표 모양만 고쳤습니다. 수치는 합성 데이터 기준이고, 실측 전문은 [verification.md](verification.md) 8절입니다.

## 0. 구현 대응

| 개선안 | 구현 | 상태 |
| --- | --- | --- |
| 1.1 추론 중 `Desk.engine` · `Desk.gpu` 를 함께 쥐어 관제 UI 조회가 멈춤 | 사실이었습니다. 물리엔진 분석 스레드뿐 아니라 시나리오 주입(`inject_scenario`) · 현장 전송(`submit_field_report`) 도 모델이 도는 내내 엔진 잠금을 쥐었고, 상태 줄 · 사건 목록 · 결정 명령이 모두 기다렸습니다 (`physics_status` 는 엔진을 잠그지 않아 멈추지 않았음). | **확인** |
| 1.2 Stage 1: 준비 → 추론 → 반영 3단계 | `core/src/state_machine/staged.rs` (`EngineAccess` · `run` · `open` · `attach_report` · `redraft` · `propose_mitigation`), `Engine::commit_run` · `attach_job` / `commit_attach` · `redraft_job` / `commit_redraft` · `mitigation_job` / `commit_mitigation`, 모델 손잡이 `Minds`(`Arc`), 세션 판 번호 `Session::rev`(반영 전 대조, 바뀌었으면 준비부터 다시), `field::submit_staged`. 앱은 `impl EngineAccess for Desk` 로 세 경로를 모두 옮김 | 구현 |
| 1.2 "`desk.gpu` 도 핸들 획득 단위로 좁힌다" | 하지 않았습니다. 상주 관리자는 다른 쪽이 핸들을 쥔 묶음을 내리지 않고 오류를 냅니다 (`release_groups`). GPU 잠금을 풀면 판단 도중 gocr 가 끼어들어 둘 중 하나가 실패합니다. GPU 잠금은 모델 계산 전체를 덮은 채로 둡니다. | **확인** (개선안이 틀림) |
| 초안 다시 쓰기 (`redraft`) | 판정을 다시 묻지 않고 글만 다시 씁니다 (`analysis::redraft`). 예전에는 분석 전체를 다시 돌려 SDS 축 질문 4개를 한 번 더 물었습니다 | 구현 (개선안 밖) |
| 현장 보고 재분석의 서명 재요청 · 실패 | 세션을 꺼내지 않고 사본으로 분석하므로, 서명 재요청 · 분석 실패 때 판정 후보와 인터록 기록이 그대로 남습니다 (예전에는 판정 후보가 지워짐) | 구현 (개선안 밖) |
| 1.2 Stage 2: `Residency` 예산 기반 공존 (`pinned`) | `core/src/ai/residency.rs`: `Coexist` · `pinned` · `coexist_hits` · `make_room`. 모델이 바뀌어도 SigLIP2 비전 타워를 남기고, 다음 단계를 올린 뒤 여유분(640 MB) + 공존 여유(512 MB)가 남지 않으면 올리기 **전에** 내립니다. **기본은 꺼짐**, `BLUEPRINT_COEXIST=siglip2` 로 켭니다 | 구현 (선택) |
| "매번 가중치를 디스크에서 다시 올리는 스왑 비용" | 다시 올리기는 맞지만 디스크가 아니라 매핑된 파일에서 0.28 s, 내리기 0.05 s 입니다 (RX 6600 기록). 문서 한 장(12 ~ 38 s)의 1 ~ 3 % 라 기본을 바꾸지 않았습니다 (한 번에 한 모델 원칙 유지) | **확인** |
| 2.1 "A0 기획 3.2절에 델타 패치가 있다" | 3.2절은 깊이 · 절차 검증입니다. 델타 패치는 [powerplant-timeseries-sds-plan.md](powerplant-timeseries-sds-plan.md) Stage 1 · 2 에 있고 미구현이었습니다 | **확인** (인용이 틀림) |
| 2.1 타일 재스캔 24.6 s · 전체 250.5 s · gocr 99 % · 조각 173개 중 KKS 23개 | 실측과 같습니다. 다만 개선안은 이미 있는 원장 상자 빠른 확인 경로(상자 하나 2.2 s, 전체 35/35 · 32.5 s)를 빠뜨렸습니다 | **확인** |
| 2.2 마스터 `PatchGrid` int8 캐시 | `core/src/ai/siglip2/delta.rs` `MasterPatches` (패치마다 배율 + int8, 마스터 5장 1.4 MB), `MasterSet` · `Master` · `DocumentReader::index_masters` | 구현 |
| 2.2 패치 코사인 0.92 미만을 바뀐 패치로 | 실측으로 기각했습니다. 스캔 흐림 · 색조 · 잡음만으로 같은 도면의 패치 코사인 평균이 0.69 ~ 0.71 이고, 그대로인 A0 타일도 247개 중 58 ~ 194개가 0.92 아래였습니다. 패치 코사인은 **어느 마스터인지**(2위와 차이 0.09 ~ 0.11)에만 쓰고, **무엇이 바뀌었는지**는 잉크로 봅니다: 마스터 잉크를 3 px 넓힌 영역 밖의 새 획 + 스캔 잉크를 넓힌 영역 밖의 지운 획 (`ink_changes`, 16 px 칸, 3 %) | 구현 (방식을 바꿈) |
| 2.2 1단계 픽셀 차분으로 바뀐 곳 없는 타일 건너뛰기 | 같은 잉크 대조를 타일마다 합니다 (타일당 약 10 ms). 다시 스캔한 A0 는 16장 모두 바뀐 칸 0, Red-line A0 는 B3 · B4 두 장만 | 구현 |
| 2.2 `retain_changed_crops` | `delta::retain_changed`: 바뀐 칸에 걸친 조각만 gocr 로. 표제란(도면번호 · 개정)과 서명은 늘 읽음. 읽은 도면번호가 고른 마스터와 다르면 앱이 그 장을 전부 다시 읽음 (`read_scans`) | 구현 |
| 3.1 마스터 5장 · A0 버림 0개 | 사실이었습니다 (안내문만 버리는 규칙이라) | **확인** |
| 3.1 "버려야 할 것은 표제란 · 심볼 범례 · 도각 눈금" | 합성 도면에는 심볼 범례 표 · 도각 눈금이 없습니다. 표제란은 버리면 안 됩니다: 도면번호 · 개정을 읽어 MOC 대조(구버전 도면 경고)에 씁니다. 실측으로 본 낭비는 **글자 없는 띠**(상자 테두리 · 배관 · 점선뿐, gocr 가 빈 글을 냄)였습니다: 마스터 80조각 중 12개, A0 타일 173조각 중 34개 | **확인** (개선안이 틀림) |
| 3.2 도면 전용 `Prejudice` 원형 · Z 점수 버림 | 대신 글자 모양 덩어리(8 방향 연결 요소, 높이 · 너비가 글자 높이의 0.4 ~ 2.8 배, 길고 가는 선 제외)가 둘 미만인 조각을 읽지 않습니다 (`InkPage::glyphs`, `PagePlan::blank`). 앵커와 상관없이 양식 · 도면 모두에 씁니다. 지난 판독 기록으로 대 보면 빈 조각 마스터 8/12 · A0 23/34 를 걸러 내고 글자 조각은 잃지 않습니다 (A0 에서 걸리는 글자 조각 7개는 `---` · `]` · `L` · `j` 같은 잡음과 배관 이름 하나) | 구현 (방식을 바꿈) |
| 3.2 안내문(고정 주기) 버리기 | 하지 않았습니다. 마스터의 안내문 세 줄은 서명란과 같은 행이라 한 조각으로 묶여(예: PID-101 `[64, 1237, 1857, 1333]`) 문단 규칙에 걸리지 않습니다. 떼려면 레이아웃 분할을 바꿔야 하고, 이는 설비 상자 가로 띠 판독 전체에 영향을 줍니다 | 미구현 |
| 4 검증 행렬 | 아래 1 ~ 4. `cargo test -p blueprint-app` 은 없어서(앱 크레이트에 시험 없음) 엔진 잠금 시험은 코어(`core/tests/engine_lock.rs`)와 앱 E2E(`PHYS` 의 상태 줄 응답 시간)로 대신했습니다. 공존 시험은 시험용 가중치 파일을 쓰려고 `residency.rs` 안에 두었습니다 (`core/tests/residency_coexist.rs` 대신) | 아래 |

### 검증 행렬 결과

| 검증 항목 | 개선안 목표 | 결과 |
| --- | --- | --- |
| 1. 추론 중 엔진 잠금 점유 | 준비 + 반영 < 10 ms | **달성.** 대역 모델(질문 60 ms · 글쓰기 250 ms): 예전 분석 495 ms 동안 조회 최대 475 ms 대기 → 나눔: 조회 92번 최대 대기 0.0 ms, 엔진 잠금 3번 합 0.03 ms (디버그 빌드). 앱 실측(물리엔진 분석 중 상태 줄 응답)은 [verification.md](verification.md) 8.5 |
| 2. SigLIP2 → Zoro-1 전환 스왑 | 여유 VRAM ≥ 1.5 GB 에서 스왑 0회 | **켰을 때 달성, 기본은 꺼짐.** 8 GB 에서 문서 8장 · 시나리오 8개 동안 비전 타워 올림 8회 → 1회 (2.66 s → 0.28 s), Zoro-1 판단 · 글쓰기 시간 같음. 아낀 시간이 장당 0.3 s 라 기본은 그대로 ([verification.md](verification.md) 8.2) |
| 3. A0 · Red-line 델타 재스캔 | 바뀐 곳 없는 타일 < 0.1 s, 볼펜 수정 타일 < 3.0 s | **바뀐 곳 없는 타일 달성 (17 ms).** 볼펜 수정 타일은 목표에 못 미침: Red-line 이 걸친 A0 타일 2장 55.3 s → 11.9 s (장당 6 s), 현장 Red-line 한 장 36.6 → 11.2 s · 33.9 → 7.0 s, 뽑은 값은 전부 읽기와 같음. 구름 · 메모 · 직인 · 일람표 값이 여러 띠에 걸쳐 조각이 7 ~ 11개 남기 때문 ([verification.md](verification.md) 8.3) |
| 4. 도면 고정 영역 버림 | 도면당 3 ~ 6개, KKS 35/35 유지 | **KKS 35/35 유지, 개수는 미달.** 글자 없는 띠: 마스터 82 → 75조각(장당 1 ~ 2개), A0 타일 173 → 145조각 · 250.5 → 238.4 s. 도면번호 · 개정 5/5 (표제란은 버리지 않음) ([verification.md](verification.md) 8.4) |

---

## 받은 개선안 (원문)

### 1. `Residency` 예산 기반 공존(`Coexist`) 및 전역 락(`Desk.gpu` / `Desk.engine`) 분리

#### 1.1 현황 및 문제점 (As-Is)

1. **추론 중 관제 UI 및 상태 조회 블로킹 (`physics_mode.rs` / `lib.rs`)**
   - 현재 자율 대응 모드(`physics_mode.rs`)에서 사건이 열리면 백그라운드 분석 스레드가 `Desk.gpu` 뮤텍스와 `Desk.engine` 뮤텍스를 **동시에 잠근 채로** `Zoro-1` 판단(`judge_by_criteria`)과 글쓰기(`TextWriter`)가 끝날 때까지 수 초~수십 초 동안 놓지 않습니다.
   - 이 때문에 모델이 생각하는 동안 프론트엔드의 상태 폴링이나 교대감독의 세션 조회(`Engine` 접근)까지 함께 블로킹됩니다.
2. **여유 VRAM이 충분할 때도 무조건 전량 언로드 (`core/src/ai/residency.rs`)**
   - 현재 `Residency::enter`는 `"모델이 바뀌면 이전 모델을 통째로 내립니다. 동시 상주가 없습니다."`라는 엄격한 단일 상주 정책만 가지고 있습니다.
   - 그 결과 현장 Red-line 스캔본이나 게이지 점검표가 들어올 때 `SigLIP2 (비전)` ➔ `gocr` ➔ `Zoro-1 (판단)` 전환 시마다 가용 VRAM 예산과 무관하게 매번 가중치를 디스크에서 다시 올리는 스왑 비용이 발생합니다.

#### 1.2 개선 설계 (To-Be)

1. **Stage 1: `Desk.engine` 락 범위 축소 (Prepare ➔ GPU Inference ➔ Commit 3단계 분리)**
   - **Step A (Prepare - `engine` 락 < 1ms):** `lock(&desk.engine)`을 잡아 Gate 0 ~ Gate 2 규칙 검사와 상류 그래프 역추적(`causal`)까지만 수행하고, 모델 판단에 필요한 입력 데이터(`DecisionTask`, 시계열 요약)만 복제한 뒤 **`desk.engine` 락을 즉시 해제**합니다.
   - **Step B (Inference - `engine` 락 없음):** `PointerModel`(`Zoro-1`)과 `TextWriter` 호출은 `desk.engine` 락 밖에서 수행합니다. (VRAM 직렬화는 이미 `SharedResidency` 내부의 `Arc<Mutex<Residency>>`와 `Arc<Loaded>` 핸들이 담당하므로 `desk.gpu` 전역 뮤텍스도 추론 전체가 아니라 핸들 획득 단위로 좁힙니다.)
   - **Step C (Commit - `engine` 락 < 1ms):** 추론 결과(`Judgement`, `Draft`)가 나오면 다시 짧게 `lock(&desk.engine)`을 잡아 세션을 갱신(세대 번호 `generation` 대조로 취소 여부 확인)하고 Gate 4~5를 마무리합니다.
2. **Stage 2: `Residency`에 예산 기반 핀(`Pinned` / `Coexist`) 슬롯 추가**
   - `core/src/ai/residency.rs`의 `Residency` 구조체에 경량 상주 슬롯(`pinned: BTreeMap<(ModelId, Group), Arc<Loaded>>`)을 추가합니다.
   - `Residency::enter(model_id, phase)` 호출 시, 직전 모델이 경량 비전 타워(`SigLIP2::Vision`)이고 현재 가용 VRAM(`free_bytes`)에서 `pinned_bytes + next_phase_bytes + safety_margin(512MB)`을 빼도 양수라면, `SigLIP2` 비전 타워를 언로드하지 않고 `pinned`로 이동시켜 유지합니다.
   - 만약 `Zoro-1` 적재 시 VRAM이 부족하면(`can_coexist == false`), 그 즉시 `pinned`를 먼저 비워(`clear` + `settle`) 기존과 동일하게 저사양(4GB) 안전성을 100% 보장합니다.

```rust
// [core/src/ai/residency.rs] 개선 구조 요약
pub struct Residency {
    device: Device,
    budget: Budget,
    current: Option<Current>,
    /// 여유 VRAM 예산이 허락할 때 유지하는 경량 모델 그룹 (예: SigLIP2 Vision 타워)
    pinned: BTreeMap<(ModelId, Group), Arc<Loaded>>,
    pub coexist_hits: u64,
    history: Vec<Transition>,
}
```

### 2. A0 도면 및 Red-line 판독 시간 단축을 위한 `SDS 델타 패치(Delta-Patch Diff)` 구현

#### 2.1 현황 및 문제점 (As-Is)

- `docs/powerplant-a0-interlock-plan.md` 3.2절에는 *"마스터 P&ID 도면의 SigLIP2 패치 특징(`PatchGrid`)을 캐시해 두고, Red-line 도면 스캔 시 코사인 유사도가 달라진 패치만 선택 추출해 `gocr` 시간을 70% 이상 단축한다"*고 기획되어 있습니다.
- 그러나 실제 구현(`core/src/tiling.rs` 및 `document.rs`)에는 이 **SDS 델타 패치 비교기가 빠져 있어**, 타일 재스캔 시 1장당 **24.6초(300 DPI는 59.9초)**, 전체 타일 스캔 시 **250.5초**가 소요됩니다.
- 실측 분석 결과, 전체 판독 시간의 99%가 `gocr`(비전 97초 + 언어 151초)이며, 판독한 조각 173개 중 실제 유의미한 코드는 13%(23개)에 불과하고 나머지는 변하지 않은 배관·화살표·표제란이었습니다.

#### 2.2 개선 설계 (To-Be)

`logis-center`의 `layout_cache.rs`(int8 양자화 패치 캐시 및 코사인 대조) 기법을 가져와 `blueprint-sandbox`에 **마스터 도면 `PatchGrid` 베이스라인 캐시 & 델타 필터**를 구현합니다.

1. **마스터 도면/타일 패치 캐시 (`core/src/ai/siglip2/delta_cache.rs` 신설)**
   - 마스터 도면(PID-101~105) 또는 A0 타일별로 `SigLIP2`가 추출한 `PatchGrid`($N \times 1152$ 차원 벡터)를 `int8` 양자화(`scale: f32` + `q: Vec<i8>`, 패치당 약 1.1KB, 타일당 ~300KB)하여 디스크/메모리에 캐싱합니다.
2. **픽셀 + 패치 2단계 델타 게이트 (`filter_delta_crops`)**
   - **1단계 (타일/영역 조기 스킵 - 0.05초):** A0 도면 또는 Red-line 스캔본이 들어오면, 마스터 이미지와 썸네일/저해상도 픽셀 차분($\lvert I_{\text{scan}} - I_{\text{master}} \rvert$)을 먼저 계산해 변화가 없는 타일은 아예 `SigLIP2`조차 태우지 않고 원장(Lattice) 캐시값을 즉시 반환합니다.
   - **2단계 (패치 코사인 디핑 - 타일 내 조각 필터링):** 변화가 감지된 타일에서 `SigLIP2` `PatchGrid`를 뽑은 뒤, 각 패치 $i$에 대해 마스터 캐시 패치와의 코사인 유사도 $s_i = \cos(v_i^{\text{scan}}, v_i^{\text{master}})$를 계산합니다.
   - `plan.rs`가 생성한 크롭 후보(`Bbox`) 중, 내부에 변화 패치($s_i < \tau_{\text{delta}}$, 기본 $\tau_{\text{delta}} = 0.92$)를 최소 1개 이상 포함하는 크롭(볼펜 수기 수정, 신규 스티커, 서명란)만 남기고, 마스터와 똑같은 인쇄 영역(`s_i >= 0.92`)은 `gocr` 투입 대상에서 제외합니다.

```rust
// [core/src/ai/siglip2/delta_cache.rs] 핵심 인터페이스
pub struct MasterPatchCache {
    pub sheet_id: String,
    pub rows: usize,
    pub cols: usize,
    pub dim: usize,
    /// 패치별 L2 정규화된 int8 양자화 특징 (logis-center layout_cache 방식)
    pub patches_i8: Vec<i8>,
    pub scales: Vec<f32>,
}

impl MasterPatchCache {
    /// 스캔된 PatchGrid와 마스터 캐시를 비교하여 코사인 유사도가 threshold 미만인(변경된) 패치 마스크 반환
    pub fn diff_mask(&self, scan_grid: &PatchGrid, cos_threshold: f32) -> Vec<bool>;

    /// 크롭 계획(Plan) 중 변경된 패치를 덮고 있는 크롭만 필터링 (gocr 호출 80%+ 절감)
    pub fn retain_changed_crops(&self, crops: Vec<PlannedCrop>, scan_grid: &PatchGrid, cos_threshold: f32) -> Vec<PlannedCrop>;
}
```

- **기대 효과:** Red-line 도면이나 부분 수정 타일 스캔 시 `gocr`에 넘어가는 조각 수가 20~30개에서 **실제 볼펜 수정이 일어난 1~3개 조각**으로 줄어들어, 타일 재스캔 시간이 **24.6초 ➔ 1.5~2.5초 이내**로 단축됩니다.

### 3. 도면 고정 범례(Guide)·표제란 미제거(버림 0개) 해결: `Prejudice` 배제 앵커 & 이중 센터링 이식

#### 3.1 현황 및 문제점 (As-Is)

- 현재 `blueprint-sandbox`의 `core/src/ai/siglip2/plan.rs`는 조각을 버릴 때 다음 두 조건이 동시에 만족될 때만 버립니다:
  1. 레이아웃 모양이 문단(`layout::runs`)일 것
  2. `crop_probe.bin`의 `guide`(안내문) 클래스 점수가 다른 모든 클래스를 여유 있게 이길 것
- **실패 원인:**
  - `crop_probe.bin`은 `zoro-gravity`의 일반 민원 행정 서식(A4 줄글 안내문)으로 학습된 가중치입니다.
  - 반면 발전소 P&ID 도면의 불필요 영역은 줄글 문단이 아니라 **도면 우측 하단 표제란(Title Block), 심볼 범례 표(Symbol Legend), 도각 테두리 눈금(Grid Border), 주의사항 박스(NOTES)** 형태입니다.
  - 따라서 기존 프로브가 이를 `guide`로 인식하지 못해 **마스터 도면 5장과 A0 타일 전체에서 버려지는 조각이 0개**이며, 매번 표제란과 범례를 `gocr`로 중복 판독하느라 시간을 낭비하고 있습니다.

#### 3.2 개선 설계 (To-Be)

기존 `crop_probe.bin` 파일을 건드리지 않고 호환성을 유지하면서, `logis-center`의 `Bias(긍정) vs Prejudice(배제)` 행·열 이중 센터링(`bank_neutral_key_matrix`)과 도면 전용 배제 프로토타입(`drawing_prejudice`)을 `siglip2/probe.rs` 및 `plan.rs`에 결합합니다.

1. **도면 전용 `Prejudice`(배제) 원형 벡터 추가 (`symbol_dict.bin` 확장 또는 `drawing_prejudice` 뱅크)**
   - 이미 `core/src/symbols.rs`에서 도면 심볼 원형 벡터를 추출해 `symbol_dict.bin`을 만든 것과 동일한 파이프라인(`scripts/build_symbol_dict.py`)을 활용합니다.
   - 마스터 도면들의 고정 비정보 영역 3종에 대한 패치 평균 원형 벡터(1152차원)를 추출해 배제 앵커(`Prejudice Prototypes`)로 등록합니다:
     - `prej_title_block`: 도면 우측 하단 표제란 / 리비전 표 헤더
     - `prej_legend_chrome`: 심볼 범례(`SYMBOL LEGEND`), 고정 주기(`NOTES`)
     - `prej_border_grid`: 도면 외곽 도각선 알파벳/숫자 인덱스 칸
2. **`logis-center`식 행·열 이중 센터링 및 상대 우위 게이트(`Relative Prejudice Gate`) 이식 (`plan.rs`)**
   - 각 크롭 후보 조각 $q$에 대해, 긍정 설비/값 점수 $S_{\text{bias}}(q)$(기존 프로브의 `value`/`table` 점수 + 설비 상자 특징)와 도면 배제 점수 $S_{\text{prej}}(q)$를 각각 표준화(Z-score)하여 차감합니다:

     $$Z_{\text{net}}(q) = Z_{\text{bias}}(q) - \max\bigl(0,\; Z_{\text{prej}}(q)\bigr)$$

   - **도면 모드(`PlanMode::Drawing`) 전용 버림 규칙 추가:** 일반 민원 서식(`PlanMode::Form`)에서는 기존처럼 `runs`(문단) 조건만 쓰되, 도면 모드(`PlanMode::Drawing`)에서는 **`runs`가 아니더라도(표 모양의 범례나 표제란이더라도) $Z_{\text{net}}(q) < -\tau_{\text{drop}}$ 이고 내부에 KKS/MOC 정규식 패턴 후보(잉크 밀도 기반)가 없으면 과감히 버림(`DropReason::DrawingPrejudice`)** 처리합니다.

### 4. 검증 및 실측 목표 (Verification Matrix)

| 검증 항목 | 현재 수치 (As-Is) | 목표 수치 (To-Be) | 검증 명령어 / 테스트 |
| --- | --- | --- | --- |
| **1. 자율 대응 추론 중 `Engine` 락 점유 시간** | 추론 내내 점유 (**~5,000 ms+**) | Prepare + Commit 합산 **< 10 ms** | `cargo test -p blueprint-app` (동시 상태 조회 테스트 추가) |
| **2. `SigLIP2` ➔ `Zoro-1` 전환 시 VRAM 스왑** | 매번 언로드 및 재적재 (Coexist 0회) | 여유 VRAM ≥ 1.5GB 환경에서 **`SigLIP2` 비전 타워 상주 유지 (스왑 0회)** | `core/tests/residency_coexist.rs` 단위 시험 |
| **3. A0 타일 / Red-line 델타 재스캔 속도** | 타일 1장 **24.6 s** (전체 **250.5 s**) | 변경 없는 타일 **< 0.1 s**, 볼펜 수정 1곳 있는 타일 **< 3.0 s** (`gocr` 조각 80%↓) | `a0_probe.exe --delta` 실측 비교 |
| **4. 도면 고정 범례·표제란 버림(Drop) 개수** | 마스터 5장 모두 **버림 0개** | 도면당 범례·표제란·외곽 잡음 **3~6개 조각 정확히 제거** (단, **KKS 회수율 35/35 = 100% 유지** 불변식) | `cargo test -p blueprint-core` & `a0_probe.exe` KKS 35/35 확인 |
