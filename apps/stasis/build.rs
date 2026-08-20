use std::env;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rerun-if-env-changed=STASIS_RUNTIME_LIBRARY_PATH");
    println!("cargo:rerun-if-env-changed=STASIS_RUNTIME_DLL_PATH");
    println!("cargo:rerun-if-env-changed=STASIS_RELEASE_ID");
    println!("cargo:rerun-if-env-changed=STASIS_SOURCE_COMMIT");
    println!("cargo:rerun-if-env-changed=STASIS_BUILD_TARGET");

    for candidate in runtime_library_candidate_paths() {
        println!("cargo:rerun-if-changed={}", candidate.display());
        if let Some(parent) = candidate.parent() {
            println!("cargo:rerun-if-changed={}", parent.display());
        }
    }

    let Some(source) = runtime_library_candidate_paths()
        .into_iter()
        .find(|candidate| candidate.exists())
    else {
        return;
    };

    let Some(output_dir) = cargo_profile_output_dir() else {
        println!(
            "cargo:warning=stasis build could not determine the cargo profile output dir; skipping runtime library staging"
        );
        return;
    };

    let Some(file_name) = source.file_name() else {
        println!(
            "cargo:warning=stasis build could not determine the runtime library name for {}",
            source.display()
        );
        return;
    };
    let destination = output_dir.join(file_name);
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

fn runtime_library_candidate_paths() -> Vec<PathBuf> {
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join(".."));

    let configured = [
        env::var_os("STASIS_RUNTIME_LIBRARY_PATH"),
        env::var_os("STASIS_RUNTIME_DLL_PATH"),
    ]
    .into_iter()
    .flatten()
    .map(PathBuf::from);
    runtime_library_candidate_paths_for(&repo_root, configured, runtime_library_file_names())
}

/// Build staging prefers runtime binaries from the current checkout over legacy copies.
///
/// Keep explicit environment paths first: they are an intentional override for CI and
/// platform-specific workflows. The runtime build outputs follow in release, unconfigured
/// (the native build's bin root), and debug order for both regular and CI build trees. Copies
/// beside the repository root or in its legacy `build` directory are last-resort fallbacks.
pub fn runtime_library_candidate_paths_for(
    repo_root: &Path,
    configured: impl IntoIterator<Item = PathBuf>,
    file_names: &[&str],
) -> Vec<PathBuf> {
    let mut candidates = configured.into_iter().collect::<Vec<_>>();
    for build_dir in ["build", "build_ci"] {
        for configuration in [Some("Release"), None, Some("Debug")] {
            for file_name in file_names {
                let mut candidate = repo_root.join("runtime").join(build_dir).join("bin");
                if let Some(configuration) = configuration {
                    candidate.push(configuration);
                }
                candidate.push(file_name);
                candidates.push(candidate);
            }
        }
    }
    for file_name in file_names {
        candidates.push(repo_root.join(file_name));
        candidates.push(repo_root.join("build").join(file_name));
    }
    candidates
}

fn runtime_library_file_names() -> &'static [&'static str] {
    if env::var_os("CARGO_CFG_WINDOWS").is_some() {
        &["stasis_graphics.dll"]
    } else if env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        &["libstasis_graphics.dylib", "stasis_graphics.dylib"]
    } else {
        &["libstasis_graphics.so", "stasis_graphics.so"]
    }
}
