//! GGML 블록 형식(Q8_0 · Q6_K · Q5_K · Q4_K) 인코더. 원본 가중치(BF16 을 F32 로 푼 값)를 **한 번만** 양자화합니다.
//!
//! 블록마다의 척도 탐색은 gravity-ocr-qsa-ngram 의 `gravity_ocr/quant.py` 와 같습니다 (그 저장소에서 실측으로 고른 방식).
//! - 중요도 `w = rms(블록) + |x|`: 곱의 대부분을 만드는 큰 가중치를 더 정확히 남깁니다. 중요도 행렬(열마다 입력 활성값의 제곱 평균,
//!   llama.cpp `imatrix` 와 같은 뜻)을 주면 그 열 값을 곱합니다.
//! - 대칭(Q8_0 · Q6_K): 척도 후보 19개 `(nmax + 0.1 s) / amax` (s = −9 ‥ 9) 마다 반올림하고, 그 반올림의 가중 최소제곱 척도
//!   `d = Σ w x q / Σ w q²` 중 가중 오차가 가장 작은 것.
//! - 비대칭(Q4_K · Q5_K): 후보 21개 `(nmax − 1 + 0.1 s) / (max − min(최소, 0))` (s = 0 ‥ 20) 마다 반올림하고 (척도, 오프셋)을
//!   가중 최소제곱으로 풉니다. GGML 은 오프셋을 빼는 값(≥ 0)으로만 담으므로 양수 오프셋이 나오면 0 으로 두고 척도만 다시 풉니다.
//! - 묶음(256) 안 블록들의 척도 · 최솟값은 GGML 대로 묶음 척도(F16)에 상대 양자화하고(Q4_K · Q5_K 6비트, Q6_K 부호 8비트),
//!   양자화한 척도로 값을 다시 반올림합니다 (quant.py `_to_super` 와 같은 순서, 묶음 척도는 최댓값을 덮도록 올림).
//!
//! 비트 배치는 GGML 그대로라 candle 이 장치에서 곱합니다 (시험이 candle 의 역양자화로 확인). quant.py 의 자체 컨테이너와 다른 점:
//! Q8_0 은 GGML 커널과 같은 −127 ‥ 127, Q6_K 묶음 척도는 GGML 의 부호 8비트(−128 ‥ 0, quant.py 는 부호 없는 8비트),
//! 같은 값만 있는 블록은 척도만 풉니다 (quant.py 는 0 으로 둠). 반올림은 numpy `rint` 와 같은 짝수 반올림입니다.
//! Q3_K 는 만들지 않습니다 (gravity-ocr 배분의 q3 자리는 한 단계 올려 Q4_K).

use anyhow::{bail, Result};
use candle_core::quantized::GgmlDType;
use half::f16;
use rayon::prelude::*;

const QK_K: usize = 256;
const QK8_0: usize = 32;
/// 병렬 한 조각의 원소 수
const PAR_ELEMS: usize = 1 << 16;

/// 이 모듈이 만드는 형식
pub fn supported(dtype: GgmlDType) -> bool {
    matches!(dtype, GgmlDType::F32 | GgmlDType::F16 | GgmlDType::BF16 | GgmlDType::Q8_0 | GgmlDType::Q6K | GgmlDType::Q5K | GgmlDType::Q4K)
}

/// 행들(`xs`, 열 `cols`, 행 우선)을 `dtype` 의 GGML 바이트로. `importance` 는 열마다의 중요도입니다 (없으면 모두 1).
/// 행끼리 독립이라 행 묶음마다 병렬입니다.
pub fn encode(xs: &[f32], cols: usize, dtype: GgmlDType, importance: Option<&[f32]>) -> Result<Vec<u8>> {
    if cols == 0 || !xs.len().is_multiple_of(cols) {
        bail!("원소 {} 개는 열 {cols} 의 행으로 나뉘지 않습니다", xs.len());
    }
    if !supported(dtype) {
        bail!("{dtype:?} 는 만들지 않습니다 (Q8_0 · Q6_K · Q5_K · Q4_K · F16 · BF16 · F32)");
    }
    if !cols.is_multiple_of(dtype.block_size()) {
        bail!("열 수 {cols} 가 {dtype:?} 블록 {} 의 배수가 아닙니다", dtype.block_size());
    }
    if let Some(w) = importance {
        if w.len() != cols {
            bail!("중요도 {} 개가 열 {cols} 과 다릅니다", w.len());
        }
        if w.iter().any(|v| !v.is_finite() || *v < 0.0) {
            bail!("중요도에 음수나 유한하지 않은 값이 있습니다");
        }
    }
    Ok(match dtype {
        GgmlDType::F32 => xs.iter().flat_map(|v| v.to_le_bytes()).collect(),
        GgmlDType::F16 => xs.iter().flat_map(|v| f16::from_f32(*v).to_bits().to_le_bytes()).collect(),
        GgmlDType::BF16 => xs.iter().flat_map(|v| half::bf16::from_f32(*v).to_bits().to_le_bytes()).collect(),
        _ => {
            let per = (PAR_ELEMS / cols).max(1) * cols;
            let parts: Vec<Vec<u8>> = xs
                .par_chunks(per)
                .map(|rows| {
                    let mut out = Vec::with_capacity(rows.len() / dtype.block_size() * dtype.type_size());
                    for row in rows.chunks_exact(cols) {
                        encode_row(row, dtype, importance, &mut out);
                    }
                    out
                })
                .collect();
            parts.concat()
        }
    })
}

fn encode_row(row: &[f32], dtype: GgmlDType, importance: Option<&[f32]>, out: &mut Vec<u8>) {
    let bs = dtype.block_size();
    for (b, x) in row.chunks_exact(bs).enumerate() {
        let colw = importance.map(|w| &w[b * bs..(b + 1) * bs]);
        match dtype {
            GgmlDType::Q8_0 => q8_0(x, colw, out),
            GgmlDType::Q6K => q6_k(x, colw, out),
            GgmlDType::Q5K => q45_k(x, colw, 31, out),
            GgmlDType::Q4K => q45_k(x, colw, 15, out),
            _ => unreachable!("supported() 가 거른 형식"),
        }
    }
}

/// quant.py `_importance`: 블록마다 `rms + |x|`. 열 중요도가 있으면 곱하고, 그 합이 0 이면(중요도 0 인 열뿐) 열 중요도 없이 씁니다.
fn importance(x: &[f32], colw: Option<&[f32]>, w: &mut [f32]) {
    let rms = (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    for (wi, xi) in w.iter_mut().zip(x) {
        *wi = rms + xi.abs();
    }
    if let Some(c) = colw {
        let total: f32 = w.iter().zip(c).map(|(a, b)| a * b).sum();
        if total > 0.0 && total.is_finite() {
            w.iter_mut().zip(c).for_each(|(a, b)| *a *= b);
        }
    }
}

/// quant.py `_fit_symmetric`: q ∈ [`qmin`, `nmax`] 반올림의 가중 최소제곱 척도 (≥ 0 이 되도록 부호를 유지하는 반올림)
pub(crate) fn fit_symmetric(x: &[f32], w: &[f32], nmax: i32, qmin: i32) -> f32 {
    let amax = x.iter().fold(0f32, |a, v| a.max(v.abs()));
    if !(amax > 0.0) {
        return 0.0;
    }
    let (lo, hi) = (qmin as f32, nmax as f32);
    let (mut best_err, mut best_d) = (f32::INFINITY, 0f32);
    for s in -9..=9 {
        let inv = (nmax as f32 + 0.1 * s as f32) / amax;
        let (mut num, mut den) = (0f32, 0f32);
        for (&xi, &wi) in x.iter().zip(w) {
            let q = (xi * inv).round_ties_even().clamp(lo, hi);
            num += wi * xi * q;
            den += wi * q * q;
        }
        let d = if den > 0.0 { num / den } else { 0.0 };
        let mut err = 0f32;
        for (&xi, &wi) in x.iter().zip(w) {
            let q = (xi * inv).round_ties_even().clamp(lo, hi);
            let e = d * q - xi;
            err += wi * e * e;
        }
        if err < best_err {
            best_err = err;
            best_d = d;
        }
    }
    best_d
}

/// quant.py `_fit_asymmetric`: q ∈ [0, `nmax`], `x ≈ d q − m` 의 (d, m ≥ 0)
pub(crate) fn fit_asymmetric(x: &[f32], w: &[f32], nmax: i32) -> (f32, f32) {
    let lo = x.iter().fold(0f32, |a, &v| a.min(v));
    let hi = x.iter().fold(f32::NEG_INFINITY, |a, &v| a.max(v));
    let range = hi - lo;
    let safe = if range > 0.0 { range } else { 1.0 };
    let sum_w: f32 = w.iter().sum();
    let sum_x: f32 = w.iter().zip(x).map(|(a, b)| a * b).sum();
    let top = nmax as f32;
    let (mut best_err, mut best_d, mut best_m) = (f32::INFINITY, 0f32, 0f32);
    for s in 0..=20 {
        let inv = (top - 1.0 + 0.1 * s as f32) / safe;
        let (mut sl, mut sl2, mut sxl) = (0f32, 0f32, 0f32);
        for (&xi, &wi) in x.iter().zip(w) {
            let q = ((xi - lo) * inv).round_ties_even().clamp(0.0, top);
            sl += wi * q;
            sl2 += wi * q * q;
            sxl += wi * xi * q;
        }
        let det = sum_w * sl2 - sl * sl;
        let scale_only = || if sl2 > 0.0 { sxl / sl2 } else { 0.0 };
        // 반올림이 모두 같으면(같은 값만 있는 블록) 척도와 오프셋을 함께 풀 수 없습니다: 0 이 아닌 단계면 척도만, 모두 0 단계면 오프셋만
        // (음의 상수 블록 = −m). quant.py 는 이때 둘 다 0 으로 둡니다.
        let (mut d, mut off) = if det > 0.0 {
            ((sum_w * sxl - sum_x * sl) / det, (sl2 * sum_x - sl * sxl) / det)
        } else if sl2 > 0.0 {
            (scale_only(), 0.0)
        } else {
            (0.0, if sum_w > 0.0 { (sum_x / sum_w).min(0.0) } else { 0.0 })
        };
        if off > 0.0 {
            d = scale_only();
            off = 0.0;
        }
        let mut err = 0f32;
        for (&xi, &wi) in x.iter().zip(w) {
            let q = ((xi - lo) * inv).round_ties_even().clamp(0.0, top);
            let e = d * q + off - xi;
            err += wi * e * e;
        }
        if err < best_err {
            best_err = err;
            best_d = d;
            best_m = -off;
        }
    }
    (best_d.max(0.0), best_m.max(0.0))
}

/// 블록 척도들(≥ 0)을 묶음 척도(F16)와 `levels` 단계 정수로. 묶음 척도 × `levels` 가 최댓값을 덮도록 F16 반올림을 올립니다.
fn super_scale<const N: usize>(v: &[f32; N], levels: u8) -> (f16, [u8; N]) {
    let max = v.iter().fold(0f32, |a, &b| a.max(b));
    let mut idx = [0u8; N];
    if !(max > 0.0) {
        return (f16::ZERO, idx);
    }
    let top = levels as f32;
    let mut d = f16::from_f32(max / top);
    if d.to_f32() * top < max {
        d = f16::from_bits(d.to_bits() + 1);
    }
    let df = d.to_f32();
    for (i, &x) in v.iter().enumerate() {
        idx[i] = (x / df).round_ties_even().clamp(0.0, top) as u8;
    }
    (d, idx)
}

/// GGML `get_scale_min_k4` (candle `quantized::utils`): 12 바이트에 묶인 6비트 척도 · 최솟값 j 번째
fn scale_min_k4(j: usize, q: &[u8; 12]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        ((q[j + 4] & 0xF) | ((q[j - 4] >> 6) << 4), (q[j + 4] >> 4) | ((q[j] >> 6) << 4))
    }
}

/// Q8_0: 블록 32, F16 척도, −127 ‥ 127
fn q8_0(x: &[f32], colw: Option<&[f32]>, out: &mut Vec<u8>) {
    let mut w = [0f32; QK8_0];
    importance(x, colw, &mut w);
    let d = f16::from_f32(fit_symmetric(x, &w, 127, -127).max(0.0));
    let df = d.to_f32();
    out.extend_from_slice(&d.to_bits().to_le_bytes());
    for &xi in x {
        let q = if df == 0.0 { 0 } else { (xi / df).round_ties_even().clamp(-127.0, 127.0) as i8 };
        out.push(q as u8);
    }
}

/// Q4_K (`nmax` 15) · Q5_K (`nmax` 31): 묶음 256 = 블록 32 × 8, 블록마다 6비트 척도 · 최솟값
fn q45_k(x: &[f32], colw: Option<&[f32]>, nmax: i32, out: &mut Vec<u8>) {
    let mut w = [0f32; 32];
    let (mut ds, mut ms) = ([0f32; 8], [0f32; 8]);
    for j in 0..8 {
        let xb = &x[32 * j..32 * j + 32];
        importance(xb, colw.map(|c| &c[32 * j..32 * j + 32]), &mut w);
        (ds[j], ms[j]) = fit_asymmetric(xb, &w, nmax);
    }
    let (d, ls) = super_scale(&ds, 63);
    let (dmin, lm) = super_scale(&ms, 63);
    let mut scales = [0u8; 12];
    for j in 0..8 {
        if j < 4 {
            scales[j] = ls[j];
            scales[j + 4] = lm[j];
        } else {
            scales[j + 4] = (ls[j] & 0xF) | ((lm[j] & 0xF) << 4);
            scales[j - 4] |= (ls[j] >> 4) << 6;
            scales[j] |= (lm[j] >> 4) << 6;
        }
    }
    // 저장된 척도 그대로(candle `to_float` 와 같은 식)로 값을 다시 반올림합니다.
    let (df, dmf) = (d.to_f32(), dmin.to_f32());
    let top = nmax as f32;
    let mut l = [0u8; QK_K];
    for j in 0..8 {
        let (sc, m) = scale_min_k4(j, &scales);
        let dd = df * sc as f32;
        let dm = dmf * m as f32;
        if dd == 0.0 {
            continue;
        }
        for i in 0..32 {
            l[32 * j + i] = ((x[32 * j + i] + dm) / dd).round_ties_even().clamp(0.0, top) as u8;
        }
    }
    out.extend_from_slice(&d.to_bits().to_le_bytes());
    out.extend_from_slice(&dmin.to_bits().to_le_bytes());
    out.extend_from_slice(&scales);
    let mut qs = [0u8; QK_K / 2];
    if nmax == 31 {
        let mut qh = [0u8; QK_K / 8];
        let (mut m1, mut m2) = (1u8, 2u8);
        for n in (0..QK_K).step_by(64) {
            for j in 0..32 {
                let (mut l1, mut l2) = (l[n + j], l[n + j + 32]);
                if l1 > 15 {
                    l1 -= 16;
                    qh[j] |= m1;
                }
                if l2 > 15 {
                    l2 -= 16;
                    qh[j] |= m2;
                }
                qs[n / 2 + j] = l1 | (l2 << 4);
            }
            m1 <<= 2;
            m2 <<= 2;
        }
        out.extend_from_slice(&qh);
    } else {
        for n in (0..QK_K).step_by(64) {
            for j in 0..32 {
                qs[n / 2 + j] = l[n + j] | (l[n + j + 32] << 4);
            }
        }
    }
    out.extend_from_slice(&qs);
}

/// Q6_K: 묶음 256 = 블록 16 × 16, 블록마다 부호 8비트 척도, 값 −32 ‥ 31
fn q6_k(x: &[f32], colw: Option<&[f32]>, out: &mut Vec<u8>) {
    let mut w = [0f32; 16];
    let mut ds = [0f32; 16];
    for s in 0..16 {
        let xb = &x[16 * s..16 * s + 16];
        importance(xb, colw.map(|c| &c[16 * s..16 * s + 16]), &mut w);
        ds[s] = fit_symmetric(xb, &w, 31, -32).max(0.0);
    }
    // GGML 처럼 음의 묶음 척도로 부호 8비트의 −128 단계까지 씁니다 (블록 척도 = 묶음 척도 × 음의 정수 ≥ 0).
    let max = ds.iter().fold(0f32, |a, &b| a.max(b));
    let mut scales = [0i8; 16];
    let mut d = f16::ZERO;
    if max > 0.0 {
        d = f16::from_f32(-max / 128.0);
        if d.to_f32().abs() * 128.0 < max {
            d = f16::from_bits(d.to_bits() + 1);
        }
        let df = d.to_f32();
        for s in 0..16 {
            scales[s] = (ds[s] / df).round_ties_even().clamp(-128.0, 127.0) as i8;
        }
    }
    let df = d.to_f32();
    let mut l = [32u8; QK_K];
    for s in 0..16 {
        let dd = df * scales[s] as f32;
        if dd == 0.0 {
            continue;
        }
        for i in 0..16 {
            let q = (x[16 * s + i] / dd).round_ties_even().clamp(-32.0, 31.0) as i32;
            l[16 * s + i] = (q + 32) as u8;
        }
    }
    let (mut ql, mut qh) = ([0u8; QK_K / 2], [0u8; QK_K / 4]);
    for (half, j) in (0..QK_K).step_by(128).enumerate() {
        for k in 0..32 {
            let (a, b, c, e) = (l[j + k], l[j + k + 32], l[j + k + 64], l[j + k + 96]);
            ql[64 * half + k] = (a & 0xF) | ((c & 0xF) << 4);
            ql[64 * half + k + 32] = (b & 0xF) | ((e & 0xF) << 4);
            qh[32 * half + k] = (a >> 4) | ((b >> 4) << 2) | ((c >> 4) << 4) | ((e >> 4) << 6);
        }
    }
    out.extend_from_slice(&ql);
    out.extend_from_slice(&qh);
    out.extend(scales.iter().map(|s| *s as u8));
    out.extend_from_slice(&d.to_bits().to_le_bytes());
}

/// GGML 바이트(행 `rows`, 열 `cols`)를 F32 로 풉니다 (candle 의 역양자화). 진단 · 시험용입니다.
pub fn decode(dtype: GgmlDType, bytes: &[u8], rows: usize, cols: usize) -> Result<Vec<f32>> {
    use candle_core::quantized::ggml_file::qtensor_from_ggml;
    use candle_core::Device;
    let half = |b: &[u8]| u16::from_le_bytes([b[0], b[1]]);
    Ok(match dtype {
        GgmlDType::F32 => bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect(),
        GgmlDType::BF16 => bytes.chunks_exact(2).map(|b| f32::from_bits((half(b) as u32) << 16)).collect(),
        GgmlDType::F16 => bytes.chunks_exact(2).map(|b| f16::from_bits(half(b)).to_f32()).collect(),
        _ => {
            // `qtensor_from_ggml` 이 블록으로 다시 보므로 u32 로 정렬된 버퍼에 옮깁니다.
            let mut words = vec![0u32; bytes.len().div_ceil(4)];
            let (_, raw, _) = unsafe { words.align_to_mut::<u8>() };
            raw[..bytes.len()].copy_from_slice(bytes);
            let q = qtensor_from_ggml(dtype, &raw[..bytes.len()], vec![rows, cols], &Device::Cpu)?;
            q.dequantize(&Device::Cpu)?.flatten_all()?.to_vec1::<f32>()?
        }
    })
}

/// 양자화 전후의 가중치 상대오차 ‖Ŵ − W‖ / ‖W‖ (진단)
pub fn relative_error(xs: &[f32], cols: usize, dtype: GgmlDType, importance: Option<&[f32]>) -> Result<f32> {
    let q = encode(xs, cols, dtype, importance)?;
    let back = decode(dtype, &q, xs.len() / cols, cols)?;
    Ok(rel(&back, xs))
}

/// ‖a − b‖ / ‖b‖
pub fn rel(a: &[f32], b: &[f32]) -> f32 {
    let (mut num, mut den) = (0f64, 0f64);
    for (x, y) in a.iter().zip(b) {
        num += ((x - y) as f64).powi(2);
        den += (*y as f64).powi(2);
    }
    (num / den.max(1e-30)).sqrt() as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::verify::det_normal;

    /// 가우시안 행 + 드문 큰 값 (실제 가중치처럼 꼬리가 두꺼움)
    fn heavy(rows: usize, cols: usize, seed: u64) -> Vec<f32> {
        let mut xs = det_normal(rows * cols, seed);
        for (i, x) in xs.iter_mut().enumerate() {
            *x *= 0.02;
            if i % 97 == 0 {
                *x *= 6.0;
            }
        }
        xs
    }

    /// candle 의 기본 양자화(`from_float`)로 만든 바이트
    fn candle_plain(xs: &[f32], dtype: GgmlDType) -> Vec<u8> {
        let mut q = dtype.cpu_zeros(xs.len());
        q.from_float(xs);
        unsafe { std::slice::from_raw_parts(q.as_ptr(), q.storage_size_in_bytes()) }.to_vec()
    }

    #[test]
    fn the_block_fits_match_gravity_ocr_quant_py() {
        // gravity_ocr/quant.py 로 같은 입력을 풀어 얻은 값 (scratch 스크립트, numpy f32)
        let x: Vec<f32> = (0..32).map(|i| (i as f32 * 1.3).sin() * 0.05 + if i % 7 == 0 { 0.2 } else { 0.0 }).collect();
        let mut w = [0f32; 32];
        importance(&x, None, &mut w);
        let close = |got: f32, want: f32| (got - want).abs() <= want.abs() * 2e-5;
        let (d, m) = fit_asymmetric(&x, &w, 15);
        assert!(close(d, 1.925078221e-02) && close(m, 4.800733924e-02), "q4 {d} {m}");
        let (d, m) = fit_asymmetric(&x, &w, 31);
        assert!(close(d, 9.597460739e-03) && close(m, 4.951747879e-02), "q5 {d} {m}");
        let x16: Vec<f32> = x[..16].iter().map(|v| v - 0.01).collect();
        let mut w16 = [0f32; 16];
        importance(&x16, None, &mut w16);
        let d = fit_symmetric(&x16, &w16, 31, -32);
        assert!(close(d, 6.606251933e-03), "q6 {d}");
        // quant.py 의 q8 은 −128 까지 씁니다 (이 모듈의 Q8_0 은 GGML 범위 −127).
        let d = fit_symmetric(&x, &w, 127, -128);
        assert!(close(d, 1.904964098e-03), "q8 {d}");
    }

    #[test]
    fn candle_reads_the_layout_and_the_error_beats_its_own_quantizer() -> Result<()> {
        let cols = 1024;
        let xs = heavy(32, cols, 7);
        for dtype in [GgmlDType::Q4K, GgmlDType::Q5K, GgmlDType::Q6K, GgmlDType::Q8_0] {
            let ours = encode(&xs, cols, dtype, None)?;
            assert_eq!(ours.len(), xs.len() / dtype.block_size() * dtype.type_size(), "{dtype:?} 바이트 수");
            let back = decode(dtype, &ours, 32, cols)?;
            let mine = rel(&back, &xs);
            let theirs = rel(&decode(dtype, &candle_plain(&xs, dtype), 32, cols)?, &xs);
            println!("{dtype:?}: 이 탐색 {mine:.5} · candle 기본 {theirs:.5}");
            // 배치가 틀리면 역양자화 값이 엉뚱해져 오차가 수십 % 가 됩니다 (4비트 · 블록 32 의 정상 오차는 약 9 %).
            let bound = match dtype {
                GgmlDType::Q4K => 0.11,
                GgmlDType::Q5K => 0.055,
                GgmlDType::Q6K => 0.03,
                _ => 0.008,
            };
            assert!(mine < bound, "{dtype:?} 오차 {mine}");
            assert!(mine <= theirs * 1.02, "{dtype:?}: 이 탐색 {mine:.5} · candle 기본 {theirs:.5}");
        }
        // 비트가 많을수록 오차가 작습니다.
        let e: Vec<f32> = [GgmlDType::Q4K, GgmlDType::Q5K, GgmlDType::Q6K, GgmlDType::Q8_0].iter().map(|d| relative_error(&xs, cols, *d, None).unwrap()).collect();
        assert!(e.windows(2).all(|w| w[0] > w[1]), "{e:?}");
        Ok(())
    }

    #[test]
    fn zero_and_constant_blocks_stay_finite() -> Result<()> {
        let cols = 256;
        let mut xs = vec![0f32; cols];
        xs.extend(std::iter::repeat_n(0.03f32, cols));
        xs.extend(std::iter::repeat_n(-0.03f32, cols));
        for dtype in [GgmlDType::Q4K, GgmlDType::Q5K, GgmlDType::Q6K, GgmlDType::Q8_0] {
            let back = decode(dtype, &encode(&xs, cols, dtype, None)?, 3, cols)?;
            assert!(back.iter().all(|v| v.is_finite()), "{dtype:?}");
            assert!(back[..cols].iter().all(|v| *v == 0.0), "{dtype:?}: 0 행은 0");
            let err = rel(&back[cols..], &xs[cols..]);
            assert!(err < 0.02, "{dtype:?}: 같은 값 행 오차 {err}");
        }
        Ok(())
    }

    #[test]
    fn column_importance_moves_the_error_to_unimportant_columns() -> Result<()> {
        let cols = 512;
        let xs = heavy(64, cols, 11);
        // 짝수 열이 100 배 중요. 중요도는 블록 안의 상대 가중이라 블록 안에서 달라야 효과가 있습니다
        // (한 블록이 모두 같은 중요도면 최소제곱의 답이 같음).
        let imp: Vec<f32> = (0..cols).map(|c| if c % 2 == 0 { 100.0 } else { 1.0 }).collect();
        let even = |v: &[f32]| -> Vec<f32> { v.iter().step_by(2).copied().collect() };
        for dtype in [GgmlDType::Q4K, GgmlDType::Q5K, GgmlDType::Q6K, GgmlDType::Q8_0] {
            let plain = decode(dtype, &encode(&xs, cols, dtype, None)?, 64, cols)?;
            let weighted = decode(dtype, &encode(&xs, cols, dtype, Some(&imp))?, 64, cols)?;
            let (a, b) = (rel(&even(&weighted), &even(&xs)), rel(&even(&plain), &even(&xs)));
            assert!(a < b, "{dtype:?}: 중요한 열 오차 {a:.5} (가중) · {b:.5} (균일)");
        }
        assert!(encode(&xs, cols, GgmlDType::Q4K, Some(&imp[..10])).is_err(), "중요도 길이가 다르면 거부");
        Ok(())
    }

    #[test]
    fn unsupported_or_misaligned_requests_are_refused() {
        assert!(encode(&[0.0; 256], 256, GgmlDType::Q3K, None).is_err(), "Q3_K 는 만들지 않음");
        assert!(encode(&[0.0; 200], 100, GgmlDType::Q8_0, None).is_err(), "열이 블록의 배수가 아님");
        assert!(encode(&[0.0; 10], 3, GgmlDType::F32, None).is_err());
        let b = encode(&[1.5, -2.0], 2, GgmlDType::BF16, None).unwrap();
        assert_eq!(decode(GgmlDType::BF16, &b, 1, 2).unwrap(), vec![1.5, -2.0]);
    }
}
