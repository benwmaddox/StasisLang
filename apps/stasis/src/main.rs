#![cfg_attr(not(debug_assertions), deny(warnings))]

use std::collections::BTreeSet;
use std::env;
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::Path;
use std::path::PathBuf;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::SystemTime;
use std::time::{Duration, Instant};

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use stasis::{
    run_jit_tests_in_directory_with_session, run_play_in_process,
    run_self_host_aot_cli_with_options, run_with_default_backend, run_with_real_backend,
    RunnerConfig, StasisTestRunSession,
};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::{AotOptimizationProfile, EngineEntrypoints};
use stasis_compiler::frontend::parser::{
    parse_top_level_functions, parse_top_level_struct_definitions,
};
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
    output_dir: PathBuf,
    entry_file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PlayCliArgs {
    watch_file: PathBuf,
    watch_dir: Option<PathBuf>,
    data_bind_json: Option<PathBuf>,
    data_bind_struct_meta: Option<PathBuf>,
    tick_sleep_micros: u64,
    ticks: Option<u64>,
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

fn parse_lookup_import_paths(source: &str) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    for line in source.lines() {
        let trimmed = line.trim_start();
        if !trimmed.starts_with("import ") {
            continue;
        }
        let Some(first_quote) = trimmed.find('"') else {
            continue;
        };
        let rest = &trimmed[first_quote + 1..];
        let Some(second_quote_rel) = rest.find('"') else {
            continue;
        };
        let candidate = &rest[..second_quote_rel];
        let path = PathBuf::from(candidate);
        if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
        {
            out.push(path);
        }
    }
    out
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

    let mut queue: Vec<PathBuf> = vec![entry_canonical];
    let mut visited: BTreeSet<PathBuf> = BTreeSet::new();
    while let Some(path) = queue.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let parent = path.parent().unwrap_or(&root_canonical);
        for import_path in parse_lookup_import_paths(&source) {
            let candidate = parent.join(import_path);
            if !candidate.exists() {
                continue;
            }
            let canonical = candidate.canonicalize().map_err(|error| {
                format!("failed to canonicalize {}: {error}", candidate.display())
            })?;
            if canonical.starts_with(&root_canonical) {
                queue.push(canonical);
            }
        }
    }

    let mut files: Vec<PathBuf> = visited.into_iter().collect();
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
        .display()
        .to_string()
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

    if let Ok(functions) = parse_top_level_functions(source) {
        for function in functions {
            if !lookup_name_matches(&function.name, &query_lower) {
                continue;
            }
            let text = match parsed.mode {
                LookupMode::Signature => source.get(function.signature_range.clone()),
                LookupMode::Definition => {
                    source.get(function.signature_range.start..function.body_range.end)
                }
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
        if let Ok(structs) = parse_top_level_struct_definitions(source) {
            for definition in structs {
                if !lookup_name_matches(&definition.name, &query_lower) {
                    continue;
                }
                if let Some(text) = source.get(definition.definition_range.clone()) {
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
    let mut tick_sleep_micros: u64 = 16000;
    let mut ticks: Option<u64> = None;
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
                    "missing values for --data-bind <json_path> <struct_meta_path>".to_string(),
                );
            }
            data_bind_json = Some(PathBuf::from(args[i + 1].clone()));
            data_bind_struct_meta = Some(PathBuf::from(args[i + 2].clone()));
            i += 3;
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
        i += 1;
    }
    let Some(watch_file) = watch_file else {
        return Err(
            "missing entry file. Use `stasis.exe play <path.stasis>` (or --watch-file <path>)"
                .to_string(),
        );
    };
    Ok(PlayCliArgs {
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        tick_sleep_micros,
        ticks,
    })
}

fn try_run_play_subcommand() -> Option<i32> {
    let mut args = env::args().skip(1);
    let first = args.next()?;
    if first != "play" {
        return None;
    }
    let arg_list: Vec<String> = args.collect();
    let parsed = match parse_play_cli_args(&arg_list) {
        Ok(value) => value,
        Err(message) => {
            eprintln!("{message}");
            return Some(2);
        }
    };

    match run_play_in_process(
        &parsed.watch_file,
        parsed.watch_dir.as_deref(),
        parsed.data_bind_json.as_deref(),
        parsed.data_bind_struct_meta.as_deref(),
        parsed.tick_sleep_micros,
        parsed.ticks,
    ) {
        Ok(()) => Some(0),
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

    if !cfg!(windows) {
        eprintln!("probe-graphics-runtime is only supported on Windows today");
        return Some(2);
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
                println!("cache_cleanup_removed_files={}", summary.removed_files);
                println!("cache_cleanup_removed_dirs={}", summary.removed_dirs);
                println!("cache_cleanup_ttl_days={ttl_days}");
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
    let mut output_dir: Option<PathBuf> = None;
    let mut entry_file: Option<PathBuf> = None;
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
            "--out-dir" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --out-dir".to_string());
                }
                output_dir = Some(PathBuf::from(args[i + 1].clone()));
                i += 2;
            }
            "--entry-file" => {
                if i + 1 >= args.len() {
                    return Err("missing value for --entry-file".to_string());
                }
                entry_file = Some(PathBuf::from(args[i + 1].clone()));
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
        output_dir,
        entry_file,
    })
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
        &parsed.output_dir,
        parsed.entry_file.as_deref(),
    ) {
        Ok(summary) => {
            println!("android_aot_bundle_dir={}", summary.bundle_dir.display());
            println!("android_aot_object_count={}", summary.object_count);
            println!(
                "android_aot_symbols_header={}",
                summary.symbols_header.display()
            );
            println!("android_aot_cmake_file={}", summary.cmake_file.display());
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
    cmake_file: PathBuf,
}

fn write_android_aot_engine_bundle(
    project_dir: &Path,
    output_dir: &Path,
    entry_file: Option<&Path>,
) -> Result<AndroidAotBundleSummary, String> {
    let mut process = AotProcess::with_optimization_profile(AotOptimizationProfile::SpeedAndSize);
    process.set_target(AotTarget::android_arm64_default());
    for (path, source) in collect_android_aot_sources(project_dir, entry_file)? {
        process.upsert_file(path, source);
    }
    process
        .compile()
        .map_err(|error| format!("failed to compile Android AOT bundle: {error:?}"))?;
    let bundle = process.write_engine_bundle(&EngineEntrypoints::runtime_default(), output_dir)?;
    let manifest = fs::read_to_string(&bundle.manifest_path).map_err(|error| {
        format!(
            "failed to read Android AOT manifest {}: {error}",
            bundle.manifest_path.display()
        )
    })?;
    let manifest_json: serde_json::Value = serde_json::from_str(&manifest)
        .map_err(|error| format!("failed to parse Android AOT manifest: {error}"))?;
    let symbols_header = output_dir.join("published_aot_symbols.h");
    let strings_header = output_dir.join("published_aot_strings.h");
    let cmake_file = output_dir.join("published_aot_objects.cmake");
    write_android_aot_symbols_header(&manifest_json, &symbols_header)?;
    write_android_aot_strings_header(&manifest_json, &strings_header)?;
    write_android_aot_cmake_file(&bundle.object_paths_by_function, &cmake_file)?;
    Ok(AndroidAotBundleSummary {
        bundle_dir: bundle.output_dir,
        object_count: bundle.object_paths_by_function.len(),
        symbols_header,
        cmake_file,
    })
}

fn collect_android_aot_sources(
    project_dir: &Path,
    entry_file: Option<&Path>,
) -> Result<Vec<(String, String)>, String> {
    if let Some(entry_file) = entry_file {
        let files = collect_lookup_entry_files(project_dir, entry_file)?;
        return files
            .into_iter()
            .map(|path| {
                let source_key = path.to_string_lossy().to_string();
                let source = fs::read_to_string(&path)
                    .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
                Ok((source_key, source))
            })
            .collect();
    }
    let mut out = Vec::new();
    collect_android_aot_sources_inner(project_dir, project_dir, &mut out)?;
    out.sort_by(|left, right| left.0.cmp(&right.0));
    if out.is_empty() {
        return Err(format!(
            "no .stasis files found under {}",
            project_dir.display()
        ));
    }
    Ok(out)
}

fn collect_android_aot_sources_inner(
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
            collect_android_aot_sources_inner(root, &path, out)?;
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

fn write_android_aot_symbols_header(
    manifest: &serde_json::Value,
    output_path: &Path,
) -> Result<(), String> {
    let main = android_aot_symbol_for(manifest, "main")?;
    let tick = android_aot_symbol_for(manifest, "tick")?;
    let render = android_aot_symbol_for(manifest, "render")?;
    let on_code_swap = android_aot_symbol_for(manifest, "on_code_swap").ok();
    let mut out = String::new();
    out.push_str("#pragma once\n\n");
    for symbol in [&main, &tick, &render] {
        out.push_str(&format!("extern void {symbol}(void);\n"));
    }
    if let Some(symbol) = on_code_swap.as_ref() {
        out.push_str(&format!("extern void {symbol}(void);\n"));
    }
    out.push_str("\n");
    out.push_str(&format!("#define STASIS_AOT_MAIN {main}\n"));
    out.push_str(&format!("#define STASIS_AOT_TICK {tick}\n"));
    out.push_str(&format!("#define STASIS_AOT_RENDER {render}\n"));
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
            "failed to write Android AOT symbol header {}: {error}",
            output_path.display()
        )
    })
}

fn android_aot_symbol_for(
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
        .ok_or_else(|| {
            format!("Android AOT manifest missing symbol for function '{function_name}'")
        })
}

fn write_android_aot_strings_header(
    manifest: &serde_json::Value,
    output_path: &Path,
) -> Result<(), String> {
    let literals = manifest
        .get("string_literals")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Android AOT manifest missing string_literals array".to_string())?;
    let mut out = String::new();
    out.push_str("#pragma once\n\n#include <stdint.h>\n\n");
    out.push_str("typedef struct StasisAotStringLiteral { int32_t id; const char *value; } StasisAotStringLiteral;\n");
    out.push_str("static const StasisAotStringLiteral STASIS_AOT_STRING_LITERALS[] = {\n");
    for literal in literals {
        let id = literal
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "Android AOT string literal missing id".to_string())?;
        let value = literal
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "Android AOT string literal missing value".to_string())?;
        let escaped = value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\r', "\\r");
        out.push_str(&format!("  {{{id}, \"{escaped}\"}},\n"));
    }
    out.push_str("};\n");
    out.push_str(&format!(
        "#define STASIS_AOT_STRING_LITERAL_COUNT {}\n",
        literals.len()
    ));
    if let Some(parent) = output_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create string header directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(output_path, out).map_err(|error| {
        format!(
            "failed to write Android AOT string header {}: {error}",
            output_path.display()
        )
    })
}

fn write_android_aot_cmake_file(
    object_paths_by_function: &std::collections::BTreeMap<String, PathBuf>,
    output_path: &Path,
) -> Result<(), String> {
    let mut out = String::new();
    out.push_str("set(STASIS_PUBLISHED_AOT_OBJECTS\n");
    for path in object_paths_by_function.values() {
        out.push_str(&format!(
            "  \"{}\"\n",
            path.to_string_lossy().replace('\\', "/")
        ));
    }
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
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_lookup_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        std::env::temp_dir().join(format!("stasis_lookup_{name}_{stamp}"))
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
            PathBuf::from("samples/bucket_catcher.stasis")
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
        assert_eq!(parsed.output_dir, PathBuf::from("target/android-aot"));
        assert_eq!(parsed.entry_file, Some(PathBuf::from("src/main.stasis")));
    }

    #[test]
    fn android_aot_bundle_writes_pong_symbols_header() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
        let project_dir = repo_root.join("mobile/android/app/src/main/assets/workshop_sample");
        let output_dir = std::env::temp_dir().join(format!("stasis_android_aot_bundle_{stamp}"));

        let summary = write_android_aot_engine_bundle(&project_dir, &output_dir, None)
            .expect("write Android AOT bundle");

        assert!(summary.object_count >= 3, "expected lifecycle objects");
        let header = fs::read_to_string(&summary.symbols_header).expect("read symbols header");
        assert!(header.contains("#define STASIS_AOT_MAIN aot_fn_"));
        assert!(header.contains("#define STASIS_AOT_TICK aot_fn_"));
        assert!(header.contains("#define STASIS_AOT_RENDER aot_fn_"));
        let cmake = fs::read_to_string(&summary.cmake_file).expect("read cmake file");
        assert!(cmake.contains("set(STASIS_PUBLISHED_AOT_OBJECTS"));

        std::fs::remove_dir_all(&output_dir).ok();
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
    maybe_cleanup_stale_stasis_cache();

    if let Some(exit) = try_run_probe_graphics_runtime_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_lookup_subcommand() {
        std::process::exit(exit);
    }
    if let Some(exit) = try_run_test_subcommand() {
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
