use std::path::PathBuf;
use std::process::{Child, Command, Stdio};

#[derive(Debug, Clone)]
pub struct RuntimeExecutionSummary {
    pub launches: u32,
    pub failures: u32,
    pub failure_reasons: Vec<String>,
}

pub struct RuntimeLauncher {
    repo_root: PathBuf,
    source_file: PathBuf,
    active_child: Option<Child>,
    summary: RuntimeExecutionSummary,
}

#[cfg(test)]
fn default_repo_root() -> PathBuf {
    use std::path::Path;
    // Tests run under Cargo, so `current_exe()` points at `target/.../deps/*` and won't
    // reliably infer the workspace root.
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..")
}

#[cfg(not(test))]
fn default_repo_root() -> PathBuf {
    // Best-effort runtime inference for non-test builds:
    // - If we're running from `.../target/{debug,release}/stasis[.exe]`, treat the parent of
    //   `target/` as repo root.
    // - Otherwise fall back to current working directory.
    let inferred = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.to_path_buf()))
        .and_then(|dir| {
            let name = dir.file_name()?.to_string_lossy().to_ascii_lowercase();
            let parent = dir.parent()?;
            let parent_name = parent.file_name()?.to_string_lossy().to_ascii_lowercase();
            if (name == "debug" || name == "release") && parent_name == "target" {
                return parent.parent().map(|root| root.to_path_buf());
            }
            None
        });
    inferred
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."))
}

impl RuntimeLauncher {
    pub fn new(source_file: PathBuf) -> Self {
        let repo_root = default_repo_root();
        Self {
            repo_root,
            source_file,
            active_child: None,
            summary: RuntimeExecutionSummary {
                launches: 0,
                failures: 0,
                failure_reasons: Vec::new(),
            },
        }
    }

    pub fn restart(&mut self) {
        if let Some(mut child) = self.active_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        match self.spawn_runtime_process() {
            Ok(child) => {
                self.active_child = Some(child);
                self.summary.launches += 1;
            }
            Err(reason) => {
                self.summary.failures += 1;
                self.summary.failure_reasons.push(reason);
            }
        }
    }

    pub fn summary(&self) -> &RuntimeExecutionSummary {
        &self.summary
    }

    fn spawn_runtime_process(&self) -> Result<Child, String> {
        let stasis_exe = self
            .repo_root
            .join("target")
            .join("debug")
            .join(if cfg!(windows) {
                "stasis.exe"
            } else {
                "stasis"
            });
        if !stasis_exe.exists() {
            return Err(format!(
                "runtime launcher requires in-process runner binary at {}",
                stasis_exe.display()
            ));
        }

        let mut command = Command::new(stasis_exe);
        // Always launch the generic play runner (no sample-specific scenarios).
        command
            .arg("play")
            .arg("--watch-file")
            .arg(&self.source_file);
        command
            .arg("--tick-sleep-us")
            .arg("16000")
            .arg("--no-runtime-launch")
            .current_dir(&self.repo_root)
            .stdin(Stdio::null())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit());

        command
            .spawn()
            .map_err(|error| format!("failed to launch runtime process: {error}"))
    }
}

impl Drop for RuntimeLauncher {
    fn drop(&mut self) {
        if let Some(mut child) = self.active_child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launcher_keeps_runner_repo_root() {
        let launcher = RuntimeLauncher::new(PathBuf::from(
            "samples/brickout_revenge/brickout_revenge_v1.stasis",
        ));
        assert!(launcher
            .repo_root
            .to_string_lossy()
            .replace('\\', "/")
            .contains("/StasisLang"));
    }
}
