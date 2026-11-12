//! 다국어 N:N 검색 평가: 12개 언어 질의 × 데이터 언어(원장 언어) ko · en · ja · de, 화면 순서로 채점 (logis-center 방식).
//!
//!   cargo run --release -p blueprint-core --example search_ml_eval [-- --split dev|held|all] [--json out.json] [--verbose]
//!                                                                  [--modes rules,stanza,full] [--only <template>]
//!
//! 질의는 `core/fixtures/search_ml/queries.json` (`scripts/search_ml/queries.py`), 데이터 언어 원장은
//! `core/fixtures/search_ml/synthetic_db.{en,ja,de}.json` (`scripts/search_ml/gen_ledgers.py`) 입니다. 한국어 원장은 앱에 든 원장 그대로
//! (en.json 영어 이름 포함)이고, 나머지 데이터 언어 원장에는 그 언어 이름만 있습니다.
//! dev 묶음(발전소 · 지금 계측 P2 · 따옴표 이름)으로 규칙을 고치고, held 묶음(데이터센터 · 지금 계측 D2 · 뜻풀이)은 마지막에 한 번 봅니다.

use std::collections::BTreeMap;
use std::time::Instant;

use blueprint_core::domain::DomainMode;
use blueprint_core::lattice::Lattice;
use blueprint_core::search::*;
use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
struct Item {
    id: String,
    split: String,
    template: String,
    domain: String,
    live: Option<String>,
    data_lang: String,
    lang: String,
    query: String,
    expect: String,
    allow: usize,
}

#[derive(Default, Clone)]
struct Acc {
    n: usize,
    mrr: f64,
    hit1: usize,
    within: usize,
    lang_ok: usize,
    ms: f64,
}

impl Acc {
    fn add(&mut self, rank: Option<usize>, allow: usize, lang_ok: bool, ms: f64) {
        self.n += 1;
        self.mrr += rank.map_or(0.0, |k| 1.0 / k as f64);
        self.hit1 += usize::from(rank == Some(1));
        self.within += usize::from(rank.is_some_and(|k| k <= allow));
        self.lang_ok += usize::from(lang_ok);
        self.ms += ms;
    }

    fn cell(&self) -> String {
        if self.n == 0 {
            return "-".into();
        }
        format!("{:.3}", self.mrr / self.n as f64)
    }

    fn line(&self) -> String {
        if self.n == 0 {
            return "-".into();
        }
        format!(
            "MRR {:.3} · Hit@1 {}/{} · 허용 {}/{} · 언어 판정 {}/{} · {:.0} ms",
            self.mrr / self.n as f64,
            self.hit1,
            self.n,
            self.within,
            self.n,
            self.lang_ok,
            self.n,
            self.ms / self.n as f64
        )
    }
}

fn arg(name: &str) -> Option<String> {
    let args: Vec<String> = std::env::args().collect();
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1).cloned())
}

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures").join("search_ml");
    let verbose = std::env::args().any(|a| a == "--verbose");
    let split = arg("--split").unwrap_or_else(|| "dev".into());
    let only = arg("--only");
    let json_out = arg("--json");
    let want_modes = arg("--modes").unwrap_or_else(|| "rules,stanza,full".into());
    let items: Vec<Item> = serde_json::from_str(&std::fs::read_to_string(root.join("queries.json")).expect("queries.json")).expect("질의 형식");
    let mut items: Vec<Item> = items
        .into_iter()
        .filter(|x| split == "all" || x.split == split)
        .filter(|x| only.as_deref().is_none_or(|t| x.template == t))
        .collect();
    // 질의 언어 차례로 돌립니다: 한국어 · 영어 밖 Stanza 는 최근 두 언어만 메모리에 두므로(models::EXTRA_LANGS) 언어를 오가면 매번 다시 읽습니다.
    // 질의마다 결과는 서로 독립이라 차례는 점수를 바꾸지 않습니다.
    items.sort_by(|a, b| a.lang.cmp(&b.lang).then(a.id.cmp(&b.id)));

    let t = Instant::now();
    let full = SearchModels::load_default();
    println!("모델: {} ({:.1} s) {:?}", full.summary(), t.elapsed().as_secs_f32(), full.notes);
    let stanza_only = SearchModels::load(Some(&blueprint_core::ai::model_store::model_dir("stanza")), None);
    let mut modes: Vec<(&str, Option<&SearchModels>)> = Vec::new();
    if want_modes.contains("rules") {
        modes.push(("규칙만", None));
    }
    if want_modes.contains("stanza") && !stanza_only.stanza_langs().is_empty() {
        modes.push(("+Stanza", Some(&stanza_only)));
    }
    if want_modes.contains("full") && full.embedder.is_some() {
        modes.push(("+Stanza+임베딩", Some(&full)));
    }

    // 원장: (영역, 데이터 언어) → 원장
    let mut ledgers: BTreeMap<(String, String), Lattice> = BTreeMap::new();
    ledgers.insert(("plant".into(), "ko".into()), Lattice::embedded().expect("원장"));
    ledgers.insert(("aidc".into(), "ko".into()), Lattice::embedded_for(DomainMode::AiDataCenter).expect("데이터센터 원장"));
    for dl in ["en", "ja", "de"] {
        let text = std::fs::read_to_string(root.join(format!("synthetic_db.{dl}.json"))).expect("데이터 언어 원장");
        ledgers.insert(("plant".into(), dl.into()), Lattice::from_json(&text).expect("데이터 언어 원장 형식"));
    }
    // 검색 표: (영역, 데이터 언어, 시나리오) → 검색 표
    let mut corpora: BTreeMap<(String, String, String), Corpus> = BTreeMap::new();
    for ((domain, dl), db) in &ledgers {
        corpora.insert((domain.clone(), dl.clone(), String::new()), Corpus::build(db, None, None));
        let sc = if domain == "plant" { EVAL_LIVE_SCENARIO } else { EVAL_AIDC_LIVE_SCENARIO };
        corpora.insert((domain.clone(), dl.clone(), sc.to_string()), live_corpus(db, sc).expect("시나리오"));
    }
    if let Some(m) = modes.last().and_then(|m| m.1) {
        let t = Instant::now();
        for ((domain, dl, sc), corpus) in &corpora {
            if sc.is_empty() {
                let _ = search_with(&ledgers[&(domain.clone(), dl.clone())], corpus, "준비", Some(m));
            }
        }
        println!("검색 표 · 개념 뱅크 임베딩 {:.1} s", t.elapsed().as_secs_f32());
    }

    let langs = ["ko", "en", "de", "fr", "es", "it", "pt", "nl", "cs", "ar", "ja", "zh-hans", "id"];
    let data_langs = ["ko", "en", "ja", "de"];
    let mut records = Vec::new();
    for (mode, m) in &modes {
        let mut by: BTreeMap<(String, String), Acc> = BTreeMap::new();
        let mut by_template: BTreeMap<String, Acc> = BTreeMap::new();
        let mut total = Acc::default();
        let mut misses = Vec::new();
        for it in &items {
            let db = &ledgers[&(it.domain.clone(), it.data_lang.clone())];
            let corpus = &corpora[&(it.domain.clone(), it.data_lang.clone(), it.live.clone().unwrap_or_default())];
            let t = Instant::now();
            let r = search_with(db, corpus, &it.query, *m);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let rank = r.hits.iter().position(|h| h.doc.id == it.expect).map(|p| p + 1);
            let lang_ok = it.lang == "id" || r.lang == it.lang;
            by.entry((it.lang.clone(), it.data_lang.clone())).or_default().add(rank, it.allow, lang_ok, ms);
            by.entry((it.lang.clone(), "*".into())).or_default().add(rank, it.allow, lang_ok, ms);
            by_template.entry(it.template.clone()).or_default().add(rank, it.allow, lang_ok, ms);
            total.add(rank, it.allow, lang_ok, ms);
            let ok = rank.is_some_and(|k| k <= it.allow);
            if verbose || !ok {
                let conds: Vec<String> = r.conditions.iter().map(|c| format!("{} {}{}", c.kind, c.text, if c.applied { "" } else { "(힌트)" })).collect();
                let top: Vec<String> = r.hits.iter().take(3).map(|h| h.doc.id.clone()).collect();
                let segs: Vec<String> = r.segments.iter().map(|s| format!("'{}'→{}({:.2})", s.text, s.label, s.cos)).collect();
                misses.push(format!(
                    "    {} [{}] {} → {} 순위 {} · 언어 {} · 위 {top:?} · 조건 {} · 조각 {}",
                    if ok { "·" } else { "✗" },
                    it.id,
                    it.query,
                    it.expect,
                    rank.map_or("-".into(), |k| k.to_string()),
                    r.lang,
                    conds.join(", "),
                    segs.join(" ")
                ));
            }
            if json_out.is_some() {
                records.push(serde_json::json!({
                    "mode": mode, "id": it.id, "template": it.template, "lang": it.lang, "data_lang": it.data_lang,
                    "detected": r.lang, "rank": rank, "allow": it.allow, "expect": it.expect,
                    "top": r.hits.iter().take(5).map(|h| h.doc.id.clone()).collect::<Vec<_>>(),
                    "conditions": r.conditions.iter().map(|c| format!("{} {}{}", c.kind, c.text, if c.applied { "" } else { "(hint)" })).collect::<Vec<_>>(),
                    "segments": r.segments.iter().map(|s| format!("{}→{}", s.text, s.label)).collect::<Vec<_>>(),
                }));
            }
        }
        println!("\n## {mode} ({split}, {} 질의) — {}", items.len(), total.line());
        if verbose || misses.len() <= 400 {
            for l in &misses {
                println!("{l}");
            }
        } else {
            println!("  (허용 순위 밖 {}건, --verbose 로 전부)", misses.len());
        }
        print!("\n| 질의 언어 \\ 데이터 언어 |");
        for dl in data_langs {
            print!(" {dl} |");
        }
        println!(" 전체 | 언어 판정 |");
        println!("| --- |{}", " --- |".repeat(data_langs.len() + 2));
        for l in langs {
            let Some(all) = by.get(&(l.to_string(), "*".to_string())) else { continue };
            print!("| {l} |");
            for dl in data_langs {
                print!(" {} |", by.get(&(l.to_string(), dl.to_string())).map(Acc::cell).unwrap_or_else(|| "-".into()));
            }
            println!(" {} ({}/{}) | {}/{} |", all.cell(), all.within, all.n, all.lang_ok, all.n);
        }
        println!("\n| 묶음 | 결과 |\n| --- | --- |");
        for (t, a) in &by_template {
            println!("| {t} | {} |", a.line());
        }
    }
    if let Some(path) = json_out {
        std::fs::write(&path, serde_json::to_string_pretty(&records).unwrap()).expect("결과 쓰기");
        println!("\n결과 {path} ({}건)", records.len());
    }
}
