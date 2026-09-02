#![cfg_attr(not(debug_assertions), deny(warnings))]

mod release_assets;
mod toolchain_cli;

use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
#[cfg(test)]
use stasis::escape_mobile_c_string_literal;
use stasis::{
    mobile_aot_function_for, run_jit_tests_in_directory_with_session,
    run_play_in_process_with_input_script_window_title_and_profile,
    run_play_in_process_with_replay, run_self_host_aot_cli_with_options, run_with_default_backend,
    run_with_real_backend, write_mobile_aot_bindings_source_with_profile_and_assets,
    PlayProfileConfig, PlayReplayConfig, RunnerConfig, StasisTestRunSession,
};
use stasis_assets::{prepare_asset_bundle, DEFAULT_ASSET_MANIFEST_PATH};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::{AotOptimizationProfile, EngineEntrypoints};
use stasis_compiler::compiler::{source_function_items, source_struct_items};
use stasis_jit::AotTarget;
use stasis_runner::swap::contracts::TargetMode;

struct CliOptions {
    runner: RunnerConfig,
    emit_events_jsonl: bool,
    events_jsonl_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TestCliArgs {
    directory: PathBuf,
    watch: bool,
    watch_settle_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AotCliContractArgs {
    project_dir: PathBuf,
    output_exe: PathBuf,
    summary_file: Option<PathBuf>,
    entry_file: Option<PathBuf>,
    quality_gate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AndroidAotBundleArgs {
    project_dir: PathBuf,
    entry_file: Option<PathBuf>,
    output_dir: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum MobileAotTarget {
    AndroidArm64,
    AndroidX86_64,
    IosArm64,
}

impl MobileAotTarget {
    fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "android-arm64" | "android" => Ok(Self::AndroidArm64),
            "android-x86_64" => Ok(Self::AndroidX86_64),
            "ios-arm64" | "ios" => Ok(Self::IosArm64),
            _ => Err(format!(
                "invalid mobile AOT target '{value}'. Use android-arm64, android-x86_64, or ios-arm64"
            )),
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::AndroidArm64 => "android-arm64",
            Self::AndroidX86_64 => "android-x86_64",
            Self::IosArm64 => "ios-arm64",
        }
    }

    fn aot_target(self) -> AotTarget {
        match self {
            Self::AndroidArm64 => AotTarget::android_arm64_default(),
            Self::AndroidX86_64 => AotTarget::android_x86_64_default(),
            Self::IosArm64 => AotTarget::ios_arm64_default(),
        }
    }

    fn asset_root_dir(self) -> &'static str {
        match self {
            Self::AndroidArm64 | Self::AndroidX86_64 => "apk_assets",
            Self::IosArm64 => "ios_assets",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MobileAotBundleArgs {
    target: MobileAotTarget,
    project_dir: PathBuf,
    entry_file: Option<PathBuf>,
    output_dir: PathBuf,
    profile_functions: Vec<String>,
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlayCliArgs {
    watch_file: Option<PathBuf>,
    watch_dir: Option<PathBuf>,
    data_bind_json: Option<PathBuf>,
    data_bind_struct_meta: Option<PathBuf>,
    input_script: Option<PathBuf>,
    replay_record: Option<PathBuf>,
    replay: Option<PathBuf>,
    tick_sleep_micros: u64,
    ticks: Option<u64>,
    screenshot: Option<PathBuf>,
    screenshot_frame: u64,
    exit_after_screenshot: bool,
    profile_functions: Vec<String>,
    profile_warmup_ticks: u64,
    profile_output: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ResolvedPlayLaunch {
    watch_file: PathBuf,
    watch_dir: PathBuf,
    window_title: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum LookupMode {
    Signature,
    Definition,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LookupCliArgs {
    mode: LookupMode,
    query: String,
    entry_file: Option<PathBuf>,
    file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LookupMatch {
    path: String,
    text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct CacheCleanupSummary {
    removed_files: u64,
    removed_dirs: u64,
}

fn parse_lookup_mode_alias(value: &str) -> Option<LookupMode> {
    match value.to_ascii_lowercase().as_str() {
        "signature" | "sig" => Some(LookupMode::Signature),
        "def" | "definition" => Some(LookupMode::Definition),
        _ => None,
    }
}

fn parse_lookup_cli_args(args: &[String]) -> Result<Option<LookupCliArgs>, String> {
    let Some(first) = args.first() else {
        return Ok(None);
    };
    let first_lower = first.to_ascii_lowercase();
    let direct_mode = parse_lookup_mode_alias(first);
    let expects_mode_arg = matches!(first_lower.as_str(), "s" | "search");
    if !expects_mode_arg && direct_mode.is_none() {
        return Ok(None);
    }

    let mut entry_file: Option<PathBuf> = None;
    let mut file: Option<PathBuf> = None;
    let mut positionals: Vec<String> = Vec::new();
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--entry" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --entry".to_string());
                }
                entry_file = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--file" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --file".to_string());
                }
                file = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown lookup flag '{other}'"));
            }
            _ => {
                positionals.push(args[i].clone());
                i += 1;
            }
        }
    }

    if entry_file.is_some() && file.is_some() {
        return Err("lookup accepts either --entry or --file, not both".to_string());
    }

    let (mode, query) = if expects_mode_arg {
        let Some(mode_text) = positionals.first() else {
            return Err("missing lookup mode. Use signature|sig or def|definition".to_string());
        };
        let Some(mode) = parse_lookup_mode_alias(mode_text) else {
            return Err(format!(
                "invalid lookup mode '{mode_text}'. Use signature|sig or def|definition"
            ));
        };
        let Some(query) = positionals.get(1) else {
            return Err("missing lookup query".to_string());
        };
        if positionals.len() > 2 {
            return Err("unexpected extra lookup arguments".to_string());
        }
        (mode, query.clone())
    } else {
        let Some(mode) = direct_mode else {
            return Err("missing lookup mode".to_string());
        };
        let Some(query) = positionals.first() else {
            return Err("missing lookup query".to_string());
        };
        if positionals.len() > 1 {
            return Err("unexpected extra lookup arguments".to_string());
        }
        (mode, query.clone())
    };

    if query.trim().is_empty() {
        return Err("missing lookup query".to_string());
    }

    Ok(Some(LookupCliArgs {
        mode,
        query,
        entry_file,
        file,
    }))
}

fn is_lookup_ignored_dir(name: &str) -> bool {
    matches!(name, ".git" | ".stasis_cache" | "target")
}

fn collect_lookup_stasis_files_recursive(root: &Path) -> Result<Vec<PathBuf>, String> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
        let mut entries: Vec<PathBuf> = fs::read_dir(dir)
            .map_err(|error| format!("failed to read directory {}: {error}", dir.display()))?
            .filter_map(|entry| entry.ok().map(|value| value.path()))
            .collect();
        entries.sort();
        for path in entries {
            let metadata = fs::symlink_metadata(&path)
                .map_err(|error| format!("failed to stat {}: {error}", path.display()))?;
            let file_type = metadata.file_type();
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
                    continue;
                };
                if is_lookup_ignored_dir(name) {
                    continue;
                }
                walk(&path, out)?;
                continue;
            }
            if path
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
            {
                out.push(path);
            }
        }
        Ok(())
    }

    let mut files = Vec::new();
    walk(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_lookup_entry_files(root: &Path, entry_file: &Path) -> Result<Vec<PathBuf>, String> {
    let root_canonical = root.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize lookup root {}: {error}",
            root.display()
        )
    })?;
    let entry_path = if entry_file.is_absolute() {
        entry_file.to_path_buf()
    } else {
        root_canonical.join(entry_file)
    };
    if !entry_path.exists() {
        return Err(format!(
            "entry file does not exist: {}",
            entry_path.display()
        ));
    }
    let entry_canonical = entry_path.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize entry file {}: {error}",
            entry_path.display()
        )
    })?;
    if !entry_canonical.starts_with(&root_canonical) {
        return Err(format!(
            "entry file {} must be within current directory {}",
            entry_canonical.display(),
            root_canonical.display()
        ));
    }

    let (graph, _) = stasis_compiler::frontend::module_graph::load_project_module_graph(
        &root_canonical,
        &entry_canonical,
    )
    .map_err(|diagnostic| diagnostic.message)?;
    let mut files: Vec<PathBuf> = graph
        .modules()
        .keys()
        .map(|path| root_canonical.join(path))
        .collect();
    files.sort();
    Ok(files)
}

fn collect_lookup_scan_files(root: &Path, parsed: &LookupCliArgs) -> Result<Vec<PathBuf>, String> {
    if let Some(path) = parsed.file.as_deref() {
        let resolved = if path.is_absolute() {
            path.to_path_buf()
        } else {
            root.join(path)
        };
        if !resolved.exists() {
            return Err(format!(
                "lookup file does not exist: {}",
                resolved.display()
            ));
        }
        let metadata = fs::metadata(&resolved)
            .map_err(|error| format!("failed to stat {}: {error}", resolved.display()))?;
        if metadata.is_dir() {
            return Err(format!(
                "lookup file must be a file: {}",
                resolved.display()
            ));
        }
        return resolved
            .canonicalize()
            .map(|path| vec![path])
            .map_err(|error| format!("failed to canonicalize {}: {error}", resolved.display()));
    }
    if let Some(path) = parsed.entry_file.as_deref() {
        return collect_lookup_entry_files(root, path);
    }
    collect_lookup_stasis_files_recursive(root)
}

fn lookup_name_matches(name: &str, query_lower: &str) -> bool {
    name.to_ascii_lowercase().contains(query_lower)
}

fn lookup_display_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

fn collect_lookup_matches_in_source(
    path: &Path,
    root: &Path,
    source: &str,
    parsed: &LookupCliArgs,
) -> Vec<LookupMatch> {
    let query_lower = parsed.query.to_ascii_lowercase();
    let display_path = lookup_display_path(path, root);
    let mut matches = Vec::new();

    if let Ok(functions) = source_function_items([(display_path.clone(), source.to_string())]) {
        for function in functions {
            if !lookup_name_matches(&function.name, &query_lower) {
                continue;
            }
            let text = match parsed.mode {
                LookupMode::Signature => source.get(
                    function.signature_range.start as usize..function.signature_range.end as usize,
                ),
                LookupMode::Definition => source.get(
                    function.signature_range.start as usize..function.source_range.end as usize,
                ),
            };
            if let Some(text) = text {
                matches.push(LookupMatch {
                    path: display_path.clone(),
                    text: text.trim().to_string(),
                });
            }
        }
    }

    if parsed.mode == LookupMode::Definition {
        if let Ok(structs) = source_struct_items(source, display_path.clone()) {
            for definition in structs {
                if !lookup_name_matches(&definition.name, &query_lower) {
                    continue;
                }
                if let Some(text) = source.get(
                    definition.definition_range.start as usize
                        ..definition.definition_range.end as usize,
                ) {
                    matches.push(LookupMatch {
                        path: display_path.clone(),
                        text: text.trim().to_string(),
                    });
                }
            }
        }
    }

    matches
}

fn render_lookup_matches(matches: &[LookupMatch]) -> String {
    let mut output = String::new();
    for item in matches {
        output.push_str(&item.path);
        output.push('\n');
        output.push_str(&item.text);
        output.push_str("\n\n");
    }
    output
}

fn run_lookup_command(parsed: &LookupCliArgs, root: &Path) -> Result<String, String> {
    let root_canonical = root.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize current directory {}: {error}",
            root.display()
        )
    })?;
    let files = collect_lookup_scan_files(&root_canonical, parsed)?;
    let mut matches = Vec::new();
    for path in files {
        let bytes = fs::read(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let source = String::from_utf8_lossy(&bytes).to_string();
        matches.extend(collect_lookup_matches_in_source(
            &path,
            &root_canonical,
            &source,
            parsed,
        ));
    }
    Ok(render_lookup_matches(&matches))
}

fn try_run_lookup_subcommand() -> Option<i32> {
    let args: Vec<String> = env::args().skip(1).collect();
    let parsed = match parse_lookup_cli_args(&args) {
        Ok(Some(value)) => value,
        Ok(None) => return None,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };
    let root = match env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to read current directory: {error}");
            return Some(1);
        }
    };
    match run_lookup_command(&parsed, &root) {
        Ok(output) => {
            print!("{output}");
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

fn parse_play_cli_args(args: &[String]) -> Result<PlayCliArgs, String> {
    let mut watch_file: Option<PathBuf> = None;
    let mut watch_dir: Option<PathBuf> = None;
    let mut data_bind_json: Option<PathBuf> = None;
    let mut data_bind_struct_meta: Option<PathBuf> = None;
    let mut input_script: Option<PathBuf> = None;
    let mut replay_record: Option<PathBuf> = None;
    let mut replay: Option<PathBuf> = None;
    let mut tick_sleep_micros: u64 = 16000;
    let mut ticks: Option<u64> = None;
    let mut screenshot: Option<PathBuf> = None;
    let mut screenshot_frame: u64 = 1;
    let mut screenshot_frame_explicit = false;
    let mut exit_after_screenshot = false;
    let mut profile_functions = Vec::new();
    let mut profile_warmup_ticks = 60_u64;
    let mut profile_warmup_explicit = false;
    let mut profile_output: Option<PathBuf> = None;
    let mut i: usize = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        // Allow `stasis.exe play <entry.stasis>` as the default path.
        if !arg.starts_with("--") && watch_file.is_none() {
            watch_file = Some(PathBuf::from(args[i].clone()));
            i += 1;
            continue;
        }
        if arg == "--watch-file" {
            if i + 1 >= args.len() {
                return Err("missing value for --watch-file".to_string());
            }
            watch_file = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--watch-dir" {
            if i + 1 >= args.len() {
                return Err("missing value for --watch-dir".to_string());
            }
            watch_dir = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--data-bind" {
            if i + 2 >= args.len() {
                return Err(
                    "missing values for --data-bind <data_path> <struct_meta_path>".to_string(),
                );
            }
            data_bind_json = Some(PathBuf::from(args[i + 1].clone()));
            data_bind_struct_meta = Some(PathBuf::from(args[i + 2].clone()));
            i += 3;
            continue;
        }
        if arg == "--input-script" {
            if i + 1 >= args.len() {
                return Err("missing value for --input-script".to_string());
            }
            input_script = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--record-replay" {
            if i + 1 >= args.len() {
                return Err("missing value for --record-replay".to_string());
            }
            replay_record = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--replay" {
            if i + 1 >= args.len() {
                return Err("missing value for --replay".to_string());
            }
            replay = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--ticks" {
            if i + 1 >= args.len() {
                return Err("missing value for --ticks".to_string());
            }
            ticks = Some(
                args[i + 1]
                    .parse::<u64>()
                    .map_err(|error| format!("invalid value for --ticks: {error}"))?,
            );
            i += 2;
            continue;
        }
        if arg == "--tick-sleep-us" {
            if i + 1 >= args.len() {
                return Err("missing value for --tick-sleep-us".to_string());
            }
            tick_sleep_micros = args[i + 1]
                .parse::<u64>()
                .map_err(|error| format!("invalid value for --tick-sleep-us: {error}"))?;
            i += 2;
            continue;
        }
        if arg == "--screenshot" {
            if i + 1 >= args.len() {
                return Err("missing value for --screenshot".to_string());
            }
            screenshot = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--screenshot-frame" {
            if i + 1 >= args.len() {
                return Err("missing value for --screenshot-frame".to_string());
            }
            screenshot_frame = args[i + 1]
                .parse::<u64>()
                .map_err(|error| format!("invalid value for --screenshot-frame: {error}"))?;
            if screenshot_frame == 0 || screenshot_frame > i32::MAX as u64 {
                return Err("--screenshot-frame must be between 1 and 2147483647".to_string());
            }
            screenshot_frame_explicit = true;
            i += 2;
            continue;
        }
        if arg == "--exit-after-screenshot" {
            exit_after_screenshot = true;
            i += 1;
            continue;
        }
        if arg == "--profile-functions" {
            if i + 1 >= args.len() {
                return Err("missing value for --profile-functions <name[,name...]>".to_string());
            }
            profile_functions.extend(
                args[i + 1]
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(str::to_string),
            );
            i += 2;
            continue;
        }
        if arg == "--profile-warmup" {
            if i + 1 >= args.len() {
                return Err("missing value for --profile-warmup <ticks>".to_string());
            }
            profile_warmup_ticks = args[i + 1]
                .parse::<u64>()
                .map_err(|error| format!("invalid value for --profile-warmup: {error}"))?;
            profile_warmup_explicit = true;
            i += 2;
            continue;
        }
        if arg == "--profile-output" {
            if i + 1 >= args.len() {
                return Err("missing value for --profile-output <path>".to_string());
            }
            profile_output = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        i += 1;
    }
    if screenshot.is_none() && (screenshot_frame_explicit || exit_after_screenshot) {
        return Err(
            "--screenshot-frame and --exit-after-screenshot require --screenshot <path>"
                .to_string(),
        );
    }
    if screenshot.is_some() && ticks.is_some_and(|count| count < screenshot_frame) {
        return Err("--ticks must be at least --screenshot-frame when capturing".to_string());
    }
    if profile_functions.is_empty() && (profile_warmup_explicit || profile_output.is_some()) {
        return Err(
            "--profile-warmup and --profile-output require --profile-functions".to_string(),
        );
    }
    if replay_record.is_some() && replay.is_some() {
        return Err("--record-replay cannot be combined with --replay".to_string());
    }
    if replay.is_some() && input_script.is_some() {
        return Err("--replay cannot be combined with --input-script".to_string());
    }
    if !profile_functions.is_empty() && ticks.is_some_and(|count| count <= profile_warmup_ticks) {
        return Err(
            "--ticks must exceed --profile-warmup so at least one frame is measured".to_string(),
        );
    }
    Ok(PlayCliArgs {
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        input_script,
        replay_record,
        replay,
        tick_sleep_micros,
        ticks,
        screenshot,
        screenshot_frame,
        exit_after_screenshot,
        profile_functions,
        profile_warmup_ticks,
        profile_output,
    })
}

fn find_play_manifest_root(start: &Path) -> Option<PathBuf> {
    let start_dir = if start.is_file() {
        start.parent()?
    } else {
        start
    };
    start_dir
        .ancestors()
        .find(|ancestor| ancestor.join("stasis.json").is_file())
        .map(Path::to_path_buf)
}

fn resolve_existing_play_file(path: &Path, launch_dir: &Path) -> Result<PathBuf, String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        launch_dir.join(path)
    };
    let resolved = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to resolve play entry {}: {error}",
            candidate.display()
        )
    })?;
    if !resolved.is_file() {
        return Err(format!("play entry is not a file: {}", resolved.display()));
    }
    Ok(resolved)
}

fn resolve_existing_play_directory(path: &Path, launch_dir: &Path) -> Result<PathBuf, String> {
    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        launch_dir.join(path)
    };
    let resolved = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to resolve play watch directory {}: {error}",
            candidate.display()
        )
    })?;
    if !resolved.is_dir() {
        return Err(format!(
            "play watch directory is not a directory: {}",
            resolved.display()
        ));
    }
    Ok(resolved)
}

fn load_play_manifest_details(root: &Path) -> Result<(PathBuf, Option<String>), String> {
    let manifest_path = root.join("stasis.json");
    let source = fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_str(&source)
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    let entry = manifest
        .get("entry")
        .and_then(serde_json::Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{} has no non-empty entry", manifest_path.display()))?;
    let entry = Path::new(entry);
    if entry.is_absolute() {
        return Err(format!(
            "{} entry must be project-relative",
            manifest_path.display()
        ));
    }
    let resolved_entry = resolve_existing_play_file(entry, root)?;
    let canonical_root = root
        .canonicalize()
        .map_err(|error| format!("failed to resolve project root {}: {error}", root.display()))?;
    if !resolved_entry.starts_with(&canonical_root) {
        return Err(format!(
            "{} entry escapes project root {}",
            manifest_path.display(),
            canonical_root.display()
        ));
    }
    let title = manifest
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    Ok((resolved_entry, title))
}

fn resolve_play_launch(
    parsed: &PlayCliArgs,
    launch_dir: &Path,
) -> Result<ResolvedPlayLaunch, String> {
    let explicit_entry = parsed
        .watch_file
        .as_deref()
        .map(|path| resolve_existing_play_file(path, launch_dir))
        .transpose()?;
    let manifest_root = if let Some(entry) = explicit_entry.as_deref() {
        find_play_manifest_root(entry)
    } else {
        find_play_manifest_root(launch_dir)
    };

    let (watch_file, window_title) = if let Some(entry) = explicit_entry {
        let title = manifest_root
            .as_deref()
            .and_then(|root| load_play_manifest_details(root).ok())
            .and_then(|(_, title)| title);
        (entry, title)
    } else {
        let root = manifest_root.as_deref().ok_or_else(|| {
            format!(
                "missing entry file and no stasis.json found from {}",
                launch_dir.display()
            )
        })?;
        load_play_manifest_details(root)?
    };

    let watch_dir = match parsed
        .watch_dir
        .as_deref()
        .filter(|path| !path.as_os_str().is_empty())
    {
        Some(path) => resolve_existing_play_directory(path, launch_dir)?,
        None => manifest_root
            .or_else(|| watch_file.parent().map(Path::to_path_buf))
            .ok_or_else(|| format!("play entry has no parent: {}", watch_file.display()))?
            .canonicalize()
            .map_err(|error| format!("failed to resolve play watch directory: {error}"))?,
    };

    Ok(ResolvedPlayLaunch {
        watch_file,
        watch_dir,
        window_title,
    })
}

struct PlayScreenshotEnvironment {
    previous: Vec<(&'static str, Option<std::ffi::OsString>)>,
    output_path: Option<PathBuf>,
}

impl Drop for PlayScreenshotEnvironment {
    fn drop(&mut self) {
        for (name, value) in self.previous.drain(..).rev() {
            if let Some(value) = value {
                env::set_var(name, value);
            } else {
                env::remove_var(name);
            }
        }
    }
}

fn configure_play_screenshot_environment(
    parsed: &PlayCliArgs,
) -> Result<PlayScreenshotEnvironment, String> {
    let Some(path) = parsed.screenshot.as_ref() else {
        return Ok(PlayScreenshotEnvironment {
            previous: Vec::new(),
            output_path: None,
        });
    };
    let resolved = if path.is_absolute() {
        path.clone()
    } else {
        env::current_dir()
            .map_err(|error| format!("failed to resolve --screenshot path: {error}"))?
            .join(path)
    };
    let names = [
        "STASIS_SCREENSHOT_ONCE",
        "STASIS_SCREENSHOT_FRAME",
        "STASIS_EXIT_AFTER_SCREENSHOT",
    ];
    let parent = resolved
        .parent()
        .ok_or_else(|| "--screenshot path has no parent directory".to_string())?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create screenshot directory {}: {error}",
            parent.display()
        )
    })?;
    if resolved.exists() {
        fs::remove_file(&resolved).map_err(|error| {
            format!(
                "failed to replace screenshot output {}: {error}",
                resolved.display()
            )
        })?;
    }
    let guard = PlayScreenshotEnvironment {
        previous: names
            .into_iter()
            .map(|name| (name, env::var_os(name)))
            .collect(),
        output_path: Some(resolved.clone()),
    };
    env::set_var("STASIS_SCREENSHOT_ONCE", resolved);
    env::set_var(
        "STASIS_SCREENSHOT_FRAME",
        parsed.screenshot_frame.to_string(),
    );
    if parsed.exit_after_screenshot {
        env::set_var("STASIS_EXIT_AFTER_SCREENSHOT", "1");
    } else {
        env::remove_var("STASIS_EXIT_AFTER_SCREENSHOT");
    }
    Ok(guard)
}

fn try_run_play_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "play" {
        return None;
    }
    if let Err(message) = toolchain_cli::verify_installed_toolchain_identity() {
        eprintln!("{message}");
        return Some(1);
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_play_cli_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };
    let launch_dir = match env::current_dir() {
        Ok(path) => path,
        Err(error) => {
            eprintln!("failed to read current directory: {error}");
            return Some(2);
        }
    };
    let launch = match resolve_play_launch(&parsed, &launch_dir) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };
    let screenshot_environment = match configure_play_screenshot_environment(&parsed) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    let profile = (!parsed.profile_functions.is_empty()).then(|| PlayProfileConfig {
        functions: parsed.profile_functions.clone(),
        warmup_ticks: parsed.profile_warmup_ticks,
        output_path: parsed.profile_output.clone(),
    });
    let replay = parsed
        .replay_record
        .clone()
        .map(PlayReplayConfig::Record)
        .or_else(|| parsed.replay.clone().map(PlayReplayConfig::Replay));
    let play_result = if let Some(replay) = replay {
        run_play_in_process_with_replay(
            &launch.watch_file,
            Some(&launch.watch_dir),
            parsed.data_bind_json.as_deref(),
            parsed.data_bind_struct_meta.as_deref(),
            parsed.input_script.as_deref(),
            parsed.tick_sleep_micros,
            parsed.ticks,
            launch.window_title.as_deref(),
            profile,
            None,
            replay,
        )
    } else {
        run_play_in_process_with_input_script_window_title_and_profile(
            &launch.watch_file,
            Some(&launch.watch_dir),
            parsed.data_bind_json.as_deref(),
            parsed.data_bind_struct_meta.as_deref(),
            parsed.input_script.as_deref(),
            parsed.tick_sleep_micros,
            parsed.ticks,
            launch.window_title.as_deref(),
            profile,
        )
    };
    match play_result {
        Ok(()) => {
            if let Some(path) = screenshot_environment.output_path.as_ref() {
                match fs::metadata(path) {
                    Ok(metadata) if metadata.len() > 0 => {}
                    Ok(_) => {
                        eprintln!("screenshot output is empty: {}", path.display());
                        return Some(1);
                    }
                    Err(error) => {
                        eprintln!("screenshot was not captured at {}: {error}", path.display());
                        return Some(1);
                    }
                }
            }
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

fn try_run_probe_graphics_runtime_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "probe-graphics-runtime" {
        return None;
    }
    if let Err(error) = toolchain_cli::verify_installed_toolchain_identity() {
        eprintln!("{error}");
        return Some(1);
    }

    let candidates = stasis_dynload::runtime_library_candidate_paths();
    for candidate in &candidates {
        if !candidate.exists() {
            continue;
        }
        match stasis_dynload::StasisGraphicsApi::load(candidate) {
            Ok(api) => {
                println!("graphics_runtime_loaded=1");
                println!("graphics_runtime_path={}", candidate.display());
                // Exercise at least one export to catch partial-load issues.
                let _ = api.sleep_ms(0);
                return Some(0);
            }
            Err(error) => {
                eprintln!("failed loading {}: {error}", candidate.display());
            }
        }
    }

    eprintln!("graphics_runtime_loaded=0");
    eprintln!("searched_candidates={}", candidates.len());
    for (idx, candidate) in candidates.iter().enumerate() {
        eprintln!("candidate[{idx}]={}", candidate.display());
    }
    Some(1)
}

fn parse_args() -> CliOptions {
    let mut config = RunnerConfig::default();
    let mut emit_events_jsonl = false;
    let mut events_jsonl_file: Option<PathBuf> = None;
    let mut args = env::args().skip(1);

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--ticks" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<u32>() {
                        config.max_ticks = parsed;
                    }
                }
            }
            "--tick-sleep-us" => {
                if let Some(value) = args.next() {
                    if let Ok(parsed) = value.parse::<u64>() {
                        config.tick_sleep_micros = parsed;
                    }
                }
            }
            "--watch-file" => {
                if let Some(value) = args.next() {
                    config.inject_file_change = Some(PathBuf::from(value));
                }
            }
            "--watch-dir" => {
                if let Some(value) = args.next() {
                    config.watch_directory = Some(PathBuf::from(value));
                }
            }
            "--target-mode" => {
                if let Some(value) = args.next() {
                    if value.eq_ignore_ascii_case("jit") || value.eq_ignore_ascii_case("jit-dev") {
                        config.target_mode = TargetMode::JitDev;
                    } else if value.eq_ignore_ascii_case("aot")
                        || value.eq_ignore_ascii_case("aot-prod")
                    {
                        config.target_mode = TargetMode::AotProd;
                    }
                }
            }
            "--host-set-profile" => {
                if let Some(value) = args.next() {
                    config.host_set_profile = Some(value);
                }
            }
            "--host-set-registry-file" => {
                if let Some(value) = args.next() {
                    config.host_set_registry_file = Some(PathBuf::from(value));
                }
            }
            "--aot-prod" => {
                config.target_mode = TargetMode::AotProd;
            }
            "--fail-compile" => {
                config.fail_compile = true;
            }
            "--no-hook" => {
                config.disable_on_code_swap_hook = true;
            }
            "--fail-hook" => {
                config.hook_failure_reason = Some("simulated on_code_swap failure".to_string());
            }
            "--fail-hook-reason" => {
                if let Some(value) = args.next() {
                    config.hook_failure_reason = Some(value);
                }
            }
            "--fail-swap" => {
                config.swap_failure_reason = Some("simulated swap failure".to_string());
            }
            "--fail-swap-reason" => {
                if let Some(value) = args.next() {
                    config.swap_failure_reason = Some(value);
                }
            }
            "--runtime-launch" => {
                config.runtime_launch = true;
            }
            "--no-runtime-launch" => {
                config.runtime_launch = false;
            }
            "--aot-probe-load" => {
                config.aot_probe_loadability = true;
            }
            "--no-aot-probe-load" => {
                config.aot_probe_loadability = false;
            }
            "--events-jsonl" => {
                emit_events_jsonl = true;
            }
            "--events-jsonl-file" => {
                if let Some(value) = args.next() {
                    emit_events_jsonl = true;
                    events_jsonl_file = Some(PathBuf::from(value));
                }
            }
            _ => {}
        }
    }

    CliOptions {
        runner: config,
        emit_events_jsonl,
        events_jsonl_file,
    }
}

fn parse_test_cli_args(args: &[String]) -> Result<TestCliArgs, String> {
    let mut directory: Option<PathBuf> = None;
    let mut watch = false;
    let mut watch_settle_ms: u64 = 0;
    let mut i: usize = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--dir" {
            if i + 1 >= args.len() {
                return Err("missing value for --dir".to_string());
            }
            directory = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--watch" {
            watch = true;
            i += 1;
            continue;
        }
        if arg == "--watch-settle-ms" {
            if i + 1 >= args.len() {
                return Err("missing value for --watch-settle-ms".to_string());
            }
            let parsed = args[i + 1].parse::<u64>().map_err(|error| {
                format!(
                    "invalid value for --watch-settle-ms '{}': {error}",
                    args[i + 1]
                )
            })?;
            watch_settle_ms = parsed;
            i += 2;
            continue;
        }
        i += 1;
    }
    let Some(directory) = directory else {
        return Err("missing required --dir <path>".to_string());
    };
    Ok(TestCliArgs {
        directory,
        watch,
        watch_settle_ms,
    })
}

fn configured_stasis_cache_ttl_days() -> u64 {
    std::env::var("STASIS_CACHE_TTL_DAYS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(7)
}

fn maybe_cleanup_stale_stasis_cache() {
    let ttl_days = configured_stasis_cache_ttl_days();
    if ttl_days == 0 {
        return;
    }
    let ttl = Duration::from_secs(ttl_days.saturating_mul(24 * 60 * 60));
    let cache_root = Path::new(".stasis_cache");
    match cleanup_stale_stasis_cache(cache_root, ttl) {
        Ok(summary) => {
            if summary.removed_files > 0 || summary.removed_dirs > 0 {
                eprintln!("cache_cleanup_removed_files={}", summary.removed_files);
                eprintln!("cache_cleanup_removed_dirs={}", summary.removed_dirs);
                eprintln!("cache_cleanup_ttl_days={ttl_days}");
            }
        }
        Err(message) => {
            eprintln!("{message}");
        }
    }
}

fn cleanup_stale_stasis_cache(
    cache_root: &Path,
    max_age: Duration,
) -> Result<CacheCleanupSummary, String> {
    if !cache_root.exists() {
        return Ok(CacheCleanupSummary::default());
    }
    let now = SystemTime::now();
    let cutoff = now.checked_sub(max_age).unwrap_or(SystemTime::UNIX_EPOCH);
    let mut summary = CacheCleanupSummary::default();
    cleanup_stale_stasis_cache_dir(cache_root, cutoff, true, &mut summary)?;
    Ok(summary)
}

fn cleanup_stale_stasis_cache_dir(
    dir: &Path,
    cutoff: SystemTime,
    keep_dir: bool,
    summary: &mut CacheCleanupSummary,
) -> Result<(), String> {
    let mut entries: Vec<PathBuf> = Vec::new();
    for entry in fs::read_dir(dir)
        .map_err(|error| format!("failed to read cache dir '{}': {error}", dir.display()))?
    {
        let entry = entry
            .map_err(|error| format!("failed reading cache entry '{}': {error}", dir.display()))?;
        entries.push(entry.path());
    }
    for path in entries {
        let metadata = fs::metadata(&path)
            .map_err(|error| format!("failed to stat cache path '{}': {error}", path.display()))?;
        if metadata.is_dir() {
            cleanup_stale_stasis_cache_dir(&path, cutoff, false, summary)?;
            continue;
        }
        if cache_entry_is_stale(&metadata, cutoff) {
            fs::remove_file(&path).map_err(|error| {
                format!(
                    "failed removing stale cache file '{}': {error}",
                    path.display()
                )
            })?;
            summary.removed_files = summary.removed_files.saturating_add(1);
        }
    }
    if keep_dir {
        return Ok(());
    }
    let is_empty = fs::read_dir(dir)
        .map_err(|error| format!("failed to read cache dir '{}': {error}", dir.display()))?
        .next()
        .is_none();
    if is_empty {
        fs::remove_dir(dir).map_err(|error| {
            format!(
                "failed removing stale cache directory '{}': {error}",
                dir.display()
            )
        })?;
        summary.removed_dirs = summary.removed_dirs.saturating_add(1);
    }
    Ok(())
}

fn cache_entry_is_stale(metadata: &fs::Metadata, cutoff: SystemTime) -> bool {
    let modified = metadata.modified().ok();
    let accessed = metadata.accessed().ok();
    match (modified, accessed) {
        (Some(left), Some(right)) => left.max(right) <= cutoff,
        (Some(used), None) | (None, Some(used)) => used <= cutoff,
        (None, None) => false,
    }
}

fn try_run_test_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "test" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_test_cli_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    if parsed.watch {
        let exit = match run_test_watch_loop(&parsed) {
            Ok(code) => code,
            Err(message) => {
                eprintln!("{message}");
                1
            }
        };
        return Some(exit);
    }

    let exit = match run_test_dir_once(&parsed.directory) {
        Ok(code) => code,
        Err(message) => {
            eprintln!("{message}");
            1
        }
    };
    Some(exit)
}

fn run_test_dir_once(directory: &Path) -> Result<i32, String> {
    let mut session = StasisTestRunSession::new();
    run_test_dir_once_with_session(directory, &mut session)
}

fn run_test_watch_loop(parsed: &TestCliArgs) -> Result<i32, String> {
    let mut session = StasisTestRunSession::new();
    let first_exit = run_test_dir_once_with_session(&parsed.directory, &mut session)?;
    println!("watch_mode=1");
    println!("watch_settle_ms={}", parsed.watch_settle_ms);
    println!("watch_last_exit={first_exit}");
    flush_test_stdout();

    let (tx, rx) = channel::<notify::Result<Event>>();
    let mut watcher = RecommendedWatcher::new(
        move |res| {
            let _ = tx.send(res);
        },
        Config::default(),
    )
    .map_err(|error| format!("failed to create test watcher: {error}"))?;
    watcher
        .watch(&parsed.directory, RecursiveMode::Recursive)
        .map_err(|error| {
            format!(
                "failed to watch test directory '{}': {error}",
                parsed.directory.display()
            )
        })?;

    let settle = Duration::from_millis(parsed.watch_settle_ms);
    loop {
        let mut saw_stasis_change = false;
        while !saw_stasis_change {
            let event = rx
                .recv()
                .map_err(|_| "test watch channel disconnected".to_string())?;
            let Ok(event) = event else {
                continue;
            };
            if is_stasis_change_event(&event) {
                saw_stasis_change = true;
            }
        }

        let mut last_change_at = Instant::now();
        loop {
            let remaining = settle
                .checked_sub(last_change_at.elapsed())
                .unwrap_or(Duration::ZERO);
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(Ok(event)) => {
                    if is_stasis_change_event(&event) {
                        last_change_at = Instant::now();
                    }
                }
                Ok(Err(_)) => {}
                Err(RecvTimeoutError::Timeout) => break,
                Err(RecvTimeoutError::Disconnected) => {
                    return Err("test watch channel disconnected".to_string());
                }
            }
        }

        println!("watch_change=stasis");
        flush_test_stdout();
        let exit = run_test_dir_once_with_session(&parsed.directory, &mut session)?;
        println!("watch_last_exit={exit}");
        flush_test_stdout();
    }
}

fn run_test_dir_once_with_session(
    directory: &Path,
    session: &mut StasisTestRunSession,
) -> Result<i32, String> {
    let started = Instant::now();
    let summary = run_jit_tests_in_directory_with_session(directory, session)?;
    println!("test_files_discovered={}", summary.files_discovered);
    println!("test_files_with_tests={}", summary.files_with_tests);
    println!("tests_discovered={}", summary.tests_discovered);
    println!("tests_run={}", summary.tests_run);
    println!("tests_passed={}", summary.tests_passed);
    println!("tests_failed={}", summary.tests_failed);
    println!(
        "timing_discovery_ms={:.3}",
        (summary.timing_discovery_us as f64) / 1000.0
    );
    println!(
        "timing_prepare_ms={:.3}",
        (summary.timing_prepare_us as f64) / 1000.0
    );
    println!(
        "timing_compile_ms={:.3}",
        (summary.timing_compile_us as f64) / 1000.0
    );
    println!(
        "timing_execute_ms={:.3}",
        (summary.timing_execute_us as f64) / 1000.0
    );
    println!(
        "timing_total_ms={:.3}",
        (summary.timing_total_us as f64) / 1000.0
    );
    for failure in &summary.failures {
        println!("test_failure={failure}");
    }
    println!("elapsed_ms={:.3}", started.elapsed().as_secs_f64() * 1000.0);
    flush_test_stdout();
    Ok(if summary.tests_failed > 0 { 1 } else { 0 })
}

fn flush_test_stdout() {
    let _ = io::stdout().flush();
}

fn is_stasis_change_event(event: &Event) -> bool {
    let is_change_kind = matches!(
        event.kind,
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
    );
    if !is_change_kind {
        return false;
    }
    event.paths.iter().any(|path| {
        path.extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("stasis"))
    })
}

fn write_events_jsonl(
    summary: &stasis::RunnerSummary,
    output_file: Option<&Path>,
) -> Result<(), String> {
    let mut writer: Box<dyn Write> = match output_file {
        Some(path) => {
            Box::new(BufWriter::new(File::create(path).map_err(|err| {
                format!("failed to create {}: {err}", path.display())
            })?))
        }
        None => Box::new(BufWriter::new(io::stdout().lock())),
    };

    for event in &summary.events {
        let line = serde_json::to_string(&event.with_schema_version())
            .map_err(|err| format!("failed to serialize event: {err}"))?;
        writeln!(writer, "{line}").map_err(|err| format!("failed to write event line: {err}"))?;
    }
    writer
        .flush()
        .map_err(|err| format!("failed to flush event output: {err}"))?;
    Ok(())
}

fn parse_aot_cli_contract_args(args: &[String]) -> Result<AotCliContractArgs, String> {
    let mut project_dir: Option<PathBuf> = None;
    let mut output_exe: Option<PathBuf> = None;
    let mut summary_file: Option<PathBuf> = None;
    let mut entry_file: Option<PathBuf> = None;
    let mut quality_gate = false;
    let mut i: usize = 0;
    while i < args.len() {
        let arg = args[i].as_str();
        if arg == "--project-dir" {
            if i + 1 >= args.len() {
                return Err("missing value for --project-dir".to_string());
            }
            project_dir = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--out" {
            if i + 1 >= args.len() {
                return Err("missing value for --out".to_string());
            }
            output_exe = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--summary-file" {
            if i + 1 >= args.len() {
                return Err("missing value for --summary-file".to_string());
            }
            summary_file = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--entry-file" {
            if i + 1 >= args.len() {
                return Err("missing value for --entry-file".to_string());
            }
            entry_file = Some(PathBuf::from(args[i + 1].clone()));
            i += 2;
            continue;
        }
        if arg == "--quality-gate" {
            quality_gate = true;
            i += 1;
            continue;
        }
        i += 1;
    }

    let Some(project_dir) = project_dir else {
        return Err("missing required --project-dir <path>".to_string());
    };
    let Some(output_exe) = output_exe else {
        return Err("missing required --out <path>".to_string());
    };
    Ok(AotCliContractArgs {
        project_dir,
        output_exe,
        summary_file,
        entry_file,
        quality_gate,
    })
}

fn parse_android_aot_bundle_args(args: &[String]) -> Result<AndroidAotBundleArgs, String> {
    let mut project_dir: Option<PathBuf> = None;
    let mut entry_file: Option<PathBuf> = None;
    let mut output_dir: Option<PathBuf> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--project-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --project-dir".to_string());
                }
                project_dir = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--entry-file" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --entry-file".to_string());
                }
                entry_file = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--out-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --out-dir".to_string());
                }
                output_dir = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            _ => i += 1,
        }
    }
    let Some(project_dir) = project_dir else {
        return Err("missing required --project-dir <path>".to_string());
    };
    let Some(output_dir) = output_dir else {
        return Err("missing required --out-dir <path>".to_string());
    };
    Ok(AndroidAotBundleArgs {
        project_dir,
        entry_file,
        output_dir,
    })
}

fn parse_mobile_aot_bundle_args(args: &[String]) -> Result<MobileAotBundleArgs, String> {
    let mut target: Option<MobileAotTarget> = None;
    let mut project_dir: Option<PathBuf> = None;
    let mut entry_file: Option<PathBuf> = None;
    let mut output_dir: Option<PathBuf> = None;
    let mut profile_functions = Vec::new();
    let mut profile_warmup_frames = 120_u32;
    let mut profile_sample_frames = 300_u32;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--target" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --target".to_string());
                }
                target = Some(MobileAotTarget::parse(&args[i + 1])?);
                i += 2;
            }
            "--project-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --project-dir".to_string());
                }
                project_dir = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--entry-file" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --entry-file".to_string());
                }
                entry_file = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--out-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --out-dir".to_string());
                }
                output_dir = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--profile-functions" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --profile-functions".to_string());
                }
                profile_functions.extend(
                    args[i + 1]
                        .split(',')
                        .map(str::trim)
                        .filter(|name| !name.is_empty())
                        .map(str::to_string),
                );
                i += 2;
            }
            "--profile-warmup-frames" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --profile-warmup-frames".to_string());
                }
                profile_warmup_frames = args[i + 1]
                    .parse()
                    .map_err(|error| format!("invalid --profile-warmup-frames: {error}"))?;
                i += 2;
            }
            "--profile-sample-frames" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --profile-sample-frames".to_string());
                }
                profile_sample_frames = args[i + 1]
                    .parse()
                    .map_err(|error| format!("invalid --profile-sample-frames: {error}"))?;
                i += 2;
            }
            other if other.starts_with("--") => {
                return Err(format!("unknown mobile AOT bundle flag '{other}'"));
            }
            other => {
                return Err(format!("unexpected mobile AOT bundle argument '{other}'"));
            }
        }
    }
    let Some(target) = target else {
        return Err(
            "missing required --target <android-arm64|android-x86_64|ios-arm64>".to_string(),
        );
    };
    let Some(project_dir) = project_dir else {
        return Err("missing required --project-dir <path>".to_string());
    };
    let Some(output_dir) = output_dir else {
        return Err("missing required --out-dir <path>".to_string());
    };
    Ok(MobileAotBundleArgs {
        target,
        project_dir,
        entry_file,
        output_dir,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
    })
}

fn try_run_mobile_aot_bundle_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "mobile-aot-bundle" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_mobile_aot_bundle_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    match write_mobile_aot_engine_bundle(
        parsed.target,
        &parsed.project_dir,
        parsed.entry_file.as_deref(),
        &parsed.output_dir,
        &parsed.profile_functions,
        parsed.profile_warmup_frames,
        parsed.profile_sample_frames,
    ) {
        Ok(summary) => {
            println!("mobile_aot_target={}", summary.target.as_str());
            println!("mobile_aot_bundle_dir={}", summary.bundle_dir.display());
            println!("mobile_aot_object_count={}", summary.object_count);
            println!(
                "mobile_aot_symbols_header={}",
                summary.symbols_header.display()
            );
            println!(
                "mobile_aot_bindings_source={}",
                summary.bindings_source.display()
            );
            if let Some(cmake_file) = summary.cmake_file.as_ref() {
                println!("mobile_aot_cmake_file={}", cmake_file.display());
            }
            println!("mobile_aot_asset_dir={}", summary.asset_dir.display());
            println!(
                "mobile_aot_package_manifest={}",
                summary.package_manifest.display()
            );
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

fn try_run_android_aot_bundle_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "android-aot-bundle" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_android_aot_bundle_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    match write_android_aot_engine_bundle(
        &parsed.project_dir,
        parsed.entry_file.as_deref(),
        &parsed.output_dir,
    ) {
        Ok(summary) => {
            println!("android_aot_bundle_dir={}", summary.bundle_dir.display());
            println!("android_aot_object_count={}", summary.object_count);
            println!(
                "android_aot_symbols_header={}",
                summary.symbols_header.display()
            );
            println!(
                "android_aot_bindings_source={}",
                summary.bindings_source.display()
            );
            println!("android_aot_cmake_file={}", summary.cmake_file.display());
            println!("android_aot_asset_dir={}", summary.asset_dir.display());
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

struct AndroidAotBundleSummary {
    bundle_dir: PathBuf,
    object_count: usize,
    symbols_header: PathBuf,
    bindings_source: PathBuf,
    cmake_file: PathBuf,
    asset_dir: PathBuf,
}

struct MobileAotBundleSummary {
    target: MobileAotTarget,
    bundle_dir: PathBuf,
    object_count: usize,
    symbols_header: PathBuf,
    bindings_source: PathBuf,
    cmake_file: Option<PathBuf>,
    asset_dir: PathBuf,
    package_manifest: PathBuf,
}

fn mobile_engine_entrypoints() -> EngineEntrypoints {
    EngineEntrypoints {
        tick: "tick".to_string(),
        render: "render".to_string(),
        on_code_swap: None,
    }
}

fn write_android_aot_engine_bundle(
    project_dir: &Path,
    entry_file: Option<&Path>,
    output_dir: &Path,
) -> Result<AndroidAotBundleSummary, String> {
    let summary = write_mobile_aot_engine_bundle(
        MobileAotTarget::AndroidArm64,
        project_dir,
        entry_file,
        output_dir,
        &[],
        0,
        0,
    )?;
    let cmake_file = summary
        .cmake_file
        .ok_or_else(|| "Android mobile AOT bundle missing CMake file".to_string())?;
    Ok(AndroidAotBundleSummary {
        bundle_dir: summary.bundle_dir,
        object_count: summary.object_count,
        symbols_header: summary.symbols_header,
        bindings_source: summary.bindings_source,
        cmake_file,
        asset_dir: summary.asset_dir,
    })
}

fn write_mobile_aot_engine_bundle(
    target: MobileAotTarget,
    project_dir: &Path,
    entry_file: Option<&Path>,
    output_dir: &Path,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
) -> Result<MobileAotBundleSummary, String> {
    let mut process = AotProcess::with_optimization_profile(AotOptimizationProfile::SpeedAndSize);
    process.set_import_base_dir(project_dir);
    process.set_target(target.aot_target());
    process.set_profile_functions(profile_functions.iter().cloned())?;
    let sources = collect_mobile_aot_sources(project_dir, entry_file)?;
    for (path, source) in &sources {
        process.upsert_file(path.clone(), source.clone());
    }
    process
        .compile()
        .map_err(|error| format!("failed to compile mobile AOT bundle: {error:?}"))?;
    let snapshot = process
        .program_snapshot()
        .ok_or_else(|| "mobile AOT compile produced no ProgramSnapshot".to_string())?;
    let resolved = release_assets::resolve_snapshot_assets(project_dir, snapshot)?;
    let bundle = process.write_engine_bundle(&mobile_engine_entrypoints(), output_dir)?;
    let manifest = fs::read_to_string(&bundle.manifest_path).map_err(|error| {
        format!(
            "failed to read mobile AOT manifest {}: {error}",
            bundle.manifest_path.display()
        )
    })?;
    let manifest_json: serde_json::Value = serde_json::from_str(&manifest)
        .map_err(|error| format!("failed to parse mobile AOT manifest: {error}"))?;
    let symbols_header = output_dir.join("published_aot_symbols.h");
    write_mobile_aot_symbols_header(&manifest_json, &symbols_header)?;
    let bindings_source = output_dir.join("published_aot_bindings.c");
    write_mobile_aot_bindings_source_with_profile_and_assets(
        &manifest_json,
        &process.state_layout(),
        &resolved,
        &bindings_source,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
    )?;
    let cmake_file = if matches!(
        target,
        MobileAotTarget::AndroidArm64 | MobileAotTarget::AndroidX86_64
    ) {
        let path = output_dir.join("published_aot_objects.cmake");
        write_android_aot_cmake_file(&bundle.object_paths_by_function_id, &bindings_source, &path)?;
        Some(path)
    } else {
        None
    };
    let asset_dir = write_mobile_asset_bundle(target, output_dir, &resolved)?;
    let package_manifest = write_mobile_aot_package_manifest(
        target,
        &bundle.manifest_path,
        &manifest_json,
        &asset_dir,
        &bundle.object_paths_by_function_id,
        &symbols_header,
        &bindings_source,
        cmake_file.as_deref(),
        output_dir,
    )?;
    Ok(MobileAotBundleSummary {
        target,
        bundle_dir: bundle.output_dir,
        object_count: bundle.object_paths_by_function_id.len(),
        symbols_header,
        bindings_source,
        cmake_file,
        asset_dir,
        package_manifest,
    })
}

fn write_mobile_asset_bundle(
    target: MobileAotTarget,
    output_dir: &Path,
    resolved: &stasis_assets::ResolvedAssetManifest,
) -> Result<PathBuf, String> {
    let asset_root = output_dir.join(target.asset_root_dir());
    if asset_root.exists() {
        fs::remove_dir_all(&asset_root).map_err(|error| {
            format!(
                "failed to clear mobile AOT asset output {}: {error}",
                asset_root.display()
            )
        })?;
    }
    let game_root = asset_root.join("stasis_game");
    prepare_asset_bundle(
        resolved,
        &game_root,
        output_dir.join("asset-preparation-cache"),
    )
    .map_err(|error| format!("failed to prepare mobile AOT assets: {error}"))?;
    Ok(asset_root)
}

fn collect_mobile_aot_sources(
    project_dir: &Path,
    entry_file: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    if let Some(entry_file) = entry_file {
        return collect_mobile_aot_sources_from_entry(project_dir, entry_file);
    }
    let mut out = Vec::new();
    collect_mobile_aot_sources_inner(project_dir, project_dir, &mut out)?;
    out.sort_by(|left, right| left.0.cmp(&right.0));
    if out.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }
    Ok(out)
}

fn collect_mobile_aot_sources_from_entry(
    project_dir: &Path,
    entry_file: &Path,
) -> Result<Vec<(String, String)>, String> {
    let project_root = project_dir.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize mobile AOT project directory {}: {error}",
            project_dir.display()
        )
    })?;
    let entry_path = resolve_mobile_aot_project_file(&project_root, entry_file)?;
    let (graph, sources) = stasis_compiler::frontend::module_graph::load_project_module_graph(
        &project_root,
        &entry_path,
    )
    .map_err(|diagnostic| diagnostic.message)?;

    let mut out = Vec::new();
    for relative in graph.modules().keys() {
        if relative.ends_with(".test.stasis") {
            continue;
        }
        let source = sources
            .get(relative)
            .cloned()
            .ok_or_else(|| format!("module graph source missing for {relative}"))?;
        out.push((relative.clone(), source));
    }
    out.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(out)
}

fn resolve_mobile_aot_project_file(project_root: &Path, file: &Path) -> Result<PathBuf, String> {
    let candidate = if file.is_absolute() {
        file.to_path_buf()
    } else {
        project_root.join(file)
    };
    let canonical = candidate.canonicalize().map_err(|error| {
        format!(
            "failed to canonicalize mobile AOT entry file {}: {error}",
            candidate.display()
        )
    })?;
    if !canonical.starts_with(project_root) {
        return Err(format!(
            "mobile AOT entry file {} must stay under project directory {}",
            canonical.display(),
            project_root.display()
        ));
    }
    if canonical.extension().and_then(|value| value.to_str()) != Some("stasis") {
        return Err(format!(
            "mobile AOT entry file must be a .stasis file: {}",
            canonical.display()
        ));
    }
    Ok(canonical)
}

fn collect_mobile_aot_sources_inner(
    root: &Path,
    current: &Path,
    out: &mut Vec<(String, String)>,
) -> Result<(), String> {
    for entry in fs::read_dir(current)
        .map_err(|error| format!("failed to read directory {}: {error}", current.display()))?
    {
        let entry = entry.map_err(|error| format!("failed to read directory entry: {error}"))?;
        let path = entry.path();
        if path.is_dir() {
            if path
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name == "tests" || name == "build")
            {
                continue;
            }
            collect_mobile_aot_sources_inner(root, &path, out)?;
            continue;
        }
        if path.extension().and_then(|value| value.to_str()) != Some("stasis") {
            continue;
        }
        if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.ends_with(".test.stasis"))
        {
            continue;
        }
        let relative = path
            .strip_prefix(root)
            .map_err(|error| format!("failed to relativize {}: {error}", path.display()))?
            .to_string_lossy()
            .replace('\\', "/");
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        out.push((relative, source));
    }
    Ok(())
}

fn write_mobile_aot_symbols_header(
    manifest: &serde_json::Value,
    output_path: &Path,
) -> Result<(), String> {
    mobile_aot_function_for(manifest, "main")?;
    mobile_aot_function_for(manifest, "tick")?;
    mobile_aot_function_for(manifest, "render")?;
    let on_code_swap = mobile_aot_symbol_for(manifest, "on_code_swap").ok();
    let mut out = String::new();
    out.push_str("#pragma once\n\n#include <stdint.h>\n\n");
    out.push_str("extern void stasis_aot_bind_runtime_globals(void);\n");
    out.push_str("extern int32_t stasis_mobile_main_entry(void);\n");
    out.push_str("extern int32_t stasis_mobile_tick_entry(void);\n");
    out.push_str("extern int32_t stasis_mobile_render_entry(void);\n");
    if let Some(symbol) = on_code_swap.as_ref() {
        out.push_str(&format!("extern void {symbol}(void);\n"));
    }
    out.push_str("\n");
    out.push_str("#define STASIS_AOT_BIND_RUNTIME_GLOBALS stasis_aot_bind_runtime_globals\n");
    out.push_str("#define STASIS_AOT_MAIN stasis_mobile_main_entry\n");
    out.push_str("#define STASIS_AOT_TICK stasis_mobile_tick_entry\n");
    out.push_str("#define STASIS_AOT_RENDER stasis_mobile_render_entry\n");
    if let Some(symbol) = on_code_swap.as_ref() {
        out.push_str(&format!("#define STASIS_AOT_ON_CODE_SWAP {symbol}\n"));
    }
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create symbol header directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(output_path, out).map_err(|error| {
        format!(
            "failed to write mobile AOT symbol header {}: {error}",
            output_path.display()
        )
    })
}

fn mobile_aot_symbol_for(
    manifest: &serde_json::Value,
    function_name: &str,
) -> Result<String, String> {
    let functions = manifest
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Android AOT manifest missing functions array".to_string())?;
    functions
        .iter()
        .find(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(function_name))
        .and_then(|entry| entry.get("symbol").and_then(serde_json::Value::as_str))
        .map(str::to_string)
        .ok_or_else(|| format!("mobile AOT manifest missing symbol for function '{function_name}'"))
}

fn write_mobile_aot_package_manifest(
    target: MobileAotTarget,
    engine_manifest_path: &Path,
    engine_manifest: &serde_json::Value,
    asset_dir: &Path,
    object_paths_by_function_id: &std::collections::BTreeMap<u32, PathBuf>,
    symbols_header: &Path,
    bindings_source: &Path,
    cmake_file: Option<&Path>,
    output_dir: &Path,
) -> Result<PathBuf, String> {
    let manifest_functions = engine_manifest["functions"]
        .as_array()
        .ok_or_else(|| "mobile AOT engine manifest missing functions".to_string())?;
    let objects = object_paths_by_function_id
        .iter()
        .map(|(function_id, path)| {
            let name = manifest_functions
                .iter()
                .find(|function| function["function_id"].as_u64() == Some(u64::from(*function_id)))
                .and_then(|function| function["name"].as_str())
                .ok_or_else(|| {
                    format!("mobile AOT engine manifest missing function id {function_id}")
                })?;
            Ok(serde_json::json!({
                "function": name,
                "function_id": function_id,
                "path": mobile_aot_relative_path(output_dir, path)?
            }))
        })
        .collect::<Result<Vec<serde_json::Value>, String>>()?;
    let asset_manifest = asset_dir
        .join("stasis_game")
        .join(DEFAULT_ASSET_MANIFEST_PATH);
    let mut manifest = serde_json::json!({
        "schema": "stasis.mobile_aot_bundle.v1",
        "target": target.as_str(),
        "engine_manifest": mobile_aot_relative_path(output_dir, engine_manifest_path)?,
        "symbols_header": mobile_aot_relative_path(output_dir, symbols_header)?,
        "bindings_source": mobile_aot_relative_path(output_dir, bindings_source)?,
        "asset_root": mobile_aot_relative_path(output_dir, asset_dir)?,
        "asset_manifest": mobile_aot_relative_path(output_dir, &asset_manifest)?,
        "objects": objects,
        "entrypoints": {
            "main": "main",
            "tick": "tick",
            "render": "render",
            "on_code_swap": "on_code_swap"
        }
    });
    if let Some(cmake_file) = cmake_file {
        manifest["android_cmake_file"] =
            serde_json::Value::String(mobile_aot_relative_path(output_dir, cmake_file)?);
    }
    let path = output_dir.join("mobile_aot_bundle_manifest.json");
    let body = serde_json::to_string_pretty(&manifest)
        .map_err(|error| format!("failed to serialize mobile AOT package manifest: {error}"))?;
    fs::write(&path, body).map_err(|error| {
        format!(
            "failed to write mobile AOT package manifest {}: {error}",
            path.display()
        )
    })?;
    Ok(path)
}

fn mobile_aot_relative_path(output_dir: &Path, path: &Path) -> Result<String, String> {
    path.strip_prefix(output_dir)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| {
            format!(
                "mobile AOT metadata path {} is outside output directory {}",
                path.display(),
                output_dir.display()
            )
        })
}

fn write_android_aot_cmake_file(
    object_paths_by_function_id: &std::collections::BTreeMap<u32, PathBuf>,
    bindings_source: &Path,
    output_path: &Path,
) -> Result<(), String> {
    let output_dir = output_path.parent().ok_or_else(|| {
        format!(
            "Android AOT CMake output has no parent: {}",
            output_path.display()
        )
    })?;
    let mut out = String::new();
    out.push_str("set(STASIS_PUBLISHED_AOT_OBJECTS\n");
    for path in object_paths_by_function_id.values() {
        let relative = mobile_aot_relative_path(output_dir, path)?;
        out.push_str(&format!("  \"${{CMAKE_CURRENT_LIST_DIR}}/{relative}\"\n"));
    }
    let bindings_relative = mobile_aot_relative_path(output_dir, bindings_source)?;
    out.push_str(&format!(
        "  \"${{CMAKE_CURRENT_LIST_DIR}}/{bindings_relative}\"\n"
    ));
    out.push_str(")\n");
    fs::write(output_path, out).map_err(|error| {
        format!(
            "failed to write Android AOT CMake file {}: {error}",
            output_path.display()
        )
    })
}
fn try_run_aot_cli_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "aot-cli" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_aot_cli_contract_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };
    if let Some(path) = parsed.summary_file.as_ref() {
        if let Some(parent) = path.parent() {
            if let Err(error) = std::fs::create_dir_all(parent) {
                eprintln!(
                    "failed to create summary directory {}: {error}",
                    parent.display()
                );
                return Some(2);
            }
        }
    }
    let _ = parsed.quality_gate;
    let result = run_self_host_aot_cli_with_options(
        &parsed.project_dir,
        &parsed.output_exe,
        parsed.summary_file.as_deref(),
        parsed.entry_file.as_deref(),
    );

    match result {
        Ok(summary) => {
            println!("aot_cli_source_file_count={}", summary.source_file_count);
            println!("aot_cli_output_exe={}", summary.linked_image_path.display());
            println!("aot_cli_entry_symbol={}", summary.entry_symbol);
            Some(0)
        }
        Err(message) => {
            eprintln!("{message}");
            Some(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_assets::{load_project_asset_manifest, AssetLimits};
    use std::collections::BTreeSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn mobile_c_string_literal_escaping_is_byte_exact() {
        assert_eq!(
            escape_mobile_c_string_literal("a\0b\n\"\\\u{e9}"),
            "a\\000b\\n\\\"\\\\\\303\\251"
        );
    }

    fn temp_lookup_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("stasis_lookup_{name}_{stamp}"))
    }

    struct TemporaryWorkshopProject {
        root: PathBuf,
        project: PathBuf,
    }

    impl Drop for TemporaryWorkshopProject {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.root).ok();
        }
    }

    fn copy_test_tree(source: &Path, destination: &Path) {
        std::fs::create_dir_all(destination).expect("create copied test directory");
        for entry in std::fs::read_dir(source).expect("read test source directory") {
            let entry = entry.expect("read test source entry");
            let file_type = entry.file_type().expect("read test source entry type");
            let target = destination.join(entry.file_name());
            if file_type.is_dir() {
                copy_test_tree(&entry.path(), &target);
            } else if file_type.is_file() {
                std::fs::copy(entry.path(), target).expect("copy test source file");
            }
        }
    }

    fn materialize_workshop_project(name: &str) -> (PathBuf, TemporaryWorkshopProject) {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let root =
            std::env::temp_dir().join(format!("stasis_{name}_{}_{}", std::process::id(), stamp));
        let project = root.join("project");
        copy_test_tree(
            &repo_root.join("mobile/android/app/src/main/assets/workshop_sample"),
            &project,
        );
        copy_test_tree(
            &repo_root.join("src/stdlib"),
            &project.join("vendor/stasis/src/stdlib"),
        );
        (repo_root, TemporaryWorkshopProject { root, project })
    }

    fn retained_mobile_assets(
        project_dir: &Path,
        sources: &[(String, String)],
    ) -> stasis_assets::ResolvedAssetManifest {
        let mut process = AotProcess::new();
        process.set_import_base_dir(project_dir);
        for (path, source) in sources {
            process.upsert_file(path.clone(), source.clone());
        }
        process.compile().expect("compile mobile asset fixture");
        let resolved = load_project_asset_manifest(project_dir, AssetLimits::default())
            .expect("resolve mobile asset fixture");
        release_assets::retain_snapshot_assets(
            project_dir,
            process.program_snapshot().expect("program snapshot"),
            &resolved,
        )
        .expect("retain source-referenced mobile assets")
    }

    #[test]
    fn parse_test_cli_args_accepts_required_dir_flag() {
        let args = vec!["--dir".to_string(), "tests/stasis".to_string()];
        let parsed = parse_test_cli_args(&args).expect("parse should succeed");
        assert_eq!(parsed.directory, PathBuf::from("tests/stasis"));
        assert!(!parsed.watch);
        assert_eq!(parsed.watch_settle_ms, 0);
    }

    #[test]
    fn parse_play_cli_args_accepts_data_bind_paths() {
        let args = vec![
            "samples/bucket_catcher.stasis".to_string(),
            "--watch-dir".to_string(),
            "samples".to_string(),
            "--data-bind".to_string(),
            "samples/bucket_catcher/data/config.json".to_string(),
            "samples/bucket_catcher/data/config.struct-meta.json".to_string(),
        ];
        let parsed = parse_play_cli_args(&args).expect("parse should succeed");
        assert_eq!(
            parsed.watch_file,
            Some(PathBuf::from("samples/bucket_catcher.stasis"))
        );
        assert_eq!(parsed.watch_dir, Some(PathBuf::from("samples")));
        assert_eq!(
            parsed.data_bind_json,
            Some(PathBuf::from("samples/bucket_catcher/data/config.json"))
        );
        assert_eq!(
            parsed.data_bind_struct_meta,
            Some(PathBuf::from(
                "samples/bucket_catcher/data/config.struct-meta.json"
            ))
        );
    }

    #[test]
    fn parse_play_cli_args_accepts_selective_function_profiling() {
        let args = vec![
            "main.stasis".to_string(),
            "--ticks".to_string(),
            "240".to_string(),
            "--profile-functions".to_string(),
            "render,draw_board, draw_enemies".to_string(),
            "--profile-warmup".to_string(),
            "40".to_string(),
            "--profile-output".to_string(),
            "profile.json".to_string(),
        ];
        let parsed = parse_play_cli_args(&args).expect("parse profiling arguments");
        assert_eq!(
            parsed.profile_functions,
            vec!["render", "draw_board", "draw_enemies"]
        );
        assert_eq!(parsed.profile_warmup_ticks, 40);
        assert_eq!(parsed.profile_output, Some(PathBuf::from("profile.json")));
    }

    #[test]
    fn parse_play_cli_args_requires_a_measured_frame_after_profile_warmup() {
        let args = vec![
            "main.stasis".to_string(),
            "--ticks".to_string(),
            "60".to_string(),
            "--profile-functions".to_string(),
            "render".to_string(),
        ];
        assert!(parse_play_cli_args(&args)
            .expect_err("warmup consumes the bounded run")
            .contains("must exceed --profile-warmup"));
    }

    #[test]
    fn parse_play_cli_args_accepts_manifest_inferred_entry() {
        let parsed = parse_play_cli_args(&[]).expect("parse should succeed");
        assert_eq!(parsed.watch_file, None);
        assert_eq!(parsed.watch_dir, None);
    }

    #[test]
    fn play_launch_infers_manifest_entry_from_nested_directory() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project = env::temp_dir().join(format!("stasis_play_manifest_{stamp}"));
        let source_dir = project.join("src");
        let nested_dir = source_dir.join("nested");
        fs::create_dir_all(&nested_dir).expect("create project directories");
        fs::write(
            project.join("stasis.json"),
            r#"{"manifest_version":1,"name":"Manifest Game","entry":"src/main.stasis"}"#,
        )
        .expect("write manifest");
        fs::write(
            source_dir.join("main.stasis"),
            "function main(): i32 { return 0; }\n",
        )
        .expect("write entry");

        let parsed = parse_play_cli_args(&[]).expect("parse arguments");
        let resolved = resolve_play_launch(&parsed, &nested_dir).expect("resolve manifest entry");
        assert_eq!(
            resolved.watch_file,
            source_dir
                .join("main.stasis")
                .canonicalize()
                .expect("canonical entry")
        );
        assert_eq!(
            resolved.watch_dir,
            project.canonicalize().expect("canonical project")
        );
        assert_eq!(resolved.window_title.as_deref(), Some("Manifest Game"));
        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn explicit_nested_entry_still_uses_manifest_project_root() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project = env::temp_dir().join(format!("stasis_play_explicit_{stamp}"));
        let source_dir = project.join("src");
        fs::create_dir_all(&source_dir).expect("create source directory");
        fs::write(
            project.join("stasis.json"),
            r#"{"manifest_version":1,"name":"Explicit Game","entry":"src/main.stasis"}"#,
        )
        .expect("write manifest");
        fs::write(
            source_dir.join("main.stasis"),
            "function main(): i32 { return 0; }\n",
        )
        .expect("write entry");

        let parsed = parse_play_cli_args(&["main.stasis".to_string()]).expect("parse arguments");
        let resolved = resolve_play_launch(&parsed, &source_dir).expect("resolve explicit entry");
        assert_eq!(
            resolved.watch_dir,
            project.canonicalize().expect("canonical project")
        );
        assert_eq!(resolved.window_title.as_deref(), Some("Explicit Game"));
        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn explicit_external_entry_does_not_inherit_callers_manifest() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = env::temp_dir().join(format!("stasis_play_external_{stamp}"));
        let caller = root.join("caller");
        let external = root.join("external");
        fs::create_dir_all(&caller).expect("create caller directory");
        fs::create_dir_all(&external).expect("create external directory");
        fs::write(
            caller.join("stasis.json"),
            r#"{"manifest_version":1,"name":"Caller","entry":"main.stasis"}"#,
        )
        .expect("write caller manifest");
        fs::write(
            caller.join("main.stasis"),
            "function main(): i32 { return 0; }\n",
        )
        .expect("write caller entry");
        let external_entry = external.join("standalone.stasis");
        fs::write(&external_entry, "function main(): i32 { return 0; }\n")
            .expect("write external entry");

        let parsed = parse_play_cli_args(&[external_entry.display().to_string()])
            .expect("parse explicit entry");
        let resolved = resolve_play_launch(&parsed, &caller).expect("resolve external entry");
        assert_eq!(
            resolved.watch_dir,
            external
                .canonicalize()
                .expect("canonical external directory")
        );
        assert_eq!(resolved.window_title, None);
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn play_without_entry_or_manifest_reports_discovery_root() {
        let stamp = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let directory = env::temp_dir().join(format!("stasis_play_no_manifest_{stamp}"));
        fs::create_dir_all(&directory).expect("create directory");
        let parsed = parse_play_cli_args(&[]).expect("parse arguments");
        let error = resolve_play_launch(&parsed, &directory).expect_err("manifest is required");
        assert!(error.contains("missing entry file and no stasis.json found"));
        assert!(error.contains(&directory.display().to_string()));
        fs::remove_dir_all(&directory).ok();
    }

    #[test]
    fn parse_play_cli_args_rejects_incomplete_data_bind_flag() {
        let args = vec![
            "samples/bucket_catcher.stasis".to_string(),
            "--data-bind".to_string(),
            "samples/bucket_catcher/data/config.json".to_string(),
        ];
        let error = parse_play_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing values for --data-bind"));
    }

    #[test]
    fn parse_play_cli_args_accepts_input_script() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--input-script".to_string(),
            "qa/taps.json".to_string(),
        ];
        let parsed = parse_play_cli_args(&args).expect("parse should succeed");
        assert_eq!(parsed.input_script, Some(PathBuf::from("qa/taps.json")));
    }

    #[test]
    fn parse_play_cli_args_rejects_missing_input_script_path() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--input-script".to_string(),
        ];
        let error = parse_play_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing value for --input-script"));
    }

    #[test]
    fn parse_play_cli_args_accepts_record_and_replay_modes() {
        let record = parse_play_cli_args(&[
            "game.stasis".to_string(),
            "--record-replay".to_string(),
            "runs/game.replay.json".to_string(),
        ])
        .expect("record arguments");
        assert_eq!(
            record.replay_record,
            Some(PathBuf::from("runs/game.replay.json"))
        );

        let replay = parse_play_cli_args(&[
            "game.stasis".to_string(),
            "--replay".to_string(),
            "runs/game.replay.json".to_string(),
        ])
        .expect("replay arguments");
        assert_eq!(replay.replay, Some(PathBuf::from("runs/game.replay.json")));
    }

    #[test]
    fn parse_play_cli_args_rejects_ambiguous_replay_inputs() {
        let error = parse_play_cli_args(&[
            "game.stasis".to_string(),
            "--record-replay".to_string(),
            "one.json".to_string(),
            "--replay".to_string(),
            "two.json".to_string(),
        ])
        .expect_err("record and replay conflict");
        assert!(error.contains("cannot be combined"));

        let error = parse_play_cli_args(&[
            "game.stasis".to_string(),
            "--replay".to_string(),
            "one.json".to_string(),
            "--input-script".to_string(),
            "input.json".to_string(),
        ])
        .expect_err("replay and input script conflict");
        assert!(error.contains("--input-script"));
    }

    #[test]
    fn parse_play_cli_args_accepts_screenshot_configuration() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--screenshot".to_string(),
            "artifacts/frame-12.png".to_string(),
            "--screenshot-frame".to_string(),
            "12".to_string(),
            "--exit-after-screenshot".to_string(),
        ];
        let parsed = parse_play_cli_args(&args).expect("parse should succeed");
        assert_eq!(
            parsed.screenshot,
            Some(PathBuf::from("artifacts/frame-12.png"))
        );
        assert_eq!(parsed.screenshot_frame, 12);
        assert!(parsed.exit_after_screenshot);
    }

    #[test]
    fn parse_play_cli_args_rejects_zero_screenshot_frame() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--screenshot".to_string(),
            "frame.png".to_string(),
            "--screenshot-frame".to_string(),
            "0".to_string(),
        ];
        let error = parse_play_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("between 1 and 2147483647"));
    }

    #[test]
    fn parse_play_cli_args_requires_screenshot_for_capture_options() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--exit-after-screenshot".to_string(),
        ];
        let error = parse_play_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("require --screenshot"));
    }

    #[test]
    fn parse_play_cli_args_rejects_ticks_before_screenshot_frame() {
        let args = vec![
            "samples/brickout_revenge/brickout_revenge_v1.stasis".to_string(),
            "--ticks".to_string(),
            "11".to_string(),
            "--screenshot".to_string(),
            "frame.png".to_string(),
            "--screenshot-frame".to_string(),
            "12".to_string(),
        ];
        let error = parse_play_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("--ticks must be at least --screenshot-frame"));
    }

    #[test]
    fn parse_lookup_cli_args_accepts_search_alias_and_file_filter() {
        let args = vec![
            "s".to_string(),
            "signature".to_string(),
            "tick".to_string(),
            "--file".to_string(),
            "samples/example.stasis".to_string(),
        ];
        let parsed = parse_lookup_cli_args(&args)
            .expect("parse should succeed")
            .expect("lookup command");
        assert_eq!(parsed.mode, LookupMode::Signature);
        assert_eq!(parsed.query, "tick");
        assert_eq!(parsed.file, Some(PathBuf::from("samples/example.stasis")));
        assert_eq!(parsed.entry_file, None);
    }

    #[test]
    fn parse_lookup_cli_args_accepts_direct_definition_alias() {
        let args = vec!["definition".to_string(), "tick".to_string()];
        let parsed = parse_lookup_cli_args(&args)
            .expect("parse should succeed")
            .expect("lookup command");
        assert_eq!(parsed.mode, LookupMode::Definition);
        assert_eq!(parsed.query, "tick");
    }

    #[test]
    fn parse_lookup_cli_args_rejects_conflicting_scope_flags() {
        let args = vec![
            "search".to_string(),
            "sig".to_string(),
            "tick".to_string(),
            "--entry".to_string(),
            "entry.stasis".to_string(),
            "--file".to_string(),
            "only.stasis".to_string(),
        ];
        let error = parse_lookup_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("either --entry or --file"));
    }

    #[test]
    fn run_lookup_command_signature_scans_directory_and_skips_invalid_files() {
        let root = temp_lookup_root("signature_scope");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("alpha.stasis"),
            "function tick(): i32 { return 1; }\n",
        )
        .expect("write alpha");
        std::fs::write(
            root.join("beta.stasis"),
            "function helper_tick(value: i32): i32 { return value; }\n",
        )
        .expect("write beta");
        std::fs::write(
            root.join("broken.stasis"),
            "function tick_broken(): i32 { return 0;\n",
        )
        .expect("write broken");

        let parsed = LookupCliArgs {
            mode: LookupMode::Signature,
            query: "tick".to_string(),
            entry_file: None,
            file: None,
        };
        let output = run_lookup_command(&parsed, &root).expect("lookup should succeed");
        assert_eq!(
            output,
            "alpha.stasis\nfunction tick(): i32\n\nbeta.stasis\nfunction helper_tick(value: i32): i32\n\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_lookup_command_definition_entry_scans_import_closure_only() {
        let root = temp_lookup_root("entry_scope");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(
            root.join("entry.stasis"),
            "import \"./nested/helper.stasis\";\nfunction tick_main(): i32 { return helper_tick(); }\n",
        )
        .expect("write entry");
        std::fs::write(
            nested.join("helper.stasis"),
            "struct TickState {\n    value: i32;\n}\nfunction helper_tick(): i32 { return 7; }\n",
        )
        .expect("write helper");
        std::fs::write(
            root.join("unrelated.stasis"),
            "function tick_unrelated(): i32 { return 99; }\n",
        )
        .expect("write unrelated");

        let parsed = LookupCliArgs {
            mode: LookupMode::Definition,
            query: "tick".to_string(),
            entry_file: Some(PathBuf::from("entry.stasis")),
            file: None,
        };
        let output = run_lookup_command(&parsed, &root).expect("lookup should succeed");
        assert_eq!(
            output,
            "entry.stasis\nfunction tick_main(): i32 { return helper_tick(); }\n\nnested/helper.stasis\nfunction helper_tick(): i32 { return 7; }\n\nnested/helper.stasis\nstruct TickState {\n    value: i32;\n}\n\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_lookup_command_definition_entry_follows_bom_prefixed_imports() {
        let root = temp_lookup_root("entry_bom_scope");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(
            root.join("entry.stasis"),
            "\u{feff}import \"./nested/helper.stasis\";\nfunction tick_main(): i32 { return helper_tick(); }\n",
        )
        .expect("write entry");
        std::fs::write(
            nested.join("helper.stasis"),
            "function helper_tick(): i32 { return 7; }\n",
        )
        .expect("write helper");

        let parsed = LookupCliArgs {
            mode: LookupMode::Definition,
            query: "tick".to_string(),
            entry_file: Some(PathBuf::from("entry.stasis")),
            file: None,
        };
        let output = run_lookup_command(&parsed, &root).expect("lookup should succeed");
        assert_eq!(
            output,
            "entry.stasis\nfunction tick_main(): i32 { return helper_tick(); }\n\nnested/helper.stasis\nfunction helper_tick(): i32 { return 7; }\n\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[cfg(unix)]
    #[test]
    fn run_lookup_command_signature_skips_symlinked_directories() {
        use std::os::unix::fs::symlink;

        let root = temp_lookup_root("signature_symlink_scope");
        let nested = root.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(
            nested.join("helper.stasis"),
            "function tick_nested(): i32 { return 5; }\n",
        )
        .expect("write helper");
        symlink("./nested", root.join("nested_link")).expect("create dir symlink");

        let parsed = LookupCliArgs {
            mode: LookupMode::Signature,
            query: "tick".to_string(),
            entry_file: None,
            file: None,
        };
        let output = run_lookup_command(&parsed, &root).expect("lookup should succeed");
        assert_eq!(
            output,
            "nested/helper.stasis\nfunction tick_nested(): i32\n\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn run_lookup_command_file_scope_limits_results_to_one_file() {
        let root = temp_lookup_root("file_scope");
        std::fs::create_dir_all(&root).expect("mkdir");
        std::fs::write(
            root.join("chosen.stasis"),
            "function tick_local(): i32 { return 3; }\n",
        )
        .expect("write chosen");
        std::fs::write(
            root.join("other.stasis"),
            "function tick_other(): i32 { return 4; }\n",
        )
        .expect("write other");

        let parsed = LookupCliArgs {
            mode: LookupMode::Definition,
            query: "tick".to_string(),
            entry_file: None,
            file: Some(PathBuf::from("chosen.stasis")),
        };
        let output = run_lookup_command(&parsed, &root).expect("lookup should succeed");
        assert_eq!(
            output,
            "chosen.stasis\nfunction tick_local(): i32 { return 3; }\n\n"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parse_test_cli_args_rejects_missing_required_dir_flag() {
        let args = vec!["--ticks".to_string(), "10".to_string()];
        let error = parse_test_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing required --dir"));
    }

    #[test]
    fn parse_test_cli_args_accepts_watch_options() {
        let args = vec![
            "--dir".to_string(),
            "tests/stasis".to_string(),
            "--watch".to_string(),
            "--watch-settle-ms".to_string(),
            "250".to_string(),
        ];
        let parsed = parse_test_cli_args(&args).expect("parse should succeed");
        assert_eq!(parsed.directory, PathBuf::from("tests/stasis"));
        assert!(parsed.watch);
        assert_eq!(parsed.watch_settle_ms, 250);
    }

    #[test]
    fn parse_test_cli_args_rejects_invalid_watch_settle_value() {
        let args = vec![
            "--dir".to_string(),
            "tests/stasis".to_string(),
            "--watch-settle-ms".to_string(),
            "invalid".to_string(),
        ];
        let error = parse_test_cli_args(&args).expect_err("parse should fail");
        assert!(error.contains("invalid value for --watch-settle-ms"));
    }

    #[test]
    fn cleanup_stale_stasis_cache_removes_files_when_ttl_zero() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_cache_cleanup_zero_{}", stamp));
        let cache_root = root.join(".stasis_cache");
        let nested = cache_root.join("nested");
        std::fs::create_dir_all(&nested).expect("mkdir");
        let file = nested.join("stale.bin");
        std::fs::write(&file, "x").expect("write");

        let summary =
            cleanup_stale_stasis_cache(&cache_root, Duration::from_secs(0)).expect("cleanup");
        assert!(summary.removed_files >= 1, "{summary:?}");
        assert!(!file.exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn cleanup_stale_stasis_cache_keeps_recent_files_for_large_ttl() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_cache_cleanup_keep_{}", stamp));
        let cache_root = root.join(".stasis_cache");
        std::fs::create_dir_all(&cache_root).expect("mkdir");
        let file = cache_root.join("fresh.bin");
        std::fs::write(&file, "x").expect("write");

        let summary =
            cleanup_stale_stasis_cache(&cache_root, Duration::from_secs(365 * 24 * 60 * 60))
                .expect("cleanup");
        assert_eq!(summary.removed_files, 0, "{summary:?}");
        assert!(file.exists());

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn parse_android_aot_bundle_args_accepts_required_flags() {
        let args = vec![
            "--project-dir".to_string(),
            "mobile/android/app/src/main/assets/workshop_sample".to_string(),
            "--entry-file".to_string(),
            "src/main.stasis".to_string(),
            "--out-dir".to_string(),
            "target/android-aot".to_string(),
        ];
        let parsed = parse_android_aot_bundle_args(&args).expect("parse should succeed");
        assert_eq!(
            parsed.project_dir,
            PathBuf::from("mobile/android/app/src/main/assets/workshop_sample")
        );
        assert_eq!(parsed.entry_file, Some(PathBuf::from("src/main.stasis")));
        assert_eq!(parsed.output_dir, PathBuf::from("target/android-aot"));
    }

    #[test]
    fn parse_mobile_aot_bundle_args_accepts_ios_target() {
        let args = vec![
            "--target".to_string(),
            "ios-arm64".to_string(),
            "--project-dir".to_string(),
            "samples/brickout_revenge".to_string(),
            "--entry-file".to_string(),
            "src/main.stasis".to_string(),
            "--out-dir".to_string(),
            "target/mobile-aot".to_string(),
        ];
        let parsed = parse_mobile_aot_bundle_args(&args).expect("parse should succeed");
        assert_eq!(parsed.target, MobileAotTarget::IosArm64);
        assert_eq!(
            parsed.project_dir,
            PathBuf::from("samples/brickout_revenge")
        );
        assert_eq!(parsed.entry_file, Some(PathBuf::from("src/main.stasis")));
        assert_eq!(parsed.output_dir, PathBuf::from("target/mobile-aot"));
    }

    #[test]
    fn parse_mobile_aot_bundle_args_accepts_android_x86_64_target() {
        let args = vec![
            "--target".to_string(),
            "android-x86_64".to_string(),
            "--project-dir".to_string(),
            "samples/render_parity".to_string(),
            "--entry-file".to_string(),
            "main.stasis".to_string(),
            "--out-dir".to_string(),
            "target/android-x86_64-aot".to_string(),
        ];
        let parsed = parse_mobile_aot_bundle_args(&args).expect("parse should succeed");
        assert_eq!(parsed.target, MobileAotTarget::AndroidX86_64);
        assert_eq!(parsed.target.as_str(), "android-x86_64");
        assert_eq!(
            parsed.target.aot_target(),
            AotTarget::android_x86_64_default()
        );
        assert_eq!(parsed.target.asset_root_dir(), "apk_assets");
    }

    #[test]
    fn parse_mobile_aot_bundle_args_accepts_bounded_function_profile() {
        let args = vec![
            "--target".to_string(),
            "android-x86_64".to_string(),
            "--project-dir".to_string(),
            "samples/render_parity".to_string(),
            "--out-dir".to_string(),
            "target/profiled-mobile-aot".to_string(),
            "--profile-functions".to_string(),
            "render,draw_board".to_string(),
            "--profile-warmup-frames".to_string(),
            "30".to_string(),
            "--profile-sample-frames".to_string(),
            "90".to_string(),
        ];
        let parsed = parse_mobile_aot_bundle_args(&args).expect("parse profile flags");
        assert_eq!(parsed.profile_functions, ["render", "draw_board"]);
        assert_eq!(parsed.profile_warmup_frames, 30);
        assert_eq!(parsed.profile_sample_frames, 90);
    }

    #[test]
    fn android_aot_bundle_writes_pong_symbols_header() {
        use object::{Object, ObjectSymbol};

        let (repo_root, workshop) = materialize_workshop_project("android_aot_bundle");
        let output_dir = workshop.root.join("out");

        let summary = write_android_aot_engine_bundle(
            &workshop.project,
            Some(Path::new("src/main.stasis")),
            &output_dir,
        )
        .expect("write Android AOT bundle");

        assert!(summary.object_count >= 3, "expected lifecycle objects");
        let header = fs::read_to_string(&summary.symbols_header).expect("read symbols header");
        for entry in ["main", "tick", "render"] {
            assert!(header.contains(&format!(
                "extern int32_t stasis_mobile_{entry}_entry(void);"
            )));
            assert!(header.contains(&format!(
                "#define STASIS_AOT_{} stasis_mobile_{entry}_entry",
                entry.to_ascii_uppercase()
            )));
        }
        let mobile_runtime_header =
            fs::read_to_string(repo_root.join("runtime/stasis_mobile_runtime.h"))
                .expect("read mobile runtime header");
        assert!(mobile_runtime_header.contains("typedef int32_t (*StasisMobileI32Entry)(void)"));
        let bindings =
            fs::read_to_string(&summary.bindings_source).expect("read mobile AOT bindings source");
        assert!(bindings.contains("int32_t stasis_mobile_main_entry(void)"));
        assert!(bindings.contains("void stasis_aot_bind_runtime_globals(void)"));
        assert!(!bindings.contains("stasis_jit_register_code_ptr"));
        assert!(bindings.contains("stasis_published_sprite_handle_for_path"));
        assert!(bindings.contains("{\"assets/ball.svg\","));
        assert!(header.contains("#define STASIS_AOT_BIND_RUNTIME_GLOBALS"));
        let engine_manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(output_dir.join("engine_bundle_manifest.json"))
                .expect("read engine manifest"),
        )
        .expect("parse engine manifest");
        let (main_symbol, main_return_type) =
            mobile_aot_function_for(&engine_manifest, "main").expect("canonical main mapping");
        assert_eq!(main_return_type, 1);
        assert!(bindings.contains(&format!("extern int32_t {main_symbol}(void);")));
        assert!(bindings.contains(&format!(
            "stasis_mobile_main_entry(void) {{ return {main_symbol}(); }}"
        )));
        assert!(engine_manifest["functions"]
            .as_array()
            .expect("functions")
            .iter()
            .filter(|function| matches!(
                function["name"].as_str(),
                Some("main" | "tick" | "render")
            ))
            .all(|function| function["return_type"] == 1));
        let cmake = fs::read_to_string(&summary.cmake_file).expect("read cmake file");
        assert!(cmake.contains("set(STASIS_PUBLISHED_AOT_OBJECTS"));
        assert!(cmake.contains("${CMAKE_CURRENT_LIST_DIR}/"));
        assert!(!cmake.contains(&output_dir.to_string_lossy().replace('\\', "/")));
        assert!(cmake.contains("published_aot_bindings.c"));
        let package_manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(output_dir.join("mobile_aot_bundle_manifest.json"))
                .expect("read package manifest"),
        )
        .expect("parse package manifest");
        assert_eq!(
            package_manifest["engine_manifest"],
            "engine_bundle_manifest.json"
        );
        assert_eq!(
            package_manifest["symbols_header"],
            "published_aot_symbols.h"
        );
        assert_eq!(
            package_manifest["bindings_source"],
            "published_aot_bindings.c"
        );
        assert_eq!(package_manifest["asset_root"], "apk_assets");
        assert_eq!(
            package_manifest["asset_manifest"],
            "apk_assets/stasis_game/assets/manifest.json"
        );
        assert_eq!(
            package_manifest["android_cmake_file"],
            "published_aot_objects.cmake"
        );
        assert!(package_manifest["objects"]
            .as_array()
            .expect("objects")
            .iter()
            .all(|entry| entry["path"]
                .as_str()
                .is_some_and(|path| !Path::new(path).is_absolute())));

        let mut defined = BTreeSet::new();
        let mut undefined = BTreeSet::new();
        for entry in fs::read_dir(&summary.bundle_dir).expect("read bundle directory") {
            let path = entry.expect("bundle entry").path();
            if path.extension().and_then(|value| value.to_str()) != Some("o") {
                continue;
            }
            let bytes = fs::read(&path).expect("read AOT object");
            let object = object::File::parse(bytes.as_slice()).expect("parse AOT object");
            for symbol in object.symbols() {
                let Ok(name) = symbol.name() else { continue };
                if symbol.is_undefined() {
                    undefined.insert(name.to_string());
                } else {
                    defined.insert(name.to_string());
                }
            }
        }
        let mobile_aot_runtime =
            fs::read_to_string(repo_root.join("runtime/stasis_mobile_aot_runtime.c"))
                .expect("read shared mobile AOT runtime");
        let mobile_aot_header =
            fs::read_to_string(repo_root.join("runtime/stasis_mobile_aot_runtime.h"))
                .expect("read shared mobile AOT runtime header");
        let graphics_runtime = fs::read_to_string(repo_root.join("runtime/stasis_graphics.c"))
            .expect("read graphics runtime");
        let generated_bindings =
            fs::read_to_string(summary.bundle_dir.join("published_aot_bindings.c"))
                .expect("read generated AOT bindings");
        let missing: Vec<_> = undefined
            .difference(&defined)
            .filter(|symbol| {
                !mobile_aot_runtime.contains(&format!("{symbol}("))
                    && !mobile_aot_header.contains(&format!("{symbol}("))
                    && !graphics_runtime.contains(&format!("{symbol}("))
                    && !generated_bindings.contains(symbol.as_str())
            })
            .cloned()
            .collect();
        assert!(
            missing.is_empty(),
            "mobile runtime or generated bindings must provide AOT imports: {missing:?}"
        );
        let runtime_exports = fs::read_to_string(
            repo_root.join("crates/stasis_compiler/src/backend/runtime_exports.rs"),
        )
        .expect("read compiler AOT runtime exports");
        let unsupported_exports: Vec<_> = runtime_exports
            .lines()
            .map(str::trim)
            .filter_map(|line| line.strip_prefix('"')?.strip_suffix("\","))
            .filter(|symbol| {
                !mobile_aot_runtime.contains(&format!("{symbol}("))
                    && !mobile_aot_header.contains(&format!("{symbol}("))
                    && !graphics_runtime.contains(&format!("{symbol}("))
            })
            .collect();
        assert!(
            unsupported_exports.is_empty(),
            "shared mobile core must cover every compiler AOT import: {unsupported_exports:?}"
        );
        assert!(summary
            .asset_dir
            .join("stasis_game/assets/manifest.json")
            .is_file());
        assert!(summary
            .asset_dir
            .join("stasis_game/assets/ball.svg")
            .is_file());
    }

    #[test]
    fn mobile_aot_entry_file_ignores_unimported_invalid_source() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_dir = std::env::temp_dir().join(format!("stasis_mobile_entry_{stamp}"));
        let src_dir = project_dir.join("src");
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::write(
            src_dir.join("main.stasis"),
            "import \"helper.stasis\";\nfunction main(): i32 { return helper(); }\n",
        )
        .expect("write main");
        std::fs::write(
            src_dir.join("helper.stasis"),
            "function helper(): i32 { return 7; }\n",
        )
        .expect("write helper");
        std::fs::write(
            src_dir.join("stray.stasis"),
            "function nope(: i32 { return 0; }\n",
        )
        .expect("write stray");

        let sources = collect_mobile_aot_sources(&project_dir, Some(Path::new("src/main.stasis")))
            .expect("collect entry graph");
        let paths: Vec<&str> = sources.iter().map(|(path, _)| path.as_str()).collect();

        assert_eq!(paths, vec!["src/helper.stasis", "src/main.stasis"]);

        std::fs::remove_dir_all(&project_dir).ok();
    }

    #[test]
    fn mobile_asset_bundle_copies_only_source_referenced_project_fonts() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_mobile_fonts_{stamp}"));
        let project_dir = root.join("project");
        let src_dir = project_dir.join("src");
        let font_dir = project_dir.join("assets/fonts");
        let output_dir = root.join("out");
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::create_dir_all(&font_dir).expect("mkdir fonts");
        std::fs::write(font_dir.join("ui.ttf"), b"referenced font").expect("write font");
        std::fs::write(font_dir.join("unused.ttf"), b"unused font").expect("write font");
        std::fs::write(
            project_dir.join("assets/manifest.json"),
            format!(
                r#"{{
  "schema": "stasis-assets",
  "version": 2,
  "assets": [
    {{"id":"ui","path":"assets/fonts/ui.ttf","content_sha256":"{}","format":{{"kind":"font","encoding":"ttf"}},"dependencies":[]}},
    {{"id":"unused","path":"assets/fonts/unused.ttf","content_sha256":"{}","format":{{"kind":"font","encoding":"ttf"}},"dependencies":[]}}
  ]
}}
"#,
                stasis_assets::sha256_bytes(b"referenced font"),
                stasis_assets::sha256_bytes(b"unused font")
            ),
        )
        .expect("write manifest");
        let sources = vec![(
            "src/main.stasis".to_string(),
            r#"extern function load_font(path: string, size: i32): i32;
function main(): i32 { load_font("../assets/fonts/ui.ttf", 16); return 0; }
"#
            .to_string(),
        )];

        let resolved = retained_mobile_assets(&project_dir, &sources);
        let asset_root =
            write_mobile_asset_bundle(MobileAotTarget::AndroidArm64, &output_dir, &resolved)
                .expect("package font asset");

        assert!(asset_root.join("stasis_game/assets/fonts/ui.ttf").is_file());
        assert!(!asset_root
            .join("stasis_game/assets/fonts/unused.ttf")
            .exists());
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mobile_asset_bundle_packages_only_reachable_manifest_assets_for_both_targets() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_mobile_asset_closure_{stamp}"));
        let project_dir = root.join("project");
        let src_dir = project_dir.join("src");
        let svg_dir = project_dir.join("assets/svg");
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::create_dir_all(&svg_dir).expect("mkdir svg");
        let used_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"/>"#;
        let unused_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><path d="M0 0"/></svg>"#;
        std::fs::write(svg_dir.join("used.svg"), used_svg).expect("write used svg");
        std::fs::write(svg_dir.join("unused.svg"), unused_svg).expect("write unused svg");
        std::fs::write(
            project_dir.join("assets/manifest.json"),
            format!(
                r#"{{
  "schema": "stasis-assets",
  "version": 2,
  "assets": [
    {{"id":"used","path":"assets/svg/used.svg","content_sha256":"{}","format":{{"kind":"sprite","encoding":"svg","width":32,"height":32}},"dependencies":[]}},
    {{"id":"unused","path":"assets/svg/unused.svg","content_sha256":"{}","format":{{"kind":"sprite","encoding":"svg","width":32,"height":32}},"dependencies":[]}}
  ]
}}
"#,
                stasis_assets::sha256_bytes(used_svg),
                stasis_assets::sha256_bytes(unused_svg)
            ),
        )
        .expect("write manifest");
        let sources = vec![(
            "src/main.stasis".to_string(),
            r#"struct Sprite { handle: i32; width: i32; height: i32; }
global hero: Sprite;
function @extern("stasis_jit_sprite_load_from") load_sprite_from(self: Sprite, path: string, width: i32, height: i32): bool;
function main(): void { hero.load_sprite_from("../assets/svg/used.svg", 32, 32); }
"#
            .to_string(),
        )];

        for target in [MobileAotTarget::AndroidArm64, MobileAotTarget::IosArm64] {
            let output_dir = root.join(target.as_str());
            let resolved = retained_mobile_assets(&project_dir, &sources);
            let asset_root = write_mobile_asset_bundle(target, &output_dir, &resolved)
                .expect("package filtered assets");
            let game_root = asset_root.join("stasis_game");
            assert!(game_root.join("assets/svg/used.svg").is_file());
            assert!(!game_root.join("assets/svg/unused.svg").exists());
            let packaged: serde_json::Value = serde_json::from_slice(
                &std::fs::read(game_root.join("assets/manifest.json"))
                    .expect("read packaged manifest"),
            )
            .expect("parse packaged manifest");
            assert_eq!(packaged["assets"].as_array().expect("assets").len(), 1);
            assert_eq!(packaged["assets"][0]["id"], "used");
        }
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mobile_aot_packages_inferred_static_svg_without_source_manifest() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_mobile_inferred_{stamp}"));
        let project_dir = root.join("project");
        let src_dir = project_dir.join("src");
        let svg_dir = project_dir.join("assets/svg");
        let output_dir = root.join("out");
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::create_dir_all(&svg_dir).expect("mkdir svg");
        let used_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="37" height="19"><path d="M0 0"/></svg>"#;
        let unused_svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="5" height="7"/>"#;
        std::fs::write(svg_dir.join("used.svg"), used_svg).expect("write used svg");
        std::fs::write(svg_dir.join("unused.svg"), unused_svg).expect("write unused svg");
        std::fs::write(
            src_dir.join("main.stasis"),
            r#"struct Sprite { handle: i32; width: i32; height: i32; }
global hero: Sprite;
function @extern("stasis_jit_sprite_load_from") load_sprite_from(self: Sprite, path: string, width: i32, height: i32): bool;
function main(): i32 { if (hero.load_sprite_from("../assets/svg/used.svg", 37, 19)) { return hero.handle; } return 0; }
function tick(): i32 { return 0; }
function render(): i32 { return 0; }
"#,
        )
        .expect("write source");

        let summary = write_mobile_aot_engine_bundle(
            MobileAotTarget::IosArm64,
            &project_dir,
            Some(Path::new("src/main.stasis")),
            &output_dir,
            &[],
            0,
            0,
        )
        .expect("package inferred mobile assets");

        assert!(!project_dir.join("assets/manifest.json").exists());
        let game_root = summary.asset_dir.join("stasis_game");
        assert!(game_root.join("assets/svg/used.svg").is_file());
        assert!(!game_root.join("assets/svg/unused.svg").exists());
        let packaged: stasis_assets::AssetManifest = serde_json::from_slice(
            &std::fs::read(game_root.join("assets/manifest.json"))
                .expect("read generated manifest"),
        )
        .expect("parse generated manifest");
        assert_eq!(packaged.assets.len(), 1);
        let entry = &packaged.assets[0];
        assert_eq!(entry.path, "assets/svg/used.svg");
        assert_eq!(entry.content_sha256, stasis_assets::sha256_bytes(used_svg));
        assert_eq!(
            entry.format,
            stasis_assets::AssetFormat::Sprite {
                encoding: stasis_assets::SpriteEncoding::Svg,
                width: 37,
                height: 19,
                layout: None,
            }
        );
        let expected_handle = stasis_assets::stable_asset_handle(entry).as_i32();
        let bindings = std::fs::read_to_string(&summary.bindings_source).expect("read bindings");
        assert!(bindings.contains(&format!("{{\"assets/svg/used.svg\", {expected_handle}}}")));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn mobile_aot_bundle_rejects_cyclic_import_graph() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_dir = std::env::temp_dir().join(format!("stasis_mobile_imports_{stamp}"));
        let src_dir = project_dir.join("src");
        let asset_dir = project_dir.join("assets");
        let output_dir = std::env::temp_dir().join(format!("stasis_mobile_imports_out_{stamp}"));
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::create_dir_all(&asset_dir).expect("mkdir assets");
        std::fs::write(
            src_dir.join("main.stasis"),
            r#"import "window.stasis";
function main(): i32 { return 0; }
function tick(): i32 { return 0; }
function render(): i32 { return window_width(); }
"#,
        )
        .expect("write main");
        std::fs::write(
            src_dir.join("window.stasis"),
            r#"import "frame.stasis";
function window_width(): i32 { return frame_width(); }
"#,
        )
        .expect("write window");
        std::fs::write(
            src_dir.join("frame.stasis"),
            r#"import "window.stasis";
function frame_width(): i32 { return 360; }
"#,
        )
        .expect("write frame");
        std::fs::write(
            asset_dir.join("manifest.json"),
            r#"{
  "schema": "stasis-assets",
  "version": 1,
  "assets": []
}
"#,
        )
        .expect("write manifest");

        let error = match write_mobile_aot_engine_bundle(
            MobileAotTarget::AndroidArm64,
            &project_dir,
            Some(Path::new("src/main.stasis")),
            &output_dir,
            &[],
            0,
            0,
        ) {
            Ok(_) => panic!("cyclic imports must be rejected by the compiler graph"),
            Err(error) => error,
        };
        assert!(error
            .contains("import cycle: src/window.stasis -> src/frame.stasis -> src/window.stasis"));

        std::fs::remove_dir_all(&project_dir).ok();
        std::fs::remove_dir_all(&output_dir).ok();
    }

    #[test]
    fn mobile_aot_bundle_allows_missing_on_code_swap() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project_dir = std::env::temp_dir().join(format!("stasis_mobile_no_hook_{stamp}"));
        let src_dir = project_dir.join("src");
        let asset_dir = project_dir.join("assets");
        let output_dir = std::env::temp_dir().join(format!("stasis_mobile_no_hook_out_{stamp}"));
        std::fs::create_dir_all(&src_dir).expect("mkdir src");
        std::fs::create_dir_all(&asset_dir).expect("mkdir assets");
        std::fs::write(
            src_dir.join("main.stasis"),
            "function main(): i32 { return 0; }\nfunction tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
        )
        .expect("write main");
        std::fs::write(
            asset_dir.join("manifest.json"),
            "{\n  \"schema\": \"stasis-assets\",\n  \"version\": 1,\n  \"assets\": []\n}\n",
        )
        .expect("write manifest");

        let summary = write_mobile_aot_engine_bundle(
            MobileAotTarget::IosArm64,
            &project_dir,
            Some(Path::new("src/main.stasis")),
            &output_dir,
            &[],
            0,
            0,
        )
        .expect("missing on_code_swap should be accepted for mobile");
        let header = fs::read_to_string(&summary.symbols_header).expect("read symbols header");
        let bindings = fs::read_to_string(&summary.bindings_source).expect("read bindings source");
        let manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(output_dir.join("engine_bundle_manifest.json"))
                .expect("read engine manifest"),
        )
        .expect("parse engine manifest");
        let (main_symbol, main_return_type) =
            mobile_aot_function_for(&manifest, "main").expect("canonical main mapping");

        assert!(header.contains("#define STASIS_AOT_MAIN stasis_mobile_main_entry"));
        assert!(header.contains("#define STASIS_AOT_TICK stasis_mobile_tick_entry"));
        assert!(header.contains("#define STASIS_AOT_RENDER stasis_mobile_render_entry"));
        assert_eq!(main_return_type, 1);
        assert!(bindings.contains(&format!(
            "stasis_mobile_main_entry(void) {{ return {main_symbol}(); }}"
        )));
        assert!(!header.contains("STASIS_AOT_ON_CODE_SWAP"));

        std::fs::remove_dir_all(&project_dir).ok();
        std::fs::remove_dir_all(&output_dir).ok();
    }

    #[test]
    fn mobile_aot_bundle_writes_ios_arm64_artifact_manifest() {
        let (_repo_root, workshop) = materialize_workshop_project("ios_aot_bundle");
        let output_dir = workshop.root.join("out");

        let summary = write_mobile_aot_engine_bundle(
            MobileAotTarget::IosArm64,
            &workshop.project,
            Some(Path::new("src/main.stasis")),
            &output_dir,
            &[],
            0,
            0,
        )
        .expect("write iOS mobile AOT bundle");

        assert_eq!(summary.target, MobileAotTarget::IosArm64);
        assert!(summary.object_count >= 3, "expected lifecycle objects");
        assert!(
            summary.cmake_file.is_none(),
            "iOS should not emit Android CMake glue"
        );
        assert!(summary
            .asset_dir
            .join("stasis_game/assets/manifest.json")
            .is_file());
        let package_manifest: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(&summary.package_manifest).expect("read package manifest"),
        )
        .expect("parse package manifest");
        assert_eq!(package_manifest["target"], "ios-arm64");
        assert_eq!(package_manifest["schema"], "stasis.mobile_aot_bundle.v1");
        assert_eq!(
            package_manifest["engine_manifest"],
            "engine_bundle_manifest.json"
        );
        assert_eq!(package_manifest["asset_root"], "ios_assets");
        assert_eq!(
            package_manifest["asset_manifest"],
            "ios_assets/stasis_game/assets/manifest.json"
        );
        assert!(package_manifest["objects"]
            .as_array()
            .expect("objects")
            .iter()
            .all(|entry| entry["path"]
                .as_str()
                .is_some_and(|path| { path.ends_with(".o") && !Path::new(path).is_absolute() })));
        let header = fs::read_to_string(&summary.symbols_header).expect("read symbols header");
        assert!(header.contains("#define STASIS_AOT_MAIN stasis_mobile_main_entry"));
    }
    #[test]
    fn parse_aot_cli_contract_args_accepts_required_flags() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(parsed.project_dir, PathBuf::from("proj"));
        assert_eq!(parsed.output_exe, PathBuf::from("bin.exe"));
        assert_eq!(parsed.summary_file, None);
        assert_eq!(parsed.entry_file, None);
        assert!(!parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_summary_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--summary-file".to_string(),
            "summary.json".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(parsed.summary_file, Some(PathBuf::from("summary.json")));
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_entry_file_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--entry-file".to_string(),
            "entry.stasis".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert_eq!(parsed.entry_file, Some(PathBuf::from("entry.stasis")));
        assert!(!parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_accepts_quality_gate_flag() {
        let args = vec![
            "--project-dir".to_string(),
            "proj".to_string(),
            "--out".to_string(),
            "bin.exe".to_string(),
            "--quality-gate".to_string(),
        ];
        let parsed = parse_aot_cli_contract_args(&args).expect("parse should succeed");
        assert!(parsed.quality_gate);
    }

    #[test]
    fn parse_aot_cli_contract_args_rejects_missing_required_flags() {
        let args = vec!["--project-dir".to_string(), "proj".to_string()];
        let error = parse_aot_cli_contract_args(&args).expect_err("parse should fail");
        assert!(error.contains("missing required --out"));
    }

    #[test]
    fn aot_cli_host_glue_stays_out_of_compile_orchestration() {
        let source = include_str!("main.rs");
        let forbidden_collect = ["collect_stasis_files_recursive", "("].concat();
        let forbidden_compile = ["compile_changed_files", "("].concat();
        assert!(
            !source.contains(&forbidden_collect),
            "aot-cli host should not enumerate project sources directly"
        );
        assert!(
            !source.contains(&forbidden_compile),
            "aot-cli host should not run compiler frontend directly"
        );
        assert!(
            source.contains("run_self_host_aot_cli_with_options("),
            "aot-cli host should delegate through the library AOT entrypoint"
        );
    }
}

fn main() {
    if !toolchain_cli::should_skip_stale_stasis_cache_cleanup() {
        maybe_cleanup_stale_stasis_cache();
    }

    if let Some(exit) = toolchain_cli::try_run() {
        std::process::exit(exit);
    }

    if let Some(exit) = try_run_probe_graphics_runtime_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_lookup_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_test_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_mobile_aot_bundle_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_android_aot_bundle_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_aot_cli_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_play_subcommand() {
        std::process::exit(exit);
    }

    let options = parse_args();
    let use_simulated = options.runner.fail_compile
        || options.runner.hook_failure_reason.is_some()
        || options.runner.swap_failure_reason.is_some();
    let summary = if use_simulated {
        run_with_default_backend(options.runner)
    } else {
        run_with_real_backend(options.runner)
    };
    let exit_code = if summary.compile_failures > 0 || summary.swap_commit_failures > 0 {
        1
    } else {
        0
    };

    if options.emit_events_jsonl {
        if let Err(err) = write_events_jsonl(&summary, options.events_jsonl_file.as_deref()) {
            eprintln!("{err}");
            std::process::exit(2);
        }
        std::process::exit(exit_code);
    }

    println!("ticks_executed={}", summary.ticks_executed);
    println!("compile_successes={}", summary.compile_successes);
    println!("compile_failures={}", summary.compile_failures);
    println!("hook_runs={}", summary.hook_runs);
    println!("hook_failures={}", summary.hook_failures);
    println!("swap_commit_successes={}", summary.swap_commit_successes);
    println!("swap_commit_failures={}", summary.swap_commit_failures);
    println!(
        "swap_indicator_armed_count={}",
        summary.swap_indicator_armed_count
    );
    println!("swap_flash_peak_ticks={}", summary.swap_flash_peak_ticks);
    println!(
        "swap_flash_ticks_remaining={}",
        summary.swap_flash_ticks_remaining
    );
    if let Some(compile_ms) = summary.last_compile_duration_ms {
        println!("last_compile_duration_ms={compile_ms}");
    }
    if let Some(commit_ms) = summary.last_commit_duration_ms {
        println!("last_commit_duration_ms={commit_ms}");
    }
    println!("has_in_flight_work={}", summary.has_in_flight_work);
    println!("runtime_launches={}", summary.runtime_launches);
    println!(
        "runtime_launch_failures={}",
        summary.runtime_launch_failures
    );
    println!(
        "aot_linked_image_activations={}",
        summary.aot_linked_image_activations
    );
    if let Some(path) = summary.active_aot_linked_image_path.as_ref() {
        println!("active_aot_linked_image_path={}", path.display());
    }
    if let Some(size) = summary.active_aot_linked_image_size_bytes {
        println!("active_aot_linked_image_size_bytes={size}");
    }
    if let Some(generation) = summary.active_aot_linked_image_generation {
        println!("active_aot_linked_image_generation={generation}");
    }
    println!(
        "retired_aot_linked_images={}",
        summary.retired_aot_linked_images
    );
    if let Some(window) = summary.window {
        println!(
            "window_profile={}x{} vertical={}",
            window.width,
            window.height,
            window.is_vertical()
        );
    }
    for diagnostic in &summary.compile_diagnostics {
        println!("compile_diagnostic={diagnostic}");
    }
    for reason in &summary.swap_failure_reasons {
        println!("swap_failure_reason={reason}");
    }
    for reason in &summary.hook_failure_reasons {
        println!("hook_failure_reason={reason}");
    }
    for reason in &summary.runtime_launch_failure_reasons {
        println!("runtime_launch_failure_reason={reason}");
    }

    std::process::exit(exit_code);
}
