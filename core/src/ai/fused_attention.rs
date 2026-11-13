//! 디코딩 패스의 묶음 질의(GQA) 주의를 커널 하나로 (ROCm, `kernels/attend.hip`, gravity-ocr 의 `gk_lm_attention` 과 같은 생각).
//!
//! 일반 경로(`attention::attend`)는 층마다 질의 복사 · 캐시 구간마다 행렬곱 · 이어 붙이기 · 척도 · softmax · 확률 조각 복사 · 행렬곱 · 더하기로
//! 커널을 열다섯 번쯤 띄웁니다. 새 토큰이 몇 개뿐인 디코딩 패스에서는 이 띄우기 비용이 계산보다 큽니다.
//! 이 커널은 (질의 행, KV 헤드)마다 한 블록에서, 같은 KV 헤드를 읽는 질의 헤드들이 키 · 값을 한 번 읽어 나눠 씁니다.
//! 캐시는 `LayerCache` 의 모양(전치한 키 `[Hkv, D, T]` · 값 `[Hkv, T, D]`, 구간 둘)을 그대로 읽어 이어 붙이지 않습니다.
//!
//! 쓰는 조건: ROCm 장치 · F32 · 연속 메모리, 새 토큰 `n ≤ MAX_ROWS`, head_dim 64 · 128 · 256, 질의 헤드 / KV 헤드 ≤ 8, 캐시 구간 ≤ 2,
//! 처음 쓰기 전 CPU 기준과의 자가 검증(`ai::verify`) 통과. 아니면 `None` 이고 호출자가 일반 경로를 씁니다.
//! 기본은 꺼짐이고 `BLUEPRINT_FUSED_ATTN=1` 이면 켭니다 (RX 6600 에서 행렬곱 경로보다 느렸음, [`ENV`]).

use anyhow::Result;
use candle_core::Tensor;

/// 켜는 환경 변수 (`1` · `on`). 기본은 꺼짐입니다: RX 6600 실측에서 행렬곱 경로보다 느렸습니다 (gocr 디코드 9.29 → 12.47 ms/토큰,
/// `docs/verification.md` 15절). 새 토큰 하나면 블록이 KV 헤드 수(8)뿐이라 GPU 를 다 쓰지 못하고, 검증 패스(33행)는 블록마다 같은 K · V 를 다시 읽습니다.
pub const ENV: &str = "BLUEPRINT_FUSED_ATTN";
/// 커널을 쓰는 새 토큰 수의 상한 (그보다 많으면 프리필이라 행렬곱 경로가 빠름)
pub const MAX_ROWS: usize = 64;

pub fn enabled() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| std::env::var(ENV).is_ok_and(|v| matches!(v.trim().to_ascii_lowercase().as_str(), "1" | "on" | "true")))
}

/// 새 토큰의 질의 `q` `[H, n, D]` 로 캐시 구간들 `past` (전치한 키 `[Hkv, D, T]`, 값 `[Hkv, T, D]`) 와 새 토큰의 `kt` `[Hkv, D, n]` · `v` `[Hkv, n, D]`
/// 를 읽어 `[H, n, D]` 를 돌려줍니다. `causal` 이면 새 토큰 i 는 새 토큰 0 ‥ i 만 봅니다. 쓸 수 없으면 `Ok(None)`.
pub fn attend(q: &Tensor, kt: &Tensor, v: &Tensor, past: &[(&Tensor, &Tensor)], causal: bool, scale: f64) -> Result<Option<Tensor>> {
    #[cfg(feature = "rocm")]
    {
        rocm::attend(q, kt, v, past, causal, scale)
    }
    #[cfg(not(feature = "rocm"))]
    {
        let _ = (q, kt, v, past, causal, scale);
        Ok(None)
    }
}

#[cfg(feature = "rocm")]
mod rocm {
    use std::ffi::c_void;
    use std::sync::OnceLock;

    use anyhow::{anyhow, bail, Result};
    use candle_core::rocm_backend::hip_runtime::module::HipModule;
    use candle_core::{DType, Device, Storage, Tensor};

    use super::MAX_ROWS;
    use crate::ai::attention::{attend as reference, AttnMode, StepMask};
    use crate::ai::verify;

    /// `build.rs` 가 컴파일한 코드 객체
    const IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/attend.co"));
    const KERNEL: &str = "bp_attend";
    /// CPU 기준값과의 허용 상대오차 (합 순서만 다름)
    const TOL: f32 = 1e-4;

    fn module() -> Option<&'static HipModule> {
        static M: OnceLock<Option<HipModule>> = OnceLock::new();
        M.get_or_init(|| {
            if IMAGE.is_empty() {
                return None;
            }
            match HipModule::load_data(IMAGE) {
                Ok(m) if m.get_function(KERNEL).is_ok() => Some(m),
                Ok(_) => None,
                Err(e) => {
                    eprintln!("[BLUEPRINT-ATTN] 주의 커널을 올릴 수 없어 일반 경로를 씁니다 ({e}, 빌드 대상 {})", env!("BLUEPRINT_HIP_ARCHS"));
                    None
                }
            }
        })
        .as_ref()
    }

    /// 텐서 첫 원소의 장치 주소 (F32)
    fn ptr(t: &Tensor) -> Result<*mut c_void> {
        let (st, layout) = t.storage_and_layout();
        match &*st {
            Storage::Rocm(s) => Ok(unsafe { (s.device_ptr() as *mut u8).add(layout.start_offset() * 4) } as *mut c_void),
            _ => bail!("ROCm 장치의 텐서가 아닙니다"),
        }
    }

    /// 모양 · 형식이 커널의 가정에 맞는가
    fn fits(q: &Tensor, kt: &Tensor, v: &Tensor, past: &[(&Tensor, &Tensor)]) -> Result<bool> {
        let (h, n, d) = q.dims3()?;
        let (hkv, kd, kn) = kt.dims3()?;
        if n == 0 || n > MAX_ROWS || past.len() > 2 || hkv == 0 || h % hkv != 0 || h / hkv > 8 || !matches!(d, 64 | 128 | 256) {
            return Ok(false);
        }
        if (kd, kn) != (d, n) || v.dims3()? != (hkv, n, d) {
            return Ok(false);
        }
        for (pk, pv) in past {
            let (a, b, len) = pk.dims3()?;
            if (a, b) != (hkv, d) || pv.dims3()? != (hkv, len, d) {
                return Ok(false);
            }
        }
        let all = [q, kt, v].into_iter().chain(past.iter().flat_map(|(a, b)| [*a, *b]));
        Ok(all.into_iter().all(|t| t.dtype() == DType::F32 && t.is_contiguous()))
    }

    fn launch(m: &HipModule, q: &Tensor, kt: &Tensor, v: &Tensor, past: &[(&Tensor, &Tensor)], causal: bool, scale: f64) -> Result<Tensor> {
        let (h, n, d) = q.dims3()?;
        let hkv = kt.dim(0)?;
        let out = Tensor::zeros((h, n, d), DType::F32, q.device())?;
        let seg = |i: usize| -> Result<(*mut c_void, *mut c_void, i32)> {
            Ok(match past.get(i) {
                Some((k, v)) => (ptr(k)?, ptr(v)?, v.dim(1)? as i32),
                None => (std::ptr::null_mut(), std::ptr::null_mut(), 0),
            })
        };
        let (mut k0, mut v0, mut l0) = seg(0)?;
        let (mut k1, mut v1, mut l1) = seg(1)?;
        let (mut qp, mut k2, mut v2, mut op) = (ptr(q)?, ptr(kt)?, ptr(v)?, ptr(&out)?);
        let (mut l2, mut nn, mut rep, mut dd, mut cc) = (n as i32, n as i32, (h / hkv) as i32, d as i32, causal as i32);
        let mut sc = scale as f32;
        let a = |p: &mut *mut c_void| p as *mut *mut c_void as *mut c_void;
        let i = |p: &mut i32| p as *mut i32 as *mut c_void;
        let mut args: [*mut c_void; 16] = [
            a(&mut qp),
            a(&mut k0),
            a(&mut v0),
            i(&mut l0),
            a(&mut k1),
            a(&mut v1),
            i(&mut l1),
            a(&mut k2),
            a(&mut v2),
            i(&mut l2),
            a(&mut op),
            i(&mut nn),
            i(&mut rep),
            i(&mut dd),
            i(&mut cc),
            &mut sc as *mut f32 as *mut c_void,
        ];
        let f = m.get_function(KERNEL).map_err(|e| anyhow!("{e}"))?;
        // 캔들의 커널과 같은 기본 스트림에 띄우므로 앞뒤 계산과 순서가 맞습니다.
        unsafe { HipModule::launch(f, (n as u32, hkv as u32, 1), (d as u32, 1, 1), 0, &mut args) }.map_err(|e| anyhow!("{e}"))?;
        Ok(out)
    }

    /// 실제 헤드 구성으로 [캐시 두 구간 + 새 토큰] 주의를 CPU 기준(`attention::attend`)과 견줍니다.
    fn probe(device: &Device, m: &HipModule, hkv: usize, rep: usize, d: usize) -> Result<f32> {
        let cpu = Device::Cpu;
        let mut worst = 0f32;
        for (n, causal, seed) in [(1usize, true, 1u64), (5, true, 2), (7, false, 3)] {
            let t = |shape: &[usize], s: u64| Tensor::from_vec(verify::det_normal(shape.iter().product(), 0x7B00 + s), shape.to_vec(), &cpu);
            let (lb, lt) = (300usize, 17usize);
            let q = (t(&[hkv * rep, n, d], seed)? * 2.0)?;
            let (kb, vb) = (t(&[hkv, d, lb], seed + 10)?, t(&[hkv, lb, d], seed + 20)?);
            let (ktl, vtl) = (t(&[hkv, d, lt], seed + 30)?, t(&[hkv, lt, d], seed + 40)?);
            let (kn, vn) = (t(&[hkv, d, n], seed + 50)?, t(&[hkv, n, d], seed + 60)?);
            let scale = 1.0 / (d as f64).sqrt();
            let mode = if causal { AttnMode::Causal } else { AttnMode::Block };
            let want = reference(&q, &kn, &vn, &[(&kb, &vb), (&ktl, &vtl)], &StepMask::new(mode, n, rep, &cpu)?, scale)?;
            let on = |x: &Tensor| x.to_device(device);
            let (gq, gkb, gvb, gkt, gvt, gkn, gvn) = (on(&q)?, on(&kb)?, on(&vb)?, on(&ktl)?, on(&vtl)?, on(&kn)?, on(&vn)?);
            let got = launch(m, &gq, &gkn, &gvn, &[(&gkb, &gvb), (&gkt, &gvt)], causal, scale)?.to_device(&cpu)?;
            worst = worst.max(verify::rel_err(&got, &want)?);
        }
        Ok(worst)
    }

    /// 시험용: 이 헤드 구성의 자가 검증 오차 (커널을 올릴 수 없으면 None)
    #[cfg(test)]
    pub fn probe_error(device: &Device, hkv: usize, rep: usize, d: usize) -> Result<Option<f32>> {
        match module() {
            Some(m) => Ok(Some(probe(device, m, hkv, rep, d)?)),
            None => Ok(None),
        }
    }

    pub fn attend(q: &Tensor, kt: &Tensor, v: &Tensor, past: &[(&Tensor, &Tensor)], causal: bool, scale: f64) -> Result<Option<Tensor>> {
        let device = q.device();
        if !matches!(device, Device::Rocm(_)) || !super::enabled() || !fits(q, kt, v, past)? {
            return Ok(None);
        }
        let Some(m) = module() else { return Ok(None) };
        let (h, _, d) = q.dims3()?;
        let hkv = kt.dim(0)?;
        let op = format!("fused_attend_d{d}_h{h}_kv{hkv}");
        if !verify::trusted(device, &op, TOL, || probe(device, m, hkv, h / hkv, d)) {
            return Ok(None);
        }
        Ok(Some(launch(m, q, kt, v, past, causal, scale)?))
    }
}

#[cfg(all(test, feature = "rocm"))]
mod tests {
    use candle_core::Device;

    /// gocr (KV 헤드 8 · 질의 헤드 2 개씩 · 128) 와 Zoro-1 전체 주의 (4 · 4 개씩 · 256) 구성에서 CPU 기준과 같은가 (GPU 필요)
    #[test]
    #[ignore = "ROCm GPU 가 필요합니다"]
    fn the_kernel_matches_the_reference_attention() -> anyhow::Result<()> {
        let device = Device::new_rocm(0)?;
        for (hkv, rep, d) in [(8, 2, 128), (4, 4, 256), (2, 1, 64)] {
            let err = super::rocm::probe_error(&device, hkv, rep, d)?.expect("커널 이미지");
            assert!(err < 1e-4, "kv {hkv} · rep {rep} · d {d}: {err}");
        }
        Ok(())
    }
}
