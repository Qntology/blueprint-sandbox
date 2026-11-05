//! 엔진 잠금을 짧게 쥐는 실행: 준비 → 모델 → 반영.
//!
//! 앱은 엔진을 `Mutex` 하나로 지킵니다. 판단 · 글쓰기 모델이 도는 수 초 ~ 수십 초 동안 그 잠금을 쥐면 사건 목록 · 상태 줄 ·
//! 교대감독의 조회와 결정이 모두 멈춥니다. 그래서 모델을 부르는 일을 세 걸음으로 나눕니다.
//!
//! 1. 준비 (엔진 잠금): 사건 번호를 받고, 모델 손잡이(`Minds`, `Arc`)와 분석할 입력의 사본을 만듭니다.
//! 2. 모델 (엔진 잠금 없음): Gate 0 ~ 4 분석 · 완화 조치 네 기준 · AOP 초안 글쓰기. 앱은 이때 GPU 잠금만 쥡니다.
//! 3. 반영 (엔진 잠금): 결과를 세션에 넣고 Gate 4-B · 5 를 마칩니다. 그사이 세션이 바뀌었으면(세션 판 번호 `rev`)
//!    결과를 버리고 준비부터 다시 합니다 (초안 다시 쓰기는 버리기만 합니다).
//!
//! GPU 잠금은 모델 계산 전체를 덮은 채로 둡니다. 상주 관리자는 다른 쪽이 핸들을 쥔 가중치 묶음을 내리지 않고 오류를 내므로,
//! 모델 계산은 여전히 한 번에 하나여야 합니다 (판단 도중 gocr 가 끼어들면 둘 중 하나가 실패합니다).

use std::cell::RefCell;
use std::sync::Mutex;

use anyhow::{bail, Result};
use chrono::{DateTime, Utc};

use super::analysis::{analyze, redraft as draft_again};
use super::pipeline::{Engine, Outcome};
use crate::case::{FieldReport, Incident};
use crate::lattice::Lattice;
use crate::mitigation::{self, Mitigation};

/// 세션이 바뀌어 반영하지 못했을 때 준비부터 다시 하는 횟수
const ATTEMPTS: usize = 3;

/// 반영 걸음의 결과
#[derive(Debug, Clone, PartialEq)]
pub enum Commit<T> {
    Done(T),
    /// 모델이 도는 사이 세션이 바뀌었습니다 (현장 보고로 다시 분석됨 등)
    Stale,
}

/// 엔진 잠금과 모델 계산을 맡는 쪽. 앱은 `Mutex<Engine>` 과 GPU 잠금으로, 시험 · 예제는 엔진 하나로 씁니다.
pub trait EngineAccess {
    /// 엔진을 잠그고 `f` 를 부릅니다. 여기서는 모델을 돌리지 않습니다.
    fn engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R;

    /// 모델을 돌립니다. 엔진은 잠그지 않습니다 (앱은 GPU 잠금을 쥡니다).
    fn infer<R>(&self, f: impl FnOnce() -> R) -> R {
        f()
    }
}

impl EngineAccess for Mutex<Engine> {
    fn engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        f(&mut self.lock().unwrap_or_else(|p| p.into_inner()))
    }
}

/// 엔진 하나를 그대로 쓰는 경우 (잠금 없음)
impl EngineAccess for RefCell<&mut Engine> {
    fn engine<R>(&self, f: impl FnOnce(&mut Engine) -> R) -> R {
        f(&mut self.borrow_mut())
    }
}

/// 사건 하나를 Gate 0 부터 돌립니다. `quick` 이면 글쓰기 없이 (AOP 초안은 사실 줄 템플릿, 물리엔진 모드).
pub fn run(access: &impl EngineAccess, db: &Lattice, incident: Incident, now: DateTime<Utc>, quick: bool) -> Result<Outcome> {
    let minds = access.engine(|e| e.minds());
    let minds = if quick { minds.without_writer() } else { minds };
    let analysis = access.infer(|| analyze(db, &incident, minds.brains(), &Engine::sds_id(&incident)))?;
    access.engine(|e| e.commit_run(incident, analysis, now))
}

/// 사건 번호를 받아 사건을 만들고 돌립니다 (`prefix`: 알람 `INC`, 현장 보고 `FLD`).
pub fn open(access: &impl EngineAccess, db: &Lattice, prefix: &str, now: DateTime<Utc>, quick: bool, make: impl FnOnce(&str) -> Incident) -> Result<(String, Outcome)> {
    let id = access.engine(|e| e.next_id(prefix, now));
    let incident = make(&id);
    let outcome = run(access, db, incident, now, quick)?;
    Ok((id, outcome))
}

/// 진행 중인 사건에 현장 보고를 붙이고 다시 분석합니다.
pub fn attach_report(access: &impl EngineAccess, db: &Lattice, incident_id: &str, report: FieldReport, now: DateTime<Utc>) -> Result<Outcome> {
    for _ in 0..ATTEMPTS {
        let (job, minds) = access.engine(|e| e.attach_job(incident_id, &report).map(|j| (j, e.minds())))?;
        let analysis = access.infer(|| analyze(db, &job.incident, minds.brains(), &Engine::sds_id(&job.incident)));
        match access.engine(|e| e.commit_attach(job, analysis, now))? {
            Commit::Done(out) => return Ok(out),
            Commit::Stale => continue,
        }
    }
    bail!("사건 {incident_id} 이 분석하는 사이 계속 바뀌어 현장 보고를 반영하지 못했습니다")
}

/// 진행 중 사건의 AOP 초안을 글쓰기 모델로 다시 씁니다. 판정은 다시 묻지 않습니다 (그 질문들만큼 GPU 를 덜 씀).
/// 그사이 세션이 바뀌었거나 닫혔으면 버리고 false.
pub fn redraft(access: &impl EngineAccess, db: &Lattice, incident_id: &str) -> Result<bool> {
    let Some(job) = access.engine(|e| e.redraft_job(incident_id))? else { return Ok(false) };
    let drafted = access.infer(|| draft_again(db, &job.incident, &job.analysis, job.writer.as_ref()));
    Ok(access.engine(|e| e.commit_redraft(job, drafted)))
}

/// Tier 1 완화 조치를 세우고 네 기준으로 묻습니다 (자동이면 실행 기록까지).
pub fn propose_mitigation(access: &impl EngineAccess, db: &Lattice, incident_id: &str, now: DateTime<Utc>) -> Result<Option<Mitigation>> {
    for _ in 0..ATTEMPTS {
        let Some(job) = access.engine(|e| e.mitigation_job(db, incident_id))? else { return Ok(None) };
        let decided = access.infer(|| mitigation::judge(&job.facts, job.model.as_deref()))?;
        match access.engine(|e| e.commit_mitigation(db, job, decided, now))? {
            Commit::Done(m) => return Ok(Some(m)),
            Commit::Stale => continue,
        }
    }
    bail!("사건 {incident_id} 이 판단하는 사이 계속 바뀌어 완화 조치를 내지 못했습니다")
}
