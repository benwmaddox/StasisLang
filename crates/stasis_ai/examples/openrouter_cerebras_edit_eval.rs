//! Explicitly paid and mutating integration evaluation. This launches the real
//! Stasis workspace AI command so the normal host executor owns the edit and
//! test gate. It is inert unless every opt-in variable is supplied.

use std::path::PathBuf;
use std::process::Command;

fn main() -> Result<(), String> {
    if std::env::var("STASIS_RUN_OPENROUTER_EDIT_EVAL").as_deref() != Ok("1") {
        println!("skipped: set STASIS_RUN_OPENROUTER_EDIT_EVAL=1 to opt in");
        return Ok(());
    }
    std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| "OPENROUTER_API_KEY is required for the edit evaluation".to_string())?;
    let project = PathBuf::from(
        std::env::var("STASIS_OPENROUTER_EDIT_EVAL_PROJECT").map_err(|_| {
            "STASIS_OPENROUTER_EDIT_EVAL_PROJECT must name a disposable valid Stasis workspace"
                .to_string()
        })?,
    );
    if !project.join("stasis.toml").is_file() {
        return Err("edit evaluation project must contain stasis.toml".to_string());
    }
    let executable = std::env::var_os("STASIS_EVAL_STASIS_EXE")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("stasis"));
    let prompt = "Credentialed provider evaluation. Make exactly one bounded tested edit: add a new top-level function openrouter_eval_marker(): i32 returning 423 in the entry source. Inspect the target and references first. Do not modify any existing function or asset. Finish only after the host returns a successful atomic write receipt with its compile and test gate passed.";
    let output = Command::new(executable)
        .current_dir(&project)
        .env("STASIS_AI_PROVIDER", "openrouter")
        .env("STASIS_AI_MODEL", "openai/gpt-oss-120b")
        .env("STASIS_AI_ROUTE_ONLY", "cerebras")
        .env("STASIS_AI_ROUTE_ORDER", "cerebras")
        .env("STASIS_AI_ALLOW_FALLBACKS", "false")
        .args(["ai", prompt])
        .output()
        .map_err(|error| format!("failed launching Stasis edit evaluation: {error}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "Stasis edit evaluation failed with {}: {}",
            output.status,
            stderr.chars().take(500).collect::<String>()
        ));
    }
    println!("bounded tested edit completed through the Stasis host executor");
    println!("project: {}", project.display());
    Ok(())
}
