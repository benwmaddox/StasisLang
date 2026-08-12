use super::{
    compare_live_validation_values, compile_workspace_jit, execute_noarg_entry,
    validate_workspace_destination, RuntimeValidationRequirement, Workspace,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::jit::{JitProcess, JitScalarValue};
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use std::collections::BTreeMap;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

const SCENARIO_SCHEMA_VERSION: u32 = 1;
const FAILURE_RECEIPT_SCHEMA_VERSION: u32 = 1;
const MAX_HEADLESS_TICKS: u64 = 1_000_000;
const MAX_SCENARIOS: usize = 256;
const MAX_CASES: usize = 256;
const MAX_TOTAL_CASES: usize = 1_024;
const MAX_TOTAL_TICKS: u64 = 10_000_000;
const MAX_DISCOVERY_ENTRIES: usize = 16_384;
const MAX_DIRECTORY_ENTRIES: usize = 4_096;
const MAX_INVARIANTS: usize = 64;
const MAX_STATE_ENTRIES: usize = 4_096;
const MAX_SCENARIO_BYTES: u64 = 1_048_576;
const MAX_FAILURE_HASHES: usize = 1_024;

#[derive(Debug)]
pub(super) struct HeadlessRunSummary {
    pub ticks_executed: u64,
    pub state_hash: Option<String>,
}

#[derive(Debug, Default)]
pub(super) struct ScenarioTestSummary {
    pub scenarios_discovered: usize,
    pub cases_run: usize,
    pub cases_passed: usize,
    pub cases_failed: usize,
    pub failures: Vec<String>,
    pub failure_receipts: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    schema_version: u32,
    name: String,
    ticks: u64,
    #[serde(default)]
    state_file: Option<PathBuf>,
    #[serde(default)]
    state: BTreeMap<String, Value>,
    #[serde(default)]
    invariants: Vec<RuntimeValidationRequirement>,
    #[serde(default)]
    expected_hashes: Vec<String>,
    #[serde(default)]
    property: Option<PropertyCases>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PropertyCases {
    seed_path: String,
    seeds: Vec<i32>,
}

#[derive(Debug, Serialize)]
struct FailureReceipt<'a> {
    schema_version: u32,
    scenario: &'a str,
    scenario_name: &'a str,
    seed: Option<i32>,
    failed_tick: u64,
    reason: &'a str,
    observed_hashes: &'a [String],
    observed_hashes_truncated: bool,
    rerun: String,
    rerun_argv: [String; 2],
}

struct ScenarioFailure {
    seed: Option<i32>,
    tick: u64,
    reason: String,
    hashes: Vec<String>,
    hashes_truncated: bool,
}

struct ScenarioRun {
    cases_run: usize,
    cases_passed: usize,
    failure: Option<ScenarioFailure>,
}

pub(super) fn run_ticks(jit: &JitProcess, ticks: u64) -> Result<HeadlessRunSummary, String> {
    validate_tick_count(ticks)?;
    for _ in 0..ticks {
        execute_noarg_entry(jit, "tick")?;
    }
    Ok(HeadlessRunSummary {
        ticks_executed: ticks,
        state_hash: (ticks > 0)
            .then(|| simulation_state_hash(jit))
            .transpose()?,
    })
}

pub(super) fn run_scenarios(
    workspace: &Workspace,
    directory: &Path,
) -> Result<ScenarioTestSummary, String> {
    let scenario_files = collect_scenario_files(workspace, directory)?;
    let scenarios = load_scenarios_for_run(workspace, &scenario_files)?;
    let mut summary = ScenarioTestSummary {
        scenarios_discovered: scenarios.len(),
        ..ScenarioTestSummary::default()
    };
    for (scenario_path, scenario) in scenarios {
        let run = run_scenario(workspace, &scenario_path, &scenario)?;
        summary.cases_run += run.cases_run;
        summary.cases_passed += run.cases_passed;
        if let Some(failure) = run.failure {
            summary.cases_failed += 1;
            let relative = relative_scenario_path(workspace, &scenario_path)?;
            let receipt = write_failure_receipt(workspace, &relative, &scenario, &failure)?;
            summary.failures.push(format!(
                "scenario '{}' seed {} failed at tick {}: {}",
                scenario.name,
                failure
                    .seed
                    .map_or_else(|| "none".to_string(), |seed| seed.to_string()),
                failure.tick,
                failure.reason
            ));
            summary.failure_receipts.push(receipt);
        }
    }
    Ok(summary)
}

fn load_scenarios_for_run(
    workspace: &Workspace,
    paths: &[PathBuf],
) -> Result<Vec<(PathBuf, Scenario)>, String> {
    let mut total_cases = 0usize;
    let mut total_ticks = 0u64;
    let mut scenarios = Vec::with_capacity(paths.len());
    for path in paths {
        let scenario = load_scenario(workspace, path)?;
        let cases = scenario
            .property
            .as_ref()
            .map_or(1, |property| property.seeds.len());
        (total_cases, total_ticks) =
            checked_scenario_work(total_cases, total_ticks, cases, scenario.ticks)?;
        scenarios.push((path.clone(), scenario));
    }
    Ok(scenarios)
}

fn checked_scenario_work(
    total_cases: usize,
    total_ticks: u64,
    cases: usize,
    ticks_per_case: u64,
) -> Result<(usize, u64), String> {
    let total_cases = total_cases
        .checked_add(cases)
        .ok_or_else(|| "scenario case count overflow".to_string())?;
    let ticks = ticks_per_case
        .checked_mul(cases as u64)
        .ok_or_else(|| "scenario tick work overflow".to_string())?;
    let total_ticks = total_ticks
        .checked_add(ticks)
        .ok_or_else(|| "scenario tick work overflow".to_string())?;
    if total_cases > MAX_TOTAL_CASES || total_ticks > MAX_TOTAL_TICKS {
        return Err(format!(
            "scenario invocation requires {total_cases} case(s) and {total_ticks} tick(s); limits are {MAX_TOTAL_CASES} cases and {MAX_TOTAL_TICKS} ticks"
        ));
    }
    Ok((total_cases, total_ticks))
}

fn run_scenario(
    workspace: &Workspace,
    scenario_path: &Path,
    scenario: &Scenario,
) -> Result<ScenarioRun, String> {
    let jit = compile_workspace_jit(workspace)?;
    execute_noarg_entry(&jit, "main")?;
    let state = load_scenario_state(workspace, scenario_path, scenario)?;
    apply_state(&jit, &state)?;
    let baseline = stasis_dynload::snapshot_jit_runtime_state_bounded(MAX_STATE_SNAPSHOT_BYTES)?;
    let seeds = scenario
        .property
        .as_ref()
        .map(|property| property.seeds.iter().copied().map(Some).collect::<Vec<_>>())
        .unwrap_or_else(|| vec![None]);

    let case_count = seeds.len();
    let mut cases_passed = 0;
    for seed in seeds {
        stasis_dynload::restore_jit_runtime_state(&baseline);
        if let (Some(property), Some(seed)) = (&scenario.property, seed) {
            write_state_value(&jit, &property.seed_path, &Value::from(seed))?;
        }
        let mut hashes = Vec::with_capacity(
            usize::try_from(scenario.ticks)
                .unwrap_or(MAX_FAILURE_HASHES)
                .min(MAX_FAILURE_HASHES),
        );
        for tick in 1..=scenario.ticks {
            execute_noarg_entry(&jit, "tick")?;
            let hash = simulation_state_hash(&jit)?;
            if hashes.len() < MAX_FAILURE_HASHES {
                hashes.push(hash.clone());
            }
            for invariant in &scenario.invariants {
                let actual = state_value_json(&read_state_value(&jit, &invariant.path)?)?;
                if !compare_live_validation_values(&actual, &invariant.op, &invariant.value)? {
                    return Ok(ScenarioRun {
                        cases_run: cases_passed + 1,
                        cases_passed,
                        failure: Some(ScenarioFailure {
                            seed,
                            tick,
                            reason: format!(
                                "invariant {} {} {} observed {}",
                                invariant.path, invariant.op, invariant.value, actual
                            ),
                            hashes,
                            hashes_truncated: tick as usize > MAX_FAILURE_HASHES,
                        }),
                    });
                }
            }
            if let Some(expected) = scenario.expected_hashes.get((tick - 1) as usize) {
                if expected != &hash {
                    return Ok(ScenarioRun {
                        cases_run: cases_passed + 1,
                        cases_passed,
                        failure: Some(ScenarioFailure {
                            seed,
                            tick,
                            reason: format!(
                                "state hash mismatch: expected {expected}, observed {hash}"
                            ),
                            hashes,
                            hashes_truncated: tick as usize > MAX_FAILURE_HASHES,
                        }),
                    });
                }
            }
        }
        cases_passed += 1;
    }
    Ok(ScenarioRun {
        cases_run: case_count,
        cases_passed,
        failure: None,
    })
}

fn load_scenario(workspace: &Workspace, path: &Path) -> Result<Scenario, String> {
    validate_workspace_destination(workspace, "scenario", path)?;
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed to inspect scenario {}: {error}", path.display()))?;
    if metadata.len() > MAX_SCENARIO_BYTES {
        return Err(format!(
            "scenario {} exceeds the {}-byte limit",
            path.display(),
            MAX_SCENARIO_BYTES
        ));
    }
    let source = fs::read_to_string(path)
        .map_err(|error| format!("failed to read scenario {}: {error}", path.display()))?;
    let scenario: Scenario = serde_json::from_str(&source)
        .map_err(|error| format!("invalid scenario {}: {error}", path.display()))?;
    validate_scenario(&scenario)?;
    Ok(scenario)
}

fn validate_scenario(scenario: &Scenario) -> Result<(), String> {
    if scenario.schema_version != SCENARIO_SCHEMA_VERSION {
        return Err(format!(
            "unsupported scenario schema_version {}; expected {}",
            scenario.schema_version, SCENARIO_SCHEMA_VERSION
        ));
    }
    if scenario.name.trim().is_empty() || scenario.name.len() > 120 {
        return Err("scenario name must contain 1..=120 characters".to_string());
    }
    validate_tick_count(scenario.ticks)?;
    if scenario.invariants.is_empty() || scenario.invariants.len() > MAX_INVARIANTS {
        return Err(format!(
            "scenario invariants must contain 1..={MAX_INVARIANTS} checks"
        ));
    }
    if scenario.state.len() > MAX_STATE_ENTRIES {
        return Err(format!(
            "scenario inline state exceeds the {MAX_STATE_ENTRIES}-entry limit"
        ));
    }
    if !scenario.expected_hashes.is_empty()
        && scenario.expected_hashes.len() != scenario.ticks as usize
    {
        return Err(
            "expected_hashes must be empty or contain exactly one hash per tick".to_string(),
        );
    }
    if scenario
        .expected_hashes
        .iter()
        .any(|hash| hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()))
    {
        return Err(
            "expected_hashes must contain 64-character hexadecimal SHA-256 values".to_string(),
        );
    }
    if let Some(property) = &scenario.property {
        if property.seeds.is_empty() || property.seeds.len() > MAX_CASES {
            return Err(format!(
                "property seeds must contain 1..={MAX_CASES} values"
            ));
        }
        if !scenario.expected_hashes.is_empty() && property.seeds.len() != 1 {
            return Err(
                "expected_hashes with property testing requires exactly one seed".to_string(),
            );
        }
    }
    Ok(())
}

fn validate_tick_count(ticks: u64) -> Result<(), String> {
    if ticks > MAX_HEADLESS_TICKS {
        return Err(format!(
            "headless tick count {ticks} exceeds the {MAX_HEADLESS_TICKS}-tick limit"
        ));
    }
    Ok(())
}

fn load_scenario_state(
    workspace: &Workspace,
    scenario_path: &Path,
    scenario: &Scenario,
) -> Result<BTreeMap<String, Value>, String> {
    let mut state = if let Some(relative) = &scenario.state_file {
        if relative.is_absolute()
            || relative
                .components()
                .any(|component| matches!(component, std::path::Component::ParentDir))
        {
            return Err("scenario state_file must be a relative path without '..'".to_string());
        }
        let path = scenario_path
            .parent()
            .unwrap_or(&workspace.root)
            .join(relative);
        validate_workspace_destination(workspace, "scenario state", &path)?;
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "failed to inspect scenario state {}: {error}",
                path.display()
            )
        })?;
        if metadata.len() > MAX_SCENARIO_BYTES {
            return Err(format!(
                "scenario state {} exceeds the {}-byte limit",
                path.display(),
                MAX_SCENARIO_BYTES
            ));
        }
        let source = fs::read_to_string(&path).map_err(|error| {
            format!("failed to read scenario state {}: {error}", path.display())
        })?;
        serde_json::from_str::<BTreeMap<String, Value>>(&source)
            .map_err(|error| format!("invalid scenario state {}: {error}", path.display()))?
    } else {
        BTreeMap::new()
    };
    for (path, value) in &scenario.state {
        if state.insert(path.clone(), value.clone()).is_some() {
            return Err(format!(
                "scenario state path '{path}' appears in both state_file and inline state"
            ));
        }
    }
    if state.len() > MAX_STATE_ENTRIES {
        return Err(format!(
            "scenario state exceeds the {MAX_STATE_ENTRIES}-entry limit"
        ));
    }
    Ok(state)
}

fn apply_state(jit: &JitProcess, state: &BTreeMap<String, Value>) -> Result<(), String> {
    for (path, value) in state {
        write_state_value(jit, path, value)?;
    }
    Ok(())
}

fn write_state_value(jit: &JitProcess, path: &str, value: &Value) -> Result<(), String> {
    let reference = parse_state_path(path)?;
    let current = read_state_reference(jit, &reference)?;
    let value = coerce_value(value, current)?;
    match reference {
        StatePath::Scalar(path) => jit.write_global_scalar(path, value),
        StatePath::Collection { path, index, field } => {
            jit.write_global_collection_scalar(path, field, index, value)
        }
    }
}

fn read_state_value(jit: &JitProcess, path: &str) -> Result<JitScalarValue, String> {
    let reference = parse_state_path(path)?;
    read_state_reference(jit, &reference)
}

fn read_state_reference(jit: &JitProcess, path: &StatePath<'_>) -> Result<JitScalarValue, String> {
    match path {
        StatePath::Scalar(path) => jit.read_global_scalar(path),
        StatePath::Collection { path, index, field } => {
            jit.read_global_collection_scalar(path, field, *index)
        }
    }
}

enum StatePath<'a> {
    Scalar(&'a str),
    Collection {
        path: &'a str,
        index: i32,
        field: &'a str,
    },
}

fn parse_state_path(path: &str) -> Result<StatePath<'_>, String> {
    let Some(open) = path.find('[') else {
        return Ok(StatePath::Scalar(path));
    };
    let close = path[open + 1..]
        .find(']')
        .map(|offset| open + 1 + offset)
        .ok_or_else(|| format!("state path '{path}' is missing ']'"))?;
    let index = path[open + 1..close]
        .parse::<i32>()
        .map_err(|error| format!("state path '{path}' has an invalid index: {error}"))?;
    let suffix = &path[close + 1..];
    let field = if suffix.is_empty() {
        ""
    } else {
        suffix
            .strip_prefix('.')
            .ok_or_else(|| format!("state path '{path}' has an invalid collection suffix"))?
    };
    if path[..open].is_empty() || field.contains('[') || field.contains(']') {
        return Err(format!("state path '{path}' is invalid"));
    }
    Ok(StatePath::Collection {
        path: &path[..open],
        index,
        field,
    })
}

fn coerce_value(value: &Value, target: JitScalarValue) -> Result<JitScalarValue, String> {
    let mismatch = || format!("value {value} is not a valid {}", target.type_name());
    match target {
        JitScalarValue::I32(_) => value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .map(JitScalarValue::I32)
            .ok_or_else(mismatch),
        JitScalarValue::U8(_) => value
            .as_u64()
            .and_then(|value| u8::try_from(value).ok())
            .map(JitScalarValue::U8)
            .ok_or_else(mismatch),
        JitScalarValue::U16(_) => value
            .as_u64()
            .and_then(|value| u16::try_from(value).ok())
            .map(JitScalarValue::U16)
            .ok_or_else(mismatch),
        JitScalarValue::U32(_) => value
            .as_u64()
            .and_then(|value| u32::try_from(value).ok())
            .map(JitScalarValue::U32)
            .ok_or_else(mismatch),
        JitScalarValue::F32(_) => value
            .as_f64()
            .filter(|value| {
                value.is_finite() && *value >= f32::MIN as f64 && *value <= f32::MAX as f64
            })
            .map(|value| JitScalarValue::F32(value as f32))
            .ok_or_else(mismatch),
        JitScalarValue::F64(_) => value
            .as_f64()
            .filter(|value| value.is_finite())
            .map(JitScalarValue::F64)
            .ok_or_else(mismatch),
        JitScalarValue::Bool(_) => value
            .as_bool()
            .map(JitScalarValue::Bool)
            .ok_or_else(mismatch),
    }
}

fn state_value_json(value: &JitScalarValue) -> Result<Value, String> {
    serde_json::to_value(value)
        .map_err(|error| format!("failed to encode state value: {error}"))?
        .get("value")
        .cloned()
        .ok_or_else(|| "encoded state value did not contain a value".to_string())
}

fn simulation_state_hash(jit: &JitProcess) -> Result<String, String> {
    let layout = jit.state_layout();
    let mut hasher = Sha256::new();
    hasher.update(b"stasis.simulation-state.v1\0");
    let mut scalars = layout.scalars;
    scalars.sort_by(|left, right| left.path.cmp(&right.path));
    for scalar in scalars {
        if is_host_or_presentation_path(&scalar.path) {
            continue;
        }
        hash_value(
            &mut hasher,
            &scalar.path,
            jit.read_global_scalar(&scalar.path)?,
        );
    }
    let mut collections = layout.collections;
    collections.sort_by(|left, right| left.path.cmp(&right.path));
    for collection in collections {
        if is_host_or_presentation_path(&collection.path) {
            continue;
        }
        let mut fields = collection.fields;
        fields.sort_by(|left, right| left.field.cmp(&right.field));
        for field in fields {
            for index in 0..collection.capacity {
                let label = format!("{}[{index}].{}", collection.path, field.field);
                let value =
                    jit.read_global_collection_scalar(&collection.path, &field.field, index)?;
                hash_value(&mut hasher, &label, value);
            }
        }
    }
    let unsupported = layout
        .opaque
        .into_iter()
        .filter(|value| !is_host_or_presentation_path(&value.path))
        .map(|value| value.path)
        .collect::<Vec<_>>();
    if !unsupported.is_empty() {
        return Err(format!(
            "simulation state hash does not support opaque state: {}",
            unsupported.join(", ")
        ));
    }
    Ok(format!("{:x}", hasher.finalize()))
}

fn hash_value(hasher: &mut Sha256, path: &str, value: JitScalarValue) {
    hasher.update((path.len() as u64).to_le_bytes());
    hasher.update(path.as_bytes());
    match value {
        JitScalarValue::I32(value) => {
            hasher.update([1]);
            hasher.update(value.to_le_bytes());
        }
        JitScalarValue::F32(value) => {
            hasher.update([2]);
            hasher.update(value.to_bits().to_le_bytes());
        }
        JitScalarValue::F64(value) => {
            hasher.update([3]);
            hasher.update(value.to_bits().to_le_bytes());
        }
        JitScalarValue::Bool(value) => hasher.update([4, u8::from(value)]),
        JitScalarValue::U8(value) => hasher.update([5, value]),
        JitScalarValue::U16(value) => {
            hasher.update([6]);
            hasher.update(value.to_le_bytes());
        }
        JitScalarValue::U32(value) => {
            hasher.update([7]);
            hasher.update(value.to_le_bytes());
        }
    }
}

fn is_host_or_presentation_path(path: &str) -> bool {
    path == "host_i32"
        || path == "host_f32"
        || path.starts_with("host_req_")
        || stasis_compiler::backend::state_layout::is_command_buffer_path(path)
}

fn collect_scenario_files(workspace: &Workspace, root: &Path) -> Result<Vec<PathBuf>, String> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    validate_workspace_destination(workspace, "scenario root", root)?;
    let mut pending = vec![root.to_path_buf()];
    let mut files = Vec::new();
    let mut discovered_entries = 0usize;
    while let Some(path) = pending.pop() {
        validate_workspace_destination(workspace, "scenario discovery path", &path)?;
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            format!(
                "failed to inspect scenario discovery path {}: {error}",
                path.display()
            )
        })?;
        if is_link_or_reparse_point(&metadata) {
            return Err(format!(
                "scenario discovery does not follow links or reparse points: {}",
                path.display()
            ));
        }
        if path.is_file() {
            if is_scenario_file(&path) {
                files.push(path);
            }
            continue;
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&path).map_err(|error| {
            format!(
                "failed to read scenario directory {}: {error}",
                path.display()
            )
        })? {
            let entry = entry.map_err(|error| {
                format!(
                    "failed to enumerate scenario directory {}: {error}",
                    path.display()
                )
            })?;
            discovered_entries = checked_discovery_entry(discovered_entries, entries.len(), &path)?;
            entries.push(entry);
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries.into_iter().rev() {
            let child = entry.path();
            let metadata = fs::symlink_metadata(&child).map_err(|error| {
                format!(
                    "failed to inspect scenario discovery entry {}: {error}",
                    child.display()
                )
            })?;
            if is_link_or_reparse_point(&metadata) {
                return Err(format!(
                    "scenario discovery does not follow links or reparse points: {}",
                    child.display()
                ));
            }
            if metadata.is_dir()
                && matches!(
                    entry.file_name().to_str(),
                    Some(".git" | ".stasis_cache" | "target" | "build")
                )
            {
                continue;
            }
            if metadata.is_dir() || (metadata.is_file() && is_scenario_file(&child)) {
                validate_workspace_destination(workspace, "scenario discovery entry", &child)?;
                pending.push(child);
            }
        }
        if files.len() > MAX_SCENARIOS || pending.len() > MAX_SCENARIOS * 8 {
            return Err(format!(
                "scenario discovery exceeds the {MAX_SCENARIOS}-file limit"
            ));
        }
    }
    files.sort_by(|left, right| {
        stasis::natural_path_cmp(
            &left.to_string_lossy().to_lowercase(),
            &right.to_string_lossy().to_lowercase(),
        )
    });
    if files.len() > MAX_SCENARIOS {
        return Err(format!(
            "scenario discovery exceeds the {MAX_SCENARIOS}-file limit"
        ));
    }
    Ok(files)
}

fn checked_discovery_entry(
    discovered_entries: usize,
    directory_entries: usize,
    directory: &Path,
) -> Result<usize, String> {
    if directory_entries >= MAX_DIRECTORY_ENTRIES {
        return Err(format!(
            "scenario directory {} exceeds the {MAX_DIRECTORY_ENTRIES}-entry limit",
            directory.display()
        ));
    }
    let discovered_entries = discovered_entries
        .checked_add(1)
        .ok_or_else(|| "scenario discovery entry count overflow".to_string())?;
    if discovered_entries > MAX_DISCOVERY_ENTRIES {
        return Err(format!(
            "scenario discovery exceeds the {MAX_DISCOVERY_ENTRIES}-entry limit"
        ));
    }
    Ok(discovered_entries)
}

fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    if metadata.file_type().is_symlink() {
        return true;
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
        return metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0;
    }
    #[cfg(not(windows))]
    false
}

fn is_scenario_file(path: &Path) -> bool {
    path.file_name()
        .and_then(|value| value.to_str())
        .is_some_and(|name| name.ends_with(".scenario.json"))
}

fn relative_scenario_path(workspace: &Workspace, path: &Path) -> Result<String, String> {
    path.strip_prefix(&workspace.root)
        .map(|relative| relative.to_string_lossy().replace('\\', "/"))
        .map_err(|_| format!("scenario is outside workspace: {}", path.display()))
}

fn write_failure_receipt(
    workspace: &Workspace,
    scenario_path: &str,
    scenario: &Scenario,
    failure: &ScenarioFailure,
) -> Result<String, String> {
    let output = workspace.root.join(&workspace.manifest.output);
    validate_workspace_destination(workspace, "scenario failure output", &output)?;
    let directory = output.join("headless-replays");
    fs::create_dir_all(&directory).map_err(|error| {
        format!(
            "failed to create scenario failure directory {}: {error}",
            directory.display()
        )
    })?;
    let mut name = scenario
        .name
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() {
                char::from(byte.to_ascii_lowercase())
            } else {
                '-'
            }
        })
        .collect::<String>();
    while name.contains("--") {
        name = name.replace("--", "-");
    }
    let name = name.trim_matches('-');
    let name = if name.is_empty() { "scenario" } else { name };
    let file_name = failure_receipt_file_name(scenario_path, name, failure.seed);
    let path = directory.join(file_name);
    validate_workspace_destination(workspace, "scenario failure receipt", &path)?;
    let receipt = FailureReceipt {
        schema_version: FAILURE_RECEIPT_SCHEMA_VERSION,
        scenario: scenario_path,
        scenario_name: &scenario.name,
        seed: failure.seed,
        failed_tick: failure.tick,
        reason: &failure.reason,
        observed_hashes: &failure.hashes,
        observed_hashes_truncated: failure.hashes_truncated,
        rerun: rerun_command(scenario_path),
        rerun_argv: ["test".to_string(), scenario_path.to_string()],
    };
    let mut bytes = serde_json::to_vec_pretty(&receipt)
        .map_err(|error| format!("failed to encode scenario failure receipt: {error}"))?;
    bytes.push(b'\n');
    let mut file = atomic_write_file::AtomicWriteFile::open(&path).map_err(|error| {
        format!(
            "failed to stage scenario receipt {}: {error}",
            path.display()
        )
    })?;
    file.write_all(&bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| {
            format!(
                "failed to stage scenario receipt {}: {error}",
                path.display()
            )
        })?;
    file.commit().map_err(|error| {
        format!(
            "failed to publish scenario failure receipt {}: {error}",
            path.display()
        )
    })?;
    relative_scenario_path(workspace, &path)
}

fn failure_receipt_file_name(
    scenario_path: &str,
    scenario_name: &str,
    seed: Option<i32>,
) -> String {
    let digest = format!("{:x}", Sha256::digest(scenario_path.as_bytes()));
    let seed = seed.map_or_else(|| "default".to_string(), |seed| seed.to_string());
    format!("{scenario_name}-{}-seed-{seed}.replay.json", &digest[..12])
}

fn rerun_command(scenario_path: &str) -> String {
    format!("stasis test \"{}\"", scenario_path.replace('"', "\\\""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn test_workspace(name: &str) -> Workspace {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_headless_{name}_{stamp}"));
        fs::create_dir_all(&root).expect("create workspace");
        Workspace {
            root,
            manifest: super::super::ProjectManifest::new("headless_test".to_string()),
        }
    }

    fn remove_workspace(workspace: &Workspace) {
        let _ = fs::remove_dir_all(&workspace.root);
    }

    #[test]
    fn state_paths_parse_scalars_and_collection_fields() {
        assert!(matches!(
            parse_state_path("score"),
            Ok(StatePath::Scalar("score"))
        ));
        assert!(matches!(
            parse_state_path("world.enemies[3].hp"),
            Ok(StatePath::Collection {
                path: "world.enemies",
                index: 3,
                field: "hp"
            })
        ));
        assert!(parse_state_path("items[nope]").is_err());
    }

    #[test]
    fn presentation_and_host_paths_are_excluded_from_simulation_hashes() {
        for path in [
            "host_i32",
            "host_f32",
            "host_req_flags",
            "gfx_cmd_i32",
            "render_cmd_i32",
            "audio_cmd_i32",
            "cmd_i32",
            "world.render_cmd_i32",
            "world.cmd_i32",
        ] {
            assert!(is_host_or_presentation_path(path), "{path}");
        }
        assert!(!is_host_or_presentation_path("world.score"));
    }

    #[test]
    fn scenario_bounds_reject_unbounded_cases() {
        let scenario = Scenario {
            schema_version: SCENARIO_SCHEMA_VERSION,
            name: "bounded".to_string(),
            ticks: MAX_HEADLESS_TICKS + 1,
            state_file: None,
            state: BTreeMap::new(),
            invariants: vec![RuntimeValidationRequirement {
                path: "score".to_string(),
                op: "eq".to_string(),
                value: json!(0),
            }],
            expected_hashes: Vec::new(),
            property: None,
        };
        assert!(validate_scenario(&scenario).is_err());
    }

    #[test]
    fn receipt_names_and_rerun_arguments_preserve_scenario_identity() {
        let first = failure_receipt_file_name("tests/one.scenario.json", "same-name", Some(7));
        let second = failure_receipt_file_name("tests/two.scenario.json", "same-name", Some(7));
        assert_ne!(first, second);
        let path = "tests/scenario with spaces.scenario.json";
        assert_eq!(
            rerun_command(path),
            "stasis test \"tests/scenario with spaces.scenario.json\""
        );
    }

    #[test]
    fn invocation_work_budget_rejects_excess_before_execution() {
        assert_eq!(checked_scenario_work(0, 0, 2, 3), Ok((2, 6)));
        assert!(checked_scenario_work(0, 0, MAX_TOTAL_CASES, MAX_HEADLESS_TICKS).is_err());
        assert!(checked_scenario_work(MAX_TOTAL_CASES, 0, 1, 0).is_err());
    }

    #[test]
    fn preflight_binds_the_scenario_bytes_used_for_execution() {
        let workspace = test_workspace("bound_preflight");
        let path = workspace.root.join("case.scenario.json");
        fs::write(
            &path,
            r#"{"schema_version":1,"name":"before","ticks":1,"invariants":[{"path":"score","op":"eq","value":0}]}"#,
        )
        .expect("write scenario");
        let loaded =
            load_scenarios_for_run(&workspace, std::slice::from_ref(&path)).expect("load scenario");
        fs::write(
            &path,
            r#"{"schema_version":1,"name":"after","ticks":2,"invariants":[{"path":"score","op":"eq","value":0}]}"#,
        )
        .expect("replace scenario");

        assert_eq!(loaded[0].1.name, "before");
        assert_eq!(loaded[0].1.ticks, 1);
        remove_workspace(&workspace);
    }

    #[test]
    fn discovery_uses_natural_path_order() {
        let workspace = test_workspace("natural_order");
        let tests = workspace.root.join("tests");
        fs::create_dir_all(&tests).expect("create tests");
        fs::write(tests.join("case10.scenario.json"), "{}").expect("write case10");
        fs::write(tests.join("case2.scenario.json"), "{}").expect("write case2");

        let files = collect_scenario_files(&workspace, &tests).expect("discover scenarios");
        let names = files
            .iter()
            .map(|path| {
                path.file_name()
                    .expect("scenario file name")
                    .to_string_lossy()
                    .into_owned()
            })
            .collect::<Vec<_>>();
        assert_eq!(names, ["case2.scenario.json", "case10.scenario.json"]);
        remove_workspace(&workspace);
    }

    #[test]
    fn discovery_limits_reject_before_collecting_an_excess_entry() {
        let directory = Path::new("tests");
        assert_eq!(checked_discovery_entry(0, 0, directory), Ok(1));
        assert!(checked_discovery_entry(0, MAX_DIRECTORY_ENTRIES, directory).is_err());
        assert!(checked_discovery_entry(MAX_DISCOVERY_ENTRIES, 0, directory).is_err());
    }

    #[test]
    fn discovery_rejects_linked_directories_when_links_are_available() {
        let workspace = test_workspace("linked_directory");
        let tests = workspace.root.join("tests");
        let target = workspace.root.join("target_scenarios");
        fs::create_dir_all(&tests).expect("create tests");
        fs::create_dir_all(&target).expect("create target");
        fs::write(target.join("case.scenario.json"), "{}").expect("write scenario");
        let link = tests.join("linked");
        #[cfg(windows)]
        let linked = std::os::windows::fs::symlink_dir(&target, &link).is_ok();
        #[cfg(unix)]
        let linked = std::os::unix::fs::symlink(&target, &link).is_ok();
        if linked {
            let error = collect_scenario_files(&workspace, &tests)
                .expect_err("linked discovery entry must be rejected");
            assert!(error.contains("does not follow links or reparse points"));
        }
        remove_workspace(&workspace);
    }
}
