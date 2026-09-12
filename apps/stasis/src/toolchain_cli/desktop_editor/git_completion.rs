use sha2::{Digest, Sha256};
use stasis_ai::session_store::{
    CompletionPath, CompletionPathProvenance, TaskCompletionCommit, TaskGitBaseline,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct CompletionPlan {
    pub task_id: String,
    pub head: String,
    pub paths: Vec<CompletionPath>,
    expected_states: BTreeMap<String, String>,
}

#[cfg(all(test, target_os = "windows"))]
pub(super) fn evidence_plan(task_id: &str) -> CompletionPlan {
    CompletionPlan {
        task_id: task_id.into(),
        head: "0123456789012345678901234567890123456789".into(),
        paths: vec![
            CompletionPath {
                path: "src/main.stasis".into(),
                provenance: CompletionPathProvenance::StasisEdit,
            },
            CompletionPath {
                path: "assets/brown-maze-background.png".into(),
                provenance: CompletionPathProvenance::ExternalEdit,
            },
        ],
        expected_states: BTreeMap::new(),
    }
}

pub(super) fn capture_baseline(project_root: &Path) -> Result<Option<TaskGitBaseline>, String> {
    if !is_repository(project_root)? {
        return Ok(None);
    }
    let head = head(project_root)?;
    let dirty_paths = dirty_path_states(project_root)?;
    Ok(Some(TaskGitBaseline { head, dirty_paths }))
}

pub(super) fn plan(
    project_root: &Path,
    task_id: &str,
    baseline: &TaskGitBaseline,
    stasis_paths: &BTreeSet<String>,
) -> Result<CompletionPlan, String> {
    let current_head = head(project_root)?;
    if current_head != baseline.head {
        return Err("Git HEAD changed while this task was active. Review the repository and run focused tests again before completing it.".into());
    }
    let current = dirty_path_states(project_root)?;
    let mut ambiguous = Vec::new();
    let mut paths = Vec::new();
    let mut expected_states = BTreeMap::new();
    for (path, state) in current {
        match baseline.dirty_paths.get(&path) {
            Some(original) if original == &state => continue,
            Some(_) => ambiguous.push(path),
            None => {
                let provenance = if stasis_paths.contains(&path) {
                    CompletionPathProvenance::StasisEdit
                } else {
                    CompletionPathProvenance::ExternalEdit
                };
                expected_states.insert(path.clone(), state);
                paths.push(CompletionPath { path, provenance });
            }
        }
    }
    if !ambiguous.is_empty() {
        return Err(format!(
            "These paths were already dirty when the task started and changed again, so ownership is ambiguous: {}. Commit or restore them manually before completing the task.",
            ambiguous.join(", ")
        ));
    }
    Ok(CompletionPlan {
        task_id: task_id.to_string(),
        head: current_head,
        paths,
        expected_states,
    })
}

pub(super) fn commit(
    project_root: &Path,
    plan: &CompletionPlan,
    objective: &str,
) -> Result<TaskCompletionCommit, String> {
    if head(project_root)? != plan.head {
        return Err("Git HEAD changed after completion review; review the task again.".into());
    }
    let current = dirty_path_states(project_root)?;
    for (path, expected) in &plan.expected_states {
        if current.get(path) != Some(expected) {
            return Err(format!(
                "{path} changed after completion review; review the task again."
            ));
        }
    }

    let index_path = temporary_index_path(&plan.task_id);
    let mut read_tree = git(project_root);
    read_tree
        .env("GIT_INDEX_FILE", &index_path)
        .args(["read-tree", "HEAD"]);
    checked(read_tree, "initialize an isolated task index")?;

    let result = (|| {
        let path_args = plan.paths.iter().map(|value| value.path.as_str());
        let mut add = git(project_root);
        add.env("GIT_INDEX_FILE", &index_path)
            .arg("add")
            .arg("-A")
            .arg("--");
        add.args(path_args);
        checked(add, "stage reviewed task paths")?;

        let subject = format!("stasis: {}", bounded_subject(objective));
        let mut commit = git(project_root);
        commit
            .env("GIT_INDEX_FILE", &index_path)
            .args(["commit", "-m", &subject]);
        checked(commit, "commit reviewed task paths")?;
        let commit = head(project_root)?;

        let mut reset = git(project_root);
        reset.args(["reset", "-q", "HEAD", "--"]);
        reset.args(plan.paths.iter().map(|value| value.path.as_str()));
        checked(reset, "refresh the main Git index after task commit")?;

        Ok(TaskCompletionCommit {
            commit,
            paths: plan.paths.clone(),
            reverted_by: None,
        })
    })();
    let _ = std::fs::remove_file(index_path);
    result
}

pub(super) fn revert(project_root: &Path, commit: &str) -> Result<String, String> {
    if head(project_root)? != commit {
        return Err("The task commit is no longer Git HEAD, so Stasis cannot safely roll it back automatically.".into());
    }
    let mut command = git(project_root);
    command.args(["revert", "--no-edit", commit]);
    checked(command, "revert the completed task")?;
    head(project_root)
}

pub(super) fn receipt_paths(receipt: &serde_json::Value) -> BTreeSet<String> {
    receipt
        .pointer("/plan/changed_files")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| change.get("file").and_then(serde_json::Value::as_str))
        .filter_map(normalize_relative_path)
        .collect()
}

fn is_repository(project_root: &Path) -> Result<bool, String> {
    let output = git(project_root)
        .args(["rev-parse", "--is-inside-work-tree"])
        .output()
        .map_err(|error| format!("could not inspect Git repository: {error}"))?;
    Ok(output.status.success() && String::from_utf8_lossy(&output.stdout).trim() == "true")
}

fn head(project_root: &Path) -> Result<String, String> {
    let output = checked(
        {
            let mut command = git(project_root);
            command.args(["rev-parse", "HEAD"]);
            command
        },
        "read Git HEAD",
    )?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn dirty_path_states(project_root: &Path) -> Result<BTreeMap<String, String>, String> {
    let mut paths = BTreeSet::new();
    let tracked = checked(
        {
            let mut command = git(project_root);
            command.args(["diff", "--name-only", "-z", "HEAD", "--", "."]);
            command
        },
        "list changed project paths",
    )?;
    paths.extend(parse_nul_paths(&tracked.stdout)?);
    let untracked = checked(
        {
            let mut command = git(project_root);
            command.args([
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                ".",
            ]);
            command
        },
        "list untracked project paths",
    )?;
    paths.extend(parse_nul_paths(&untracked.stdout)?);

    paths
        .into_iter()
        .map(|path| {
            let state = path_state(project_root, &path)?;
            Ok((path, state))
        })
        .collect()
}

fn parse_nul_paths(bytes: &[u8]) -> Result<Vec<String>, String> {
    bytes
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| {
            std::str::from_utf8(path)
                .map_err(|_| "Git returned a path that is not valid UTF-8".to_string())
                .and_then(|path| {
                    normalize_relative_path(path)
                        .ok_or_else(|| format!("Git returned an unsafe project path: {path}"))
                })
        })
        .collect()
}

fn normalize_relative_path(path: &str) -> Option<String> {
    let normalized = path.replace('\\', "/");
    let parsed = Path::new(&normalized);
    if parsed.is_absolute()
        || parsed
            .components()
            .any(|part| matches!(part, std::path::Component::ParentDir))
    {
        return None;
    }
    Some(normalized)
}

fn path_state(project_root: &Path, relative: &str) -> Result<String, String> {
    let path = project_root.join(relative);
    if !path.exists() {
        return Ok("deleted".into());
    }
    let mut file = File::open(&path)
        .map_err(|error| format!("could not fingerprint {}: {error}", path.display()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("could not fingerprint {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        digest.update(&buffer[..read]);
    }
    Ok(format!("file:{:x}", digest.finalize()))
}

fn temporary_index_path(task_id: &str) -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "stasis-task-index-{}-{}-{nonce}",
        std::process::id(),
        task_id.replace(|character: char| !character.is_ascii_alphanumeric(), "_")
    ))
}

fn bounded_subject(objective: &str) -> String {
    let one_line = objective.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut subject = one_line.chars().take(72).collect::<String>();
    if one_line.chars().count() > 72 {
        subject.push_str("...");
    }
    subject
}

fn git(project_root: &Path) -> Command {
    let mut command = Command::new("git");
    command.current_dir(project_root);
    command
}

fn checked(mut command: Command, operation: &str) -> Result<Output, String> {
    let output = command
        .output()
        .map_err(|error| format!("could not {operation}: {error}"))?;
    if output.status.success() {
        Ok(output)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!("could not {operation}: {detail}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn repository(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "stasis-git-completion-{name}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir_all(&root).unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["init", "-q"]);
                command
            },
            "init",
        )
        .unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["config", "user.name", "Stasis Test"]);
                command
            },
            "config name",
        )
        .unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["config", "user.email", "stasis@example.invalid"]);
                command
            },
            "config email",
        )
        .unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["config", "commit.gpgsign", "false"]);
                command
            },
            "disable test signing",
        )
        .unwrap();
        fs::write(root.join("game.stasis"), "fn main() {}\n").unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["add", "."]);
                command
            },
            "add",
        )
        .unwrap();
        checked(
            {
                let mut command = git(&root);
                command.args(["commit", "-q", "-m", "initial"]);
                command
            },
            "commit",
        )
        .unwrap();
        root
    }

    #[test]
    fn classifies_stasis_and_external_files_and_commits_only_task_changes() {
        let root = repository("commit");
        fs::write(root.join("notes.txt"), "preexisting\n").unwrap();
        let baseline = capture_baseline(&root).unwrap().unwrap();
        fs::write(root.join("game.stasis"), "fn main() { draw() }\n").unwrap();
        fs::write(root.join("background.png"), b"external image").unwrap();
        let stasis_paths = BTreeSet::from(["game.stasis".to_string()]);
        let plan = plan(&root, "task-1", &baseline, &stasis_paths).unwrap();
        assert_eq!(plan.paths.len(), 2);
        assert_eq!(plan.paths[0].path, "background.png");
        assert_eq!(
            plan.paths[0].provenance,
            CompletionPathProvenance::ExternalEdit
        );
        assert_eq!(
            plan.paths[1].provenance,
            CompletionPathProvenance::StasisEdit
        );

        let receipt = commit(&root, &plan, "Improve the background").unwrap();
        assert_eq!(receipt.paths, plan.paths);
        let dirty = dirty_path_states(&root).unwrap();
        assert_eq!(dirty.keys().cloned().collect::<Vec<_>>(), vec!["notes.txt"]);
        assert!(String::from_utf8_lossy(
            &checked(
                {
                    let mut command = git(&root);
                    command.args(["show", "--format=", "--name-only", "HEAD"]);
                    command
                },
                "show"
            )
            .unwrap()
            .stdout
        )
        .contains("background.png"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_a_preexisting_path_changed_during_the_task() {
        let root = repository("ambiguous");
        fs::write(root.join("game.stasis"), "first dirty state\n").unwrap();
        let baseline = capture_baseline(&root).unwrap().unwrap();
        fs::write(root.join("game.stasis"), "second dirty state\n").unwrap();
        let error = plan(&root, "task-1", &baseline, &BTreeSet::new()).unwrap_err();
        assert!(error.contains("ownership is ambiguous"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn permits_explicit_completion_without_a_project_change() {
        let root = repository("no-change");
        let baseline = capture_baseline(&root).unwrap().unwrap();
        let plan = plan(&root, "task-1", &baseline, &BTreeSet::new()).unwrap();
        assert!(plan.paths.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reverts_only_when_task_commit_is_still_head() {
        let root = repository("revert");
        let baseline = capture_baseline(&root).unwrap().unwrap();
        fs::write(root.join("image.png"), b"pixels").unwrap();
        let plan = plan(&root, "task-1", &baseline, &BTreeSet::new()).unwrap();
        let receipt = commit(&root, &plan, "Add image").unwrap();
        let reverted = revert(&root, &receipt.commit).unwrap();
        assert_ne!(reverted, receipt.commit);
        assert!(!root.join("image.png").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
