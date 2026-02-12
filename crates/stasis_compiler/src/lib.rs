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

    #[cfg(windows)]
    fn run_cranelift_helper(source: &Path) -> Output {
        Command::new("cmd")
            .arg("/C")
            .arg(cranelift_run_helper_path())
            .arg(source)
            .env("STASIS_DISABLE_ARTIFACT_CACHE", "1")
            .current_dir(repo_root())
            .output()
            .expect("failed to execute cranelift run helper")
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
    fn bootstrap_reports_unknown_function_binding() {
        let source = fixture_path("invalid_unknown_function_binding.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        let text = combined_output_text(&output);
        assert!(
            !output.status.success(),
            "expected binding failure for {}, but compile succeeded.\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("Unknown function"),
            "expected unknown-function diagnostic for {}\n{}",
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

        let output = run_cranelift_helper(&source);

        assert!(
            output.status.success(),
            "cranelift run helper failed for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_returns_main_status_code() {
        let source = fixture_path("run_main_returns_7.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let code = output.status.code();
        assert_eq!(
            code,
            Some(7),
            "expected exit code 7 for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_print_i32_and_print_string_output() {
        let source = fixture_path("run_print_i32_and_string.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert!(
            output.status.success(),
            "expected success for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("S3_PRINT_START:"),
            "missing print_string prefix in output for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("42"),
            "missing print_i32 value in output for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains(":S3_PRINT_END"),
            "missing print_string suffix in output for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_print_ascii_and_utf8_output() {
        let source = fixture_path("run_print_ascii_utf8.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert!(
            output.status.success(),
            "expected success for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("ASCII_MSG"),
            "missing ASCII output for {}\n{}",
            source.display(),
            text
        );
        assert!(
            text.contains("UTF8_MSG"),
            "missing UTF8 output for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_parser_s4_valid_control_flow_fixture() {
        let source = fixture_path("parser_s4_valid_control_flow.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        assert!(
            output.status.success(),
            "expected compile success for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_reports_parser_error_for_s4_invalid_let_fixture() {
        let source = fixture_path("parser_s4_invalid_let_missing_init_or_type.stasis");
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
            text.contains("Local variables must declare a type")
                || text.contains("let name: type = value")
                || text.contains("Expected '='"),
            "expected let-declaration diagnostic for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_parser_s4_counts_fixture() {
        let source = fixture_path("run_parser_s4_counts.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "expected exit code 0 for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_parser_invalid_let_missing_init_or_type_fixture() {
        let source = fixture_path("run_parser_invalid_let_missing_init_or_type.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(2),
            "expected parse error exit code 2 for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_parser_s4_precedence_fixture() {
        let source = fixture_path("run_parser_s4_precedence.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "expected precedence parse success for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_parser_s4_loops_counts_fixture() {
        let source = fixture_path("run_parser_s4_loops_counts.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(0),
            "expected loop parse success for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_parser_invalid_for_missing_step_fixture() {
        let source = fixture_path("run_parser_invalid_for_missing_step.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(2),
            "expected parse error exit code 2 for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn cranelift_run_helper_enum_to_i32_conversion_fixture() {
        let source = fixture_path("run_enum_to_i32_conversion.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_cranelift_helper(&source);
        let text = combined_output_text(&output);
        assert_eq!(
            output.status.code(),
            Some(11),
            "expected enum_to_i32 conversion exit code 11 for {}\n{}",
            source.display(),
            text
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_entry_validation_ok_fixture() {
        let source = fixture_path("entry_validation_ok.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        assert!(
            output.status.success(),
            "expected compile success for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_entry_validation_missing_main_fixture() {
        let source = fixture_path("entry_validation_missing_main.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        assert!(
            output.status.success(),
            "expected compile success for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[cfg(windows)]
    #[test]
    fn bootstrap_compiles_entry_validation_invalid_main_signature_fixture() {
        let source = fixture_path("entry_validation_invalid_main_signature.stasis");
        assert!(source.exists(), "missing fixture {}", source.display());

        let output = run_bootstrap_emit_ir(&source);
        assert!(
            output.status.success(),
            "expected compile success for {}\nstdout:\n{}\nstderr:\n{}",
            source.display(),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
