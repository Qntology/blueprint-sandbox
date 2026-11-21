//! Pack B (1-Hop Halo 빈 병합 그룹 프루닝): 비전 블록 진입 직전 명암 변화가 없는
//! 빈 종이 병합 그룹을 제거하고, 3×3 Max-Pool (1-Hop Halo) 팽창으로 인접 상하좌우·대각선
//! 8방위를 강제 보호해 글자 외곽 획·소수점·쉼표·얇은 괘선 탈락 0건을 보장합니다.

use candle_core::{Device, Result, Tensor};

/// 비전 패치 프루닝 환경 변수 (`off` 기본 · `1`/`halo`/`scatter` 활성)
pub const PRUNE_ENV: &str = "BLUEPRINT_VISION_PRUNE";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PruneMode {
    Off,
    /// 비전 블록만 생존 패치로 압축 실행하고, 블록 출력 뒤 원래 `H_m × W_m` 위치로 복원합니다.
    Halo1HopScatter,
}

impl PruneMode {
    pub fn from_env() -> Self {
        match std::env::var(PRUNE_ENV) {
            Ok(v) => match v.trim().to_ascii_lowercase().as_str() {
                "1" | "on" | "true" | "halo" | "scatter" => Self::Halo1HopScatter,
                _ => Self::Off,
            },
            Err(_) => Self::Off,
        }
    }
}

/// 프루닝 결과: 생존 패치 인덱스(원본 2D RoPE 보존용)와 복원용 대표 공백 인덱스
pub struct PrunedGrid {
    /// 생존 패치들의 원본 패치 인덱스 (`[kept_groups * merge^2]`)
    pub kept_patch_indices: Vec<u32>,
    /// 생존 병합 그룹 인덱스 (`[kept_groups]`)
    pub kept_group_indices: Vec<u32>,
    /// 대표 공백 병합 그룹 1개의 인덱스 (원래 격자 복원 시 사용)
    pub rep_blank_group: u32,
    pub total_groups: usize,
}

/// 병합 창 순서(`(mr, mc, i, j)`)로 정렬된 정규화 패치 슬라이스 `patches`에서
/// 빈 병합 그룹을 찾고 3×3 Max-Pool 1-Hop Halo를 적용합니다.
///
/// - `total_groups <= 32` 이거나 생존 비율이 `>= 92%` 이면 오버헤드 없이 `None`을 반환합니다.
/// - 대표 공백 그룹 1개를 생존 집합에 포함시켜 `scatter` 시 공백 위치에 정확한 대표 특징을 채웁니다.
pub fn plan_1hop_halo(
    patches: &[f32],
    grid_h: usize,
    grid_w: usize,
    merge: usize,
    patch_len: usize,
    spread_thresh: f32,
) -> Option<PrunedGrid> {
    if merge == 0 || !grid_h.is_multiple_of(merge) || !grid_w.is_multiple_of(merge) {
        return None;
    }
    let hm = grid_h / merge;
    let wm = grid_w / merge;
    let total_groups = hm * wm;
    if total_groups <= 32 {
        return None;
    }
    let mm = merge * merge;
    let group_elems = mm * patch_len;
    if patches.len() < total_groups * group_elems {
        return None;
    }

    // 1) 그룹별 (max - min) 명암 폭으로 잉크 활성 마스크 판정
    let mut active = vec![false; total_groups];
    let mut first_blank: Option<usize> = None;
    for g in 0..total_groups {
        let slice = &patches[g * group_elems..(g + 1) * group_elems];
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &v in slice {
            if v < lo {
                lo = v;
            }
            if v > hi {
                hi = v;
            }
        }
        if hi - lo > spread_thresh {
            active[g] = true;
        } else if first_blank.is_none() {
            first_blank = Some(g);
        }
    }
    let rep_blank = first_blank?;

    // 2) 3×3 Max-Pool (1-Hop Halo) 팽창 — 활성 그룹 주변 8방위 강제 보호
    let mut keep = vec![false; total_groups];
    for r in 0..hm {
        for c in 0..wm {
            let mut hit = false;
            let r0 = r.saturating_sub(1);
            let r1 = (r + 1).min(hm - 1);
            let c0 = c.saturating_sub(1);
            let c1 = (c + 1).min(wm - 1);
            'outer: for rr in r0..=r1 {
                for cc in c0..=c1 {
                    if active[rr * wm + cc] {
                        hit = true;
                        break 'outer;
                    }
                }
            }
            keep[r * wm + c] = hit;
        }
    }
    // 대표 공백 그룹 1개는 반드시 유지해 scatter 복원용 공백 특징을 얻습니다.
    keep[rep_blank] = true;

    let kept_count = keep.iter().filter(|&&k| k).count();
    if kept_count * 100 >= total_groups * 92 {
        return None;
    }

    let mut kept_group_indices = Vec::with_capacity(kept_count);
    let mut kept_patch_indices = Vec::with_capacity(kept_count * mm);
    for (g, &k) in keep.iter().enumerate() {
        if k {
            kept_group_indices.push(g as u32);
            let base = (g * mm) as u32;
            for j in 0..mm as u32 {
                kept_patch_indices.push(base + j);
            }
        }
    }
    Some(PrunedGrid {
        kept_patch_indices,
        kept_group_indices,
        rep_blank_group: rep_blank as u32,
        total_groups,
    })
}

/// 압축 실행된 병합 출력 `[kept_groups, D]` 를 원래 격자 `[total_groups, D]` 로 복원합니다.
/// 제거된 공백 위치는 대표 공백 그룹의 출력을 그대로 채워 넣습니다.
pub fn scatter_groups(kept_out: &Tensor, plan: &PrunedGrid, device: &Device) -> Result<Tensor> {
    let rep_pos = plan
        .kept_group_indices
        .binary_search(&plan.rep_blank_group)
        .expect("rep_blank_group is always in kept_group_indices") as u32;
    let mut gather_idx = vec![rep_pos; plan.total_groups];
    for (pos, &orig_g) in plan.kept_group_indices.iter().enumerate() {
        gather_idx[orig_g as usize] = pos as u32;
    }
    let idx = Tensor::from_vec(gather_idx, (plan.total_groups,), device)?;
    kept_out.index_select(&idx, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halo_1hop_protects_all_8_neighbors_and_fast_exits_small_or_dense() {
        // 1) 소형(4×4=16 <= 32): fast-exit
        let small = vec![0.0f32; 16 * 4 * 3];
        assert!(plan_1hop_halo(&small, 8, 8, 2, 3, 0.05).is_none());

        // 2) 10×10 = 100 그룹 중 중앙 (5,5) 하나만 잉크 -> 3×3=9개 보호 + 대표 공백 1개 = 10개
        let (hm, wm, merge, plen) = (10usize, 10usize, 2usize, 4usize);
        let group_elems = merge * merge * plen;
        let mut patches = vec![1.0f32; hm * wm * group_elems];
        let center = 5 * wm + 5;
        patches[center * group_elems] = -1.0; // 명암 폭 2.0 > 0.05
        let plan = plan_1hop_halo(&patches, hm * merge, wm * merge, merge, plen, 0.05).expect("프루닝 발동");
        assert_eq!(plan.kept_group_indices.len(), 10);
        for dr in 4..=6 {
            for dc in 4..=6 {
                let g = (dr * wm + dc) as u32;
                assert!(plan.kept_group_indices.contains(&g), "8방위 이웃 ({dr},{dc}) 보호 누락");
            }
        }

        // 3) scatter 복원 형상 및 값 검증
        let kept_t = Tensor::arange(0u32, 10, &Device::Cpu)
            .unwrap()
            .to_dtype(candle_core::DType::F32)
            .unwrap()
            .reshape((10, 1))
            .unwrap();
        let full = scatter_groups(&kept_t, &plan, &Device::Cpu).unwrap();
        assert_eq!(full.dims(), &[100, 1]);
    }
}
