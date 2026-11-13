//! `rocm` 기능에서 `kernels/attend.hip`(디코딩 패스의 묶음 질의 주의, `ai::fused_attention`)을 HIP 코드 객체로 컴파일합니다.
//!
//! 도구 찾기는 candle-rocm-vulkan 의 `candle-core` 빌드 스크립트(Apache-2.0)와 같은 순서입니다: `HIPCC`, `ROCM_PATH` / `HIP_PATH`,
//! `PATH`. GPU 대상은 `HIP_ARCH` / `CANDLE_ROCM_ARCHS` (scripts/rocm-env.bat 이 gfx1032 로 둠), 없으면 `amdgpu-arch`,
//! 그래도 없으면 흔한 RDNA 대상들입니다. `rocm` 기능이 아니거나 `CANDLE_ROCM_SKIP_KERNEL_BUILD=1` 이면 빈 파일을 둡니다
//! (엔진은 일반 경로를 씀).

use std::path::{Path, PathBuf};
use std::process::Command;

const DEFAULT_ARCHS: &[&str] = &["gfx1030", "gfx1031", "gfx1032", "gfx1100", "gfx1101", "gfx1102", "gfx1200", "gfx1201"];

fn env_path(var: &str) -> Option<PathBuf> {
    let v = std::env::var(var).ok()?;
    let v = v.trim().trim_end_matches(['\\', '/']);
    (!v.is_empty()).then(|| PathBuf::from(v))
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn rocm_root() -> Option<PathBuf> {
    for var in ["ROCM_PATH", "HIP_PATH"] {
        if let Some(p) = env_path(var).filter(|p| p.is_dir()) {
            return Some(p);
        }
    }
    let fixed = PathBuf::from(if cfg!(windows) { "C:/Program Files/AMD/ROCm" } else { "/opt/rocm" });
    if !fixed.is_dir() {
        return None;
    }
    if cfg!(windows) {
        // 버전 이름 폴더 중 가장 새것
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(&fixed).ok()?.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
        dirs.sort();
        return dirs.pop();
    }
    Some(fixed)
}

fn hipcc(root: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = env_path("HIPCC") {
        return Some(p);
    }
    if let Some(root) = root {
        for name in ["hipcc", "hipcc.bin"] {
            let p = root.join("bin").join(exe(name));
            if p.is_file() {
                return Some(p);
            }
        }
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|d| d.join(exe("hipcc"))).find(|p| p.is_file())
}

fn archs(root: Option<&Path>) -> Vec<String> {
    for var in ["HIP_ARCH", "CANDLE_ROCM_ARCHS"] {
        if let Ok(v) = std::env::var(var) {
            let list: Vec<String> = v.split([',', ';', ' ']).map(|s| s.trim().to_string()).filter(|s| s.starts_with("gfx")).collect();
            if !list.is_empty() {
                return list;
            }
        }
    }
    if let Some(root) = root {
        for dir in [root.join("bin"), root.join("llvm").join("bin"), root.join("lib").join("llvm").join("bin")] {
            let tool = dir.join(exe("amdgpu-arch"));
            if !tool.is_file() {
                continue;
            }
            if let Ok(out) = Command::new(&tool).env("PATH", root.join("bin")).output() {
                let mut found: Vec<String> = Vec::new();
                for tok in String::from_utf8_lossy(&out.stdout).split_whitespace() {
                    let base = tok.split(':').next().unwrap_or(tok).to_string();
                    if base.starts_with("gfx") && base != "gfx000" && !found.contains(&base) {
                        found.push(base);
                    }
                }
                if !found.is_empty() {
                    return found;
                }
            }
        }
    }
    DEFAULT_ARCHS.iter().map(|s| s.to_string()).collect()
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=kernels/attend.hip");
    for var in ["HIPCC", "ROCM_PATH", "HIP_PATH", "HIP_ARCH", "CANDLE_ROCM_ARCHS", "CANDLE_ROCM_SKIP_KERNEL_BUILD"] {
        println!("cargo:rerun-if-env-changed={var}");
    }
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("attend.co");
    // 포크와 같은 약속: `CANDLE_ROCM_SKIP_KERNEL_BUILD=1` (hipcc 없는 CI 의 타입 검사)이면 빈 이미지를 둡니다.
    let skip = std::env::var("CANDLE_ROCM_SKIP_KERNEL_BUILD").is_ok_and(|v| v == "1");
    if std::env::var_os("CARGO_FEATURE_ROCM").is_none() || skip {
        std::fs::write(&out, b"").unwrap();
        println!("cargo:rustc-env=BLUEPRINT_HIP_ARCHS=none");
        return;
    }
    let root = rocm_root();
    let Some(cc) = hipcc(root.as_deref()) else {
        panic!("hipcc 를 찾을 수 없습니다: HIPCC, ROCM_PATH 또는 HIP_PATH 를 정하세요 (rocm 기능은 kernels/attend.hip 을 컴파일함)");
    };
    let archs = archs(root.as_deref());
    let mut cmd = Command::new(&cc);
    cmd.arg("--genco");
    for a in &archs {
        cmd.arg(format!("--offload-arch={a}"));
    }
    cmd.args(["-O3", "-std=c++17", "-o"]).arg(&out).arg("kernels/attend.hip");
    let res = cmd.output().unwrap_or_else(|e| panic!("{cc:?} 를 실행할 수 없습니다: {e}"));
    if !res.status.success() {
        panic!("{cc:?} 실패:\n{}\n{}", String::from_utf8_lossy(&res.stdout), String::from_utf8_lossy(&res.stderr));
    }
    println!("cargo:rustc-env=BLUEPRINT_HIP_ARCHS={}", archs.join(","));
}
