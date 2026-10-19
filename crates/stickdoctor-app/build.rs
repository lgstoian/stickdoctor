//! Embeds the Windows icon, version info and manifest when a resource compiler
//! is available. Resolves `rc.exe` from PATH first, then from the Windows SDK
//! (including the versioned x64 directory), so a plain `cargo build` on a
//! machine with Visual Studio Build Tools works without `vcvars`.

use std::env;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap());
    let rc_file = manifest_dir.join("assets").join("app.rc");
    println!("cargo:rerun-if-changed={}", rc_file.display());
    println!("cargo:rerun-if-changed=assets/stickdoctor.ico");
    println!("cargo:rerun-if-changed=assets/stickdoctor.manifest");

    if !rc_file.exists() {
        println!("cargo:warning=assets/app.rc not found; skipping resources");
        return;
    }

    let out_dir = PathBuf::from(env::var("OUT_DIR").unwrap());
    let target_env = env::var("CARGO_CFG_TARGET_ENV").unwrap_or_default();

    if target_env == "msvc" {
        if let Some(rc) = find_rc() {
            compile_msvc(&rc, &rc_file, &out_dir);
            return;
        }
    }

    if let Some(windres) = find_tool("windres.exe").or_else(|| find_tool("windres")) {
        compile_gnu(&windres, &rc_file, &out_dir);
        return;
    }

    println!(
        "cargo:warning=no resource compiler (rc.exe / windres) found; \
         building without icon/version resources"
    );
}

fn compile_msvc(rc: &Path, rc_file: &Path, out_dir: &Path) {
    let res = out_dir.join("stickdoctor.res");
    let status = Command::new(rc)
        .arg("/nologo")
        .arg(format!("/fo{}", res.display()))
        .arg(rc_file)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg={}", res.display());
            println!(
                "cargo:warning=embedded Windows resources via {}",
                rc.display()
            );
        }
        other => println!(
            "cargo:warning=rc.exe failed ({:?}); building without resources",
            other
        ),
    }
}

fn compile_gnu(windres: &Path, rc_file: &Path, out_dir: &Path) {
    let obj = out_dir.join("stickdoctor_res.o");
    let status = Command::new(windres)
        .arg(rc_file)
        .arg("-O")
        .arg("coff")
        .arg("-o")
        .arg(&obj)
        .status();
    match status {
        Ok(s) if s.success() => {
            println!("cargo:rustc-link-arg={}", obj.display());
            println!("cargo:warning=embedded Windows resources via windres");
        }
        other => println!(
            "cargo:warning=windres failed ({:?}); building without resources",
            other
        ),
    }
}

fn find_tool(name: &str) -> Option<PathBuf> {
    if let Ok(path) = env::var("PATH") {
        for dir in env::split_paths(&path) {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Locate `rc.exe`: PATH, then the Windows SDK versioned x64 directories.
fn find_rc() -> Option<PathBuf> {
    if let Some(p) = find_tool("rc.exe") {
        return Some(p);
    }
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = env::var("WindowsSdkDir") {
        roots.push(PathBuf::from(dir));
    }
    roots.push(PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10"));
    roots.push(PathBuf::from(r"C:\Program Files\Windows Kits\10"));

    for root in roots {
        let bin = root.join("bin");
        let entries = match std::fs::read_dir(&bin) {
            Ok(e) => e,
            Err(_) => continue,
        };
        let mut versions: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        versions.sort();
        // Newest SDK first.
        for version in versions.iter().rev() {
            for arch in ["x64", "x86"] {
                let candidate = version.join(arch).join("rc.exe");
                if candidate.is_file() {
                    return Some(candidate);
                }
            }
        }
    }
    None
}
