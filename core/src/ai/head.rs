//! 출력 투영으로 다음 토큰 고르기 (gocr · zoro 가 함께 씁니다).
//!
//! 🌟 [VOCAB TRIM] 출력 투영을 잘라 냈으면(`ai::trim`) 남긴 K 행만 곱하고, 그 안에서 고릅니다. 잘린 행은 어디에도 없습니다
//! (예전 `output_rest.weight` · `head_rest.weight` 와 그 검사는 없앴습니다). 확률은 남긴 K 행의 소프트맥스이고,
//! 고른 행 `i` 는 [`HeadMap::kept`]`[i]` 로 전체 어휘 번호가 됩니다. 남긴 행은 바이트 폴백 · 지원 언어 문자 체계를 모두 담아
//! 어떤 글이든 쓸 수 있게 고릅니다 (`ai::trim`).

use std::sync::Arc;

use anyhow::{bail, Result};
use candle_core::{Device, Tensor, D};

use super::residency::{Loaded, HEAD_VIEW};
use super::weights::WeightFile;

/// 잘라 낸 출력 투영의 텐서 이름과 행마다 전체 어휘 번호
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMap {
    /// 남긴 행의 텐서: 잘라 둔 파일의 `output.weight`(gocr), 또는 행 보기 [`HEAD_VIEW`]
    pub kept_name: &'static str,
    /// 행마다 전체 어휘 번호 (오름차순)
    pub kept: Vec<u32>,
}

impl HeadMap {
    /// 파일의 `<구조>.head.ids` 가 있으면 잘라 둔 출력 투영(`output.weight` 의 행 = 그 번호)입니다. 없으면 자르지 않은 파일입니다.
    pub fn from_file(file: &WeightFile, arch: &str, vocab: usize) -> Result<Option<Self>> {
        let key = format!("{arch}.head.ids");
        if !file.has_meta(&key) {
            return Ok(None);
        }
        let kept = file.meta_u32_array(&key)?;
        check_ids(&kept, vocab, &key)?;
        let rows = file.info("output.weight")?.dims[0];
        if rows != kept.len() {
            bail!("output.weight 의 행 {rows} 이 {key} 의 번호 {} 개와 다릅니다", kept.len());
        }
        Ok(Some(Self { kept_name: "output.weight", kept }))
    }

    /// `base` 텐서에서 `ids`(전체 어휘 번호, 오름차순)의 행만 행 보기 [`HEAD_VIEW`] 로 둡니다. 새 파일은 만들지 않습니다.
    /// `base_ids` 는 `base` 의 행마다 어휘 번호입니다 (`None` 이면 행 번호 = 어휘 번호: 묶인 임베딩 `token_embd.weight`, 자르지 않은 `output.weight`).
    pub fn view(file: &WeightFile, base: &str, base_ids: Option<&[u32]>, ids: Vec<u32>, vocab: usize) -> Result<Self> {
        check_ids(&ids, vocab, HEAD_VIEW)?;
        let rows: Vec<u32> = match base_ids {
            None => ids.clone(),
            Some(b) => ids
                .iter()
                .map(|t| b.binary_search(t).map(|r| r as u32).map_err(|_| anyhow::anyhow!("토큰 {t} 이 {base} 의 행에 없습니다")))
                .collect::<Result<_>>()?,
        };
        file.set_row_view(HEAD_VIEW, base, rows)?;
        Ok(Self { kept_name: HEAD_VIEW, kept: ids })
    }

    /// 이 토큰이 남긴 행에 있는가
    pub fn keeps(&self, token: u32) -> bool {
        self.kept.binary_search(&token).is_ok()
    }

    /// 남긴 행의 자리 (없으면 `None`)
    pub fn row_of(&self, token: u32) -> Option<usize> {
        self.kept.binary_search(&token).ok()
    }
}

fn check_ids(ids: &[u32], vocab: usize, what: &str) -> Result<()> {
    if ids.is_empty() || ids.windows(2).any(|w| w[0] >= w[1]) || ids.last().is_some_and(|&l| l as usize >= vocab) {
        bail!("{what} 는 어휘 {vocab} 안의 비지 않은 오름차순 번호여야 합니다");
    }
    Ok(())
}

/// 행 보기의 원본 행 수를 확인할 때 (시험)
pub fn view_rows(file: &WeightFile) -> Option<Arc<Vec<u32>>> {
    file.row_view_rows(HEAD_VIEW)
}

/// `[n, R]` 로짓의 행마다 (최댓값, 그 자리, logsumexp). 장치에서 셋만 계산해 가져오고, 장치 연산이 안 되면 CPU 에서 합니다.
fn summarize(r: &Tensor) -> Result<Vec<(f32, u32, f32)>> {
    let on_device = || -> Result<Vec<(f32, u32, f32)>> {
        let max = r.max_keepdim(D::Minus1)?;
        let arg = r.argmax_keepdim(D::Minus1)?;
        let lse = (r.broadcast_sub(&max)?.exp()?.sum_keepdim(D::Minus1)?.log()? + &max)?;
        let max = max.flatten_all()?.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
        let arg = arg.flatten_all()?.to_device(&Device::Cpu)?.to_vec1::<u32>()?;
        let lse = lse.flatten_all()?.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
        Ok(max.into_iter().zip(arg).zip(lse).map(|((m, a), l)| (m, a, l)).collect())
    };
    match on_device() {
        Ok(v) => Ok(v),
        Err(_) => Ok(r.to_device(&Device::Cpu)?.to_vec2::<f32>()?.iter().map(|row| stat_of(row)).collect()),
    }
}

fn stat_of(row: &[f32]) -> (f32, u32, f32) {
    let (mut arg, mut max) = (0usize, f32::MIN);
    for (i, &v) in row.iter().enumerate() {
        if v > max {
            (arg, max) = (i, v);
        }
    }
    let z: f32 = row.iter().map(|v| (v - max).exp()).sum();
    (max, arg as u32, max + z.ln())
}

/// 정규화한 은닉 상태 `h` `[n, hidden]` 의 로짓 (CPU 행). `map` 이 있으면 남긴 행 텐서로, 없으면 `full`
/// (자르지 않은 출력 투영: gocr `output.weight`, zoro 묶인 `token_embd.weight`) 로 곱합니다.
pub fn logits(output: &Loaded, full: &str, map: Option<&HeadMap>, h: &Tensor) -> Result<Vec<Vec<f32>>> {
    let l = output.linear(map.map_or(full, |m| m.kept_name), h)?;
    Ok(l.to_device(&Device::Cpu)?.to_vec2::<f32>()?)
}

/// Pack A/C 무손실 Mean-Centered Cauchy-Schwarz (CS) 구출 가드.
///
/// 잘린 나머지 어휘(`rest_ids`)의 평균 벡터 `mu_rest` 와 중심화 최대 노름 상한 `w_rest_bound` 를 보관해,
/// `max_core >= h·mu_rest + ||h||_2 * w_rest_bound` 인 스텝(>95%)은 0-비용으로 무손실을 증명하고,
/// 상한 돌파 의심 스텝만 호스트 SIMD 내적으로 나머지 행을 검사해 전체 어휘와 100% 동일한 그리디 토큰을 보장합니다.
#[derive(Debug, Clone)]
pub struct RestRescueGuard {
    pub cs: candle_core::lm_head::CsGuardParams,
    /// 잘린 나머지 어휘의 전체 토큰 번호 (오름차순)
    pub rest_ids: Vec<u32>,
    /// 호스트에 보관한 나머지 가중치 행 `[rest_ids.len(), K]` (F32, 구출 시 SIMD 내적용)
    pub rest_rows_f32: Vec<f32>,
    pub hidden: usize,
}

impl RestRescueGuard {
    /// 밀집 F32 REST 가중치 `[V_rest, K]` 와 토큰 번호 목록으로 CS 가드를 구성합니다.
    pub fn from_dense(rest_w: &Tensor, rest_ids: Vec<u32>, safety_margin: f32) -> Result<Self> {
        let (v_rest, k) = rest_w.dims2()?;
        if v_rest != rest_ids.len() {
            bail!("RestRescueGuard: 가중치 행 {v_rest} != 토큰 번호 {} 개", rest_ids.len());
        }
        let cs = candle_core::lm_head::CsGuardParams::from_rest_weight(rest_w, None, safety_margin)?;
        let rest_rows_f32 = rest_w.to_device(&Device::Cpu)?.to_dtype(candle_core::DType::F32)?.flatten_all()?.to_vec1::<f32>()?;
        Ok(Self { cs, rest_ids, rest_rows_f32, hidden: k })
    }

    /// 주어진 은닉 행 `h_row` (`[K]`) 에 대해 REST 어휘의 최댓값 (전체 토큰 ID, 로짓) 을 SIMD 내적으로 계산합니다.
    pub fn best_rest(&self, h_row: &[f32]) -> Option<(u32, f32)> {
        let (local, val) = blueprint_cpu_kernels::dot_argmax_rows_f32(h_row, &self.rest_rows_f32, self.rest_ids.len(), self.hidden)?;
        Some((self.rest_ids[local], val))
    }
}

/// Pack A/C 장치 내 그리디 1위 토큰 + 소프트맥스 확률 (`[n, V]` 전체 D2H 전송 제거).
///
/// GPU 에서 `candle_core::lm_head::logits_argmax_cs` 로 argmax · `kept` 재매핑 · CS 상한 검사를 수행하고,
/// 행당 스칼라 2개(`max`, `lse`)만 호스트로 가져옵니다. `guard` 가 있고 `rescue_mask == 1` 인 행만
/// 호스트 SIMD 내적으로 REST 어휘를 검사해 100% 무손실 그리디 일치를 보장합니다.
pub fn top1_on_device(
    output: &Loaded,
    full: &str,
    map: Option<&HeadMap>,
    guard: Option<&RestRescueGuard>,
    h: &Tensor,
) -> Result<Vec<(u32, f32)>> {
    let dev = h.device();
    let l = output.linear(map.map_or(full, |m| m.kept_name), h)?;
    let n = l.dim(0)?;
    if !dev.is_cpu() && super::verify::lm_head_argmax_trusted(dev) {
        let cids_t = match map {
            Some(m) => Some(Tensor::from_slice(&m.kept, (m.kept.len(),), dev)?),
            None => None,
        };
        let cs_dev = match guard {
            Some(g) => Some(candle_core::lm_head::CsGuardParams {
                mu_rest: g.cs.mu_rest.to_device(dev)?,
                w_rest_bound: g.cs.w_rest_bound,
                b_rest_max: g.cs.b_rest_max,
            }),
            None => None,
        };
        let res = candle_core::lm_head::logits_argmax_cs(&l, Some(h), cids_t.as_ref(), cs_dev.as_ref())?;
        let mut ids = res.ids.to_device(&Device::Cpu)?.to_vec1::<u32>()?;
        let max_t = res.max_vals.as_ref().ok_or_else(|| anyhow::anyhow!("logits_argmax_cs max_vals 없음"))?;
        let max_vals = max_t.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
        let max_2d = max_t.reshape((n, 1))?;
        let lse = (l.broadcast_sub(&max_2d)?.exp()?.sum_keepdim(D::Minus1)?.log()? + &max_2d)?
            .flatten_all()?
            .to_device(&Device::Cpu)?
            .to_vec1::<f32>()?;
        let rescue_flags = match res.rescue_mask {
            Some(ref m) => m.to_device(&Device::Cpu)?.to_vec1::<u8>()?,
            None => vec![0u8; n],
        };
        let mut probs: Vec<f32> = max_vals.iter().zip(&lse).map(|(&mx, &ls)| (mx - ls).exp()).collect();
        if let Some(g) = guard {
            if rescue_flags.iter().any(|&f| f != 0) {
                let h_cpu = h.to_device(&Device::Cpu)?.to_dtype(candle_core::DType::F32)?.to_vec2::<f32>()?;
                for i in 0..n {
                    if rescue_flags[i] != 0 {
                        if let Some((rid, rval)) = g.best_rest(&h_cpu[i]) {
                            if rval > max_vals[i] || (rval == max_vals[i] && rid < ids[i]) {
                                ids[i] = rid;
                                probs[i] = ((rval - lse[i]).exp()).min(1.0);
                            }
                        }
                    }
                }
            }
        }
        return Ok(ids.into_iter().zip(probs).collect());
    }
    // CPU 또는 커널 폴백 경로
    let rows = l.to_device(&Device::Cpu)?.to_vec2::<f32>()?;
    let mut out: Vec<(u32, f32)> = rows.iter().map(|row| top1(row, map)).collect();
    if let Some(g) = guard {
        let h_cpu = h.to_device(&Device::Cpu)?.to_dtype(candle_core::DType::F32)?.to_vec2::<f32>()?;
        for i in 0..n {
            let (mx, _, lse) = stat_of(&rows[i]);
            let mu = g.cs.mu_rest.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
            let xm = blueprint_cpu_kernels::dot_f32_simd(&h_cpu[i], &mu);
            let xn = blueprint_cpu_kernels::dot_f32_simd(&h_cpu[i], &h_cpu[i]).sqrt();
            let upper = xn.mul_add(g.cs.w_rest_bound, xm) + g.cs.b_rest_max;
            if mx < upper {
                if let Some((rid, rval)) = g.best_rest(&h_cpu[i]) {
                    if rval > mx || (rval == mx && rid < out[i].0) {
                        out[i] = (rid, ((rval - lse).exp()).min(1.0));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// 행 하나에서 확률이 큰 토큰 셋 (`attention::top3` 와 같은 약속, 번호는 전체 어휘). 모자라면 `(u32::MAX, 0.0)`.
pub fn top3(row: &[f32], map: Option<&HeadMap>) -> [(u32, f32); 3] {
    let best = super::attention::top3(row);
    match map {
        None => best,
        Some(m) => best.map(|(i, p)| if i == u32::MAX { (i, p) } else { (m.kept[i as usize], p) }),
    }
}

/// 행 하나의 1위 토큰과 확률
pub fn top1(row: &[f32], map: Option<&HeadMap>) -> (u32, f32) {
    top3(row, map)[0]
}

/// teacher forcing 점수: 행마다 (정답 토큰의 음의 로그 확률, 1위 토큰). 진단 · 양자화 배분 측정용 (`examples/quant_screen.rs`).
/// 남긴 행의 소프트맥스이고, 정답이 잘린 행의 토큰이면 음의 로그 확률은 무한대입니다.
pub fn score(output: &Loaded, full: &str, map: Option<&HeadMap>, h: &Tensor, targets: &[u32]) -> Result<Vec<(f32, u32)>> {
    let n = h.dim(0)?;
    if targets.len() != n {
        bail!("정답 {} 개와 행 {n} 개가 다릅니다", targets.len());
    }
    let dev = h.device();
    let at: Vec<Option<u32>> = targets.iter().map(|&t| map.map_or(Some(t), |m| m.row_of(t).map(|r| r as u32))).collect();
    let k = output.linear(map.map_or(full, |m| m.kept_name), h)?;
    let stats = summarize(&k)?;
    let idx = Tensor::from_vec(at.iter().map(|a| a.unwrap_or(0)).collect::<Vec<u32>>(), (n, 1), dev)?;
    let picked = k.gather(&idx, 1)?.flatten_all()?.to_device(&Device::Cpu)?.to_vec1::<f32>()?;
    Ok((0..n)
        .map(|i| {
            let (_, arg, lse) = stats[i];
            let top = map.map_or(arg, |m| m.kept[arg as usize]);
            let nll = if at[i].is_some() { lse - picked[i] } else { f32::INFINITY };
            (nll, top)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::attention;
    use crate::ai::verify::det_normal;

    #[test]
    fn a_trimmed_head_picks_inside_the_kept_rows_and_maps_back_to_the_vocabulary() {
        // 어휘 8, 남긴 행 2 · 5 · 6 (나머지는 없음)
        let full: Vec<f32> = det_normal(8, 4).iter().map(|v| v * 3.0).collect();
        let map = HeadMap { kept_name: HEAD_VIEW, kept: vec![2, 5, 6] };
        let kept: Vec<f32> = map.kept.iter().map(|&i| full[i as usize]).collect();
        let got = top3(&kept, Some(&map));
        assert!(got.iter().all(|(t, _)| map.keeps(*t)), "고른 토큰은 남긴 행의 것: {got:?}");
        // 남긴 행끼리의 순서와 확률은 그 K 행만의 소프트맥스와 같습니다.
        let want = attention::top3(&kept);
        for (g, w) in got.iter().zip(want) {
            assert_eq!(g.0, map.kept[w.0 as usize]);
            assert!((g.1 - w.1).abs() < 1e-6);
        }
        assert_eq!(top1(&[2.0, 3.0], None).0, 1, "자르지 않은 표는 그대로");
        assert_eq!(map.row_of(5), Some(1));
        assert_eq!(map.row_of(3), None);
    }

    #[test]
    fn rest_rescue_guard_matches_full_vocab_argmax_losslessly() -> Result<()> {
        use crate::ai::residency::{Budget, Group, ModelId, ModelSpec, Phase, Residency};
        use crate::ai::weights::testing::{write_gguf, TempDir};
        use candle_core::quantized::gguf_file::Value;
        use candle_core::quantized::GgmlDType;

        let (vocab, k, m) = (64usize, 32usize, 8usize);
        let w_full = Tensor::from_vec(det_normal(vocab * k, 77), (vocab, k), &Device::Cpu)?;
        let h = Tensor::from_vec(det_normal(m * k, 78), (m, k), &Device::Cpu)?;
        let full_logits = h.matmul(&w_full.t()?)?.to_vec2::<f32>()?;
        let want: Vec<u32> = full_logits.iter().map(|r| attention::top1(r).0).collect();

        // 어휘 절반(짝수 번호)만 core 로 남기고, 홀수 번호는 REST 로 분리합니다.
        let kept: Vec<u32> = (0..vocab as u32).filter(|i| i % 2 == 0).collect();
        let rest_ids: Vec<u32> = (0..vocab as u32).filter(|i| i % 2 == 1).collect();
        let rest_idx = Tensor::from_slice(&rest_ids, (rest_ids.len(),), &Device::Cpu)?;
        let rest_w = w_full.index_select(&rest_idx, 0)?;
        let guard = RestRescueGuard::from_dense(&rest_w, rest_ids, 1e-3)?;

        let dir = TempDir::new("head-cs");
        let path = dir.0.join("cs.gguf");
        write_gguf(
            &path,
            &[("general.architecture", Value::String("zoro".into()))],
            &[("token_embd.weight".into(), w_full.clone(), GgmlDType::F32)],
        )?;
        let file = Arc::new(WeightFile::open(&path)?);
        let map = HeadMap::view(&file, "token_embd.weight", None, kept, vocab)?;
        let spec = ModelSpec::new(ModelId::Zoro, file);
        let mut res = Residency::new(Device::Cpu, Budget::unlimited());
        let loaded = res.enter(&spec, &Phase::new("head", [Group::Head]))?;
        let got = top1_on_device(&loaded[0], "token_embd.weight", Some(&map), Some(&guard), &h)?;
        let got_ids: Vec<u32> = got.iter().map(|(id, _)| *id).collect();
        assert_eq!(got_ids, want, "CS 가드 구출 결과는 전체 어휘 argmax 와 100% 일치해야 합니다");
        Ok(())
    }
}
