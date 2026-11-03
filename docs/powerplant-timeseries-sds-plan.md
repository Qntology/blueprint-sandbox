# 파이프라인 단계별 시계열 · SDS 로직 (기획 Part 8-3)

> 상태: 기획 기준서입니다. 아래 본문은 받은 기획안(Part 8-3)을 그대로 옮기고 표 모양만 고쳤습니다. 수치는 합성 데이터 기준입니다.

## 0. 구현 대응

| 기획 | 구현 | 상태 |
| --- | --- | --- |
| `RingWindow<T, 60>` · EWMA · 기울기 · 가속도 · TTF | `core/src/timeseries.rs` (`RingWindow<N>` 은 ×1000 정수, `metrics` 가 `TimeSeriesMetrics` 를 만듦) | 구현 |
| 변곡점(Inflection) | `Series::departure`: 기준 구간 20초의 평균에서 데드밴드와 4σ 중 큰 폭을 넘는 첫 시각을 표본 사이 보간으로 | 구현 |
| SDS-Trigger (정적 임계 · 동적 기울기 TTF ≤ 30초 · 현장 스캔) · 데드밴드 | `core/src/sds.rs` (`trigger_at`, `first_trigger`) | 구현 |
| 센서 동결 (30초 분산 0) · 불가능한 슬루율 잡음 | `Series::is_flat`, `Series::spikes` → `gate1_formal::signal_health` | 구현 |
| 스캔 시점 동기화 `x(t_scan)` | `gate2_consistency::field_vs_dcs` (`Series::value_at` 보간) | 구현 |
| 다중 보팅 상태 벡터 (`SDS_CH_A_DRIFT` 등) | `gate2_consistency::vote` | 구현 |
| 인과 전파 시차 그래프 (`TemporalPropagation`) | `core/src/causal.rs` (`t_alarm - Δt_max ≤ t_up ≤ t_alarm - Δt_min`, 역류 여파 분리) | 구현 |
| SDS 3축 분할 질의 (시간 · 물리 · 센서) | `verdict::judge_by_axes`, `gate3_substantive::evaluate_alarm` | 구현 |
| 시계열 AOP 초안 · 시간 숫자 팩트체크 | `drafting::draft_aop` + `factcheck::check` (사실 줄의 시차 · TTF 숫자까지 대조) | 구현 |
| SDS 블랙박스 봉인 (`tb_audit_log`) | `sds::{freeze, seal}` (±30초 시계열 · 상태 코드 · 축 확신도 · 결정 · 반응 시간), `AuditCategory::SdsSealed` | 구현 |
| 도면 리비전 타임라인 · 구버전 도면 경고 | `Lattice::latest_rev`, `MocCheck::stale_drawing` | 구현 |
| SDS 델타 패치 디핑 (바뀐 패치만 gocr) | — | 미구현 |
| `GdnState` 프리픽스 상태 캐시 | — | 미구현 (zoro 에 접두 상태 저장 · 복원이 없음) |
| P14 타임머신과 60초 윈도 연동 | `app/src/TimeScrubber.tsx` (슬라이더 시각의 값으로 세 뷰를 다시 칠함), SDS 분석기 파형 | 구현 |

---

기존 `zoro-gravity` 아키텍처는 단일 시점의 정적(Snapshot) 사실만을 다루고 있으며, 문서(`docs/automation-mode.md` 9절)에 기록된 것처럼 `logis-center`의 **SDS(Selective State/Decision Snapshot · 상태 변이 진단 및 분할 기법)** 또한 기존에는 단순 기록용에 머물러 있었습니다.

하지만 **1.2GW 화력발전소**에서는 "단일 시점의 수치(예: 온도 615°C)"보다 "시간에 따른 변화율($dT/dt$: 1분 만에 +18°C 급등)과 상·하류 신호 간의 전파 시차($\Delta t$: 4.2초 전 급수밸브 닫힘)"가 고장 원인을 가르는 결정적 단서가 됩니다.

따라서 새로운 시계열 DB나 외부 AI 모델을 추가하지 않고, 현재 `zoro-gravity`의 5-Gate 파이프라인(`Gate 0 ~ Gate 5`)과 `Zoro-1`의 선형 주의 순환 상태(`GdnState`), 그리고 `trace.rs`를 결합하여 **파이프라인 전 단계에 ‘시계열(Time-Series) 로직’과 ‘실질적 SDS(Selective Decision/State Snapshot & Decomposition) 로직’을 내장하는 Part 8-3 심화 기획안**을 작성해 드립니다.

---


## Part 8-3. 파이프라인 단계별 시계열(Time-Series) 및 SDS(상태 변이 분할·스냅샷) 로직 반영 기획 (`docs/powerplant-timeseries-sds-plan.md`)

기존 정적 스냅샷 중심의 5-Gate 심사를 넘어, 실시간 센서 스트림의 변화율(기울기·가속도·전파 시차 $\Delta t$)을 추적하는 ‘시계열 로직’과, 유의미한 상태 전이 순간만 포착해 다차원 의사결정을 직교 기준별로 쪼개어 검증·기록하는 ‘SDS(Selective Decision/State Snapshot & Decomposition) 로직’을 파이프라인 전 단계(Stage 1 ~ 5)에 이식하는 기획입니다.

---

### 1. 핵심 개념 재정의: 발전소 도메인에서의 시계열 & SDS 로직

| 구분 | 기존 `zoro-gravity` / `logis-center` 한계 | 발전소 관제 파이프라인(Part 8-3) 반영 설계 |
| --- | --- | --- |
| **시계열 로직**<br>*(Time-Series)* | • `CaseFile`에 단일 시점의 값(`latest_transfer`, `contract_rent`)만 저장 <br>• 시간에 따른 추세(Trend)나 상·하류 알람 간 선후 관계($\Delta t$) 계산 불가 | • **슬라이딩 링버퍼(`RingWindow<T, 60>`)**: 설비 태그별 최근 60초($1\text{Hz}$) 시계열과 EWMA(지수이동평균), 변화율($dx/dt$), 잔여 도달 시간(**TTF**: Time-to-Trip) 계산<br>• **인과 시차($\Delta t$) 교차상관**: 마인드맵(`graph.rs`) 상류 변화 시점($t_0$)과 하류 알람 시점($t_1$)의 물리적 유체 이동 지연시간 대조 |
| **SDS 로직**<br>*(Selective Decision & State Snapshot)* | • `logis-center`에서 SDS는 단순 로그 기록용에 불과했고 게이트 판정에 개입하지 못함 (`docs/automation-mode.md` 9절) | • **SDS-Trigger (선택적 상태 스냅샷)**: 평시 노이즈는 버리고, 변화율($dx/dt$)이 임계치를 넘는 **변곡점(Inflection Point)에서만 전후 $\pm 30$초 시계열과 도면 좌표를 동결(Freeze) 캡처**<br>• **SDS-Decompose (판단 분할 & 상태 전이)**: 복합 고장 징후를 **시간축(선후 관계) × 물리축(압력·온도·유량) 직교 기준으로 분할 질문**(`judge_by_criteria` 확장)하여 환각 및 오판율을 0으로 수렴 |

---

### 2. 파이프라인 단계별(Stage 1 ~ 5) 시계열 & SDS 반영 총괄 매핑표

| 파이프라인 단계 | 적용 모듈 | 시계열(Time-Series) 반영 로직 | SDS(Selective Decision/State) 반영 로직 |
| --- | --- | --- | --- |
| **Stage 0**<br>**`Gate 0` (수집·라우팅)** | `intake.rs`<br>`gate0_router.rs` | • **60초 링버퍼 & $dx/dt$ 기울기 감시**: 정적 임계치 도달 전이라도 기울기(급등/급락)가 위험 수준이면 조기 경보 발령 | • **SDS 데드밴드(Deadband) 필터**: 미세 진동은 무시하고 유의미한 상태 변이($\vert{}\Delta x\vert{} > \sigma$) 발생 시에만 사건 세션(`ActiveSession`) 생성 및 $\pm 30$초 블랙박스 스냅샷 고정 |
| **Stage 1 & 2**<br>**`SigLIP2` + `gocr**` | `ai::siglip2`<br>`extract.rs`<br>`lattice.rs` | • **도면 리비전 시계열(`Rev` 타임라인)**: 설비 부품 교체 이력(`building_ledger` 변동일 구조 확장)과 스캔 도면의 시간축 유효성 매칭 | • **SDS 델타 패치 디핑(Delta-Patch Diff)**: Red-line 도면 스캔 시 마스터 도면의 기존 패치 특징과 달라진(수기 마크업된) 조각만 선택적으로 `gocr`에 투입 |
| **Stage 3**<br>**`Gate 1 ~ Gate 2`** | `gate1_formal.rs`<br>`gate2_consistency.rs` | • **센서 동결(Flatline) & 스파이크 검출**: $\text{Var}(x)_{10s} == 0$ (센서 고착) 또는 물리적으로 불가능한 0.1초 단발 스파이크 제거<br>• **스캔 시점 동기화($t_{\text{scan}}$)**: 현장 게이지 촬영 시각과 동일한 시각의 DCS 버퍼값($x(t_{\text{scan}})$)을 꺼내 비교 | • **SDS 다중 보팅 상태 벡터**: 3채널 센서(A·B·C)와 현장 스캔본의 시계열 궤적 이탈도(Divergence)를 SDS 상태 코드(`SDS_CH_A_DRIFT`, `SDS_ALL_SLEW`)로 압축 |
| **Stage 4**<br>**`Gate 3` + `Zoro-1 판단**` | `graph.rs`<br>`gate3_substantive.rs`<br>`ai::zoro` | • **인과 전파 시차($\Delta t$) 그래프 탐색**: 상류 설비 변곡점 시각($t_{\text{up}}$)이 하류 알람 시각($t_{\text{down}}$)보다 물리적 유속 지연시간(예: $2 \sim 15$초)만큼 앞섰는지 검증<br>• **잔여 시간(TTF) 산출**: 트립 한계 도달까지 남은 시간($\text{TTF} = \frac{L - x(t)}{dx/dt}$) 계산 | • **SDS 시공간 분할 질의 (`judge_by_criteria` 고도화)**: `Zoro-1`에게 ① 시간 선후 관계, ② 물리량 보존(유량↓=온도↑), ③ 인접 계통 건전성을 분리 질문하여 최솟값 채택 <br>• **`GdnState` 순환 스냅샷 재사용**: `Zoro-1`의 24개 선형 주의 층(`GdnState`)에 직전 정상 상태 은닉 텐서를 캐시해 판단 속도 가속 |
| **Stage 5**<br>**`Gate 4~5` + `Zoro-1 글쓰기**` | `drafting.rs`<br>`factcheck.rs`<br>`audit.rs` | • **시계열 타임라인 AOP 초안**: *"14:20:00 급수밸브 18% 고착 → 4.2초 뒤 과열기 638°C 돌파 (트립까지 잔여 45초)"* 형태의 시간순 보고서 생성<br>• **시계열 수치 팩트체크**: 초안 내 시각·지연시간($\Delta t$)·변화율을 원문 시계열과 1:1 대조 | • **SDS 블랙박스 감사 봉인 (`tb_audit_log`)**: 사고 전후 60초 시계열 파형 해시 + 게이트별 SDS 상태 전이 행렬을 수정 불가능한 감사 로그로 봉인 (팔란티어 타임머신 리플레이 원본) |

---

### 3. 단계별 시계열 & SDS 상세 설계

#### 3.1 [Stage 0: `Gate 0`] 시계열 링버퍼 & SDS 변곡점 트리거 (`gate0_router.rs` & `intake.rs`)

발전소 DCS에서 1초마다 쏟아지는 수만 개의 센서 값을 매번 5-Gate로 돌리면 GPU가 마비됩니다. 따라서 `Gate 0` 앞단에 **메모리 고정 크기 시계열 링버퍼**와 **SDS 변곡점 트리거**를 둡니다.

* **시계열 로직 (`TimeSeriesRing`)**:
* 설비 센서 태그마다 최근 60개 샘플($t_{-59} \dots t_0$)을 유지하며, 정수 연산(스케일 $\times 1000$)으로 **1차 미분(속도 $v = dx/dt$)**, **2차 미분(가속도 $a = d^2x/dt^2$)**, 트립 도달 예상 시간($\text{TTF}$)을 상시 계산합니다.

* **SDS 로직 (`SdsTrigger`)**:
* 다음 3가지 조건 중 하나가 충족될 때만 ‘SDS 스냅샷(사건 번호 부여)’을 발동해 Gate 1로 넘깁니다:
1. **정적 임계 돌파**: $x(t_0) \ge L_{\text{alarm}}$
2. **동적 기울기 돌파 (Pre-Alarm)**: 아직 경보선($625^\circ\text{C}$) 아래라도 상승 속도가 $+3.5^\circ\text{C/s}$ 이상이어서 **$\text{TTF} \le 30\text{초}$** 이내에 트립선에 닿을 것으로 예측될 때
3. **현장 스캔 투입**: 운전원이 현장 계기판 점검표나 Red-line 도면을 스캐너에 넣었을 때

#### 3.2 [Stage 1 & 2: `SigLIP2` + `gocr`] 도면 리비전 타임라인 & SDS 델타 패치 스캔 (`ai::siglip2`)

* **시계열 로직 (설비 생애주기 타임라인)**:
* 기존 `building_ledger`의 `["변동일", "변동내용"]` 표 구조를 발전소 설비의 시간순 정비·부품 교체 이력(Time-Versioned Spec)으로 확장합니다.

* 스캔된 도면의 발행일/개정 번호(`Rev.3`)가 정비 원장의 최신 교체일보다 과거이면 *"구버전 도면 스캔 경고"*를 발생시킵니다.

* **SDS 로직 (Selective Patch Diff)**:
* 마스터 P&ID 도면의 `SigLIP2` 패치 특징(`PatchGrid`)은 이미 초기 구축 시 캐시되어 있습니다.

* 현장 엔지니어가 볼펜으로 수정한 **Red-line 도면**을 넣으면, 전체 도면을 다 읽지 않고 **마스터 도면 패치와 코사인 유사도가 달라진 패치(볼펜 글씨가 추가된 영역)와 서명란(`Sign`)만 SDS 조각으로 선택 추출**해 `gocr` 시간을 70% 이상 단축합니다.

#### 3.3 [Stage 3: `Gate 1 ~ Gate 2`] 시계열 센서 건전성 & 시점 동기화 대조 (`gate1_formal.rs` & `gate2_consistency.rs`)

기존 `Gate 1`과 `Gate 2`가 서류 유효기간과 금액 차액을 검사했다면, 발전소에서는 **시간축 위에서의 신호 무결성과 시점 일치**를 검사합니다.

* **Gate 1 시계열 형식 심사**:
* **센서 동결(Frozen/Flatline) 적발**: 최근 30초 동안 노이즈조차 없이 분산 $\text{Var}(x) == 0.0$이면 실제 물리 현상이 아니라 **계측기 AD 컨버터 행(Hang) 고장**으로 판정합니다.
* **물리적 불가능 슬루율(Slew-rate Violation) 적발**: 대형 보일러 온도가 0.5초 만에 $+200^\circ\text{C}$ 뛰었다가 돌아오는 것은 열역학적으로 불가능하므로 전자기 노이즈(EMI Spike)로 분류해 자동 필터링합니다.

* **Gate 2 시점 동기화(`Time-Aligned`) 3중 대조**:
* 현장 운전원이 로컬 압력계를 촬영해 올린 시각이 `14:21:05`이고 주제어실 확인 시각이 `14:24:00`이라면, 현재(`14:24:00`) DCS 값과 비교하면 오판합니다.
* 반드시 **DCS 시계열 링버퍼에서 촬영 시각(`14:21:05`)과 정확히 일치하는 타임스탬프의 DCS 스냅샷**을 꺼내어 현장 스캔값과 대조합니다.

#### 3.4 [Stage 4: `Gate 3` + `Zoro-1 판단`] 인과 시차($\Delta t$) 그래프 & SDS 분할 판단 (`gate3_substantive.rs`)

이 단계가 **시계열 로직과 SDS 로직이 결합되는 가장 핵심적인 구간**입니다.

1. **시계열 인과 그래프 탐색 (`Temporal Causal Graph` — `graph.rs` 확장)**:
* 기존 `graph.rs`는 단순 연결 여부(`close_path`)만 보았지만, 여기서는 각 간선(`Edge`)에 물리적 전파 지연시간 범위($\Delta t_{\min} \sim \Delta t_{\max}$)를 부여합니다.

* 예: `104동 차단기 ➔ 103동 급수펌프` (전력선: $\Delta t \in [0.0\text{s}, 0.5\text{s}]$)
* 예: `103동 급수밸브 ➔ 101동 보일러 과열기` (고압급수 배관: $\Delta t \in [2.0\text{s}, 12.0\text{s}]$)

* **역추적 공식**: 하류 알람 발생 시각 $t_{\text{alarm}}$에 대해, 상류 설비의 변곡점 발생 시각 $t_{\text{up}}$이 **$t_{\text{alarm}} - \Delta t_{\max} \le t_{\text{up}} \le t_{\text{alarm}} - \Delta t_{\min}$** 구간 안에 들어올 때만 ‘진짜 인과 경로(Causal Root Cause)’로 채택합니다! (하류 알람보다 오히려 늦게 발생한 상류 변화는 원인이 아닌 역류 여파로 자동 분리됩니다.)

2. **SDS 다차원 직교 분할 질문 (`docs/automation-mode.md` 9절 방식 C 고도화)**:

* `Zoro-1`의 `jev_head`에 복합 시계열 상황을 한 덩어리로 물으면 모델이 시간 선후 관계를 놓칠 수 있습니다.

* 따라서 SDS 분할기가 질문을 **3가지 직교 축**으로 쪼개어 각각 확률을 구하고 최솟값을 취합니다:

* **SDS 질문 ① (시간 선후 검증)**: *"상류 급수밸브 개도 급락($t=0.0\text{s}$)이 하류 보일러 온도 돌파($t=+4.2\text{s}$)보다 먼저 발생했는가?"*
* **SDS 질문 ② (열역학/유체 방향 검증)**: *"상류 유량 감소($-39.5\%$)와 하류 온도 상승($+28^\circ\text{C}$)의 물리적 증감 방향이 일치하는가?"*
* **SDS 질문 ③ (계측기 건전성 검증)**: *"다중 채널 보팅 및 현장 게이지 대조 결과 센서 오류가 아닌 실제 물리적 변화인가?"*

3. **`Zoro-1` 선형 주의 `GdnState` 시계열 상태 캐싱 (`ai::zoro::ops.rs` 활용)**:

* 현재 `Zoro-1` 백본은 24개의 게이트 델타 규칙(Gated Delta Net) 선형 주의 층을 가지고 있으며, `GdnState { conv, recurrent }` 텐서로 문맥 상태를 이어 갑니다.

* 발전소 기본 계통 구조와 평시 정상 시계열 상태를 읽은 직후의 `GdnState` 스냅샷을 메모리에 고정(Prefix State Snapshot)해 두고, 알람 발생 시 **변화된 시계열 델타(SDS Delta Facts) 토큰만 이어서 순전파**하면 프리필 시간을 0.63초에서 **0.15초 이하**로 단축할 수 있습니다.

#### 3.5 [Stage 5: `Gate 4~5` + `Zoro-1 글쓰기`] 시계열 AOP 초안 & SDS 블랙박스 봉인 (`drafting.rs` & `audit.rs`)

* **시계열 타임라인 초안 작성 & `factcheck.rs` 확장**:

* `Zoro-1 글쓰기`가 조치 절차서(AOP)를 작성할 때 단순 현재값뿐 아니라 시간차($\Delta t = 4.2\text{초}$), 변화율($+6.6^\circ\text{C/s}$), 트립 도달 잔여 시간($\text{TTF} = 45\text{초}$)을 포함해 서술합니다.
* `factcheck.rs`는 원문 시계열 사실 줄에 등장한 시간 숫자(`4.2`, `45`)까지 모두 대조하여 시간 왜곡 환각을 차단합니다.

* **SDS 블랙박스 감사 봉인 (`audit.rs` 확장)**:

* 사건 확정 시 `tb_audit_log`에 항공기 블랙박스(FDR)처럼 ① 사고 전후 60초 시계열 요약 벡터, ② Gate 0~4의 SDS 상태 전이 코드, ③ 인간 교대감독의 개입 반응 시간(Response Latency)을 영구 기록합니다.

---

### 4. 시계열 & SDS 데이터 구조체 명세 (`core/src/case.rs` & `core/src/sds.rs` 기획)

향후 코드 구현 시 추가될 핵심 도메인 구조체 설계입니다.

```rust
/// 1. 설비 센서 하나당 유지되는 60초 시계열 요약 및 미분 지표
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TimeSeriesMetrics {
    pub sensor_tag: String,
    /// 변곡점(Inflection Point) 최초 발생 시각 (기준점 t0, epoch ms)
    pub inflection_at_ms: i64,
    /// 최근 60초 슬라이딩 윈도우 샘플 (1Hz, 스케일 x1000 정수 보관)
    pub window_milli: Vec<i64>,
    /// 1차 미분: 초당 변화율 (dx/dt, 예: +6.6 °C/s -> 6600)
    pub slope_per_sec_milli: i64,
    /// 2차 미분: 변화 가속도 (d²x/dt²)
    pub accel_milli: i64,
    /// 현재 추세 유지 시 트립(Trip) 임계치 도달까지 남은 예상 시간 (초)
    pub estimated_ttf_secs: Option<u32>,
}

/// 2. 마인드맵 간선(Edge) 위에서의 시계열 전파 검증 결과
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TemporalPropagation {
    pub upstream_kks: String,
    pub downstream_kks: String,
    /// 실제 관측된 상·하류 변곡점 발생 시차 (Δt = t_down - t_up, 초)
    pub observed_lag_secs: f32,
    /// 배관 길이·유속 기준 물리적 허용 지연시간 범위 (예: 2.0s ~ 12.0s)
    pub expected_lag_range: (f32, f32),
    /// 시간 선후 관계 및 물리적 전파 속도가 부합하는지 여부
    pub causal_match: bool,
}

/// 3. SDS (Selective Decision & State Snapshot) 패킷
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SdsSnapshot {
    pub sds_id: String,
    /// 트리거 사유: "THRESHOLD_BREACH" | "HIGH_SLOPE_PRE_ALARM" | "FIELD_SCAN_INJECTED"
    pub trigger_kind: String,
    /// 게이트별 압축 상태 코드 (예: [G1_SIGNAL_VALID, G2_VOTING_CONSISTENT, G3_CAUSAL_LAG_MATCH])
    pub gate_state_vector: Vec<String>,
    /// SDS 직교 분할 질문별 Zoro-1 확신도 (시간축 / 물리축 / 센서축)
    pub decomposed_confidences: Vec<(String, f32)>,
    /// 최소 확신도 (자동 확정 또는 긴급 경보 기준값)
    pub min_confidence: f32,
}

```

---

### 5. 아스키(ASCII) 레이아웃: 시계열 파형 동기화 & SDS 분할 진단 패널

앞서 설계한 주제어실(MCR) 콘솔에서 **특정 설비나 병목 구간을 클릭했을 때 펼쳐지는 ‘시계열 다중 파형 동기화 + SDS 직교 분할 검증’ 화면**의 아스키 레이아웃입니다.

```text
+======================================================================================================================+
| ⏱️ [시계열 & SDS 심층 분석기] 다중 계통 시계열 파형 동기화 (Δt) 및 SDS 직교 분할 검증                [X 닫기]        |
+======================================================================================================================+
| [SDS 스냅샷 ID: SDS-20261104-0042]  트리거: 동적 기울기 돌파 (+6.6°C/s) | 트립 도달 예상 잔여시간(TTF): 🚨 45초 전!  |
+-----------------------------------------------------------------------+----------------------------------------------+
| [1. 상·하류 다중 설비 시계열 파형 동기화 차트 (t = -20s ~ +10s)]      | [2. SDS 시공간 직교 분할 검증 (Zoro-1 판단)] |
|                                                                       |                                              |
|  ① [Depth -1] 103동 주급수펌프 토출압력 (10LAB10CP001, MPa)           | 복합 알람을 3개 직교 기준으로 쪼개어 질문한  |
|  30.0 |                  .---.---.---.--- (역류 압력 적체 +0.9MPa)    | 뒤 가장 낮은 확률을 최종 확신도로 채택합니다 |
|  28.5 |──.───.───.───.──/                                             |                                              |
|       +-------------------------------------------------------------- | +------------------------------------------+ |
|  ② [Depth  0] 🔥 103동 급수조절밸브 개도율 (10LAB20CG102, %)          | | [SDS 축 1] 시간 선후 및 전파 시차(Δt)    | |
|  78.0 |──.───.───● (t=0.0s: 밸브 포지셔너 급락 변곡점!)               | | • 상류(0.0s) ➔ 하류(+4.2s) 물리유속 부합 | |
|  18.0 |           \__.───.───.───.───.─── (18.0% 기계적 고착)         | | █████████████████████████████████  0.988 | |
|       +-------------------------------------------------------------- | +------------------------------------------+ |
|  ③ [Depth +1] ⚠️ 101동 보일러 과열기 온도 (10HAH10CT001, °C)          | | [SDS 축 2] 열역학/유체 보존 방향성       | |
|  638  |                          .───●─── (t=+4.2s: 트립선 635°C 돌파)| | • 급수유량(-39.5%) ➔ 증기온도(+28°C) 일치| |
|  610  |──.───.───.───.───.───.──/ (기울기 dx/dt = +6.6°C/s 급등)      | | ████████████████████████████████   0.974 | |
|       +----------+-------+-----------+---------------+--------------- | +------------------------------------------+ |
|      -10s       -5s     t=0.0s     t=+1.8s         t=+4.2s      현재  | | [SDS 축 3] 센서 건전성 (Flatline/드리프트| |
|                         (발원)    (상류역류)      (하류전파)          | | • 3채널 보팅 및 슬루율 정상 물리범위 내  | |
|                                                                       | | █████████████████████████████████  0.981 | |
|  📌 [시계열 인과 시차(Δt) 판정 결과]                                  | +------------------------------------------+ |
|  • 관측 시차 Δt = +4.2초 (배관 물리 허용 범위 2.0초 ~ 12.0초 내 일치) |  ➔ SDS 최종 채택 확신도 (Min): 0.974 (확정) |
|  • 결론: 101동 보일러 과열은 4.2초 전 발생한 103동 밸브 고착이 원인임 |  ➔ GdnState 프리필 캐시 적중: 0.14초 소요   |
+-----------------------------------------------------------------------+----------------------------------------------+

```

---

### 6. 기존 로드맵(P11 ~ P14)에의 통합 반영 계획

별도의 단계를 늘리지 않고, 앞서 수립한 **P11 ~ P14 각 단계에 시계열 및 SDS 모듈을 자연스럽게 내장**합니다.

* **P11 (시드 DB 및 시나리오 확장)**:
* `synthetic_db.json`의 `edges`에 배관/전력선별 물리적 전파 지연시간 범위(`lag_secs: [min, max]`)를 추가하고, `plant_scenarios.json`에 60초 시계열 파형 배열과 변곡점 시각(`inflection_at_ms`)을 추가합니다.

* **P12 (`SigLIP2` 델타 패치 & `Gate 1~2` 시계열 필터)**:
* `gate1_formal.rs`에 센서 Flatline(동결) 및 불가능한 슬루율(Spike) 필터를 추가하고, `gate2_consistency.rs`에 스캔 시각($t_{\text{scan}}$) 기준 타임스탬프 정렬 대조를 구현합니다.

* **P13 (`Gate 3` 인과 시차 $\Delta t$ 역추적 & SDS 3축 분할 질문 + `GdnState` 스냅샷)**:
* `graph.rs`에 $\Delta t$ 범위 매칭을 추가하고, `gate3_substantive.rs`의 `judge_by_criteria`를 **시간축·물리축·센서축 3단계 SDS 분할 질문**으로 구성합니다. 또한 `ai::zoro`의 `GdnState` 프리픽스 스냅샷 캐싱을 연결합니다.

* **P14 (UI 시계열 다중 파형 차트 & 팔란티어 타임머신 연동)**:
* `PlantTopologyView.tsx` 하단 타임머신 슬라이더를 움직일 때 위 `TimeSeriesMetrics`의 60초 윈도우 값을 읽어 도면·Mermaid·옵시디언 뷰의 색상과 병목 배지가 시간순($t=0.0\text{s} \rightarrow +1.8\text{s} \rightarrow +4.2\text{s}$)으로 리플레이되도록 연결합니다.
