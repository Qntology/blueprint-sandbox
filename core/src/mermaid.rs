//! Mermaid 계통 다이어그램 원본 (기획 Part 8 6.1절 뷰 ②).
//!
//! 섹터(Lv2)마다 `subgraph` 로 묶고 설비 간 배관 · 전력 흐름을 화살표로 그립니다. 근본 원인 경로는 굵은 선(`==>`),
//! 프로세스 마이닝을 켜면 간선에 흐름 손실과 전파 Δt 를 적습니다 (`==> |🚨 병목 1,120 t/h (-39.5%) · Δt +4.3s|`).
//! 원본을 코어에서 만들어 시험하고, 화면은 mermaid 로 그린 뒤 노드 클릭을 선택 설비와 맞춥니다.

use std::collections::BTreeSet;

use crate::lattice::Lattice;
use crate::mining::EdgeFlow;
use crate::plant::fmt_value;

pub struct MermaidOptions<'a> {
    /// 그릴 설비 (None 이면 전체)
    pub scope: Option<&'a BTreeSet<String>>,
    pub selected: Option<&'a str>,
    pub alarm: Option<&'a str>,
    pub root: Option<&'a str>,
    /// 근본 원인 경로 (상류 → 경보)
    pub path: &'a [String],
    pub mining: bool,
}

/// mermaid 노드 이름
pub fn node_id(kks: &str) -> String {
    format!("N_{kks}")
}

fn quote(s: &str) -> String {
    s.replace('"', "'")
}

pub fn source(db: &Lattice, flows: &[EdgeFlow], o: &MermaidOptions<'_>) -> String {
    let shown = |kks: &str| o.scope.is_none_or(|s| s.contains(kks));
    let mut out = String::from("flowchart LR\n");
    for sector in db.sectors() {
        let members: Vec<_> = db.equipment_all().iter().filter(|e| e.sector == sector.key && shown(&e.kks)).collect();
        if members.is_empty() {
            continue;
        }
        out.push_str(&format!("  subgraph S_{}[\"{} {} ({})\"]\n", sector.key, sector.building, sector.label, sector.kks));
        for e in members {
            let floor = db.floor(&e.floor).map(|f| f.code.clone()).unwrap_or_default();
            out.push_str(&format!("    {}[\"{}<br/>{} · {}\"]\n", node_id(&e.kks), e.kks, quote(&e.name), floor));
        }
        out.push_str("  end\n");
    }
    let mut link = 0usize;
    let mut path_links = Vec::new();
    for f in flows.iter().filter(|f| shown(&f.from) && shown(&f.to)) {
        let on_path = o.path.windows(2).any(|w| w[0] == f.from && w[1] == f.to);
        let mut label = f.label.clone();
        if o.mining {
            if f.bottleneck {
                if let (Some(c), Some(u)) = (f.current, f.unit.as_deref()) {
                    label = format!("🚨 병목 {} {u} ({:+.1}%)", fmt_value(c), (f.ratio - 1.0) * 100.0);
                }
            }
            if let Some(lag) = f.lag_secs {
                label.push_str(&format!(" · Δt +{lag:.1}s"));
            }
        }
        let arrow = if on_path { "==>" } else if f.medium == "power" { "-.->" } else { "-->" };
        out.push_str(&format!("  {} {arrow}|\"{}\"| {}\n", node_id(&f.from), quote(&label), node_id(&f.to)));
        if on_path {
            path_links.push(link);
        }
        link += 1;
    }
    out.push_str("  classDef base fill:#1e2230,stroke:#56607a,color:#dfe5f2\n");
    out.push_str("  classDef pathNode fill:#3a2a12,stroke:#f0a020,color:#ffe8c2,stroke-width:2px\n");
    out.push_str("  classDef alarm fill:#4a3a10,stroke:#f5c518,color:#fff3c4,stroke-width:3px\n");
    out.push_str("  classDef root fill:#5a1a1a,stroke:#ff4d4f,color:#ffd6d6,stroke-width:4px\n");
    out.push_str("  classDef selected stroke:#3aa0ff,stroke-width:5px\n");
    let all: Vec<String> = db.equipment_all().iter().filter(|e| shown(&e.kks)).map(|e| node_id(&e.kks)).collect();
    if !all.is_empty() {
        out.push_str(&format!("  class {} base\n", all.join(",")));
    }
    let path_nodes: Vec<String> = o.path.iter().filter(|k| shown(k)).map(|k| node_id(k)).collect();
    if !path_nodes.is_empty() {
        out.push_str(&format!("  class {} pathNode\n", path_nodes.join(",")));
    }
    if let Some(a) = o.alarm.filter(|a| shown(a)) {
        out.push_str(&format!("  class {} alarm\n", node_id(a)));
    }
    if let Some(r) = o.root.filter(|r| shown(r)) {
        out.push_str(&format!("  class {} root\n", node_id(r)));
    }
    if let Some(s) = o.selected.filter(|s| shown(s)) {
        out.push_str(&format!("  class {} selected\n", node_id(s)));
    }
    for i in path_links {
        out.push_str(&format!("  linkStyle {i} stroke:#ff4d4f,stroke-width:4px\n"));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mining::edge_flows;

    #[test]
    fn the_source_groups_sectors_and_marks_the_path() {
        let db = Lattice::embedded().unwrap();
        let flows = edge_flows(&db, None, 0, None);
        let path = vec!["10LAB20AA102".to_string(), "10HAD10BB001".to_string(), "10HAH10BB001".to_string()];
        let scope: BTreeSet<String> = ["10LAB10AP001", "10LAB20AA102", "10HAD10BB001", "10HAH10BB001", "10BBA10CB001"].iter().map(|s| s.to_string()).collect();
        let src = source(&db, &flows, &MermaidOptions { scope: Some(&scope), selected: Some("10LAB20AA102"), alarm: Some("10HAH10BB001"), root: Some("10LAB20AA102"), path: &path, mining: true });
        assert!(src.starts_with("flowchart LR"));
        assert!(src.contains("subgraph S_feedwater[\"103동 급수·보조펌프동 (10ULB)\"]"));
        assert!(src.contains("N_10LAB20AA102 ==>|\"고압급수\"| N_10HAD10BB001"));
        assert!(src.contains("N_10BBA10CB001 -.->|\"6.9kV 전원\"| N_10LAB10AP001"), "전력선은 점선");
        assert!(src.contains("class N_10LAB20AA102 root"));
        assert!(!src.contains("N_10HTB10BB001"), "범위 밖 설비는 그리지 않습니다");
        assert_eq!(src.matches("linkStyle").count(), 2);
    }
}
