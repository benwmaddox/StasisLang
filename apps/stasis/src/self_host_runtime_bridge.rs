use std::ffi::OsString;
use std::path::Path;
use std::sync::{Mutex, OnceLock};

const ARG_COUNT_KEY: &str = "STASIS_SELF_HOST_ARG_COUNT";
const ARG_PREFIX: &str = "STASIS_SELF_HOST_ARG_";
const SUMMARY_KEY: &str = "STASIS_AOT_SUMMARY_FILE";

const SOURCE_COUNT_KEY: &str = "STASIS_SELF_HOST_SOURCE_FILE_COUNT";
const SOURCE_PATH_PREFIX: &str = "STASIS_SELF_HOST_SOURCE_PATH_";
const SOURCE_TEXT_PREFIX: &str = "STASIS_SELF_HOST_SOURCE_TEXT_";

const IR_BUNDLE_KEY: &str = "STASIS_SELF_HOST_IR_BUNDLE_PATH";
const OBJECT_BUNDLE_KEY: &str = "STASIS_SELF_HOST_OBJECT_BUNDLE_PATH";
const LINK_TEMPLATE_EXE_KEY: &str = "STASIS_SELF_HOST_LINK_TEMPLATE_EXE";
const SUMMARY_TEMPLATE_FILE_KEY: &str = "STASIS_SELF_HOST_SUMMARY_TEMPLATE_FILE";

#[derive(Debug, Clone)]
pub struct CliArgsEnvSnapshot {
    saved: Vec<(String, Option<OsString>)>,
}

#[derive(Debug, Clone)]
pub struct SourceFilesEnvSnapshot {
    saved: Vec<(String, Option<OsString>)>,
}

#[derive(Debug, Clone)]
pub struct StagedBridgePathsEnvSnapshot {
    saved: Vec<(String, Option<OsString>)>,
}

pub fn stasis_process_env_lock() -> &'static Mutex<()> {
    static PROCESS_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    PROCESS_ENV_LOCK.get_or_init(|| Mutex::new(()))
}

pub fn publish_cli_args_to_env(
    args: &[String],
    summary_file: Option<&Path>,
) -> CliArgsEnvSnapshot {
    let mut keys = vec![ARG_COUNT_KEY.to_string(), SUMMARY_KEY.to_string()];
    keys.extend(collect_indexed_env_keys(ARG_PREFIX));
    let saved = capture_env_values(&keys);

    clear_indexed_env_keys(ARG_PREFIX);
    std::env::set_var(ARG_COUNT_KEY, args.len().to_string());
    for (index, value) in args.iter().enumerate() {
        std::env::set_var(format!("{ARG_PREFIX}{index}"), value);
    }
    if let Some(path) = summary_file {
        std::env::set_var(SUMMARY_KEY, path);
    } else {
        std::env::remove_var(SUMMARY_KEY);
    }

    CliArgsEnvSnapshot { saved }
}

pub fn restore_cli_args_env(snapshot: CliArgsEnvSnapshot) {
    clear_indexed_env_keys(ARG_PREFIX);
    std::env::remove_var(ARG_COUNT_KEY);
    std::env::remove_var(SUMMARY_KEY);
    restore_env_values(snapshot.saved);
}

pub fn publish_source_files_to_env(source_payload: &[(String, String)]) -> SourceFilesEnvSnapshot {
    let mut keys = vec![SOURCE_COUNT_KEY.to_string()];
    keys.extend(collect_indexed_env_keys(SOURCE_PATH_PREFIX));
    keys.extend(collect_indexed_env_keys(SOURCE_TEXT_PREFIX));
    let saved = capture_env_values(&keys);

    clear_indexed_env_keys(SOURCE_PATH_PREFIX);
    clear_indexed_env_keys(SOURCE_TEXT_PREFIX);
    std::env::set_var(SOURCE_COUNT_KEY, source_payload.len().to_string());
    for (index, (path, source)) in source_payload.iter().enumerate() {
        std::env::set_var(format!("{SOURCE_PATH_PREFIX}{index}"), path);
        std::env::set_var(format!("{SOURCE_TEXT_PREFIX}{index}"), source);
    }

    SourceFilesEnvSnapshot { saved }
}

pub fn restore_source_files_env(snapshot: SourceFilesEnvSnapshot) {
    clear_indexed_env_keys(SOURCE_PATH_PREFIX);
    clear_indexed_env_keys(SOURCE_TEXT_PREFIX);
    std::env::remove_var(SOURCE_COUNT_KEY);
    restore_env_values(snapshot.saved);
}

pub fn publish_staged_bridge_paths_to_env(
    ir_bundle_path: &Path,
    object_bundle_path: &Path,
    link_template_exe: &Path,
    summary_template_file: Option<&Path>,
) -> StagedBridgePathsEnvSnapshot {
    let keys = vec![
        IR_BUNDLE_KEY.to_string(),
        OBJECT_BUNDLE_KEY.to_string(),
        LINK_TEMPLATE_EXE_KEY.to_string(),
        SUMMARY_TEMPLATE_FILE_KEY.to_string(),
    ];
    let saved = capture_env_values(&keys);

    std::env::set_var(IR_BUNDLE_KEY, ir_bundle_path);
    std::env::set_var(OBJECT_BUNDLE_KEY, object_bundle_path);
    std::env::set_var(LINK_TEMPLATE_EXE_KEY, link_template_exe);
    if let Some(path) = summary_template_file {
        std::env::set_var(SUMMARY_TEMPLATE_FILE_KEY, path);
    } else {
        std::env::remove_var(SUMMARY_TEMPLATE_FILE_KEY);
    }

    StagedBridgePathsEnvSnapshot { saved }
}

pub fn restore_staged_bridge_paths_env(snapshot: StagedBridgePathsEnvSnapshot) {
    std::env::remove_var(IR_BUNDLE_KEY);
    std::env::remove_var(OBJECT_BUNDLE_KEY);
    std::env::remove_var(LINK_TEMPLATE_EXE_KEY);
    std::env::remove_var(SUMMARY_TEMPLATE_FILE_KEY);
    restore_env_values(snapshot.saved);
}

fn capture_env_values(keys: &[String]) -> Vec<(String, Option<OsString>)> {
    keys.iter()
        .map(|key| (key.clone(), std::env::var_os(key)))
        .collect()
}

fn restore_env_values(saved: Vec<(String, Option<OsString>)>) {
    for (key, value) in saved {
        if let Some(value) = value {
            std::env::set_var(key, value);
        } else {
            std::env::remove_var(key);
        }
    }
}

fn clear_indexed_env_keys(prefix: &str) {
    for key in collect_indexed_env_keys(prefix) {
        std::env::remove_var(key);
    }
}

fn collect_indexed_env_keys(prefix: &str) -> Vec<String> {
    let mut keys: Vec<String> = std::env::vars_os()
        .filter_map(|(key, _)| {
            let key = key.into_string().ok()?;
            let suffix = key.strip_prefix(prefix)?;
            if suffix.is_empty() || !suffix.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
            Some(key)
        })
        .collect();
    keys.sort();
    keys.dedup();
    keys
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_cli_args_round_trips_previous_env_state() {
        let _guard = stasis_process_env_lock().lock().expect("lock process env");
        std::env::set_var(ARG_COUNT_KEY, "9");
        std::env::set_var(format!("{ARG_PREFIX}0"), "old");
        std::env::set_var(SUMMARY_KEY, "old-summary.json");

        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "program.exe".to_string(),
        ];
        let snapshot = publish_cli_args_to_env(&args, None);

        assert_eq!(std::env::var(ARG_COUNT_KEY).ok().as_deref(), Some("4"));
        assert_eq!(
            std::env::var(format!("{ARG_PREFIX}0")).ok().as_deref(),
            Some("--project-dir")
        );
        assert!(std::env::var(SUMMARY_KEY).is_err());

        restore_cli_args_env(snapshot);
        assert_eq!(std::env::var(ARG_COUNT_KEY).ok().as_deref(), Some("9"));
        assert_eq!(
            std::env::var(format!("{ARG_PREFIX}0")).ok().as_deref(),
            Some("old")
        );
        assert_eq!(
            std::env::var(SUMMARY_KEY).ok().as_deref(),
            Some("old-summary.json")
        );
    }

    #[test]
    fn publish_source_files_round_trips_previous_env_state() {
        let _guard = stasis_process_env_lock().lock().expect("lock process env");
        std::env::set_var(SOURCE_COUNT_KEY, "1");
        std::env::set_var(format!("{SOURCE_PATH_PREFIX}0"), "before_path");
        std::env::set_var(format!("{SOURCE_TEXT_PREFIX}0"), "before_text");

        let payload = vec![("a.stasis".to_string(), "function main(): i32 { return 7; }\n".to_string())];
        let snapshot = publish_source_files_to_env(&payload);

        assert_eq!(std::env::var(SOURCE_COUNT_KEY).ok().as_deref(), Some("1"));
        assert_eq!(
            std::env::var(format!("{SOURCE_PATH_PREFIX}0"))
                .ok()
                .as_deref(),
            Some("a.stasis")
        );
        assert_eq!(
            std::env::var(format!("{SOURCE_TEXT_PREFIX}0"))
                .ok()
                .as_deref(),
            Some("function main(): i32 { return 7; }\n")
        );

        restore_source_files_env(snapshot);
        assert_eq!(std::env::var(SOURCE_COUNT_KEY).ok().as_deref(), Some("1"));
        assert_eq!(
            std::env::var(format!("{SOURCE_PATH_PREFIX}0"))
                .ok()
                .as_deref(),
            Some("before_path")
        );
        assert_eq!(
            std::env::var(format!("{SOURCE_TEXT_PREFIX}0"))
                .ok()
                .as_deref(),
            Some("before_text")
        );
    }
}
