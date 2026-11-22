//! GPU 커널 자가 검증 (logis-center `models/common/accel_check.rs` 와 같은 방식).
//! 장치와 연산 조합마다 처음 쓰기 전에 한 번만 CPU 기준 계산과 비교합니다.
//! 틀리면 그 장치의 그 연산만 CPU 로 돌리고, 판정은 프로세스가 끝날 때까지 캐시합니다.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Mutex, MutexGuard, OnceLock};

use anyhow::Result;
use candle_core::{DType, Device, Tensor};

use super::accel;

/// 자가 검증을 끄는 환경 변수 (`off` / `0` / `false` …). logis-center 의 `LOGIS_ACCEL_CHECK` 와 같은 역할.
pub const CHECK_ENV: &str = "BLUEPRINT_ACCEL_CHECK";

pub fn checks_enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| match std::env::var(CHECK_ENV) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "off" | "false" | "no" | "disable" | "disabled"
        ),
        Err(_) => true,
    })
}

fn verdicts() -> MutexGuard<'static, HashMap<String, bool>> {
    static V: OnceLock<Mutex<HashMap<String, bool>>> = OnceLock::new();
    V.get_or_init(|| Mutex::new(HashMap::new())).lock().unwrap_or_else(|p| p.into_inner())
}

/// `op` 연산을 `device` 에서 써도 되는지 정합니다.
///
/// `probe` 는 장치에서 계산한 결과와 CPU 기준값의 상대오차를 돌려줍니다. 같은 (장치, 연산) 에 대해 한 번만 호출됩니다.
/// CPU 이거나 `BLUEPRINT_ACCEL_CHECK=off` 이면 검증 없이 true 입니다.
pub fn trusted(device: &Device, op: &str, tol: f32, probe: impl FnOnce() -> Result<f32>) -> bool {
    if !accel::is_gpu(device) || !checks_enabled() {
        return true;
    }
    let key = format!("{}#{:?}/{op}", accel::backend_name(device), device.location());
    if let Some(ok) = verdicts().get(&key) {
        return *ok;
    }
    let ok = match catch_unwind(AssertUnwindSafe(probe)) {
        Ok(Ok(err)) if err <= tol => {
            eprintln!("[BLUEPRINT-ACCEL] 통과 · {key} (상대오차 {err:.1e} ≤ {tol:.0e})");
            true
        }
        Ok(Ok(err)) => {
            eprintln!("[BLUEPRINT-ACCEL] ❌ {key}: 상대오차 {err:.2e} > {tol:.0e} → 이 연산은 CPU 로 실행합니다");
            false
        }
        Ok(Err(e)) => {
            eprintln!("[BLUEPRINT-ACCEL] ❌ {key}: 검증 실행 실패 ({e}) → CPU 로 실행합니다");
            false
        }
        Err(_) => {
            eprintln!("[BLUEPRINT-ACCEL] ❌ {key}: 검증 중 패닉 → CPU 로 실행합니다");
            false
        }
    };
    verdicts().insert(key, ok);
    ok
}

/// 실제 데이터에서 잘못된 것이 드러난 연산을 이 프로세스가 끝날 때까지 쓰지 않습니다.
pub fn revoke(device: &Device, op: &str) {
    verdicts().insert(format!("{}#{:?}/{op}", accel::backend_name(device), device.location()), false);
}

/// 결정적 정규분포 난수 (xorshift64 + Box–Muller). 장치와 무관하게 같은 검증 입력을 만듭니다.
pub fn det_normal(n: usize, seed: u64) -> Vec<f32> {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0xD1B5_4A32_D192_ED03;
    if s == 0 {
        s = 1;
    }
    let mut next = move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        ((s >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    };
    (0..n)
        .map(|_| {
            let (u1, u2) = (next(), next());
            ((-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()) as f32
        })
        .collect()
}

/// 상대오차 ‖a−b‖/‖b‖ (f32). `b` 가 CPU 기준값입니다. NaN/Inf 가 섞이면 Inf.
pub fn rel_err(a: &Tensor, b: &Tensor) -> Result<f32> {
    let a = a.to_dtype(DType::F32)?;
    let b = b.to_dtype(DType::F32)?.to_device(a.device())?;
    let d = (&a - &b)?.sqr()?.sum_all()?.to_scalar::<f32>()?;
    let n = b.sqr()?.sum_all()?.to_scalar::<f32>()?;
    let r = (d / n.max(1e-30)).sqrt();
    Ok(if r.is_finite() { r } else { f32::INFINITY })
}

/// 장치의 `candle_nn::ops::layer_norm` 커널이 CPU 기준과 일치하는지 검증합니다.
/// 평균이 큰 행(`1000 + N(0,1)`)도 함께 점검해 단패스 `E[x²]-E[x]²` 소거 오차(BUG(LN-1))를 탐지합니다.
pub fn layer_norm_trusted(device: &Device) -> bool {
    trusted(device, "layer_norm", 1e-3, || {
        let mut raw = det_normal(4 * 128, 21);
        // 후반 2행에 큰 DC 오프셋(+1000.0)을 더해 두 단계 중심 분산 정밀도를 검증합니다.
        for v in &mut raw[2 * 128..] {
            *v += 1000.0;
        }
        let x = Tensor::from_vec(raw, (4, 128), &Device::Cpu)?;
        let w = Tensor::from_vec(det_normal(128, 22), 128, &Device::Cpu)?;
        let b = Tensor::from_vec(det_normal(128, 23), 128, &Device::Cpu)?;
        let x64 = x.to_dtype(DType::F64)?;
        let mean = x64.mean_keepdim(candle_core::D::Minus1)?;
        let c = x64.broadcast_sub(&mean)?;
        let var = c.sqr()?.mean_keepdim(candle_core::D::Minus1)?;
        let want = c
            .broadcast_div(&(var + 1e-5)?.sqrt()?)?
            .to_dtype(DType::F32)?
            .broadcast_mul(&w)?
            .broadcast_add(&b)?;
        let got = candle_nn::ops::layer_norm(
            &x.to_device(device)?,
            &w.to_device(device)?,
            &b.to_device(device)?,
            1e-5,
        )?;
        rel_err(&got.to_device(&Device::Cpu)?, &want)
    })
}

/// 장치의 `candle_nn::flash_attn` (비인과) 커널이 CPU 기준과 일치하는지 검증합니다.
pub fn flash_attn_trusted(device: &Device) -> bool {
    trusted(device, "flash_attn", 5e-3, || {
        let (b, h, n, d) = (1usize, 4usize, 32usize, 64usize);
        let q = Tensor::from_vec(det_normal(b * h * n * d, 31), (b, h, n, d), &Device::Cpu)?;
        let k = Tensor::from_vec(det_normal(b * h * n * d, 32), (b, h, n, d), &Device::Cpu)?;
        let v = Tensor::from_vec(det_normal(b * h * n * d, 33), (b, h, n, d), &Device::Cpu)?;
        let scale = 1.0f32 / (d as f32).sqrt();
        let scores = (q.matmul(&k.transpose(2, 3)?.contiguous()?)? * scale as f64)?;
        let want = candle_nn::ops::softmax_last_dim(&scores)?.matmul(&v)?;
        let cfg = candle_nn::flash_attn::FlashAttnConfig::new(scale);
        let got = candle_nn::flash_attn::flash_attn_bhsd(
            &q.to_device(device)?,
            &k.to_device(device)?,
            &v.to_device(device)?,
            &cfg,
        )?;
        rel_err(&got.to_device(&Device::Cpu)?, &want)
    })
}

/// Pack C 융합 디코더 커널(`residual_rmsnorm` · `silu_mul`)이 장치에서 CPU 기준과 일치하는지 검증합니다.
pub fn decoder_ops_trusted(device: &Device) -> bool {
    trusted(device, "decoder_ops", 1e-3, || {
        let (rows, d) = (4usize, 256usize);
        let x = Tensor::from_vec(det_normal(rows * d, 51), (rows, d), &Device::Cpu)?;
        let delta = Tensor::from_vec(det_normal(rows * d, 52), (rows, d), &Device::Cpu)?;
        let w = Tensor::from_vec(det_normal(d, 53), d, &Device::Cpu)?;
        let (want_r, want_y) = candle_core::decoder_ops::residual_rmsnorm(&x, Some(&delta), &w, 1e-5, false)?;
        let (got_r, got_y) = candle_core::decoder_ops::residual_rmsnorm(
            &x.to_device(device)?,
            Some(&delta.to_device(device)?),
            &w.to_device(device)?,
            1e-5,
            false,
        )?;
        let e1 = rel_err(&got_r.to_device(&Device::Cpu)?, &want_r)?;
        let e2 = rel_err(&got_y.to_device(&Device::Cpu)?, &want_y)?;
        let gate = Tensor::from_vec(det_normal(rows * d, 54), (rows, d), &Device::Cpu)?;
        let up = Tensor::from_vec(det_normal(rows * d, 55), (rows, d), &Device::Cpu)?;
        let want_s = candle_core::decoder_ops::silu_mul(&gate, &up)?;
        let got_s = candle_core::decoder_ops::silu_mul(&gate.to_device(device)?, &up.to_device(device)?)?;
        let e3 = rel_err(&got_s.to_device(&Device::Cpu)?, &want_s)?;
        Ok(e1.max(e2).max(e3))
    })
}

/// Pack A/C `lm_head::logits_argmax_cs` 커널이 장치에서 CPU 기준과 일치하는지 검증합니다.
pub fn lm_head_argmax_trusted(device: &Device) -> bool {
    trusted(device, "lm_head_argmax_cs", 1e-3, || {
        let (m, v, k) = (2usize, 128usize, 64usize);
        let logits = Tensor::from_vec(det_normal(m * v, 61), (m, v), &Device::Cpu)?;
        let hidden = Tensor::from_vec(det_normal(m * k, 62), (m, k), &Device::Cpu)?;
        let rest_w = Tensor::from_vec(det_normal(32 * k, 63), (32, k), &Device::Cpu)?;
        let cids: Vec<u32> = (0..v as u32).map(|i| i * 2 + 1).collect();
        let cids_t = Tensor::from_vec(cids, (v,), &Device::Cpu)?;
        let cs_cpu = candle_core::lm_head::CsGuardParams::from_rest_weight(&rest_w, None, 1e-3)?;
        let want = candle_core::lm_head::logits_argmax_cs(&logits, Some(&hidden), Some(&cids_t), Some(&cs_cpu))?;
        let cs_dev = candle_core::lm_head::CsGuardParams {
            mu_rest: cs_cpu.mu_rest.to_device(device)?,
            w_rest_bound: cs_cpu.w_rest_bound,
            b_rest_max: cs_cpu.b_rest_max,
        };
        let got = candle_core::lm_head::logits_argmax_cs(
            &logits.to_device(device)?,
            Some(&hidden.to_device(device)?),
            Some(&cids_t.to_device(device)?),
            Some(&cs_dev),
        )?;
        let (want_ids, _) = want.download_ids_and_mask()?;
        let (got_ids, _) = got.download_ids_and_mask()?;
        if want_ids != got_ids {
            return Ok(f32::INFINITY);
        }
        let want_v = want.max_vals.as_ref().unwrap();
        let got_v = got.max_vals.as_ref().unwrap().to_device(&Device::Cpu)?;
        rel_err(&got_v, want_v)
    })
}

/// GPU Dense Mode A `lm_head::fused_linear_argmax_cs` (0-byte `[M, V]` 로짓 물질화) 검증.
pub fn lm_head_dense_argmax_trusted(device: &Device) -> bool {
    trusted(device, "lm_head_dense_argmax_cs", 1e-3, || {
        let (m, v, k) = (2usize, 128usize, 64usize);
        let h_cpu = Tensor::from_vec(det_normal(m * k, 65), (m, k), &Device::Cpu)?;
        let w_cpu = Tensor::from_vec(det_normal(v * k, 66), (v, k), &Device::Cpu)?;
        let want = candle_core::lm_head::fused_linear_argmax_cs(&h_cpu, &w_cpu, None, None, None)?;
        let got = candle_core::lm_head::fused_linear_argmax_cs(
            &h_cpu.to_device(device)?,
            &w_cpu.to_device(device)?,
            None,
            None,
            None,
        )?;
        let (want_ids, _) = want.download_ids_and_mask()?;
        let (got_ids, _) = got.download_ids_and_mask()?;
        if want_ids != got_ids {
            return Ok(f32::INFINITY);
        }
        let want_v = want.max_vals.as_ref().unwrap();
        let got_v = got.max_vals.as_ref().unwrap().to_device(&Device::Cpu)?;
        rel_err(&got_v, want_v)
    })
}

/// CUDA · ROCm Quantized Mode A `lm_head::fused_quant_argmax_cs` (0-byte `[M, V]` 로짓 물질화, `M > 8` 청킹 포함) 검증.
pub fn lm_head_quant_argmax_trusted(device: &Device) -> bool {
    if !(device.is_rocm() || device.is_cuda()) {
        return false;
    }
    trusted(device, "lm_head_quant_argmax_cs", 1e-3, || {
        use candle_core::quantized::{GgmlDType, QTensor};
        let (m, v, k) = (12usize, 128usize, 256usize);
        let h_cpu = Tensor::from_vec(det_normal(m * k, 71), (m, k), &Device::Cpu)?;
        let w_cpu = Tensor::from_vec(det_normal(v * k, 72), (v, k), &Device::Cpu)?;
        let h_dev = h_cpu.to_device(device)?;
        let w_dev = w_cpu.to_device(device)?;
        for dtype in [GgmlDType::Q8_0, GgmlDType::Q4_0, GgmlDType::Q4K, GgmlDType::Q5K, GgmlDType::Q6K] {
            let q_cpu = QTensor::quantize(&w_cpu, dtype)?;
            let logits_ref = q_cpu.forward_exact(&h_cpu)?;
            let want = candle_core::lm_head::logits_argmax_cs(&logits_ref, Some(&h_cpu), None, None)?;
            let q_dev = QTensor::quantize(&w_dev, dtype)?;
            let got = candle_core::lm_head::fused_quant_argmax_cs(&h_dev, &q_dev, None, None, None)?;
            let (want_ids, _) = want.download_ids_and_mask()?;
            let (got_ids, _) = got.download_ids_and_mask()?;
            if want_ids != got_ids {
                return Ok(f32::INFINITY);
            }
            if let (Some(wv), Some(gv)) = (want.max_vals.as_ref(), got.max_vals.as_ref()) {
                let err = rel_err(&gv.to_device(&Device::Cpu)?, wv)?;
                if err > 1e-3 {
                    return Ok(err);
                }
            }
        }
        Ok(0.0)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn det_normal_is_deterministic_and_centered() {
        let a = det_normal(4096, 7);
        let b = det_normal(4096, 7);
        assert_eq!(a, b);
        let mean = a.iter().sum::<f32>() / a.len() as f32;
        assert!(mean.abs() < 0.1, "mean {mean}");
        assert_ne!(det_normal(16, 1), det_normal(16, 2));
    }

    #[test]
    fn rel_err_is_zero_for_equal_and_inf_for_nan() -> Result<()> {
        let x = Tensor::from_vec(vec![1f32, 2.0, 3.0], 3, &Device::Cpu)?;
        assert_eq!(rel_err(&x, &x)?, 0.0);
        let nan = Tensor::from_vec(vec![f32::NAN, 2.0, 3.0], 3, &Device::Cpu)?;
        assert_eq!(rel_err(&nan, &x)?, f32::INFINITY);
        Ok(())
    }

    #[test]
    fn cpu_is_always_trusted_without_probing() {
        let ok = trusted(&Device::Cpu, "never-probed", 0.0, || panic!("CPU 는 검증하지 않습니다"));
        assert!(ok);
    }
}
