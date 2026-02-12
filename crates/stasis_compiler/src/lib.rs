#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

/// Placeholder compiler substrate crate for Rewrite V1.
pub fn crate_ready() -> bool {
    true
}

pub fn incremental_compiler_source_path() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .join("compiler")
        .join("incremental_compiler.stasis")
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::incremental_compiler_source_path;

    #[test]
    fn crate_is_ready() {
        assert!(super::crate_ready());
    }

    #[test]
    fn incremental_compiler_source_exists() {
        let source = incremental_compiler_source_path();
        assert!(
            source.exists(),
            "missing incremental compiler source at {}",
            source.display()
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_incremental_compiler_source() {
        let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..");
        let bootstrap = repo_root
            .join("bootstrap")
            .join("windows")
            .join("stasisc.bat");
        assert!(
            bootstrap.exists(),
            "missing bootstrap compiler at {}",
            bootstrap.display()
        );

        let source = incremental_compiler_source_path();
        let output = Command::new("cmd")
            .arg("/C")
            .arg(&bootstrap)
            .arg("run")
            .arg(&source)
            .arg("--emit-ir")
            .current_dir(&repo_root)
            .output()
            .expect("failed to execute bootstrap compiler");

        assert!(
            output.status.success(),
            "bootstrap compile failed for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
