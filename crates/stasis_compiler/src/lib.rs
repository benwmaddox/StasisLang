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
    use std::path::{Path, PathBuf};
    use std::process::{Command, Output};

    use super::incremental_compiler_source_path;

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
    }

    fn fixture_path(name: &str) -> PathBuf {
        repo_root().join("tests").join("stasis").join(name)
    }

    #[cfg(windows)]
    fn cranelift_run_helper_path() -> PathBuf {
        repo_root()
            .join("bootstrap")
            .join("windows")
            .join("stasis-cranelift-run.bat")
    }

    #[cfg(windows)]
    fn bootstrap_path() -> PathBuf {
        repo_root()
            .join("bootstrap")
            .join("windows")
            .join("stasisc.bat")
    }

    #[cfg(windows)]
    fn run_bootstrap_emit_ir(source: &Path) -> Output {
        Command::new("cmd")
            .arg("/C")
            .arg(bootstrap_path())
            .arg("run")
            .arg(source)
            .arg("--emit-ir")
            .current_dir(repo_root())
            .output()
            .expect("failed to execute bootstrap compiler")
    }

    #[cfg(windows)]
    fn combined_output_text(output: &Output) -> String {
        format!(
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        )
    }

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
        let bootstrap = bootstrap_path();
        assert!(
            bootstrap.exists(),
            "missing bootstrap compiler at {}",
            bootstrap.display()
        );

        let source = incremental_compiler_source_path();
        let output = run_bootstrap_emit_ir(&source);
        let text = combined_output_text(&output);

        assert!(
            output.status.success(),
            "bootstrap compile failed for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            text.contains("Total time="),
            "expected compile output summary for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_parser_valid_fixture() {
        let source = fixture_path("parser_valid_main.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        assert!(
            output.status.success(),
            "expected parse success for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_reports_parser_error_for_invalid_fixture() {
        let source = fixture_path("parser_invalid_missing_semicolon.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        let text = combined_output_text(&output);
        assert!(
            !output.status.success(),
            "expected parse failure for {}, but compile succeeded.\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("error:"),
            "expected diagnostic output for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("Expected ';' after expression.")
                || text.contains("Expected ';'")
                || text.contains("Unexpected token"),
            "expected parse-semicolon style diagnostic for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_executes_minimal_program() {
        let helper = cranelift_run_helper_path();
        assert!(helper.exists(), "missing helper script {}", helper.display());

        let source = repo_root()
            .join("bootstrap")
            .join("smoke")
            .join("minimal.stasis");
        assert!(source.exists(), "missing smoke file {}", source.display());

        let output = Command::new("cmd")
            .arg("/C")
            .arg(&helper)
            .arg(&source)
            .current_dir(repo_root())
            .output()
            .expect("failed to execute cranelift run helper");

        assert!(
            output.status.success(),
            "cranelift run helper failed for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
