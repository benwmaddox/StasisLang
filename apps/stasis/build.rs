use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=STASIS_RUNTIME_DLL_PATH");

    if env::var_os("CARGO_CFG_WINDOWS").is_none() {
        return;
    }

    for candidate in runtime_dll_candidate_paths() {
        println!("cargo:rerun-if-changed={}", candidate.display());
        if let Some(parent) = candidate.parent() {
            println!("cargo:rerun-if-changed={}", parent.display());
        }
    }

    let Some(source) = runtime_dll_candidate_paths()
        .into_iter()
        .find(|candidate| candidate.exists())
    else {
        return;
    };

    let Some(output_dir) = cargo_profile_output_dir() else {
        println!(
            "cargo:warning=stasis build could not determine the cargo profile output dir; skipping runtime DLL staging"
        );
        return;
    };

    let destination = output_dir.join("stasis_graphics.dll");
    if let Err(error) = fs::create_dir_all(&output_dir) {
        println!(
            "cargo:warning=stasis build failed to create output dir {}: {error}",
            output_dir.display()
        );
        return;
    }

    if let Err(error) = fs::copy(&source, &destination) {
        println!(
            "cargo:warning=stasis build failed to stage {} to {}: {error}",
            source.display(),
            destination.display()
        );
        return;
    }
}

fn cargo_profile_output_dir() -> Option<PathBuf> {
    let out_dir = PathBuf::from(env::var_os("OUT_DIR")?);
    let profile = env::var("PROFILE").ok()?;
    out_dir
        .ancestors()
        .find(|ancestor| ancestor.file_name() == Some(OsStr::new(&profile)))
        .map(Path::to_path_buf)
}

fn runtime_dll_candidate_paths() -> Vec<PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));

    let mut candidates = Vec::new();
    if let Some(configured) = env::var_os("STASIS_RUNTIME_DLL_PATH") {
        candidates.push(PathBuf::from(configured));
    }
    candidates.push(repo_root.join("stasis_graphics.dll"));
    candidates.push(repo_root.join("build").join("stasis_graphics.dll"));
    candidates.push(
        repo_root
            .join("runtime")
            .join("build")
            .join("bin")
            .join("Release")
            .join("stasis_graphics.dll"),
    );
    candidates.push(
        repo_root
            .join("runtime")
            .join("build")
            .join("bin")
            .join("Debug")
            .join("stasis_graphics.dll"),
    );
    candidates.push(
        repo_root
            .join("runtime")
            .join("build_ci")
            .join("bin")
            .join("Release")
            .join("stasis_graphics.dll"),
    );
    candidates
}
