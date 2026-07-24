//! Compile the SumpHash CUDA kernel to PTX at build time using nvcc, if a
//! CUDA toolkit is available. If not, emit an empty PTX so the crate still
//! builds (GPU mining then reports unavailable at runtime and callers fall
//! back to the CPU miner).

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn find_nvcc() -> Option<PathBuf> {
    // 1. CUDA_PATH/bin/nvcc
    if let Ok(cuda) = env::var("CUDA_PATH") {
        let p = Path::new(&cuda).join("bin").join("nvcc.exe");
        if p.exists() {
            return Some(p);
        }
        let p = Path::new(&cuda).join("bin").join("nvcc");
        if p.exists() {
            return Some(p);
        }
    }
    // 2. default Windows install location, newest version
    let base = Path::new("C:/Program Files/NVIDIA GPU Computing Toolkit/CUDA");
    if base.exists() {
        let mut versions: Vec<PathBuf> = std::fs::read_dir(base)
            .into_iter()
            .flatten()
            .flatten()
            .map(|e| e.path())
            .collect();
        versions.sort();
        if let Some(v) = versions.last() {
            let p = v.join("bin").join("nvcc.exe");
            if p.exists() {
                return Some(p);
            }
        }
    }
    // 3. PATH
    let exe = if cfg!(windows) { "nvcc.exe" } else { "nvcc" };
    if Command::new(exe).arg("--version").output().is_ok() {
        return Some(PathBuf::from(exe));
    }
    None
}

/// Locate the MSVC Host x64 bin directory (containing cl.exe) via vswhere.
fn find_msvc_bin() -> Option<String> {
    if !cfg!(windows) {
        return None;
    }
    let vswhere =
        "C:/Program Files (x86)/Microsoft Visual Studio/Installer/vswhere.exe";
    if !Path::new(vswhere).exists() {
        return None;
    }
    let out = Command::new(vswhere)
        .args([
            "-latest",
            "-products",
            "*",
            "-requires",
            "Microsoft.VisualStudio.Component.VC.Tools.x86.x64",
            "-property",
            "installationPath",
        ])
        .output()
        .ok()?;
    let root = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if root.is_empty() {
        return None;
    }
    let msvc = Path::new(&root).join("VC/Tools/MSVC");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(&msvc)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .collect();
    versions.sort();
    let v = versions.last()?;
    let bin = v.join("bin/Hostx64/x64");
    if bin.join("cl.exe").exists() {
        Some(bin.to_string_lossy().into_owned())
    } else {
        None
    }
}

/// Rewrite the leading `.version X.Y` PTX directive to `target`.
fn clamp_ptx_version(ptx: &str, target: &str) -> String {
    ptx.lines()
        .map(|line| {
            let t = line.trim_start();
            if t.starts_with(".version ") {
                format!(".version {target}")
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn main() {
    println!("cargo:rerun-if-changed=src/sumphash.cu");
    println!("cargo:rerun-if-env-changed=CUDA_PATH");
    let out_dir = env::var("OUT_DIR").unwrap();
    let ptx_path = Path::new(&out_dir).join("sumphash.ptx");

    match find_nvcc() {
        Some(nvcc) => {
            // compute_75 PTX JITs forward onto any newer GPU (incl. Blackwell)
            let mut cmd = Command::new(&nvcc);
            cmd.args([
                "--ptx",
                "-arch=compute_75",
                "-o",
                ptx_path.to_str().unwrap(),
                "src/sumphash.cu",
            ]);
            // nvcc needs the MSVC host compiler; point it at cl.exe explicitly
            // so the build does not depend on a VS developer shell.
            if let Some(cl_dir) = find_msvc_bin() {
                cmd.arg("-ccbin").arg(cl_dir);
            }
            let status = cmd.status().expect("failed to run nvcc");
            if !status.success() {
                panic!("nvcc failed to compile sumphash.cu");
            }
            // Newer toolkits emit a PTX ISA version (e.g. 9.3) that older
            // drivers reject with CUDA_ERROR_UNSUPPORTED_PTX_VERSION. Our
            // kernel uses only long-standing instructions, so clamp the
            // .version directive down to 7.8 (CUDA 11.8) for broad driver
            // compatibility. The driver JITs it onto whatever GPU is present.
            let ptx = std::fs::read_to_string(&ptx_path).unwrap();
            let clamped = clamp_ptx_version(&ptx, "7.8");
            std::fs::write(&ptx_path, clamped).unwrap();
            println!("cargo:rustc-cfg=sump_have_ptx");
            println!("cargo:warning=SumpHash CUDA kernel compiled to PTX (ISA clamped to 7.8)");
        }
        None => {
            // no toolkit: write an empty placeholder so include_str! works
            std::fs::write(&ptx_path, "").unwrap();
            println!(
                "cargo:warning=no CUDA toolkit (nvcc) found; GPU miner disabled, \
                 CPU fallback only"
            );
        }
    }
}
