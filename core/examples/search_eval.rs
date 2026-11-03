//! 자연어 검색 평가 (logis-center 방식: 조건 필터를 거친 화면 순서로 채점).
//!
//!   cargo run -p blueprint-core --example search_eval
//!
//! 질의마다 정답의 화면 순위와 걸린 조건(힌트로 내린 것 포함)을 찍고 MRR · Hit@1 을 냅니다. 모델은 쓰지 않습니다.

use blueprint_core::lattice::Lattice;
use blueprint_core::search::{search, Corpus, EVAL_QUERIES};

fn main() {
    let db = Lattice::embedded().expect("원장");
    let corpus = Corpus::build(&db, None, None);
    let (mut mrr, mut hit1) = (0.0, 0);
    for (q, want, _) in EVAL_QUERIES {
        let r = search(&db, &corpus, q);
        let rank = r.hits.iter().position(|h| h.doc.id == want).map(|p| p + 1);
        mrr += rank.map_or(0.0, |k| 1.0 / k as f64);
        hit1 += usize::from(rank == Some(1));
        let conds: Vec<String> = r.conditions.iter().map(|c| format!("{} {}{}", c.kind, c.text, if c.applied { "" } else { "(힌트)" })).collect();
        println!("| {q} | {want} | {} | {} |", rank.map_or("-".into(), |k| k.to_string()), conds.join(", "));
    }
    let n = EVAL_QUERIES.len() as f64;
    println!("MRR {:.3} · Hit@1 {hit1}/{}", mrr / n, EVAL_QUERIES.len());
}
