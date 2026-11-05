//! 엔진 잠금 점유 시간 (개선안 1절: 준비 → 모델 → 반영).
//!
//! 판단 · 글쓰기 자리에 일부러 느린 대역을 꽂고, 한 스레드가 사건을 분석하는 동안 다른 스레드가 5 ms 마다 엔진을 조회합니다
//! (앱의 사건 목록 · 상태 줄 폴링과 같은 일). 예전처럼 엔진을 잠근 채 모델을 돌리면 조회가 분석 내내 기다리고,
//! 세 걸음으로 나누면 엔진 잠금은 준비 · 반영 때만 잠깐 잡힙니다.

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::Result;
use blueprint_core::decision::{DecisionModel, DecisionTask};
use blueprint_core::drafting::TextWriter;
use blueprint_core::lattice::Lattice;
use blueprint_core::policy::AutomationPolicy;
use blueprint_core::scenario::ScenarioSet;
use blueprint_core::state_machine::{staged, Engine, EngineAccess, Outcome};
use blueprint_core::verdict::Verdict;

const ASK_MS: u64 = 60;
const WRITE_MS: u64 = 250;

/// 질문마다 `ASK_MS` 를 쓰고 기대한 판정에 0.97 을 주는 판단 대역. 질문 수를 셉니다.
struct SlowOracle {
    want: Verdict,
    asked: AtomicUsize,
}

impl DecisionModel for SlowOracle {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        thread::sleep(Duration::from_millis(ASK_MS));
        let n = task.options.len();
        let pick = task.options.iter().position(|v| *v == self.want).unwrap_or(0);
        Ok((0..n).map(|i| if i == pick { 0.97 } else { 0.03 / (n - 1).max(1) as f32 }).collect())
    }
}

struct Shared(Arc<SlowOracle>);

impl DecisionModel for Shared {
    fn probabilities(&self, task: &DecisionTask) -> Result<Vec<f32>> {
        self.0.probabilities(task)
    }
}

/// `WRITE_MS` 를 쓰고 사실 줄 하나를 옮겨 쓰는 글쓰기 대역 (숫자 대조를 통과합니다)
struct SlowCopycat;

impl TextWriter for SlowCopycat {
    fn complete(&self, _system: &str, user: &str, _max: usize) -> Result<String> {
        thread::sleep(Duration::from_millis(WRITE_MS));
        let line = user.lines().find(|l| l.starts_with("상류 이상 설비") || l.starts_with("경보 계측값") || l.starts_with("계측 위치")).unwrap_or("원인 진단");
        Ok(format!("{} 때문입니다.", line.split_once(": ").map(|(_, v)| v).unwrap_or(line)))
    }
}

/// 앱의 `Desk` 와 같은 모양: 엔진 잠금 + GPU 잠금. 엔진 잠금을 쥔 시간을 잽니다.
struct Desk {
    engine: Mutex<Engine>,
    gpu: Mutex<()>,
    holds: Mutex<Vec<Duration>>,
}

impl EngineAccess for Desk {
    fn engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        let mut guard = self.engine.lock().unwrap();
        let held = Instant::now();
        let out = f(&mut guard);
        drop(guard);
        self.holds.lock().unwrap().push(held.elapsed());
        out
    }

    fn infer<R>(&self, f: impl FnOnce() -> R) -> R {
        let _gpu = self.gpu.lock().unwrap();
        f()
    }
}

fn desk(want: Verdict) -> (Arc<Desk>, Arc<SlowOracle>) {
    let oracle = Arc::new(SlowOracle { want, asked: AtomicUsize::new(0) });
    let engine = Engine::new(b"salt", AutomationPolicy::default(), Some(Box::new(Shared(oracle.clone())))).with_writer(Box::new(SlowCopycat));
    (Arc::new(Desk { engine: Mutex::new(engine), gpu: Mutex::new(()), holds: Mutex::new(Vec::new()) }), oracle)
}

/// 다른 스레드가 일을 하는 동안 5 ms 마다 엔진을 조회해, 조회 한 번이 기다린 가장 긴 시간과 조회 횟수를 돌려줍니다.
fn poll_while(desk: &Arc<Desk>, busy: &AtomicBool) -> (Duration, usize) {
    let (mut worst, mut n) = (Duration::ZERO, 0);
    while busy.load(Ordering::SeqCst) {
        let t = Instant::now();
        let count = desk.engine.lock().unwrap().store.active_count();
        worst = worst.max(t.elapsed());
        n += 1;
        std::hint::black_box(count);
        thread::sleep(Duration::from_millis(5));
    }
    (worst, n)
}

fn p2() -> (Lattice, ScenarioSet) {
    (Lattice::embedded().unwrap(), ScenarioSet::embedded().unwrap())
}

#[test]
fn queries_do_not_wait_for_the_model_when_the_engine_lock_is_split() {
    let (db, set) = p2();
    let db = Arc::new(db);
    let set = Arc::new(set);
    let s = set.get("P2").unwrap();
    let now = s.time(s.now_s);

    // 예전 방식: 엔진을 잠근 채 분석 (판단 질문 + 초안 글쓰기)
    let (old, _) = desk(Verdict::G3AdverseUpstreamCause);
    let busy = Arc::new(AtomicBool::new(true));
    let worker = {
        let (old, db, set, busy) = (old.clone(), db.clone(), set.clone(), busy.clone());
        thread::spawn(move || {
            let s = set.get("P2").unwrap();
            let t = Instant::now();
            let out = {
                let mut engine = old.engine.lock().unwrap();
                engine.run(&db, s.incident(&set, "INC-P2-OLD"), now)
            };
            busy.store(false, Ordering::SeqCst);
            (out, t.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(20));
    let (old_wait, _) = poll_while(&old, &busy);
    let (out, old_total) = worker.join().unwrap();
    assert!(matches!(out.unwrap(), Outcome::SupervisorReview { .. }));

    // 나눈 방식: 준비(잠금) → 모델(잠금 없음) → 반영(잠금)
    let (new, oracle) = desk(Verdict::G3AdverseUpstreamCause);
    let busy = Arc::new(AtomicBool::new(true));
    let worker = {
        let (new, db, set, busy) = (new.clone(), db.clone(), set.clone(), busy.clone());
        thread::spawn(move || {
            let s = set.get("P2").unwrap();
            let t = Instant::now();
            let out = staged::open(&*new, &db, "INC", now, false, |id| s.incident(&set, id));
            busy.store(false, Ordering::SeqCst);
            (out, t.elapsed())
        })
    };
    thread::sleep(Duration::from_millis(20));
    let (new_wait, polls) = poll_while(&new, &busy);
    let (out, new_total) = worker.join().unwrap();
    let (id, out) = out.unwrap();
    assert!(matches!(&out, Outcome::SupervisorReview { judgement: Some(j), .. } if j.verdict == Verdict::G3AdverseUpstreamCause), "{out:?}");
    assert!(oracle.asked.load(Ordering::SeqCst) >= 4, "SDS 축 질문을 모델에 물었습니다");
    let held = new.holds.lock().unwrap().clone();
    let longest = held.iter().max().copied().unwrap_or_default();
    let sum: Duration = held.iter().sum();
    println!(
        "[엔진 잠금] 예전: 분석 {:.0} ms 동안 조회가 최대 {:.0} ms 기다림 · 나눔: 분석 {:.0} ms, 조회 {polls}번 최대 대기 {:.1} ms, 엔진 잠금 {}번 · 가장 긴 {:.2} ms · 합 {:.2} ms",
        old_total.as_secs_f64() * 1e3,
        old_wait.as_secs_f64() * 1e3,
        new_total.as_secs_f64() * 1e3,
        new_wait.as_secs_f64() * 1e3,
        held.len(),
        longest.as_secs_f64() * 1e3,
        sum.as_secs_f64() * 1e3
    );
    // 모델이 도는 시간(질문 4개 이상 + 글쓰기 한 번)보다 조회가 훨씬 덜 기다려야 합니다.
    let model_floor = Duration::from_millis(4 * ASK_MS + WRITE_MS);
    assert!(old_wait + Duration::from_millis(50) >= model_floor, "예전 방식은 조회가 모델 시간만큼 기다립니다: {old_wait:?}");
    assert!(new_total >= model_floor);
    assert!(polls >= 20, "분석하는 동안 조회가 계속 됩니다: {polls}");
    // 디버그 빌드 · 공용 CI 러너의 스케줄링 지연에도 흔들리지 않게 여유를 둡니다 (실측은 위 줄에 찍힘, 개선안의 목표는 합 10 ms 미만).
    // 그래도 모델 시간(질문 4개 + 글쓰기 = 490 ms 이상)보다는 한참 짧아야 합니다.
    assert!(longest < Duration::from_millis(200), "준비 · 반영 한 번이 엔진을 오래 쥐었습니다: {longest:?}");
    assert!(new_wait < Duration::from_millis(200), "조회가 모델을 기다렸습니다: {new_wait:?}");
    assert!(new.engine.lock().unwrap().store.active(&id).is_some());
}

#[test]
fn a_redraft_writes_again_without_asking_the_model_and_is_dropped_when_the_case_changed() {
    let (db, set) = p2();
    let s = set.get("P2").unwrap();
    let now = s.time(s.now_s);
    let (desk, oracle) = desk(Verdict::G3AdverseUpstreamCause);
    // 물리엔진 모드처럼 글쓰기 없이 먼저 분석합니다.
    let (id, _) = staged::open(&*desk, &db, "INC", now, true, |id| s.incident(&set, id)).unwrap();
    let draft = |d: &Desk| d.engine.lock().unwrap().store.active(&id).unwrap().analysis.draft.clone().unwrap();
    assert!(!draft(&desk).by_model, "처음에는 템플릿");
    let asked = oracle.asked.load(Ordering::SeqCst);
    assert!(staged::redraft(&*desk, &db, &id).unwrap());
    assert_eq!(oracle.asked.load(Ordering::SeqCst), asked, "판정은 다시 묻지 않습니다");
    let d = draft(&desk);
    assert!(d.by_model && d.problems.is_empty(), "{d:?}");
    let stage5 = desk.engine.lock().unwrap().store.active(&id).unwrap().analysis.stages.iter().find(|x| x.stage == 5).cloned().unwrap();
    assert!(stage5.summary.contains("초안"), "{stage5:?}");

    // 다시 쓰는 사이 세션이 다시 열리면(현장 보고 재분석과 같음) 옛 입력으로 쓴 초안은 버립니다.
    let job = desk.engine.lock().unwrap().redraft_job(&id).unwrap().unwrap();
    {
        let mut e = desk.engine.lock().unwrap();
        let session = e.store.take(&id).unwrap();
        e.store.open(session).unwrap();
    }
    let drafted = blueprint_core::state_machine::analysis::redraft(&db, &job.incident, &job.analysis, job.writer.as_ref());
    assert!(drafted.is_some());
    assert!(!desk.engine.lock().unwrap().commit_redraft(job, drafted), "판 번호가 바뀌었으면 넣지 않습니다");
}

#[test]
fn a_field_report_is_attached_while_the_lock_is_free_and_a_stale_result_is_redone() {
    let (db, set) = p2();
    let s = set.get("P1").unwrap();
    let (desk, _) = desk(Verdict::G2SensorDriftIsolated);
    let (id, out) = staged::open(&*desk, &db, "INC", s.time(s.now_s), false, |id| s.incident(&set, id)).unwrap();
    assert!(matches!(out, Outcome::AwaitingField { .. }), "{out:?}");
    let report = s.field_reports(|w| db.person(w).map(|p| p.name.clone())).remove(0);
    let taken = report.taken_at;

    // 준비한 뒤 세션이 바뀌면(다른 반영) 반영은 Stale 이고, 세션은 그대로입니다.
    let job = desk.engine.lock().unwrap().attach_job(&id, &report).unwrap();
    {
        let mut e = desk.engine.lock().unwrap();
        let session = e.store.take(&id).unwrap();
        e.store.open(session).unwrap();
    }
    let minds = desk.engine.lock().unwrap().minds();
    let analysis = blueprint_core::state_machine::analysis::analyze(&db, &job.incident, minds.brains(), &Engine::sds_id(&job.incident));
    let stale = desk.engine.lock().unwrap().commit_attach(job, analysis, taken).unwrap();
    assert!(matches!(stale, staged::Commit::Stale));
    assert!(desk.engine.lock().unwrap().store.active(&id).unwrap().incident.reports.is_empty(), "보고가 붙지 않았습니다");

    // 처음부터 다시 하면 붙습니다 (모델이 도는 동안 엔진은 비어 있음).
    let out = staged::attach_report(&*desk, &db, &id, report, taken).unwrap();
    assert!(matches!(&out, Outcome::SupervisorReview { reason, .. } if reason.contains("트립 보류")), "{out:?}");
    let e = desk.engine.lock().unwrap();
    let session = e.store.active(&id).unwrap();
    assert_eq!(session.incident.reports.len(), 1);
    assert!(session.analysis.draft.as_ref().is_some_and(|d| d.by_model));
}

#[test]
fn a_bad_signature_on_an_attached_report_keeps_the_case_and_its_judgement() {
    let (db, set) = p2();
    let s = set.get("P1").unwrap();
    let (desk, _) = desk(Verdict::G2SensorDriftIsolated);
    let (id, _) = staged::open(&*desk, &db, "INC", s.time(s.now_s), false, |id| s.incident(&set, id)).unwrap();
    let before = desk.engine.lock().unwrap().store.active(&id).unwrap().judgement.clone();
    let mut report = s.field_reports(|w| db.person(w).map(|p| p.name.clone())).remove(0);
    report.signature = Some(blueprint_core::case::Signature { stroke_count: 1, bbox_area_ratio: 0.001, single_line: true });
    let taken = report.taken_at;
    let out = staged::attach_report(&*desk, &db, &id, report, taken).unwrap();
    assert!(matches!(out, Outcome::FieldRetry { verdict: Verdict::G1SignInvalidRetry, .. }), "{out:?}");
    let e = desk.engine.lock().unwrap();
    let session = e.store.active(&id).expect("사건은 남습니다");
    assert!(session.incident.reports.is_empty());
    assert_eq!(session.judgement, before, "판정 후보를 잃지 않습니다");
}
