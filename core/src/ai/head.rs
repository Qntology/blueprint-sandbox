//! 출력 투영으로 다음 토큰 고르기 (gocr · zoro 가 함께 씁니다).
//!
//! 잘라 둔 출력 투영(`ai::pyramid`)이면 남긴 행(`output.weight`)으로 고르고, 잘린 행(`output_rest.weight`)의 최댓값이 이기면
//! 그 토큰을 씁니다 (잘린 행 검사). 잘린 행은 장치에서 최댓값 · 그 번호 · logsumexp 셋만 계산해 CPU 로 가져옵니다.
//! 확률은 전체 어휘의 소프트맥스입니다: 잘린 행의 logsumexp 를 분모에 더합니다.
//!
//! 블록 디퓨전 초안처럼 뒤에 검증이 따로 있는 순전파는 잘린 행을 건너뛸 수 있습니다 (`with_rest = false`, gravity-ocr 와 같음).
//! 그때의 확률은 남긴 행만의 소프트맥스입니다.

use anyhow::{bail, Result};
use candle_core::{Device, Tensor, D};

use super::residency::{Loaded, HEAD_KEPT, HEAD_REST, OUTPUT_REST};
use super::weights::WeightFile;

/// 잘라 둔 출력 투영의 행 번호와 텐서 이름
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadMap {
    /// 남긴 행의 텐서 (gocr `output.weight`, zoro `head.weight`)와 그 행마다 전체 어휘 번호
    pub kept_name: &'static str,
    pub kept: Vec<u32>,
    /// 잘린 행의 텐서 (gocr `output_rest.weight`, zoro `head_rest.weight`)와 그 행마다 전체 어휘 번호
    pub rest_name: &'static str,
    pub rest: Vec<u32>,
}

impl HeadMap {
    /// 파일의 `<구조>.head.ids` 와 잘린 행 텐서가 있으면 행 번호를 만듭니다. 둘 다 없으면 자르지 않은 파일입니다.
    /// 텐서 이름은 출력 투영을 따로 둔 모델(gocr)이면 `output.weight` · `output_rest.weight`, 묶인 임베딩 모델(zoro)이면 `head.weight` · `head_rest.weight` 입니다.
    pub fn from_file(file: &WeightFile, arch: &str, vocab: usize) -> Result<Option<Self>> {
        let key = format!("{arch}.head.ids");
        let (kept_name, rest_name) = if file.has(HEAD_KEPT) { (HEAD_KEPT, HEAD_REST) } else { ("output.weight", OUTPUT_REST) };
        if !file.has_meta(&key) && !file.has(rest_name) {
            return Ok(None);
        }
        let kept = file.meta_u32_array(&key)?;
        if kept.windows(2).any(|w| w[0] >= w[1]) || kept.last().is_some_and(|&l| l as usize >= vocab) {
            bail!("{key} 는 어휘 {vocab} 안의 오름차순 번호여야 합니다");
        }
        let mut it = kept.iter().peekable();
        let rest: Vec<u32> = (0..vocab as u32)
            .filter(|i| {
                if it.peek() == Some(&i) {
                    it.next();
                    false
                } else {
                    true
                }
            })
            .collect();
        let (k, r) = (file.info(kept_name)?.dims[0], file.info(rest_name)?.dims[0]);
        if k != kept.len() || r != rest.len() {
            bail!("남긴 행 {k} · 잘린 행 {r} 이 번호 {} · {} 와 다릅니다", kept.len(), rest.len());
        }
        Ok(Some(Self { kept_name, kept, rest_name, rest }))
    }

    /// 잘린 행의 토큰인가
    pub fn is_cut(&self, token: u32) -> bool {
        self.kept.binary_search(&token).is_err()
    }
}

/// 잘린 행의 요약 (한 행)
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RestStat {
    pub max: f32,
    /// 최댓값의 전체 어휘 번호
    pub token: u32,
    /// log Σ exp(잘린 행 로짓)
    pub lse: f32,
}

/// 은닉 상태 행마다의 로짓: 남긴 행(자르지 않은 파일이면 전체 어휘) 그대로와 잘린 행의 요약
#[derive(Debug, Clone, PartialEq)]
pub struct Logits {
    pub rows: Vec<Vec<f32>>,
    pub rest: Vec<Option<RestStat>>,
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

/// 정규화한 은닉 상태 `h` `[n, hidden]` 의 로짓. `map` 이 있으면 남긴 행 텐서로 곱하고 (`with_rest` 면) 잘린 행도 봅니다.
/// 없으면 `full` (자르지 않은 출력 투영: gocr `output.weight`, zoro 묶인 `token_embd.weight`) 로 전체 어휘를 곱합니다.
pub fn logits(output: &Loaded, full: &str, map: Option<&HeadMap>, h: &Tensor, with_rest: bool) -> Result<Logits> {
    let kept = output.linear(map.map_or(full, |m| m.kept_name), h)?;
    let rest = match map.filter(|_| with_rest) {
        Some(m) => summarize(&output.linear(m.rest_name, h)?)?.into_iter().map(|(max, i, lse)| Some(RestStat { max, token: m.rest[i as usize], lse })).collect(),
        None => vec![None; kept.dim(0)?],
    };
    Ok(Logits { rows: kept.to_device(&Device::Cpu)?.to_vec2::<f32>()?, rest })
}

/// 행 하나에서 확률이 큰 토큰 셋 (`attention::top3` 와 같은 약속, 잘린 행의 최댓값도 후보). 모자라면 `(u32::MAX, 0.0)`.
pub fn top3(row: &[f32], rest: Option<RestStat>, map: Option<&HeadMap>) -> [(u32, f32); 3] {
    let mut best = [(u32::MAX, f32::MIN); 3];
    let mut offer = |t: u32, l: f32| {
        if l > best[2].1 {
            best[2] = (t, l);
            if best[2].1 > best[1].1 {
                best.swap(1, 2);
                if best[1].1 > best[0].1 {
                    best.swap(0, 1);
                }
            }
        }
    };
    let id = |i: usize| map.map_or(i as u32, |m| m.kept[i]);
    for (i, &l) in row.iter().enumerate() {
        offer(id(i), l);
    }
    if let Some(r) = rest {
        offer(r.token, r.max);
    }
    let max = best[0].1;
    let mut z: f32 = row.iter().map(|l| (l - max).exp()).sum();
    if let Some(r) = rest {
        z += (r.lse - max).exp();
    }
    best.map(|(t, l)| if t == u32::MAX { (t, 0.0) } else { (t, (l - max).exp() / z) })
}

/// 행 하나의 1위 토큰과 확률
pub fn top1(row: &[f32], rest: Option<RestStat>, map: Option<&HeadMap>) -> (u32, f32) {
    top3(row, rest, map)[0]
}

/// teacher forcing 점수: 행마다 (정답 토큰의 음의 로그 확률, 1위 토큰). 진단 · 양자화 배분 측정용 (`examples/quant_screen.rs`).
/// 전체 어휘 소프트맥스이고, 로짓은 장치에서 줄여 행마다 몇 개의 값만 가져옵니다.
pub fn score(output: &Loaded, full: &str, map: Option<&HeadMap>, h: &Tensor, targets: &[u32]) -> Result<Vec<(f32, u32)>> {
    let n = h.dim(0)?;
    if targets.len() != n {
        bail!("정답 {} 개와 행 {n} 개가 다릅니다", targets.len());
    }
    let dev = h.device();
    // 정답이 남긴 행 · 잘린 행 중 어디에 있는지
    let (mut kept_at, mut rest_at, mut in_rest) = (vec![0u32; n], vec![0u32; n], vec![false; n]);
    for (i, &t) in targets.iter().enumerate() {
        match map {
            None => kept_at[i] = t,
            Some(m) => match m.kept.binary_search(&t) {
                Ok(k) => kept_at[i] = k as u32,
                Err(_) => {
                    in_rest[i] = true;
                    rest_at[i] = m.rest.binary_search(&t).map_err(|_| anyhow::anyhow!("토큰 {t} 이 어휘 밖입니다"))? as u32;
                }
            },
        }
    }
    let pick = |l: &Tensor, at: &[u32]| -> Result<Vec<f32>> {
        let idx = Tensor::from_vec(at.to_vec(), (n, 1), dev)?;
        Ok(l.gather(&idx, 1)?.flatten_all()?.to_device(&Device::Cpu)?.to_vec1::<f32>()?)
    };
    let k = output.linear(map.map_or(full, |m| m.kept_name), h)?;
    let ks = summarize(&k)?;
    let kt = pick(&k, &kept_at)?;
    let (rs, rt) = match map {
        Some(m) => {
            let r = output.linear(m.rest_name, h)?;
            (Some(summarize(&r)?), Some(pick(&r, &rest_at)?))
        }
        None => (None, None),
    };
    Ok((0..n)
        .map(|i| {
            let (kmax, karg, klse) = ks[i];
            let mut lse = klse;
            let mut top = map.map_or(karg, |m| m.kept[karg as usize]);
            let mut logit = kt[i];
            if let (Some(rs), Some(rt), Some(m)) = (&rs, &rt, map) {
                let (rmax, rarg, rlse) = rs[i];
                let hi = lse.max(rlse);
                lse = hi + ((lse - hi).exp() + (rlse - hi).exp()).ln();
                if rmax > kmax {
                    top = m.rest[rarg as usize];
                }
                if in_rest[i] {
                    logit = rt[i];
                }
            }
            (lse - logit, top)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::attention;
    use crate::ai::verify::det_normal;

    #[test]
    fn a_split_head_gives_the_full_vocabulary_softmax() {
        // 어휘 8, 남긴 행 2 · 5 · 6 (나머지 잘림)
        let full: Vec<f32> = det_normal(8, 4).iter().map(|v| v * 3.0).collect();
        let map = HeadMap { kept_name: "output.weight", kept: vec![2, 5, 6], rest_name: OUTPUT_REST, rest: vec![0, 1, 3, 4, 7] };
        let kept: Vec<f32> = map.kept.iter().map(|&i| full[i as usize]).collect();
        let rest: Vec<f32> = map.rest.iter().map(|&i| full[i as usize]).collect();
        let (max, at, lse) = stat_of(&rest);
        let stat = RestStat { max, token: map.rest[at as usize], lse };
        let want = attention::top3(&full);
        let got = top3(&kept, Some(stat), Some(&map));
        // 1위와 그 확률은 전체 어휘와 같습니다 (2 · 3위는 잘린 행 가운데 최댓값 하나만 후보라 다를 수 있음).
        assert_eq!(got[0].0, want[0].0);
        assert!((got[0].1 - want[0].1).abs() < 1e-6, "{got:?} {want:?}");
        // 잘린 행을 보지 않으면 남긴 행만의 소프트맥스입니다.
        let only = top3(&kept, None, Some(&map));
        assert!(map.kept.contains(&only[0].0));
    }

    #[test]
    fn a_cut_token_wins_when_its_logit_is_the_largest() {
        let map = HeadMap { kept_name: "output.weight", kept: vec![0, 1], rest_name: OUTPUT_REST, rest: vec![2, 3] };
        let rest = [1.0f32, 9.0];
        let (max, at, lse) = stat_of(&rest);
        let (t, p) = top1(&[2.0, 3.0], Some(RestStat { max, token: map.rest[at as usize], lse }), Some(&map));
        assert_eq!(t, 3, "잘린 행의 9.0 이 이깁니다 (PRESSURE → RESSURE 결함을 막음)");
        let z = (2.0f32 - 9.0).exp() + (3.0f32 - 9.0).exp() + (1.0f32 - 9.0).exp() + 1.0;
        assert!((p - 1.0 / z).abs() < 1e-6);
        let (t, _) = top1(&[2.0, 3.0], None, None);
        assert_eq!(t, 1, "자르지 않은 표는 그대로");
        assert!(map.is_cut(3) && !map.is_cut(1));
    }
}
