//! 계통 마인드맵 (기획 Part 8 4 · 5절). 설비를 노드로, 배관 · 전력선 · 축 · 통풍을 방향 있는 간선으로 둡니다.
//!
//! - 홉 뎁스: 선택한 설비가 `Depth 0`, 들어오는 간선을 거슬러 올라가면 `-1, -2 …`(상류), 나가는 간선을 따라가면 `+1, +2 …`(하류)
//! - 상류 경로: 알람 설비에서 `MAX_HOPS` 안의 상류 설비마다 가장 짧은 경로와, 간선 지연 범위를 더한 누적 지연 범위
//! - 도면 점프: 경로가 지나는 도면 간 연결점(`off_page_connectors`)을 함께 돌려줍니다. 서로 다른 도면의 사고를 하나로 묶습니다.
//!
//! 계통에는 순환이 있습니다 (복수 → 급수 → 증기 → 복수). 탐색은 방문 표시로 끊고 홉 한도 안에서만 돕니다.

use std::collections::{BTreeMap, HashMap, VecDeque};

use serde::Serialize;

use crate::lattice::Lattice;

/// 역추적 홉 한도 (명세: 2 ~ 3 홉)
pub const MAX_HOPS: usize = 3;

/// 마인드맵 한 칸: 설비와 부호 있는 홉 거리
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Hop {
    pub kks: String,
    /// 음수 상류, 양수 하류, 0 은 중심
    pub depth: i32,
}

/// 중심 설비에서 방향을 지켜 `max_hops` 안에 닿는 설비. 상류와 하류 양쪽에서 닿으면 가까운 쪽을, 같으면 상류를 씁니다.
pub fn hop_depths(db: &Lattice, center: &str, max_hops: usize) -> Vec<Hop> {
    let up = bfs(db, center, max_hops, true);
    let down = bfs(db, center, max_hops, false);
    let mut best: BTreeMap<String, i32> = BTreeMap::new();
    for (kks, d) in up {
        best.insert(kks, -(d as i32));
    }
    for (kks, d) in down {
        match best.get(&kks) {
            Some(cur) if cur.unsigned_abs() as usize <= d => {}
            _ => {
                best.insert(kks, d as i32);
            }
        }
    }
    best.insert(center.to_string(), 0);
    let mut hops: Vec<Hop> = best.into_iter().map(|(kks, depth)| Hop { kks, depth }).collect();
    hops.sort_by(|a, b| a.depth.cmp(&b.depth).then(a.kks.cmp(&b.kks)));
    hops
}

/// 대상 설비에서 본 설비의 깊이. 방향을 지킨 홉 깊이가 있으면 그것이고, 대상과 같은 상류 · 같은 하류에 나란히 붙은
/// 우회 설비(바이패스 밸브)는 0 입니다. 인터록 단계의 `req_depth` 를 원장 로딩 때 이것으로 검사합니다.
pub fn depth_of(db: &Lattice, target: &str, kks: &str) -> Option<i32> {
    if kks == target {
        return Some(0);
    }
    if let Some(h) = hop_depths(db, target, MAX_HOPS).into_iter().find(|h| h.kks == kks) {
        return Some(h.depth);
    }
    let ins = |k: &str| db.in_edges(k).into_iter().map(|e| e.from.clone()).collect::<Vec<_>>();
    let outs = |k: &str| db.out_edges(k).into_iter().map(|e| e.to.clone()).collect::<Vec<_>>();
    let shares = |a: Vec<String>, b: Vec<String>| a.iter().any(|x| b.contains(x));
    (shares(ins(target), ins(kks)) && shares(outs(target), outs(kks))).then_some(0)
}

fn bfs(db: &Lattice, start: &str, max_hops: usize, upstream: bool) -> HashMap<String, usize> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut queue = VecDeque::from([(start.to_string(), 0usize)]);
    seen.insert(start.to_string(), 0);
    while let Some((cur, d)) = queue.pop_front() {
        if d >= max_hops {
            continue;
        }
        let next: Vec<String> = if upstream {
            db.in_edges(&cur).into_iter().map(|e| e.from.clone()).collect()
        } else {
            db.out_edges(&cur).into_iter().map(|e| e.to.clone()).collect()
        };
        for n in next {
            if !seen.contains_key(&n) {
                seen.insert(n.clone(), d + 1);
                queue.push_back((n, d + 1));
            }
        }
    }
    seen.remove(start);
    seen
}

/// 방향을 보지 않은 홉 거리 (옵시디언 뷰에서 무관한 노드를 흐리게 할 때 씁니다)
pub fn undirected_distance(db: &Lattice, center: &str) -> HashMap<String, usize> {
    let mut seen: HashMap<String, usize> = HashMap::from([(center.to_string(), 0)]);
    let mut queue = VecDeque::from([center.to_string()]);
    while let Some(cur) = queue.pop_front() {
        let d = seen[&cur];
        let neighbors = db.in_edges(&cur).into_iter().map(|e| e.from.clone()).chain(db.out_edges(&cur).into_iter().map(|e| e.to.clone()));
        for n in neighbors.collect::<Vec<_>>() {
            if !seen.contains_key(&n) {
                seen.insert(n.clone(), d + 1);
                queue.push_back(n);
            }
        }
    }
    seen
}

/// 상류 설비에서 목표 설비까지의 경로
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UpPath {
    /// 상류 설비부터 목표 설비까지 (양 끝 포함)
    pub nodes: Vec<String>,
    /// 간선 지연 범위를 더한 누적 범위 (초)
    pub lag: (f64, f64),
    /// 경로가 지나는 도면 간 연결점
    pub connectors: Vec<String>,
}

impl UpPath {
    pub fn hops(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    pub fn source(&self) -> &str {
        &self.nodes[0]
    }
}

/// `target` 의 상류 설비마다 가장 짧은 경로 (홉이 같으면 먼저 찾은 것). 가까운 설비부터 돌려줍니다.
pub fn upstream_paths(db: &Lattice, target: &str, max_hops: usize) -> Vec<UpPath> {
    // 거슬러 올라가며 각 설비의 "다음 칸(하류 쪽)" 을 기억합니다.
    let mut toward: HashMap<String, String> = HashMap::new();
    let mut depth: HashMap<String, usize> = HashMap::from([(target.to_string(), 0)]);
    let mut order: Vec<String> = Vec::new();
    let mut queue = VecDeque::from([target.to_string()]);
    while let Some(cur) = queue.pop_front() {
        let d = depth[&cur];
        if d >= max_hops {
            continue;
        }
        for e in db.in_edges(&cur) {
            if depth.contains_key(&e.from) {
                continue;
            }
            depth.insert(e.from.clone(), d + 1);
            toward.insert(e.from.clone(), cur.clone());
            order.push(e.from.clone());
            queue.push_back(e.from.clone());
        }
    }
    order
        .into_iter()
        .map(|source| {
            let mut nodes = vec![source.clone()];
            let (mut lo, mut hi) = (0.0, 0.0);
            let mut connectors = Vec::new();
            let mut at = source;
            while let Some(next) = toward.get(&at) {
                if let Some(e) = db.edge_between(&at, next) {
                    lo += e.lag[0];
                    hi += e.lag[1];
                }
                if let Some(c) = db.connector_between(&at, next) {
                    connectors.push(c.id.clone());
                }
                nodes.push(next.clone());
                at = next.clone();
            }
            UpPath { nodes, lag: (lo, hi), connectors }
        })
        .collect()
}

/// `from` 에서 `to` 로 흐름 방향을 따라 가는 가장 짧은 경로
pub fn path_between(db: &Lattice, from: &str, to: &str, max_hops: usize) -> Option<UpPath> {
    upstream_paths(db, to, max_hops).into_iter().find(|p| p.source() == from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> Lattice {
        Lattice::embedded().unwrap()
    }

    fn depth(hops: &[Hop], kks: &str) -> Option<i32> {
        hops.iter().find(|h| h.kks == kks).map(|h| h.depth)
    }

    #[test]
    fn the_valve_sits_between_its_pump_and_the_boiler() {
        // 기획안 6.2절의 홉 뎁스 목록: [D-2] 6.9kV 차단기, [D-1] 주급수펌프, [D 0] 급수조절밸브, [D+1] 보일러 쪽
        let hops = hop_depths(&db(), "10LAB20AA102", 2);
        assert_eq!(depth(&hops, "10LAB20AA102"), Some(0));
        assert_eq!(depth(&hops, "10LAB10AP001"), Some(-1));
        assert_eq!(depth(&hops, "10BBA10CB001"), Some(-2));
        assert_eq!(depth(&hops, "10LAA10BB001"), Some(-2));
        assert_eq!(depth(&hops, "10HAD10BB001"), Some(1));
        assert_eq!(depth(&hops, "10HAH10BB001"), Some(2));
        assert_eq!(depth(&hops, "10HTB10BB001"), None, "105동 탈황 설비는 2홉 안에 없습니다");
        assert!(hops.windows(2).all(|w| w[0].depth <= w[1].depth), "상류부터 정렬");
    }

    #[test]
    fn the_superheater_alarm_reaches_the_valve_two_hops_up_across_a_drawing_jump() {
        let paths = upstream_paths(&db(), "10HAH10BB001", MAX_HOPS);
        let valve = paths.iter().find(|p| p.source() == "10LAB20AA102").expect("급수조절밸브가 상류에 있어야 합니다");
        assert_eq!(valve.hops(), 2);
        assert_eq!(valve.nodes, vec!["10LAB20AA102", "10HAD10BB001", "10HAH10BB001"]);
        assert_eq!(valve.connectors, vec!["OPC-103-101".to_string()]);
        assert_eq!(valve.lag, (2.0, 12.0), "기획안의 고압급수 배관 Δt 범위 2.0 ~ 12.0 초");
        let pump = paths.iter().find(|p| p.source() == "10LAB10AP001").unwrap();
        assert_eq!(pump.hops(), 3);
        assert!(paths.iter().all(|p| p.hops() <= MAX_HOPS));
    }

    #[test]
    fn cycles_do_not_trap_the_search() {
        let db = db();
        // 복수기 → … → 터빈 → 복수기 순환
        let hops = hop_depths(&db, "10MAG10AC001", 10);
        assert!(hops.len() > 10);
        let all = undirected_distance(&db, "10LAB20AA102");
        assert_eq!(all.len(), db.node_count(), "계통은 하나로 이어져 있습니다");
        assert_eq!(all["10LAB20AA102"], 0);
    }

    #[test]
    fn paths_follow_the_flow_direction() {
        let db = db();
        assert!(path_between(&db, "10HLB10AN001", "10HAD10BB001", 1).is_some(), "흡인 통풍");
        assert!(path_between(&db, "10HAD10BB001", "10LAB20AA102", 3).is_none(), "흐름을 거슬러 가지 않습니다");
    }
}
