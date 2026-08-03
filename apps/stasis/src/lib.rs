#![forbid(unsafe_code)]
#![cfg_attr(not(debug_assertions), deny(warnings))]

mod compiler_backend;
mod events;
mod host_set_registry;
mod live_workspace;
mod runtime_exec;
mod stasis_test_runner;
mod watch;
mod window_config;

pub use compiler_backend::build_aot_direct_storage_source;
pub use compiler_backend::run_self_host_aot_cli;
pub use compiler_backend::run_self_host_aot_cli_with_options;
pub use events::RunnerEvent;
pub use live_workspace::LiveRunConfig;
pub use stasis_test_runner::{
    run_jit_tests_in_directory, run_jit_tests_in_directory_with_project_root_and_session,
    run_jit_tests_in_directory_with_session, StasisTestRunSession, StasisTestRunSummary,
};
pub use window_config::WindowConfig;

use compiler_backend::{IncrementalCompilerBackend, PreparedJitSwap};
use live_workspace::LiveWorkspace;
use runtime_exec::RuntimeLauncher;
use serde::Deserialize;
use serde_json::Value;
use stasis_assets::{
    load_project_asset_manifest, prepare_asset_bundle, AssetLimits, DEFAULT_ASSET_MANIFEST_PATH,
};
use stasis_compiler::backend::jit::{JitEnginePackage, JitProcess};
use stasis_compiler::backend::state_migration::{
    activate_candidate_transactionally, finalize_runtime_preview, plan_state_migration,
    MAX_STATE_SNAPSHOT_BYTES,
};
use stasis_compiler::backend::EngineEntrypoints;
use stasis_jit::FunctionPointerTable;
use stasis_runner::swap::contracts::{
    AotFunctionSymbol, CompileRequest, CompileResult, CompileStatus, Diagnostic,
    DiagnosticSeverity, FileChangeEvent, FileChangeKind, FnId, FunctionPatch, FunctionPatchSet,
    JitCodePtrOverride, LayoutHash, RequestId, SwapCommitResult, SwapCommitStatus, TargetMode,
    TextSource,
};
use stasis_runner::swap::pipeline::{CompilerBackend, DevHotSwapPipeline};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;
use std::time::Instant;
use watch::WatchService;

const SWAP_FLASH_TICKS_MAX: u32 = 180;
const TICK_BUDGET_SAMPLE_LIMIT: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
struct TickBudgetDiagnostics {
    generation: u64,
    budget_us: u64,
    sample_count: u64,
    total_us: u128,
    overruns: u64,
    recent_us: VecDeque<u64>,
}

impl TickBudgetDiagnostics {
    fn new(budget_us: u64, generation: u64) -> Self {
        Self {
            generation,
            budget_us,
            sample_count: 0,
            total_us: 0,
            overruns: 0,
            recent_us: VecDeque::with_capacity(TICK_BUDGET_SAMPLE_LIMIT),
        }
    }

    fn record(&mut self, elapsed_us: u64) {
        self.sample_count = self.sample_count.saturating_add(1);
        self.total_us = self.total_us.saturating_add(u128::from(elapsed_us));
        if elapsed_us > self.budget_us {
            self.overruns = self.overruns.saturating_add(1);
        }
        if self.recent_us.len() == TICK_BUDGET_SAMPLE_LIMIT {
            self.recent_us.pop_front();
        }
        self.recent_us.push_back(elapsed_us);
    }

    fn average_us(&self) -> u64 {
        if self.sample_count == 0 {
            return 0;
        }
        u64::try_from(self.total_us / u128::from(self.sample_count)).unwrap_or(u64::MAX)
    }

    fn p99_us(&self) -> u64 {
        let mut samples = self.recent_us.iter().copied().collect::<Vec<_>>();
        if samples.is_empty() {
            return 0;
        }
        samples.sort_unstable();
        let rank = samples.len().saturating_mul(99).div_ceil(100);
        samples[rank.saturating_sub(1)]
    }

    fn report(&self) -> String {
        format!(
            "[tick-budget] generation={} budget_us={} samples={} average_us={} p99_us={} overruns={}",
            self.generation,
            self.budget_us,
            self.sample_count,
            self.average_us(),
            self.p99_us(),
            self.overruns
        )
    }
}

fn update_tick_budget_after_swap(
    current: &mut Option<TickBudgetDiagnostics>,
    generation: &mut u64,
    candidate_budget_us: Option<u64>,
    committed: bool,
) -> Option<String> {
    if !committed {
        return None;
    }
    let completed = current.take().map(|diagnostics| diagnostics.report());
    *generation = generation.saturating_add(1);
    *current =
        candidate_budget_us.map(|budget_us| TickBudgetDiagnostics::new(budget_us, *generation));
    completed
}

#[derive(Debug, Clone, Default)]
struct PendingAotCompileMetadata {
    linked_image_path: Option<PathBuf>,
    linked_image_size_bytes: Option<u64>,
    function_symbols: Option<Vec<AotFunctionSymbol>>,
}

#[derive(Debug, Clone)]
pub struct RunnerConfig {
    pub max_ticks: u32,
    pub tick_sleep_micros: u64,
    pub window: Option<WindowConfig>,
    pub inject_file_change: Option<PathBuf>,
    pub watch_directory: Option<PathBuf>,
    pub target_mode: TargetMode,
    pub fail_compile: bool,
    pub disable_on_code_swap_hook: bool,
    pub hook_failure_reason: Option<String>,
    pub swap_failure_reason: Option<String>,
    pub runtime_launch: bool,
    pub aot_probe_loadability: bool,
    pub host_set_profile: Option<String>,
    pub host_set_registry_file: Option<PathBuf>,
}

impl Default for RunnerConfig {
    fn default() -> Self {
        Self {
            max_ticks: 120,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunnerSummary {
    pub ticks_executed: u32,
    pub compile_successes: u32,
    pub compile_failures: u32,
    pub compile_diagnostics: Vec<String>,
    pub hook_runs: u32,
    pub hook_failures: u32,
    pub hook_failure_reasons: Vec<String>,
    pub swap_commit_successes: u32,
    pub swap_commit_failures: u32,
    pub swap_failure_reasons: Vec<String>,
    pub swap_indicator_armed_count: u32,
    pub swap_flash_peak_ticks: u32,
    pub swap_flash_ticks_remaining: u32,
    pub last_compile_duration_ms: Option<u64>,
    pub last_commit_duration_ms: Option<u64>,
    pub window: Option<WindowConfig>,
    pub last_swap_status: Option<SwapCommitStatus>,
    pub has_in_flight_work: bool,
    pub events: Vec<RunnerEvent>,
    pub runtime_launches: u32,
    pub runtime_launch_failures: u32,
    pub runtime_launch_failure_reasons: Vec<String>,
    pub aot_linked_image_activations: u32,
    pub active_aot_linked_image_path: Option<PathBuf>,
    pub active_aot_linked_image_size_bytes: Option<u64>,
    pub active_aot_linked_image_generation: Option<u64>,
    pub retired_aot_linked_images: u32,
}

const STASIS_HOST_SET_PROFILE_ENV: &str = "STASIS_HOST_SET_PROFILE";
const STASIS_HOST_SET_REGISTRY_FILE_ENV: &str = "STASIS_HOST_SET_REGISTRY_FILE";

fn infer_host_set_profile_from_target_mode(
    target_mode: TargetMode,
) -> host_set_registry::HostSetProfile {
    match target_mode {
        TargetMode::JitDev => host_set_registry::HostSetProfile::Dev,
        TargetMode::AotProd => host_set_registry::HostSetProfile::Prod,
    }
}

fn resolve_host_set_contract(
    config: &RunnerConfig,
) -> Result<host_set_registry::HostSetContract, String> {
    let profile = if let Some(profile) = config.host_set_profile.as_deref() {
        host_set_registry::HostSetProfile::parse(profile).ok_or_else(|| {
            format!("invalid --host-set-profile '{profile}' (expected dev|test|prod)")
        })?
    } else if let Ok(profile) = std::env::var(STASIS_HOST_SET_PROFILE_ENV) {
        host_set_registry::HostSetProfile::parse(&profile).ok_or_else(|| {
            format!("invalid {STASIS_HOST_SET_PROFILE_ENV}='{profile}' (expected dev|test|prod)")
        })?
    } else {
        infer_host_set_profile_from_target_mode(config.target_mode)
    };

    let registry_file = config
        .host_set_registry_file
        .as_deref()
        .map(|path| path.to_path_buf())
        .or_else(|| {
            std::env::var_os(STASIS_HOST_SET_REGISTRY_FILE_ENV)
                .filter(|path| !path.is_empty())
                .map(PathBuf::from)
        });

    host_set_registry::resolve_profile_contract(profile, registry_file.as_deref())
}

pub fn run_with_default_backend(config: RunnerConfig) -> RunnerSummary {
    let backend = move |request: CompileRequest| -> CompileResult {
        if config.fail_compile {
            CompileResult::failed(
                request.request_id,
                vec![Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: "simulated compile failure".to_string(),
                    path: request.changed_files.first().cloned(),
                    line: Some(1),
                    column: Some(1),
                }],
            )
        } else {
            let patch_set = FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(1) }],
            };
            let hook_symbol = if request.target_mode == TargetMode::JitDev {
                Some("on_code_swap".to_string())
            } else {
                None
            };
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([1; 32]),
                patch_set,
                request.host_set_id.clone(),
                request.host_set_hash,
                hook_symbol,
                None,
                None,
                None,
                None,
                None,
            )
        }
    };

    run_with_backend(config, backend)
}

fn hash_global_path(path: &str) -> i32 {
    // Must match `crates/stasis_compiler/src/backend/jit.rs::hash_global_path`.
    let mut hash: u32 = 2166136261;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash as i32
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct PlayStructMetadata {
    version: i32,
    #[serde(rename = "globalName")]
    global_name: String,
    #[serde(default, rename = "csvTable")]
    csv_table: Option<CsvTableMetadata>,
    fields: Vec<PlayStructFieldMetadata>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct CsvTableMetadata {
    #[serde(rename = "rowsPath")]
    pub(crate) rows_path: String,
    #[serde(rename = "rowCountPath")]
    pub(crate) row_count_path: String,
    pub(crate) capacity: usize,
    #[serde(rename = "keyColumns")]
    pub(crate) key_columns: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
struct PlayStructFieldMetadata {
    #[serde(rename = "jsonPath")]
    json_path: String,
    #[serde(default, rename = "csvColumn")]
    csv_column: Option<String>,
    #[serde(rename = "type")]
    type_name: String,
    #[serde(rename = "arrayCount")]
    array_count: i32,
}

#[derive(Debug, Clone)]
pub(crate) struct CsvBindingField {
    pub(crate) path: String,
    pub(crate) csv_column: Option<String>,
    pub(crate) type_name: String,
    pub(crate) array_count: usize,
}

fn csv_column_name<'a>(field: &'a CsvBindingField) -> &'a str {
    field.csv_column.as_deref().unwrap_or(&field.path)
}

fn parse_csv_records(source: &str) -> Result<Vec<Vec<String>>, String> {
    let mut records = Vec::new();
    let mut row = Vec::new();
    let mut field = String::new();
    let mut chars = source.chars().peekable();
    let mut in_quotes = false;
    let mut closed_quote = false;

    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek() == Some(&'"') {
                    chars.next();
                    field.push('"');
                } else {
                    in_quotes = false;
                    closed_quote = true;
                }
            } else {
                field.push(ch);
            }
            continue;
        }
        if closed_quote && !matches!(ch, ',' | '\r' | '\n') {
            return Err("unexpected character after closing quote".to_string());
        }
        match ch {
            '"' if field.is_empty() && !closed_quote => in_quotes = true,
            '"' => return Err("quote must begin a CSV field".to_string()),
            ',' => {
                row.push(std::mem::take(&mut field));
                closed_quote = false;
            }
            '\n' => {
                row.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut row));
                closed_quote = false;
            }
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                row.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut row));
                closed_quote = false;
            }
            _ => field.push(ch),
        }
    }
    if in_quotes {
        return Err("unterminated quoted CSV field".to_string());
    }
    if !field.is_empty() || !row.is_empty() || closed_quote {
        row.push(field);
        records.push(row);
    }
    Ok(records)
}

fn parse_csv_cell(value: &str, field: &CsvBindingField) -> Result<Value, String> {
    let trimmed = value.trim();
    match field.type_name.as_str() {
        "string" => Ok(Value::String(value.to_string())),
        "bool" => match trimmed.to_ascii_lowercase().as_str() {
            "true" | "1" => Ok(Value::Bool(true)),
            "false" | "0" => Ok(Value::Bool(false)),
            _ => Err(format!(
                "field {} requires true, false, 1, or 0",
                field.path
            )),
        },
        "i32" => trimmed
            .parse::<i32>()
            .map(|number| Value::Number(number.into()))
            .map_err(|error| format!("field {} requires an i32: {error}", field.path)),
        "u8" => trimmed
            .parse::<u8>()
            .map(|number| Value::Number(number.into()))
            .map_err(|error| format!("field {} requires a u8: {error}", field.path)),
        "u16" => trimmed
            .parse::<u16>()
            .map(|number| Value::Number(number.into()))
            .map_err(|error| format!("field {} requires a u16: {error}", field.path)),
        "u32" => trimmed
            .parse::<u32>()
            .map(|number| Value::Number(number.into()))
            .map_err(|error| format!("field {} requires a u32: {error}", field.path)),
        "f32" | "f64" => {
            let number = trimmed
                .parse::<f64>()
                .map_err(|error| format!("field {} requires a number: {error}", field.path))?;
            let number = serde_json::Number::from_f64(number)
                .ok_or_else(|| format!("field {} must be finite", field.path))?;
            Ok(Value::Number(number))
        }
        other => Err(format!(
            "field {} has unsupported CSV type {other}",
            field.path
        )),
    }
}

pub(crate) fn parse_flat_csv_binding(
    source: &str,
    fields: &[CsvBindingField],
) -> Result<Value, String> {
    let mut metadata_paths = BTreeSet::new();
    for field in fields {
        if field.csv_column.is_some() {
            return Err("csvColumn metadata requires csvTable".to_string());
        }
        if field.path.is_empty() || field.path.contains('.') {
            return Err(format!(
                "CSV metadata path {} must name one flat column",
                field.path
            ));
        }
        if !metadata_paths.insert(field.path.as_str()) {
            return Err(format!("duplicate CSV metadata path {}", field.path));
        }
    }
    let records = parse_csv_records(source)?;
    let Some(headers) = records.first() else {
        return Err("CSV data requires a header row".to_string());
    };
    if records.len() == 1 {
        return Err("CSV data requires at least one data row".to_string());
    }
    let mut header_indices = BTreeMap::new();
    for (index, header) in headers.iter().enumerate() {
        let header = if index == 0 {
            header.strip_prefix('\u{feff}').unwrap_or(header)
        } else {
            header
        };
        if header.is_empty() {
            return Err("CSV headers must not be empty".to_string());
        }
        if header.contains('.') {
            return Err(format!(
                "CSV header {header} cannot contain nested path separators"
            ));
        }
        if !metadata_paths.contains(header) {
            return Err(format!(
                "CSV column {header} does not exist in target metadata"
            ));
        }
        if header_indices.insert(header.to_string(), index).is_some() {
            return Err(format!("duplicate CSV header {header}"));
        }
    }
    for (row_index, row) in records.iter().enumerate().skip(1) {
        if row.len() != headers.len() {
            return Err(format!(
                "CSV row {} has {} columns; expected {}",
                row_index + 1,
                row.len(),
                headers.len()
            ));
        }
    }

    let data_rows = &records[1..];
    let mut object = serde_json::Map::new();
    for field in fields {
        let column = header_indices
            .get(&field.path)
            .copied()
            .ok_or_else(|| format!("CSV is missing metadata column {}", field.path))?;
        let is_array = field.type_name != "string" && field.array_count > 1;
        let value = if is_array {
            if data_rows.len() != field.array_count {
                return Err(format!(
                    "CSV column {} requires {} rows, found {}",
                    field.path,
                    field.array_count,
                    data_rows.len()
                ));
            }
            Value::Array(
                data_rows
                    .iter()
                    .map(|row| parse_csv_cell(&row[column], field))
                    .collect::<Result<Vec<_>, _>>()?,
            )
        } else {
            if data_rows.len() != 1 {
                return Err(format!(
                    "scalar CSV column {} requires exactly one data row",
                    field.path
                ));
            }
            parse_csv_cell(&data_rows[0][column], field)?
        };
        object.insert(field.path.clone(), value);
    }
    Ok(Value::Object(object))
}

pub(crate) fn parse_csv_table_binding(
    source: &str,
    fields: &[CsvBindingField],
    table: &CsvTableMetadata,
) -> Result<Value, String> {
    if table.rows_path.is_empty()
        || table.row_count_path.is_empty()
        || table.rows_path.contains('.')
        || table.row_count_path.contains('.')
    {
        return Err("CSV table rowsPath and rowCountPath must be flat properties".to_string());
    }
    if table.capacity == 0 {
        return Err("CSV table capacity must be greater than zero".to_string());
    }
    if table.key_columns.is_empty() {
        return Err("CSV table requires at least one stable key column".to_string());
    }

    let prefix = format!("{}.", table.rows_path);
    let mut columns = BTreeMap::new();
    for field in fields {
        let suffix = field.path.strip_prefix(&prefix).ok_or_else(|| {
            format!(
                "CSV table target {} must be below rowsPath {}",
                field.path, table.rows_path
            )
        })?;
        if suffix.is_empty() {
            return Err(format!("CSV table target {} has no row field", field.path));
        }
        if suffix.contains('.') {
            return Err(format!(
                "CSV table target {} must name one flat row field",
                field.path
            ));
        }
        if field.array_count != table.capacity {
            return Err(format!(
                "CSV table target {} capacity {} does not match table capacity {}",
                field.path, field.array_count, table.capacity
            ));
        }
        let column = csv_column_name(field);
        if column.is_empty() || column.contains('.') {
            return Err(format!("CSV table column {column} must be a flat header"));
        }
        if columns.insert(column.to_string(), field).is_some() {
            return Err(format!("duplicate CSV table column {column}"));
        }
    }
    let mut key_columns = BTreeSet::new();
    for key in &table.key_columns {
        if !key_columns.insert(key) {
            return Err(format!("duplicate CSV table key column {key}"));
        }
        if !columns.contains_key(key) {
            return Err(format!("CSV table key column {key} has no target field"));
        }
    }

    let records = parse_csv_records(source)?;
    let Some(headers) = records.first() else {
        return Err("CSV table requires a header row".to_string());
    };
    let mut header_indices = BTreeMap::new();
    for (index, raw_header) in headers.iter().enumerate() {
        let header = if index == 0 {
            raw_header.strip_prefix('\u{feff}').unwrap_or(raw_header)
        } else {
            raw_header
        };
        if header.is_empty() || header.contains('.') {
            return Err(format!(
                "CSV table header {header} must be flat and non-empty"
            ));
        }
        if !columns.contains_key(header) {
            return Err(format!(
                "CSV column {header} does not exist in target metadata"
            ));
        }
        if header_indices.insert(header.to_string(), index).is_some() {
            return Err(format!("duplicate CSV header {header}"));
        }
    }
    for column in columns.keys() {
        if !header_indices.contains_key(column) {
            return Err(format!("CSV is missing metadata column {column}"));
        }
    }

    let data_rows = &records[1..];
    if data_rows.len() > table.capacity {
        return Err(format!(
            "CSV table has {} rows; capacity is {}",
            data_rows.len(),
            table.capacity
        ));
    }
    let mut stable_keys = BTreeSet::new();
    for (row_index, row) in data_rows.iter().enumerate() {
        if row.len() != headers.len() {
            return Err(format!(
                "CSV row {} has {} columns; expected {}",
                row_index + 2,
                row.len(),
                headers.len()
            ));
        }
        let mut parts = Vec::new();
        for key in &table.key_columns {
            let raw_value = &row[*header_indices.get(key).expect("key header validated")];
            if raw_value.trim().is_empty() {
                return Err(format!(
                    "CSV table key column {key} is blank on row {}",
                    row_index + 2
                ));
            }
            let field = columns.get(key).expect("key target validated");
            parts.push(parse_csv_cell(raw_value, field)?.to_string());
        }
        let stable_key = parts.join("\u{1f}");
        if !stable_keys.insert(stable_key) {
            return Err(format!("duplicate CSV table key on row {}", row_index + 2));
        }
    }

    let mut rows = serde_json::Map::new();
    for (column, field) in columns {
        let column_index = *header_indices
            .get(&column)
            .expect("column header validated");
        let mut values = data_rows
            .iter()
            .map(|row| parse_csv_cell(&row[column_index], field))
            .collect::<Result<Vec<_>, _>>()?;
        let default = match field.type_name.as_str() {
            "bool" => Value::Bool(false),
            "u8" | "u16" | "u32" | "i32" => Value::Number(0.into()),
            "f32" | "f64" => serde_json::json!(0.0),
            other => {
                return Err(format!(
                    "CSV table target {} has unsupported column type {other}",
                    field.path
                ));
            }
        };
        values.resize(table.capacity, default);
        let suffix = field
            .path
            .strip_prefix(&prefix)
            .expect("field prefix validated");
        rows.insert(suffix.to_string(), Value::Array(values));
    }
    let mut root = serde_json::Map::new();
    root.insert(table.rows_path.clone(), Value::Object(rows));
    root.insert(
        table.row_count_path.clone(),
        Value::Number((data_rows.len() as u64).into()),
    );
    Ok(Value::Object(root))
}

fn resolve_play_sidecar_path(path: &Path, launch_dir: &Path) -> PathBuf {
    if path.is_absolute() {
        return path.to_path_buf();
    }
    launch_dir.join(path)
}

fn resolve_play_data_binding_paths(
    watch_file: &Path,
    launch_dir: &Path,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
) -> Result<Vec<(PathBuf, PathBuf)>, String> {
    match (data_bind_json, data_bind_struct_meta) {
        (Some(json_path), Some(struct_meta_path)) => {
            return Ok(vec![(
                resolve_play_sidecar_path(json_path, launch_dir),
                resolve_play_sidecar_path(struct_meta_path, launch_dir),
            )]);
        }
        (None, None) => {}
        _ => return Err("play data binding requires both json and struct-meta paths".to_string()),
    }

    fn discover_pairs(data_dir: &Path) -> Result<Vec<(PathBuf, PathBuf)>, String> {
        fn collect_data_paths(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
            for entry in fs::read_dir(dir).map_err(|error| {
                format!("failed to read data directory {}: {error}", dir.display())
            })? {
                let entry = entry.map_err(|error| {
                    format!("failed to read data directory {}: {error}", dir.display())
                })?;
                let path = entry.path();
                if path.is_dir() {
                    collect_data_paths(&path, out)?;
                } else if path.is_file()
                    && path.extension().is_some_and(|ext| {
                        ext.eq_ignore_ascii_case("json") || ext.eq_ignore_ascii_case("csv")
                    })
                    && !path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.ends_with(".struct-meta.json"))
                {
                    out.push(path);
                }
            }
            Ok(())
        }

        if !data_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut data_paths = Vec::new();
        collect_data_paths(data_dir, &mut data_paths)?;
        data_paths.sort();
        let mut out = Vec::new();
        let mut metadata_owners = BTreeMap::new();
        for data_path in data_paths {
            let stem = data_path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("invalid data file name {}", data_path.display()))?;
            let meta_path = data_path.with_file_name(format!("{stem}.struct-meta.json"));
            if !meta_path.is_file() {
                return Err(format!(
                    "data file {} requires matching metadata {}",
                    data_path.display(),
                    meta_path.display()
                ));
            }
            if let Some(owner) = metadata_owners.insert(meta_path.clone(), data_path.clone()) {
                return Err(format!(
                    "data files {} and {} cannot share metadata {}",
                    owner.display(),
                    data_path.display(),
                    meta_path.display()
                ));
            }
            out.push((data_path, meta_path));
        }
        Ok(out)
    }

    let watch_file_path = resolve_play_sidecar_path(watch_file, launch_dir);
    let mut roots = vec![launch_dir.join("data")];
    let Some(file_stem) = watch_file_path.file_stem() else {
        return Ok(Vec::new());
    };
    let base_dir = watch_file_path.parent().unwrap_or(launch_dir);
    roots.push(base_dir.join(file_stem).join("data"));
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        for pair in discover_pairs(&root)? {
            let key = normalize_watch_path_for_log(&pair.0);
            if seen.insert(key) {
                out.push(pair);
            }
        }
    }
    Ok(out)
}
fn json_value_by_path<'a>(root: &'a Value, path: &str) -> Option<&'a Value> {
    if path.is_empty() {
        return Some(root);
    }

    let mut current = root;
    for segment in path.split('.') {
        current = current.get(segment)?;
    }
    Some(current)
}

fn validate_play_binding_source(root: &Value, metadata: &PlayStructMetadata) -> Result<(), String> {
    let mut paths: Vec<String> = metadata
        .fields
        .iter()
        .map(|field| field.json_path.clone())
        .collect();
    if let Some(table) = &metadata.csv_table {
        paths.push(table.row_count_path.clone());
    }
    validate_binding_source_paths(root, &paths)
}

pub(crate) fn validate_binding_source_paths(root: &Value, paths: &[String]) -> Result<(), String> {
    let mut field_paths = BTreeSet::new();
    for path in paths {
        if path.is_empty() {
            return Err("binding metadata paths must not be empty".to_string());
        }
        if !field_paths.insert(path.as_str()) {
            return Err(format!("duplicate binding metadata path {path}"));
        }
        if json_value_by_path(root, path).is_none() {
            return Err(format!("binding source is missing target property {path}"));
        }
    }

    fn walk(value: &Value, path: &str, field_paths: &BTreeSet<&str>) -> Result<(), String> {
        let Value::Object(properties) = value else {
            return Ok(());
        };
        for (name, child) in properties {
            let child_path = if path.is_empty() {
                name.clone()
            } else {
                format!("{path}.{name}")
            };
            let exact = field_paths.contains(child_path.as_str());
            let prefix = field_paths
                .iter()
                .any(|field| field.starts_with(&format!("{child_path}.")));
            if !exact && !prefix {
                return Err(format!(
                    "binding source property {child_path} does not exist in target metadata"
                ));
            }
            if prefix {
                walk(child, &child_path, field_paths)?;
            }
        }
        Ok(())
    }

    walk(root, "", &field_paths)
}

fn validate_play_binding_targets(
    metadata: &PlayStructMetadata,
    jit: &JitProcess,
) -> Result<(), String> {
    if let Some(table) = &metadata.csv_table {
        let collection_path = format!("{}.{}", metadata.global_name, table.rows_path);
        let capacity = jit
            .global_collection_capacity(&collection_path)
            .ok_or_else(|| {
                format!(
                    "binding target property {collection_path} does not exist in compiled globals"
                )
            })?;
        if usize::try_from(capacity).ok() != Some(table.capacity) {
            return Err(format!(
                "binding target collection {collection_path} has capacity {capacity}; metadata requires {}",
                table.capacity
            ));
        }
        let row_count_path = format!("{}.{}", metadata.global_name, table.row_count_path);
        let row_count_type = jit.global_scalar_type(&row_count_path).ok_or_else(|| {
            format!("binding target property {row_count_path} does not exist in compiled globals")
        })?;
        if row_count_type != "i32" {
            return Err(format!(
                "binding target property {row_count_path} has type {row_count_type}; CSV row count requires i32"
            ));
        }
        let prefix = format!("{}.", table.rows_path);
        for field in &metadata.fields {
            let suffix = field.json_path.strip_prefix(&prefix).ok_or_else(|| {
                format!(
                    "CSV table target {} must be below rowsPath {}",
                    field.json_path, table.rows_path
                )
            })?;
            let target_type = jit
                .global_collection_field_type(&collection_path, suffix)
                .ok_or_else(|| {
                    format!(
                        "binding target property {collection_path}[].{suffix} does not exist in compiled globals"
                    )
                })?;
            if target_type != field.type_name {
                return Err(format!(
                    "binding target property {collection_path}[].{suffix} has type {target_type}; metadata requires {}",
                    field.type_name
                ));
            }
        }
        return Ok(());
    }
    for field in &metadata.fields {
        let full_path = format!("{}.{}", metadata.global_name, field.json_path);
        let target_type = jit.global_binding_type(&full_path).ok_or_else(|| {
            format!("binding target property {full_path} does not exist in compiled globals")
        })?;
        if target_type != field.type_name {
            return Err(format!(
                "binding target property {full_path} has type {target_type}; metadata requires {}",
                field.type_name
            ));
        }
        let metadata_capacity =
            (field.type_name == "string" || field.array_count > 1).then_some(field.array_count);
        let target_capacity = jit.global_binding_capacity(&full_path);
        if target_capacity != metadata_capacity {
            return Err(format!(
                "binding target property {full_path} has capacity {}; metadata requires {}",
                target_capacity
                    .map(|capacity| capacity.to_string())
                    .unwrap_or_else(|| "scalar".to_string()),
                metadata_capacity
                    .map(|capacity| capacity.to_string())
                    .unwrap_or_else(|| "scalar".to_string())
            ));
        }
    }
    Ok(())
}

fn validate_unique_play_binding_targets(
    loaded: &[(PathBuf, Value, PlayStructMetadata)],
) -> Result<(), String> {
    let mut owners = BTreeMap::new();
    for (data_path, _, metadata) in loaded {
        let targets = metadata
            .fields
            .iter()
            .map(|field| &field.json_path)
            .chain(metadata.csv_table.iter().map(|table| &table.row_count_path));
        for target in targets {
            let full_path = format!("{}.{}", metadata.global_name, target);
            if let Some(previous_path) = owners.insert(full_path.clone(), data_path) {
                return Err(format!(
                    "binding target property {full_path} is mapped by both {} and {}",
                    previous_path.display(),
                    data_path.display()
                ));
            }
        }
    }
    Ok(())
}

fn play_integer_bits(value: &Value, type_name: &str) -> Option<i32> {
    match type_name {
        "i32" => value.as_i64().and_then(|number| i32::try_from(number).ok()),
        "u8" => value
            .as_u64()
            .and_then(|number| u8::try_from(number).ok())
            .map(i32::from),
        "u16" => value
            .as_u64()
            .and_then(|number| u16::try_from(number).ok())
            .map(i32::from),
        "u32" => value
            .as_u64()
            .and_then(|number| u32::try_from(number).ok())
            .map(|number| number as i32),
        _ => None,
    }
}

fn apply_play_bound_table_array(
    field: &PlayStructFieldMetadata,
    collection_hash: i32,
    field_hash: i32,
    value: &Value,
) {
    let Some(items) = value.as_array() else {
        return;
    };
    match field.type_name.as_str() {
        "bool" | "u8" | "u16" | "u32" | "i32" => {
            for (index, item) in items.iter().enumerate() {
                let value = if field.type_name == "bool" {
                    item.as_bool().map(|flag| i32::from(flag))
                } else {
                    play_integer_bits(item, &field.type_name)
                };
                if let Some(value) = value {
                    stasis_dynload::stasis_jit_global_i32_array_store(
                        collection_hash,
                        field_hash,
                        index as i32,
                        value,
                    );
                }
            }
        }
        "f32" => {
            for (index, item) in items.iter().enumerate() {
                if let Some(value) = item.as_f64() {
                    stasis_dynload::stasis_jit_global_f32_array_store(
                        collection_hash,
                        field_hash,
                        index as i32,
                        value as f32,
                    );
                }
            }
        }
        "f64" => {
            for (index, item) in items.iter().enumerate() {
                if let Some(value) = item.as_f64() {
                    stasis_dynload::stasis_jit_global_f64_array_store(
                        collection_hash,
                        field_hash,
                        index as i32,
                        value,
                    );
                }
            }
        }
        _ => {}
    }
}

fn apply_play_csv_table_value(
    root: &Value,
    metadata: &PlayStructMetadata,
    table: &CsvTableMetadata,
) -> Result<(), String> {
    let collection_path = format!("{}.{}", metadata.global_name, table.rows_path);
    let collection_hash = hash_global_path(&collection_path);
    let prefix = format!("{}.", table.rows_path);
    for field in &metadata.fields {
        let suffix = field.json_path.strip_prefix(&prefix).ok_or_else(|| {
            format!(
                "CSV table target {} must be below rowsPath {}",
                field.json_path, table.rows_path
            )
        })?;
        let value = json_value_by_path(root, &field.json_path)
            .ok_or_else(|| format!("CSV table is missing target {}", field.json_path))?;
        apply_play_bound_table_array(field, collection_hash, hash_global_path(suffix), value);
    }
    let row_count = json_value_by_path(root, &table.row_count_path)
        .and_then(Value::as_u64)
        .and_then(|value| i32::try_from(value).ok())
        .ok_or_else(|| "CSV table row_count is invalid".to_string())?;
    stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 1, row_count);
    stasis_dynload::stasis_jit_collection_i32_store(
        collection_hash,
        2,
        i32::try_from(table.capacity).map_err(|_| "CSV table capacity exceeds i32".to_string())?,
    );
    let row_count_path = format!("{}.{}", metadata.global_name, table.row_count_path);
    stasis_dynload::stasis_jit_global_i32_store(hash_global_path(&row_count_path), row_count);
    Ok(())
}

fn truncate_utf8_to_capacity(value: &str, max_bytes: usize) -> (Vec<u8>, i32) {
    let mut out = Vec::new();
    let mut chars = 0i32;
    for ch in value.chars() {
        let mut encoded = [0u8; 4];
        let bytes = ch.encode_utf8(&mut encoded).as_bytes();
        if out.len() + bytes.len() > max_bytes {
            break;
        }
        out.extend_from_slice(bytes);
        chars += 1;
    }
    (out, chars)
}

fn apply_play_bound_string(path: &str, fallback_capacity: i32, value: &str) {
    let collection_hash = hash_global_path(path);
    let seeded_capacity = stasis_dynload::stasis_jit_collection_i32_load(collection_hash, 2);
    let capacity = if seeded_capacity > 0 {
        seeded_capacity as usize
    } else if fallback_capacity > 0 {
        fallback_capacity as usize
    } else {
        0
    };
    if capacity == 0 {
        return;
    }

    let max_copy = capacity.saturating_sub(1);
    let (bytes, char_count) = truncate_utf8_to_capacity(value, max_copy);
    for index in 0..capacity {
        stasis_dynload::stasis_jit_global_i32_array_store(collection_hash, 0, index as i32, 0);
    }
    for (index, byte) in bytes.iter().enumerate() {
        stasis_dynload::stasis_jit_global_i32_array_store(
            collection_hash,
            0,
            index as i32,
            i32::from(*byte),
        );
    }
    stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 1, bytes.len() as i32);
    if seeded_capacity <= 0 {
        stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 2, capacity as i32);
    }
    stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 3, char_count);
}

fn apply_play_bound_array(field: &PlayStructFieldMetadata, path: &str, value: &Value) {
    let Some(items) = value.as_array() else {
        return;
    };
    let collection_hash = hash_global_path(path);
    let capacity = usize::try_from(field.array_count.max(0)).unwrap_or(0);
    let count = capacity.min(items.len());
    if capacity == 0 {
        return;
    }

    match field.type_name.as_str() {
        "bool" | "u8" | "u16" | "u32" | "i32" => {
            for (index, item) in items.iter().take(count).enumerate() {
                let value = match field.type_name.as_str() {
                    "bool" => item.as_bool().map(|flag| if flag { 1 } else { 0 }),
                    _ => play_integer_bits(item, &field.type_name),
                };
                let Some(value) = value else {
                    continue;
                };
                stasis_dynload::stasis_jit_global_i32_array_store(
                    collection_hash,
                    0,
                    index as i32,
                    value,
                );
            }
        }
        "f32" => {
            for (index, item) in items.iter().take(count).enumerate() {
                let Some(value) = item.as_f64() else {
                    continue;
                };
                stasis_dynload::stasis_jit_global_f32_array_store(
                    collection_hash,
                    0,
                    index as i32,
                    value as f32,
                );
            }
        }
        "f64" => {
            for (index, item) in items.iter().take(count).enumerate() {
                let Some(value) = item.as_f64() else {
                    continue;
                };
                stasis_dynload::stasis_jit_global_f64_array_store(
                    collection_hash,
                    0,
                    index as i32,
                    value,
                );
            }
        }
        _ => {}
    }

    stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 1, count as i32);
    stasis_dynload::stasis_jit_collection_i32_store(collection_hash, 2, capacity as i32);
}

fn apply_play_bound_value(field: &PlayStructFieldMetadata, value: &Value, full_path: &str) {
    if field.type_name == "string" {
        if let Some(text) = value.as_str() {
            apply_play_bound_string(full_path, field.array_count, text);
        }
        return;
    }
    if field.array_count > 1 {
        apply_play_bound_array(field, full_path, value);
        return;
    }

    let path_hash = hash_global_path(full_path);
    match field.type_name.as_str() {
        "bool" => {
            let Some(flag) = value.as_bool() else {
                return;
            };
            stasis_dynload::stasis_jit_global_i32_store(path_hash, if flag { 1 } else { 0 });
        }
        "u8" | "u16" | "u32" | "i32" => {
            let Some(number) = play_integer_bits(value, &field.type_name) else {
                return;
            };
            stasis_dynload::stasis_jit_global_i32_store(path_hash, number);
        }
        "f32" => {
            let Some(number) = value.as_f64() else {
                return;
            };
            stasis_dynload::stasis_jit_global_f32_store(path_hash, number as f32);
        }
        "f64" => {
            let Some(number) = value.as_f64() else {
                return;
            };
            stasis_dynload::stasis_jit_global_f64_store(path_hash, number);
        }
        _ => {}
    }
}

fn apply_play_data_binding_value(
    root: &Value,
    metadata: &PlayStructMetadata,
) -> Result<(), String> {
    if metadata.version != 1 {
        return Err(format!(
            "unsupported struct-meta version {} (expected 1)",
            metadata.version
        ));
    }

    if let Some(table) = &metadata.csv_table {
        return apply_play_csv_table_value(root, metadata, table);
    }

    for field in &metadata.fields {
        let Some(value) = json_value_by_path(root, &field.json_path) else {
            continue;
        };
        let full_path = if field.json_path.is_empty() {
            metadata.global_name.clone()
        } else {
            format!("{}.{}", metadata.global_name, field.json_path)
        };
        apply_play_bound_value(field, value, &full_path);
    }

    Ok(())
}

fn load_and_apply_play_data_bindings(
    paths: &[(PathBuf, PathBuf)],
    jit: Option<&JitProcess>,
) -> Result<(), String> {
    let mut loaded = Vec::new();
    for (data_path, meta_path) in paths {
        let meta_source = fs::read_to_string(meta_path).map_err(|error| {
            format!(
                "failed to read data-bind struct-meta {}: {error}",
                meta_path.display()
            )
        })?;
        let metadata: PlayStructMetadata = serde_json::from_str(&meta_source).map_err(|error| {
            format!(
                "failed to parse data-bind struct-meta {}: {error}",
                meta_path.display()
            )
        })?;
        let data_source = fs::read_to_string(data_path).map_err(|error| {
            format!("failed to read data file {}: {error}", data_path.display())
        })?;
        let is_csv = data_path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("csv"));
        let data_root = if is_csv {
            let fields: Vec<CsvBindingField> = metadata
                .fields
                .iter()
                .map(|field| CsvBindingField {
                    path: field.json_path.clone(),
                    csv_column: field.csv_column.clone(),
                    type_name: field.type_name.clone(),
                    array_count: usize::try_from(field.array_count.max(0)).unwrap_or(0),
                })
                .collect();
            let parsed = if let Some(table) = &metadata.csv_table {
                parse_csv_table_binding(&data_source, &fields, table)
            } else {
                parse_flat_csv_binding(&data_source, &fields)
            };
            parsed.map_err(|error| {
                format!("failed to parse data CSV {}: {error}", data_path.display())
            })?
        } else {
            if metadata.csv_table.is_some() {
                return Err(format!(
                    "csvTable metadata requires a CSV data file: {}",
                    data_path.display()
                ));
            }
            serde_json::from_str(&data_source).map_err(|error| {
                format!("failed to parse data JSON {}: {error}", data_path.display())
            })?
        };
        loaded.push((data_path.clone(), data_root, metadata));
    }
    for (_, _, metadata) in &loaded {
        if metadata.version != 1 {
            return Err(format!(
                "unsupported struct-meta version {}; expected 1",
                metadata.version
            ));
        }
    }
    validate_unique_play_binding_targets(&loaded)?;
    for (_, data_root, metadata) in &loaded {
        validate_play_binding_source(data_root, metadata)?;
        if let Some(jit) = jit {
            validate_play_binding_targets(metadata, jit)?;
        }
    }
    for (_, json_root, metadata) in loaded {
        apply_play_data_binding_value(&json_root, &metadata)?;
    }
    Ok(())
}

fn resolve_play_watch_dir(watch_file: &Path, watch_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = watch_dir {
        if !dir.as_os_str().is_empty() {
            return dir.to_path_buf();
        }
    }

    if let Some(parent) = watch_file.parent() {
        if !parent.as_os_str().is_empty() {
            return parent.to_path_buf();
        }
    }

    // `Path::parent()` can yield an empty path for basename-only inputs (e.g. "game.stasis").
    // Treat that case as "current directory" so we never attempt set_current_dir("").
    PathBuf::from(".")
}

fn prepare_play_asset_working_dir(watch_dir: &Path) -> Result<PathBuf, String> {
    let Some(project_root) = watch_dir
        .ancestors()
        .find(|ancestor| ancestor.join(DEFAULT_ASSET_MANIFEST_PATH).is_file())
    else {
        return Ok(watch_dir.to_path_buf());
    };
    let relative_watch_dir = watch_dir.strip_prefix(project_root).map_err(|error| {
        format!(
            "failed to map play directory {} into asset project {}: {error}",
            watch_dir.display(),
            project_root.display()
        )
    })?;
    let prepared_root = project_root.join(".stasis_cache/play-assets");
    if prepared_root.exists() {
        fs::remove_dir_all(&prepared_root).map_err(|error| {
            format!(
                "failed to clear prepared play assets {}: {error}",
                prepared_root.display()
            )
        })?;
    }
    let resolved = load_project_asset_manifest(project_root, AssetLimits::default())
        .map_err(|error| format!("failed to resolve play assets: {error}"))?;
    prepare_asset_bundle(
        &resolved,
        &prepared_root,
        project_root.join(".stasis_cache/assets"),
    )
    .map_err(|error| format!("failed to prepare play assets: {error}"))?;
    let prepared_watch_dir = prepared_root.join(relative_watch_dir);
    fs::create_dir_all(&prepared_watch_dir).map_err(|error| {
        format!(
            "failed to create prepared play directory {}: {error}",
            prepared_watch_dir.display()
        )
    })?;
    Ok(prepared_watch_dir)
}

const PLAY_INPUT_MAX_FRAMES: usize = 10_000;
const PLAY_INPUT_MAX_POINTERS: usize = 8;
const PLAY_INPUT_MAX_FILE_BYTES: u64 = 16 * 1024 * 1024;
const HOST_I_POINTER_COUNT: usize = 7;
const HOST_I_DROPPED_POINTERS: usize = 8;
const HOST_I_POINTER_BASE: usize = 544;
const HOST_I_POINTER_STRIDE: usize = 4;
const HOST_F_POINTER_STRIDE: usize = 6;
const HOST_F_LOGICAL_W: usize = 50;
const HOST_F_LOGICAL_H: usize = 51;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayInputScriptDocument {
    version: u32,
    frames: Vec<PlayInputFrame>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PlayInputFrame {
    frame: u64,
    pointers: Vec<PlayInputPointer>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlayInputPointer {
    id: i32,
    is_down: bool,
    went_down: bool,
    went_up: bool,
    x: f32,
    y: f32,
}

#[derive(Debug)]
struct PlayInputTimeline {
    frames: Vec<PlayInputFrame>,
    next_frame: usize,
    pointers: Vec<PlayInputPointer>,
}

fn validate_play_input_script(
    document: PlayInputScriptDocument,
) -> Result<PlayInputTimeline, String> {
    if document.version != 1 {
        return Err(format!(
            "unsupported input-script version {} (expected 1)",
            document.version
        ));
    }
    if document.frames.len() > PLAY_INPUT_MAX_FRAMES {
        return Err(format!(
            "input-script has too many frames (maximum {PLAY_INPUT_MAX_FRAMES})"
        ));
    }
    let mut previous_frame = 0u64;
    for frame in &document.frames {
        if frame.frame == 0 || frame.frame > i32::MAX as u64 {
            return Err("input-script frame must be between 1 and 2147483647".to_string());
        }
        if frame.frame <= previous_frame {
            return Err("input-script frames must be strictly increasing".to_string());
        }
        previous_frame = frame.frame;
        if frame.pointers.len() > PLAY_INPUT_MAX_POINTERS {
            return Err(format!(
                "input-script frame {} has too many pointers (maximum {PLAY_INPUT_MAX_POINTERS})",
                frame.frame
            ));
        }
        let mut ids = BTreeSet::new();
        for pointer in &frame.pointers {
            if pointer.id < 0 {
                return Err(format!(
                    "input-script frame {} pointer id must be non-negative",
                    frame.frame
                ));
            }
            if !ids.insert(pointer.id) {
                return Err(format!(
                    "input-script frame {} contains duplicate pointer id {}",
                    frame.frame, pointer.id
                ));
            }
            if !pointer.x.is_finite()
                || !pointer.y.is_finite()
                || pointer.x < 0.0
                || pointer.y < 0.0
            {
                return Err(format!(
                    "input-script frame {} pointer coordinates must be finite and non-negative",
                    frame.frame
                ));
            }
            if pointer.went_down && pointer.went_up {
                return Err(format!(
                    "input-script frame {} pointer cannot go down and up together",
                    frame.frame
                ));
            }
            if pointer.went_down && !pointer.is_down {
                return Err(format!(
                    "input-script frame {} wentDown requires isDown=true",
                    frame.frame
                ));
            }
            if pointer.went_up && pointer.is_down {
                return Err(format!(
                    "input-script frame {} wentUp requires isDown=false",
                    frame.frame
                ));
            }
        }
    }
    Ok(PlayInputTimeline {
        frames: document.frames,
        next_frame: 0,
        pointers: Vec::new(),
    })
}

fn load_play_input_script(path: &Path, launch_dir: &Path) -> Result<PlayInputTimeline, String> {
    let resolved = resolve_play_sidecar_path(path, launch_dir);
    let metadata = fs::metadata(&resolved).map_err(|error| {
        format!(
            "failed to inspect input-script {}: {error}",
            resolved.display()
        )
    })?;
    validate_play_input_script_size(metadata.len())?;
    let file = fs::File::open(&resolved).map_err(|error| {
        format!(
            "failed to open input-script {}: {error}",
            resolved.display()
        )
    })?;
    let mut source = String::with_capacity(metadata.len() as usize);
    file.take(PLAY_INPUT_MAX_FILE_BYTES + 1)
        .read_to_string(&mut source)
        .map_err(|error| {
            format!(
                "failed to read input-script {}: {error}",
                resolved.display()
            )
        })?;
    validate_play_input_script_size(source.len() as u64)?;
    let document: PlayInputScriptDocument = serde_json::from_str(&source).map_err(|error| {
        format!(
            "failed to parse input-script {}: {error}",
            resolved.display()
        )
    })?;
    validate_play_input_script(document)
}

fn validate_play_input_script_size(byte_len: u64) -> Result<(), String> {
    if byte_len > PLAY_INPUT_MAX_FILE_BYTES {
        return Err(format!(
            "input-script is too large ({byte_len} bytes; maximum {PLAY_INPUT_MAX_FILE_BYTES})"
        ));
    }
    Ok(())
}

fn validate_play_input_script_ticks(
    max_ticks: Option<u64>,
    timeline: &PlayInputTimeline,
) -> Result<(), String> {
    if let Some(limit) = max_ticks {
        if timeline
            .frames
            .last()
            .is_some_and(|frame| frame.frame > limit)
        {
            return Err("max_ticks must reach the final input-script frame".to_string());
        }
    }
    Ok(())
}

fn apply_play_input_frame(
    timeline: &mut PlayInputTimeline,
    frame: u64,
    host_i32: &mut [i32],
    host_f32: &mut [f32],
) -> Result<(), String> {
    if host_i32.len() < HOST_I_POINTER_BASE + PLAY_INPUT_MAX_POINTERS * HOST_I_POINTER_STRIDE
        || host_f32.len() <= HOST_F_LOGICAL_H
    {
        return Err("host frame buffers are too small for input-script pointers".to_string());
    }
    let viewport_w = host_f32[HOST_F_LOGICAL_W];
    let viewport_h = host_f32[HOST_F_LOGICAL_H];
    if viewport_w <= 0.0 || viewport_h <= 0.0 {
        return Err("input-script requires positive host viewport dimensions".to_string());
    }

    let previous = timeline.pointers.clone();
    let scripted = timeline
        .frames
        .get(timeline.next_frame)
        .is_some_and(|event| event.frame == frame);
    if scripted {
        timeline.pointers = timeline.frames[timeline.next_frame].pointers.clone();
        timeline.next_frame += 1;
    } else {
        for pointer in &mut timeline.pointers {
            pointer.went_down = false;
            pointer.went_up = false;
        }
    }

    host_i32[HOST_I_POINTER_COUNT] = timeline.pointers.len() as i32;
    host_i32[HOST_I_DROPPED_POINTERS] = 0;
    for slot in 0..PLAY_INPUT_MAX_POINTERS {
        let i32_base = HOST_I_POINTER_BASE + slot * HOST_I_POINTER_STRIDE;
        let f32_base = slot * HOST_F_POINTER_STRIDE;
        for value in &mut host_i32[i32_base..i32_base + HOST_I_POINTER_STRIDE] {
            *value = 0;
        }
        for value in &mut host_f32[f32_base..f32_base + HOST_F_POINTER_STRIDE] {
            *value = 0.0;
        }
    }
    for (slot, pointer) in timeline.pointers.iter().enumerate() {
        if pointer.x > viewport_w || pointer.y > viewport_h {
            return Err(format!(
                "input-script frame {frame} pointer {} is outside the {}x{} viewport",
                pointer.id, viewport_w, viewport_h
            ));
        }
        let prior = previous.iter().find(|candidate| candidate.id == pointer.id);
        let dx = prior.map_or(0.0, |value| pointer.x - value.x);
        let dy = prior.map_or(0.0, |value| pointer.y - value.y);
        let i32_base = HOST_I_POINTER_BASE + slot * HOST_I_POINTER_STRIDE;
        let f32_base = slot * HOST_F_POINTER_STRIDE;
        host_i32[i32_base] = pointer.id;
        host_i32[i32_base + 1] = i32::from(pointer.is_down);
        host_i32[i32_base + 2] = i32::from(pointer.went_down);
        host_i32[i32_base + 3] = i32::from(pointer.went_up);
        host_f32[f32_base] = pointer.x;
        host_f32[f32_base + 1] = pointer.y;
        host_f32[f32_base + 2] = dx;
        host_f32[f32_base + 3] = dy;
        host_f32[f32_base + 4] = (pointer.x / viewport_w).clamp(0.0, 1.0);
        host_f32[f32_base + 5] = (pointer.y / viewport_h).clamp(0.0, 1.0);
    }
    Ok(())
}

fn run_guest_main_with_initial_host_requests(
    initialize_requests: impl FnOnce() -> Result<(), String>,
    run_main: impl FnOnce() -> Result<i32, String>,
    apply_requests: impl FnOnce() -> Result<(), String>,
) -> Result<i32, String> {
    initialize_requests()?;
    let result = run_main()?;
    if result == 0 {
        apply_requests()?;
    }
    Ok(result)
}

pub fn run_play_in_process(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
) -> Result<(), String> {
    run_play_in_process_inner(
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        None,
        tick_sleep_micros,
        max_ticks,
        None,
        None,
    )
}

fn resolve_play_window_title(watch_file: &Path, configured_title: Option<&str>) -> String {
    configured_title
        .filter(|title| !title.is_empty())
        .map(str::to_string)
        .or_else(|| {
            watch_file
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
        })
        .unwrap_or_else(|| "stasis".to_string())
}

#[allow(clippy::too_many_arguments)]
pub fn run_play_in_process_with_window_title(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
    window_title: &str,
) -> Result<(), String> {
    run_play_in_process_inner(
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        None,
        tick_sleep_micros,
        max_ticks,
        Some(window_title),
        None,
    )
}

pub fn run_play_in_process_with_input_script(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
    input_script: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
) -> Result<(), String> {
    run_play_in_process_inner(
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        input_script,
        tick_sleep_micros,
        max_ticks,
        None,
        None,
    )
}

pub fn run_live_in_process(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
    server: stasis_runner::live::LiveSessionServer,
    config: LiveRunConfig,
) -> Result<(), String> {
    run_live_in_process_with_data(
        watch_file,
        watch_dir,
        None,
        None,
        tick_sleep_micros,
        max_ticks,
        server,
        config,
    )
}

#[allow(clippy::too_many_arguments)]
pub fn run_live_in_process_with_data(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
    server: stasis_runner::live::LiveSessionServer,
    config: LiveRunConfig,
) -> Result<(), String> {
    run_play_in_process_inner(
        watch_file,
        watch_dir,
        data_bind_json,
        data_bind_struct_meta,
        None,
        tick_sleep_micros,
        max_ticks,
        None,
        Some((server, config)),
    )
}

#[allow(clippy::too_many_arguments)]
fn run_play_in_process_inner(
    watch_file: &Path,
    watch_dir: Option<&Path>,
    data_bind_json: Option<&Path>,
    data_bind_struct_meta: Option<&Path>,
    input_script: Option<&Path>,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
    window_title: Option<&str>,
    live: Option<(stasis_runner::live::LiveSessionServer, LiveRunConfig)>,
) -> Result<(), String> {
    let watch_dir = resolve_play_watch_dir(watch_file, watch_dir);
    let launch_dir = std::env::current_dir()
        .map_err(|error| format!("failed to read current directory before play launch: {error}"))?;
    let data_binding_paths = resolve_play_data_binding_paths(
        watch_file,
        &launch_dir,
        data_bind_json,
        data_bind_struct_meta,
    )?;
    let mut input_timeline = input_script
        .map(|path| load_play_input_script(path, &launch_dir))
        .transpose()?;
    if let Some(timeline) = input_timeline.as_ref() {
        validate_play_input_script_ticks(max_ticks, timeline)?;
    }

    // Make relative asset paths (e.g. "assets/ball.svg") resolve against the game directory.
    // Use the watch dir so dev workflows stay consistent across `stasis.exe` launch locations.
    let watch_dir_abs = watch_dir
        .canonicalize()
        .unwrap_or_else(|_| watch_dir.clone());

    let mut watcher = WatchService::start(&watch_dir).map_err(|error| {
        format!(
            "failed to start watch service for {}: {error}",
            watch_dir.display()
        )
    })?;
    let mut data_watchers = Vec::new();
    let mut watched_data_dirs = BTreeSet::new();
    for (json_path, _) in &data_binding_paths {
        let Some(parent) = json_path.parent() else {
            continue;
        };
        let parent_abs = parent
            .canonicalize()
            .unwrap_or_else(|_| parent.to_path_buf());
        if parent_abs.starts_with(&watch_dir_abs) {
            continue;
        }
        let key = normalize_watch_path_for_log(&parent_abs);
        if watched_data_dirs.insert(key) {
            data_watchers.push(WatchService::start(&parent_abs).map_err(|error| {
                format!(
                    "failed to watch data directory {}: {error}",
                    parent_abs.display()
                )
            })?);
        }
    }

    let root_path = watch_file
        .canonicalize()
        .unwrap_or_else(|_| watch_file.to_path_buf());
    let root_path_str = root_path.to_string_lossy().to_string();
    let repository_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let canonical_repository = repository_root
        .canonicalize()
        .unwrap_or_else(|_| repository_root.clone());
    let project_root =
        resolve_play_project_root(&root_path, &watch_dir_abs, &canonical_repository)?;

    let asset_working_dir = prepare_play_asset_working_dir(&watch_dir_abs)?;
    std::env::set_current_dir(&asset_working_dir).map_err(|error| {
        format!(
            "failed to set current directory to {}: {error}",
            asset_working_dir.display()
        )
    })?;

    let mut watch_dependency_paths =
        Some(collect_watch_dependency_paths(&project_root, &root_path)?);

    // Allocate and register all global buffers used by HostFrame / gfx_cmd + window requests.
    let mut host_i32: Vec<i32> = vec![0; 768];
    let mut host_f32: Vec<f32> = vec![0.0; 64];
    let mut gfx_cmd_i32: Vec<i32> = vec![0; 18464];
    let mut gfx_cmd_f32: Vec<f32> = vec![0.0; 108676];
    let mut gfx_cmd_u8: Vec<u8> = vec![0; 65536];

    let mut host_req_seq: i32 = 0;
    let mut host_req_flags: i32 = 0;
    let mut host_req_window_w_px: i32 = 0;
    let mut host_req_window_h_px: i32 = 0;

    stasis_dynload::register_global_i32_array(
        hash_global_path("host_i32"),
        0,
        host_i32.as_mut_ptr(),
        host_i32.len(),
    );
    stasis_dynload::register_global_f32_array(
        hash_global_path("host_f32"),
        0,
        host_f32.as_mut_ptr(),
        host_f32.len(),
    );
    stasis_dynload::register_global_i32_array(
        hash_global_path("gfx_cmd_i32"),
        0,
        gfx_cmd_i32.as_mut_ptr(),
        gfx_cmd_i32.len(),
    );
    stasis_dynload::register_global_f32_array(
        hash_global_path("gfx_cmd_f32"),
        0,
        gfx_cmd_f32.as_mut_ptr(),
        gfx_cmd_f32.len(),
    );
    stasis_dynload::register_global_u8_array(
        hash_global_path("gfx_cmd_u8"),
        0,
        gfx_cmd_u8.as_mut_ptr(),
        gfx_cmd_u8.len(),
    );

    stasis_dynload::register_global_i32_ptr(hash_global_path("host_req_seq"), &mut host_req_seq);
    stasis_dynload::register_global_i32_ptr(
        hash_global_path("host_req_flags"),
        &mut host_req_flags,
    );
    stasis_dynload::register_global_i32_ptr(
        hash_global_path("host_req_window_w_px"),
        &mut host_req_window_w_px,
    );
    stasis_dynload::register_global_i32_ptr(
        hash_global_path("host_req_window_h_px"),
        &mut host_req_window_h_px,
    );

    let gfx = stasis_dynload::StasisGraphicsApi::load_default()?;
    let configured_title = window_title.or_else(|| {
        live.as_ref()
            .and_then(|(_, config)| config.window_title.as_deref())
    });
    let title = resolve_play_window_title(watch_file, configured_title);
    // Create a small default window up-front so runtime calls (fonts/sprites) succeed during guest main().
    // Guest `init_window(...)` requests will be applied immediately after main returns.
    let _ = gfx.init_window(800, 600, &title)?;

    let mut jit = JitProcess::new();
    jit.set_project_root(project_root.to_string_lossy())?;
    let root_source = fs::read_to_string(&root_path)
        .map_err(|error| format!("failed to read {}: {error}", root_path.display()))?;
    jit.upsert_file(root_path_str.clone(), root_source);
    let _ = jit
        .compile()
        .map_err(|error| format!("initial JIT compile failed: {error:?}"))?;
    let mut tick_budget_generation = 0u64;
    let mut tick_budget = jit
        .tick_budget_us()?
        .map(|budget| TickBudgetDiagnostics::new(budget, tick_budget_generation));
    load_and_apply_play_data_bindings(&data_binding_paths, Some(&jit))?;
    let package = jit
        .build_engine_package(&EngineEntrypoints::runtime_default())
        .map_err(|error| format!("failed to build engine package: {error}"))?;

    // Establish the request sequence baseline before guest startup. Otherwise the
    // runtime's first apply call treats main()'s request as its baseline and drops it.
    let main_rc = run_guest_main_with_initial_host_requests(
        || gfx.host_bulk_init(&host_req_seq),
        || {
            jit.execute_i32_noarg_by_name("main")
                .map_err(format_live_main_error)
        },
        || {
            gfx.host_bulk_apply_requests(
                &host_req_seq,
                &host_req_flags,
                &host_req_window_w_px,
                &host_req_window_h_px,
            )
        },
    )?;
    if main_rc != 0 {
        return Err(format!("guest main() returned non-zero status {main_rc}"));
    }

    if max_ticks == Some(0) {
        return Ok(());
    }

    stasis_dynload::begin_jit_host_entry_session(package.host_entry_targets(1)?)?;
    let mut tick_code_ptr = stasis_dynload::jit_host_tick_trampoline_ptr() as u64;
    let mut render_code_ptr = stasis_dynload::jit_host_render_trampoline_ptr() as u64;
    let mut requested_watch_revision = 0_u64;
    let mut finished_watch_revision = 0_u64;
    let mut watch_patch_job: Option<WatchPatchJob> = None;
    let mut live = live
        .map(|(server, config)| LiveWorkspace::new(server, config, &jit))
        .transpose()?;

    let mut ticks_executed: u64 = 0;
    loop {
        if let Some(live) = live.as_mut() {
            live.process_boundary(
                ticks_executed,
                &mut jit,
                &mut tick_code_ptr,
                &mut render_code_ptr,
            );
            if live.should_quit() {
                break;
            }
        }
        let completed_watch =
            watch_patch_job
                .as_ref()
                .and_then(|job| match job.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(format!(
                        "watch compile worker {} disconnected",
                        job.revision
                    ))),
                });
        if let Some(result) = completed_watch {
            let job = watch_patch_job.take().expect("completed watch job exists");
            let _ = job.worker.join();
            finished_watch_revision = job.revision;
            if job.revision < requested_watch_revision {
                println!(
                    "[watch] discarded superseded revision {} (latest {})",
                    job.revision, requested_watch_revision
                );
            } else {
                match result {
                    Err(error) => println!("[swap] revision {} rejected: {error}", job.revision),
                    Ok(prepared) => {
                        let emitted = prepared
                            .candidate
                            .generation_metadata()
                            .map_or(0, |metadata| metadata.emitted_function_ids.len());
                        let reused = prepared
                            .candidate
                            .generation_metadata()
                            .map_or(0, |metadata| metadata.reused_function_ids.len());
                        let candidate_tick_budget = prepared.candidate.tick_budget_us();
                        let commit_started = Instant::now();
                        let commit_result = commit_play_candidate_between_ticks(
                            &mut jit,
                            prepared.candidate,
                            prepared.package.symbol_code_ptrs.keys().cloned().collect(),
                            &prepared.package,
                        );
                        let commit_ms = commit_started.elapsed().as_millis();
                        match commit_result {
                            Err(error) => println!(
                                "[swap] revision {} aborted (compile={}ms package={}ms commit={}ms): {error}",
                                prepared.revision,
                                prepared.compile_ms,
                                prepared.package_ms,
                                commit_ms
                            ),
                            Ok(entrypoints) => {
                                tick_code_ptr = entrypoints.tick_code_ptr;
                                render_code_ptr = entrypoints.render_code_ptr;
                                if let Ok(Some(candidate_tick_budget)) = candidate_tick_budget {
                                    if let Some(report) = update_tick_budget_after_swap(
                                        &mut tick_budget,
                                        &mut tick_budget_generation,
                                        Some(candidate_tick_budget),
                                        true,
                                    ) {
                                        println!("{report}");
                                    }
                                }
                                watch_dependency_paths = Some(prepared.dependency_paths);
                                if let Some(live) = live.as_mut() {
                                    live.refresh_after_external_edit(&jit);
                                }
                                println!(
                                    "[swap] revision {} published re_jit={} reused={} compile={}ms package={}ms commit={}ms",
                                    prepared.revision,
                                    emitted,
                                    reused,
                                    prepared.compile_ms,
                                    prepared.package_ms,
                                    commit_ms
                                );
                            }
                        }
                    }
                }
            }
        }
        if watch_patch_job.is_none() && requested_watch_revision > finished_watch_revision {
            match start_watch_patch_job(
                requested_watch_revision,
                jit.staged_candidate(),
                project_root.clone(),
                root_path.clone(),
                root_path_str.clone(),
            ) {
                Ok(job) => watch_patch_job = Some(job),
                Err(error) => {
                    finished_watch_revision = requested_watch_revision;
                    println!("[watch] failed starting background compile: {error}");
                }
            }
        }
        // Drain file events and recompile at tick boundaries (all-or-nothing).
        let mut needs_recompile = false;
        let mut ignored_changes: u32 = 0;
        let mut triggered_paths: Vec<String> = Vec::new();
        let mut watch_events = watcher.drain_stasis_changes();
        for data_watcher in &mut data_watchers {
            watch_events.extend(data_watcher.drain_stasis_changes());
        }
        let mut needs_data_reload = false;
        for event in watch_events {
            if live
                .as_mut()
                .is_some_and(|workspace| workspace.consumes_self_write(&event.path))
            {
                ignored_changes = ignored_changes.saturating_add(1);
                continue;
            }
            let event_path = normalize_watch_path_for_log(&event.path);
            let is_data_event = data_binding_paths.iter().any(|(json_path, meta_path)| {
                event_path == normalize_watch_path_for_log(json_path)
                    || event_path == normalize_watch_path_for_log(meta_path)
            });
            if is_data_event {
                needs_data_reload = true;
                continue;
            }
            let submit = should_submit_watch_event(
                &event,
                Some(&root_path),
                watch_dependency_paths.as_ref(),
            );
            if submit {
                needs_recompile = true;
                triggered_paths.push(normalize_watch_path_for_log(&event.path));
            } else {
                ignored_changes = ignored_changes.saturating_add(1);
            }
        }
        if needs_data_reload {
            match load_and_apply_play_data_bindings(&data_binding_paths, Some(&jit)) {
                Ok(()) => println!("[data] rebound {} file(s)", data_binding_paths.len()),
                Err(error) => println!("[data] reload rejected: {error}"),
            }
        }
        if ignored_changes > 0 && !needs_recompile {
            println!("[watch] ignored {ignored_changes} change(s) (not in dependency graph)");
        }
        if needs_recompile {
            let latest_watch_path = triggered_paths
                .first()
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            requested_watch_revision = requested_watch_revision.saturating_add(1);
            println!(
                "[watch] queued revision {} for {}{}",
                requested_watch_revision,
                latest_watch_path,
                if watch_patch_job.is_some() {
                    " (superseding in-flight compile)"
                } else {
                    ""
                }
            );
        }

        gfx.host_get_frame(&mut host_i32, &mut host_f32)?;
        if host_i32.get(9).copied().unwrap_or(0) != 0 {
            break;
        }
        if let Some(timeline) = input_timeline.as_mut() {
            apply_play_input_frame(
                timeline,
                ticks_executed.saturating_add(1),
                &mut host_i32,
                &mut host_f32,
            )?;
        }
        gfx.host_bulk_apply_requests(
            &host_req_seq,
            &host_req_flags,
            &host_req_window_w_px,
            &host_req_window_h_px,
        )?;

        let run_tick = live.as_ref().is_none_or(LiveWorkspace::should_run_tick);
        let mut tick_micros = 0;
        if run_tick {
            let tick_started = Instant::now();
            let tick_rc = stasis_dynload::invoke_noarg_i32(tick_code_ptr as usize)?;
            tick_micros = tick_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
            if let Some(budget) = tick_budget.as_mut() {
                budget.record(tick_micros);
            }
            if tick_rc != 0 {
                break;
            }
        }
        let render_started = Instant::now();
        let render_rc = stasis_dynload::invoke_noarg_i32(render_code_ptr as usize)?;
        let render_micros = render_started.elapsed().as_micros().min(u64::MAX as u128) as u64;
        if render_rc != 0 {
            break;
        }

        gfx.host_set_performance_metrics(tick_micros, render_micros)?;
        gfx.gfx_submit_u8(&gfx_cmd_i32, &gfx_cmd_f32, &gfx_cmd_u8)?;
        if tick_sleep_micros > 0 {
            let ms = (tick_sleep_micros / 1000) as i32;
            if ms > 0 {
                gfx.sleep_ms(ms)?;
            }
        }

        if let Some(live) = live.as_mut() {
            if run_tick {
                ticks_executed = ticks_executed.saturating_add(1);
                live.after_tick();
            }
            live.publish_watches(ticks_executed, &jit);
        } else {
            ticks_executed = ticks_executed.saturating_add(1);
        }
        if let Some(limit) = max_ticks {
            if ticks_executed >= limit {
                break;
            }
        }
    }

    if let Some(job) = watch_patch_job.take() {
        let _ = job.worker.join();
    }
    if let Some(budget) = tick_budget {
        println!("{}", budget.report());
    }

    Ok(())
}

#[derive(Clone, Copy, Debug)]
struct PlayEntrypoints {
    tick_code_ptr: u64,
    render_code_ptr: u64,
}

fn commit_play_candidate_between_ticks(
    active: &mut JitProcess,
    candidate: JitProcess,
    changed_functions: Vec<String>,
    package: &JitEnginePackage,
) -> Result<PlayEntrypoints, String> {
    let revision = stasis_dynload::jit_host_entry_targets()
        .map_or(1, |targets| targets.revision.saturating_add(1));
    let targets = package.host_entry_targets(revision)?;
    stasis_dynload::validate_jit_host_entry_targets(&targets)?;
    let mut preview = plan_state_migration(
        &active.state_layout(),
        &candidate.state_layout(),
        changed_functions,
        false,
        None,
    )?;
    finalize_runtime_preview(&candidate, &mut preview);
    activate_candidate_transactionally(
        Some(&*active),
        &candidate,
        &preview,
        package.on_code_swap_code_ptr.is_some(),
        || {
            package.on_code_swap_code_ptr.map_or(Ok(()), |code_ptr| {
                stasis_dynload::invoke_code_swap_hook(code_ptr as usize)
            })
        },
        Result::is_ok,
    )??;
    stasis_dynload::publish_jit_host_entry_targets(targets)?;
    active.accept_staged_candidate(candidate);
    Ok(PlayEntrypoints {
        tick_code_ptr: stasis_dynload::jit_host_tick_trampoline_ptr() as u64,
        render_code_ptr: stasis_dynload::jit_host_render_trampoline_ptr() as u64,
    })
}

struct PreparedWatchPatch {
    revision: u64,
    candidate: JitProcess,
    package: JitEnginePackage,
    compile_ms: u128,
    package_ms: u128,
    dependency_paths: BTreeSet<String>,
}

struct WatchPatchJob {
    revision: u64,
    receiver: std::sync::mpsc::Receiver<Result<PreparedWatchPatch, String>>,
    worker: std::thread::JoinHandle<()>,
}

fn start_watch_patch_job(
    revision: u64,
    mut candidate: JitProcess,
    project_root: PathBuf,
    root_path: PathBuf,
    root_path_str: String,
) -> Result<WatchPatchJob, String> {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let worker = std::thread::Builder::new()
        .name(format!("stasis-watch-compile-{revision}"))
        .spawn(move || {
            let result = (|| {
                if let Ok(next_root_source) = fs::read_to_string(&root_path) {
                    candidate.upsert_file(root_path_str.clone(), next_root_source);
                }
                let _ = candidate.refresh_imported_sources_from_disk(&root_path_str);
                let compile_started = Instant::now();
                candidate
                    .compile_staged()
                    .map_err(|error| format!("compile failed: {error:?}"))?;
                let compile_ms = compile_started.elapsed().as_millis();
                let package_started = Instant::now();
                let package = candidate
                    .build_engine_package(&EngineEntrypoints::runtime_default())
                    .map_err(|error| format!("build_engine_package failed: {error}"))?;
                let package_ms = package_started.elapsed().as_millis();
                let dependency_paths = collect_watch_dependency_paths(&project_root, &root_path)?;
                Ok(PreparedWatchPatch {
                    revision,
                    candidate,
                    package,
                    compile_ms,
                    package_ms,
                    dependency_paths,
                })
            })();
            let _ = sender.send(result);
        })
        .map_err(|error| format!("failed starting watch compile worker: {error}"))?;
    Ok(WatchPatchJob {
        revision,
        receiver,
        worker,
    })
}

fn format_live_main_error(error: String) -> String {
    if error.contains("not i32-returning") || error.contains("not a no-argument function") {
        format!("interactive entry signature mismatch: expected `function main(): i32`; {error}")
    } else {
        format!("guest main() failed: {error}")
    }
}

pub fn run_with_real_backend(mut config: RunnerConfig) -> RunnerSummary {
    let (prepared_tx, prepared_rx) = std::sync::mpsc::sync_channel(1);
    normalize_real_backend_config_paths(&mut config);
    let explicit = config
        .watch_directory
        .as_deref()
        .or(config.inject_file_change.as_deref())
        .unwrap_or_else(|| Path::new("."));
    let mut project_root = explicit
        .canonicalize()
        .unwrap_or_else(|_| explicit.to_path_buf());
    if project_root.is_file() {
        project_root.pop();
    }
    if let Some(repository_root) = project_root.ancestors().find(|ancestor| {
        ancestor.join("Cargo.toml").is_file() && ancestor.join("docs/spec.md").is_file()
    }) {
        project_root = repository_root.to_path_buf();
    }
    let backend = IncrementalCompilerBackend::new_for_project_with_prepared_jit_swaps(
        project_root.clone(),
        prepared_tx,
    )
    .expect("runner compiler project root must be absolute");
    run_with_backend_and_prepared_swaps(config, backend, Some(prepared_rx), Some(project_root))
}

fn resolve_play_project_root(
    root_path: &Path,
    watch_dir: &Path,
    repository_root: &Path,
) -> Result<PathBuf, String> {
    if !root_path.starts_with(watch_dir) {
        return Err(format!(
            "play entry {} is outside watch directory {}",
            root_path.display(),
            watch_dir.display()
        ));
    }
    if root_path.starts_with(repository_root) {
        Ok(repository_root.to_path_buf())
    } else {
        Ok(watch_dir.to_path_buf())
    }
}

fn normalize_real_backend_config_paths(config: &mut RunnerConfig) {
    if let Some(path) = config.inject_file_change.as_mut() {
        *path = normalize_watch_path_for_compare(path);
    }
    if let Some(path) = config.watch_directory.as_mut() {
        *path = normalize_watch_path_for_compare(path);
    }
}

fn is_stasis_source_file(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
}

fn is_test_stasis_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".test.stasis"))
}

fn contains_entry_function(path: &Path) -> bool {
    let Ok(content) = fs::read_to_string(path) else {
        return false;
    };
    content.contains("function main(")
        || content.contains("function tick(")
        || content.contains("function @inline main(")
        || content.contains("function @inline tick(")
}

fn collect_stasis_sources_recursive(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            collect_stasis_sources_recursive(&path, out);
        } else if is_stasis_source_file(&path) {
            out.push(path);
        }
    }
}

fn normalize_watch_path_for_compare(path: &Path) -> PathBuf {
    if path.exists() {
        if let Ok(canonical) = fs::canonicalize(path) {
            return canonical;
        }
    }
    if path.is_absolute() {
        return path.to_path_buf();
    }
    match std::env::current_dir() {
        Ok(cwd) => cwd.join(path),
        Err(_) => path.to_path_buf(),
    }
}

fn normalize_watch_path_for_log(path: &Path) -> String {
    let normalized = normalize_watch_path_for_compare(path);
    let mut text = normalized.to_string_lossy().to_string();
    text = text.replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    if let Some(stripped) = text.strip_prefix("\\\\?\\") {
        text = stripped.to_string();
    }
    text.to_ascii_lowercase()
}

fn collect_watch_dependency_paths(
    project_root: &Path,
    root_source: &Path,
) -> Result<BTreeSet<String>, String> {
    if !root_source.exists() {
        return Err(format!(
            "watch root source does not exist: {}",
            root_source.display()
        ));
    }
    let (graph, _) = stasis_compiler::frontend::module_graph::load_project_module_graph(
        project_root,
        root_source,
    )
    .map_err(|diagnostic| {
        format!(
            "{}:{}-{}: {}",
            diagnostic.path, diagnostic.start, diagnostic.end, diagnostic.message
        )
    })?;
    Ok(graph
        .modules()
        .keys()
        .map(|path| normalize_watch_path_for_log(&project_root.join(path)))
        .collect())
}

fn should_submit_watch_event(
    event: &FileChangeEvent,
    root_source: Option<&Path>,
    dependency_paths: Option<&BTreeSet<String>>,
) -> bool {
    let Some(root_source) = root_source else {
        return true;
    };
    let Some(dependency_paths) = dependency_paths else {
        return true;
    };
    let normalized_event = normalize_watch_path_for_log(&event.path);
    if normalized_event == normalize_watch_path_for_log(root_source) {
        return true;
    }
    dependency_paths.contains(&normalized_event)
}

fn infer_watch_directory_entry_source(watch_directory: &Path) -> Option<PathBuf> {
    if !watch_directory.is_dir() {
        return None;
    }

    for preferred in ["main.stasis", "game.stasis", "app.stasis"] {
        let candidate = watch_directory.join(preferred);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    let mut sources: Vec<PathBuf> = Vec::new();
    collect_stasis_sources_recursive(watch_directory, &mut sources);
    sources.retain(|path| !is_test_stasis_file(path));
    if sources.is_empty() {
        return None;
    }
    if sources.len() == 1 {
        return Some(sources[0].clone());
    }

    let entry_candidates: Vec<PathBuf> = sources
        .iter()
        .filter(|path| contains_entry_function(path))
        .cloned()
        .collect();
    if !entry_candidates.is_empty() {
        return Some(entry_candidates[0].clone());
    }
    Some(sources[0].clone())
}

fn resolve_initial_source_file(config: &RunnerConfig) -> Option<PathBuf> {
    if let Some(explicit) = config.inject_file_change.as_ref() {
        return Some(explicit.clone());
    }
    let watch_directory = config.watch_directory.as_deref()?;
    infer_watch_directory_entry_source(watch_directory)
}

pub fn run_with_backend<B: CompilerBackend>(config: RunnerConfig, backend: B) -> RunnerSummary {
    run_with_backend_and_prepared_swaps(config, backend, None, None)
}

fn run_with_backend_and_prepared_swaps<B: CompilerBackend>(
    config: RunnerConfig,
    backend: B,
    prepared_jit_swap_rx: Option<std::sync::mpsc::Receiver<PreparedJitSwap>>,
    compiler_project_root: Option<PathBuf>,
) -> RunnerSummary {
    let mut watcher = config
        .watch_directory
        .as_deref()
        .and_then(|dir| WatchService::start(dir).ok());
    let initial_source_file = resolve_initial_source_file(&config);
    let watch_project_root = compiler_project_root.or_else(|| {
        config
            .watch_directory
            .as_ref()
            .filter(|path| path.is_dir())
            .cloned()
    });
    let (mut watch_dependency_paths, initial_watch_error) = match (
        watch_project_root.as_deref(),
        initial_source_file.as_deref(),
    ) {
        (Some(project_root), Some(source)) => {
            match collect_watch_dependency_paths(project_root, source) {
                Ok(paths) => (Some(paths), None),
                Err(error) => (None, Some(error)),
            }
        }
        _ => (None, None),
    };
    let window = config.window;

    let host_set_contract = match resolve_host_set_contract(&config) {
        Ok(contract) => contract,
        Err(message) => {
            return RunnerSummary {
                ticks_executed: 0,
                compile_successes: 0,
                compile_failures: 1,
                compile_diagnostics: vec![message],
                hook_runs: 0,
                hook_failures: 0,
                hook_failure_reasons: Vec::new(),
                swap_commit_successes: 0,
                swap_commit_failures: 0,
                swap_failure_reasons: Vec::new(),
                swap_indicator_armed_count: 0,
                swap_flash_peak_ticks: 0,
                swap_flash_ticks_remaining: 0,
                last_compile_duration_ms: None,
                last_commit_duration_ms: None,
                window,
                last_swap_status: None,
                has_in_flight_work: false,
                events: Vec::new(),
                runtime_launches: 0,
                runtime_launch_failures: 0,
                runtime_launch_failure_reasons: Vec::new(),
                aot_linked_image_activations: 0,
                active_aot_linked_image_path: None,
                active_aot_linked_image_size_bytes: None,
                active_aot_linked_image_generation: None,
                retired_aot_linked_images: 0,
            };
        }
    };

    let mut pipeline = DevHotSwapPipeline::with_target_mode(backend, config.target_mode);
    pipeline.set_host_set_contract(
        Some(host_set_contract.host_set_id.clone()),
        Some(host_set_contract.host_set_hash),
    );
    let mut pointer_table = FunctionPointerTable::new();
    let mut hook_runs: u32 = 0;
    let mut hook_failures: u32 = 0;
    let mut hook_failure_reasons: Vec<String> = Vec::new();
    let mut swap_commit_successes: u32 = 0;
    let mut swap_commit_failures: u32 = 0;
    let mut swap_failure_reasons: Vec<String> = Vec::new();
    let mut swap_indicator_armed_count: u32 = 0;
    let mut swap_flash_peak_ticks: u32 = 0;
    let mut swap_flash_ticks_remaining: u32 = 0;
    let mut compile_successes: u32 = 0;
    let mut compile_failures: u32 = u32::from(initial_watch_error.is_some());
    let mut compile_diagnostics: Vec<String> = initial_watch_error.into_iter().collect();
    let mut last_compile_duration_ms: Option<u64> = None;
    let mut last_commit_duration_ms: Option<u64> = None;
    let mut last_seen_compile_id: Option<RequestId> = None;
    let mut last_seen_commit_id: Option<RequestId> = None;
    let mut last_swap_status: Option<SwapCommitStatus> = None;
    let mut events: Vec<RunnerEvent> = Vec::new();
    let mut file_change_sent = false;
    let hook_failure_reason = config.hook_failure_reason.clone();
    let swap_failure_reason = config.swap_failure_reason.clone();
    let mut pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
    let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
        BTreeMap::new();
    let requires_prepared_jit = prepared_jit_swap_rx.is_some();
    let mut pending_jit_candidates: BTreeMap<RequestId, JitProcess> = BTreeMap::new();
    let mut active_jit_candidate: Option<JitProcess> = None;
    let mut active_layout_hash: Option<LayoutHash> = None;
    let mut aot_linked_image_activations: u32 = 0;
    let mut active_aot_linked_image_path: Option<PathBuf> = None;
    let mut active_aot_linked_image_size_bytes: Option<u64> = None;
    let mut active_aot_linked_image_generation: Option<u64> = None;
    let mut retired_aot_linked_images: u32 = 0;

    let mut runtime_launcher = config
        .runtime_launch
        .then(|| initial_source_file.clone().map(RuntimeLauncher::new))
        .flatten();
    let mut runtime_launch_failures: u32 = 0;
    let mut runtime_launch_failure_reasons: Vec<String> = Vec::new();
    if config.runtime_launch && runtime_launcher.is_none() {
        runtime_launch_failures = 1;
        runtime_launch_failure_reasons.push(
            "runtime launch requested but no --watch-file source file is configured".to_string(),
        );
    }

    for tick in 0..config.max_ticks {
        if !file_change_sent {
            if let Some(path) = &initial_source_file {
                let event = FileChangeEvent::new(
                    path.clone(),
                    u64::from(tick) + 1,
                    TextSource::FileWatcher,
                    FileChangeKind::Modified,
                );
                pipeline.submit_file_change(event);
                file_change_sent = true;
            }
        }

        if let Some(watch_service) = watcher.as_mut() {
            let mut refresh_dependency_graph = false;
            for event in watch_service.drain_stasis_changes() {
                if should_submit_watch_event(
                    &event,
                    initial_source_file.as_deref(),
                    watch_dependency_paths.as_ref(),
                ) {
                    refresh_dependency_graph = true;
                    pipeline.submit_file_change(event);
                }
            }
            if refresh_dependency_graph {
                if let (Some(project_root), Some(root_source)) = (
                    watch_project_root.as_deref(),
                    initial_source_file.as_deref(),
                ) {
                    match collect_watch_dependency_paths(project_root, root_source) {
                        Ok(next_graph) => watch_dependency_paths = Some(next_graph),
                        Err(error) => {
                            compile_failures = compile_failures.saturating_add(1);
                            compile_diagnostics.push(error);
                        }
                    }
                }
            }
        }

        pipeline.pump_coordinator();
        capture_pending_aot_compile_metadata(&pipeline, &mut pending_aot_metadata);
        capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
        capture_prepared_jit_swaps(prepared_jit_swap_rx.as_ref(), &mut pending_jit_candidates);
        pipeline.process_commits_at_safe_point(|request| {
            let request_id = request.request_id;
            let request_layout_hash = request.layout_hash;
            let hook_may_mutate_state = commit_request_may_execute_runtime_hook(
                &request,
                &config,
                hook_failure_reason.is_some(),
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            );
            let prepared_candidate = pending_jit_candidates.remove(&request_id);
            let commit = || {
                apply_commit_request(
                    request,
                    &mut pointer_table,
                    &config,
                    &host_set_contract,
                    &mut hook_runs,
                    &mut hook_failures,
                    &mut hook_failure_reasons,
                    &mut swap_commit_successes,
                    &mut swap_commit_failures,
                    &mut swap_failure_reasons,
                    &mut events,
                    hook_failure_reason.as_ref(),
                    swap_failure_reason.as_ref(),
                    &pending_aot_metadata,
                    &pending_jit_code_ptr_overrides,
                )
            };
            let result = if config.target_mode == TargetMode::JitDev
                && (requires_prepared_jit || prepared_candidate.is_some())
            {
                apply_prepared_jit_transaction(
                    request_id,
                    prepared_candidate,
                    &mut active_jit_candidate,
                    hook_may_mutate_state,
                    commit,
                )
            } else {
                apply_runtime_state_transaction(
                    active_layout_hash,
                    request_layout_hash,
                    hook_may_mutate_state,
                    commit,
                )
            };
            let result = match result {
                Ok(result) => result,
                Err(message) => {
                    record_swap_failure(
                        &message,
                        &mut swap_commit_failures,
                        &mut swap_failure_reasons,
                    );
                    return SwapCommitResult::failed(request_id, message);
                }
            };
            if result.status == SwapCommitStatus::Success {
                active_layout_hash = Some(request_layout_hash);
            }
            result
        });

        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_compile_duration_ms,
            &mut last_commit_duration_ms,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            let aot_metadata = pending_aot_metadata.remove(&request_id);
            pending_jit_code_ptr_overrides.remove(&request_id);
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
                if config.target_mode == TargetMode::AotProd {
                    if let Some(metadata) = aot_metadata {
                        if let Some(linked_path) = metadata.linked_image_path {
                            if active_aot_linked_image_path
                                .as_ref()
                                .is_some_and(|active| active != &linked_path)
                            {
                                retired_aot_linked_images += 1;
                            }
                            active_aot_linked_image_path = Some(linked_path);
                            active_aot_linked_image_size_bytes = metadata.linked_image_size_bytes;
                            active_aot_linked_image_generation = pipeline
                                .last_commit_result()
                                .and_then(|result| result.new_generation.map(|value| value.0));
                            aot_linked_image_activations += 1;
                        }
                    }
                }
                if config.runtime_launch {
                    if let Some(launcher) = runtime_launcher.as_mut() {
                        launcher.restart();
                    }
                }
            }
        } else if let Some(last_compile) = pipeline.last_compile_result() {
            if last_compile.status == CompileStatus::Failed {
                pending_aot_metadata.remove(&last_compile.request_id);
                pending_jit_code_ptr_overrides.remove(&last_compile.request_id);
            }
        }
        if swap_flash_ticks_remaining > 0 {
            swap_flash_ticks_remaining -= 1;
        }
        thread::yield_now();
        sleep_for_tick(config.tick_sleep_micros);
    }

    let drain_start = std::time::Instant::now();
    while drain_start.elapsed() < Duration::from_secs(30) {
        if !pipeline.has_in_flight_work() && pipeline.pending_commit_requests() == 0 {
            break;
        }

        pipeline.pump_coordinator();
        capture_pending_aot_compile_metadata(&pipeline, &mut pending_aot_metadata);
        capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
        capture_prepared_jit_swaps(prepared_jit_swap_rx.as_ref(), &mut pending_jit_candidates);
        pipeline.process_commits_at_safe_point(|request| {
            let request_id = request.request_id;
            let request_layout_hash = request.layout_hash;
            let hook_may_mutate_state = commit_request_may_execute_runtime_hook(
                &request,
                &config,
                hook_failure_reason.is_some(),
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            );
            let prepared_candidate = pending_jit_candidates.remove(&request_id);
            let commit = || {
                apply_commit_request(
                    request,
                    &mut pointer_table,
                    &config,
                    &host_set_contract,
                    &mut hook_runs,
                    &mut hook_failures,
                    &mut hook_failure_reasons,
                    &mut swap_commit_successes,
                    &mut swap_commit_failures,
                    &mut swap_failure_reasons,
                    &mut events,
                    hook_failure_reason.as_ref(),
                    swap_failure_reason.as_ref(),
                    &pending_aot_metadata,
                    &pending_jit_code_ptr_overrides,
                )
            };
            let result = if config.target_mode == TargetMode::JitDev
                && (requires_prepared_jit || prepared_candidate.is_some())
            {
                apply_prepared_jit_transaction(
                    request_id,
                    prepared_candidate,
                    &mut active_jit_candidate,
                    hook_may_mutate_state,
                    commit,
                )
            } else {
                apply_runtime_state_transaction(
                    active_layout_hash,
                    request_layout_hash,
                    hook_may_mutate_state,
                    commit,
                )
            };
            let result = match result {
                Ok(result) => result,
                Err(message) => {
                    record_swap_failure(
                        &message,
                        &mut swap_commit_failures,
                        &mut swap_failure_reasons,
                    );
                    return SwapCommitResult::failed(request_id, message);
                }
            };
            if result.status == SwapCommitStatus::Success {
                active_layout_hash = Some(request_layout_hash);
            }
            result
        });
        pipeline.pump_coordinator();
        let new_commit = observe_pipeline_results(
            &pipeline,
            &mut last_seen_compile_id,
            &mut last_seen_commit_id,
            &mut compile_successes,
            &mut compile_failures,
            &mut compile_diagnostics,
            &mut last_compile_duration_ms,
            &mut last_commit_duration_ms,
            &mut last_swap_status,
            &mut events,
        );
        if let Some((request_id, status)) = new_commit {
            let aot_metadata = pending_aot_metadata.remove(&request_id);
            pending_jit_code_ptr_overrides.remove(&request_id);
            if status == SwapCommitStatus::Success {
                swap_indicator_armed_count += 1;
                swap_flash_ticks_remaining = SWAP_FLASH_TICKS_MAX;
                swap_flash_peak_ticks = swap_flash_peak_ticks.max(swap_flash_ticks_remaining);
                events.push(RunnerEvent::SwapIndicatorArmed {
                    request_id: request_id.0,
                    ticks: SWAP_FLASH_TICKS_MAX,
                });
                if config.target_mode == TargetMode::AotProd {
                    if let Some(metadata) = aot_metadata {
                        if let Some(linked_path) = metadata.linked_image_path {
                            if active_aot_linked_image_path
                                .as_ref()
                                .is_some_and(|active| active != &linked_path)
                            {
                                retired_aot_linked_images += 1;
                            }
                            active_aot_linked_image_path = Some(linked_path);
                            active_aot_linked_image_size_bytes = metadata.linked_image_size_bytes;
                            active_aot_linked_image_generation = pipeline
                                .last_commit_result()
                                .and_then(|result| result.new_generation.map(|value| value.0));
                            aot_linked_image_activations += 1;
                        }
                    }
                }
                if config.runtime_launch {
                    if let Some(launcher) = runtime_launcher.as_mut() {
                        launcher.restart();
                    }
                }
            }
        } else if let Some(last_compile) = pipeline.last_compile_result() {
            if last_compile.status == CompileStatus::Failed {
                pending_aot_metadata.remove(&last_compile.request_id);
                pending_jit_code_ptr_overrides.remove(&last_compile.request_id);
            }
        }
        thread::yield_now();
        thread::sleep(Duration::from_millis(1));
    }

    let runtime_launches = runtime_launcher
        .as_ref()
        .map(|launcher| launcher.summary().launches)
        .unwrap_or(0);
    if let Some(launcher) = runtime_launcher.as_ref() {
        runtime_launch_failures += launcher.summary().failures;
        runtime_launch_failure_reasons.extend(launcher.summary().failure_reasons.iter().cloned());
    }

    let has_in_flight_work = pipeline.has_in_flight_work();
    events.push(RunnerEvent::Summary {
        ticks_executed: config.max_ticks,
        compile_successes,
        compile_failures,
        swap_commit_successes,
        swap_commit_failures,
        swap_indicator_armed_count,
        swap_flash_peak_ticks,
        swap_flash_ticks_remaining,
        window_width: window.map(|w| w.width),
        window_height: window.map(|w| w.height),
        has_in_flight_work,
        last_compile_duration_ms,
        last_commit_duration_ms,
    });

    RunnerSummary {
        ticks_executed: config.max_ticks,
        compile_successes,
        compile_failures,
        compile_diagnostics,
        hook_runs,
        hook_failures,
        hook_failure_reasons,
        swap_commit_successes,
        swap_commit_failures,
        swap_failure_reasons,
        swap_indicator_armed_count,
        swap_flash_peak_ticks,
        swap_flash_ticks_remaining,
        last_compile_duration_ms,
        last_commit_duration_ms,
        window,
        last_swap_status,
        has_in_flight_work,
        events,
        runtime_launches,
        runtime_launch_failures,
        runtime_launch_failure_reasons,
        aot_linked_image_activations,
        active_aot_linked_image_path,
        active_aot_linked_image_size_bytes,
        active_aot_linked_image_generation,
        retired_aot_linked_images,
    }
}

fn capture_pending_aot_compile_metadata(
    pipeline: &DevHotSwapPipeline,
    pending_aot_metadata: &mut BTreeMap<RequestId, PendingAotCompileMetadata>,
) {
    let Some(result) = pipeline.last_compile_result() else {
        return;
    };
    if result.status != CompileStatus::Success {
        return;
    }
    pending_aot_metadata
        .entry(result.request_id)
        .or_insert_with(|| PendingAotCompileMetadata {
            linked_image_path: result.aot_linked_image_path.clone(),
            linked_image_size_bytes: result.aot_linked_image_size_bytes,
            function_symbols: result.aot_function_symbols.clone(),
        });
}

fn capture_pending_jit_compile_metadata(
    pipeline: &DevHotSwapPipeline,
    pending_jit_code_ptr_overrides: &mut BTreeMap<RequestId, Vec<JitCodePtrOverride>>,
) {
    let Some(result) = pipeline.last_compile_result() else {
        return;
    };
    if result.status != CompileStatus::Success {
        return;
    }
    let Some(overrides) = result.jit_code_ptr_overrides.clone() else {
        return;
    };
    pending_jit_code_ptr_overrides
        .entry(result.request_id)
        .or_insert(overrides);
}

fn capture_prepared_jit_swaps(
    receiver: Option<&std::sync::mpsc::Receiver<PreparedJitSwap>>,
    pending: &mut BTreeMap<RequestId, JitProcess>,
) {
    let Some(receiver) = receiver else {
        return;
    };
    while let Ok(prepared) = receiver.try_recv() {
        pending.insert(prepared.request_id, prepared.candidate);
    }
}

fn format_layout_hash_hex(layout_hash: LayoutHash) -> String {
    let mut out = String::with_capacity(64);
    for byte in layout_hash.0 {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}

fn record_swap_failure(
    message: &str,
    swap_commit_failures: &mut u32,
    swap_failure_reasons: &mut Vec<String>,
) {
    *swap_commit_failures += 1;
    swap_failure_reasons.push(message.to_string());
}

fn commit_request_may_execute_runtime_hook(
    request: &stasis_runner::swap::contracts::SwapCommitRequest,
    config: &RunnerConfig,
    has_forced_hook_failure: bool,
    pending_aot_metadata: &BTreeMap<RequestId, PendingAotCompileMetadata>,
    pending_jit_code_ptr_overrides: &BTreeMap<RequestId, Vec<JitCodePtrOverride>>,
) -> bool {
    if config.disable_on_code_swap_hook || has_forced_hook_failure || request.hook_symbol.is_none()
    {
        return false;
    }
    match config.target_mode {
        TargetMode::JitDev => pending_jit_code_ptr_overrides.contains_key(&request.request_id),
        TargetMode::AotProd => {
            pending_aot_metadata.contains_key(&request.request_id)
                && std::env::var("STASIS_AOT_EXECUTE_NATIVE_HOOK")
                    .ok()
                    .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
        }
    }
}

fn apply_prepared_jit_transaction(
    request_id: RequestId,
    candidate: Option<JitProcess>,
    active: &mut Option<JitProcess>,
    hook_may_mutate_state: bool,
    apply: impl FnOnce() -> SwapCommitResult,
) -> Result<SwapCommitResult, String> {
    let candidate = candidate.ok_or_else(|| {
        format!(
            "JIT commit {} is missing its staged runtime candidate",
            request_id.0
        )
    })?;
    let incoming_layout = candidate.state_layout();
    let active_layout = active
        .as_ref()
        .map(JitProcess::state_layout)
        .unwrap_or_else(|| incoming_layout.clone());
    let mut preview =
        plan_state_migration(&active_layout, &incoming_layout, Vec::new(), false, None)?;
    finalize_runtime_preview(&candidate, &mut preview);
    let result = activate_candidate_transactionally(
        active.as_ref(),
        &candidate,
        &preview,
        hook_may_mutate_state,
        apply,
        |result| result.status == SwapCommitStatus::Success,
    )?;
    if result.status == SwapCommitStatus::Success {
        *active = Some(candidate);
    }
    Ok(result)
}

fn apply_runtime_state_transaction(
    active_layout_hash: Option<LayoutHash>,
    request_layout_hash: LayoutHash,
    hook_may_mutate_state: bool,
    apply: impl FnOnce() -> SwapCommitResult,
) -> Result<SwapCommitResult, String> {
    let layout_changed = active_layout_hash.is_some_and(|hash| hash != request_layout_hash);
    if layout_changed {
        return Err(format!(
            "layout-changing commit from {} to {} requires a staged JIT candidate; restart required for this backend",
            format_layout_hash_hex(active_layout_hash.expect("layout change has active hash")),
            format_layout_hash_hex(request_layout_hash)
        ));
    }
    if !hook_may_mutate_state {
        return Ok(apply());
    }
    let snapshot = stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES)?;
    let result = apply();
    if result.status != SwapCommitStatus::Success {
        stasis_dynload::restore_jit_runtime_state(&snapshot);
    }
    Ok(result)
}

#[allow(clippy::too_many_arguments)]
fn apply_commit_request(
    request: stasis_runner::swap::contracts::SwapCommitRequest,
    pointer_table: &mut FunctionPointerTable,
    config: &RunnerConfig,
    expected_host_set: &host_set_registry::HostSetContract,
    hook_runs: &mut u32,
    hook_failures: &mut u32,
    hook_failure_reasons: &mut Vec<String>,
    swap_commit_successes: &mut u32,
    swap_commit_failures: &mut u32,
    swap_failure_reasons: &mut Vec<String>,
    events: &mut Vec<RunnerEvent>,
    hook_failure_reason: Option<&String>,
    swap_failure_reason: Option<&String>,
    pending_aot_metadata: &BTreeMap<RequestId, PendingAotCompileMetadata>,
    pending_jit_code_ptr_overrides: &BTreeMap<RequestId, Vec<JitCodePtrOverride>>,
) -> SwapCommitResult {
    if request.host_set_id.as_deref() != Some(expected_host_set.host_set_id.as_str())
        || request.host_set_hash != Some(expected_host_set.host_set_hash)
    {
        let message = format!(
            "host-set contract mismatch: expected id='{}' hash={:02x?}",
            expected_host_set.host_set_id, expected_host_set.host_set_hash
        );
        *swap_commit_failures += 1;
        swap_failure_reasons.push(message.clone());
        return SwapCommitResult::failed(request.request_id, message);
    }

    if config.target_mode == TargetMode::JitDev && !config.disable_on_code_swap_hook {
        if let Some(hook_symbol) = request.hook_symbol.as_deref() {
            *hook_runs += 1;
            if let Some(reason) = hook_failure_reason {
                *hook_failures += 1;
                hook_failure_reasons.push(reason.clone());
                let hook_error = format!("{hook_symbol} failed: {reason}");
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            }

            if pending_jit_code_ptr_overrides.contains_key(&request.request_id) {
                let Some(hook_fn_id) = request.hook_fn_id else {
                    *hook_failures += 1;
                    let hook_error = format!("{hook_symbol} failed: missing hook_fn_id metadata");
                    hook_failure_reasons.push(hook_error.clone());
                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "failed".to_string(),
                        error: Some(hook_error.clone()),
                    });
                    *swap_commit_failures += 1;
                    swap_failure_reasons.push(hook_error.clone());
                    return SwapCommitResult::failed(request.request_id, hook_error);
                };

                let overrides = pending_jit_code_ptr_overrides
                    .get(&request.request_id)
                    .expect("request id should exist after contains_key");
                let code_ptr = overrides
                    .iter()
                    .find(|entry| entry.fn_id == hook_fn_id)
                    .map(|entry| entry.code_ptr)
                    .unwrap_or(0);
                if code_ptr == 0 {
                    *hook_failures += 1;
                    let hook_error = format!(
                        "{hook_symbol} failed: missing JIT hook code pointer override for fn_id={}",
                        hook_fn_id.0
                    );
                    hook_failure_reasons.push(hook_error.clone());
                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "failed".to_string(),
                        error: Some(hook_error.clone()),
                    });
                    *swap_commit_failures += 1;
                    swap_failure_reasons.push(hook_error.clone());
                    return SwapCommitResult::failed(request.request_id, hook_error);
                }
                if let Err(error) = stasis_dynload::invoke_code_swap_hook(code_ptr as usize) {
                    *hook_failures += 1;
                    let hook_error = format!("{hook_symbol} failed: {error}");
                    hook_failure_reasons.push(hook_error.clone());
                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "failed".to_string(),
                        error: Some(hook_error.clone()),
                    });
                    *swap_commit_failures += 1;
                    swap_failure_reasons.push(hook_error.clone());
                    return SwapCommitResult::failed(request.request_id, hook_error);
                }
            }

            events.push(RunnerEvent::HookResult {
                request_id: request.request_id.0,
                symbol: hook_symbol.to_string(),
                status: "success".to_string(),
                error: None,
            });
        }
    }

    if config.target_mode == TargetMode::AotProd
        && !config.disable_on_code_swap_hook
        && std::env::var("STASIS_AOT_EXECUTE_NATIVE_HOOK")
            .ok()
            .is_some_and(|value| value == "1" || value.eq_ignore_ascii_case("true"))
    {
        if let Some(hook_symbol) = request.hook_symbol.as_deref() {
            *hook_runs += 1;
            if let Some(reason) = hook_failure_reason {
                *hook_failures += 1;
                hook_failure_reasons.push(reason.clone());
                let hook_error = format!("{hook_symbol} failed: {reason}");
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            }

            let Some(hook_fn_id) = request.hook_fn_id else {
                *hook_failures += 1;
                let hook_error = format!("{hook_symbol} failed: missing hook_fn_id metadata");
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            };

            let Some(metadata) = pending_aot_metadata.get(&request.request_id) else {
                *hook_failures += 1;
                let hook_error =
                    format!("{hook_symbol} failed: missing AOT compile metadata for request");
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            };

            let Some(linked_image) = metadata.linked_image_path.as_ref() else {
                *hook_failures += 1;
                let hook_error = format!("{hook_symbol} failed: missing linked AOT image path");
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            };

            let Some(symbols) = metadata.function_symbols.as_ref() else {
                *hook_failures += 1;
                let hook_error =
                    format!("{hook_symbol} failed: missing AOT function symbol mapping metadata");
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            };

            let export = symbols
                .iter()
                .find(|entry| entry.fn_id == hook_fn_id)
                .map(|entry| entry.symbol.as_str());
            let Some(export) = export else {
                *hook_failures += 1;
                let hook_error = format!(
                    "{hook_symbol} failed: missing AOT hook symbol for fn_id={}",
                    hook_fn_id.0
                );
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            };

            let lib = match stasis_dynload::Library::load(linked_image) {
                Ok(lib) => lib,
                Err(error) => {
                    *hook_failures += 1;
                    let hook_error = format!("{hook_symbol} failed: {error}");
                    hook_failure_reasons.push(hook_error.clone());
                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "failed".to_string(),
                        error: Some(hook_error.clone()),
                    });
                    *swap_commit_failures += 1;
                    swap_failure_reasons.push(hook_error.clone());
                    return SwapCommitResult::failed(request.request_id, hook_error);
                }
            };

            let address = match lib.symbol_address(export) {
                Ok(address) => address,
                Err(error) => {
                    *hook_failures += 1;
                    let hook_error = format!("{hook_symbol} failed: {error}");
                    hook_failure_reasons.push(hook_error.clone());
                    events.push(RunnerEvent::HookResult {
                        request_id: request.request_id.0,
                        symbol: hook_symbol.to_string(),
                        status: "failed".to_string(),
                        error: Some(hook_error.clone()),
                    });
                    *swap_commit_failures += 1;
                    swap_failure_reasons.push(hook_error.clone());
                    return SwapCommitResult::failed(request.request_id, hook_error);
                }
            };

            if let Err(error) = stasis_dynload::invoke_code_swap_hook(address) {
                *hook_failures += 1;
                let hook_error = format!("{hook_symbol} failed: {error}");
                hook_failure_reasons.push(hook_error.clone());
                events.push(RunnerEvent::HookResult {
                    request_id: request.request_id.0,
                    symbol: hook_symbol.to_string(),
                    status: "failed".to_string(),
                    error: Some(hook_error.clone()),
                });
                *swap_commit_failures += 1;
                swap_failure_reasons.push(hook_error.clone());
                return SwapCommitResult::failed(request.request_id, hook_error);
            }

            events.push(RunnerEvent::HookResult {
                request_id: request.request_id.0,
                symbol: hook_symbol.to_string(),
                status: "success".to_string(),
                error: None,
            });
        }
    }

    if let Some(reason) = swap_failure_reason {
        *swap_commit_failures += 1;
        swap_failure_reasons.push(reason.clone());
        return SwapCommitResult::failed(request.request_id, reason.clone());
    }

    if config.target_mode == TargetMode::AotProd && config.aot_probe_loadability {
        let Some(metadata) = pending_aot_metadata.get(&request.request_id) else {
            let message = format!(
                "AOT loadability probe failed for request {}: missing compile metadata",
                request.request_id.0
            );
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        };
        let Some(path) = metadata.linked_image_path.as_ref() else {
            let message = format!(
                "AOT loadability probe failed for request {}: missing linked image path",
                request.request_id.0
            );
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        };
        if let Err(message) = probe_aot_loadability(path) {
            *swap_commit_failures += 1;
            swap_failure_reasons.push(message.clone());
            return SwapCommitResult::failed(request.request_id, message);
        }
    }

    *swap_commit_successes += 1;
    let outcome = if config.target_mode == TargetMode::JitDev {
        if let Some(overrides) = pending_jit_code_ptr_overrides.get(&request.request_id) {
            pointer_table.commit_patch_set_with_overrides(&request.fn_patch_set, overrides)
        } else {
            pointer_table.commit_patch_set(&request.fn_patch_set)
        }
    } else {
        pointer_table.commit_patch_set(&request.fn_patch_set)
    };
    SwapCommitResult::success(
        request.request_id,
        outcome.swapped_fn_ids,
        outcome.new_generation,
    )
}

fn observe_pipeline_results(
    pipeline: &DevHotSwapPipeline,
    last_seen_compile_id: &mut Option<RequestId>,
    last_seen_commit_id: &mut Option<RequestId>,
    compile_successes: &mut u32,
    compile_failures: &mut u32,
    compile_diagnostics: &mut Vec<String>,
    last_compile_duration_ms: &mut Option<u64>,
    last_commit_duration_ms: &mut Option<u64>,
    last_swap_status: &mut Option<SwapCommitStatus>,
    events: &mut Vec<RunnerEvent>,
) -> Option<(RequestId, SwapCommitStatus)> {
    let mut new_commit: Option<(RequestId, SwapCommitStatus)> = None;
    if let Some(result) = pipeline.last_compile_result() {
        if *last_seen_compile_id != Some(result.request_id) {
            *last_seen_compile_id = Some(result.request_id);
            *last_compile_duration_ms = pipeline.last_compile_duration().map(duration_ms);
            match result.status {
                CompileStatus::Success => {
                    *compile_successes += 1;
                    events.push(RunnerEvent::CompileResult {
                        request_id: result.request_id.0,
                        status: "success".to_string(),
                        diagnostics: Vec::new(),
                        compile_duration_ms: *last_compile_duration_ms,
                    });
                }
                CompileStatus::Failed => {
                    *compile_failures += 1;
                    let mut event_diagnostics = Vec::new();
                    if result.diagnostics.is_empty() {
                        let message = "compile failed with no diagnostics".to_string();
                        compile_diagnostics.push(message.clone());
                        event_diagnostics.push(message);
                    } else {
                        for diagnostic in &result.diagnostics {
                            let formatted = format_diagnostic(diagnostic);
                            compile_diagnostics.push(formatted.clone());
                            event_diagnostics.push(formatted);
                        }
                    }
                    events.push(RunnerEvent::CompileResult {
                        request_id: result.request_id.0,
                        status: "failed".to_string(),
                        diagnostics: event_diagnostics,
                        compile_duration_ms: *last_compile_duration_ms,
                    });
                }
            }
        }
    }

    if let Some(result) = pipeline.last_commit_result() {
        *last_swap_status = Some(result.status.clone());
        if *last_seen_commit_id != Some(result.request_id) {
            *last_seen_commit_id = Some(result.request_id);
            *last_commit_duration_ms = pipeline.last_commit_duration().map(duration_ms);
            new_commit = Some((result.request_id, result.status.clone()));
            let status = match result.status {
                SwapCommitStatus::Success => "success",
                SwapCommitStatus::Failed => "failed",
            };
            let swapped_fn_ids = result.swapped_fn_ids.iter().map(|id| id.0).collect();
            let new_generation = result.new_generation.map(|generation| generation.0);
            events.push(RunnerEvent::SwapCommitResult {
                request_id: result.request_id.0,
                status: status.to_string(),
                swapped_fn_ids,
                new_generation,
                error: result.error.clone(),
                commit_duration_ms: *last_commit_duration_ms,
            });
        }
    }
    new_commit
}

fn duration_ms(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    u64::try_from(millis).unwrap_or(u64::MAX)
}

fn probe_aot_loadability(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!(
            "AOT loadability probe failed: linked image does not exist at {}",
            path.display()
        ));
    }
    stasis_dynload::Library::load(path)
        .map(|_| ())
        .map_err(|error| {
            format!(
                "AOT loadability probe failed for {}: {error}",
                path.display()
            )
        })
}

fn sleep_for_tick(tick_sleep_micros: u64) {
    let micros = if tick_sleep_micros > 0 {
        tick_sleep_micros
    } else {
        // Tiny default pause improves cross-thread determinism for test/runtime loops.
        50
    };
    thread::sleep(Duration::from_micros(micros));
}

fn format_diagnostic(diagnostic: &Diagnostic) -> String {
    let severity = match diagnostic.severity {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
        DiagnosticSeverity::Note => "note",
    };

    let path_part = diagnostic
        .path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "<unknown>".to_string());

    let line = diagnostic.line.unwrap_or(0);
    let column = diagnostic.column.unwrap_or(0);
    format!(
        "{severity}:{path_part}:{line}:{column}: {}",
        diagnostic.message
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn tick_budget_diagnostics_report_bounded_average_p99_and_overruns() {
        let mut diagnostics = TickBudgetDiagnostics::new(100, 7);
        for sample in [25, 50, 75, 100, 125] {
            diagnostics.record(sample);
        }
        assert_eq!(diagnostics.average_us(), 75);
        assert_eq!(diagnostics.p99_us(), 125);
        assert_eq!(diagnostics.overruns, 1);
        assert_eq!(
            diagnostics.report(),
            "[tick-budget] generation=7 budget_us=100 samples=5 average_us=75 p99_us=125 overruns=1"
        );

        for sample in 0..(TICK_BUDGET_SAMPLE_LIMIT + 10) {
            diagnostics.record(sample as u64);
        }
        assert_eq!(diagnostics.recent_us.len(), TICK_BUDGET_SAMPLE_LIMIT);
    }

    #[test]
    fn tick_budget_generations_advance_only_after_committed_swaps() {
        let mut generation = 0;
        let mut current = Some(TickBudgetDiagnostics::new(100, generation));
        current.as_mut().expect("generation zero").record(75);

        assert_eq!(
            update_tick_budget_after_swap(&mut current, &mut generation, Some(80), false),
            None
        );
        assert_eq!(generation, 0);
        assert_eq!(
            current.as_ref().expect("preserved generation").sample_count,
            1
        );

        let completed =
            update_tick_budget_after_swap(&mut current, &mut generation, Some(80), true)
                .expect("generation zero report");
        assert!(completed.contains("generation=0 budget_us=100 samples=1"));
        let next = current.as_ref().expect("generation one");
        assert_eq!(generation, 1);
        assert_eq!(next.generation, 1);
        assert_eq!(next.sample_count, 0);
    }

    const STASIS_GRAPHICS_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_graphics.c"
    ));
    const STASIS_RUNNER_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_runner.c"
    ));
    const STASIS_MOBILE_RUNTIME_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_mobile_runtime.c"
    ));
    const STASIS_MOBILE_RUNTIME_HEADER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_mobile_runtime.h"
    ));
    const STASIS_MOBILE_MAIN_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../mobile/shells/common/stasis_mobile_main.c"
    ));
    const STASIS_MOBILE_AOT_RUNTIME_SOURCE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_mobile_aot_runtime.c"
    ));
    const STASIS_MOBILE_AOT_RUNTIME_HEADER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_mobile_aot_runtime.h"
    ));
    const STASIS_RUNTIME_CMAKE: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/CMakeLists.txt"
    ));
    const STASIS_RENDER_CONTRACT_HEADER: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../runtime/stasis_render_contract.h"
    ));
    const BETWEEN_TICK_LAYOUT_V1: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/between_tick_layout_migration/v1.stasis"
    ));
    const BETWEEN_TICK_LAYOUT_V2: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/between_tick_layout_migration/v2.stasis"
    ));
    const BETWEEN_TICK_LAYOUT_REJECT: &str = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../samples/between_tick_layout_migration/reject.stasis"
    ));

    fn jit_global_table_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    static REJECTING_HOOK_PATH_HASH: std::sync::atomic::AtomicI32 =
        std::sync::atomic::AtomicI32::new(0);

    extern "C" fn mutating_rejecting_hook() {
        let path_hash = REJECTING_HOOK_PATH_HASH.load(std::sync::atomic::Ordering::Relaxed);
        stasis_dynload::stasis_jit_global_i32_store(path_hash, 99);
        stasis_dynload::stasis_jit_reject_code_swap();
    }

    fn process_env_lock() -> &'static std::sync::Mutex<()> {
        static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
        LOCK.get_or_init(|| std::sync::Mutex::new(()))
    }

    #[test]
    fn windows_performance_hud_uses_frame_work_and_60_fps_budget() {
        for required in [
            "event.key.keysym.sym == SDLK_F3",
            "stasis_host_set_performance_metrics",
            "g_perf_pending_guest_render_us",
            "stasis_perf_elapsed_us(g_perf_render_started_counter, now)",
            "budget@60fps=%d%%",
            "const double total_ms = tick_ms + render_ms",
        ] {
            assert!(
                STASIS_GRAPHICS_SOURCE.contains(required),
                "Windows performance HUD contract should contain {required}"
            );
        }
        assert!(
            STASIS_RUNNER_SOURCE.contains("host_set_performance_metrics(tick_us, render_us)"),
            "Windows AOT runner should feed tick and render timing into the shared HUD"
        );
    }

    fn with_env_var_set(key: &str, value: &str, f: impl FnOnce()) {
        let _lock = process_env_lock()
            .lock()
            .expect("process env lock should succeed");
        let old = std::env::var_os(key);
        std::env::set_var(key, value);
        f();
        if let Some(old) = old {
            std::env::set_var(key, old);
        } else {
            std::env::remove_var(key);
        }
    }

    fn input_pointer(
        id: i32,
        is_down: bool,
        went_down: bool,
        went_up: bool,
        x: f32,
        y: f32,
    ) -> PlayInputPointer {
        PlayInputPointer {
            id,
            is_down,
            went_down,
            went_up,
            x,
            y,
        }
    }

    #[test]
    fn initial_host_request_baseline_precedes_guest_main_and_apply() {
        use std::cell::{Cell, RefCell};

        let sequence = Cell::new(0);
        let baseline = Cell::new(-1);
        let phases = RefCell::new(Vec::new());
        let result = run_guest_main_with_initial_host_requests(
            || {
                phases.borrow_mut().push("init");
                baseline.set(sequence.get());
                Ok(())
            },
            || {
                phases.borrow_mut().push("main");
                sequence.set(1);
                Ok(0)
            },
            || {
                phases.borrow_mut().push("apply");
                assert_eq!(baseline.get(), 0);
                assert_eq!(sequence.get(), 1);
                Ok(())
            },
        )
        .expect("startup should succeed");

        assert_eq!(result, 0);
        assert_eq!(&*phases.borrow(), &["init", "main", "apply"]);
    }

    #[test]
    fn live_main_signature_error_names_the_required_signature() {
        let error =
            format_live_main_error("function 'main' is not i32-returning (type id 0)".to_string());
        assert!(error.contains("expected `function main(): i32`"));
        assert!(error.contains("not i32-returning"));
    }

    #[test]
    fn input_script_validates_order_pointer_bounds_and_transitions() {
        let out_of_order = PlayInputScriptDocument {
            version: 1,
            frames: vec![
                PlayInputFrame {
                    frame: 2,
                    pointers: vec![],
                },
                PlayInputFrame {
                    frame: 1,
                    pointers: vec![],
                },
            ],
        };
        assert!(validate_play_input_script(out_of_order)
            .expect_err("order should fail")
            .contains("strictly increasing"));

        let invalid_transition = PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 1,
                pointers: vec![input_pointer(0, false, true, false, 1.0, 1.0)],
            }],
        };
        assert!(validate_play_input_script(invalid_transition)
            .expect_err("transition should fail")
            .contains("wentDown requires isDown=true"));
    }

    #[test]
    fn input_script_parses_documented_camel_case_json() {
        let source = r#"{
            "version": 1,
            "frames": [
                {"frame": 1, "pointers": [
                    {"id": 3, "isDown": true, "wentDown": true, "wentUp": false, "x": 90, "y": 60}
                ]},
                {"frame": 2, "pointers": [
                    {"id": 3, "isDown": false, "wentDown": false, "wentUp": true, "x": 90, "y": 60}
                ]}
            ]
        }"#;
        let document: PlayInputScriptDocument =
            serde_json::from_str(source).expect("documented JSON should parse");
        let timeline = validate_play_input_script(document).expect("document should validate");
        assert_eq!(timeline.frames.len(), 2);
        assert!(timeline.frames[0].pointers[0].is_down);
        assert!(timeline.frames[1].pointers[0].went_up);

        let unknown = r#"{"version": 1, "frames": [], "extra": true}"#;
        assert!(serde_json::from_str::<PlayInputScriptDocument>(unknown).is_err());
    }

    #[test]
    fn input_script_rejects_resource_and_pointer_bounds() {
        assert!(validate_play_input_script_size(PLAY_INPUT_MAX_FILE_BYTES).is_ok());
        assert!(
            validate_play_input_script_size(PLAY_INPUT_MAX_FILE_BYTES + 1)
                .expect_err("oversized script should fail")
                .contains("too large")
        );

        let too_many_frames = PlayInputScriptDocument {
            version: 1,
            frames: vec![
                PlayInputFrame {
                    frame: 1,
                    pointers: vec![],
                };
                PLAY_INPUT_MAX_FRAMES + 1
            ],
        };
        assert!(validate_play_input_script(too_many_frames)
            .expect_err("frame bound should fail")
            .contains("too many frames"));

        let too_many_pointers = PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 1,
                pointers: (0..=PLAY_INPUT_MAX_POINTERS)
                    .map(|id| input_pointer(id as i32, false, false, false, 1.0, 1.0))
                    .collect(),
            }],
        };
        assert!(validate_play_input_script(too_many_pointers)
            .expect_err("pointer bound should fail")
            .contains("too many pointers"));
    }

    #[test]
    fn input_script_rejects_duplicate_ids_bad_coordinates_and_short_tick_limits() {
        let unsupported = PlayInputScriptDocument {
            version: 2,
            frames: vec![],
        };
        assert!(validate_play_input_script(unsupported)
            .expect_err("version should fail")
            .contains("unsupported input-script version"));

        for (name, pointers, expected) in [
            (
                "duplicate",
                vec![
                    input_pointer(2, false, false, false, 1.0, 1.0),
                    input_pointer(2, false, false, false, 2.0, 2.0),
                ],
                "duplicate pointer id",
            ),
            (
                "negative id",
                vec![input_pointer(-1, false, false, false, 1.0, 1.0)],
                "id must be non-negative",
            ),
            (
                "negative",
                vec![input_pointer(2, false, false, false, -1.0, 1.0)],
                "finite and non-negative",
            ),
            (
                "nonfinite",
                vec![input_pointer(2, false, false, false, f32::NAN, 1.0)],
                "finite and non-negative",
            ),
        ] {
            let document = PlayInputScriptDocument {
                version: 1,
                frames: vec![PlayInputFrame { frame: 3, pointers }],
            };
            let error = validate_play_input_script(document).expect_err(name);
            assert!(error.contains(expected), "{name}: {error}");
        }

        let timeline = validate_play_input_script(PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 3,
                pointers: vec![],
            }],
        })
        .expect("timeline should validate");
        assert!(validate_play_input_script_ticks(Some(2), &timeline)
            .expect_err("short max_ticks should fail")
            .contains("max_ticks"));
    }

    #[test]
    fn input_script_rejects_pointer_outside_current_viewport() {
        let document = PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 1,
                pointers: vec![input_pointer(0, false, false, false, 181.0, 60.0)],
            }],
        };
        let mut timeline = validate_play_input_script(document).expect("valid static bounds");
        let mut host_i32 = vec![0; 768];
        let mut host_f32 = vec![0.0; 64];
        host_f32[HOST_F_LOGICAL_W] = 180.0;
        host_f32[HOST_F_LOGICAL_H] = 120.0;
        assert!(
            apply_play_input_frame(&mut timeline, 1, &mut host_i32, &mut host_f32)
                .expect_err("viewport bound should fail")
                .contains("outside the 180x120 viewport")
        );
    }

    #[test]
    fn input_script_uses_logical_dimensions_from_first_snapshot() {
        let document = PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 1,
                pointers: vec![input_pointer(0, true, true, false, 180.0, 360.0)],
            }],
        };
        let mut timeline = validate_play_input_script(document).expect("valid script");
        let mut host_i32 = vec![0; 768];
        let mut host_f32 = vec![0.0; 64];
        host_f32[HOST_F_LOGICAL_W] = 360.0;
        host_f32[HOST_F_LOGICAL_H] = 720.0;

        apply_play_input_frame(&mut timeline, 1, &mut host_i32, &mut host_f32)
            .expect("logical dimensions should accept first-frame input");
        assert_eq!(host_f32[4], 0.5);
        assert_eq!(host_f32[5], 0.5);
    }

    #[test]
    fn input_script_overrides_host_pointer_and_clears_edge_flags() {
        let document = PlayInputScriptDocument {
            version: 1,
            frames: vec![PlayInputFrame {
                frame: 1,
                pointers: vec![input_pointer(7, true, true, false, 90.0, 60.0)],
            }],
        };
        let mut timeline = validate_play_input_script(document).expect("valid script");
        let mut host_i32 = vec![99; 768];
        let mut host_f32 = vec![99.0; 64];
        host_f32[HOST_F_LOGICAL_W] = 180.0;
        host_f32[HOST_F_LOGICAL_H] = 120.0;

        apply_play_input_frame(&mut timeline, 1, &mut host_i32, &mut host_f32).expect("frame one");
        assert_eq!(host_i32[HOST_I_POINTER_COUNT], 1);
        assert_eq!(host_i32[HOST_I_POINTER_BASE], 7);
        assert_eq!(host_i32[HOST_I_POINTER_BASE + 1], 1);
        assert_eq!(host_i32[HOST_I_POINTER_BASE + 2], 1);
        assert_eq!(host_f32[4], 0.5);
        assert_eq!(host_f32[5], 0.5);

        apply_play_input_frame(&mut timeline, 2, &mut host_i32, &mut host_f32)
            .expect("unscripted frame");
        assert_eq!(host_i32[HOST_I_POINTER_BASE + 1], 1);
        assert_eq!(host_i32[HOST_I_POINTER_BASE + 2], 0);
        assert_eq!(host_i32[HOST_I_POINTER_BASE + 3], 0);
        assert_eq!(host_f32[2], 0.0);
        assert_eq!(host_f32[3], 0.0);
    }

    #[test]
    fn sprite_runtime_clamps_initial_sprite_growth_to_configured_limit() {
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("if (min_capacity > limit)")
                && STASIS_GRAPHICS_SOURCE.contains(
                    "clamp_i32(SPRITE_TABLE_INITIAL_CAPACITY, 1, limit)"
                ),
            "runtime sprite allocation should clamp the initial growth step to STASIS_GFX_MAX_SPRITES"
        );
    }

    #[test]
    fn graphics_runtime_presents_asset_free_loading_on_sdl_and_opengl() {
        let graphics_source = STASIS_GRAPHICS_SOURCE.replace("\r\n", "\n");
        for required in [
            "static void stasis_present_gpu_loading(void)",
            "SDL_RenderPresent(g_renderer);",
            "SDL_GL_SwapWindow(g_window);",
            "g_use_sdl_renderer ? \"sdl\" : \"gl\"",
        ] {
            assert!(
                graphics_source.contains(required),
                "shared graphics loading screen should contain {required}"
            );
        }
        assert_eq!(
            graphics_source
                .matches("stasis_present_gpu_loading();")
                .count(),
            3,
            "loading should be invoked at startup and both real SDL reset events"
        );
        assert!(
            graphics_source.contains("SDL_PumpEvents();\n    stasis_present_gpu_loading();"),
            "every desktop backend must pump initial window messages before presenting loading"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("stasis_graphics_runtime_abi_version(void)"),
            "the graphics DLL must expose an explicit compatibility boundary"
        );
        assert!(
            STASIS_RUNNER_SOURCE.contains("stasis_set_asset_root(exe_dir)")
                && STASIS_RUNNER_SOURCE.contains("set_graphics_asset_root(launcher_asset_root)")
                && STASIS_RUNNER_SOURCE.contains("stasis_set_packaged_graphics_path(exe_dir)")
                && STASIS_RUNNER_SOURCE.contains("incompatible stasis_graphics.dll"),
            "generated launchers must anchor packaged assets and reject mismatched graphics DLLs"
        );
        assert!(
            STASIS_RUNNER_SOURCE.contains("strncmp(out, \"\\\\\\\\?\\\\UNC\\\\\", 8)")
                && STASIS_RUNNER_SOURCE.contains("strncmp(out, \"\\\\\\\\?\\\\\", 4)"),
            "Windows launchers must normalize extended drive and UNC paths before asset loading"
        );
    }

    #[test]
    fn mobile_runtime_uses_fixed_entries_and_sdl_only_static_target() {
        for required in [
            "typedef void (*StasisMobileBindEntry)(void)",
            "typedef int32_t (*StasisMobileI32Entry)(void)",
            "StasisMobileBindEntry bind_runtime_entry",
            "StasisMobileI32Entry main_entry",
            "StasisMobileI32Entry tick_entry",
            "StasisMobileI32Entry render_entry",
            "stasis_mobile_runtime_last_entry_result(void)",
            "STASIS_MOBILE_RUNTIME_ABI_VERSION 1",
        ] {
            assert!(
                STASIS_MOBILE_RUNTIME_HEADER.contains(required),
                "mobile runtime ABI should contain {required}"
            );
        }
        for required in [
            "runtime_state.entries.bind_runtime_entry()",
            "runtime_state.entries.main_entry()",
            "runtime_state.entries.tick_entry()",
            "runtime_state.entries.render_entry()",
            "runtime_state.last_entry_result != 0",
            "stasis_should_quit()",
            "stasis_mobile_poll_events()",
            "stasis_mobile_set_paused(runtime_state.paused)",
            "void stasis_mobile_runtime_shutdown(void)",
        ] {
            assert!(
                STASIS_MOBILE_RUNTIME_SOURCE.contains(required),
                "mobile lifecycle should contain {required}"
            );
        }
        assert!(
            STASIS_RUNTIME_CMAKE
                .contains("configure_stasis_target(stasis_mobile_runtime ON TRUE OFF)"),
            "mobile target should be static, SDL-only, and exclude the SDL desktop main shim"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("stasis_storage_load_i32")
                && STASIS_GRAPHICS_SOURCE.contains("stasis_storage_save_i32")
                && !STASIS_RUNTIME_CMAKE
                    .contains("stasis_mobile_aot_runtime.c\n        stasis_platform_storage.c")
                && !STASIS_MOBILE_MAIN_SOURCE.contains("stasis_storage_set_root"),
            "graphical mobile packages must use the single SDL preference host"
        );
        assert!(
            !STASIS_MOBILE_RUNTIME_SOURCE.contains("stasis_dynload")
                && !STASIS_MOBILE_RUNTIME_SOURCE.contains("on_code_swap")
                && !STASIS_MOBILE_RUNTIME_SOURCE.contains("stasis_runner"),
            "mobile runtime must not acquire desktop loader or hot-swap dependencies"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("SDL_DestroyRenderer(g_renderer);")
                && STASIS_GRAPHICS_SOURCE.contains("g_renderer = NULL;")
                && STASIS_GRAPHICS_SOURCE.contains("g_window = NULL;"),
            "mobile lifecycle cleanup should support safe shutdown and initialization retry"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("STASIS_EXPORT int stasis_mobile_poll_events(void)")
                && STASIS_GRAPHICS_SOURCE
                    .contains("SDL_PauseAudioDevice(g_audio_device, paused ? 1 : 0)"),
            "mobile pause should continue polling events and pause the audio device"
        );
        for required in [
            "static void stasis_present_gpu_loading(void)",
            "Stasis renderer loading screen presented",
            "stasis_renderer_lifecycle_resume(&g_resource_lifecycle);",
        ] {
            assert!(
                STASIS_GRAPHICS_SOURCE.contains(required),
                "mobile renderer lifecycle should contain {required}"
            );
        }
        assert!(
            !STASIS_GRAPHICS_SOURCE.contains(
                "stasis_renderer_lifecycle_resume(&g_resource_lifecycle);\n                    stasis_invalidate_renderer_resources(0);"
            ),
            "ordinary foreground resume must not invalidate a surviving SDL renderer"
        );
    }

    #[test]
    fn mobile_aot_runtime_supports_u16_storage() {
        assert!(STASIS_MOBILE_AOT_RUNTIME_HEADER.contains("stasis_jit_register_global_u16_array"));
        for required in [
            "STASIS_VALUE_U16",
            "sizeof(uint16_t)",
            "((uint16_t *)entry->data)[i]",
        ] {
            assert!(
                STASIS_MOBILE_AOT_RUNTIME_SOURCE.contains(required),
                "mobile AOT runtime should contain {required}"
            );
        }
    }

    #[test]
    fn shipping_renderers_share_one_versioned_sdl_command_process() {
        let runtime_cmake = STASIS_RUNTIME_CMAKE.replace("\r\n", "\n");
        for required in [
            "STASIS_RENDER_V2_MAGIC 0x47584631",
            "STASIS_RENDER_V2_VERSION 2",
            "STASIS_RENDER_V2_TRACE_VERSION 2",
            "STASIS_RENDER_SPRITE_F32_STRIDE 4",
            "STASIS_RENDER_I32_COUNT",
            "STASIS_RENDER_F32_COUNT",
            "stasis_render_v2_validate",
            "stasis_render_v2_validation_name",
            "stasis_render_v2_is_valid",
            "stasis_render_v2_trace",
        ] {
            assert!(
                STASIS_RENDER_CONTRACT_HEADER.contains(required),
                "shared renderer contract should contain {required}"
            );
        }
        assert!(
            runtime_cmake.contains(
                "Build the canonical SDL_Renderer runtime (disable only for legacy GL conformance)"
            ) && runtime_cmake.contains("    ON\n)"),
            "shipping runtime should default to the canonical SDL backend"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("stasis_render_v2_validate(cmd_i32, cmd_f32)")
                && STASIS_GRAPHICS_SOURCE.contains("STASIS_RENDER_FLAG_PRESENT")
                && STASIS_GRAPHICS_SOURCE
                    .contains("SDL_SetHint(SDL_HINT_RENDER_SCALE_QUALITY, \"linear\")")
                && STASIS_GRAPHICS_SOURCE.contains("SDL_RenderSetClipRect(g_renderer, NULL)")
                && STASIS_GRAPHICS_SOURCE.contains("if (a < 0) a = 0;")
                && STASIS_GRAPHICS_SOURCE.contains("if (a > 255) a = 255;")
                && STASIS_GRAPHICS_SOURCE.contains("SDL_BLENDMODE_BLEND")
                && STASIS_GRAPHICS_SOURCE.contains("SDL_RenderCopyEx")
                && STASIS_GRAPHICS_SOURCE.contains("STASIS_PARITY_CAPTURE_STAGE")
                && STASIS_GRAPHICS_SOURCE.contains("Stasis parity capture: stage=%s"),
            "desktop and mobile should enter the shared versioned command interpreter"
        );
        assert!(
            STASIS_MOBILE_RUNTIME_SOURCE.contains("STASIS_RENDER_I32_COUNT")
                && STASIS_MOBILE_RUNTIME_SOURCE.contains("stasis_gfx_submit_u8("),
            "mobile AOT should bind and submit the same command representation"
        );
    }

    #[test]
    fn sprite_runtime_clears_reused_atlas_padding_before_mipmap_regeneration() {
        assert!(
            STASIS_GRAPHICS_SOURCE.contains(
                "unsigned char* clear_pixels = (unsigned char*)calloc(pixel_count, 4);"
            ),
            "runtime sprite upload should zero the reused atlas allocation before writing sprite pixels"
        );
        assert!(
            STASIS_GRAPHICS_SOURCE.contains(
                "atlas_page_clear_region(page, alloc_x, alloc_y, alloc_w, alloc_h);"
            ) && STASIS_GRAPHICS_SOURCE.contains(
                "atlas_page_upload_region(page, sprite_x, sprite_y, w, h, pixels)"
            ),
            "runtime sprite upload should clear padded texels before updating the sprite interior and regenerating mipmaps"
        );
    }

    #[test]
    fn sprite_runtime_bounds_decode_inputs_before_allocating_pixels() {
        for required in [
            "STASIS_GFX_MAX_SPRITE_DIMENSION",
            "STASIS_GFX_MAX_SPRITE_PIXELS",
            "STASIS_GFX_MAX_SPRITE_FILE_BYTES",
            "sprite_source_within_limits(path, raster_w, raster_h)",
            "sprite_dimensions_exceeded",
            "sprite_pixels_exceeded",
            "sprite_file_too_large",
        ] {
            assert!(
                STASIS_GRAPHICS_SOURCE.contains(required),
                "runtime sprite decode should contain {required}"
            );
        }
        let scaled_extent = STASIS_GRAPHICS_SOURCE
            .find("const int raster_w = stasis_display_scaled_extent(max_w, g_pixel_scale);")
            .expect("scaled sprite extent");
        let bounds_check = STASIS_GRAPHICS_SOURCE
            .find("sprite_source_within_limits(path, raster_w, raster_h)")
            .expect("scaled sprite bounds check");
        let image_bake = STASIS_GRAPHICS_SOURCE
            .find("bake_image_to_rgba_sized(path, raster_w, raster_h, &pixels, &w, &h)")
            .expect("scaled sprite image bake");
        assert!(
            scaled_extent < bounds_check && bounds_check < image_bake,
            "runtime must scale and validate sprite bounds before image allocation"
        );
    }

    #[test]
    fn sprite_reload_preserves_previous_gpu_resource_until_replacement_succeeds() {
        assert!(
            STASIS_GRAPHICS_SOURCE.contains("SDL_Texture* previous = e->sdl_tex;")
                && STASIS_GRAPHICS_SOURCE.contains("if (previous) SDL_DestroyTexture(previous);")
                && STASIS_GRAPHICS_SOURCE.contains("const int can_reuse_existing = 0;")
                && STASIS_GRAPHICS_SOURCE.contains("sprite_gpu_upload_failed"),
            "sprite reload should publish a completed replacement before releasing the previous resource"
        );
    }

    #[test]
    fn sprite_release_invalidates_stale_handles_before_slot_reuse() {
        for required in [
            "stasis_gfx_release_sprite",
            "SPRITE_HANDLE_INDEX_BITS",
            "SPRITE_HANDLE_GENERATION_MASK",
            "g_sprites[idx].generation != generation",
            "!g_sprites[i].used && !g_sprites[i].retired",
            "e->retired = next_generation == 0u ? 1 : 0",
        ] {
            assert!(
                STASIS_GRAPHICS_SOURCE.contains(required),
                "sprite lifetime ownership should contain {required}"
            );
        }
    }

    #[test]
    fn invalid_sprite_draws_use_a_procedural_fallback_resource() {
        for required in [
            "static SpriteEntry g_sprite_fallback",
            "static const unsigned char pixels[16]",
            "if (!e) e = sprite_fallback_get()",
            "SDL_CreateTexture(",
            "atlas_alloc(2, 2, \"<fallback>\"",
        ] {
            assert!(
                STASIS_GRAPHICS_SOURCE.contains(required),
                "fallback sprite path should contain {required}"
            );
        }
    }

    fn decode_zero_terminated_utf8(bytes: &[u8]) -> String {
        let end = bytes
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(bytes.len());
        String::from_utf8(bytes[..end].to_vec()).expect("utf8 should decode")
    }

    fn expected_host_set(config: &RunnerConfig) -> host_set_registry::HostSetContract {
        resolve_host_set_contract(config).expect("host-set contract should resolve in tests")
    }

    fn attach_host_set(
        request: &mut stasis_runner::swap::contracts::SwapCommitRequest,
        contract: &host_set_registry::HostSetContract,
    ) {
        request.host_set_id = Some(contract.host_set_id.clone());
        request.host_set_hash = Some(contract.host_set_hash);
    }

    #[test]
    fn apply_play_data_binding_value_populates_registered_scalars_and_strings() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_f64_global_table();

        let mut json_loaded = 0i32;
        let mut screen_width = 0i32;
        let mut wide_value = 0i32;
        let mut background_red = 0.0f32;
        let mut font_bytes = vec![0u8; 64];

        let json_loaded_hash = hash_global_path("state.config.json_loaded");
        let screen_width_hash = hash_global_path("state.config.screen_width");
        let wide_value_hash = hash_global_path("state.config.wide_value");
        let background_red_hash = hash_global_path("state.config.background_red");
        let font_path_hash = hash_global_path("state.config.font_path");

        stasis_dynload::register_global_i32_ptr(json_loaded_hash, &mut json_loaded);
        stasis_dynload::register_global_i32_ptr(screen_width_hash, &mut screen_width);
        stasis_dynload::register_global_i32_ptr(wide_value_hash, &mut wide_value);
        stasis_dynload::register_global_f32_ptr(background_red_hash, &mut background_red);
        stasis_dynload::register_global_u8_array(
            font_path_hash,
            0,
            font_bytes.as_mut_ptr(),
            font_bytes.len(),
        );

        let metadata = PlayStructMetadata {
            version: 1,
            global_name: "state".to_string(),
            csv_table: None,
            fields: vec![
                PlayStructFieldMetadata {
                    json_path: "config.json_loaded".to_string(),
                    csv_column: None,
                    type_name: "bool".to_string(),
                    array_count: 1,
                },
                PlayStructFieldMetadata {
                    json_path: "config.screen_width".to_string(),
                    csv_column: None,
                    type_name: "i32".to_string(),
                    array_count: 1,
                },
                PlayStructFieldMetadata {
                    json_path: "config.background_red".to_string(),
                    csv_column: None,
                    type_name: "f32".to_string(),
                    array_count: 1,
                },
                PlayStructFieldMetadata {
                    json_path: "config.wide_value".to_string(),
                    csv_column: None,
                    type_name: "u32".to_string(),
                    array_count: 1,
                },
                PlayStructFieldMetadata {
                    json_path: "config.font_path".to_string(),
                    csv_column: None,
                    type_name: "string".to_string(),
                    array_count: 64,
                },
            ],
        };
        let root = serde_json::json!({
            "config": {
                "json_loaded": true,
                "screen_width": 800,
                "wide_value": 4294967295_u64,
                "background_red": 0.25,
                "font_path": "C:/Windows/Fonts/consola.ttf"
            }
        });

        apply_play_data_binding_value(&root, &metadata).expect("binding should succeed");

        assert_eq!(json_loaded, 1);
        assert_eq!(screen_width, 800);
        assert_eq!(wide_value as u32, u32::MAX);
        assert!((background_red - 0.25).abs() < f32::EPSILON);
        assert_eq!(
            decode_zero_terminated_utf8(&font_bytes),
            "C:/Windows/Fonts/consola.ttf"
        );
        assert_eq!(
            stasis_dynload::stasis_jit_collection_i32_load(font_path_hash, 1),
            "C:/Windows/Fonts/consola.ttf".len() as i32
        );
        assert_eq!(
            stasis_dynload::stasis_jit_collection_i32_load(font_path_hash, 3),
            "C:/Windows/Fonts/consola.ttf".chars().count() as i32
        );

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
        stasis_dynload::clear_jit_f64_global_table();
    }

    #[test]
    fn apply_play_data_binding_value_rejects_unknown_metadata_version() {
        let metadata = PlayStructMetadata {
            version: 2,
            global_name: "state".to_string(),
            csv_table: None,
            fields: Vec::new(),
        };
        let root = serde_json::json!({});

        let error =
            apply_play_data_binding_value(&root, &metadata).expect_err("binding should fail");
        assert!(error.contains("unsupported struct-meta version"));
    }

    #[test]
    fn parse_flat_csv_binding_supports_scalar_and_columnar_data() {
        let scalar = parse_flat_csv_binding(
            "enabled,label,wide\r\ntrue,\"Fast, tough\",4294967295\r\n",
            &[
                CsvBindingField {
                    path: "enabled".to_string(),
                    csv_column: None,
                    type_name: "bool".to_string(),
                    array_count: 1,
                },
                CsvBindingField {
                    path: "label".to_string(),
                    csv_column: None,
                    type_name: "string".to_string(),
                    array_count: 32,
                },
                CsvBindingField {
                    path: "wide".to_string(),
                    csv_column: None,
                    type_name: "u32".to_string(),
                    array_count: 1,
                },
            ],
        )
        .expect("scalar CSV should parse");
        assert_eq!(
            scalar,
            serde_json::json!({"enabled": true, "label": "Fast, tough", "wide": 4294967295_u64})
        );

        let arrays = parse_flat_csv_binding(
            "hp,speed\n70,1.5\n110,2.25\n85,3\n",
            &[
                CsvBindingField {
                    path: "hp".to_string(),
                    csv_column: None,
                    type_name: "i32".to_string(),
                    array_count: 3,
                },
                CsvBindingField {
                    path: "speed".to_string(),
                    csv_column: None,
                    type_name: "f32".to_string(),
                    array_count: 3,
                },
            ],
        )
        .expect("columnar CSV should parse");
        assert_eq!(
            arrays,
            serde_json::json!({"hp": [70, 110, 85], "speed": [1.5, 2.25, 3.0]})
        );
    }

    #[test]
    fn parse_flat_csv_binding_rejects_nested_metadata_paths() {
        let error = parse_flat_csv_binding(
            "screen_width\n800\n",
            &[CsvBindingField {
                path: "config.screen_width".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 1,
            }],
        )
        .expect_err("nested CSV metadata should fail");
        assert!(error.contains("flat column"));
    }

    #[test]
    fn parse_flat_csv_binding_rejects_columns_missing_from_target_metadata() {
        let error = parse_flat_csv_binding(
            "hp,typo\n70,99\n",
            &[CsvBindingField {
                path: "hp".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 1,
            }],
        )
        .expect_err("extra CSV columns should fail");
        assert!(error.contains("column typo does not exist in target metadata"));
    }

    #[test]
    fn parse_csv_table_binding_tracks_count_pads_capacity_and_validates_keys() {
        let fields = vec![
            CsvBindingField {
                path: "rows.id".to_string(),
                csv_column: Some("id".to_string()),
                type_name: "i32".to_string(),
                array_count: 4,
            },
            CsvBindingField {
                path: "rows.hp".to_string(),
                csv_column: Some("health".to_string()),
                type_name: "i32".to_string(),
                array_count: 4,
            },
        ];
        let table = CsvTableMetadata {
            rows_path: "rows".to_string(),
            row_count_path: "row_count".to_string(),
            capacity: 4,
            key_columns: vec!["id".to_string()],
        };
        let parsed = parse_csv_table_binding("id,health\n10,70\n20,110\n", &fields, &table)
            .expect("table should parse");
        assert_eq!(
            parsed,
            serde_json::json!({
                "rows": {"id": [10, 20, 0, 0], "hp": [70, 110, 0, 0]},
                "row_count": 2
            })
        );

        let duplicate = parse_csv_table_binding("id,health\n10,70\n10,110\n", &fields, &table)
            .expect_err("duplicate stable keys should fail");
        assert!(duplicate.contains("duplicate CSV table key"));
    }

    #[test]
    fn load_play_data_bindings_applies_struct_array_csv_table() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let collection_hash = hash_global_path("level.rows");
        let mut ids = [99i32; 4];
        let mut hp = [99i32; 4];
        let mut row_count = 0i32;
        stasis_dynload::register_global_i32_array(
            collection_hash,
            hash_global_path("id"),
            ids.as_mut_ptr(),
            ids.len(),
        );
        stasis_dynload::register_global_i32_array(
            collection_hash,
            hash_global_path("hp"),
            hp.as_mut_ptr(),
            hp.len(),
        );
        stasis_dynload::register_global_i32_ptr(
            hash_global_path("level.row_count"),
            &mut row_count,
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_table_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let data_path = temp_root.join("waves.csv");
        let meta_path = temp_root.join("waves.struct-meta.json");
        fs::write(&data_path, "id,hp\n10,70\n20,110\n").expect("write CSV");
        fs::write(
            &meta_path,
            r#"{"version":1,"globalName":"level","csvTable":{"rowsPath":"rows","rowCountPath":"row_count","capacity":4,"keyColumns":["id"]},"fields":[{"jsonPath":"rows.id","csvColumn":"id","type":"i32","arrayCount":4},{"jsonPath":"rows.hp","csvColumn":"hp","type":"i32","arrayCount":4}]}"#,
        )
        .expect("write metadata");

        load_and_apply_play_data_bindings(&[(data_path, meta_path)], None)
            .expect("CSV table binding should apply");
        assert_eq!(ids, [10, 20, 0, 0]);
        assert_eq!(hp, [70, 110, 0, 0]);
        assert_eq!(row_count, 2);

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn validate_play_binding_source_requires_an_exact_property_set() {
        let metadata = PlayStructMetadata {
            version: 1,
            global_name: "state".to_string(),
            csv_table: None,
            fields: vec![PlayStructFieldMetadata {
                json_path: "config.width".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 1,
            }],
        };
        validate_play_binding_source(&serde_json::json!({"config": {"width": 800}}), &metadata)
            .expect("exact JSON properties should pass");

        let extra = validate_play_binding_source(
            &serde_json::json!({"config": {"width": 800, "widht": 600}}),
            &metadata,
        )
        .expect_err("extra JSON properties should fail");
        assert!(extra.contains("config.widht does not exist in target metadata"));

        let missing = validate_play_binding_source(&serde_json::json!({"config": {}}), &metadata)
            .expect_err("missing JSON properties should fail");
        assert!(missing.contains("missing target property config.width"));
    }

    #[test]
    fn validate_play_binding_targets_rejects_missing_compiled_global() {
        let jit = JitProcess::new();
        let metadata = PlayStructMetadata {
            version: 1,
            global_name: "state".to_string(),
            csv_table: None,
            fields: vec![PlayStructFieldMetadata {
                json_path: "typo".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 1,
            }],
        };
        let error = validate_play_binding_targets(&metadata, &jit)
            .expect_err("missing runtime targets should fail");
        assert!(error.contains("state.typo does not exist in compiled globals"));
    }

    #[test]
    fn validate_play_binding_targets_rejects_wrong_type_and_capacity() {
        let mut jit = JitProcess::new();
        jit.upsert_file(
            "binding-shape.stasis",
            "global State { speed: f32; values: i32[2]; }\nfunction main(): i32 { return 0; }\n",
        );
        jit.compile().expect("binding shape fixture should compile");

        let wrong_type = PlayStructMetadata {
            version: 1,
            global_name: "State".to_string(),
            csv_table: None,
            fields: vec![PlayStructFieldMetadata {
                json_path: "speed".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 1,
            }],
        };
        let error = validate_play_binding_targets(&wrong_type, &jit)
            .expect_err("wrong scalar type should fail");
        assert!(error.contains("State.speed has type f32; metadata requires i32"));

        let wrong_capacity = PlayStructMetadata {
            version: 1,
            global_name: "State".to_string(),
            csv_table: None,
            fields: vec![PlayStructFieldMetadata {
                json_path: "values".to_string(),
                csv_column: None,
                type_name: "i32".to_string(),
                array_count: 3,
            }],
        };
        let error = validate_play_binding_targets(&wrong_capacity, &jit)
            .expect_err("wrong array capacity should fail");
        assert!(error.contains("State.values has capacity 2; metadata requires 3"));
    }

    #[test]
    fn load_play_data_bindings_applies_columnar_csv_arrays() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let mut hp = [0i32; 3];
        stasis_dynload::register_global_i32_array(
            hash_global_path("state.hp"),
            0,
            hp.as_mut_ptr(),
            hp.len(),
        );

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_csv_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let data_path = temp_root.join("balance.csv");
        let meta_path = temp_root.join("balance.struct-meta.json");
        fs::write(&data_path, "hp\n70\n110\n85\n").expect("write CSV");
        fs::write(
            &meta_path,
            r#"{"version":1,"globalName":"state","fields":[{"jsonPath":"hp","type":"i32","arrayCount":3}]}"#,
        )
        .expect("write metadata");

        load_and_apply_play_data_bindings(&[(data_path, meta_path)], None)
            .expect("CSV binding should apply");
        assert_eq!(hp, [70, 110, 85]);

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn load_play_data_bindings_rejects_set_before_mutating_runtime() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let mut value = 5i32;
        stasis_dynload::register_global_i32_ptr(hash_global_path("state.value"), &mut value);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_atomic_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let first_json = temp_root.join("first.json");
        let first_meta = temp_root.join("first.struct-meta.json");
        let second_json = temp_root.join("second.json");
        let second_meta = temp_root.join("second.struct-meta.json");
        fs::write(&first_json, r#"{"value":9}"#).expect("write first json");
        fs::write(
            &first_meta,
            r#"{"version":1,"globalName":"state","fields":[{"jsonPath":"value","type":"i32","arrayCount":1}]}"#,
        )
        .expect("write first metadata");
        fs::write(&second_json, "{invalid").expect("write invalid json");
        fs::write(
            &second_meta,
            r#"{"version":1,"globalName":"other","fields":[]}"#,
        )
        .expect("write second metadata");

        let error = load_and_apply_play_data_bindings(
            &[(first_json, first_meta), (second_json, second_meta)],
            None,
        )
        .expect_err("invalid set should be rejected");
        assert!(error.contains("failed to parse data JSON"));
        assert_eq!(
            value, 5,
            "the earlier valid file must not be partially applied"
        );

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn load_play_data_bindings_rejects_duplicate_targets_before_mutating_runtime() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let mut value = 5i32;
        stasis_dynload::register_global_i32_ptr(hash_global_path("state.value"), &mut value);

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_duplicate_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let first_json = temp_root.join("first.json");
        let first_meta = temp_root.join("first.struct-meta.json");
        let second_json = temp_root.join("second.json");
        let second_meta = temp_root.join("second.struct-meta.json");
        fs::write(&first_json, r#"{"value":9}"#).expect("write first json");
        fs::write(&second_json, r#"{"value":10}"#).expect("write second json");
        let metadata = r#"{"version":1,"globalName":"state","fields":[{"jsonPath":"value","type":"i32","arrayCount":1}]}"#;
        fs::write(&first_meta, metadata).expect("write first metadata");
        fs::write(&second_meta, metadata).expect("write second metadata");

        let error = load_and_apply_play_data_bindings(
            &[(first_json, first_meta), (second_json, second_meta)],
            None,
        )
        .expect_err("duplicate targets should be rejected");
        assert!(error.contains("binding target property state.value is mapped by both"));
        assert_eq!(value, 5, "duplicate files must not partially apply");

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_global_table();
        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn resolve_play_data_binding_paths_auto_discovers_bucket_layout() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_auto_{stamp}"));
        let sample_dir = temp_root.join("samples");
        let watch_file = sample_dir.join("bucket_catcher.stasis");
        let data_dir = sample_dir.join("bucket_catcher").join("data");
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write watch file");
        fs::write(data_dir.join("config.json"), "{}\n").expect("write json");
        fs::write(data_dir.join("config.struct-meta.json"), "{}\n").expect("write struct meta");

        let resolved = resolve_play_data_binding_paths(&watch_file, &temp_root, None, None)
            .expect("auto discovery should succeed");

        assert_eq!(
            resolved,
            vec![(
                data_dir.join("config.json"),
                data_dir.join("config.struct-meta.json")
            )]
        );

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn resolve_play_data_binding_paths_discovers_all_project_data_in_name_order() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_project_{stamp}"));
        let data_dir = temp_root.join("data");
        let watch_file = temp_root.join("src").join("main.stasis");
        fs::create_dir_all(watch_file.parent().expect("source parent")).expect("create src");
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write entry");
        for stem in ["pieces", "enemies"] {
            fs::write(data_dir.join(format!("{stem}.json")), "{}\n").expect("write json");
            fs::write(data_dir.join(format!("{stem}.struct-meta.json")), "{}\n")
                .expect("write metadata");
        }
        let nested_data_dir = data_dir.join("tables");
        fs::create_dir_all(&nested_data_dir).expect("create nested data dir");
        fs::write(nested_data_dir.join("tuning.csv"), "hp\n70\n").expect("write CSV");
        fs::write(nested_data_dir.join("tuning.struct-meta.json"), "{}\n")
            .expect("write CSV metadata");

        let resolved = resolve_play_data_binding_paths(&watch_file, &temp_root, None, None)
            .expect("project data discovery should succeed");

        assert_eq!(
            resolved,
            vec![
                (
                    data_dir.join("enemies.json"),
                    data_dir.join("enemies.struct-meta.json")
                ),
                (
                    data_dir.join("pieces.json"),
                    data_dir.join("pieces.struct-meta.json")
                ),
                (
                    nested_data_dir.join("tuning.csv"),
                    nested_data_dir.join("tuning.struct-meta.json")
                )
            ]
        );

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn resolve_play_data_binding_paths_errors_on_partial_auto_sidecars() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_partial_{stamp}"));
        let sample_dir = temp_root.join("samples");
        let watch_file = sample_dir.join("bucket_catcher.stasis");
        let data_dir = sample_dir.join("bucket_catcher").join("data");
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write watch file");
        fs::write(data_dir.join("config.json"), "{}\n").expect("write json");

        let error = resolve_play_data_binding_paths(&watch_file, &temp_root, None, None)
            .expect_err("partial sidecars should fail");
        assert!(error.contains("requires matching metadata"));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn resolve_play_data_binding_paths_rejects_json_csv_stem_collision() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_play_bind_collision_{stamp}"));
        let data_dir = temp_root.join("data");
        let watch_file = temp_root.join("main.stasis");
        fs::create_dir_all(&data_dir).expect("create data dir");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write entry");
        fs::write(data_dir.join("balance.json"), "{}\n").expect("write JSON");
        fs::write(data_dir.join("balance.csv"), "hp\n70\n").expect("write CSV");
        fs::write(data_dir.join("balance.struct-meta.json"), "{}\n").expect("write metadata");

        let error = resolve_play_data_binding_paths(&watch_file, &temp_root, None, None)
            .expect_err("shared JSON/CSV metadata should fail");
        assert!(error.contains("cannot share metadata"));

        let _ = fs::remove_dir_all(&temp_root);
    }

    #[test]
    fn compiler_thread_stages_jit_candidate_without_activating_runtime() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_staged_candidate_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("main.stasis");
        fs::write(
            &source,
            "global values: i32[4];\nfunction main(): i32 { return values.max_length; }\n",
        )
        .expect("write source");

        let max_length_hash = hash_global_path("values.max_length");
        stasis_dynload::stasis_jit_global_i32_store(max_length_hash, 99);
        let (prepared_tx, prepared_rx) = std::sync::mpsc::sync_channel(1);
        let mut backend = IncrementalCompilerBackend::new_with_prepared_jit_swaps(prepared_tx);
        let result = backend.compile(CompileRequest::new(
            RequestId(89),
            vec![source],
            TargetMode::JitDev,
        ));

        assert_eq!(result.status, CompileStatus::Success);
        assert_eq!(
            stasis_dynload::stasis_jit_global_i32_load(max_length_hash),
            99,
            "background compilation must not seed the active runtime"
        );
        let prepared = prepared_rx
            .try_recv()
            .expect("successful compile should stage a candidate");
        assert_eq!(prepared.request_id, RequestId(89));
        prepared
            .candidate
            .activate_staged_runtime()
            .expect("safe-point activation should succeed");
        assert_eq!(
            stasis_dynload::stasis_jit_global_i32_load(max_length_hash),
            4
        );

        stasis_dynload::clear_jit_i32_global_table();
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn runner_collection_growth_uses_shared_migration_transaction() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();

        let mut active = JitProcess::new();
        active.upsert_file(
            "runner_collection.stasis",
            "global values: i32[2]; function main(): i32 { return values[0]; }",
        );
        active.compile().expect("active compile");
        active
            .write_global_collection_scalar(
                "values",
                "",
                0,
                stasis_compiler::backend::jit::JitScalarValue::I32(11),
            )
            .expect("seed first value");
        active
            .write_global_collection_scalar(
                "values",
                "",
                1,
                stasis_compiler::backend::jit::JitScalarValue::I32(22),
            )
            .expect("seed second value");

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "runner_collection.stasis",
            "global values: i32[4]; function main(): i32 { return values[0]; }",
        );
        candidate.compile_staged().expect("candidate compile");
        let mut active_candidate = Some(active);
        let result = apply_prepared_jit_transaction(
            RequestId(90),
            Some(candidate),
            &mut active_candidate,
            false,
            || {
                SwapCommitResult::success(
                    RequestId(90),
                    Vec::new(),
                    stasis_runner::swap::contracts::CodeGeneration(1),
                )
            },
        )
        .expect("shared transaction should apply collection migration");

        assert_eq!(result.status, SwapCommitStatus::Success);
        let active = active_candidate.expect("candidate should become active");
        assert_eq!(
            active.read_global_collection_scalar("values", "", 0),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(11))
        );
        assert_eq!(
            active.read_global_collection_scalar("values", "", 1),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(22))
        );
        assert_eq!(
            active.read_global_collection_scalar("values", "", 2),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(0))
        );

        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_i32_array_global_table();
    }

    #[test]
    fn layout_migration_commits_between_ticks_before_new_code_runs() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();

        let source_path = "between_tick_layout_migration.stasis";
        let mut active = JitProcess::new();
        active.upsert_file(source_path, BETWEEN_TICK_LAYOUT_V1);
        active.compile().expect("compile v1");
        let active_package = active
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("build v1 package");
        stasis_dynload::begin_jit_host_entry_session(
            active_package
                .host_entry_targets(1)
                .expect("v1 host targets"),
        )
        .expect("publish v1 host targets");
        let active_entrypoints = PlayEntrypoints {
            tick_code_ptr: stasis_dynload::jit_host_tick_trampoline_ptr() as u64,
            render_code_ptr: stasis_dynload::jit_host_render_trampoline_ptr() as u64,
        };
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(0));
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.tick_code_ptr as usize),
            Ok(11)
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.render_code_ptr as usize),
            Ok(0)
        );
        assert_eq!(
            active.read_global_scalar("state.rendered_generation"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(1))
        );

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(source_path, BETWEEN_TICK_LAYOUT_V2);
        candidate.compile_staged().expect("compile staged v2");
        let package = candidate
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("build staged v2 package");
        let published = commit_play_candidate_between_ticks(
            &mut active,
            candidate,
            package.symbol_code_ptrs.keys().cloned().collect(),
            &package,
        )
        .expect("commit and publish v2 at the production between-tick boundary");
        assert_eq!(published.tick_code_ptr, active_entrypoints.tick_code_ptr);
        assert_eq!(
            published.render_code_ptr,
            active_entrypoints.render_code_ptr
        );
        assert_eq!(
            stasis_dynload::jit_host_entry_targets()
                .expect("published v2 targets")
                .revision,
            2
        );

        assert_eq!(
            active.read_global_scalar("state.score"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(11))
        );
        assert_eq!(
            active.read_global_scalar("state.combo"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(5))
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(published.tick_code_ptr as usize),
            Ok(16),
            "the first v2 tick must observe migrated state and v2 code together"
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(published.render_code_ptr as usize),
            Ok(0)
        );
        assert_eq!(
            active.read_global_scalar("state.rendered_generation"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(2)),
            "render must switch to the same accepted generation as tick"
        );

        stasis_dynload::clear_jit_i32_global_table();
    }

    #[test]
    fn background_watch_patch_keeps_old_multi_root_windows_until_atomic_publish() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        let root = std::env::temp_dir().join(format!("stasis-watch-patch-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).expect("create watch patch fixture");
        let source_path = root.join("main.stasis");
        let source_v1 = "function shared(): i32 { return 1; } function main(): i32 { return 0; } function tick(): i32 { return shared(); } function render(): i32 { return shared() + 10; } function on_code_swap(): void { return; }";
        let source_v2 = source_v1.replace("return 1", "return 2");
        fs::write(&source_path, source_v1).expect("write v1");

        let source_path_text = source_path.to_string_lossy().to_string();
        let mut active = JitProcess::new();
        active
            .set_project_root(root.to_string_lossy())
            .expect("set fixture project root");
        active.upsert_file(source_path_text.clone(), source_v1);
        active.compile().expect("compile v1");
        let active_package = active
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("build v1 package");
        stasis_dynload::begin_jit_host_entry_session(
            active_package.host_entry_targets(1).expect("v1 targets"),
        )
        .expect("publish v1 targets");
        let tick_trampoline = stasis_dynload::jit_host_tick_trampoline_ptr();
        let render_trampoline = stasis_dynload::jit_host_render_trampoline_ptr();

        fs::write(&source_path, source_v2).expect("write v2");
        let job = start_watch_patch_job(
            2,
            active.staged_candidate(),
            root.clone(),
            source_path.clone(),
            source_path_text,
        )
        .expect("start background patch");
        assert_eq!(stasis_dynload::invoke_noarg_i32(tick_trampoline), Ok(1));
        assert_eq!(stasis_dynload::invoke_noarg_i32(render_trampoline), Ok(11));
        let prepared = job
            .receiver
            .recv_timeout(Duration::from_secs(5))
            .expect("background result")
            .expect("background compile");
        job.worker.join().expect("join background compile");
        assert_eq!(
            prepared
                .candidate
                .generation_metadata()
                .expect("patch metadata")
                .emitted_function_ids
                .len(),
            3
        );
        assert_eq!(stasis_dynload::invoke_noarg_i32(tick_trampoline), Ok(1));
        assert_eq!(stasis_dynload::invoke_noarg_i32(render_trampoline), Ok(11));
        stasis_dynload::publish_jit_host_entry_targets(
            active_package
                .host_entry_targets(2)
                .expect("intervening live-edit targets"),
        )
        .expect("simulate live edit publishing while watch compile is pending");

        commit_play_candidate_between_ticks(
            &mut active,
            prepared.candidate,
            prepared.package.symbol_code_ptrs.keys().cloned().collect(),
            &prepared.package,
        )
        .expect("publish v2");
        assert_eq!(
            stasis_dynload::jit_host_tick_trampoline_ptr(),
            tick_trampoline
        );
        assert_eq!(
            stasis_dynload::jit_host_render_trampoline_ptr(),
            render_trampoline
        );
        assert_eq!(stasis_dynload::invoke_noarg_i32(tick_trampoline), Ok(2));
        assert_eq!(stasis_dynload::invoke_noarg_i32(render_trampoline), Ok(12));
        assert_eq!(
            stasis_dynload::jit_host_entry_targets()
                .expect("watch targets")
                .revision,
            3,
            "watch publication must advance from the intervening live revision"
        );
        fs::remove_dir_all(root).expect("remove watch patch fixture");
    }

    #[test]
    fn rejected_between_tick_layout_migration_keeps_old_code_and_state() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();

        let source_path = "between_tick_layout_migration.stasis";
        let mut active = JitProcess::new();
        active.upsert_file(source_path, BETWEEN_TICK_LAYOUT_V1);
        active.compile().expect("compile v1");
        let active_package = active
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("build v1 package");
        let active_entrypoints = PlayEntrypoints {
            tick_code_ptr: active_package.tick_code_ptr,
            render_code_ptr: active_package.render_code_ptr,
        };
        assert_eq!(active.execute_i32_noarg_by_name("main"), Ok(0));
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.tick_code_ptr as usize),
            Ok(11)
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.render_code_ptr as usize),
            Ok(0)
        );
        assert_eq!(
            active.read_global_scalar("state.rendered_generation"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(1))
        );

        let mut candidate = active.staged_candidate();
        candidate.upsert_file(source_path, BETWEEN_TICK_LAYOUT_REJECT);
        candidate
            .compile_staged()
            .expect("compile rejecting candidate");
        let package = candidate
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .expect("build rejecting package");
        let error = commit_play_candidate_between_ticks(
            &mut active,
            candidate,
            package.symbol_code_ptrs.keys().cloned().collect(),
            &package,
        )
        .expect_err("swap hook must reject at the production boundary");
        assert!(error.contains("rejection"));

        assert_eq!(
            active.read_global_scalar("state.score"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(11))
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.tick_code_ptr as usize),
            Ok(12),
            "the next tick must still execute v1 code against restored v1 state"
        );
        assert_eq!(
            stasis_dynload::invoke_noarg_i32(active_entrypoints.render_code_ptr as usize),
            Ok(0)
        );
        assert_eq!(
            active.read_global_scalar("state.rendered_generation"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(1)),
            "rejection must restore the v1 render generation with v1 state"
        );

        stasis_dynload::clear_jit_i32_global_table();
    }

    #[test]
    fn runner_rejects_missing_jit_candidate_without_mutating_state() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();
        let score_hash = hash_global_path("score");
        stasis_dynload::stasis_jit_global_i32_store(score_hash, 7);
        let mut active = None;
        let mut applied = false;

        let error = apply_prepared_jit_transaction(RequestId(91), None, &mut active, false, || {
            applied = true;
            SwapCommitResult::success(
                RequestId(91),
                Vec::new(),
                stasis_runner::swap::contracts::CodeGeneration(1),
            )
        })
        .expect_err("missing candidate must reject");

        assert!(error.contains("missing its staged runtime candidate"));
        assert!(!applied, "commit callback must not run");
        assert_eq!(stasis_dynload::stasis_jit_global_i32_load(score_hash), 7);
        stasis_dynload::clear_jit_i32_global_table();
    }

    #[test]
    fn layout_change_without_jit_candidate_requires_restart() {
        let mut applied = false;
        let error = apply_runtime_state_transaction(
            Some(LayoutHash([1; 32])),
            LayoutHash([2; 32]),
            false,
            || {
                applied = true;
                SwapCommitResult::success(
                    RequestId(92),
                    Vec::new(),
                    stasis_runner::swap::contracts::CodeGeneration(1),
                )
            },
        )
        .expect_err("layout-changing fallback backend must reject");

        assert!(error.contains("requires a staged JIT candidate"));
        assert!(!applied, "commit callback must not run");
    }

    #[test]
    fn watched_hook_rejection_restores_mutated_runtime_state() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        let mut active = JitProcess::new();
        active.upsert_file(
            "watch_rollback.stasis",
            "function main(): i32 { return 0; }",
        );
        active.compile().expect("active compile");
        let mut candidate = active.staged_candidate();
        candidate.compile_staged().expect("candidate compile");
        stasis_dynload::clear_jit_i32_global_table();
        let path_hash = hash_global_path("WatchRollback.score");
        REJECTING_HOOK_PATH_HASH.store(path_hash, std::sync::atomic::Ordering::Relaxed);
        stasis_dynload::stasis_jit_global_i32_store(path_hash, 7);

        let preview = plan_state_migration(
            &active.state_layout(),
            &candidate.state_layout(),
            Vec::new(),
            false,
            None,
        )
        .expect("plan");
        let error = activate_candidate_transactionally(
            Some(&active),
            &candidate,
            &preview,
            true,
            || stasis_dynload::invoke_code_swap_hook(mutating_rejecting_hook as usize),
            Result::is_ok,
        )
        .expect("runtime transaction should execute")
        .expect_err("hook should reject");

        assert!(error.contains("rejection"));
        assert_eq!(stasis_dynload::stasis_jit_global_i32_load(path_hash), 7);
        stasis_dynload::clear_jit_i32_global_table();
    }

    #[test]
    fn watched_code_only_candidate_bypasses_state_snapshot_limit() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        let mut active = JitProcess::new();
        active.upsert_file("watch_large.stasis", "function main(): i32 { return 0; }");
        active.compile().expect("active compile");
        let mut candidate = active.staged_candidate();
        candidate.upsert_file("watch_large.stasis", "function main(): i32 { return 1; }");
        candidate.compile_staged().expect("candidate compile");
        let oversized_len = MAX_STATE_SNAPSHOT_BYTES / std::mem::size_of::<i32>() + 1;
        stasis_dynload::ensure_jit_i32_array_capacity(9002, 0, oversized_len)
            .expect("oversized test storage should allocate");
        assert!(
            stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES).is_err()
        );

        let preview = plan_state_migration(
            &active.state_layout(),
            &candidate.state_layout(),
            Vec::new(),
            false,
            None,
        )
        .expect("plan");
        activate_candidate_transactionally(
            Some(&active),
            &candidate,
            &preview,
            false,
            || Ok::<(), String>(()),
            Result::is_ok,
        )
        .expect("hook-free candidate activation should not snapshot state")
        .expect("commit should succeed");

        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_array_global_table();
    }

    #[test]
    fn runner_commit_hook_rejection_restores_migrated_and_hook_state() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        stasis_dynload::clear_jit_i32_global_table();
        let mut active = JitProcess::new();
        active.upsert_file(
            "runner_rollback.stasis",
            "global score: i32; function main(): i32 { return score; }",
        );
        active.compile().expect("active compile");
        let mut candidate = active.staged_candidate();
        candidate.upsert_file(
            "runner_rollback.stasis",
            "global score: i32; global added: i32; function main(): i32 { return score + added; }",
        );
        candidate.compile_staged().expect("candidate compile");
        active
            .write_global_scalar(
                "score",
                stasis_compiler::backend::jit::JitScalarValue::I32(7),
            )
            .expect("seed active score");
        let added_hash = hash_global_path("added");
        stasis_dynload::stasis_jit_global_i32_store(added_hash, 3);
        REJECTING_HOOK_PATH_HASH.store(added_hash, std::sync::atomic::Ordering::Relaxed);
        let request_id = RequestId(90);
        let hook_fn_id = FnId(2);
        let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([2; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(1) }],
            },
            Some("on_code_swap".to_string()),
        );
        request.hook_fn_id = Some(hook_fn_id);
        let config = RunnerConfig::default();
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut request, &host_set_contract);
        let mut pointer_table = FunctionPointerTable::new();
        let mut hook_runs = 0;
        let mut hook_failures = 0;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0;
        let mut swap_commit_failures = 0;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata = BTreeMap::new();
        let pending_jit_code_ptr_overrides = BTreeMap::from([(
            request_id,
            vec![JitCodePtrOverride {
                fn_id: hook_fn_id,
                code_ptr: mutating_rejecting_hook as usize as u64,
            }],
        )]);

        let mut active_candidate = Some(active);
        let result = apply_prepared_jit_transaction(
            request_id,
            Some(candidate),
            &mut active_candidate,
            true,
            || {
                apply_commit_request(
                    request,
                    &mut pointer_table,
                    &config,
                    &host_set_contract,
                    &mut hook_runs,
                    &mut hook_failures,
                    &mut hook_failure_reasons,
                    &mut swap_commit_successes,
                    &mut swap_commit_failures,
                    &mut swap_failure_reasons,
                    &mut events,
                    None,
                    None,
                    &pending_aot_metadata,
                    &pending_jit_code_ptr_overrides,
                )
            },
        )
        .expect("runtime transaction should execute");

        assert_eq!(result.status, SwapCommitStatus::Failed);
        assert_eq!(
            active_candidate
                .as_ref()
                .expect("active candidate retained")
                .read_global_scalar("score"),
            Ok(stasis_compiler::backend::jit::JitScalarValue::I32(7))
        );
        assert_eq!(stasis_dynload::stasis_jit_global_i32_load(added_hash), 3);
        assert_eq!(pointer_table.generation().0, 0);
        assert_eq!(hook_runs, 1);
        assert_eq!(hook_failures, 1);
        assert_eq!(swap_commit_successes, 0);
        assert_eq!(swap_commit_failures, 1);
        stasis_dynload::clear_jit_i32_global_table();
    }

    #[test]
    fn code_only_commit_without_hook_bypasses_state_snapshot_limit() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global lock should be acquired");
        let _jit = JitProcess::new();
        let oversized_len = MAX_STATE_SNAPSHOT_BYTES / std::mem::size_of::<i32>() + 1;
        stasis_dynload::ensure_jit_i32_array_capacity(9001, 0, oversized_len)
            .expect("oversized test storage should allocate");
        assert!(
            stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES).is_err()
        );

        let result = apply_runtime_state_transaction(
            Some(LayoutHash([1; 32])),
            LayoutHash([1; 32]),
            false,
            || {
                SwapCommitResult::success(
                    RequestId(91),
                    Vec::new(),
                    stasis_runner::swap::contracts::CodeGeneration(1),
                )
            },
        )
        .expect("code-only transaction should not snapshot state");

        assert_eq!(result.status, SwapCommitStatus::Success);
        stasis_dynload::clear_registered_global_memory();
        stasis_dynload::clear_jit_i32_array_global_table();
    }

    #[test]
    fn resolve_host_set_contract_prefers_explicit_config_profile_over_env() {
        let config = RunnerConfig {
            target_mode: TargetMode::AotProd,
            host_set_profile: Some("dev".to_string()),
            host_set_registry_file: None,
            ..RunnerConfig::default()
        };
        with_env_var_set(STASIS_HOST_SET_PROFILE_ENV, "prod", || {
            let contract =
                resolve_host_set_contract(&config).expect("host-set contract should resolve");
            assert_eq!(contract.host_set_id, "stasis-dev");
        });
    }

    #[test]
    fn resolve_host_set_contract_prefers_env_profile_over_target_mode_inference() {
        let config = RunnerConfig {
            target_mode: TargetMode::AotProd,
            host_set_profile: None,
            host_set_registry_file: None,
            ..RunnerConfig::default()
        };
        with_env_var_set(STASIS_HOST_SET_PROFILE_ENV, "dev", || {
            let contract =
                resolve_host_set_contract(&config).expect("host-set contract should resolve");
            assert_eq!(contract.host_set_id, "stasis-dev");
        });
    }

    #[test]
    fn resolve_host_set_contract_uses_registry_file_from_env_when_set() {
        let config = RunnerConfig {
            target_mode: TargetMode::JitDev,
            host_set_profile: Some("dev".to_string()),
            host_set_registry_file: None,
            ..RunnerConfig::default()
        };
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let tmp: PathBuf = std::env::temp_dir().join(format!(
            "stasis_host_set_registry_env_resolution_{}.json",
            stamp
        ));
        fs::write(
            &tmp,
            "{\"profiles\":{\"dev\":{\"id\":\"editor-host\",\"hash\":\"000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f\"}}}",
        )
        .expect("write registry");

        let tmp_str = tmp.to_string_lossy().to_string();
        with_env_var_set(STASIS_HOST_SET_REGISTRY_FILE_ENV, &tmp_str, || {
            let contract =
                resolve_host_set_contract(&config).expect("host-set contract should resolve");
            assert_eq!(contract.host_set_id, "editor-host");
            assert_eq!(contract.host_set_hash[0], 0);
            assert_eq!(contract.host_set_hash[31], 31);
        });

        fs::remove_file(&tmp).ok();
    }

    #[test]
    fn apply_commit_request_rejects_missing_host_set_contract_metadata() {
        let request_id = RequestId(44);
        let request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            None,
        );

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig::default();
        let host_set_contract = expected_host_set(&config);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Failed);
        assert_eq!(hook_runs, 0);
        assert_eq!(hook_failures, 0);
        assert_eq!(swap_commit_successes, 0);
        assert_eq!(swap_commit_failures, 1);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("host-set contract mismatch")),
            "expected host-set contract mismatch error, got {:?}",
            result.error
        );
        assert_eq!(swap_failure_reasons.len(), 1);
        assert!(swap_failure_reasons[0].contains("host-set contract mismatch"));
        assert_eq!(pointer_table.generation().0, 0);
        assert!(pointer_table.code_ptr(FnId(9)).is_none());
        assert!(events.is_empty());
    }

    #[test]
    fn resolve_play_watch_dir_defaults_to_dot_for_basename_entry() {
        let resolved = resolve_play_watch_dir(Path::new("flappy.stasis"), None);
        assert_eq!(resolved, PathBuf::from("."));
    }

    #[test]
    fn resolve_play_window_title_prefers_project_name_and_falls_back_to_entry() {
        let entry = Path::new("src/main.stasis");
        assert_eq!(
            resolve_play_window_title(entry, Some("Chess TD")),
            "Chess TD"
        );
        assert_eq!(resolve_play_window_title(entry, None), "main.stasis");
        assert_eq!(resolve_play_window_title(entry, Some("")), "main.stasis");
    }

    #[test]
    fn resolve_play_watch_dir_ignores_empty_explicit_watch_dir() {
        let resolved = resolve_play_watch_dir(Path::new("flappy.stasis"), Some(Path::new("")));
        assert_eq!(resolved, PathBuf::from("."));
    }

    #[test]
    fn resolve_play_watch_dir_uses_parent_when_present() {
        let resolved = resolve_play_watch_dir(Path::new("samples/game.stasis"), None);
        assert_eq!(resolved, PathBuf::from("samples"));
    }

    #[test]
    fn resolve_play_watch_dir_prefers_explicit_watch_dir() {
        let resolved = resolve_play_watch_dir(
            Path::new("samples/game.stasis"),
            Some(Path::new("override")),
        );
        assert_eq!(resolved, PathBuf::from("override"));
    }

    #[test]
    fn standalone_play_root_supports_entry_imports_in_sibling_directory() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let project = std::env::temp_dir().join(format!("stasis_play_project_{stamp}"));
        let src = project.join("src");
        let lib = project.join("lib");
        fs::create_dir_all(&src).expect("create src");
        fs::create_dir_all(&lib).expect("create lib");
        let entry = src.join("main.stasis");
        fs::write(
            &entry,
            "import \"../lib/helper.stasis\";\nfunction main(): i32 { return helper(); }\n",
        )
        .expect("write entry");
        fs::write(
            lib.join("helper.stasis"),
            "function helper(): i32 { return 7; }\n",
        )
        .expect("write helper");
        let entry = entry.canonicalize().expect("canonical entry");
        let watch_dir = project.canonicalize().expect("canonical project");
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .canonicalize()
            .expect("canonical repository");

        let selected =
            resolve_play_project_root(&entry, &watch_dir, &repository).expect("project root");
        assert_eq!(selected, watch_dir);
        let mut jit = JitProcess::new();
        jit.set_project_root(selected.to_string_lossy())
            .expect("set project root");
        jit.upsert_file(
            entry.to_string_lossy().to_string(),
            fs::read_to_string(&entry).expect("read entry"),
        );
        jit.compile().expect("compile standalone project");
        assert_eq!(
            jit.execute_i32_noarg_by_name("main").expect("execute main"),
            7
        );

        fs::remove_dir_all(&project).ok();
    }

    #[test]
    fn play_prepares_manifest_assets_in_a_source_relative_cache_mirror() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_play_assets_{stamp}"));
        let source_dir = root.join("src");
        let asset_dir = root.join("assets/fonts");
        fs::create_dir_all(&source_dir).expect("create source directory");
        fs::create_dir_all(&asset_dir).expect("create asset directory");
        let font = b"font fixture";
        fs::write(asset_dir.join("ui.ttf"), font).expect("write font");
        let manifest = serde_json::json!({
            "schema": "stasis-assets",
            "version": 2,
            "assets": [{
                "id": "ui",
                "path": "assets/fonts/ui.ttf",
                "content_sha256": stasis_assets::sha256_bytes(font),
                "format": { "kind": "font", "encoding": "ttf" },
                "dependencies": []
            }]
        });
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec_pretty(&manifest).expect("encode manifest"),
        )
        .expect("write manifest");

        let working_dir = prepare_play_asset_working_dir(&source_dir).expect("prepare play assets");

        assert_eq!(working_dir, root.join(".stasis_cache/play-assets/src"));
        assert_eq!(
            fs::read(working_dir.join("../assets/fonts/ui.ttf")).expect("read prepared font"),
            font
        );
        assert!(root.join(".stasis_cache/assets").is_dir());
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn apply_commit_request_uses_jit_code_ptr_overrides_when_present() {
        let request_id = RequestId(44);
        let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            None,
        );

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig::default();
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut request, &host_set_contract);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();
        pending_jit_code_ptr_overrides.insert(
            request_id,
            vec![JitCodePtrOverride {
                fn_id: FnId(9),
                code_ptr: 0x9988,
            }],
        );

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Success);
        assert_eq!(swap_commit_successes, 1);
        assert_eq!(
            pointer_table.code_ptr(FnId(9)),
            Some(stasis_jit::CodePtr(0x9988))
        );
    }

    #[test]
    fn apply_commit_request_rejects_jit_hook_missing_hook_fn_id_when_overrides_present() {
        let request_id = RequestId(44);
        let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            Some("on_code_swap".to_string()),
        );
        request.hook_fn_id = None;

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            runtime_launch: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut request, &host_set_contract);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();
        pending_jit_code_ptr_overrides.insert(
            request_id,
            vec![JitCodePtrOverride {
                fn_id: FnId(9),
                code_ptr: 0x9988,
            }],
        );

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Failed);
        assert_eq!(hook_runs, 1);
        assert_eq!(hook_failures, 1);
        assert_eq!(swap_commit_successes, 0);
        assert_eq!(swap_commit_failures, 1);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("missing hook_fn_id")),
            "expected missing hook_fn_id error, got {:?}",
            result.error
        );
        assert_eq!(pointer_table.generation().0, 0);
        assert!(pointer_table.code_ptr(FnId(9)).is_none());
    }

    #[test]
    fn apply_commit_request_rejects_jit_hook_missing_code_ptr_override_entry() {
        let request_id = RequestId(44);
        let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            Some("on_code_swap".to_string()),
        );
        request.hook_fn_id = Some(FnId(7));

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            runtime_launch: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut request, &host_set_contract);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();
        // Override exists, but not for hook fn id 7, so hook dispatch is unresolved.
        pending_jit_code_ptr_overrides.insert(
            request_id,
            vec![JitCodePtrOverride {
                fn_id: FnId(9),
                code_ptr: 0x9988,
            }],
        );

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Failed);
        assert_eq!(hook_runs, 1);
        assert_eq!(hook_failures, 1);
        assert_eq!(swap_commit_successes, 0);
        assert_eq!(swap_commit_failures, 1);
        assert!(
            result
                .error
                .as_deref()
                .is_some_and(|error| error.contains("missing JIT hook code pointer override")),
            "expected missing hook code ptr override error, got {:?}",
            result.error
        );
        assert_eq!(pointer_table.generation().0, 0);
        assert!(pointer_table.code_ptr(FnId(9)).is_none());
    }

    #[test]
    fn jit_hook_failure_preserves_previous_generation() {
        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            runtime_launch: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();

        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();

        let first_request_id = RequestId(44);
        let mut first_request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            first_request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            None,
        );
        attach_host_set(&mut first_request, &host_set_contract);
        let first = apply_commit_request(
            first_request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );
        assert_eq!(first.status, SwapCommitStatus::Success);
        assert_eq!(pointer_table.generation().0, 1);
        let ptr_after_first = pointer_table.code_ptr(FnId(9));
        assert!(ptr_after_first.is_some());

        let second_request_id = RequestId(45);
        let mut second_request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            second_request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            Some("on_code_swap".to_string()),
        );
        attach_host_set(&mut second_request, &host_set_contract);
        second_request.hook_fn_id = Some(FnId(7));
        let mut second_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> = BTreeMap::new();
        // Note: override list doesn't include hook fn id 7, so commit must abort before swap.
        second_overrides.insert(
            second_request_id,
            vec![JitCodePtrOverride {
                fn_id: FnId(9),
                code_ptr: 0x9988,
            }],
        );

        let second = apply_commit_request(
            second_request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &second_overrides,
        );
        assert_eq!(second.status, SwapCommitStatus::Failed);
        assert_eq!(pointer_table.generation().0, 1);
        assert_eq!(pointer_table.code_ptr(FnId(9)), ptr_after_first);
    }

    #[test]
    fn aot_native_hook_skips_when_disabled() {
        let request_id = RequestId(44);
        let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            Some("on_code_swap".to_string()),
        );
        request.hook_fn_id = Some(FnId(7));

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            target_mode: TargetMode::AotProd,
            runtime_launch: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut request, &host_set_contract);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();

        let result = apply_commit_request(
            request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );

        assert_eq!(result.status, SwapCommitStatus::Success);
        assert_eq!(hook_runs, 0);
        assert_eq!(hook_failures, 0);
        assert_eq!(swap_commit_successes, 1);
        assert_eq!(swap_commit_failures, 0);
        assert_eq!(pointer_table.generation().0, 1);
    }

    #[test]
    fn aot_native_hook_rejects_missing_metadata_when_enabled() {
        with_env_var_set("STASIS_AOT_EXECUTE_NATIVE_HOOK", "1", || {
            let request_id = RequestId(44);
            let mut request = stasis_runner::swap::contracts::SwapCommitRequest::new(
                request_id,
                LayoutHash([7; 32]),
                FunctionPatchSet {
                    functions: vec![FunctionPatch { fn_id: FnId(9) }],
                },
                Some("on_code_swap".to_string()),
            );
            request.hook_fn_id = Some(FnId(7));

            let mut pointer_table = FunctionPointerTable::new();
            let config = RunnerConfig {
                target_mode: TargetMode::AotProd,
                runtime_launch: false,
                ..RunnerConfig::default()
            };
            let host_set_contract = expected_host_set(&config);
            attach_host_set(&mut request, &host_set_contract);
            let mut hook_runs = 0u32;
            let mut hook_failures = 0u32;
            let mut hook_failure_reasons = Vec::new();
            let mut swap_commit_successes = 0u32;
            let mut swap_commit_failures = 0u32;
            let mut swap_failure_reasons = Vec::new();
            let mut events = Vec::new();
            let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> =
                BTreeMap::new();
            let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
                BTreeMap::new();

            let result = apply_commit_request(
                request,
                &mut pointer_table,
                &config,
                &host_set_contract,
                &mut hook_runs,
                &mut hook_failures,
                &mut hook_failure_reasons,
                &mut swap_commit_successes,
                &mut swap_commit_failures,
                &mut swap_failure_reasons,
                &mut events,
                None,
                None,
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            );

            assert_eq!(result.status, SwapCommitStatus::Failed);
            assert_eq!(hook_runs, 1);
            assert_eq!(hook_failures, 1);
            assert_eq!(swap_commit_successes, 0);
            assert_eq!(swap_commit_failures, 1);
            assert!(
                result.error.as_deref().is_some_and(|error| {
                    error.contains("missing AOT compile metadata")
                        || error.contains("missing AOT compile")
                }),
                "expected missing AOT metadata error, got {:?}",
                result.error
            );
            assert_eq!(pointer_table.generation().0, 0);
        });
    }

    #[cfg(windows)]
    #[test]
    fn aot_native_hook_executes_hook_export_and_mutates_state_when_enabled() {
        fn find_lld_link() -> Option<PathBuf> {
            let candidates = [
                r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
                r"C:\Program Files\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\lld-link.exe",
            ];
            candidates
                .iter()
                .map(PathBuf::from)
                .find(|path| path.exists())
        }

        let Some(linker_path) = find_lld_link() else {
            return;
        };

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_aot_hook_exec_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let source = temp_root.join("game.stasis");
        fs::write(
            &source,
            "global State { hook_runs: i32; }\nfunction on_code_swap(): void { State.hook_runs += 1; return; }\nfunction main(): i32 { return State.hook_runs; }\n",
        )
        .expect("write source");

        let compile_config = stasis_jit::AotCompileConfig::default();
        let link_config = stasis_jit::AotLinkConfig {
            linker_path: Some(linker_path),
            runtime_lib_paths: vec![],
            target: stasis_jit::AotTarget::default(),
        };
        let artifact_root = temp_root.join("aot_artifacts");
        let mut backend = IncrementalCompilerBackend::with_aot_compile_and_link_config(
            compile_config,
            link_config,
            artifact_root,
            true,
        );

        let request_id = RequestId(18_700);
        let compiled = backend.compile(CompileRequest::new(
            request_id,
            vec![source],
            TargetMode::AotProd,
        ));
        if compiled.status != CompileStatus::Success {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }

        let linked_image = compiled
            .aot_linked_image_path
            .as_ref()
            .expect("linked image path should exist");
        let hook_fn_id = compiled
            .hook_fn_id
            .expect("hook_fn_id should be populated for on_code_swap");
        let symbols = compiled
            .aot_function_symbols
            .clone()
            .expect("AotProd compile should emit function symbols");
        if symbols.len() != 2 {
            fs::remove_dir_all(&temp_root).ok();
            return;
        }
        let main_export = symbols
            .iter()
            .find(|entry| entry.fn_id != hook_fn_id)
            .map(|entry| entry.symbol.clone())
            .expect("main export should exist");

        // Keep the image loaded so the hook call inside apply_commit_request mutates the same module instance.
        let library = stasis_dynload::Library::load(linked_image).expect("load linked image");
        let main_ptr = library
            .symbol_address(&main_export)
            .expect("resolve main export");
        let before = stasis_dynload::invoke_noarg_i32(main_ptr).expect("invoke main");
        assert_eq!(before, 0);

        let mut pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> =
            BTreeMap::new();
        pending_aot_metadata.insert(
            request_id,
            PendingAotCompileMetadata {
                linked_image_path: Some(linked_image.clone()),
                linked_image_size_bytes: compiled.aot_linked_image_size_bytes,
                function_symbols: Some(symbols),
            },
        );
        let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();

        let mut commit_request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            request_id,
            compiled.layout_hash.expect("layout hash should exist"),
            compiled
                .fn_patch_set
                .expect("patch set should exist for successful compile"),
            compiled.hook_symbol.clone(),
        );
        commit_request.hook_fn_id = Some(hook_fn_id);

        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            target_mode: TargetMode::AotProd,
            runtime_launch: false,
            aot_probe_loadability: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        attach_host_set(&mut commit_request, &host_set_contract);
        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();

        with_env_var_set("STASIS_AOT_EXECUTE_NATIVE_HOOK", "1", || {
            let result = apply_commit_request(
                commit_request,
                &mut pointer_table,
                &config,
                &host_set_contract,
                &mut hook_runs,
                &mut hook_failures,
                &mut hook_failure_reasons,
                &mut swap_commit_successes,
                &mut swap_commit_failures,
                &mut swap_failure_reasons,
                &mut events,
                None,
                None,
                &pending_aot_metadata,
                &pending_jit_code_ptr_overrides,
            );
            assert_eq!(result.status, SwapCommitStatus::Success);
        });

        let after = stasis_dynload::invoke_noarg_i32(main_ptr).expect("invoke main after hook");
        assert_eq!(after, 1);

        drop(library);
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn hook_failure_reason_preserves_previous_generation() {
        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            runtime_launch: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();

        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();

        let first_request_id = RequestId(44);
        let mut first_request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            first_request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            None,
        );
        attach_host_set(&mut first_request, &host_set_contract);
        let first = apply_commit_request(
            first_request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            None,
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );
        assert_eq!(first.status, SwapCommitStatus::Success);
        assert_eq!(pointer_table.generation().0, 1);
        let ptr_after_first = pointer_table.code_ptr(FnId(9));
        assert!(ptr_after_first.is_some());

        let reason = "state invariant mismatch".to_string();
        let second_request_id = RequestId(45);
        let mut second_request = stasis_runner::swap::contracts::SwapCommitRequest::new(
            second_request_id,
            LayoutHash([7; 32]),
            FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(9) }],
            },
            Some("on_code_swap".to_string()),
        );
        attach_host_set(&mut second_request, &host_set_contract);
        let second = apply_commit_request(
            second_request,
            &mut pointer_table,
            &config,
            &host_set_contract,
            &mut hook_runs,
            &mut hook_failures,
            &mut hook_failure_reasons,
            &mut swap_commit_successes,
            &mut swap_commit_failures,
            &mut swap_failure_reasons,
            &mut events,
            Some(&reason),
            None,
            &pending_aot_metadata,
            &pending_jit_code_ptr_overrides,
        );
        assert_eq!(second.status, SwapCommitStatus::Failed);
        assert_eq!(pointer_table.generation().0, 1);
        assert_eq!(pointer_table.code_ptr(FnId(9)), ptr_after_first);
        assert_eq!(swap_commit_successes, 1);
        assert_eq!(swap_commit_failures, 1);
        assert_eq!(hook_failures, 1);
    }

    #[test]
    fn runner_loop_compiles_and_commits_one_change() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from(
                "samples/brickout_revenge/brickout_revenge_v1.stasis",
            )),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.compile_diagnostics.len(), 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.swap_indicator_armed_count, 1);
        assert_eq!(summary.swap_flash_peak_ticks, SWAP_FLASH_TICKS_MAX);
        assert!(summary.swap_flash_ticks_remaining < SWAP_FLASH_TICKS_MAX);
        assert!(summary.window.is_none());
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 5);
        assert!(matches!(
            summary.events[4],
            RunnerEvent::Summary {
                compile_successes: 1,
                compile_failures: 0,
                swap_commit_successes: 1,
                swap_commit_failures: 0,
                swap_indicator_armed_count: 1,
                swap_flash_peak_ticks: SWAP_FLASH_TICKS_MAX,
                swap_flash_ticks_remaining: _,
                window_width: None,
                window_height: None,
                ticks_executed: _,
                has_in_flight_work: false,
                ..
            }
        ));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::CompileResult {
                ref status,
                request_id: _,
                diagnostics: _,
                ..
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::HookResult {
                ref status,
                request_id: _,
                symbol: _,
                error: None
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapCommitResult {
                ref status,
                request_id: _,
                swapped_fn_ids: _,
                new_generation: _,
                error: _,
                ..
            } if status == "success"
        )));
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapIndicatorArmed {
                request_id: _,
                ticks: SWAP_FLASH_TICKS_MAX
            }
        )));
    }

    #[test]
    fn runner_loop_reports_compile_failure_and_skips_commit() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/invalid.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: true,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 0);
        assert_eq!(summary.compile_failures, 1);
        assert_eq!(summary.hook_runs, 0);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.swap_failure_reasons.len(), 0);
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert_eq!(summary.compile_diagnostics.len(), 1);
        assert!(summary.compile_diagnostics[0].contains("simulated compile failure"));
        assert_eq!(summary.last_swap_status, None);
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 2);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::CompileResult {
                ref status,
                request_id: _,
                ref diagnostics,
                ..
            } if status == "failed" && !diagnostics.is_empty()
        )));
        assert!(matches!(
            summary.events[1],
            RunnerEvent::Summary {
                compile_successes: 0,
                compile_failures: 1,
                swap_commit_successes: 0,
                swap_commit_failures: 0,
                swap_indicator_armed_count: 0,
                swap_flash_peak_ticks: 0,
                swap_flash_ticks_remaining: 0,
                window_width: None,
                window_height: None,
                ticks_executed: _,
                has_in_flight_work: false,
                ..
            }
        ));
    }

    #[test]
    fn runner_loop_surfaces_swap_failure_reason() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/swap_fail.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: Some("simulated swap rejection: layout mismatch".to_string()),
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 0);
        assert_eq!(summary.hook_failure_reasons.len(), 0);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Failed));
        assert_eq!(summary.swap_failure_reasons.len(), 1);
        assert!(summary.swap_failure_reasons[0].contains("layout mismatch"));
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 4);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::SwapCommitResult {
                ref status,
                request_id: _,
                swapped_fn_ids: _,
                new_generation: None,
                ref error,
                ..
            } if status == "failed" && error.as_deref() == Some("simulated swap rejection: layout mismatch")
        )));
    }

    #[test]
    fn runner_loop_hook_failure_aborts_swap_and_surfaces_error() {
        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/hook_fail.stasis")),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: Some("state invariant mismatch".to_string()),
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 1);
        assert_eq!(summary.hook_failure_reasons.len(), 1);
        assert!(summary.hook_failure_reasons[0].contains("state invariant mismatch"));
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Failed));
        assert_eq!(summary.swap_failure_reasons.len(), 1);
        assert!(summary.swap_failure_reasons[0].contains("on_code_swap failed"));
        assert_eq!(summary.swap_indicator_armed_count, 0);
        assert_eq!(summary.swap_flash_peak_ticks, 0);
        assert_eq!(summary.swap_flash_ticks_remaining, 0);
        assert!(summary.window.is_none());
        assert!(!summary.has_in_flight_work);
        assert_eq!(summary.events.len(), 4);
        assert!(summary.events.iter().any(|event| matches!(
            event,
            RunnerEvent::HookResult {
                ref status,
                request_id: _,
                symbol: _,
                ref error
            } if status == "failed" && error.as_ref().is_some_and(|e| e.contains("on_code_swap failed"))
        )));
    }

    #[test]
    fn watch_directory_change_triggers_compile_and_swap() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_test_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let watch_file = temp_root.join("game.stasis");
        fs::write(&watch_file, "function main(): i32 { return 0; }\n").expect("write initial file");

        let watch_file_for_thread = watch_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(
                &watch_file_for_thread,
                "function main(): i32 { return 1; }\n",
            )
            .expect("update watched file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: None,
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert!(summary.compile_successes >= 1);
        assert!(summary.hook_runs >= 1);
        assert_eq!(summary.hook_failures, 0);
        assert!(summary.swap_commit_successes >= 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert!(summary.swap_indicator_armed_count >= 1);
        assert_eq!(summary.swap_flash_peak_ticks, SWAP_FLASH_TICKS_MAX);
        assert!(summary.swap_flash_ticks_remaining <= SWAP_FLASH_TICKS_MAX);
        assert!(summary.window.is_none());
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
        assert!(summary.events.len() >= 3);
    }

    #[test]
    fn watch_directory_dependency_change_triggers_recompile() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_dep_change_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root_file = temp_root.join("game.stasis");
        let dep_file = temp_root.join("dep.stasis");
        fs::write(
            &root_file,
            "import \"./dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(&dep_file, "function dep(): i32 { return 0; }\n").expect("write dep");

        let dep_file_for_thread = dep_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(&dep_file_for_thread, "function dep(): i32 { return 1; }\n")
                .expect("update dependency file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(root_file),
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert!(summary.compile_successes >= 2);
        assert!(summary.swap_commit_successes >= 2);
        assert_eq!(summary.compile_failures, 0);
    }

    #[test]
    fn watch_directory_ignores_non_dependency_changes() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root =
            std::env::temp_dir().join(format!("stasis_watch_ignore_unrelated_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root_file = temp_root.join("game.stasis");
        let dep_file = temp_root.join("dep.stasis");
        let unrelated_file = temp_root.join("unrelated.stasis");
        fs::write(
            &root_file,
            "import \"./dep.stasis\";\nfunction main(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(&dep_file, "function dep(): i32 { return 0; }\n").expect("write dep");
        fs::write(&unrelated_file, "function helper(): i32 { return 0; }\n")
            .expect("write unrelated");

        let unrelated_file_for_thread = unrelated_file.clone();
        let writer = thread::spawn(move || {
            thread::sleep(Duration::from_millis(40));
            fs::write(
                &unrelated_file_for_thread,
                "function helper(): i32 { return 1; }\n",
            )
            .expect("update unrelated file");
        });

        let config = RunnerConfig {
            max_ticks: 300,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(root_file),
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        writer.join().expect("writer thread join");
        fs::remove_dir_all(&temp_root).expect("cleanup temp dir");

        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.compile_failures, 0);
    }

    #[test]
    fn runner_dispatches_aot_target_mode_when_configured() {
        use std::sync::{Arc, Mutex};

        let seen_modes: Arc<Mutex<Vec<TargetMode>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_modes_capture = Arc::clone(&seen_modes);
        let backend = move |request: CompileRequest| -> CompileResult {
            seen_modes_capture
                .lock()
                .expect("poisoned")
                .push(request.target_mode);
            let patch_set = FunctionPatchSet {
                functions: vec![FunctionPatch { fn_id: FnId(1) }],
            };
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([2; 32]),
                patch_set,
                request.host_set_id.clone(),
                request.host_set_hash,
                None,
                None,
                None,
                None,
                None,
                None,
            )
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/prod_mode.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_backend(config, backend);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.hook_runs, 0);

        let modes = seen_modes.lock().expect("poisoned");
        assert_eq!(modes.as_slice(), &[TargetMode::AotProd]);
    }

    #[test]
    fn runtime_launch_requires_injected_source_file() {
        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_default_backend(config);
        assert_eq!(summary.runtime_launches, 0);
        assert_eq!(summary.runtime_launch_failures, 1);
        assert!(summary
            .runtime_launch_failure_reasons
            .iter()
            .any(|reason| reason.contains("no --watch-file source file")));
    }

    #[test]
    fn resolve_initial_source_file_prefers_explicit_watch_file() {
        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/explicit.stasis")),
            watch_directory: Some(PathBuf::from("samples/brickout_revenge")),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let resolved = resolve_initial_source_file(&config).expect("resolved source file");
        assert_eq!(resolved, PathBuf::from("samples/explicit.stasis"));
    }

    #[test]
    fn real_backend_normalizes_source_and_root_spelling_together() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_runner_root_{stamp}"));
        let nested = temp_root.join("nested");
        fs::create_dir_all(&nested).expect("create nested directory");
        let source = temp_root.join("engine.stasis");
        fs::write(&source, "function main(): i32 { return 0; }\n").expect("write source");
        let mut config = RunnerConfig {
            inject_file_change: Some(nested.join("..").join("engine.stasis")),
            watch_directory: Some(nested.join("..")),
            ..RunnerConfig::default()
        };

        normalize_real_backend_config_paths(&mut config);

        assert_eq!(
            config.inject_file_change,
            Some(source.canonicalize().expect("canonical source"))
        );
        assert_eq!(
            config.watch_directory,
            Some(temp_root.canonicalize().expect("canonical root"))
        );
        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn resolve_initial_source_file_infers_entry_from_watch_dir() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_entry_infer_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        fs::write(
            temp_root.join("helper.stasis"),
            "function util(): i32 { return 1; }\n",
        )
        .expect("write helper");
        fs::write(
            temp_root.join("main.stasis"),
            "function tick(): i32 { return 0; }\nfunction render(): i32 { return 0; }\n",
        )
        .expect("write entry");

        let config = RunnerConfig {
            max_ticks: 1,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: None,
            watch_directory: Some(temp_root.clone()),
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: true,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let resolved = resolve_initial_source_file(&config).expect("resolved source file");
        assert_eq!(resolved, temp_root.join("main.stasis"));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn collect_watch_dependency_paths_includes_nested_imports() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_dep_graph_{}", stamp));
        let sub_dir = temp_root.join("sub");
        fs::create_dir_all(&sub_dir).expect("create temp dirs");
        let root = temp_root.join("main.stasis");
        let dep = temp_root.join("dep.stasis");
        let dep2 = sub_dir.join("dep2.stasis");
        fs::write(
            &root,
            "import \"./dep.stasis\";\nfunction tick(): i32 { return dep(); }\n",
        )
        .expect("write root");
        fs::write(
            &dep,
            "import \"./sub/dep2.stasis\";\nfunction dep(): i32 { return dep2(); }\n",
        )
        .expect("write dep");
        fs::write(&dep2, "function dep2(): i32 { return 1; }\n").expect("write dep2");

        let graph = collect_watch_dependency_paths(&temp_root, &root).expect("dependency graph");
        assert!(graph.contains(&normalize_watch_path_for_log(&root)));
        assert!(graph.contains(&normalize_watch_path_for_log(&dep)));
        assert!(graph.contains(&normalize_watch_path_for_log(&dep2)));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn watch_dependencies_use_the_compiler_project_root_for_nested_entries() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let project_root = std::env::temp_dir().join(format!("stasis_watch_project_root_{stamp}"));
        let sample_dir = project_root.join("samples/brickout_revenge");
        let stdlib_dir = project_root.join("src/stdlib");
        fs::create_dir_all(&sample_dir).expect("sample directory");
        fs::create_dir_all(&stdlib_dir).expect("stdlib directory");
        let entry = sample_dir.join("main.stasis");
        let dependency = stdlib_dir.join("score.stasis");
        fs::write(
            &entry,
            "import \"../../src/stdlib/score.stasis\"; function main(): i32 { return score(); }",
        )
        .expect("entry source");
        fs::write(&dependency, "function score(): i32 { return 18; }").expect("dependency source");

        let watch_paths =
            collect_watch_dependency_paths(&project_root, &entry).expect("watch dependency graph");
        let (compiler_graph, _) =
            stasis_compiler::frontend::module_graph::load_project_module_graph(
                &project_root,
                &entry,
            )
            .expect("compiler module graph");
        let compiler_paths = compiler_graph
            .modules()
            .keys()
            .map(|path| normalize_watch_path_for_log(&project_root.join(path)))
            .collect::<BTreeSet<_>>();
        assert_eq!(watch_paths, compiler_paths);
        assert!(watch_paths.contains(&normalize_watch_path_for_log(&dependency)));

        fs::remove_dir_all(&project_root).expect("fixture cleanup");
    }

    #[test]
    fn should_submit_watch_event_filters_non_dependency_paths() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_watch_filter_{}", stamp));
        fs::create_dir_all(&temp_root).expect("create temp dir");
        let root = temp_root.join("root.stasis");
        let dep = temp_root.join("dep.stasis");
        let other = temp_root.join("other.stasis");
        fs::write(&root, "function tick(): i32 { return 0; }\n").expect("write root");
        fs::write(&dep, "function dep(): i32 { return 0; }\n").expect("write dep");
        fs::write(&other, "function other(): i32 { return 0; }\n").expect("write other");

        let mut dependency_paths: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        dependency_paths.insert(normalize_watch_path_for_log(&root));
        dependency_paths.insert(normalize_watch_path_for_log(&dep));

        let dep_event = FileChangeEvent::new(
            dep.clone(),
            1,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        );
        let other_event = FileChangeEvent::new(
            other.clone(),
            2,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        );
        assert!(should_submit_watch_event(
            &dep_event,
            Some(&root),
            Some(&dependency_paths)
        ));
        assert!(!should_submit_watch_event(
            &other_event,
            Some(&root),
            Some(&dependency_paths)
        ));

        fs::remove_dir_all(&temp_root).ok();
    }

    #[test]
    fn aot_probe_loadability_rejects_missing_linked_image() {
        let missing_linked_image = std::env::temp_dir().join(format!(
            "stasis_missing_probe_{}.dll",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        if missing_linked_image.exists() {
            fs::remove_file(&missing_linked_image).ok();
        }

        let linked_image_for_backend = missing_linked_image.clone();
        let backend = move |request: CompileRequest| -> CompileResult {
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([3; 32]),
                FunctionPatchSet {
                    functions: vec![FunctionPatch { fn_id: FnId(1) }],
                },
                request.host_set_id.clone(),
                request.host_set_hash,
                None,
                None,
                Some(linked_image_for_backend.clone()),
                Some(128),
                Some("abc".to_string()),
                None,
            )
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/probe_missing.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: true,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_backend(config, backend);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 0);
        assert_eq!(summary.swap_commit_failures, 1);
        assert!(summary
            .swap_failure_reasons
            .iter()
            .any(|reason| reason.contains("AOT loadability probe failed")));
        assert_eq!(summary.aot_linked_image_activations, 0);
    }

    #[test]
    fn aot_activation_tracks_latest_linked_image_metadata() {
        let linked_image = std::env::temp_dir().join(format!(
            "stasis_activation_probe_{}.dll",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        fs::write(&linked_image, "fake-linked-image").expect("write linked image fixture");

        let linked_image_for_backend = linked_image.clone();
        let backend = move |request: CompileRequest| -> CompileResult {
            CompileResult::success_with_host_set_metadata(
                request.request_id,
                LayoutHash([5; 32]),
                FunctionPatchSet {
                    functions: vec![FunctionPatch { fn_id: FnId(1) }],
                },
                request.host_set_id.clone(),
                request.host_set_hash,
                None,
                None,
                Some(linked_image_for_backend.clone()),
                Some(17),
                Some("abcd".to_string()),
                None,
            )
        };

        let config = RunnerConfig {
            max_ticks: 200,
            tick_sleep_micros: 0,
            window: None,
            inject_file_change: Some(PathBuf::from("samples/prod_activation.stasis")),
            watch_directory: None,
            target_mode: TargetMode::AotProd,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_backend(config, backend);
        fs::remove_file(&linked_image).ok();
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.aot_linked_image_activations, 1);
        assert_eq!(
            summary.active_aot_linked_image_path,
            Some(linked_image.clone())
        );
        assert_eq!(summary.active_aot_linked_image_size_bytes, Some(17));
        assert_eq!(summary.active_aot_linked_image_generation, Some(1));
        assert_eq!(summary.retired_aot_linked_images, 0);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_returns_7.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_binary_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_returns_6_binary.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_void_hook_and_literal_main() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_main_and_hook.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_executes_hook_and_mutates_global_state() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global table lock should succeed");
        stasis_dynload::clear_jit_i32_global_table();

        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_hook_mutates_global.stasis");
        let config = RunnerConfig {
            // Real backend compile can take multiple seconds on busy CI/dev hosts.
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.hook_runs, 1);
        assert_eq!(summary.hook_failures, 0);

        let hook_runs =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("State.hook_runs"));
        assert_eq!(hook_runs, 1);

        stasis_dynload::clear_jit_i32_global_table();
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_runs_hook_on_subsequent_commits_when_hook_body_unchanged() {
        let _global_lock = jit_global_table_lock()
            .lock()
            .expect("jit global table lock should succeed");
        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();

        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("tests")
            .join("stasis")
            .join("rust_native_jit_smoke_hook_parity_combo.stasis");

        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock should be after epoch")
            .as_nanos();
        let temp_root = std::env::temp_dir().join(format!("stasis_hook_unchanged_{stamp}"));
        fs::create_dir_all(&temp_root).expect("create temp root");
        let game = temp_root.join("game.stasis");
        fs::copy(&fixture, &game).expect("copy hook fixture");

        let (prepared_tx, prepared_rx) = std::sync::mpsc::sync_channel(1);
        let backend = IncrementalCompilerBackend::new_with_prepared_jit_swaps(prepared_tx);
        let mut pipeline = DevHotSwapPipeline::with_target_mode(backend, TargetMode::JitDev);
        let mut pointer_table = FunctionPointerTable::new();
        let config = RunnerConfig {
            target_mode: TargetMode::JitDev,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            ..RunnerConfig::default()
        };
        let host_set_contract = expected_host_set(&config);
        pipeline.set_host_set_contract(
            Some(host_set_contract.host_set_id.clone()),
            Some(host_set_contract.host_set_hash),
        );

        let mut hook_runs = 0u32;
        let mut hook_failures = 0u32;
        let mut hook_failure_reasons = Vec::new();
        let mut swap_commit_successes = 0u32;
        let mut swap_commit_failures = 0u32;
        let mut swap_failure_reasons = Vec::new();
        let mut events = Vec::new();
        let pending_aot_metadata: BTreeMap<RequestId, PendingAotCompileMetadata> = BTreeMap::new();
        let mut pending_jit_code_ptr_overrides: BTreeMap<RequestId, Vec<JitCodePtrOverride>> =
            BTreeMap::new();
        let mut pending_jit_candidates = BTreeMap::new();
        let mut active_jit_candidate = None;

        pipeline.submit_file_change(FileChangeEvent::new(
            game.clone(),
            1,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        ));

        let timeout = Duration::from_secs(90);
        let start = Instant::now();
        let mut last_commit_seen: Option<RequestId> = None;
        while start.elapsed() < timeout {
            pipeline.pump_coordinator();
            capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
            capture_prepared_jit_swaps(Some(&prepared_rx), &mut pending_jit_candidates);
            pipeline.process_commits_at_safe_point(|request| {
                let request_id = request.request_id;
                let candidate = pending_jit_candidates.remove(&request_id);
                apply_prepared_jit_transaction(
                    request_id,
                    candidate,
                    &mut active_jit_candidate,
                    true,
                    || {
                        apply_commit_request(
                            request,
                            &mut pointer_table,
                            &config,
                            &host_set_contract,
                            &mut hook_runs,
                            &mut hook_failures,
                            &mut hook_failure_reasons,
                            &mut swap_commit_successes,
                            &mut swap_commit_failures,
                            &mut swap_failure_reasons,
                            &mut events,
                            None,
                            None,
                            &pending_aot_metadata,
                            &pending_jit_code_ptr_overrides,
                        )
                    },
                )
                .unwrap_or_else(|error| SwapCommitResult::failed(request_id, error))
            });
            pipeline.pump_coordinator();

            if let Some(result) = pipeline.last_commit_result() {
                if last_commit_seen != Some(result.request_id) {
                    last_commit_seen = Some(result.request_id);
                    if result.status == SwapCommitStatus::Success {
                        break;
                    }
                }
            }

            thread::sleep(Duration::from_millis(1));
        }

        let first_commit_id = pipeline
            .last_commit_result()
            .map(|result| {
                assert_eq!(result.status, SwapCommitStatus::Success);
                result.request_id
            })
            .expect("first commit should exist");

        let contents = fs::read_to_string(&game).expect("read game fixture");
        let updated = contents.replace("return 0;", "return 1;");
        fs::write(&game, updated).expect("write updated game fixture");

        pipeline.submit_file_change(FileChangeEvent::new(
            game,
            2,
            TextSource::FileWatcher,
            FileChangeKind::Modified,
        ));

        let start = Instant::now();
        while start.elapsed() < timeout {
            pipeline.pump_coordinator();
            capture_pending_jit_compile_metadata(&pipeline, &mut pending_jit_code_ptr_overrides);
            capture_prepared_jit_swaps(Some(&prepared_rx), &mut pending_jit_candidates);
            pipeline.process_commits_at_safe_point(|request| {
                let request_id = request.request_id;
                let candidate = pending_jit_candidates.remove(&request_id);
                apply_prepared_jit_transaction(
                    request_id,
                    candidate,
                    &mut active_jit_candidate,
                    true,
                    || {
                        apply_commit_request(
                            request,
                            &mut pointer_table,
                            &config,
                            &host_set_contract,
                            &mut hook_runs,
                            &mut hook_failures,
                            &mut hook_failure_reasons,
                            &mut swap_commit_successes,
                            &mut swap_commit_failures,
                            &mut swap_failure_reasons,
                            &mut events,
                            None,
                            None,
                            &pending_aot_metadata,
                            &pending_jit_code_ptr_overrides,
                        )
                    },
                )
                .unwrap_or_else(|error| SwapCommitResult::failed(request_id, error))
            });
            pipeline.pump_coordinator();

            if let Some(result) = pipeline.last_commit_result() {
                if last_commit_seen != Some(result.request_id) {
                    last_commit_seen = Some(result.request_id);
                    if result.status == SwapCommitStatus::Success {
                        break;
                    }
                }
            }

            thread::sleep(Duration::from_millis(1));
        }

        let second_commit_id = pipeline
            .last_commit_result()
            .map(|result| {
                assert_eq!(result.status, SwapCommitStatus::Success);
                result.request_id
            })
            .expect("second commit should exist");
        assert_ne!(
            second_commit_id, first_commit_id,
            "second commit request id should differ from first"
        );

        assert_eq!(swap_commit_failures, 0);
        assert_eq!(hook_runs, 2);
        assert_eq!(hook_failures, 0);

        let hook_runs =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("State.hook_runs"));
        assert_eq!(hook_runs, 11);
        let hook_branch =
            stasis_dynload::stasis_jit_global_i32_load(hash_global_path("State.hook_branch"));
        assert_eq!(hook_branch, 2);
        let hook_sin =
            stasis_dynload::stasis_jit_global_f32_load(hash_global_path("State.hook_sin"));
        assert!(
            hook_sin.abs() < 0.0001,
            "expected sin_fast(0.0) to be near 0.0, got {hook_sin}"
        );

        stasis_dynload::clear_jit_i32_global_table();
        stasis_dynload::clear_jit_f32_global_table();
    }

    #[cfg(windows)]
    #[test]
    fn real_backend_smoke_compiles_and_commits_brickout_v1() {
        let fixture = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("samples")
            .join("brickout_revenge")
            .join("brickout_revenge_v1.stasis");
        let config = RunnerConfig {
            max_ticks: 7000,
            tick_sleep_micros: 1000,
            window: None,
            inject_file_change: Some(fixture),
            watch_directory: None,
            target_mode: TargetMode::JitDev,
            fail_compile: false,
            disable_on_code_swap_hook: false,
            hook_failure_reason: None,
            swap_failure_reason: None,
            runtime_launch: false,
            aot_probe_loadability: false,
            host_set_profile: None,
            host_set_registry_file: None,
        };

        let summary = run_with_real_backend(config);
        assert_eq!(summary.compile_successes, 1);
        assert_eq!(summary.compile_failures, 0);
        assert_eq!(summary.swap_commit_successes, 1);
        assert_eq!(summary.swap_commit_failures, 0);
        assert_eq!(summary.last_swap_status, Some(SwapCommitStatus::Success));
        assert!(!summary.has_in_flight_work);
    }
}
