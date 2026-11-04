//! 자연어 검색 평가 (logis-center 방식: 조건 필터를 거친 화면 순서로 채점).
//!
//!   cargo run -p blueprint-core --example search_eval
//!
//! 질의마다 정답의 화면 순위와 걸린 조건(힌트로 내린 것 포함)을 찍고 MRR · Hit@1 을 냅니다. 모델은 쓰지 않습니다.
//! 두 번 잽니다: 지금 계측 없이(질적 상태는 힌트), 그리고 시나리오 P2 를 끝까지 재생한 지금 계측으로(질적 상태가 필터).

use blueprint_core::lattice::Lattice;
use blueprint_core::search::{live_corpus, search, Corpus, EVAL_LIVE_QUERIES, EVAL_LIVE_SCENARIO, EVAL_QUERIES};

fn run(db: &Lattice, corpus: &Corpus, queries: &[(&str, &str, usize)]) {
    let (mut mrr, mut hit1) = (0.0, 0);
    for (q, want, _) in queries {
        let r = search(db, corpus, q);
        let rank = r.hits.iter().position(|h| h.doc.id == *want).map(|p| p + 1);
        mrr += rank.map_or(0.0, |k| 1.0 / k as f64);
        hit1 += usize::from(rank == Some(1));
        let conds: Vec<String> = r.conditions.iter().map(|c| format!("{} {}{}", c.kind, c.text, if c.applied { "" } else { " (힌트)" })).collect();
        let why = r.hits.iter().find(|h| h.doc.id == *want).map(|h| h.why.iter().filter(|w| w.contains("상태")).cloned().collect::<Vec<_>>().join(" · ")).unwrap_or_default();
        println!("| {q} | {want} | {} | {} | {why} |", rank.map_or("-".into(), |k| k.to_string()), conds.join(", "));
    }
    let n = queries.len() as f64;
    println!("MRR {:.3} · Hit@1 {hit1}/{}", mrr / n, queries.len());
}

fn main() {
    let db = Lattice::embedded().expect("원장");
    println!("## 지금 계측 없음 (원장 정상치)");
    run(&db, &Corpus::build(&db, None, None), &EVAL_QUERIES);
    println!();
    println!("## 지금 계측 있음 (시나리오 {EVAL_LIVE_SCENARIO} 재생 끝 시각)");
    let live = live_corpus(&db, EVAL_LIVE_SCENARIO).expect("시나리오");
    run(&db, &live, &EVAL_LIVE_QUERIES);
}
