use std::path::{Path, PathBuf};
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

impl RuntimeLauncher {
    pub fn new(source_file: PathBuf) -> Self {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
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
        if !cfg!(windows) {
            return Err("runtime execution path currently supports Windows only".to_string());
        }
        let stasis_exe = self
            .repo_root
            .join("target")
            .join("debug")
            .join("stasis.exe");
        if !stasis_exe.exists() {
            return Err(format!(
                "runtime launcher requires in-process runner binary at {}",
                stasis_exe.display()
            ));
        }
        let scenario = if self
            .source_file
            .to_string_lossy()
            .contains("brickout_revenge_v1")
        {
            "brickout-revenge-v1"
        } else {
            return Err(format!(
                "runtime launch scenario mapping is not defined for {}",
                self.source_file.display()
            ));
        };

        let mut command = Command::new(stasis_exe);
        command
            .arg("--scenario")
            .arg(scenario)
            .arg("--ticks")
            .arg("1000000")
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
        let launcher =
            RuntimeLauncher::new(PathBuf::from("samples/brickout_revenge/brickout_revenge_v1.stasis"));
        assert!(
            launcher
                .repo_root
                .to_string_lossy()
                .replace('\\', "/")
                .contains("/StasisLang")
        );
    }
}
