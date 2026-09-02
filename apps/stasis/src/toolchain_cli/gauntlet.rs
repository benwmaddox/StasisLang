use super::{
    absolute_path, create_new_project, load_workspace, load_workspace_with_vendor_gate,
    CommandResult, VendorGate, Workspace,
};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

pub(super) mod assets;
mod controller;

pub(super) const GAUNTLET_CONFIG_NAME: &str = "gauntlet.json";
pub(super) const GAUNTLET_GOAL_NAME: &str = "GAUNTLET_GOAL.md";
const GAUNTLET_SCHEMA_VERSION: u32 = 1;
const DEFAULT_MODEL_CALLS: u32 = 100;
const DEFAULT_STALLED_CANDIDATES: u32 = 5;
const DEFAULT_BUILDER_MAX_TURNS: u32 = 30;
const DEFAULT_MODEL_TIMEOUT_MINUTES: u32 = 30;
const MAX_MODEL_TIMEOUT_MINUTES: u32 = 120;
const MAX_CAPTURE_SCENARIOS: usize = 4;
const MAX_SCENARIO_TAPS: usize = 8;
const MAX_SCENARIO_TICKS_AFTER: u32 = 300;
const MAX_GOAL_BYTES: u64 = 256 * 1024;
const MAX_REFERENCE_BYTES: u64 = 16 * 1024 * 1024;
const GAUNTLET_UI_FONT_BYTES: &[u8] =
    include_bytes!("../../assets/gauntlet-font/Basic-Regular.ttf");
const GAUNTLET_UI_FONT_LICENSE: &str = include_str!("../../assets/gauntlet-font/OFL.txt");

#[derive(Debug, Subcommand)]
pub(super) enum GauntletCommand {
    /// Create a graphical Stasis project and start its first Gauntlet run.
    New {
        name: String,
        #[arg(long, value_name = "PATH")]
        dir: PathBuf,
        #[arg(long, value_name = "PATH")]
        goal_file: PathBuf,
        #[arg(long, value_name = "FILE")]
        reference: Vec<PathBuf>,
        #[arg(long)]
        discover_references: bool,
        #[arg(long, conflicts_with = "jsonl")]
        tui: bool,
        #[arg(long, conflicts_with = "tui")]
        jsonl: bool,
        #[arg(long, default_value_t = 8)]
        max_hours: u32,
        #[arg(long, default_value_t = DEFAULT_MODEL_CALLS)]
        max_model_calls: u32,
    },
    /// Improve an existing clean Stasis project in place unless worktree isolation is configured.
    Run {
        #[arg(long, value_name = "PATH", default_value = GAUNTLET_CONFIG_NAME)]
        config: PathBuf,
        #[arg(long, value_name = "FILE")]
        reference: Vec<PathBuf>,
        #[arg(long)]
        discover_references: bool,
        #[arg(long, conflicts_with = "jsonl")]
        tui: bool,
        #[arg(long, conflicts_with = "tui")]
        jsonl: bool,
        #[arg(long)]
        max_hours: Option<u32>,
        #[arg(long)]
        max_model_calls: Option<u32>,
    },
    /// Resume a stopped or interrupted run from its last accepted checkpoint.
    Resume {
        run_id: String,
        #[arg(long, conflicts_with = "jsonl")]
        tui: bool,
        #[arg(long, conflicts_with = "tui")]
        jsonl: bool,
    },
    /// Read one persisted run without contacting a model.
    Status { run_id: String },
    /// Cooperatively stop one active run.
    Stop { run_id: String },
    /// Integrate an accepted run branch into the original clean checkout.
    Promote { run_id: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletConfigV1 {
    pub schema_version: u32,
    pub goal_file: String,
    pub quality_bar: GauntletQualityBarConfig,
    pub budget: GauntletBudget,
    pub execution: GauntletExecution,
    #[serde(default)]
    pub models: GauntletRoleModels,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletQualityBarConfig {
    pub allow_web_discovery: bool,
    #[serde(default)]
    pub references: Vec<GauntletReference>,
    #[serde(default)]
    pub required_scenarios: Vec<GauntletScenarioRequirement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletReference {
    pub path: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletScenarioRequirement {
    pub id: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub taps: Vec<GauntletScenarioTap>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletScenarioTap {
    pub x: i32,
    pub y: i32,
    #[serde(default = "default_scenario_ticks_after")]
    pub ticks_after: u32,
}

const fn default_scenario_ticks_after() -> u32 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletBudget {
    pub wall_time_minutes: u32,
    pub model_calls: u32,
    pub stalled_candidates: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum GauntletAutonomy {
    Full,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum GauntletObserver {
    Auto,
    Tui,
    Jsonl,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum GauntletIsolation {
    #[default]
    InPlace,
    Worktree,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletExecution {
    pub autonomy: GauntletAutonomy,
    pub observer: GauntletObserver,
    #[serde(default)]
    pub isolation: GauntletIsolation,
    #[serde(default = "default_builder_max_turns")]
    pub builder_max_turns: u32,
    #[serde(default)]
    pub compaction: GauntletCompaction,
}

fn default_builder_max_turns() -> u32 {
    DEFAULT_BUILDER_MAX_TURNS
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletCompaction {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_compaction_bytes")]
    pub max_request_bytes: usize,
    #[serde(default = "default_compaction_retained_turns")]
    pub retain_recent_turns: usize,
}

impl Default for GauntletCompaction {
    fn default() -> Self {
        Self {
            enabled: true,
            max_request_bytes: default_compaction_bytes(),
            retain_recent_turns: default_compaction_retained_turns(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_compaction_bytes() -> usize {
    2 * 1024 * 1024
}

fn default_compaction_retained_turns() -> usize {
    6
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletRoleModels {
    #[serde(default = "default_luna_role_model")]
    pub scout: GauntletRoleModel,
    #[serde(default)]
    pub lead: GauntletRoleModel,
    #[serde(default = "default_luna_role_model")]
    pub builder: GauntletRoleModel,
    #[serde(default = "default_builder_escalation_model")]
    pub builder_escalation: Option<GauntletRoleModel>,
    #[serde(default = "default_builder_escalation_model")]
    pub controller_escalation: Option<GauntletRoleModel>,
    #[serde(default)]
    pub visual_critic: GauntletRoleModel,
    #[serde(default)]
    pub gameplay_critic: GauntletRoleModel,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletRoleModel {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_effort: Option<String>,
    #[serde(default = "default_model_timeout_minutes")]
    pub timeout_minutes: u32,
}

impl Default for GauntletRoleModel {
    fn default() -> Self {
        Self {
            model: None,
            reasoning_effort: None,
            timeout_minutes: default_model_timeout_minutes(),
        }
    }
}

fn default_model_timeout_minutes() -> u32 {
    DEFAULT_MODEL_TIMEOUT_MINUTES
}

impl Default for GauntletRoleModels {
    fn default() -> Self {
        Self {
            scout: default_luna_role_model(),
            lead: GauntletRoleModel::default(),
            builder: default_luna_role_model(),
            builder_escalation: default_builder_escalation_model(),
            controller_escalation: default_builder_escalation_model(),
            visual_critic: GauntletRoleModel::default(),
            gameplay_critic: GauntletRoleModel::default(),
        }
    }
}

fn default_builder_escalation_model() -> Option<GauntletRoleModel> {
    Some(GauntletRoleModel {
        model: Some("gpt-5.6-sol".to_string()),
        reasoning_effort: Some("high".to_string()),
        timeout_minutes: default_model_timeout_minutes(),
    })
}

fn default_luna_role_model() -> GauntletRoleModel {
    GauntletRoleModel {
        model: Some("gpt-5.6-luna".to_string()),
        reasoning_effort: Some("max".to_string()),
        timeout_minutes: default_model_timeout_minutes(),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(super) enum GauntletRunPhase {
    Created,
    DiscoveringBar,
    Planning,
    Building,
    Evaluating,
    Checkpointing,
    Converged,
    BudgetExhausted,
    Stalled,
    Canceled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct GauntletRunStateV1 {
    pub schema_version: u32,
    pub run_id: String,
    pub phase: GauntletRunPhase,
    pub project_root: String,
    pub original_root: String,
    pub branch: String,
    pub base_commit: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_workstream: Option<String>,
    pub model_calls: u32,
    pub accepted_candidates: u32,
    pub rejected_candidates: u32,
    pub consecutive_stalls: u32,
    #[serde(default)]
    pub quality_acceptance_streak: u32,
    pub started_unix_ms: u64,
    #[serde(default)]
    pub session_started_unix_ms: u64,
    pub updated_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub terminal_reason: Option<String>,
}

impl GauntletConfigV1 {
    fn new(
        allow_web_discovery: bool,
        references: Vec<GauntletReference>,
        max_hours: u32,
        max_model_calls: u32,
        observer: GauntletObserver,
    ) -> Result<Self, String> {
        let wall_time_minutes = max_hours
            .checked_mul(60)
            .ok_or_else(|| "Gauntlet max-hours is too large".to_string())?;
        let config = Self {
            schema_version: GAUNTLET_SCHEMA_VERSION,
            goal_file: GAUNTLET_GOAL_NAME.to_string(),
            quality_bar: GauntletQualityBarConfig {
                allow_web_discovery,
                references,
                required_scenarios: Vec::new(),
            },
            budget: GauntletBudget {
                wall_time_minutes,
                model_calls: max_model_calls,
                stalled_candidates: DEFAULT_STALLED_CANDIDATES,
            },
            execution: GauntletExecution {
                autonomy: GauntletAutonomy::Full,
                observer,
                isolation: GauntletIsolation::InPlace,
                builder_max_turns: DEFAULT_BUILDER_MAX_TURNS,
                compaction: GauntletCompaction::default(),
            },
            models: GauntletRoleModels::default(),
        };
        config.validate()?;
        Ok(config)
    }

    pub(super) fn validate(&self) -> Result<(), String> {
        if self.schema_version != GAUNTLET_SCHEMA_VERSION {
            return Err(format!(
                "unsupported Gauntlet schema version {}; expected {GAUNTLET_SCHEMA_VERSION}",
                self.schema_version
            ));
        }
        validate_relative_path("goal_file", Path::new(&self.goal_file))?;
        if self.budget.wall_time_minutes == 0
            || self.budget.model_calls == 0
            || self.budget.stalled_candidates == 0
        {
            return Err("Gauntlet budgets must be greater than zero".to_string());
        }
        if self.execution.builder_max_turns == 0
            || self.execution.builder_max_turns as usize > stasis_ai::MAX_AGENT_TURNS
        {
            return Err(format!(
                "Gauntlet builder_max_turns must be between 1 and {}",
                stasis_ai::MAX_AGENT_TURNS
            ));
        }
        if self.execution.compaction.enabled
            && (!(stasis_ai::MIN_COMPACTION_BYTES..=stasis_ai::MAX_COMPACTION_BYTES)
                .contains(&self.execution.compaction.max_request_bytes)
                || self.execution.compaction.retain_recent_turns == 0
                || self.execution.compaction.retain_recent_turns
                    > stasis_ai::MAX_COMPACTION_RETAINED_TURNS)
        {
            return Err(format!(
                "Gauntlet compaction requires max_request_bytes between {} and {} and retain_recent_turns between 1 and {}",
                stasis_ai::MIN_COMPACTION_BYTES,
                stasis_ai::MAX_COMPACTION_BYTES,
                stasis_ai::MAX_COMPACTION_RETAINED_TURNS
            ));
        }
        for (role, profile) in [
            ("scout", &self.models.scout),
            ("lead", &self.models.lead),
            ("builder", &self.models.builder),
            ("visual_critic", &self.models.visual_critic),
            ("gameplay_critic", &self.models.gameplay_critic),
        ] {
            profile.validate(role)?;
        }
        if let Some(profile) = &self.models.builder_escalation {
            profile.validate("builder_escalation")?;
        }
        if let Some(profile) = &self.models.controller_escalation {
            profile.validate("controller_escalation")?;
        }
        for reference in &self.quality_bar.references {
            validate_relative_path("reference path", Path::new(&reference.path))?;
            validate_sha256(&reference.sha256)?;
        }
        let capture_scenarios = self
            .quality_bar
            .required_scenarios
            .iter()
            .filter(|scenario| !scenario.taps.is_empty())
            .count();
        if capture_scenarios > MAX_CAPTURE_SCENARIOS {
            return Err(format!(
                "Gauntlet supports at most {MAX_CAPTURE_SCENARIOS} required scenarios with capture taps"
            ));
        }
        let mut scenario_ids = std::collections::HashSet::new();
        for scenario in &self.quality_bar.required_scenarios {
            if scenario.id.trim().is_empty() || scenario.description.trim().is_empty() {
                return Err("Gauntlet scenarios require non-empty id and description".to_string());
            }
            if scenario.id.len() > 64
                || !scenario
                    .id
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            {
                return Err(
                    "Gauntlet scenario ids must be portable ASCII names up to 64 characters"
                        .to_string(),
                );
            }
            if !scenario_ids.insert(scenario.id.as_str()) {
                return Err("Gauntlet scenario ids must be unique".to_string());
            }
            if scenario.taps.len() > MAX_SCENARIO_TAPS {
                return Err(format!(
                    "Gauntlet scenario {} exceeds {MAX_SCENARIO_TAPS} taps",
                    scenario.id
                ));
            }
            for tap in &scenario.taps {
                if tap.x < 0
                    || tap.y < 0
                    || tap.x > 16_384
                    || tap.y > 16_384
                    || !(1..=MAX_SCENARIO_TICKS_AFTER).contains(&tap.ticks_after)
                {
                    return Err(format!(
                        "Gauntlet scenario {} taps require coordinates between 0 and 16384 and ticks_after between 1 and {MAX_SCENARIO_TICKS_AFTER}",
                        scenario.id
                    ));
                }
            }
        }
        Ok(())
    }
}

impl GauntletRoleModel {
    fn validate(&self, role: &str) -> Result<(), String> {
        if !(1..=MAX_MODEL_TIMEOUT_MINUTES).contains(&self.timeout_minutes) {
            return Err(format!(
                "Gauntlet models.{role}.timeout_minutes must be between 1 and {MAX_MODEL_TIMEOUT_MINUTES}"
            ));
        }
        for (field, value) in [
            ("model", self.model.as_deref()),
            ("reasoning_effort", self.reasoning_effort.as_deref()),
        ] {
            if let Some(value) = value {
                if value.trim().is_empty()
                    || value.len() > 128
                    || !value.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric()
                            || matches!(byte, b'-' | b'_' | b'.' | b':' | b'/')
                    })
                {
                    return Err(format!(
                        "Gauntlet models.{role}.{field} contains an invalid model setting"
                    ));
                }
            }
        }
        Ok(())
    }
}

pub(super) fn execute(
    command: GauntletCommand,
    workspace_arg: Option<&Path>,
    json_output: bool,
) -> Result<CommandResult, String> {
    match command {
        GauntletCommand::New {
            name,
            dir,
            goal_file,
            reference,
            discover_references,
            tui,
            jsonl,
            max_hours,
            max_model_calls,
        } => create_and_start(NewOptions {
            name,
            dir,
            goal_file,
            references: reference,
            discover_references,
            observer: selected_observer(tui, jsonl),
            max_hours,
            max_model_calls,
        }),
        GauntletCommand::Run {
            config,
            reference,
            discover_references,
            tui,
            jsonl,
            max_hours,
            max_model_calls,
        } => {
            let workspace = load_workspace_with_vendor_gate(workspace_arg, VendorGate::Inspect)?;
            start_existing(
                &workspace,
                &config,
                &reference,
                discover_references,
                selected_observer(tui, jsonl),
                max_hours,
                max_model_calls,
            )
        }
        GauntletCommand::Resume { run_id, tui, jsonl } => {
            let workspace = load_workspace_with_vendor_gate(workspace_arg, VendorGate::Inspect)?;
            resume(&workspace, &run_id, selected_observer(tui, jsonl))
        }
        GauntletCommand::Status { run_id } => {
            let workspace = load_workspace_with_vendor_gate(workspace_arg, VendorGate::Inspect)?;
            status(&workspace, &run_id)
        }
        GauntletCommand::Stop { run_id } => {
            let workspace = load_workspace_with_vendor_gate(workspace_arg, VendorGate::Inspect)?;
            stop(&workspace, &run_id)
        }
        GauntletCommand::Promote { run_id } => {
            let workspace = load_workspace_with_vendor_gate(workspace_arg, VendorGate::Inspect)?;
            promote(&workspace, &run_id, json_output)
        }
    }
}

struct NewOptions {
    name: String,
    dir: PathBuf,
    goal_file: PathBuf,
    references: Vec<PathBuf>,
    discover_references: bool,
    observer: GauntletObserver,
    max_hours: u32,
    max_model_calls: u32,
}

fn create_and_start(options: NewOptions) -> Result<CommandResult, String> {
    if options.max_hours == 0 || options.max_model_calls == 0 {
        return Err("Gauntlet budgets must be greater than zero".to_string());
    }
    let goal = read_bounded_utf8(&options.goal_file, MAX_GOAL_BYTES, "Gauntlet goal")?;
    if goal.trim().is_empty() {
        return Err("Gauntlet goal must not be empty".to_string());
    }
    let root = absolute_path(&options.dir)?;
    create_new_project(root.clone(), options.name)?;
    write_graphical_seed(&root)?;
    fs::write(root.join(GAUNTLET_GOAL_NAME), normalized_text_file(&goal))
        .map_err(|error| format!("failed writing Gauntlet goal: {error}"))?;
    let references = import_references(&root, &options.references)?;
    let config = GauntletConfigV1::new(
        options.discover_references || references.is_empty(),
        references,
        options.max_hours,
        options.max_model_calls,
        options.observer,
    )?;
    write_json(&root.join(GAUNTLET_CONFIG_NAME), &config)?;
    let workspace = load_workspace(Some(&root))?;
    start_existing(
        &workspace,
        Path::new(GAUNTLET_CONFIG_NAME),
        &[],
        false,
        config.execution.observer.clone(),
        None,
        None,
    )
}

fn selected_observer(tui: bool, jsonl: bool) -> GauntletObserver {
    if tui {
        GauntletObserver::Tui
    } else if jsonl {
        GauntletObserver::Jsonl
    } else {
        GauntletObserver::Auto
    }
}

fn write_graphical_seed(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root.join("assets"))
        .map_err(|error| format!("failed creating Gauntlet assets: {error}"))?;
    fs::write(root.join("src/main.stasis"), GAUNTLET_SEED_SOURCE)
        .map_err(|error| format!("failed writing Gauntlet seed source: {error}"))?;
    fs::write(root.join("tests/main.test.stasis"), GAUNTLET_SEED_TEST)
        .map_err(|error| format!("failed writing Gauntlet seed test: {error}"))?;
    fs::write(root.join("assets/gauntlet-ui.ttf"), GAUNTLET_UI_FONT_BYTES)
        .map_err(|error| format!("failed writing Gauntlet UI font: {error}"))?;
    fs::write(
        root.join("assets/gauntlet-ui-OFL.txt"),
        GAUNTLET_UI_FONT_LICENSE,
    )
    .map_err(|error| format!("failed writing Gauntlet UI font license: {error}"))?;
    let manifest = GAUNTLET_ASSET_MANIFEST.replace(
        "__GAUNTLET_UI_FONT_SHA256__",
        &hex_sha256(GAUNTLET_UI_FONT_BYTES),
    );
    fs::write(root.join("assets/manifest.json"), manifest)
        .map_err(|error| format!("failed writing Gauntlet asset manifest: {error}"))?;
    Ok(())
}

fn import_references(root: &Path, paths: &[PathBuf]) -> Result<Vec<GauntletReference>, String> {
    let destination = root.join("build/gauntlet/bootstrap-references");
    let mut imported = Vec::with_capacity(paths.len());
    for (index, path) in paths.iter().enumerate() {
        let absolute = absolute_path(path)?;
        let bytes = read_bounded_bytes(&absolute, MAX_REFERENCE_BYTES, "Gauntlet reference")?;
        let extension = image_extension(&absolute)?;
        fs::create_dir_all(&destination)
            .map_err(|error| format!("failed creating Gauntlet reference directory: {error}"))?;
        let hash = hex_sha256(&bytes);
        let relative = format!(
            "build/gauntlet/bootstrap-references/reference-{index}-{}.{}",
            &hash[..12],
            extension
        );
        fs::write(root.join(&relative), bytes)
            .map_err(|error| format!("failed copying Gauntlet reference: {error}"))?;
        imported.push(GauntletReference {
            path: relative,
            sha256: hash,
            source_url: None,
        });
    }
    Ok(imported)
}

fn image_extension(path: &Path) -> Result<&'static str, String> {
    match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => Ok("png"),
        Some("jpg") | Some("jpeg") => Ok("jpg"),
        Some("webp") => Ok("webp"),
        _ => Err(format!(
            "Gauntlet reference must be PNG, JPEG, or WebP: {}",
            path.display()
        )),
    }
}

fn read_bounded_utf8(path: &Path, limit: u64, label: &str) -> Result<String, String> {
    let bytes = read_bounded_bytes(path, limit, label)?;
    String::from_utf8(bytes).map_err(|_| format!("{label} is not valid UTF-8"))
}

fn read_bounded_bytes(path: &Path, limit: u64, label: &str) -> Result<Vec<u8>, String> {
    let metadata = fs::metadata(path)
        .map_err(|error| format!("failed reading {label} {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.len() > limit {
        return Err(format!(
            "{label} must be a regular file no larger than {limit} bytes: {}",
            path.display()
        ));
    }
    let file = fs::File::open(path)
        .map_err(|error| format!("failed opening {label} {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed reading {label} {}: {error}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{label} exceeds {limit} bytes"));
    }
    Ok(bytes)
}

fn write_json(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let mut bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("failed serializing {}: {error}", path.display()))?;
    bytes.push(b'\n');
    atomic_write_bytes(path, &bytes)
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("failed committing {}: {error}", path.display()))
}

fn normalized_text_file(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    format!("{}\n", normalized.trim_end())
}

fn validate_relative_path(field: &str, path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty() || path.is_absolute() {
        return Err(format!("{field} must be a non-empty project-relative path"));
    }
    if path
        .components()
        .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(format!(
            "{field} must not contain parent or rooted components"
        ));
    }
    Ok(())
}

fn validate_sha256(value: &str) -> Result<(), String> {
    if value.len() != 64 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("Gauntlet reference sha256 must contain 64 hexadecimal characters".to_string());
    }
    Ok(())
}

fn hex_sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

const GAUNTLET_SEED_SOURCE: &str = r#"import "/vendor/stasis/stdlib/stdlib.stasis";
import "/vendor/stasis/stdlib/graphics.stasis";
import "/vendor/stasis/stdlib/audio.stasis";
import "/vendor/stasis/stdlib/collision.stasis";
import "/vendor/stasis/stdlib/flex_layout.stasis";
import "/vendor/stasis/stdlib/frame_timer.stasis";
import "/vendor/stasis/stdlib/hud_table.stasis";
import "/vendor/stasis/stdlib/sdl_scancodes.stasis";
import "/vendor/stasis/stdlib/storage.stasis";
import "/vendor/stasis/stdlib/ui_axis_layout.stasis";
import "/vendor/stasis/stdlib/ui_layout_audit.stasis";
import "/vendor/stasis/stdlib/ui_button_9slice.stasis";

struct Game {
    ticks: i32;
    swaps: i32;
}

global game: Game;
global gauntlet_ui_font: i32;

function main(): i32 {
    init_window(960, 540, "Stasis Gauntlet");
    gauntlet_ui_font = load_font("assets/gauntlet-ui.ttf", 20);
    game.ticks = 0;
    game.swaps = 0;
    return 0;
}

function tick(): i32 {
    game.ticks += 1;
    return 0;
}

function render(): i32 {
    begin_frame();
    clear(0.025, 0.035, 0.06, 1.0);
    draw_line(320.0, 270.0, 640.0, 270.0, 0.18, 0.72, 0.92, 1.0);
    draw_line(480.0, 190.0, 480.0, 350.0, 0.18, 0.72, 0.92, 1.0);
    draw_text(gauntlet_ui_font, "STASIS GAUNTLET", 392.0, 390.0, 0.86, 0.92, 1.0, 1.0);
    end_frame();
    return 0;
}

function on_code_swap(): void {
    game.swaps += 1;
    return;
}
"#;

const GAUNTLET_SEED_TEST: &str = r#"import "../src/main.stasis";

test `Gauntlet seed advances deterministically`(): bool {
    game.ticks = 0;
    tick();
    tick();
    return game.ticks == 2;
}

test `Gauntlet seed emits a visible frame`(): bool {
    return render() == 0;
}
"#;

const GAUNTLET_ASSET_MANIFEST: &str = r#"{
  "schema": "stasis-assets",
  "version": 2,
  "display": {
    "logical_width": 960,
    "logical_height": 540,
    "max_physical_width": 1920,
    "max_physical_height": 1080,
    "scale_mode": "fit"
  },
  "assets": [
    {
      "id": "gauntlet_ui_font",
      "path": "assets/gauntlet-ui.ttf",
      "content_sha256": "__GAUNTLET_UI_FONT_SHA256__",
      "format": {"kind": "font", "encoding": "ttf"},
      "dependencies": []
    }
  ]
}
"#;

fn start_existing(
    workspace: &Workspace,
    config_path: &Path,
    references: &[PathBuf],
    discover_references: bool,
    observer: GauntletObserver,
    max_hours: Option<u32>,
    max_model_calls: Option<u32>,
) -> Result<CommandResult, String> {
    controller::start(
        workspace,
        config_path,
        references,
        discover_references,
        observer,
        max_hours,
        max_model_calls,
    )
}

fn resume(
    workspace: &Workspace,
    run_id: &str,
    observer: GauntletObserver,
) -> Result<CommandResult, String> {
    controller::resume(workspace, run_id, observer)
}

fn status(workspace: &Workspace, run_id: &str) -> Result<CommandResult, String> {
    controller::status(workspace, run_id)
}

fn stop(workspace: &Workspace, run_id: &str) -> Result<CommandResult, String> {
    controller::stop(workspace, run_id)
}

fn promote(
    workspace: &Workspace,
    run_id: &str,
    json_output: bool,
) -> Result<CommandResult, String> {
    controller::promote(workspace, run_id, json_output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::process::Command;
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn config_defaults_match_the_product_contract() {
        let config = GauntletConfigV1::new(
            true,
            Vec::new(),
            8,
            DEFAULT_MODEL_CALLS,
            GauntletObserver::Auto,
        )
        .expect("config");
        assert_eq!(config.budget.wall_time_minutes, 8 * 60);
        assert_eq!(config.budget.model_calls, 100);
        assert_eq!(config.budget.stalled_candidates, 5);
        assert_eq!(config.execution.autonomy, GauntletAutonomy::Full);
        assert_eq!(config.execution.isolation, GauntletIsolation::InPlace);
        assert_eq!(config.execution.builder_max_turns, 30);
        assert!(config.execution.compaction.enabled);
        assert_eq!(
            config.execution.compaction.max_request_bytes,
            2 * 1024 * 1024
        );
        assert_eq!(config.execution.compaction.retain_recent_turns, 6);
        assert_eq!(config.models.scout.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(config.models.scout.reasoning_effort.as_deref(), Some("max"));
        assert_eq!(config.models.scout.timeout_minutes, 30);
        assert_eq!(config.models.builder.model.as_deref(), Some("gpt-5.6-luna"));
        assert_eq!(
            config.models.builder.reasoning_effort.as_deref(),
            Some("max")
        );
        let escalation = config
            .models
            .builder_escalation
            .as_ref()
            .expect("default builder escalation");
        assert_eq!(escalation.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(escalation.reasoning_effort.as_deref(), Some("high"));
        let controller_escalation = config
            .models
            .controller_escalation
            .as_ref()
            .expect("default controller escalation");
        assert_eq!(controller_escalation.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(
            controller_escalation.reasoning_effort.as_deref(),
            Some("high")
        );
        assert_eq!(controller_escalation.timeout_minutes, 30);
        assert!(config.models.lead.model.is_none());
        assert!(config.models.visual_critic.model.is_none());
        assert!(config.models.gameplay_critic.model.is_none());

        let mut value = serde_json::to_value(&config).expect("config value");
        value["models"]["builder_escalation"] = serde_json::Value::Null;
        let without_escalation: GauntletConfigV1 =
            serde_json::from_value(value).expect("disabled escalation");
        assert!(without_escalation.models.builder_escalation.is_none());
    }

    #[test]
    fn gauntlet_vendor_sync_becomes_a_clean_setup_checkpoint() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_vendor_sync_{}_{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        create_new_project(root.clone(), "vendor_sync".to_string()).expect("create project");
        let git = |args: &[&str]| {
            let output = Command::new("git")
                .args(args)
                .current_dir(&root)
                .output()
                .expect("run git");
            assert!(
                output.status.success(),
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr)
            );
            String::from_utf8_lossy(&output.stdout).trim().to_string()
        };
        git(&["init", "--quiet"]);
        git(&["add", "--all"]);
        git(&[
            "-c",
            "user.name=Gauntlet Test",
            "-c",
            "user.email=gauntlet-test@stasis.local",
            "commit",
            "--no-verify",
            "--quiet",
            "-m",
            "initial",
        ]);

        let manifest_path = root.join("stasis.json");
        let mut manifest: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read generated manifest"))
                .expect("parse generated manifest");
        let older_source = root.join("vendor/stasis/stdlib/audio.stasis");
        let mut older_contents = fs::read_to_string(&older_source).expect("read vendor source");
        older_contents.push_str("// older clean snapshot\r\n");
        fs::write(&older_source, older_contents).expect("write older vendor source");
        manifest["vendor"]["stasis"]["release_id"] = Value::String("older-toolchain".into());
        manifest["vendor"]["stasis"]["sha256"] = Value::String(
            super::super::directory_sha256(&root.join("vendor/stasis"))
                .expect("hash older vendor snapshot"),
        );
        write_json(&manifest_path, &manifest).expect("record older toolchain");
        git(&["add", "stasis.json", "vendor/stasis/stdlib/audio.stasis"]);
        git(&[
            "-c",
            "user.name=Gauntlet Test",
            "-c",
            "user.email=gauntlet-test@stasis.local",
            "commit",
            "--no-verify",
            "--quiet",
            "-m",
            "record older vendor",
        ]);

        let workspace = load_workspace_with_vendor_gate(Some(&root), VendorGate::Inspect)
            .expect("inspect without mutating vendor");
        assert!(git(&["status", "--porcelain"]).is_empty());
        controller::sync_vendor_checkpoint(&workspace).expect("checkpoint vendor sync");

        assert!(git(&["status", "--porcelain"]).is_empty());
        assert_eq!(
            git(&["log", "-1", "--format=%s"]),
            "chore: sync Stasis vendor"
        );
        let updated: Value =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read synchronized manifest"))
                .expect("parse synchronized manifest");
        assert_eq!(
            updated["vendor"]["stasis"]["release_id"],
            super::super::current_release_id()
        );
        fs::remove_dir_all(root).expect("remove vendor sync fixture");
    }

    #[test]
    fn config_rejects_unknown_fields_and_zero_budgets() {
        let source = r#"{
            "schema_version":1,
            "goal_file":"GAUNTLET_GOAL.md",
            "quality_bar":{"allow_web_discovery":true,"references":[],"required_scenarios":[]},
            "budget":{"wall_time_minutes":0,"model_calls":1,"stalled_candidates":1},
            "execution":{"autonomy":"full","observer":"auto"},
            "surprise":true
        }"#;
        assert!(serde_json::from_str::<GauntletConfigV1>(source).is_err());

        let mut config = GauntletConfigV1::new(true, Vec::new(), 8, 100, GauntletObserver::Auto)
            .expect("config");
        config.budget.model_calls = 0;
        assert!(config.validate().is_err());
        config.budget.model_calls = 100;
        config.execution.builder_max_turns = 49;
        assert!(config.validate().is_err());
        config.execution.builder_max_turns = 30;
        config.execution.compaction.max_request_bytes = 1;
        assert!(config.validate().is_err());
        config.execution.compaction = GauntletCompaction::default();
        config.models.scout.model = Some("bad model with spaces".to_string());
        assert!(config.validate().is_err());
        config.models.scout.model = Some("gpt-5.6-luna".to_string());
        config.models.scout.timeout_minutes = 0;
        assert!(config.validate().is_err());
    }

    #[test]
    fn config_validates_deterministic_capture_scenarios() {
        let mut config = GauntletConfigV1::new(true, Vec::new(), 8, 100, GauntletObserver::Auto)
            .expect("config");
        config.quality_bar.required_scenarios = vec![GauntletScenarioRequirement {
            id: "select-unit".to_string(),
            description: "Select a ready friendly unit".to_string(),
            taps: vec![GauntletScenarioTap {
                x: 150,
                y: 382,
                ticks_after: 2,
            }],
        }];
        assert!(config.validate().is_ok());

        config
            .quality_bar
            .required_scenarios
            .push(config.quality_bar.required_scenarios[0].clone());
        assert!(config
            .validate()
            .expect_err("duplicate scenario")
            .contains("unique"));
        config.quality_bar.required_scenarios.pop();

        config.quality_bar.required_scenarios[0].taps[0].ticks_after = 0;
        assert!(config
            .validate()
            .expect_err("zero ticks")
            .contains("ticks_after"));
        config.quality_bar.required_scenarios[0].taps[0].ticks_after = 1;
        config.quality_bar.required_scenarios[0].taps[0].x = -1;
        assert!(config
            .validate()
            .expect_err("negative coordinate")
            .contains("coordinates"));
        config.quality_bar.required_scenarios[0].taps[0].x = 150;
        config.quality_bar.required_scenarios[0].id = "../select".to_string();
        assert!(config
            .validate()
            .expect_err("nonportable id")
            .contains("portable ASCII"));
    }

    #[test]
    fn seed_has_the_required_graphical_lifecycle() {
        for required in [
            "/vendor/stasis/stdlib/stdlib.stasis",
            "/vendor/stasis/stdlib/graphics.stasis",
            "/vendor/stasis/stdlib/audio.stasis",
            "/vendor/stasis/stdlib/collision.stasis",
            "/vendor/stasis/stdlib/flex_layout.stasis",
            "/vendor/stasis/stdlib/frame_timer.stasis",
            "/vendor/stasis/stdlib/hud_table.stasis",
            "/vendor/stasis/stdlib/sdl_scancodes.stasis",
            "/vendor/stasis/stdlib/storage.stasis",
            "/vendor/stasis/stdlib/ui_axis_layout.stasis",
            "/vendor/stasis/stdlib/ui_layout_audit.stasis",
            "/vendor/stasis/stdlib/ui_button_9slice.stasis",
            "function main(): i32",
            "function tick(): i32",
            "function render(): i32",
            "function on_code_swap(): void",
            "init_window(960, 540, \"Stasis Gauntlet\")",
            "end_frame()",
        ] {
            assert!(GAUNTLET_SEED_SOURCE.contains(required), "{required}");
        }
        assert!(!GAUNTLET_SEED_SOURCE.contains("host_req_flags"));
        assert!(!GAUNTLET_SEED_SOURCE.contains("host_req_seq"));
        assert!(GAUNTLET_SEED_TEST.contains("return render() == 0"));
        assert!(!GAUNTLET_SEED_TEST.contains("gfx_cmd_"));
    }

    #[test]
    fn graphical_seed_compiles_and_its_tests_execute_through_jit() {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_seed_{}_{}",
            std::process::id(),
            stamp
        ));
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = fs::remove_dir_all(&self.0);
            }
        }
        let _cleanup = Cleanup(root.clone());
        create_new_project(root.clone(), "gauntlet_seed_test".to_string())
            .expect("create seed project");
        write_graphical_seed(&root).expect("write graphical seed");
        assert_eq!(
            fs::read(root.join("assets/gauntlet-ui.ttf")).expect("read seeded font"),
            GAUNTLET_UI_FONT_BYTES
        );
        assert!(fs::read_to_string(root.join("assets/gauntlet-ui-OFL.txt"))
            .expect("read seeded font license")
            .contains("SIL OPEN FONT LICENSE Version 1.1"));
        let manifest =
            fs::read_to_string(root.join("assets/manifest.json")).expect("read seeded manifest");
        assert!(manifest.contains(&hex_sha256(GAUNTLET_UI_FONT_BYTES)));
        assert!(manifest.contains("assets/gauntlet-ui.ttf"));
        let workspace = load_workspace(Some(&root)).expect("load seed workspace");
        super::super::check_workspace(&workspace).expect("seed compiles through JIT");
        super::super::test_workspace(&workspace, None).expect("seed tests execute through JIT");
    }
}
