use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stasis::{
    run_jit_tests_in_directory_with_session, run_live_in_process, run_play_in_process,
    run_self_host_aot_cli_with_options, LiveRunConfig, StasisTestRunSession,
};
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::frontend::workshop::{
    find_workshop_references, find_workshop_symbols, load_workshop_edit_workspace,
    plan_workshop_semantic_edits, workshop_direct_import_files, workshop_reachable_files,
    workshop_source_hash, workshop_source_items, write_workshop_semantic_plan,
    write_workshop_semantic_receipt, WorkshopSemanticEdit, WorkshopSemanticEditBatch,
    WorkshopSemanticEditOperation, WorkshopSemanticEditPlan, WorkshopSourceFile,
    WorkshopSourceItemKind, WorkshopSymbolSelector,
};
pub(super) use stasis_runner::live::LiveValidationRequirement as RuntimeValidationRequirement;
use stasis_runner::live::{
    compare_live_validation_values, live_session, LiveCommand, LiveRequest, LiveResponse,
    TerminalBuffer, TerminalInput,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

mod live_tui;

const MANIFEST_NAME: &str = "stasis.json";
const MANIFEST_VERSION: u32 = 1;
const RELEASE_PROVENANCE_NAME: &str = "stasis_release_provenance.json";
const PACKAGE_PROVENANCE_NAME: &str = "stasis_provenance.json";
const MOBILE_RUNTIME_FILES: &[&str] = &[
    "CMakeLists.txt",
    "nanosvg.h",
    "nanosvgrast.h",
    "stasis_display_scale.h",
    "stasis_asset_path.h",
    "stasis_render_contract.h",
    "stasis_renderer_lifecycle.h",
    "stasis_graphics.c",
    "stasis_mobile_aot_runtime.c",
    "stasis_mobile_aot_runtime.h",
    "stasis_mobile_runtime.c",
    "stasis_mobile_runtime.h",
    "stasis_platform_storage.c",
    "stasis_platform_storage.h",
    "stb_truetype.h",
];
const PROJECT_AGENT_GUIDE: &str = include_str!("../../../docs/agent_workflow.md");
const PROJECT_CLAUDE_GUIDE: &str = "# CLAUDE.md\n\n@AGENTS.md\n";
const COMMANDS: &[&str] = &[
    "new",
    "init",
    "fmt",
    "check",
    "test",
    "ai",
    "validate",
    "run",
    "build",
    "package",
    "package-mobile",
    "inspect",
    "replay",
    "verify",
    "version",
    "env",
    "symbol",
    "help",
    "__validate-runtime",
];

#[derive(Debug, Parser)]
#[command(
    name = "stasis",
    version,
    about = "The batteries-included Stasis toolchain",
    long_about = "Create, format, check, test, run, build, inspect, and package Stasis projects without invoking Cargo."
)]
struct ToolchainCli {
    #[arg(
        long,
        global = true,
        help = "Emit one deterministic JSON result object"
    )]
    json: bool,

    #[arg(
        long,
        global = true,
        value_name = "PATH",
        help = "Use this project or project directory"
    )]
    workspace: Option<PathBuf>,

    #[command(subcommand)]
    command: ToolchainCommand,
}

#[derive(Debug, Subcommand)]
enum ToolchainCommand {
    /// Create a project in a new directory.
    New {
        name: String,
        #[arg(long, value_name = "PATH")]
        dir: Option<PathBuf>,
    },
    /// Initialize a project in an existing directory.
    Init {
        #[arg(default_value = ".")]
        dir: PathBuf,
        #[arg(long)]
        name: Option<String>,
    },
    /// Format project source files deterministically.
    Fmt {
        #[arg(long)]
        check: bool,
    },
    /// Parse, analyze, and JIT-compile the project without running it.
    Check,
    /// Run project tests in one isolated JIT session.
    Test {
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
    },
    /// Run one subscription-backed AI change against the live workspace.
    Ai {
        #[arg(value_name = "PROMPT")]
        prompt: String,
    },
    /// Boot a fresh isolated runtime and validate one scalar requirement.
    Validate {
        path: String,
        op: String,
        value: String,
        #[arg(long, default_value_t = 0)]
        frames: u32,
        #[arg(long, default_value = "main")]
        setup: String,
        #[arg(long, default_value = "tick")]
        tick: String,
        #[arg(long, default_value = "render")]
        render: String,
    },
    #[command(name = "__validate-runtime", hide = true)]
    ValidateRuntime {
        #[arg(long, default_value_t = 0)]
        frames: u32,
        #[arg(long)]
        requirements_json: String,
        #[arg(long, default_value = "main")]
        setup: String,
        #[arg(long, default_value = "tick")]
        tick: String,
        #[arg(long, default_value = "render")]
        render: String,
    },
    /// JIT-compile and run main() in the headless toolchain runtime.
    Run {
        /// Recompile and rerun when project .stasis files change.
        #[arg(long)]
        watch: bool,
        /// Explicitly select the headless runtime (currently the default).
        #[arg(long)]
        headless: bool,
        /// Keep the graphical game running while accepting live workspace commands.
        #[arg(long, conflicts_with_all = ["watch", "headless"])]
        interactive: bool,
        /// Read interactive commands from a deterministic script instead of stdin.
        #[arg(long, value_name = "PATH", requires = "interactive")]
        live_script: Option<PathBuf>,
        /// Emit versioned live response envelopes as JSON lines.
        #[arg(long, requires = "interactive")]
        live_json: bool,
    },
    /// Build the project for development or as a release executable.
    Build {
        #[arg(long, value_enum, default_value_t = BuildMode::Release)]
        mode: BuildMode,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
    },
    /// Assemble a distributable desktop or mobile directory.
    Package {
        #[arg(long, value_enum, default_value_t = PackageTarget::Desktop)]
        target: PackageTarget,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Permit a visibly labeled package from a local/source toolchain.
        #[arg(long)]
        development_build: bool,
    },
    /// Assemble a release-only Android or iOS app project around one AOT game.
    PackageMobile {
        #[arg(long, value_enum)]
        target: MobilePackageTarget,
        #[arg(long, value_name = "PATH")]
        entry: Option<PathBuf>,
        #[arg(long, value_name = "PATH")]
        out: Option<PathBuf>,
        /// Permit a visibly labeled package from a local/source toolchain.
        #[arg(long)]
        development_build: bool,
    },
    /// Report compiler-owned state memory, layout, and mobile budget information.
    Inspect {
        /// Project the byte impact of a collection capacity change (PATH=COUNT).
        #[arg(long = "capacity", value_name = "PATH=COUNT")]
        capacities: Vec<String>,
        /// Mobile snapshot budget used for deterministic warnings.
        #[arg(long, default_value_t = MAX_STATE_SNAPSHOT_BYTES as u64)]
        mobile_budget_bytes: u64,
    },
    /// Replay support is reserved until the replay runtime lands.
    Replay,
    /// Replay verification is reserved until the replay runtime lands.
    Verify,
    /// Print the installed toolchain version.
    Version,
    /// Print toolchain, workspace, cache, and offline capability information.
    Env,
    /// Find and transactionally edit compiler-owned semantic symbols.
    Symbol {
        #[command(subcommand)]
        command: SymbolCommand,
    },
}

#[derive(Debug, Subcommand)]
enum SymbolCommand {
    /// List symbols in deterministic source order.
    List {
        #[arg(long)]
        query: Option<String>,
        #[arg(long, value_enum)]
        kind: Option<SymbolKindArg>,
        #[arg(long)]
        file: Vec<String>,
        #[arg(long)]
        owner: Option<String>,
        #[arg(long, default_value_t = 0)]
        page: usize,
        #[arg(long, default_value_t = 32)]
        limit: usize,
    },
    /// Find symbol metadata without returning its source.
    Find(SymbolSelectorArgs),
    /// Read one unambiguous symbol and its source.
    Read(SymbolSelectorArgs),
    /// Find compact definitions, reads, writes, and calls for a symbol or field.
    References {
        symbol: String,
        #[arg(long, default_value_t = 128)]
        limit: usize,
    },
    /// Add one declaration to an existing imported file.
    Add {
        #[command(flatten)]
        target: RequiredSymbolTargetArgs,
        #[arg(
            long,
            value_name = "SOURCE",
            conflicts_with = "source_file",
            required_unless_present = "source_file"
        )]
        source: Option<String>,
        #[arg(long, value_name = "PATH", conflicts_with = "source")]
        source_file: Option<PathBuf>,
        #[command(flatten)]
        options: SymbolEditOptions,
    },
    /// Replace one existing declaration.
    Update {
        #[command(flatten)]
        target: SymbolSelectorArgs,
        #[arg(
            long,
            value_name = "SOURCE",
            conflicts_with = "source_file",
            required_unless_present = "source_file"
        )]
        source: Option<String>,
        #[arg(long, value_name = "PATH", conflicts_with = "source")]
        source_file: Option<PathBuf>,
        #[arg(long)]
        expected_source_hash: Option<String>,
        #[command(flatten)]
        options: SymbolEditOptions,
    },
    /// Delete one existing declaration.
    Delete {
        #[command(flatten)]
        target: SymbolSelectorArgs,
        #[arg(long)]
        expected_source_hash: Option<String>,
        #[command(flatten)]
        options: SymbolEditOptions,
    },
    /// Preview or apply a shared semantic-edit request JSON file.
    Apply {
        #[arg(long, value_name = "PATH")]
        request: PathBuf,
        #[command(flatten)]
        options: SymbolEditOptions,
    },
    /// Revert a previously applied semantic-edit receipt.
    Revert {
        #[arg(long, value_name = "PATH")]
        receipt: PathBuf,
        #[arg(long)]
        dry_run: bool,
        #[arg(long)]
        no_tests: bool,
    },
}

#[derive(Debug, Clone, Args)]
struct SymbolSelectorArgs {
    name: String,
    #[arg(long, value_enum)]
    kind: Option<SymbolKindArg>,
    #[arg(long)]
    file: Option<String>,
    #[arg(long)]
    owner: Option<String>,
    #[arg(long)]
    signature: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct RequiredSymbolTargetArgs {
    name: String,
    #[arg(long, value_enum)]
    kind: SymbolKindArg,
    #[arg(long)]
    file: String,
    #[arg(long)]
    owner: Option<String>,
    #[arg(long)]
    signature: Option<String>,
}

#[derive(Debug, Clone, Args)]
struct SymbolEditOptions {
    /// Validate and report the edit without writing files.
    #[arg(long)]
    dry_run: bool,
    /// Skip project tests after compiler validation.
    #[arg(long)]
    no_tests: bool,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SymbolKindArg {
    Imports,
    Globals,
    Struct,
    Function,
    Test,
}

impl From<SymbolKindArg> for WorkshopSourceItemKind {
    fn from(value: SymbolKindArg) -> Self {
        match value {
            SymbolKindArg::Imports => Self::Imports,
            SymbolKindArg::Globals => Self::Globals,
            SymbolKindArg::Struct => Self::Struct,
            SymbolKindArg::Function => Self::Function,
            SymbolKindArg::Test => Self::Test,
        }
    }
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum BuildMode {
    Dev,
    Release,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PackageTarget {
    Desktop,
    AndroidArm64,
    IosArm64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MobilePackageTarget {
    AndroidArm64,
    IosArm64,
}

impl MobilePackageTarget {
    fn package_target(self) -> PackageTarget {
        match self {
            Self::AndroidArm64 => PackageTarget::AndroidArm64,
            Self::IosArm64 => PackageTarget::IosArm64,
        }
    }
}

impl PackageTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::AndroidArm64 => "android-arm64",
            Self::IosArm64 => "ios-arm64",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ProjectManifest {
    manifest_version: u32,
    name: String,
    entry: String,
    tests: String,
    output: String,
}

impl ProjectManifest {
    fn new(name: String) -> Self {
        Self {
            manifest_version: MANIFEST_VERSION,
            name,
            entry: "src/main.stasis".to_string(),
            tests: "tests".to_string(),
            output: "build".to_string(),
        }
    }

    fn validate(&self) -> Result<(), String> {
        if self.manifest_version != MANIFEST_VERSION {
            return Err(format!(
                "unsupported manifest_version {}; expected {}",
                self.manifest_version, MANIFEST_VERSION
            ));
        }
        validate_project_name(&self.name)?;
        for (field, value) in [
            ("entry", self.entry.as_str()),
            ("tests", self.tests.as_str()),
            ("output", self.output.as_str()),
        ] {
            validate_relative_path(field, Path::new(value))?;
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
struct Workspace {
    root: PathBuf,
    manifest: ProjectManifest,
}

#[derive(Debug)]
struct CommandResult {
    code: i32,
    human: String,
    data: Value,
}

impl CommandResult {
    fn success(human: impl Into<String>, data: Value) -> Self {
        Self {
            code: 0,
            human: human.into(),
            data,
        }
    }
}

pub(super) fn try_run() -> Option<i32> {
    let args: Vec<OsString> = env::args_os().collect();
    if !is_toolchain_invocation(&args) {
        return None;
    }
    let wants_json = args.iter().any(|arg| arg == "--json");
    let wants_version = args.iter().any(|arg| arg == "--version" || arg == "-V");
    let parsed = match ToolchainCli::try_parse_from(args) {
        Ok(parsed) => parsed,
        Err(error) => {
            let exit_code = error.exit_code();
            if wants_json {
                if exit_code == 0 {
                    if wants_version {
                        println!(
                            "{}",
                            json!({
                                "ok": true,
                                "command": "version",
                                "result": {"version": env!("CARGO_PKG_VERSION")},
                            })
                        );
                    } else {
                        println!("{}", json!({"ok": true, "commands": COMMANDS}));
                    }
                } else {
                    eprintln!(
                        "{}",
                        json!({
                            "ok": false,
                            "code": "usage_error",
                            "message": error.to_string(),
                        })
                    );
                }
            } else {
                let _ = error.print();
            }
            return Some(exit_code);
        }
    };
    let command_name = command_name(&parsed.command);
    match execute(parsed.command, parsed.workspace, parsed.json) {
        Ok(result) => {
            if parsed.json {
                println!(
                    "{}",
                    json!({
                        "ok": true,
                        "command": command_name,
                        "result": result.data,
                    })
                );
            } else if !result.human.is_empty() {
                println!("{}", result.human);
            }
            Some(result.code)
        }
        Err(message) => {
            if parsed.json {
                eprintln!(
                    "{}",
                    json!({
                        "ok": false,
                        "command": command_name,
                        "code": "command_failed",
                        "message": message,
                    })
                );
            } else {
                eprintln!("stasis {command_name}: {message}");
            }
            Some(1)
        }
    }
}

fn is_toolchain_invocation(args: &[OsString]) -> bool {
    let Some(first) = args.get(1).and_then(|arg| arg.to_str()) else {
        return true;
    };
    first == "--help"
        || first == "-h"
        || first == "--version"
        || first == "-V"
        || first == "--json"
        || first == "--workspace"
        || COMMANDS.contains(&first)
}

fn command_name(command: &ToolchainCommand) -> &'static str {
    match command {
        ToolchainCommand::New { .. } => "new",
        ToolchainCommand::Init { .. } => "init",
        ToolchainCommand::Fmt { .. } => "fmt",
        ToolchainCommand::Check => "check",
        ToolchainCommand::Test { .. } => "test",
        ToolchainCommand::Ai { .. } => "ai",
        ToolchainCommand::Validate { .. } => "validate",
        ToolchainCommand::ValidateRuntime { .. } => "__validate-runtime",
        ToolchainCommand::Run { .. } => "run",
        ToolchainCommand::Build { .. } => "build",
        ToolchainCommand::Package { .. } => "package",
        ToolchainCommand::PackageMobile { .. } => "package-mobile",
        ToolchainCommand::Inspect { .. } => "inspect",
        ToolchainCommand::Replay => "replay",
        ToolchainCommand::Verify => "verify",
        ToolchainCommand::Version => "version",
        ToolchainCommand::Env => "env",
        ToolchainCommand::Symbol { .. } => "symbol",
    }
}

fn execute(
    command: ToolchainCommand,
    workspace_arg: Option<PathBuf>,
    json_output: bool,
) -> Result<CommandResult, String> {
    match command {
        ToolchainCommand::New { name, dir } => create_project(dir.unwrap_or_else(|| PathBuf::from(&name)), name),
        ToolchainCommand::Init { dir, name } => {
            let root = absolute_path(&dir)?;
            let inferred = root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("stasis_game")
                .to_string();
            create_project(root, name.unwrap_or(inferred))
        }
        ToolchainCommand::Version => Ok(version_result()),
        ToolchainCommand::Env => env_result(workspace_arg.as_deref()),
        ToolchainCommand::Replay => Err(
            "replay is unavailable in toolchain 0.1; no replay runtime contract is implemented"
                .to_string(),
        ),
        ToolchainCommand::Verify => Err(
            "verify is unavailable in toolchain 0.1; no replay verification contract is implemented"
                .to_string(),
        ),
        other => {
            let workspace = load_workspace(workspace_arg.as_deref())?;
            match other {
                ToolchainCommand::Fmt { check } => format_workspace(&workspace, check),
                ToolchainCommand::Check => check_workspace(&workspace),
                ToolchainCommand::Test { path } => {
                    validate_optional_workspace_path(&workspace, "test path", path.as_deref())?;
                    test_workspace(&workspace, path.as_deref())
                }
                ToolchainCommand::Ai { prompt } => run_workspace_ai(&workspace, &prompt),
                ToolchainCommand::Validate {
                    path,
                    op,
                    value,
                    frames,
                    setup,
                    tick,
                    render,
                } => validate_runtime_command(
                    &workspace, path, op, value, frames, setup, tick, render,
                ),
                ToolchainCommand::ValidateRuntime {
                    frames,
                    requirements_json,
                    setup,
                    tick,
                    render,
                } => validate_fresh_runtime(
                    &workspace,
                    frames,
                    &requirements_json,
                    &setup,
                    &tick,
                    &render,
                ),
                ToolchainCommand::Run {
                    watch,
                    headless,
                    interactive,
                    live_script,
                    live_json,
                } => {
                    if interactive && json_output {
                        Err("--json cannot be combined with --interactive; use --live-json for the response stream".to_string())
                    } else if interactive {
                        validate_optional_workspace_path(
                            &workspace,
                            "live script",
                            live_script.as_deref(),
                        )?;
                        run_workspace_live(&workspace, live_script.as_deref(), live_json)
                    } else if watch && json_output {
                        Err("--json cannot be combined with --watch; watch mode is an unbounded event stream".to_string())
                    } else if watch && headless {
                        Err("--headless cannot be combined with --watch; watch mode uses the graphical hot-swap runner".to_string())
                    } else if watch {
                        run_workspace_watch(&workspace)
                    } else {
                        run_workspace(&workspace, headless)
                    }
                }
                ToolchainCommand::Build { mode, out } => {
                    validate_optional_workspace_path(&workspace, "build output", out.as_deref())?;
                    build_workspace(&workspace, mode, out.as_deref())
                }
                ToolchainCommand::Package {
                    target,
                    out,
                    development_build,
                } => {
                    validate_optional_workspace_path(&workspace, "package output", out.as_deref())?;
                    package_workspace(&workspace, target, out.as_deref(), development_build)
                }
                ToolchainCommand::PackageMobile {
                    target,
                    entry,
                    out,
                    development_build,
                } => {
                    validate_optional_workspace_path(
                        &workspace,
                        "mobile entry",
                        entry.as_deref(),
                    )?;
                    validate_optional_workspace_path(
                        &workspace,
                        "package output",
                        out.as_deref(),
                    )?;
                    package_mobile_command(
                        &workspace,
                        target,
                        entry.as_deref(),
                        out.as_deref(),
                        development_build,
                    )
                }
                ToolchainCommand::Inspect {
                    capacities,
                    mobile_budget_bytes,
                } => inspect_workspace(&workspace, &capacities, mobile_budget_bytes),
                ToolchainCommand::Symbol { command } => symbol_workspace(&workspace, command),
                _ => Err("unsupported command routing".to_string()),
            }
        }
    }
}

fn create_project(path: PathBuf, name: String) -> Result<CommandResult, String> {
    validate_project_name(&name)?;
    let root = absolute_path(&path)?;
    let bundled_stdlib = bundled_stdlib_dir()?;
    let manifest_path = root.join(MANIFEST_NAME);
    let reserved_paths = [
        manifest_path.clone(),
        root.join("AGENTS.md"),
        root.join("CLAUDE.md"),
        root.join("src/main.stasis"),
        root.join("tests/main.test.stasis"),
        root.join("stdlib"),
    ];
    for reserved in &reserved_paths {
        if reserved.exists() {
            return Err(format!("refusing to overwrite {}", reserved.display()));
        }
    }
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    fs::create_dir_all(root.join("tests"))
        .map_err(|error| format!("failed to create tests directory: {error}"))?;
    let manifest = ProjectManifest::new(name.clone());
    write_manifest(&manifest_path, &manifest)?;
    copy_dir_if_exists(&bundled_stdlib, &root.join("stdlib"))?;
    write_new_file(&root.join("AGENTS.md"), PROJECT_AGENT_GUIDE)?;
    write_new_file(&root.join("CLAUDE.md"), PROJECT_CLAUDE_GUIDE)?;
    write_new_file(
        &root.join("src/main.stasis"),
        "import \"../stdlib/stdlib.stasis\";\n\nfunction main(): i32 {\n    return 0;\n}\n",
    )?;
    write_new_file(
        &root.join("tests/main.test.stasis"),
        "test `new project is ready`(): bool {\n    return 1 == 1;\n}\n",
    )?;
    Ok(CommandResult::success(
        format!("created {} at {}", name, root.display()),
        json!({"name": name, "root": display_path(&root), "manifest": MANIFEST_NAME}),
    ))
}

fn write_new_file(path: &Path, contents: &str) -> Result<(), String> {
    if path.exists() {
        return Err(format!("refusing to overwrite {}", path.display()));
    }
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn write_manifest(path: &Path, manifest: &ProjectManifest) -> Result<(), String> {
    let mut contents = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("failed to serialize manifest: {error}"))?;
    contents.push('\n');
    fs::write(path, contents)
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn load_workspace(explicit: Option<&Path>) -> Result<Workspace, String> {
    if let Some(path) = explicit {
        let absolute = absolute_path(path)?;
        if !absolute.exists() {
            return Err(format!(
                "workspace path does not exist: {}",
                absolute.display()
            ));
        }
    }
    let start = explicit.map(absolute_path).transpose()?.unwrap_or(
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?,
    );
    let start_dir = if start.is_file() {
        start.parent().unwrap_or(&start).to_path_buf()
    } else {
        start
    };
    let root = find_workspace_root(&start_dir).ok_or_else(|| {
        format!(
            "no {MANIFEST_NAME} found from {}; run 'stasis init' first",
            start_dir.display()
        )
    })?;
    let bytes = fs::read(root.join(MANIFEST_NAME))
        .map_err(|error| format!("failed to read {MANIFEST_NAME}: {error}"))?;
    let manifest: ProjectManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {MANIFEST_NAME}: {error}"))?;
    manifest.validate()?;
    Ok(Workspace { root, manifest })
}

fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(MANIFEST_NAME).is_file() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn format_workspace(workspace: &Workspace, check: bool) -> Result<CommandResult, String> {
    let mut files = Vec::new();
    let source_root = workspace.root.join("src");
    let test_root = workspace.root.join(&workspace.manifest.tests);
    validate_workspace_destination(workspace, "source directory", &source_root)?;
    validate_workspace_destination(workspace, "test directory", &test_root)?;
    collect_stasis_files(&source_root, &mut files)?;
    collect_stasis_files(&test_root, &mut files)?;
    files.sort();
    files.dedup();
    let mut changed = Vec::new();
    for file in files {
        let original = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let formatted = format_source(&original);
        if original != formatted {
            changed.push(relative_display(&workspace.root, &file));
            if !check {
                fs::write(&file, formatted)
                    .map_err(|error| format!("failed to write {}: {error}", file.display()))?;
            }
        }
    }
    if check && !changed.is_empty() {
        return Err(format!("formatting required: {}", changed.join(", ")));
    }
    Ok(CommandResult::success(
        if changed.is_empty() {
            "all Stasis files are formatted".to_string()
        } else {
            format!("formatted {} file(s)", changed.len())
        },
        json!({"changed": changed, "check": check}),
    ))
}

fn format_source(source: &str) -> String {
    let normalized = source.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<&str> = normalized.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let mut output = lines
        .iter()
        .map(|line| line.trim_end_matches([' ', '\t']))
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    output
}

fn check_workspace(workspace: &Workspace) -> Result<CommandResult, String> {
    let jit = compile_workspace_jit(workspace)?;
    Ok(CommandResult::success(
        format!("checked {}", workspace.manifest.name),
        json!({
            "name": workspace.manifest.name,
            "entry": workspace.manifest.entry,
            "functions_emitted": jit.artifacts().len(),
        }),
    ))
}

fn compile_workspace_jit(workspace: &Workspace) -> Result<JitProcess, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    validate_workspace_destination(workspace, "entry", &entry)?;
    let files =
        load_workshop_edit_workspace(&workspace.root, Path::new(&workspace.manifest.entry))?;
    let files = workshop_reachable_files(&files, Path::new(&workspace.manifest.entry))?;
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    let mut sources = BTreeMap::new();
    for file in files {
        let path = workspace.root.join(&file.path);
        let path = path.canonicalize().unwrap_or(path);
        let path = path.to_string_lossy().to_string();
        sources.insert(path.clone(), file.source.clone());
        jit.upsert_file(path, file.source);
    }
    jit.compile().map_err(|error| {
        if let Some(diagnostic) = jit.last_source_diagnostic() {
            let source = sources
                .get(&diagnostic.path)
                .map(String::as_str)
                .unwrap_or("");
            let (line, column) = line_column(source, diagnostic.start);
            format!(
                "{}:{}:{}: {}",
                diagnostic.path, line, column, diagnostic.message
            )
        } else {
            format!("{error:?}")
        }
    })?;
    Ok(jit)
}

fn validate_fresh_runtime(
    workspace: &Workspace,
    frames: u32,
    requirements_json: &str,
    setup: &str,
    tick: &str,
    render: &str,
) -> Result<CommandResult, String> {
    if frames > 600 {
        return Err("frames exceeds the 600-frame limit".to_string());
    }
    let requirements = serde_json::from_str::<Vec<RuntimeValidationRequirement>>(requirements_json)
        .map_err(|error| format!("invalid runtime validation requirements: {error}"))?;
    if requirements.is_empty() || requirements.len() > 16 {
        return Err("requirements must contain 1..=16 checks".to_string());
    }
    for (label, name) in [("setup", setup), ("tick", tick), ("render", render)] {
        if name.is_empty()
            || !name.bytes().enumerate().all(|(index, byte)| {
                byte == b'_' || byte.is_ascii_alphabetic() || (index > 0 && byte.is_ascii_digit())
            })
        {
            return Err(format!(
                "fresh validation {label} must be a Stasis identifier"
            ));
        }
    }
    let entry = workspace.root.join(&workspace.manifest.entry);
    validate_workspace_destination(workspace, "entry", &entry)?;
    let source = fs::read_to_string(&entry)
        .map_err(|error| format!("failed to read entry {}: {error}", entry.display()))?;
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&[setup.to_string(), tick.to_string(), render.to_string()]);
    jit.upsert_file(display_path(&entry), source);
    jit.compile()
        .map_err(|error| format!("fresh validation compile failed: {error:?}"))?;
    execute_noarg_entry(&jit, setup)?;
    for _ in 0..frames {
        execute_noarg_entry(&jit, tick)?;
    }
    execute_noarg_entry(&jit, render)?;

    let mut requirements_met = true;
    let mut checks = Vec::with_capacity(requirements.len());
    for requirement in requirements {
        let actual = serde_json::to_value(jit.read_global_scalar(&requirement.path)?)
            .map_err(|error| format!("failed encoding {}: {error}", requirement.path))?
            .get("value")
            .cloned()
            .ok_or_else(|| {
                format!(
                    "inspection for {} returned no scalar value",
                    requirement.path
                )
            })?;
        let passed = compare_live_validation_values(&actual, &requirement.op, &requirement.value)?;
        requirements_met &= passed;
        checks.push(json!({
            "path": requirement.path,
            "op": requirement.op,
            "expected": requirement.value,
            "actual": actual,
            "passed": passed,
        }));
    }
    Ok(CommandResult::success(
        "fresh runtime validation complete",
        json!({
            "baseline": "fresh",
            "entrypoints": {"setup": setup, "tick": tick, "render": render},
            "frames": frames,
            "requirements_met": requirements_met,
            "checks": checks,
        }),
    ))
}

#[allow(clippy::too_many_arguments)]
fn validate_runtime_command(
    workspace: &Workspace,
    path: String,
    op: String,
    value: String,
    frames: u32,
    setup: String,
    tick: String,
    render: String,
) -> Result<CommandResult, String> {
    let expected = serde_json::from_str(&value).unwrap_or(Value::String(value));
    let requirements = serde_json::to_string(&[RuntimeValidationRequirement {
        path: path.clone(),
        op: op.clone(),
        value: expected,
    }])
    .map_err(|error| format!("failed encoding runtime requirement: {error}"))?;
    let result = validate_fresh_runtime(workspace, frames, &requirements, &setup, &tick, &render)?;
    if !result
        .data
        .get("requirements_met")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        let check = result
            .data
            .get("checks")
            .and_then(Value::as_array)
            .and_then(|checks| checks.first())
            .cloned()
            .unwrap_or(Value::Null);
        return Err(format!(
            "runtime validation failed: {path} {op} {}; actual {}",
            scalar_text(check.get("expected").unwrap_or(&Value::Null)),
            scalar_text(check.get("actual").unwrap_or(&Value::Null)),
        ));
    }
    let check = result
        .data
        .get("checks")
        .and_then(Value::as_array)
        .and_then(|checks| checks.first())
        .cloned()
        .unwrap_or(Value::Null);
    Ok(CommandResult::success(
        format!(
            "runtime validation passed: {path} {op} {} (actual {}, {frames} frame(s))",
            scalar_text(check.get("expected").unwrap_or(&Value::Null)),
            scalar_text(check.get("actual").unwrap_or(&Value::Null)),
        ),
        result.data,
    ))
}

fn execute_noarg_entry(jit: &JitProcess, name: &str) -> Result<(), String> {
    match jit.execute_i32_noarg_by_name(name) {
        Ok(_) => Ok(()),
        Err(error) if error.contains("not i32-returning") => jit.execute_void_noarg_by_name(name),
        Err(error) => Err(error),
    }
}

fn test_workspace(workspace: &Workspace, path: Option<&Path>) -> Result<CommandResult, String> {
    let directory = path
        .map(|value| workspace.root.join(value))
        .unwrap_or_else(|| workspace.root.join(&workspace.manifest.tests));
    validate_workspace_destination(workspace, "test directory", &directory)?;
    let mut session = StasisTestRunSession::new();
    let summary = run_jit_tests_in_directory_with_session(&directory, &mut session)?;
    let data = json!({
        "files_discovered": summary.files_discovered,
        "files_with_tests": summary.files_with_tests,
        "tests_discovered": summary.tests_discovered,
        "tests_run": summary.tests_run,
        "tests_passed": summary.tests_passed,
        "tests_failed": summary.tests_failed,
        "failures": summary.failures,
    });
    if summary.tests_failed > 0 {
        return Err(format!(
            "{} test(s) failed: {}",
            summary.tests_failed,
            summary.failures.join(" | ")
        ));
    }
    Ok(CommandResult::success(
        format!(
            "{} test(s) passed in {} file(s)",
            summary.tests_passed, summary.files_with_tests
        ),
        data,
    ))
}

fn run_workspace(workspace: &Workspace, _headless: bool) -> Result<CommandResult, String> {
    let jit = compile_workspace_jit(workspace)?;
    let guest_exit = match jit.execute_i32_noarg_by_name("main") {
        Ok(value) => value,
        Err(error) if error.contains("not i32-returning") => {
            jit.execute_void_noarg_by_name("main")?;
            0
        }
        Err(error) => return Err(error),
    };
    Ok(CommandResult {
        code: guest_exit,
        human: format!("program exited with code {guest_exit}"),
        data: json!({"exit_code": guest_exit, "backend": "jit", "headless": true}),
    })
}

fn run_workspace_watch(workspace: &Workspace) -> Result<CommandResult, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    run_play_in_process(&entry, Some(&workspace.root), None, None, 16_000, None)?;
    Ok(CommandResult::success(
        "graphical watch session ended",
        json!({"backend": "jit", "headless": false, "watch": true}),
    ))
}

fn run_workspace_ai(workspace: &Workspace, prompt: &str) -> Result<CommandResult, String> {
    if prompt.trim().is_empty() {
        return Err("AI prompt must not be empty".to_string());
    }
    let entry = workspace.root.join(&workspace.manifest.entry);
    let (client, server) = live_session(stasis_runner::live::DEFAULT_LIVE_QUEUE_CAPACITY);
    let ai_root = workspace.root.clone();
    let prompt = prompt.to_string();
    let canceled = Arc::new(AtomicBool::new(false));
    let ai_canceled = Arc::clone(&canceled);
    let ai = thread::spawn(move || {
        let result =
            live_tui::run_scripted_ai_with_cancel(&client, &ai_root, &prompt, &ai_canceled);
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        result
    });
    let config = LiveRunConfig::new(
        workspace.root.clone(),
        PathBuf::from(&workspace.manifest.entry),
        PathBuf::from(&workspace.manifest.output),
    );
    let run_result =
        run_live_in_process(&entry, Some(&workspace.root), 16_000, None, server, config);
    if let Err(error) = run_result {
        canceled.store(true, Ordering::Release);
        let _ = ai.join();
        return Err(error);
    }
    let (summary, trace, usage_trace) = ai
        .join()
        .map_err(|_| "live AI thread panicked".to_string())??;
    Ok(CommandResult::success(
        format!(
            "AI complete: {summary}\nAI trace: {}\nAI usage: {}",
            trace.display(),
            usage_trace.display()
        ),
        json!({
            "backend": "jit",
            "provider": "installed_codex_subscription",
            "summary": summary,
            "trace": trace,
            "usage_trace": usage_trace,
        }),
    ))
}

fn run_workspace_live(
    workspace: &Workspace,
    script: Option<&Path>,
    json_lines: bool,
) -> Result<CommandResult, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    let (client, server) = live_session(stasis_runner::live::DEFAULT_LIVE_QUEUE_CAPACITY);
    let script = script.map(|path| workspace.root.join(path));
    let terminal_root = workspace.root.clone();
    let terminal = thread::spawn(move || {
        run_live_terminal(client, script.as_deref(), json_lines, &terminal_root)
    });
    let config = LiveRunConfig::new(
        workspace.root.clone(),
        PathBuf::from(&workspace.manifest.entry),
        PathBuf::from(&workspace.manifest.output),
    );
    let run_result =
        run_live_in_process(&entry, Some(&workspace.root), 16_000, None, server, config);
    if !terminal.is_finished() {
        return match run_result {
            Ok(()) => Err("live runner ended before the terminal session completed".to_string()),
            Err(error) => Err(error),
        };
    }
    let terminal_result = terminal
        .join()
        .map_err(|_| "live terminal thread panicked".to_string())?;
    run_result?;
    terminal_result?;
    Ok(CommandResult::success(
        "interactive live session ended",
        json!({"backend": "jit", "headless": false, "interactive": true}),
    ))
}

fn run_live_terminal(
    client: stasis_runner::live::LiveSessionClient,
    script: Option<&Path>,
    json_lines: bool,
    project_root: &Path,
) -> Result<(), String> {
    let result = run_live_terminal_inner(&client, script, json_lines, project_root);
    if result.is_err() {
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
    }
    result
}

fn run_live_terminal_inner(
    client: &stasis_runner::live::LiveSessionClient,
    script: Option<&Path>,
    json_lines: bool,
    project_root: &Path,
) -> Result<(), String> {
    let mut terminal = TerminalBuffer::new();
    let mut saw_quit = false;
    let mut script_failure = None;
    if let Some(script) = script {
        let file = fs::File::open(script)
            .map_err(|error| format!("failed to open live script {}: {error}", script.display()))?;
        for line in io::BufReader::new(file).lines() {
            let line = line.map_err(|error| format!("failed reading live script: {error}"))?;
            if let Some(prompt) = line.trim().strip_prefix(":ai ") {
                let (summary, trace, usage_trace) =
                    live_tui::run_scripted_ai(client, project_root, prompt)?;
                if json_lines {
                    println!(
                        "{}",
                        serde_json::json!({
                            "schema_version": 1,
                            "kind": "ai_completed",
                            "ok": true,
                            "summary": summary,
                            "trace": trace,
                            "usage_trace": usage_trace,
                        })
                    );
                } else {
                    println!("AI complete: {summary}");
                    println!("AI trace: {}", trace.display());
                    println!("AI usage: {}", usage_trace.display());
                }
                continue;
            }
            if line.trim() == ":ai" {
                return Err("live script :ai requires a prompt".to_string());
            }
            if let TerminalInput::Request(request) = terminal.feed_line(&line)? {
                saw_quit |= matches!(&request.command, LiveCommand::Quit);
                let request_id = request.request_id;
                if !submit_and_print_live_response(client, request, json_lines, true)?
                    && script_failure.is_none()
                {
                    script_failure = Some(format!("live request {request_id} failed"));
                }
            }
        }
        if terminal.cancel_pending() {
            return Err(
                "live script ended with unfinished multiline input; add :end or :abort".to_string(),
            );
        }
    } else {
        saw_quit = live_tui::run(client, project_root)?;
    }
    if !saw_quit {
        submit_and_print_live_response(
            client,
            LiveRequest::new(u64::MAX, LiveCommand::Quit),
            json_lines,
            true,
        )?;
    }
    match script_failure {
        Some(error) => Err(error),
        None => Ok(()),
    }
}

fn submit_and_print_live_response(
    client: &stasis_runner::live::LiveSessionClient,
    request: LiveRequest,
    json_lines: bool,
    wait_for_preparation: bool,
) -> Result<bool, String> {
    let request_id = request.request_id;
    client.submit(request)?;
    loop {
        let response = client.receive_timeout(Duration::from_secs(300))?;
        let request_succeeded = response.ok;
        print_live_response(&response, json_lines)?;
        if response.request_id == request_id {
            if wait_for_preparation
                && matches!(
                    response.kind.as_str(),
                    "edit_preparing" | "completion_preparing"
                )
            {
                continue;
            }
            return Ok(request_succeeded);
        }
    }
}

fn print_live_response(response: &LiveResponse, json_lines: bool) -> Result<(), String> {
    if json_lines {
        println!(
            "{}",
            serde_json::to_string(response)
                .map_err(|error| format!("failed serializing live response: {error}"))?
        );
    } else if response.ok {
        println!("{}", format_live_response(response));
    } else {
        eprintln!("{}", format_live_response(response));
    }
    Ok(())
}

fn format_live_response(response: &LiveResponse) -> String {
    if !response.ok {
        return format!(
            "error: {}",
            response.error.as_deref().unwrap_or("unknown live error")
        );
    }
    let data = response.data.as_ref().unwrap_or(&Value::Null);
    match response.kind.as_str() {
        "help" => format_live_help(data),
        "status" => format_live_status(data),
        "paused" => "paused".to_string(),
        "resumed" => "running".to_string(),
        "step_scheduled" => format!(
            "step scheduled: {} tick(s)",
            data.get("ticks").and_then(Value::as_u64).unwrap_or(0)
        ),
        "quitting" => "session closed".to_string(),
        "cancellation_requested" => format!(
            "cancel requested for #{}{}",
            data.get("request_id").and_then(Value::as_u64).unwrap_or(0),
            if data
                .get("background")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                " (background edit)"
            } else {
                ""
            }
        ),
        "symbols" => format_live_symbols(data),
        "symbol" => format_live_symbol(data),
        "references" => format_live_references(data),
        "completion" | "palette" if response.truncated => {
            "completion response exceeded the output bound; narrow the query".to_string()
        }
        "completion" | "palette" => format_live_completion(data),
        "inspection" => format_live_inspection(data),
        "state_inspection" => format_live_state_inspection(data),
        "runtime_validation" => format_live_runtime_validation(data),
        "print" => format!(
            "{} = {}",
            string_field(data, "static_type", "value"),
            scalar_text(data.get("value").unwrap_or(&Value::Null))
        ),
        "watch_added" => format!(
            "watching {} = {}",
            string_field(data, "path", "value"),
            scalar_text(data.get("value").unwrap_or(&Value::Null))
        ),
        "watch" => format!(
            "{} -> {}",
            string_field(data, "path", "value"),
            scalar_text(data.get("value").unwrap_or(&Value::Null))
        ),
        "watch_error" => format!(
            "{} watch error: {}",
            string_field(data, "path", "value"),
            string_field(data, "error", "unknown watch error")
        ),
        "watch_removed" => format_live_watch_removed(data),
        "watch_backpressure" => format!(
            "watch output dropped {} event(s)",
            data.get("dropped_events")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ),
        "mutation_preview" => format_live_mutation(data, "preview"),
        "mutation_committed" => format_live_mutation(data, "set"),
        "transaction_preview" => format_live_transaction(data, "preview"),
        "transaction_committed" => format_live_transaction(data, "committed"),
        "cell_saved" => format!(
            "saved scratch cell '{}'",
            string_field(data, "name", "unnamed")
        ),
        "cells" => format_live_cells(data),
        "cells_cleared" => match data.get("name").and_then(Value::as_str) {
            Some(name) => format!("cleared scratch cell '{name}'"),
            None => "cleared all scratch cells".to_string(),
        },
        "edit_preparing" => format!(
            "preparing edit #{}...",
            data.get("request_id").and_then(Value::as_u64).unwrap_or(0)
        ),
        "edit_preview" => format!("preview ready: {}", format_live_plan(data)),
        "edit_applied" => format!(
            "applied: {} (tests {})",
            format_live_plan(data),
            string_field(data, "tests", "unknown")
        ),
        "edit_undone" => format!(
            "undo complete (tests {})",
            string_field(data, "tests", "unknown")
        ),
        "edit_redone" => format!(
            "redo complete (tests {})",
            string_field(data, "tests", "unknown")
        ),
        "changes" => format_live_changes(data),
        kind => kind.to_string(),
    }
}

fn string_field<'a>(data: &'a Value, name: &str, fallback: &'a str) -> &'a str {
    data.get(name).and_then(Value::as_str).unwrap_or(fallback)
}

fn scalar_text(value: &Value) -> String {
    let value = value.get("value").unwrap_or(value);
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        _ => "<structured value>".to_string(),
    }
}

fn format_live_inspection(data: &Value) -> String {
    if data.get("kind").and_then(Value::as_str) == Some("predicate") {
        let matches = data
            .get("total_matches")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let suffix = if data
            .get("scan_truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
            || data
                .get("matches_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        {
            " (bounded)"
        } else {
            ""
        };
        return format!(
            "{}: {matches} match(es){suffix}",
            string_field(data, "query", "predicate")
        );
    }
    format!(
        "{}: {} = {}",
        string_field(data, "path", "value"),
        string_field(data, "static_type", "unknown"),
        scalar_text(data.get("value").unwrap_or(&Value::Null))
    )
}

fn format_live_state_inspection(data: &Value) -> String {
    let mut lines = vec![format!(
        "{} live state value(s){}",
        data.get("total").and_then(Value::as_u64).unwrap_or(0),
        if data
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            " (view bounded)"
        } else {
            ""
        }
    )];
    for item in data
        .get("items")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        lines.push(format!(
            "  {}: {} = {}",
            string_field(item, "path", "value"),
            string_field(item, "static_type", "unknown"),
            scalar_text(item.get("value").unwrap_or(&Value::Null))
        ));
    }
    for collection in data
        .get("collections")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        lines.push(format!(
            "  {} [{}/{}]",
            string_field(collection, "path", "collection"),
            collection
                .get("active_count")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            collection
                .get("capacity")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ));
        for field in collection
            .get("fields")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[])
        {
            let name = string_field(field, "field", "element");
            lines.push(format!(
                "    {}: {}",
                if name.is_empty() { "element" } else { name },
                string_field(field, "type_name", "unknown")
            ));
        }
    }
    for state_struct in data
        .get("structs")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[])
    {
        lines.push(format!(
            "  {}: {}",
            string_field(state_struct, "path", "state"),
            string_field(state_struct, "type_name", "struct")
        ));
    }
    if let Some(memory) = data.get("memory") {
        lines.push(format!(
            "  memory: {} bytes; snapshot: {} bytes",
            memory
                .get("total_capacity_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0),
            memory
                .get("snapshot_bytes")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        ));
    }
    lines.join("\n")
}

fn format_live_help(data: &Value) -> String {
    let commands = data
        .get("commands")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let mut lines = vec!["Commands:".to_string()];
    for group in commands.chunks(3) {
        lines.push(format!(
            "  {}",
            group
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join("  ")
        ));
    }
    lines.push("Multiline: :end to submit; :abort or Ctrl-C to cancel.".to_string());
    lines.join("\n")
}

fn format_live_status(data: &Value) -> String {
    let state = if data.get("paused").and_then(Value::as_bool).unwrap_or(false) {
        "paused"
    } else {
        "running"
    };
    let cursor = data
        .get("history_cursor")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let history = data
        .get("history_length")
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let watches = string_array(data.get("watches"));
    let cells = data
        .get("scratch_cells")
        .and_then(Value::as_array)
        .map(|cells| {
            cells
                .iter()
                .filter_map(|cell| cell.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let mut line = format!("{state} | edits {cursor}/{history}");
    if !watches.is_empty() {
        line.push_str(&format!(" | watches {watches}"));
    }
    if !cells.is_empty() {
        line.push_str(&format!(" | cells {cells}"));
    }
    if let Some(request) = data.get("preparing_request_id").and_then(Value::as_u64) {
        line.push_str(&format!(" | preparing #{request}"));
    }
    line
}

fn string_array(value: Option<&Value>) -> String {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default()
}

fn format_live_symbols(data: &Value) -> String {
    let total = data.get("total").and_then(Value::as_u64).unwrap_or(0);
    let page = data.get("page").and_then(Value::as_u64).unwrap_or(0);
    let mut lines = vec![format!("{total} symbol(s), page {page}:")];
    if let Some(items) = data.get("items").and_then(Value::as_array) {
        for item in items {
            lines.push(format!(
                "  {}  [{}]",
                string_field(item, "signature", string_field(item, "name", "unnamed")),
                string_field(item, "file", "unknown file")
            ));
        }
    }
    lines.join("\n")
}

fn format_live_symbol(data: &Value) -> String {
    let header = format!(
        "{}  [{}]",
        string_field(data, "signature", string_field(data, "name", "unnamed")),
        string_field(data, "file", "unknown file")
    );
    match data.get("source").and_then(Value::as_str) {
        Some(source) => format!("{header}\n{}", source.trim()),
        None => header,
    }
}

fn format_live_references(data: &Value) -> String {
    let references = data
        .get("references")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    if references.is_empty() {
        return format!(
            "no references found for {}",
            string_field(data, "symbol", "symbol")
        );
    }
    references
        .iter()
        .map(|reference| {
            format!(
                "{}  {}  {}  {}",
                string_field(reference, "kind", "read"),
                string_field(reference, "file", "unknown"),
                string_field(reference, "containing_name", "unknown"),
                string_field(reference, "containing_signature", ""),
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_live_runtime_validation(data: &Value) -> String {
    let Some(check) = data
        .get("checks")
        .and_then(Value::as_array)
        .and_then(|checks| checks.first())
    else {
        return "runtime validation returned no check".to_string();
    };
    format!(
        "{}: {} {} {} (actual {}, {} frame(s))",
        if check
            .get("passed")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "PASS"
        } else {
            "FAIL"
        },
        string_field(check, "path", "value"),
        string_field(check, "op", "eq"),
        scalar_text(check.get("expected").unwrap_or(&Value::Null)),
        scalar_text(check.get("actual").unwrap_or(&Value::Null)),
        data.get("frames").and_then(Value::as_u64).unwrap_or(0),
    )
}

fn format_live_completion(data: &Value) -> String {
    let mut lines = Vec::new();
    if let Some(items) = data.get("items").and_then(Value::as_array) {
        for item in items {
            lines.push(format!(
                "{}  {}  {}",
                string_field(item, "text", ""),
                string_field(item, "kind", ""),
                string_field(item, "detail", "")
            ));
        }
    }
    if data
        .get("truncated")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        lines.push("... more matches; keep typing".to_string());
    }
    if lines.is_empty() {
        "no completions".to_string()
    } else {
        lines.join("\n")
    }
}

fn format_live_watch_removed(data: &Value) -> String {
    let watches = string_array(data.get("watches"));
    if watches.is_empty() {
        "no active watches".to_string()
    } else {
        format!("active watches: {watches}")
    }
}

fn format_live_mutation(data: &Value, action: &str) -> String {
    format!(
        "{action} {}: {} -> {} ({})",
        string_field(data, "path", "value"),
        scalar_text(data.get("old").unwrap_or(&Value::Null)),
        scalar_text(data.get("new").unwrap_or(&Value::Null)),
        string_field(data, "static_type", "unknown")
    )
}

fn format_live_transaction(data: &Value, action: &str) -> String {
    let transaction = data.get("result").unwrap_or(data);
    let name = data.get("name").and_then(Value::as_str);
    let mut lines = vec![match name {
        Some(name) => format!("scratch '{name}' {action}:"),
        None => format!("transaction {action}:"),
    }];
    if let Some(mutations) = transaction.get("mutations").and_then(Value::as_array) {
        for mutation in mutations {
            lines.push(format!(
                "  {}: {} -> {} ({})",
                string_field(mutation, "path", "value"),
                scalar_text(mutation.get("old").unwrap_or(&Value::Null)),
                scalar_text(mutation.get("new").unwrap_or(&Value::Null)),
                string_field(mutation, "static_type", "unknown")
            ));
        }
    }
    lines.join("\n")
}

fn format_live_cells(data: &Value) -> String {
    let Some(cells) = data.get("cells").and_then(Value::as_array) else {
        return "no scratch cells".to_string();
    };
    if cells.is_empty() {
        return "no scratch cells".to_string();
    }
    cells
        .iter()
        .map(|cell| string_field(cell, "name", "unnamed").to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_live_plan(data: &Value) -> String {
    let plan = data.get("plan").unwrap_or(data);
    let file_count = plan
        .get("changed_files")
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    let symbols = plan
        .pointer("/reload/changed_symbols")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.get("name").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join(", ")
        })
        .unwrap_or_default();
    let reload = plan
        .pointer("/reload/expected_reload")
        .and_then(Value::as_str)
        .unwrap_or("reload unknown");
    let mut summary = if symbols.is_empty() {
        format!("{file_count} file(s), {reload}")
    } else {
        format!("{symbols}; {file_count} file(s), {reload}")
    };
    if let Some(swap) = data.get("swap") {
        let compatible = if swap
            .get("state_layout_compatible")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "compatible"
        } else {
            "incompatible"
        };
        let migration_steps = swap
            .get("migration_steps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        let estimated_us = swap
            .get("estimated_commit_cost_us")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        summary.push_str(&format!(
            "; state layout {compatible}, {migration_steps} migration step(s), estimated commit {estimated_us} us"
        ));
        if let Some(functions) = swap.get("changed_functions").and_then(Value::as_array) {
            let functions = functions
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", ");
            summary.push_str(&format!(
                "\nchanged functions: {}",
                if functions.is_empty() {
                    "none"
                } else {
                    &functions
                }
            ));
        }
        if let (Some(from), Some(to)) = (
            swap.get("from_layout_version").and_then(Value::as_str),
            swap.get("to_layout_version").and_then(Value::as_str),
        ) {
            summary.push_str(&format!("\nlayout version: {from} -> {to}"));
        }
        if let Some(scope) = swap.get("migration_scope") {
            let kind = string_field(scope, "kind", "none");
            let path = scope.get("path").and_then(Value::as_str);
            summary.push_str(&format!(
                "\nmigration scope: {kind}{}",
                path.map_or_else(String::new, |path| format!(" ({path})"))
            ));
        }
        if let Some(steps) = swap.get("migration_steps").and_then(Value::as_array) {
            for step in steps {
                let path = string_field(step, "path", "state");
                let field = step.get("field").and_then(Value::as_str);
                let start = step.get("start_index").and_then(Value::as_u64).unwrap_or(0);
                let elements = step.get("elements").and_then(Value::as_u64).unwrap_or(1);
                let target = field.map_or_else(
                    || path.to_string(),
                    |field| {
                        if field.is_empty() {
                            path.to_string()
                        } else {
                            display_live_collection_path(path, field)
                        }
                    },
                );
                let range = field
                    .is_some()
                    .then(|| format!("[{start}..{})", start.saturating_add(elements)))
                    .unwrap_or_default();
                summary.push_str(&format!(
                    "\nmigration: {} {target}{range} ({})",
                    string_field(step, "kind", "unknown"),
                    string_field(step, "type_name", "unknown")
                ));
            }
        }
        if let Some(warnings) = swap.get("warnings").and_then(Value::as_array) {
            for warning in warnings.iter().filter_map(Value::as_str) {
                summary.push_str(&format!("\nwarning: {warning}"));
            }
        }
        if let Some(rejection) = swap.get("rejection").and_then(Value::as_str) {
            summary.push_str(&format!("\nrejected: {rejection}"));
        }
    }
    summary
}

fn display_live_collection_path(path: &str, field: &str) -> String {
    if field.is_empty() {
        format!("{path}[]")
    } else {
        format!("{path}[].{field}")
    }
}

fn format_live_changes(data: &Value) -> String {
    let cursor = data.get("cursor").and_then(Value::as_u64).unwrap_or(0);
    let Some(entries) = data.get("entries").and_then(Value::as_array) else {
        return "no semantic edit history".to_string();
    };
    if entries.is_empty() {
        return "no semantic edit history".to_string();
    }
    let mut lines = vec![format!(
        "edit history ({cursor}/{} applied):",
        entries.len()
    )];
    for entry in entries {
        let index = entry.get("index").and_then(Value::as_u64).unwrap_or(0);
        let marker = if entry
            .get("applied")
            .and_then(Value::as_bool)
            .unwrap_or(false)
        {
            "*"
        } else {
            " "
        };
        let files = entry
            .get("changed_files")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        lines.push(format!(" {marker} #{index}: {files} file(s)"));
    }
    lines.join("\n")
}

fn build_workspace(
    workspace: &Workspace,
    mode: BuildMode,
    output: Option<&Path>,
) -> Result<CommandResult, String> {
    match mode {
        BuildMode::Dev => {
            let jit = compile_workspace_jit(workspace)?;
            let receipt = output
                .map(|path| workspace.root.join(path))
                .unwrap_or_else(|| {
                    workspace
                        .root
                        .join(&workspace.manifest.output)
                        .join("dev-build.json")
                });
            validate_workspace_destination(workspace, "build output", &receipt)?;
            if let Some(parent) = receipt.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            let data = json!({
                "backend": "jit",
                "entry": workspace.manifest.entry,
                "functions_emitted": jit.artifacts().len(),
            });
            let mut contents = serde_json::to_string_pretty(&data)
                .map_err(|error| format!("failed to serialize dev build receipt: {error}"))?;
            contents.push('\n');
            fs::write(&receipt, contents)
                .map_err(|error| format!("failed to write {}: {error}", receipt.display()))?;
            Ok(CommandResult::success(
                format!("built JIT development image: {}", receipt.display()),
                json!({"backend": "jit", "receipt": display_path(&receipt)}),
            ))
        }
        BuildMode::Release => {
            let output = output
                .map(|path| workspace.root.join(path))
                .unwrap_or_else(|| default_release_output(workspace));
            validate_workspace_destination(workspace, "build output", &output)?;
            if let Some(parent) = output.parent() {
                fs::create_dir_all(parent)
                    .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
            }
            let summary = run_self_host_aot_cli_with_options(
                &workspace.root,
                &output,
                None,
                Some(Path::new(&workspace.manifest.entry)),
            )?;
            Ok(CommandResult::success(
                format!("built release executable: {}", output.display()),
                json!({
                    "backend": "aot",
                    "output": display_path(&output),
                    "source_files": summary.source_file_count,
                    "entry_symbol": summary.entry_symbol,
                }),
            ))
        }
    }
}

fn package_workspace(
    workspace: &Workspace,
    target: PackageTarget,
    output: Option<&Path>,
    development_build: bool,
) -> Result<CommandResult, String> {
    let package_root = output
        .map(|path| workspace.root.join(path))
        .unwrap_or_else(|| {
            workspace.root.join("dist").join(format!(
                "{}-{}",
                workspace.manifest.name,
                target.as_str()
            ))
        });
    validate_workspace_destination(workspace, "package output", &package_root)?;
    if package_root.exists() {
        return Err(format!(
            "package output already exists: {}",
            package_root.display()
        ));
    }
    if !matches!(target, PackageTarget::Desktop) {
        return package_mobile_workspace(
            workspace,
            target,
            Path::new(&workspace.manifest.entry),
            &package_root,
            development_build,
        );
    }
    let staging_name = format!(
        ".{}.staging",
        package_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stasis-package")
    );
    let staging_root = package_root.with_file_name(staging_name);
    if staging_root.exists() {
        return Err(format!(
            "package staging output already exists: {}",
            staging_root.display()
        ));
    }
    let provenance = resolve_package_provenance(development_build)?;
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("failed to create {}: {error}", staging_root.display()))?;
    let assembled = (|| -> Result<(), String> {
        let executable = staging_root.join(executable_name(&workspace.manifest.name));
        build_workspace(workspace, BuildMode::Release, Some(&executable))?;
        let summary = executable.with_file_name(format!(
            "{}.summary.json",
            executable.file_name().unwrap_or_default().to_string_lossy()
        ));
        if summary.exists() {
            fs::remove_file(&summary).map_err(|error| {
                format!(
                    "failed to remove package build receipt {}: {error}",
                    summary.display()
                )
            })?;
        }
        copy_file(
            &workspace.root.join(MANIFEST_NAME),
            &staging_root.join(MANIFEST_NAME),
        )?;
        let assets = workspace.root.join("assets");
        validate_workspace_destination(workspace, "assets directory", &assets)?;
        copy_dir_if_exists(&assets, &staging_root.join("assets"))?;
        if let Some(runtime) = installed_runtime_library() {
            copy_file(
                &runtime,
                &staging_root.join(runtime.file_name().unwrap_or_default()),
            )?;
        }
        write_json_file(&staging_root.join(PACKAGE_PROVENANCE_NAME), &provenance)?;
        Ok(())
    })();
    if let Err(error) = assembled {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }
    fs::rename(&staging_root, &package_root).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_root);
        format!(
            "failed to publish package {}: {error}",
            package_root.display()
        )
    })?;
    Ok(CommandResult::success(
        format!("packaged {} at {}", target.as_str(), package_root.display()),
        json!({
            "target": target.as_str(),
            "output": display_path(&package_root),
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
        }),
    ))
}

fn package_mobile_command(
    workspace: &Workspace,
    target: MobilePackageTarget,
    entry: Option<&Path>,
    output: Option<&Path>,
    development_build: bool,
) -> Result<CommandResult, String> {
    let target = target.package_target();
    let entry = entry.unwrap_or_else(|| Path::new(&workspace.manifest.entry));
    let package_root = output
        .map(|path| workspace.root.join(path))
        .unwrap_or_else(|| {
            workspace.root.join("dist").join(format!(
                "{}-{}",
                workspace.manifest.name,
                target.as_str()
            ))
        });
    validate_workspace_destination(workspace, "package output", &package_root)?;
    package_mobile_workspace(workspace, target, entry, &package_root, development_build)
}

fn package_mobile_workspace(
    workspace: &Workspace,
    target: PackageTarget,
    entry: &Path,
    package_root: &Path,
    development_build: bool,
) -> Result<CommandResult, String> {
    if package_root.exists() {
        return Err(format!(
            "package output already exists: {}",
            package_root.display()
        ));
    }
    let staging_root = package_root.with_file_name(format!(
        ".{}.staging",
        package_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stasis-mobile")
    ));
    if staging_root.exists() {
        return Err(format!(
            "package staging output already exists: {}",
            staging_root.display()
        ));
    }
    let provenance = resolve_package_provenance(development_build)?;
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("failed to create {}: {error}", staging_root.display()))?;
    let child_result = (|| -> Result<(), String> {
        let aot_root = staging_root.join("aot");
        let executable = env::current_exe()
            .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
        let child = Command::new(executable)
            .arg("mobile-aot-bundle")
            .arg("--target")
            .arg(target.as_str())
            .arg("--project-dir")
            .arg(&workspace.root)
            .arg("--entry-file")
            .arg(entry)
            .arg("--out-dir")
            .arg(&aot_root)
            .output()
            .map_err(|error| format!("failed to launch mobile AOT packaging: {error}"))?;
        if !child.status.success() {
            return Err(format!(
                "mobile AOT packaging failed with exit code {}: stdout={} stderr={}",
                child.status.code().unwrap_or(1),
                String::from_utf8_lossy(&child.stdout).trim(),
                String::from_utf8_lossy(&child.stderr).trim()
            ));
        }
        assemble_mobile_shell(workspace, target, &aot_root, &staging_root, &provenance)?;
        Ok(())
    })();
    if let Err(error) = child_result {
        let _ = fs::remove_dir_all(&staging_root);
        return Err(error);
    }
    fs::rename(&staging_root, package_root).map_err(|error| {
        let _ = fs::remove_dir_all(&staging_root);
        format!(
            "failed to publish mobile package {}: {error}",
            package_root.display()
        )
    })?;
    Ok(CommandResult::success(
        format!("packaged {} at {}", target.as_str(), package_root.display()),
        json!({
            "target": target.as_str(),
            "output": display_path(package_root),
            "entry": display_path(entry),
            "project": match target {
                PackageTarget::AndroidArm64 => "android",
                PackageTarget::IosArm64 => "ios/StasisMobile.xcodeproj",
                PackageTarget::Desktop => unreachable!(),
            },
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
        }),
    ))
}

fn assemble_mobile_shell(
    workspace: &Workspace,
    target: PackageTarget,
    aot_root: &Path,
    staging_root: &Path,
    provenance: &Value,
) -> Result<(), String> {
    let mobile_assets = bundled_mobile_assets_dir()?;
    let runtime = bundled_mobile_runtime_dir()?;
    let platform = match target {
        PackageTarget::AndroidArm64 => "android",
        PackageTarget::IosArm64 => "ios",
        PackageTarget::Desktop => return Err("desktop is not a mobile package target".to_string()),
    };
    let common_destination = staging_root.join("common");
    let platform_destination = staging_root.join(platform);
    copy_required_dir(&mobile_assets.join("common"), &common_destination)?;
    copy_required_dir(&mobile_assets.join(platform), &platform_destination)?;
    copy_mobile_runtime(&runtime, &staging_root.join("runtime"))?;
    write_json_file(&staging_root.join(PACKAGE_PROVENANCE_NAME), provenance)?;
    write_mobile_provenance_header(&common_destination, provenance)?;

    let asset_source = match target {
        PackageTarget::AndroidArm64 => aot_root.join("apk_assets/stasis_game"),
        PackageTarget::IosArm64 => aot_root.join("ios_assets/stasis_game"),
        PackageTarget::Desktop => unreachable!(),
    };
    let asset_destination = match target {
        PackageTarget::AndroidArm64 => staging_root.join("android/app/src/main/assets/stasis_game"),
        PackageTarget::IosArm64 => staging_root.join("ios/StasisMobile/stasis_game"),
        PackageTarget::Desktop => unreachable!(),
    };
    let package_id = mobile_package_id(&workspace.manifest.name);
    let jni_package = package_id.replace('.', "_");
    let replacements = [
        ("@STASIS_APP_NAME@", workspace.manifest.name.as_str()),
        ("@STASIS_PACKAGE_ID@", package_id.as_str()),
        ("@STASIS_JNI_PACKAGE@", jni_package.as_str()),
        ("@STASIS_ASSET_BASE@", "."),
    ];
    replace_shell_tokens(&common_destination, &replacements)?;
    replace_shell_tokens(&platform_destination, &replacements)?;
    copy_required_dir(&asset_source, &asset_destination)?;
    write_json_file(&asset_destination.join(PACKAGE_PROVENANCE_NAME), provenance)?;
    fs::write(
        asset_destination.join("stasis_asset_base.marker"),
        b"stasis.mobile.asset_base.v1\n",
    )
    .map_err(|error| format!("failed to write packaged asset base marker: {error}"))?;
    if !asset_destination.join("assets/manifest.json").is_file() {
        return Err(format!(
            "mobile AOT bundle is missing asset manifest: {}",
            asset_destination.join("assets/manifest.json").display()
        ));
    }
    if matches!(target, PackageTarget::IosArm64) {
        write_ios_object_config(aot_root, &staging_root.join("ios/StasisMobile.xcconfig"))?;
    }
    fs::write(
        staging_root.join("stasis_mobile_package.json"),
        serde_json::to_string_pretty(&json!({
            "schema": "stasis.mobile_package.v1",
            "target": target.as_str(),
            "name": workspace.manifest.name,
            "aot_manifest": "aot/mobile_aot_bundle_manifest.json",
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
            "assets": match target {
                PackageTarget::AndroidArm64 => "android/app/src/main/assets/stasis_game",
                PackageTarget::IosArm64 => "ios/StasisMobile/stasis_game",
                PackageTarget::Desktop => unreachable!(),
            },
        }))
        .map_err(|error| format!("failed to encode mobile package manifest: {error}"))?
            + "\n",
    )
    .map_err(|error| format!("failed to write mobile package manifest: {error}"))?;
    Ok(())
}

fn mobile_package_id(project_name: &str) -> String {
    let mut component = "game".to_string();
    for byte in project_name.bytes() {
        if byte.is_ascii_alphanumeric() {
            component.push((byte as char).to_ascii_lowercase());
        } else {
            component.push_str(&format!("x{byte:02x}"));
        }
    }
    format!("com.stasislang.{component}")
}

fn copy_required_dir(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.is_dir() {
        return Err(format!(
            "required directory is missing: {}",
            source.display()
        ));
    }
    copy_dir_if_exists(source, destination)
}

fn copy_mobile_runtime(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    for name in MOBILE_RUNTIME_FILES {
        copy_file(&source.join(name), &destination.join(name))?;
    }
    Ok(())
}

fn write_json_file(path: &Path, value: &Value) -> Result<(), String> {
    let mut body = serde_json::to_string_pretty(value)
        .map_err(|error| format!("failed to encode {}: {error}", path.display()))?;
    body.push('\n');
    fs::write(path, body).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|error| {
        format!(
            "failed to read provenance input {}: {error}",
            path.display()
        )
    })?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|error| format!("failed to hash {}: {error}", path.display()))?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn provenance_relative_path(root: &Path, value: &str) -> Result<PathBuf, String> {
    let relative = Path::new(value);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("release provenance contains unsafe path: {value}"));
    }
    Ok(root.join(relative))
}

fn content_hashes(root: &Path, prefix: &str) -> Result<serde_json::Map<String, Value>, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        prefix: &str,
        output: &mut serde_json::Map<String, Value>,
    ) -> Result<(), String> {
        let mut entries = fs::read_dir(directory)
            .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| format!("failed to enumerate {}: {error}", directory.display()))?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let kind = entry.file_type().map_err(|error| {
                format!("failed to inspect {}: {error}", entry.path().display())
            })?;
            if kind.is_symlink() {
                return Err(format!(
                    "provenance input cannot contain a symlink: {}",
                    entry.path().display()
                ));
            }
            if kind.is_dir() {
                visit(root, &entry.path(), prefix, output)?;
            } else if kind.is_file() {
                let path = entry.path();
                let relative = path
                    .strip_prefix(root)
                    .map_err(|_| format!("provenance path escaped root: {}", path.display()))?;
                let key = format!("{prefix}/{}", relative.to_string_lossy().replace('\\', "/"));
                output.insert(key, Value::String(sha256_file(&path)?));
            }
        }
        Ok(())
    }

    let mut output = serde_json::Map::new();
    visit(root, root, prefix, &mut output)?;
    Ok(output)
}

fn release_provenance_path() -> Result<PathBuf, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let directory = executable.parent().unwrap_or(Path::new("."));
    for candidate in [
        directory.join(RELEASE_PROVENANCE_NAME),
        directory.join("..").join(RELEASE_PROVENANCE_NAME),
    ] {
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "installed toolchain is missing {RELEASE_PROVENANCE_NAME}; reinstall an official release or pass --development-build for a visibly labeled local package"
    ))
}

fn provenance_string_field<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field]
        .as_str()
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("release provenance is missing {field}"))
}

fn verify_release_provenance(path: &Path) -> Result<Value, String> {
    let bytes = fs::read(path).map_err(|error| {
        format!(
            "failed to read release provenance {}: {error}",
            path.display()
        )
    })?;
    let value: Value = serde_json::from_slice(&bytes).map_err(|error| {
        format!(
            "failed to parse release provenance {}: {error}",
            path.display()
        )
    })?;
    if value["schema"] != "stasis.release_provenance.v1"
        || value["development_build"] != false
        || value["dirty_state"] != false
        || value["command_buffer"]["version"] != 1
    {
        return Err("release provenance is not a clean official gfx_cmd v1 build".to_string());
    }
    let release_tag = provenance_string_field(&value, "release_tag")?;
    let source_commit = provenance_string_field(&value, "source_commit")?;
    if !(release_tag.starts_with("v") || release_tag.starts_with("nightly-"))
        || source_commit.len() != 40
        || !source_commit.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err("release provenance has an invalid release tag or source commit".to_string());
    }
    if value["dependencies"]
        .as_object()
        .is_none_or(|items| items.is_empty())
        || value["backends"].as_array().is_none_or(Vec::is_empty)
        || value["features"].as_array().is_none_or(Vec::is_empty)
    {
        return Err(
            "release provenance is missing dependencies, backends, or features".to_string(),
        );
    }
    let dependencies = &value["dependencies"];
    if dependencies["cargo_packages"]
        .as_array()
        .is_none_or(Vec::is_empty)
        || dependencies["sdl2"].as_str().is_none_or(str::is_empty)
        || dependencies["sdl2_image"]
            .as_str()
            .is_none_or(str::is_empty)
    {
        return Err("release provenance is missing exact dependency versions".to_string());
    }
    let root = path.parent().unwrap_or(Path::new("."));
    let compiler = &value["compiler"];
    let compiler_path = provenance_relative_path(root, provenance_string_field(compiler, "path")?)?;
    let expected_compiler = provenance_string_field(compiler, "sha256")?;
    let actual_compiler = sha256_file(&compiler_path)?;
    let running_compiler = sha256_file(
        &env::current_exe()
            .map_err(|error| format!("failed to locate stasis executable: {error}"))?,
    )?;
    if actual_compiler != expected_compiler || running_compiler != expected_compiler {
        return Err(format!(
            "release compiler hash mismatch for {}: expected {expected_compiler}, packaged {actual_compiler}, running {running_compiler}",
            compiler_path.display()
        ));
    }
    let sources = value["runtime_sources"]
        .as_object()
        .ok_or_else(|| "release provenance is missing runtime_sources".to_string())?;
    for name in MOBILE_RUNTIME_FILES {
        let key = format!("runtime/{name}");
        let expected = sources
            .get(&key)
            .and_then(Value::as_str)
            .ok_or_else(|| format!("release provenance is missing runtime hash {key}"))?;
        let source = provenance_relative_path(root, &key)?;
        let actual = sha256_file(&source)?;
        if actual != expected {
            return Err(format!(
                "release runtime hash mismatch for {key}: expected {expected}, found {actual}"
            ));
        }
    }
    let expected_shells = value["mobile_shell_sources"]
        .as_object()
        .ok_or_else(|| "release provenance is missing mobile_shell_sources".to_string())?;
    let shell_root = root.join("mobile/shells");
    let actual_shells = content_hashes(&shell_root, "mobile/shells")?;
    if &actual_shells != expected_shells {
        return Err(
            "release mobile shell source hashes do not match the installed templates".to_string(),
        );
    }
    Ok(value)
}

fn git_text(args: &[&str]) -> Option<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn development_provenance() -> Result<Value, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let runtime = bundled_mobile_runtime_dir()?;
    let mobile_shells = bundled_mobile_assets_dir()?;
    let mut sources = serde_json::Map::new();
    for name in MOBILE_RUNTIME_FILES {
        sources.insert(
            format!("runtime/{name}"),
            Value::String(sha256_file(&runtime.join(name))?),
        );
    }
    Ok(json!({
        "schema": "stasis.release_provenance.v1",
        "release_tag": Value::Null,
        "source_commit": git_text(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string()),
        "dirty_state": true,
        "development_build": true,
        "compiler": {
            "path": executable.file_name().unwrap_or_default().to_string_lossy(),
            "sha256": sha256_file(&executable)?,
        },
        "runtime_sources": sources,
        "mobile_shell_sources": content_hashes(&mobile_shells, "mobile/shells")?,
        "command_buffer": {"name": "gfx_cmd", "version": 1},
        "backends": ["sdl2"],
        "features": ["aot", "jit", "mobile-aot", "shared-renderer"],
        "dependencies": {"stasis": env!("CARGO_PKG_VERSION"), "toolchain": "development"},
    }))
}

fn resolve_package_provenance(development_build: bool) -> Result<Value, String> {
    if development_build {
        development_provenance()
    } else {
        verify_release_provenance(&release_provenance_path()?)
    }
}

fn c_string(value: &str) -> Result<String, String> {
    serde_json::to_string(value)
        .map_err(|error| format!("failed to quote provenance text: {error}"))
}

fn write_mobile_provenance_header(destination: &Path, provenance: &Value) -> Result<(), String> {
    let tag = provenance["release_tag"].as_str().unwrap_or("development");
    let commit = provenance["source_commit"].as_str().unwrap_or("unknown");
    let label = if provenance["development_build"].as_bool() == Some(true) {
        "non-release development build"
    } else {
        "official release"
    };
    let body = format!(
        "#ifndef STASIS_PACKAGE_PROVENANCE_H\n#define STASIS_PACKAGE_PROVENANCE_H\n#define STASIS_PACKAGE_RELEASE_TAG {}\n#define STASIS_PACKAGE_SOURCE_COMMIT {}\n#define STASIS_PACKAGE_BUILD_LABEL {}\n#endif\n",
        c_string(tag)?,
        c_string(commit)?,
        c_string(label)?,
    );
    fs::write(destination.join("stasis_package_provenance.h"), body)
        .map_err(|error| format!("failed to write mobile provenance header: {error}"))
}

fn replace_shell_tokens(root: &Path, replacements: &[(&str, &str)]) -> Result<(), String> {
    for entry in
        fs::read_dir(root).map_err(|error| format!("failed to read {}: {error}", root.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read {} entry: {error}", root.display()))?
            .path();
        if path.is_dir() {
            replace_shell_tokens(&path, replacements)?;
            continue;
        }
        let Ok(mut source) = fs::read_to_string(&path) else {
            continue;
        };
        let original = source.clone();
        for (token, value) in replacements {
            source = source.replace(token, value);
        }
        if source != original {
            fs::write(&path, source)
                .map_err(|error| format!("failed to update {}: {error}", path.display()))?;
        }
    }
    Ok(())
}

fn write_ios_object_config(aot_root: &Path, output: &Path) -> Result<(), String> {
    let mut objects = Vec::new();
    for entry in fs::read_dir(aot_root)
        .map_err(|error| format!("failed to read {}: {error}", aot_root.display()))?
    {
        let path = entry
            .map_err(|error| format!("failed to read {} entry: {error}", aot_root.display()))?
            .path();
        if path.extension().and_then(|value| value.to_str()) == Some("o") {
            objects.push(path);
        }
    }
    objects.sort();
    if objects.is_empty() {
        return Err("mobile AOT bundle did not emit any object files".to_string());
    }
    let object_flags = objects
        .iter()
        .map(|path| {
            format!(
                "$(PROJECT_DIR)/../aot/{}",
                path.file_name().unwrap_or_default().to_string_lossy()
            )
        })
        .collect::<Vec<_>>()
        .join(" ");
    fs::write(
        output,
        format!(
            "GCC_PREPROCESSOR_DEFINITIONS = $(inherited) STASIS_GRAPHICS_SDL_ONLY=1\nFRAMEWORK_SEARCH_PATHS = $(inherited) $(STASIS_SDL_FRAMEWORKS)/SDL2.xcframework/ios-arm64 $(STASIS_SDL_FRAMEWORKS)/SDL2_image.xcframework/ios-arm64\nHEADER_SEARCH_PATHS = $(inherited) $(PROJECT_DIR)/../aot $(PROJECT_DIR)/../runtime $(STASIS_SDL_FRAMEWORKS)/SDL2.xcframework/ios-arm64/SDL2.framework/Headers $(STASIS_SDL_FRAMEWORKS)/SDL2_image.xcframework/ios-arm64/SDL2_image.framework/Headers\nLD_RUNPATH_SEARCH_PATHS = $(inherited) @executable_path/Frameworks\nOTHER_LDFLAGS = $(inherited) -framework SDL2 -framework SDL2_image {object_flags}\n"
        ),
    )
    .map_err(|error| format!("failed to write {}: {error}", output.display()))
}

impl SymbolSelectorArgs {
    fn selector(&self) -> WorkshopSymbolSelector {
        WorkshopSymbolSelector {
            name: self.name.clone(),
            kind: self.kind.map(Into::into),
            file: self.file.clone(),
            owner: self.owner.clone(),
            signature: self.signature.clone(),
        }
    }
}

impl RequiredSymbolTargetArgs {
    fn selector(&self) -> WorkshopSymbolSelector {
        WorkshopSymbolSelector {
            name: self.name.clone(),
            kind: Some(self.kind.into()),
            file: Some(self.file.clone()),
            owner: self.owner.clone(),
            signature: self.signature.clone(),
        }
    }
}

fn symbol_workspace(
    workspace: &Workspace,
    command: SymbolCommand,
) -> Result<CommandResult, String> {
    let files =
        load_workshop_edit_workspace(&workspace.root, Path::new(&workspace.manifest.entry))?;
    let editable_files = files
        .iter()
        .filter(|file| is_editable_workshop_path(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    match command {
        SymbolCommand::List {
            query,
            kind,
            file: files,
            owner,
            page,
            limit,
        } => {
            let limit = limit.clamp(1, 200);
            let mut items = workshop_source_items(&editable_files)?;
            if files.len() > 16 {
                return Err("symbol list accepts at most 16 --file values".to_string());
            }
            let default_scope = files.is_empty();
            let mut scope_files = if default_scope {
                vec![normalize_symbol_file(&workspace.manifest.entry)]
            } else {
                files
                    .iter()
                    .map(|file| normalize_symbol_file(file))
                    .collect::<Vec<_>>()
            };
            if default_scope {
                scope_files.extend(workshop_direct_import_files(
                    &editable_files,
                    Path::new(&workspace.manifest.entry),
                )?);
            }
            let available_files = items
                .iter()
                .map(|item| normalize_symbol_file(&item.file))
                .collect::<BTreeSet<_>>();
            for file in &scope_files {
                if !available_files.contains(file) {
                    return Err(format!("symbol list file is not in the project: {file}"));
                }
            }
            let scope_files = scope_files.into_iter().collect::<BTreeSet<_>>();
            let imports = scope_files
                .iter()
                .map(|file| {
                    Ok((
                        file.clone(),
                        workshop_direct_import_files(&editable_files, Path::new(file))?,
                    ))
                })
                .collect::<Result<BTreeMap<_, _>, String>>()?;
            items.retain(|item| {
                item.kind != WorkshopSourceItemKind::Imports
                    && !(item.kind == WorkshopSourceItemKind::Globals
                        && item.source.trim().is_empty())
                    && query.as_deref().is_none_or(|query| {
                        let query = query.to_ascii_lowercase();
                        item.name.to_ascii_lowercase().contains(&query)
                            || item.signature.to_ascii_lowercase().contains(&query)
                    })
                    && kind.is_none_or(|kind| item.kind == kind.into())
                    && scope_files.contains(&normalize_symbol_file(&item.file))
                    && owner
                        .as_deref()
                        .is_none_or(|owner| item.owner.as_deref() == Some(owner))
            });
            let total = items.len();
            let items = items
                .into_iter()
                .skip(page.saturating_mul(limit))
                .take(limit)
                .collect::<Vec<_>>();
            let human = items
                .iter()
                .map(|item| {
                    format!(
                        "{:?}\t{}\t{}\t{}",
                        item.kind,
                        item.file,
                        item.owner.as_deref().unwrap_or(""),
                        item.name
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            let metadata = items
                .into_iter()
                .map(|item| {
                    let mut value = json!({
                        "kind": item.kind,
                        "name": item.name,
                        "file": item.file,
                        "signature": item.signature,
                    });
                    if let Some(owner) = item.owner {
                        value["owner"] = Value::String(owner);
                    }
                    value
                })
                .collect::<Vec<_>>();
            Ok(CommandResult::success(
                human,
                json!({"schema_version": 1, "files": scope_files, "imports": imports, "page": page, "limit": limit, "total": total, "items": metadata}),
            ))
        }
        SymbolCommand::Find(args) => {
            let items = find_workshop_symbols(&editable_files, &args.selector())?;
            let metadata = items
                .iter()
                .map(|item| {
                    json!({
                        "kind": item.kind,
                        "name": item.name,
                        "owner": item.owner,
                        "file": item.file,
                        "signature": item.signature,
                        "source_hash": item.source_hash,
                    })
                })
                .collect::<Vec<_>>();
            Ok(CommandResult::success(
                format!("found {} item(s)", metadata.len()),
                json!({"schema_version": 1, "matches": metadata}),
            ))
        }
        SymbolCommand::Read(args) => {
            let selector = args.selector();
            let mut items = find_workshop_symbols(&editable_files, &selector)?;
            if items.len() != 1 {
                return Err(format!(
                    "symbol read requires exactly one match; found {}",
                    items.len()
                ));
            }
            let item = items.remove(0);
            Ok(CommandResult::success(
                item.source.clone(),
                json!({"schema_version": 1, "item": item}),
            ))
        }
        SymbolCommand::References { symbol, limit } => {
            let references = find_workshop_references(&editable_files, &symbol, limit)?;
            let human = references
                .iter()
                .map(|reference| {
                    format!(
                        "{:?}\t{}\t{}\t{}",
                        reference.kind,
                        reference.file,
                        reference.containing_name,
                        reference.containing_signature,
                    )
                })
                .collect::<Vec<_>>()
                .join("\n");
            Ok(CommandResult::success(
                human,
                json!({"schema_version": 1, "symbol": symbol, "references": references}),
            ))
        }
        SymbolCommand::Add {
            target,
            source,
            source_file,
            options,
        } => {
            let source = read_symbol_source(workspace, source, source_file)?;
            let batch = WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![WorkshopSemanticEdit {
                    operation: WorkshopSemanticEditOperation::Add,
                    target: target.selector(),
                    new_source: Some(source),
                    expected_source_hash: None,
                }],
            };
            apply_symbol_batch(workspace, &files, batch, options)
        }
        SymbolCommand::Update {
            target,
            source,
            source_file,
            expected_source_hash,
            options,
        } => {
            let source = read_symbol_source(workspace, source, source_file)?;
            let batch = WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![WorkshopSemanticEdit {
                    operation: WorkshopSemanticEditOperation::Update,
                    target: target.selector(),
                    new_source: Some(source),
                    expected_source_hash,
                }],
            };
            apply_symbol_batch(workspace, &files, batch, options)
        }
        SymbolCommand::Delete {
            target,
            expected_source_hash,
            options,
        } => {
            let batch = WorkshopSemanticEditBatch {
                schema_version: 1,
                edits: vec![WorkshopSemanticEdit {
                    operation: WorkshopSemanticEditOperation::Delete,
                    target: target.selector(),
                    new_source: None,
                    expected_source_hash,
                }],
            };
            apply_symbol_batch(workspace, &files, batch, options)
        }
        SymbolCommand::Apply { request, options } => {
            let source = read_workspace_input(workspace, "semantic edit request", &request)?;
            let batch = serde_json::from_str::<WorkshopSemanticEditBatch>(&source)
                .map_err(|error| format!("invalid semantic edit request: {error}"))?;
            apply_symbol_batch(workspace, &files, batch, options)
        }
        SymbolCommand::Revert {
            receipt,
            dry_run,
            no_tests,
        } => revert_symbol_plan(workspace, &receipt, dry_run, no_tests),
    }
}

fn normalize_symbol_file(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches("./").to_string()
}

fn read_symbol_source(
    workspace: &Workspace,
    source: Option<String>,
    source_file: Option<PathBuf>,
) -> Result<String, String> {
    match (source, source_file) {
        (Some(source), None) => Ok(source),
        (None, Some(path)) => read_workspace_input(workspace, "symbol source", &path),
        (Some(_), Some(_)) => Err("use only one of --source or --source-file".to_string()),
        (None, None) => Err("symbol add/update requires --source or --source-file".to_string()),
    }
}

fn apply_symbol_batch(
    workspace: &Workspace,
    files: &[WorkshopSourceFile],
    mut batch: WorkshopSemanticEditBatch,
    options: SymbolEditOptions,
) -> Result<CommandResult, String> {
    normalize_cli_semantic_batch(files, &mut batch)?;
    let (after, plan) = plan_workshop_semantic_edits(files, &batch)?;
    validate_semantic_files(workspace, &after)?;
    if options.dry_run {
        return Ok(CommandResult::success(
            format!(
                "preview: {} file(s) would change; {:?}",
                plan.changed_files.len(),
                plan.reload.expected_reload
            ),
            json!({
                "status": "preview",
                "validated": true,
                "tests": "not_run_for_preview",
                "plan": plan,
            }),
        ));
    }

    write_workshop_semantic_plan(&workspace.root, &plan, false)?;
    let validation = if options.no_tests {
        Ok(json!({"compiler": "passed", "tests": "skipped"}))
    } else {
        test_workspace(workspace, None).map(
            |result| json!({"compiler": "passed", "tests": "passed", "test_result": result.data}),
        )
    };
    let validation = match validation {
        Ok(validation) => validation,
        Err(error) => {
            write_workshop_semantic_plan(&workspace.root, &plan, true).map_err(|rollback| {
                format!(
                    "semantic edit validation failed: {error}; rollback also failed: {rollback}"
                )
            })?;
            let restored = load_workshop_edit_workspace(
                &workspace.root,
                Path::new(&workspace.manifest.entry),
            )?;
            validate_semantic_files(workspace, &restored)?;
            return Err(format!(
                "semantic edit validation failed and all source changes were rolled back: {error}"
            ));
        }
    };
    let receipt = match write_symbol_receipt(workspace, &plan) {
        Ok(receipt) => receipt,
        Err(error) => {
            write_workshop_semantic_plan(&workspace.root, &plan, true).map_err(|rollback| {
                format!("semantic receipt failed: {error}; rollback also failed: {rollback}")
            })?;
            let restored = load_workshop_edit_workspace(
                &workspace.root,
                Path::new(&workspace.manifest.entry),
            )?;
            validate_semantic_files(workspace, &restored)?;
            return Err(format!(
                "semantic receipt failed and all source changes were rolled back: {error}"
            ));
        }
    };
    Ok(CommandResult::success(
        format!(
            "applied {} semantic edit(s); receipt {}",
            plan.edits.len(),
            receipt.display()
        ),
        json!({
            "status": "applied",
            "plan": plan,
            "validation": validation,
            "receipt": relative_display(&workspace.root, &receipt),
        }),
    ))
}

fn normalize_cli_semantic_batch(
    files: &[WorkshopSourceFile],
    batch: &mut WorkshopSemanticEditBatch,
) -> Result<(), String> {
    let editable = files
        .iter()
        .filter(|file| is_editable_workshop_path(&file.path))
        .cloned()
        .collect::<Vec<_>>();
    for edit in &mut batch.edits {
        if let Some(file) = edit.target.file.as_deref() {
            if !is_editable_workshop_path(file) {
                return Err(format!(
                    "semantic edits are limited to project src/ and tests/ files: {file}"
                ));
            }
            continue;
        }
        if edit.operation == WorkshopSemanticEditOperation::Add {
            return Err("semantic add requires target.file".to_string());
        }
        let matches = find_workshop_symbols(&editable, &edit.target)?;
        if matches.len() != 1 {
            return Err(format!(
                "semantic edit requires one editable target; found {}",
                matches.len()
            ));
        }
        edit.target.file = Some(matches[0].file.clone());
    }
    Ok(())
}

fn validate_semantic_files(
    workspace: &Workspace,
    files: &[WorkshopSourceFile],
) -> Result<(), String> {
    let files = workshop_reachable_files(files, Path::new(&workspace.manifest.entry))?;
    let mut jit = JitProcess::new();
    jit.set_local_runtime_helper_trampolines(true);
    jit.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    for file in &files {
        let path = workspace.root.join(&file.path);
        let compiler_path = path
            .canonicalize()
            .unwrap_or(path)
            .to_string_lossy()
            .to_string();
        jit.upsert_file(compiler_path, file.source.clone());
    }
    jit.compile().map_err(|error| {
        jit.last_source_diagnostic()
            .map(|diagnostic| {
                format!(
                    "{}:{}-{}: {}",
                    diagnostic.path, diagnostic.start, diagnostic.end, diagnostic.message
                )
            })
            .unwrap_or_else(|| format!("{error:?}"))
    })?;
    Ok(())
}

fn write_symbol_receipt(
    workspace: &Workspace,
    plan: &WorkshopSemanticEditPlan,
) -> Result<PathBuf, String> {
    let relative_directory = Path::new(&workspace.manifest.output).join("semantic-edits");
    let receipt = workspace
        .root
        .join(&relative_directory)
        .join("receipt.json");
    validate_workspace_destination(workspace, "semantic edit receipt", &receipt)?;
    write_workshop_semantic_receipt(&workspace.root, &relative_directory, plan)
        .map(|relative| workspace.root.join(relative))
}

fn revert_symbol_plan(
    workspace: &Workspace,
    receipt: &Path,
    dry_run: bool,
    no_tests: bool,
) -> Result<CommandResult, String> {
    let source = read_workspace_input(workspace, "semantic edit receipt", receipt)?;
    let plan = serde_json::from_str::<WorkshopSemanticEditPlan>(&source)
        .map_err(|error| format!("invalid semantic edit receipt: {error}"))?;
    if plan.schema_version != 1 {
        return Err(format!(
            "unsupported semantic edit receipt schema version {}",
            plan.schema_version
        ));
    }
    let current =
        load_workshop_edit_workspace(&workspace.root, Path::new(&workspace.manifest.entry))?;
    let mut restored = current.clone();
    for change in &plan.changed_files {
        let file = restored
            .iter_mut()
            .find(|file| file.path == change.file)
            .ok_or_else(|| format!("receipt file is no longer imported: {}", change.file))?;
        if workshop_source_hash(&file.source) != change.after_hash {
            return Err(format!(
                "refusing revert because {} changed after the recorded edit",
                change.file
            ));
        }
        file.source = change.before_source.clone();
    }
    validate_semantic_files(workspace, &restored)?;
    if dry_run {
        return Ok(CommandResult::success(
            format!(
                "preview: {} file(s) would be restored",
                plan.changed_files.len()
            ),
            json!({"status": "revert_preview", "validated": true, "plan": plan}),
        ));
    }
    write_workshop_semantic_plan(&workspace.root, &plan, true)?;
    if !no_tests {
        if let Err(error) = test_workspace(workspace, None) {
            write_workshop_semantic_plan(&workspace.root, &plan, false).map_err(|rollback| {
                format!("revert tests failed: {error}; reapply also failed: {rollback}")
            })?;
            return Err(format!(
                "revert tests failed and the edited sources were reapplied: {error}"
            ));
        }
    }
    Ok(CommandResult::success(
        format!("reverted {} file(s)", plan.changed_files.len()),
        json!({
            "status": "reverted",
            "changed_files": plan.changed_files.iter().map(|change| &change.file).collect::<Vec<_>>(),
            "compiler": "passed",
            "tests": if no_tests { "skipped" } else { "passed" },
        }),
    ))
}

fn read_workspace_input(workspace: &Workspace, field: &str, path: &Path) -> Result<String, String> {
    validate_optional_workspace_path(workspace, field, Some(path))?;
    let absolute = workspace.root.join(path);
    fs::read_to_string(&absolute)
        .map_err(|error| format!("failed to read {}: {error}", absolute.display()))
}

fn is_editable_workshop_path(path: &str) -> bool {
    let normalized = path.replace('\\', "/");
    normalized.starts_with("src/") || normalized.starts_with("tests/")
}

fn inspect_workspace(
    workspace: &Workspace,
    capacities: &[String],
    mobile_budget_bytes: u64,
) -> Result<CommandResult, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    let tests = workspace.root.join(&workspace.manifest.tests);
    let output = workspace.root.join(&workspace.manifest.output);
    let capacity_overrides = parse_capacity_overrides(capacities)?;
    let jit = compile_workspace_jit(workspace)?;
    let memory = jit.state_memory_report(&capacity_overrides, mobile_budget_bytes)?;
    let mut human = vec![format!(
        "state memory: {} capacity bytes; {} snapshot bytes; {} mobile budget bytes",
        memory.total_capacity_bytes, memory.snapshot_bytes, memory.mobile_budget_bytes
    )];
    if memory.projected_capacity_bytes != memory.total_capacity_bytes {
        human.push(format!(
            "projected capacity: {} bytes ({:+} bytes)",
            memory.projected_capacity_bytes,
            i128::from(memory.projected_capacity_bytes) - i128::from(memory.total_capacity_bytes)
        ));
    }
    if !memory.largest_pools.is_empty() {
        human.push("largest pools:".to_string());
        human.extend(memory.largest_pools.iter().map(|pool| {
            format!(
                "  {}: {} bytes ({} x {})",
                pool.path, pool.capacity_bytes, pool.capacity, pool.bytes_per_element
            )
        }));
    }
    human.extend(
        memory
            .warnings
            .iter()
            .map(|warning| format!("warning: {warning}")),
    );
    Ok(CommandResult::success(
        human.join("\n"),
        json!({
            "name": workspace.manifest.name,
            "root": display_path(&workspace.root),
            "entry": display_path(&entry),
            "tests": display_path(&tests),
            "output": display_path(&output),
            "manifest_version": workspace.manifest.manifest_version,
            "memory": memory,
        }),
    ))
}

fn parse_capacity_overrides(values: &[String]) -> Result<BTreeMap<String, u64>, String> {
    let mut overrides = BTreeMap::new();
    for value in values {
        let (path, count) = value
            .split_once('=')
            .ok_or_else(|| format!("invalid capacity override '{value}'; expected PATH=COUNT"))?;
        if path.is_empty() {
            return Err(format!(
                "invalid capacity override '{value}'; path cannot be empty"
            ));
        }
        let count = count
            .parse::<u64>()
            .map_err(|error| format!("invalid capacity override '{value}': {error}"))?;
        if overrides.insert(path.to_string(), count).is_some() {
            return Err(format!("duplicate capacity override for '{path}'"));
        }
    }
    Ok(overrides)
}

fn env_result(explicit: Option<&Path>) -> Result<CommandResult, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let workspace = if explicit.is_some() {
        Some(load_workspace(explicit)?)
    } else {
        load_workspace(None).ok()
    };
    let cache = workspace
        .as_ref()
        .map(|value| value.root.join(".stasis_cache"));
    let data = json!({
        "version": env!("CARGO_PKG_VERSION"),
        "executable": display_path(&executable),
        "toolchain_root": display_path(executable.parent().unwrap_or(Path::new("."))),
        "workspace": workspace.as_ref().map(|value| display_path(&value.root)),
        "cache": cache.as_ref().map(|value| display_path(value)),
        "offline_core_workflows": true,
        "manifest": MANIFEST_NAME,
    });
    Ok(CommandResult::success(
        format!(
            "stasis={} executable={} workspace={}",
            env!("CARGO_PKG_VERSION"),
            executable.display(),
            workspace
                .as_ref()
                .map(|value| value.root.display().to_string())
                .unwrap_or_else(|| "none".to_string())
        ),
        data,
    ))
}

fn version_result() -> CommandResult {
    CommandResult::success(
        format!("stasis {}", env!("CARGO_PKG_VERSION")),
        json!({"version": env!("CARGO_PKG_VERSION")}),
    )
}

fn default_release_output(workspace: &Workspace) -> PathBuf {
    workspace
        .root
        .join(&workspace.manifest.output)
        .join(executable_name(&workspace.manifest.name))
}

fn executable_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn installed_runtime_library() -> Option<PathBuf> {
    let executable = env::current_exe().ok()?;
    let directory = executable.parent()?;
    let name = if cfg!(windows) {
        "stasis_graphics.dll"
    } else if cfg!(target_os = "macos") {
        "libstasis_graphics.dylib"
    } else {
        "libstasis_graphics.so"
    };
    let path = directory.join(name);
    path.is_file().then_some(path)
}

fn bundled_stdlib_dir() -> Result<PathBuf, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let executable_dir = executable.parent().unwrap_or(Path::new("."));
    let source_tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src/stdlib");
    for candidate in [
        executable_dir.join("src/stdlib"),
        executable_dir.join("../src/stdlib"),
        source_tree,
    ] {
        if candidate.join("stdlib.stasis").is_file() {
            return Ok(candidate);
        }
    }
    Err(
        "installed toolchain is missing src/stdlib; reinstall the complete release archive"
            .to_string(),
    )
}

fn bundled_mobile_assets_dir() -> Result<PathBuf, String> {
    bundled_toolchain_directory("mobile/shells", "mobile shell templates")
}

fn bundled_mobile_runtime_dir() -> Result<PathBuf, String> {
    bundled_toolchain_directory("runtime", "mobile runtime sources")
}

fn bundled_toolchain_directory(relative: &str, label: &str) -> Result<PathBuf, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let executable_dir = executable.parent().unwrap_or(Path::new("."));
    let source_tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for candidate in [
        executable_dir.join(relative),
        executable_dir.join("..").join(relative),
        source_tree.join(relative),
    ] {
        if candidate.is_dir() {
            return Ok(candidate);
        }
    }
    Err(format!(
        "installed toolchain is missing {relative} ({label}); reinstall the complete release archive"
    ))
}

fn collect_stasis_files(root: &Path, files: &mut Vec<PathBuf>) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    let mut entries: Vec<_> = fs::read_dir(root)
        .map_err(|error| format!("failed to read {}: {error}", root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate {}: {error}", root.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if file_type.is_symlink() {
            continue;
        }
        if file_type.is_dir() {
            collect_stasis_files(&entry.path(), files)?;
        } else if entry.path().extension().and_then(|value| value.to_str()) == Some("stasis") {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn copy_dir_if_exists(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    fs::create_dir_all(destination)
        .map_err(|error| format!("failed to create {}: {error}", destination.display()))?;
    let mut entries: Vec<_> = fs::read_dir(source)
        .map_err(|error| format!("failed to read {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate {}: {error}", source.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        let target = destination.join(entry.file_name());
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            copy_dir_if_exists(&entry.path(), &target)?;
        } else {
            copy_file(&entry.path(), &target)?;
        }
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    fs::copy(source, destination).map(|_| ()).map_err(|error| {
        format!(
            "failed to copy {} to {}: {error}",
            source.display(),
            destination.display()
        )
    })
}

fn validate_project_name(name: &str) -> Result<(), String> {
    let valid = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-');
    if valid {
        Ok(())
    } else {
        Err("project name must be 1-64 ASCII letters, digits, '-' or '_'".to_string())
    }
}

fn validate_relative_path(field: &str, path: &Path) -> Result<(), String> {
    if path.as_os_str().is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|component| matches!(component, Component::ParentDir | Component::Prefix(_)))
    {
        return Err(format!("{field} must be a non-empty project-relative path"));
    }
    Ok(())
}

fn validate_optional_workspace_path(
    workspace: &Workspace,
    field: &str,
    path: Option<&Path>,
) -> Result<(), String> {
    let Some(path) = path else {
        return Ok(());
    };
    validate_relative_path(field, path)?;
    validate_workspace_destination(workspace, field, &workspace.root.join(path))
}

fn validate_workspace_destination(
    workspace: &Workspace,
    field: &str,
    candidate: &Path,
) -> Result<(), String> {
    let root = workspace
        .root
        .canonicalize()
        .map_err(|error| format!("failed to resolve workspace root: {error}"))?;
    let mut ancestor = candidate;
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| format!("failed to resolve {field}"))?;
    }
    let resolved = ancestor
        .canonicalize()
        .map_err(|error| format!("failed to resolve {field}: {error}"))?;
    if !resolved.starts_with(&root) {
        return Err(format!("{field} resolves outside the workspace"));
    }
    Ok(())
}

fn absolute_path(path: &Path) -> Result<PathBuf, String> {
    if path.is_absolute() {
        Ok(path.to_path_buf())
    } else {
        env::current_dir()
            .map(|current| current.join(path))
            .map_err(|error| format!("failed to read current directory: {error}"))
    }
}

fn display_path(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}

fn relative_display(root: &Path, path: &Path) -> String {
    display_path(path.strip_prefix(root).unwrap_or(path))
}

fn line_column(source: &str, offset: usize) -> (usize, usize) {
    let mut bounded = offset.min(source.len());
    while !source.is_char_boundary(bounded) {
        bounded -= 1;
    }
    let prefix = &source[..bounded];
    let line = prefix.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let column = prefix
        .rsplit_once('\n')
        .map(|(_, tail)| tail.chars().count() + 1)
        .unwrap_or_else(|| prefix.chars().count() + 1);
    (line, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_ai::live_tool_specs;
    use std::collections::BTreeMap;

    #[test]
    fn invalid_interactive_command_does_not_poison_terminal_buffer() {
        let mut terminal = TerminalBuffer::new();
        assert_eq!(
            terminal.feed_line("view render"),
            Err("unknown live command 'view'; use :help".to_string())
        );
        let TerminalInput::Request(request) = terminal
            .feed_line(":status")
            .expect("valid command after invalid input")
        else {
            panic!("expected status request")
        };
        assert_eq!(request.command, LiveCommand::Status);
    }

    #[test]
    fn human_palette_output_reports_bounded_truncation() {
        let response = LiveResponse::success(
            9,
            44,
            "palette",
            json!({
                "replacement_start": 0,
                "replacement_end": 2,
                "page": 0,
                "truncated": true,
                "items": [{"text": "tick", "kind": "function", "detail": "tick(): i32"}]
            }),
        );
        assert_eq!(
            format_live_response(&response),
            "tick  function  tick(): i32\n... more matches; keep typing"
        );
        let bounded = LiveResponse::success(
            10,
            45,
            "palette",
            json!({"items": [{"text": "x".repeat(1024), "kind": "function", "detail": "x"}]}),
        )
        .bounded(256);
        assert_eq!(
            format_live_response(&bounded),
            "completion response exceeded the output bound; narrow the query"
        );
    }

    #[test]
    fn human_live_output_formats_scalars_without_json_envelopes() {
        let response = LiveResponse::success(
            7,
            42,
            "inspection",
            json!({
                "path": "player.score",
                "static_type": "i32",
                "value": {"type": "i32", "value": 12}
            }),
        );
        assert_eq!(format_live_response(&response), "player.score: i32 = 12");
    }

    #[test]
    fn human_live_output_formats_references_and_validation_evidence() {
        let references = LiveResponse::success(
            12,
            42,
            "references",
            json!({
                "symbol": "GameState.player_y",
                "references": [{
                    "kind": "write",
                    "file": "src/main.stasis",
                    "containing_name": "update_player_paddle",
                    "containing_signature": "update_player_paddle(): void"
                }]
            }),
        );
        assert_eq!(
            format_live_response(&references),
            "write  src/main.stasis  update_player_paddle  update_player_paddle(): void"
        );

        let validation = LiveResponse::success(
            13,
            42,
            "runtime_validation",
            json!({
                "frames": 2,
                "checks": [{
                    "path": "Render.command1_h",
                    "op": "eq",
                    "expected": 144,
                    "actual": 144,
                    "passed": true
                }]
            }),
        );
        assert_eq!(
            format_live_response(&validation),
            "PASS: Render.command1_h eq 144 (actual 144, 2 frame(s))"
        );
    }

    #[test]
    fn every_live_ai_tool_has_a_human_command_surface() {
        let mappings = BTreeMap::from([
            ("list_symbols", "stasis symbol list / :symbols"),
            ("find_references", "stasis symbol references / :references"),
            ("read_symbol", "stasis symbol read / :read"),
            ("write_symbol", "stasis symbol update / :update"),
            ("delete_symbol", "stasis symbol delete / :delete"),
            ("inspect_runtime_state", ":inspect"),
            ("run_frame", ":step / stasis validate --frames"),
        ]);

        for tool in live_tool_specs() {
            assert!(
                mappings.contains_key(tool.tool.as_str()),
                "live AI tool '{}' needs a useful CLI/TUI mapping",
                tool.tool
            );
        }
    }

    #[test]
    fn human_live_output_summarizes_plans_without_source_or_receipt_json() {
        let response = LiveResponse::success(
            8,
            43,
            "edit_applied",
            json!({
                "plan": {
                    "changed_files": [{
                        "file": "src/main.stasis",
                        "before_source": "old source",
                        "after_source": "very long new source"
                    }],
                    "reload": {
                        "changed_symbols": [{"name": "tick"}],
                        "expected_reload": "FastReload"
                    }
                },
                "receipt": "build/live-edits/receipt.json",
                "tests": "passed"
            }),
        );
        let output = format_live_response(&response);
        assert_eq!(
            output,
            "applied: tick; 1 file(s), FastReload (tests passed)"
        );
        assert!(!output.contains("source"));
        assert!(!output.contains("receipt"));
        assert!(!output.contains('{'));
    }

    #[test]
    fn human_live_preview_shows_layout_migration_cost_and_warning() {
        let response = LiveResponse::success(
            9,
            44,
            "edit_preview",
            json!({
                "plan": {
                    "changed_files": [{"file": "src/main.stasis"}],
                    "reload": {
                        "changed_symbols": [{"name": "State"}],
                        "expected_reload": "ResetRequired"
                    }
                },
                "swap": {
                    "state_layout_compatible": true,
                    "changed_functions": ["tick"],
                    "from_layout_version": "layout-v1",
                    "to_layout_version": "layout-v2",
                    "migration_scope": {"kind": "struct", "path": "State"},
                    "migration_steps": [
                        {"kind": "copy", "path": "State.score", "type_name": "i32", "elements": 1, "start_index": 0},
                        {"kind": "initialize", "path": "State.items", "field": "", "type_name": "i32", "elements": 4, "start_index": 4}
                    ],
                    "estimated_commit_cost_us": 37,
                    "warnings": ["collection 'State.items' shrinks from 8 to 4"]
                }
            }),
        );
        assert_eq!(
            format_live_response(&response),
            "preview ready: State; 1 file(s), ResetRequired; state layout compatible, 2 migration step(s), estimated commit 37 us\nchanged functions: tick\nlayout version: layout-v1 -> layout-v2\nmigration scope: struct (State)\nmigration: copy State.score (i32)\nmigration: initialize State.items[4..8) (i32)\nwarning: collection 'State.items' shrinks from 8 to 4"
        );
    }

    #[test]
    fn human_live_output_formats_scratch_transactions_as_mutation_lines() {
        let response = LiveResponse::success(
            9,
            44,
            "transaction_preview",
            json!({
                "name": "lift_ball",
                "persistent": false,
                "result": {
                    "preview": true,
                    "mutations": [{
                        "path": "ball_y",
                        "static_type": "f32",
                        "old": {"type": "f32", "value": 300.0},
                        "new": {"type": "f32", "value": 180.0}
                    }]
                }
            }),
        );
        assert_eq!(
            format_live_response(&response),
            "scratch 'lift_ball' preview:\n  ball_y: 300.0 -> 180.0 (f32)"
        );
    }

    #[test]
    fn human_live_output_never_uses_raw_json_as_an_unknown_fallback() {
        let response = LiveResponse::success(
            10,
            45,
            "future_response",
            json!({"large": {"nested": [1, 2, 3]}}),
        );
        assert_eq!(format_live_response(&response), "future_response");

        let failure = LiveResponse::failure(11, 46, "layout change rejected");
        assert_eq!(
            format_live_response(&failure),
            "error: layout change rejected"
        );
    }

    #[test]
    fn human_live_output_reports_watch_backpressure_count() {
        let response =
            LiveResponse::success(12, 47, "watch_backpressure", json!({"dropped_events": 9}));
        assert_eq!(
            format_live_response(&response),
            "watch output dropped 9 event(s)"
        );
    }
    use std::sync::atomic::{AtomicU64, Ordering};

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(1);

    fn temp_dir(name: &str) -> PathBuf {
        env::temp_dir().join(format!(
            "stasis_toolchain_cli_{name}_{}_{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::SeqCst)
        ))
    }

    fn remove_temp(path: &Path) {
        let _ = fs::remove_dir_all(path);
    }

    #[test]
    fn fresh_runtime_validation_boots_ticks_renders_and_checks_state() {
        let root = temp_dir("fresh_validation");
        fs::create_dir_all(root.join("src")).expect("src");
        fs::write(
            root.join(MANIFEST_NAME),
            serde_json::to_vec(&ProjectManifest::new("fresh_validation".into())).expect("manifest"),
        )
        .expect("write manifest");
        fs::write(
            root.join("src/main.stasis"),
            "global State { value: i32; rendered: i32; }\nfunction main(): i32 { State.value = 1; State.rendered = 0; return 0; }\nfunction tick(): i32 { State.value += 1; return 0; }\nfunction render(): i32 { State.rendered = 1; return 0; }\n",
        )
        .expect("write source");
        let workspace = load_workspace(Some(&root)).expect("workspace");
        let requirements = serde_json::to_string(&vec![
            RuntimeValidationRequirement {
                path: "State.value".into(),
                op: "eq".into(),
                value: json!(3),
            },
            RuntimeValidationRequirement {
                path: "State.rendered".into(),
                op: "eq".into(),
                value: json!(1),
            },
        ])
        .expect("requirements");

        let result = validate_fresh_runtime(&workspace, 2, &requirements, "main", "tick", "render")
            .expect("validation");

        assert_eq!(result.data["baseline"], "fresh");
        assert_eq!(result.data["requirements_met"], true);
        assert_eq!(result.data["checks"].as_array().expect("checks").len(), 2);
        remove_temp(&root);
    }

    #[test]
    fn workspace_discovery_works_from_nested_directories() {
        let root = temp_dir("discovery");
        create_project(root.clone(), "demo".to_string()).expect("create project");
        let nested = root.join("src/nested");
        fs::create_dir_all(&nested).expect("create nested");
        assert_eq!(find_workspace_root(&nested), Some(root.clone()));
        remove_temp(&root);
    }

    #[test]
    fn source_formatter_is_deterministic() {
        let source = "function main(): i32 {  \r\n    return 0;\t\r\n}\r\n\r\n";
        let expected = "function main(): i32 {\n    return 0;\n}\n";
        assert_eq!(format_source(source), expected);
        assert_eq!(format_source(expected), expected);
    }

    #[test]
    fn generated_project_checks_tests_and_runs_through_jit() {
        let root = temp_dir("smoke");
        create_project(root.clone(), "smoke".to_string()).expect("create project");
        let workspace = load_workspace(Some(&root)).expect("load workspace");
        check_workspace(&workspace).expect("check project");
        test_workspace(&workspace, None).expect("test project");
        let run = run_workspace(&workspace, true).expect("run project");
        assert_eq!(run.code, 0);
        remove_temp(&root);
    }

    #[test]
    fn run_accepts_headless_and_watch_modes() {
        let parsed = ToolchainCli::try_parse_from([
            "stasis",
            "--workspace",
            "demo",
            "run",
            "--headless",
            "--watch",
        ])
        .expect("parse run flags");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::Run {
                watch: true,
                headless: true,
                ..
            }
        ));
    }

    #[test]
    fn ai_command_accepts_one_prompt() {
        let parsed = ToolchainCli::try_parse_from([
            "stasis",
            "--workspace",
            "demo",
            "ai",
            "make the paddle twice as long",
        ])
        .expect("parse AI command");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::Ai { ref prompt }
                if prompt == "make the paddle twice as long"
        ));
    }

    #[test]
    fn package_mobile_cli_accepts_entry_override() {
        let parsed = ToolchainCli::try_parse_from([
            "stasis",
            "--workspace",
            "demo",
            "package-mobile",
            "--target",
            "ios-arm64",
            "--entry",
            "src/mobile.stasis",
            "--out",
            "dist/ios",
        ])
        .expect("parse package-mobile flags");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::PackageMobile {
                target: MobilePackageTarget::IosArm64,
                entry: Some(ref entry),
                out: Some(ref out),
                development_build: false,
            } if entry == Path::new("src/mobile.stasis") && out == Path::new("dist/ios")
        ));
    }

    #[test]
    fn release_provenance_rejects_substituted_renderer_sources() {
        let root = temp_dir("release_provenance_mismatch");
        let runtime = root.join("runtime");
        fs::create_dir_all(&runtime).expect("create runtime fixture");
        let compiler_name = executable_name("stasis");
        fs::copy(
            env::current_exe().expect("current test executable"),
            root.join(&compiler_name),
        )
        .expect("write compiler fixture");
        let mut runtime_sources = serde_json::Map::new();
        for name in MOBILE_RUNTIME_FILES {
            let path = runtime.join(name);
            fs::write(&path, format!("official {name}\n")).expect("write runtime fixture");
            runtime_sources.insert(
                format!("runtime/{name}"),
                Value::String(sha256_file(&path).expect("hash runtime fixture")),
            );
        }
        let shells = root.join("mobile/shells/common");
        fs::create_dir_all(&shells).expect("create shell fixture");
        fs::write(shells.join("stasis_mobile_main.c"), b"official shell\n")
            .expect("write shell fixture");
        let mobile_shell_sources = content_hashes(&root.join("mobile/shells"), "mobile/shells")
            .expect("hash shell fixture");
        let manifest = json!({
            "schema": "stasis.release_provenance.v1",
            "release_tag": "nightly-20260719-131",
            "source_commit": "0123456789012345678901234567890123456789",
            "dirty_state": false,
            "development_build": false,
            "compiler": {
                "path": compiler_name,
                "sha256": sha256_file(&root.join(executable_name("stasis"))).expect("hash compiler"),
            },
            "runtime_sources": runtime_sources,
            "mobile_shell_sources": mobile_shell_sources,
            "command_buffer": {"name": "gfx_cmd", "version": 1},
            "backends": ["sdl2"],
            "features": ["aot", "jit", "mobile-aot", "shared-renderer"],
            "dependencies": {
                "cargo_lock_sha256": "fixture",
                "cargo_packages": ["fixture 1.0.0 workspace"],
                "sdl2": "2.30.0",
                "sdl2_image": "2.8.0",
            },
        });
        let manifest_path = root.join(RELEASE_PROVENANCE_NAME);
        write_json_file(&manifest_path, &manifest).expect("write provenance fixture");
        verify_release_provenance(&manifest_path).expect("accept matching release");

        fs::write(
            runtime.join("stasis_graphics.c"),
            b"high-DPI worktree renderer\n",
        )
        .expect("substitute renderer");
        let error = verify_release_provenance(&manifest_path).expect_err("reject mismatch");
        assert!(error.contains("runtime hash mismatch for runtime/stasis_graphics.c"));
        fs::write(
            runtime.join("stasis_graphics.c"),
            "official stasis_graphics.c\n",
        )
        .expect("restore renderer");
        fs::write(
            shells.join("stasis_mobile_main.c"),
            b"substituted mobile shell\n",
        )
        .expect("substitute mobile shell");
        let error = verify_release_provenance(&manifest_path).expect_err("reject shell mismatch");
        assert!(error.contains("mobile shell source hashes do not match"));
        remove_temp(&root);
    }

    #[test]
    fn mobile_package_ids_are_valid_for_java_and_apple() {
        assert_eq!(
            mobile_package_id("mobile_game"),
            "com.stasislang.gamemobilex5fgame"
        );
        assert_eq!(
            mobile_package_id("123-game"),
            "com.stasislang.game123x2dgame"
        );
        for name in ["mobile_game", "123-game"] {
            let package_id = mobile_package_id(name);
            let component = package_id.rsplit('.').next().expect("package component");
            assert!(component
                .chars()
                .next()
                .is_some_and(|value| value.is_ascii_alphabetic()));
            assert!(component.chars().all(|value| value.is_ascii_alphanumeric()));
        }
    }

    #[test]
    fn mobile_shells_are_platform_projects_over_one_shared_runtime() {
        let root = temp_dir("mobile_shells");
        let aot = root.join("aot");
        fs::create_dir_all(aot.join("apk_assets/stasis_game/assets"))
            .expect("create Android AOT assets");
        fs::create_dir_all(aot.join("ios_assets/stasis_game/assets"))
            .expect("create iOS AOT assets");
        fs::write(aot.join("game.o"), b"object").expect("write object");
        fs::write(
            aot.join("apk_assets/stasis_game/assets/manifest.json"),
            "{\"schema\":\"stasis-assets\",\"version\":1,\"assets\":[]}",
        )
        .expect("write Android asset manifest");
        fs::write(
            aot.join("apk_assets/stasis_game/assets/token.txt"),
            "@STASIS_APP_NAME@ @STASIS_PACKAGE_ID@",
        )
        .expect("write Android token asset");
        fs::write(
            aot.join("ios_assets/stasis_game/assets/manifest.json"),
            "{\"schema\":\"stasis-assets\",\"version\":1,\"assets\":[]}",
        )
        .expect("write iOS asset manifest");
        let workspace = Workspace {
            root: root.clone(),
            manifest: ProjectManifest::new("mobile_smoke".to_string()),
        };

        let android = root.join("android-package");
        fs::create_dir_all(&android).expect("create Android staging");
        let provenance = development_provenance().expect("development provenance");
        assemble_mobile_shell(
            &workspace,
            PackageTarget::AndroidArm64,
            &aot,
            &android,
            &provenance,
        )
        .expect("assemble Android shell");
        let android_cmake =
            fs::read_to_string(android.join("android/app/src/main/cpp/CMakeLists.txt"))
                .expect("read Android CMake");
        assert!(android_cmake.contains("stasis_mobile_runtime"));
        assert!(android_cmake.contains("STASIS_AOT_OBJECTS"));
        assert!(!android_cmake.contains("stasis_dynload"));
        let mobile_main = fs::read_to_string(android.join("common/stasis_mobile_main.c"))
            .expect("read shared mobile main");
        assert!(mobile_main.contains("stasis_mobile_runtime_last_entry_result"));
        let runtime_header = fs::read_to_string(android.join("runtime/stasis_mobile_runtime.h"))
            .expect("read shared mobile runtime header");
        assert!(runtime_header.contains("typedef int32_t (*StasisMobileI32Entry)(void)"));
        assert!(android.join("runtime/stasis_display_scale.h").is_file());
        assert!(android.join("runtime/stasis_asset_path.h").is_file());
        assert!(android.join("runtime/stasis_platform_storage.c").is_file());
        assert!(android.join("runtime/stasis_platform_storage.h").is_file());
        assert!(android.join("runtime/stasis_render_contract.h").is_file());
        assert!(android
            .join("runtime/stasis_renderer_lifecycle.h")
            .is_file());
        assert!(android
            .join("android/app/src/main/assets/stasis_game/assets/manifest.json")
            .is_file());
        assert!(android
            .join("android/app/src/main/assets/stasis_game/stasis_asset_base.marker")
            .is_file());
        assert_eq!(
            fs::read_to_string(
                android.join("android/app/src/main/assets/stasis_game/assets/token.txt")
            )
            .expect("read packaged token asset"),
            "@STASIS_APP_NAME@ @STASIS_PACKAGE_ID@"
        );
        let java = fs::read_to_string(
            android.join("android/app/src/main/java/com/stasislang/game/MainActivity.java"),
        )
        .expect("read Android activity");
        assert!(java.contains(".stasis_game.staging"));
        assert!(java.contains("new File(root, \".\")"));
        let jni =
            fs::read_to_string(android.join("android/app/src/main/cpp/stasis_android_assets.c"))
                .expect("read Android asset bridge");
        assert!(
            jni.contains("Java_com_stasislang_gamemobilex5fsmoke_MainActivity_nativeSetAssetRoot")
        );
        assert!(!jni.contains("@STASIS_"));

        let ios = root.join("ios-package");
        fs::create_dir_all(&ios).expect("create iOS staging");
        assemble_mobile_shell(&workspace, PackageTarget::IosArm64, &aot, &ios, &provenance)
            .expect("assemble iOS shell");
        let project = fs::read_to_string(ios.join("ios/StasisMobile.xcodeproj/project.pbxproj"))
            .expect("read Xcode project");
        let config =
            fs::read_to_string(ios.join("ios/StasisMobile.xcconfig")).expect("read Xcode config");
        assert!(project.contains("stasis_mobile_runtime.c in Sources"));
        assert!(!project.contains("stasis_platform_storage.c in Sources"));
        assert!(config.contains("$(PROJECT_DIR)/../aot/game.o"));
        assert!(config.contains("STASIS_GRAPHICS_SDL_ONLY=1"));
        assert!(ios.join("runtime/stasis_display_scale.h").is_file());
        assert!(ios.join("runtime/stasis_asset_path.h").is_file());
        assert!(ios.join("runtime/stasis_render_contract.h").is_file());
        assert!(ios.join("runtime/stasis_renderer_lifecycle.h").is_file());
        assert!(ios.join("runtime/stasis_platform_storage.c").is_file());
        assert!(ios.join("runtime/stasis_platform_storage.h").is_file());
        assert!(config.contains("@executable_path/Frameworks"));
        assert!(project.contains("Embed SDL frameworks"));
        assert!(ios
            .join("ios/StasisMobile/stasis_game/assets/manifest.json")
            .is_file());
        assert!(ios
            .join("ios/StasisMobile/stasis_game/stasis_asset_base.marker")
            .is_file());
        assert!(!project.contains("@STASIS_"));

        remove_temp(&root);
    }

    #[test]
    fn manifest_rejects_paths_that_escape_the_project() {
        let manifest = ProjectManifest {
            output: "../outside".to_string(),
            ..ProjectManifest::new("demo".to_string())
        };
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn init_preflights_reserved_paths_without_partial_writes() {
        let root = temp_dir("preflight");
        fs::create_dir_all(root.join("src")).expect("create source directory");
        fs::write(root.join("src/main.stasis"), "user source\n").expect("write user source");

        let error = create_project(root.clone(), "demo".to_string()).expect_err("reject conflict");
        assert!(error.contains("refusing to overwrite"));
        assert!(!root.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(root.join("src/main.stasis")).expect("read user source"),
            "user source\n"
        );
        remove_temp(&root);
    }

    #[test]
    fn init_preserves_existing_agent_instructions_without_partial_writes() {
        let root = temp_dir("agent_guide_preflight");
        fs::create_dir_all(&root).expect("create project directory");
        fs::write(root.join("AGENTS.md"), "user guidance\n").expect("write user guide");

        let error = create_project(root.clone(), "demo".to_string()).expect_err("reject conflict");
        assert!(error.contains("AGENTS.md"));
        assert!(!root.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(root.join("AGENTS.md")).expect("read user guide"),
            "user guidance\n"
        );
        remove_temp(&root);
    }

    #[test]
    fn init_preserves_existing_claude_pointer_without_partial_writes() {
        let root = temp_dir("claude_guide_preflight");
        fs::create_dir_all(&root).expect("create project directory");
        fs::write(root.join("CLAUDE.md"), "user guidance\n").expect("write user guide");

        let error = create_project(root.clone(), "demo".to_string()).expect_err("reject conflict");
        assert!(error.contains("CLAUDE.md"));
        assert!(!root.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(root.join("CLAUDE.md")).expect("read user guide"),
            "user guidance\n"
        );
        remove_temp(&root);
    }

    #[test]
    fn cli_output_paths_cannot_escape_the_workspace() {
        let root = temp_dir("output_path");
        create_project(root.clone(), "demo".to_string()).expect("create project");
        let workspace = load_workspace(Some(&root)).expect("load workspace");
        assert!(validate_optional_workspace_path(
            &workspace,
            "build output",
            Some(Path::new("../outside"))
        )
        .is_err());
        assert!(validate_optional_workspace_path(
            &workspace,
            "build output",
            Some(Path::new("build/game"))
        )
        .is_ok());
        remove_temp(&root);
    }

    #[test]
    fn failed_package_does_not_publish_or_leave_staging_output() {
        let root = temp_dir("package_failure");
        create_project(root.clone(), "demo".to_string()).expect("create project");
        fs::write(root.join("src/main.stasis"), "function main(: i32 {\n")
            .expect("write invalid source");
        let workspace = load_workspace(Some(&root)).expect("load workspace");

        assert!(package_workspace(
            &workspace,
            PackageTarget::Desktop,
            Some(Path::new("dist/out")),
            true,
        )
        .is_err());
        assert!(!root.join("dist/out").exists());
        assert!(!root.join("dist/.out.staging").exists());
        remove_temp(&root);
    }

    #[cfg(windows)]
    #[test]
    fn default_manifest_output_rejects_symlink_escape() {
        use std::os::windows::fs::symlink_dir;

        let root = temp_dir("symlink_output");
        let outside = temp_dir("symlink_outside");
        create_project(root.clone(), "demo".to_string()).expect("create project");
        fs::create_dir_all(&outside).expect("create outside directory");
        if symlink_dir(&outside, root.join("build")).is_err() {
            remove_temp(&root);
            remove_temp(&outside);
            return;
        }
        let workspace = load_workspace(Some(&root)).expect("load workspace");
        assert!(build_workspace(&workspace, BuildMode::Dev, None)
            .expect_err("reject escaped default output")
            .contains("outside the workspace"));
        remove_temp(&root);
        remove_temp(&outside);
    }
}
