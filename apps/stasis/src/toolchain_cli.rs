use clap::{Args, Parser, Subcommand, ValueEnum};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stasis::{
    load_and_apply_play_data_bindings_for_test, resolve_play_data_binding_paths,
    run_live_in_process, run_live_in_process_with_data, run_play_in_process_with_replay,
    run_play_in_process_with_window_title, run_self_host_aot_cli_with_options, LiveRunConfig,
    PlayReplayConfig, StasisTestRunSession,
};
use stasis_assets::{
    load_project_asset_manifest, prepare_asset_bundle, AssetFormat, AssetLimits, AudioEncoding,
    FontEncoding, SpriteEncoding, DEFAULT_ASSET_MANIFEST_PATH,
};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;
use stasis_compiler::backend::state_migration::MAX_STATE_SNAPSHOT_BYTES;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::frontend::formatter::format_source;
use stasis_compiler::frontend::types::{TYPE_ID_F32, TYPE_ID_I32};
use stasis_compiler::frontend::workshop::{
    find_workshop_references, find_workshop_symbols, load_workshop_edit_workspace,
    plan_workshop_semantic_edits, workshop_direct_import_files, workshop_reachable_files,
    workshop_source_hash, workshop_source_items, write_workshop_semantic_plan,
    write_workshop_semantic_receipt, WorkshopSemanticEdit, WorkshopSemanticEditBatch,
    WorkshopSemanticEditOperation, WorkshopSemanticEditPlan, WorkshopSourceFile,
    WorkshopSourceItemKind, WorkshopSymbolSelector,
};
use stasis_jit::AotTarget;
pub(super) use stasis_runner::live::LiveValidationRequirement as RuntimeValidationRequirement;
use stasis_runner::live::{
    compare_live_validation_values, live_session, LiveCommand, LiveRequest, LiveResponse,
    TerminalBuffer, TerminalInput,
};
use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::io::{self, BufRead, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod dap;
mod gauntlet;
mod headless;
mod live_tui;
mod record;

const MANIFEST_NAME: &str = "stasis.json";
const MANIFEST_VERSION: u32 = 1;
const RELEASE_PROVENANCE_NAME: &str = "stasis_release_provenance.json";
const PACKAGE_PROVENANCE_NAME: &str = "stasis_provenance.json";
const WINDOWS_DESKTOP_PAYLOAD_DIR: &str = "app";
const MOBILE_RUNTIME_FILES: &[&str] = &[
    "CMakeLists.txt",
    "MINIMP3-LICENSE.txt",
    "minimp3.h",
    "minimp3_ex.h",
    "nanosvg.h",
    "nanosvgrast.h",
    "stasis_display_scale.h",
    "stasis_asset_path.h",
    "stasis_render_contract.h",
    "stasis_renderer_lifecycle.h",
    "stasis_performance_metrics.h",
    "stasis_audio_assets.c",
    "stasis_audio_assets.h",
    "stasis_graphics.c",
    "stasis_runner.manifest",
    "stasis_runner_macos.plist.in",
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
const PROJECT_ARCHITECTURE_GUIDE: &str = include_str!("../../../docs/project_architecture.md");
const PROJECT_ARCHITECTURE_NAME: &str = "PROJECT_ARCHITECTURE.md";
const KNOWLEDGE_FILES: &[&str] = &[
    "README.md",
    "a-little-stasis/01-three-entry-points.md",
    "a-little-stasis/02-state-has-owners.md",
    "a-little-stasis/03-a-tick-is-an-ordered-recipe.md",
    "a-little-stasis/04-input-crosses-a-boundary.md",
    "a-little-stasis/05-bounded-storage-is-policy.md",
    "a-little-stasis/06-query-materialize-commit.md",
    "a-little-stasis/07-test-systems-not-balance-numbers.md",
    "a-little-stasis/08-projection-is-not-authority.md",
    "practical-examples/breakout-remove-one-brick-per-collision.md",
    "practical-examples/platformer-land-in-the-crossing-tick.md",
    "practical-examples/pong-score-after-the-ball-crosses-the-goal.md",
    "practical-examples/snake-reject-a-reverse-turn.md",
    "examples/src/breakout_brick.stasis",
    "examples/src/game_patterns.stasis",
    "examples/src/platformer_landing.stasis",
    "examples/src/pong_goal.stasis",
    "examples/src/snake_turn.stasis",
    "examples/stasis.json",
    "examples/tests/breakout_brick.test.stasis",
    "examples/tests/game_patterns.test.stasis",
    "examples/tests/platformer_landing.test.stasis",
    "examples/tests/pong_goal.test.stasis",
    "examples/tests/snake_turn.test.stasis",
    "geometry-and-collision.md",
    "semantic-edit-and-validation.md",
];
const DEFAULT_PROJECT_SOURCE: &str = r#"import "/vendor/stasis/stdlib/stdlib.stasis";
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

struct GameState {
    ticks: i32;
}

global state: GameState;

function main(): i32 {
    state.ticks = 0;
    return 0;
}

function @effects(state) tick(): i32 {
    state.ticks += 1;
    return 0;
}

function @effects(graphics) render(): i32 {
    begin_frame();
    clear(0.05, 0.07, 0.10, 1.0);
    end_frame();
    return 0;
}
"#;
const PROJECT_VSCODE_SETTINGS: &str = r#"{
  "[stasis]": {
    "editor.defaultFormatter": "stasislang.stasis",
    "editor.formatOnSave": true
  }
}
"#;
const PROJECT_VSCODE_EXTENSIONS: &str = r#"{
  "recommendations": [
    "stasislang.stasis"
  ]
}
"#;
const PROJECT_PRE_COMMIT_HOOK: &str = r#"#!/bin/sh
set -eu

if ! command -v stasis >/dev/null 2>&1; then
    echo "Commit blocked: stasis is not available on PATH; install Stasis and run 'stasis format'." >&2
    exit 1
fi

echo "Stasis pre-commit: checking canonical source format"
if ! stasis format --check; then
    echo "Stasis pre-commit: formatting source before blocking this commit"
    if ! stasis format; then
        echo "Commit blocked: 'stasis format' failed." >&2
        exit 1
    fi
    echo "Commit blocked: review and stage the formatting changes, then commit again." >&2
    exit 1
fi

if ! git diff --quiet -- ':(glob)**/*.stasis'; then
    echo "Commit blocked: stage the formatted Stasis changes, then commit again." >&2
    exit 1
fi
"#;
const TARGET_BUILD_HELP: &str = r#"Build targets:
  Windows, Linux, or macOS (current host)
    stasis build --mode release
    stasis package --target desktop

  Web (WebAssembly browser bundle)
    stasis package --target web

  Android devices (64-bit ARM app project)
    stasis package-mobile --target android-arm64

  Android emulator (x86-64 test app project)
    stasis package-mobile --target android-x86_64 --development-build

  iPhone and iPad (64-bit ARM app project)
    stasis package-mobile --target ios-arm64

Desktop builds target the operating system running stasis. Web output is a static bundle to
serve over HTTP. Mobile commands create Gradle or Xcode projects for final SDK builds; source
toolchains create local release packages when official provenance is absent."#;
const COMMANDS: &[&str] = &[
    "new",
    "init",
    "fmt",
    "format",
    "check",
    "test",
    "ai",
    "gauntlet",
    "validate",
    "run",
    "record",
    "lsp",
    "dap",
    "tui",
    "build",
    "package",
    "package-mobile",
    "inspect",
    "replay",
    "verify",
    "version",
    "editor-info",
    "env",
    "vendor",
    "symbol",
    "help",
    "__validate-runtime",
];

#[derive(Debug, Parser)]
#[command(
    name = "stasis",
    version,
    about = "The batteries-included Stasis toolchain",
    long_about = "Create, format, check, test, run, live-edit, build, inspect, and package Stasis projects without invoking Cargo.",
    after_help = TARGET_BUILD_HELP
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
    /// Apply the canonical Stasis source layout.
    #[command(visible_alias = "format")]
    Fmt {
        #[arg(long)]
        check: bool,
        /// Read source from stdin and write the formatted source to stdout.
        #[arg(long, conflicts_with_all = ["check", "paths"])]
        stdin: bool,
        #[arg(value_name = "PATH")]
        paths: Vec<PathBuf>,
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
    /// Create, run, observe, and recover an autonomous live-game Gauntlet.
    Gauntlet {
        #[command(subcommand)]
        command: gauntlet::GauntletCommand,
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
        /// Execute exactly this many simulation ticks after main().
        #[arg(long, default_value_t = 0, value_name = "COUNT")]
        ticks: u64,
        /// Run bounded ticks without wall-clock pacing (headless only).
        #[arg(long)]
        fast_forward: bool,
    },
    /// Render a deterministic fixed-rate headless PNG sequence or MP4 recording.
    Record {
        #[command(flatten)]
        args: record::RecordArgs,
    },
    /// Run the persistent Stasis language server.
    Lsp {
        /// Communicate with the editor over standard input and output.
        #[arg(long)]
        stdio: bool,
    },
    /// Run the Stasis debug adapter over the Debug Adapter Protocol.
    Dap {
        /// Communicate with the editor over standard input and output.
        #[arg(long)]
        stdio: bool,
    },
    /// Run one graphical entry with hot swap and the live-workspace TUI.
    Tui {
        /// Override the entry declared in stasis.json.
        #[arg(value_name = "ENTRY")]
        entry: Option<PathBuf>,
        /// Override the watched directory; defaults to the entry file's parent.
        #[arg(long, value_name = "PATH")]
        watch_dir: Option<PathBuf>,
        /// Override the project data and struct-metadata files.
        #[arg(long, num_args = 2, value_names = ["DATA_PATH", "STRUCT_META_PATH"])]
        data_bind: Vec<PathBuf>,
        /// Read live commands from a deterministic script instead of opening the TUI.
        #[arg(long, value_name = "PATH")]
        live_script: Option<PathBuf>,
        /// Emit versioned live response envelopes as JSON lines.
        #[arg(long)]
        live_json: bool,
        /// Read live commands from stdin and emit response envelopes as JSON lines.
        #[arg(long, conflicts_with = "live_script")]
        live_stdio: bool,
        #[arg(long, default_value_t = 16_000)]
        tick_sleep_us: u64,
        #[arg(long)]
        ticks: Option<u64>,
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
        /// Force a visibly labeled development package.
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
        /// Force a visibly labeled development package.
        #[arg(long)]
        development_build: bool,
        /// Select named Stasis functions for bounded mobile AOT profiling.
        #[arg(long, value_delimiter = ',', value_name = "NAME[,NAME...]")]
        profile_functions: Vec<String>,
        #[arg(long, default_value_t = 120)]
        profile_warmup_frames: u32,
        #[arg(long, default_value_t = 300)]
        profile_sample_frames: u32,
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
    /// Replay one recorded HostFrame-diff session through tick and render.
    Replay {
        #[arg(value_name = "RECORDING")]
        recording: PathBuf,
        /// Override the manifest entry with a project-relative .stasis file.
        #[arg(long, value_name = "ENTRY")]
        entry: Option<PathBuf>,
        #[arg(long, default_value_t = 16_000)]
        tick_sleep_us: u64,
    },
    /// Replay verification is reserved until the replay runtime lands.
    Verify,
    /// Print the installed toolchain version.
    Version,
    /// Report the editor protocols and the sibling graphics runtime identity.
    EditorInfo,
    /// Print toolchain, workspace, cache, and offline capability information.
    Env,
    /// Inspect or update the checked-in Stasis vendor snapshot.
    Vendor {
        #[command(subcommand)]
        command: VendorCommand,
    },
    /// Find and transactionally edit compiler-owned semantic symbols.
    Symbol {
        #[command(subcommand)]
        command: SymbolCommand,
    },
}

#[derive(Debug, Subcommand)]
enum VendorCommand {
    /// Compare the checked-in snapshot with its manifest and this executable.
    Status,
    /// Atomically replace the checked-in snapshot with this executable's sources.
    Update,
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
    Web,
    AndroidArm64,
    #[value(name = "android-x86_64")]
    AndroidX86_64,
    IosArm64,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum MobilePackageTarget {
    AndroidArm64,
    #[value(name = "android-x86_64")]
    AndroidX86_64,
    IosArm64,
}

impl MobilePackageTarget {
    fn package_target(self) -> PackageTarget {
        match self {
            Self::AndroidArm64 => PackageTarget::AndroidArm64,
            Self::AndroidX86_64 => PackageTarget::AndroidX86_64,
            Self::IosArm64 => PackageTarget::IosArm64,
        }
    }
}

impl PackageTarget {
    fn as_str(self) -> &'static str {
        match self {
            Self::Desktop => "desktop",
            Self::Web => "web",
            Self::AndroidArm64 => "android-arm64",
            Self::AndroidX86_64 => "android-x86_64",
            Self::IosArm64 => "ios-arm64",
        }
    }

    fn is_android(self) -> bool {
        matches!(self, Self::AndroidArm64 | Self::AndroidX86_64)
    }

    fn is_mobile(self) -> bool {
        matches!(
            self,
            Self::AndroidArm64 | Self::AndroidX86_64 | Self::IosArm64
        )
    }

    fn android_abi(self) -> Option<&'static str> {
        match self {
            Self::AndroidArm64 => Some("arm64-v8a"),
            Self::AndroidX86_64 => Some("x86_64"),
            Self::Desktop | Self::Web | Self::IosArm64 => None,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    stdlib: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    vendor: Option<VendorManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    android: Option<AndroidProjectManifest>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<ProjectCapabilities>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    web: Option<WebProjectManifest>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
struct ProjectCapabilities {
    #[serde(default)]
    network: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct WebProjectManifest {
    #[serde(default)]
    entry: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    loading_font: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct VendorManifest {
    stasis: StasisVendorManifest,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct StasisVendorManifest {
    release_id: String,
    sha256: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct AndroidProjectManifest {
    application_id: String,
    label: String,
    orientation: String,
    version_code: u32,
    version_name: String,
}

impl ProjectManifest {
    fn new(name: String) -> Self {
        Self {
            manifest_version: MANIFEST_VERSION,
            name,
            entry: "src/main.stasis".to_string(),
            tests: "tests".to_string(),
            output: "build".to_string(),
            stdlib: None,
            vendor: None,
            android: None,
            capabilities: None,
            web: None,
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
        if let Some(web) = &self.web {
            if !web.entry.is_empty() {
                validate_relative_path("web.entry", Path::new(&web.entry))?;
            }
        }
        if self
            .stdlib
            .as_deref()
            .is_some_and(|value| value != "toolchain")
        {
            return Err("stdlib must be 'toolchain' when specified".to_string());
        }
        if self.stdlib.is_some() && self.vendor.is_some() {
            return Err("stdlib and vendor modes cannot be enabled together".to_string());
        }
        if let Some(vendor) = &self.vendor {
            if vendor.stasis.release_id.trim().is_empty() {
                return Err("vendor.stasis.release_id must not be empty".to_string());
            }
            if vendor.stasis.sha256.len() != 64
                || !vendor
                    .stasis
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            {
                return Err("vendor.stasis.sha256 must be a lowercase SHA-256 digest".to_string());
            }
        }
        if let Some(android) = &self.android {
            validate_android_application_id(&android.application_id)?;
            if android.label.trim().is_empty()
                || android.label != android.label.trim()
                || !android.label.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b' ' | b'_' | b'-' | b'.')
                })
            {
                return Err(
                    "android label must use letters, numbers, spaces, _, -, or .".to_string(),
                );
            }
            if !matches!(
                android.orientation.as_str(),
                "unspecified" | "sensorLandscape" | "sensorPortrait" | "fullSensor"
            ) {
                return Err(
                    "android orientation must be unspecified, sensorLandscape, sensorPortrait, or fullSensor"
                        .to_string(),
                );
            }
            if android.version_code == 0 {
                return Err("android version_code must be positive".to_string());
            }
            if android.version_name.is_empty()
                || !android.version_name.bytes().all(|byte| {
                    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+')
                })
            {
                return Err("android version_name is invalid".to_string());
            }
        }
        if let Some(web) = &self.web {
            if let Some(path) = web.loading_font.as_deref() {
                normalize_web_loading_font_path(path)?;
            }
        }
        Ok(())
    }
}

fn validate_android_application_id(value: &str) -> Result<(), String> {
    let parts = value.split('.').collect::<Vec<_>>();
    if parts.len() < 2
        || parts.iter().any(|part| {
            part.is_empty()
                || !part.as_bytes()[0].is_ascii_alphabetic()
                || !part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        })
    {
        return Err(format!("invalid android application_id: {value}"));
    }
    Ok(())
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
    let raw_output = matches!(&parsed.command, ToolchainCommand::Fmt { stdin: true, .. });
    let started_at = (!parsed.json
        && matches!(
            &parsed.command,
            ToolchainCommand::Build { .. }
                | ToolchainCommand::Package { .. }
                | ToolchainCommand::PackageMobile { .. }
        ))
    .then(Instant::now);
    match execute(parsed.command, parsed.workspace, parsed.json) {
        Ok(mut result) => {
            if let Some(started_at) = started_at {
                append_elapsed_confirmation(&mut result.human, started_at.elapsed());
            }
            if parsed.json {
                println!(
                    "{}",
                    json!({
                        "ok": true,
                        "command": command_name,
                        "result": result.data,
                    })
                );
            } else if raw_output {
                print!("{}", result.human);
                let _ = io::stdout().flush();
            } else if !result.human.is_empty() {
                println!("{}", result.human);
            }
            Some(result.code)
        }
        Err(message) => {
            if parsed.json {
                let mut error = json!({
                    "ok": false,
                    "command": command_name,
                    "code": "command_failed",
                    "message": &message,
                });
                if let Some(payload) = message
                    .strip_prefix(crate::release_assets::ASSET_DIAGNOSTIC_PREFIX)
                    .and_then(|payload| serde_json::from_str::<Value>(payload).ok())
                {
                    error["code"] = json!("asset_validation_failed");
                    error["message"] = json!("asset validation failed");
                    error["diagnostics"] = payload;
                }
                eprintln!("{error}");
            } else {
                eprintln!("stasis {command_name}: {message}");
            }
            Some(1)
        }
    }
}

fn append_elapsed_confirmation(output: &mut String, elapsed: Duration) {
    if !output.is_empty() {
        output.push('\n');
    }
    output.push_str("Completed in ");
    if elapsed.as_secs() >= 60 {
        let minutes = elapsed.as_secs() / 60;
        let seconds = elapsed.as_secs_f64() - minutes as f64 * 60.0;
        output.push_str(&format!("{minutes}m {seconds:.1}s."));
    } else if elapsed.as_secs() >= 1 {
        output.push_str(&format!("{:.2}s.", elapsed.as_secs_f64()));
    } else {
        output.push_str(&format!("{}ms.", elapsed.as_millis()));
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
        ToolchainCommand::Gauntlet { .. } => "gauntlet",
        ToolchainCommand::Validate { .. } => "validate",
        ToolchainCommand::ValidateRuntime { .. } => "__validate-runtime",
        ToolchainCommand::Run { .. } => "run",
        ToolchainCommand::Record { .. } => "record",
        ToolchainCommand::Lsp { .. } => "lsp",
        ToolchainCommand::Dap { .. } => "dap",
        ToolchainCommand::Tui { .. } => "tui",
        ToolchainCommand::Build { .. } => "build",
        ToolchainCommand::Package { .. } => "package",
        ToolchainCommand::PackageMobile { .. } => "package-mobile",
        ToolchainCommand::Inspect { .. } => "inspect",
        ToolchainCommand::Replay { .. } => "replay",
        ToolchainCommand::Verify => "verify",
        ToolchainCommand::Version => "version",
        ToolchainCommand::EditorInfo => "editor-info",
        ToolchainCommand::Env => "env",
        ToolchainCommand::Vendor { .. } => "vendor",
        ToolchainCommand::Symbol { .. } => "symbol",
    }
}

fn execute(
    command: ToolchainCommand,
    workspace_arg: Option<PathBuf>,
    json_output: bool,
) -> Result<CommandResult, String> {
    match command {
        ToolchainCommand::New { name, dir } => create_new_project(dir.unwrap_or_else(|| PathBuf::from(&name)), name),
        ToolchainCommand::Init { dir, name } => {
            let root = absolute_path(&dir)?;
            let inferred = root
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or("stasis_game")
                .to_string();
            create_project(root, name.unwrap_or(inferred))
        }
        ToolchainCommand::Gauntlet { command } => {
            gauntlet::execute(command, workspace_arg.as_deref(), json_output)
        }
        ToolchainCommand::Version => Ok(version_result()),
        ToolchainCommand::EditorInfo => editor_info_result(),
        ToolchainCommand::Env => env_result(workspace_arg.as_deref()),
        ToolchainCommand::Verify => Err(
            "verify is unavailable in toolchain 0.1; no replay verification contract is implemented"
                .to_string(),
        ),
        ToolchainCommand::Fmt {
            stdin: true,
            check,
            paths,
        } => {
            debug_assert!(!check);
            debug_assert!(paths.is_empty());
            if workspace_arg.is_some() {
                Err("--workspace cannot be combined with format --stdin".to_string())
            } else if json_output {
                Err("--json cannot be combined with format --stdin; stdout is the formatted source".to_string())
            } else {
                let mut source = String::new();
                io::stdin()
                    .read_to_string(&mut source)
                    .map_err(|error| format!("failed reading source from stdin: {error}"))?;
                let formatted = format_source(&source)?;
                Ok(CommandResult::success(formatted, json!({"source": "stdin"})))
            }
        }
        ToolchainCommand::Fmt {
            check,
            paths,
            stdin: false,
        } if !paths.is_empty() => {
            if workspace_arg.is_some() {
                Err("--workspace cannot be combined with explicit format paths".to_string())
            } else {
                format_explicit_paths(&paths, check)
            }
        }
        other => {
            let workspace_path = workspace_arg.as_deref().or(match &other {
                ToolchainCommand::Tui { entry, .. } => entry.as_deref(),
                _ => None,
            });
            let vendor_gate = match &other {
                ToolchainCommand::Vendor { .. } => VendorGate::Inspect,
                _ => VendorGate::Sync,
            };
            let workspace = load_workspace_with_vendor_gate(workspace_path, vendor_gate)?;
            match other {
                ToolchainCommand::Fmt { check, .. } => format_workspace(&workspace, check),
                ToolchainCommand::Check => check_workspace(&workspace),
                ToolchainCommand::Test { path } => {
                    validate_optional_workspace_path(&workspace, "test path", path.as_deref())?;
                    test_workspace(&workspace, path.as_deref())
                }
                ToolchainCommand::Ai { prompt } => run_workspace_ai(&workspace, &prompt),
                ToolchainCommand::Gauntlet { .. } => {
                    unreachable!("gauntlet commands route before workspace discovery")
                }
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
                    ticks,
                    fast_forward,
                } => {
                    if watch && json_output {
                        Err("--json cannot be combined with --watch; watch mode is an unbounded event stream".to_string())
                    } else if watch && headless {
                        Err("--headless cannot be combined with --watch; watch mode uses the graphical hot-swap runner".to_string())
                    } else if watch && ticks != 0 {
                        Err("--ticks cannot be combined with --watch; use play --ticks for a bounded graphical run".to_string())
                    } else if watch && fast_forward {
                        Err("--fast-forward cannot be combined with --watch".to_string())
                    } else if fast_forward && ticks == 0 {
                        Err("--fast-forward requires --ticks greater than zero".to_string())
                    } else if watch {
                        run_workspace_watch(&workspace)
                    } else {
                        run_workspace(&workspace, headless, ticks, fast_forward)
                    }
                }
                ToolchainCommand::Record { args } => record::execute(&workspace, args),
                ToolchainCommand::Replay {
                    recording,
                    entry,
                    tick_sleep_us,
                } => replay_workspace(
                    &workspace,
                    &recording,
                    entry.as_deref(),
                    tick_sleep_us,
                ),
                ToolchainCommand::Lsp { stdio } => {
                    if json_output {
                        Err("--json cannot be combined with lsp; LSP owns stdout".to_string())
                    } else {
                        let _ = stdio;
                        stasis_lsp::run_stdio(&workspace.root)?;
                        Ok(CommandResult::success(String::new(), json!({})))
                    }
                }
                ToolchainCommand::Dap { stdio } => {
                    if json_output {
                        Err("--json cannot be combined with dap; DAP owns stdout".to_string())
                    } else {
                        let _ = stdio;
                        dap::run(&workspace)?;
                        Ok(CommandResult::success(String::new(), json!({})))
                    }
                }
                ToolchainCommand::Tui {
                    entry,
                    watch_dir,
                    data_bind,
                    live_script,
                    live_json,
                    live_stdio,
                    tick_sleep_us,
                    ticks,
                } => {
                    if json_output {
                        Err("--json cannot be combined with tui; use --live-json for the response stream".to_string())
                    } else {
                        let entry = entry
                            .as_deref()
                            .unwrap_or_else(|| Path::new(&workspace.manifest.entry));
                        run_workspace_tui(
                            &workspace,
                            entry,
                            watch_dir.as_deref(),
                            &data_bind,
                            live_script.as_deref(),
                            live_json,
                            live_stdio,
                            tick_sleep_us,
                            ticks,
                        )
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
                    profile_functions,
                    profile_warmup_frames,
                    profile_sample_frames,
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
                        &profile_functions,
                        profile_warmup_frames,
                        profile_sample_frames,
                    )
                }
                ToolchainCommand::Inspect {
                    capacities,
                    mobile_budget_bytes,
                } => inspect_workspace(&workspace, &capacities, mobile_budget_bytes),
                ToolchainCommand::Vendor { command } => vendor_command(&workspace, command),
                ToolchainCommand::Symbol { command } => symbol_workspace(&workspace, command),
                _ => Err("unsupported command routing".to_string()),
            }
        }
    }
}

fn create_new_project(path: PathBuf, name: String) -> Result<CommandResult, String> {
    create_project_with_options(path, name, true)
}

fn create_project(path: PathBuf, name: String) -> Result<CommandResult, String> {
    create_project_with_options(path, name, false)
}

fn create_project_with_options(
    path: PathBuf,
    name: String,
    initialize_git: bool,
) -> Result<CommandResult, String> {
    validate_project_name(&name)?;
    if initialize_git {
        require_git()?;
    }
    let root = absolute_path(&path)?;
    let vendor_manifest = current_vendor_manifest()?;
    let manifest_path = root.join(MANIFEST_NAME);
    let mut reserved_paths = vec![
        manifest_path.clone(),
        root.join("AGENTS.md"),
        root.join("CLAUDE.md"),
        root.join(PROJECT_ARCHITECTURE_NAME),
        root.join("src/main.stasis"),
        root.join("tests/main.test.stasis"),
        root.join("vendor/stasis"),
        root.join(".vscode/settings.json"),
        root.join(".vscode/extensions.json"),
    ];
    if initialize_git {
        reserved_paths.push(root.join(".githooks/pre-commit"));
    }
    let vscode_directory = root.join(".vscode");
    if vscode_directory.exists() {
        let metadata = fs::symlink_metadata(&vscode_directory).map_err(|error| {
            format!("failed to inspect {}: {error}", vscode_directory.display())
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing to write editor settings through {}",
                vscode_directory.display()
            ));
        }
    }
    let vendor_directory = root.join("vendor");
    if vendor_directory.exists() {
        let metadata = fs::symlink_metadata(&vendor_directory).map_err(|error| {
            format!("failed to inspect {}: {error}", vendor_directory.display())
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing to write vendored packages through {}",
                vendor_directory.display()
            ));
        }
    }
    for reserved in &reserved_paths {
        if reserved.exists() {
            return Err(format!("refusing to overwrite {}", reserved.display()));
        }
    }
    fs::create_dir_all(root.join("src"))
        .map_err(|error| format!("failed to create {}: {error}", root.display()))?;
    fs::create_dir_all(root.join("tests"))
        .map_err(|error| format!("failed to create tests directory: {error}"))?;
    let mut manifest = ProjectManifest::new(name.clone());
    manifest.vendor = Some(vendor_manifest);
    write_manifest(&manifest_path, &manifest)?;
    let vendor_package = root.join("vendor/stasis");
    copy_bundled_vendor_package(&vendor_package)?;
    write_new_file(&root.join("AGENTS.md"), PROJECT_AGENT_GUIDE)?;
    write_new_file(&root.join("CLAUDE.md"), PROJECT_CLAUDE_GUIDE)?;
    write_new_file(
        &root.join(PROJECT_ARCHITECTURE_NAME),
        PROJECT_ARCHITECTURE_GUIDE,
    )?;
    write_new_file(&root.join("src/main.stasis"), DEFAULT_PROJECT_SOURCE)?;
    write_new_file(
        &root.join("tests/main.test.stasis"),
        "test `new project is ready`(): bool {\r\n    return 1 == 1;\r\n}\r\n",
    )?;
    fs::create_dir_all(root.join(".vscode"))
        .map_err(|error| format!("failed to create VS Code settings directory: {error}"))?;
    write_new_file(&root.join(".vscode/settings.json"), PROJECT_VSCODE_SETTINGS)?;
    write_new_file(
        &root.join(".vscode/extensions.json"),
        PROJECT_VSCODE_EXTENSIONS,
    )?;
    if initialize_git {
        let hook = root.join(".githooks/pre-commit");
        fs::create_dir_all(hook.parent().expect("hook parent"))
            .map_err(|error| format!("failed to create {}: {error}", hook.display()))?;
        write_new_file(&hook, PROJECT_PRE_COMMIT_HOOK)?;
        make_executable(&hook)?;
        initialize_git_hooks(&root)?;
    }
    Ok(CommandResult::success(
        format!("created {} at {}", name, root.display()),
        json!({
            "name": name,
            "root": display_path(&root),
            "manifest": MANIFEST_NAME,
            "format_hook": initialize_git,
        }),
    ))
}

fn require_git() -> Result<(), String> {
    let output = Command::new("git")
        .arg("--version")
        .output()
        .map_err(|error| format!("stasis new requires Git to install its format hook: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err("stasis new requires Git to install its format hook".to_string())
    }
}

fn initialize_git_hooks(root: &Path) -> Result<(), String> {
    for args in [
        &["init", "--quiet"][..],
        &["config", "--local", "core.hooksPath", ".githooks"][..],
    ] {
        let output = Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .map_err(|error| format!("failed to run git {}: {error}", args.join(" ")))?;
        if !output.status.success() {
            let diagnostic = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(format!(
                "failed to run git {}{}",
                args.join(" "),
                if diagnostic.is_empty() {
                    String::new()
                } else {
                    format!(": {diagnostic}")
                }
            ));
        }
    }
    Ok(())
}

fn make_executable(path: &Path) -> Result<(), String> {
    #[cfg(not(unix))]
    let _ = path;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;

        let mut permissions = fs::metadata(path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?
            .permissions();
        permissions.set_mode(0o755);
        fs::set_permissions(path, permissions)
            .map_err(|error| format!("failed to make {} executable: {error}", path.display()))?;
    }
    Ok(())
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum VendorGate {
    Sync,
    Inspect,
}

fn load_workspace(explicit: Option<&Path>) -> Result<Workspace, String> {
    load_workspace_with_vendor_gate(explicit, VendorGate::Sync)
}

fn load_workspace_with_vendor_gate(
    explicit: Option<&Path>,
    vendor_gate: VendorGate,
) -> Result<Workspace, String> {
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
    let discovered_root = find_workspace_root(&start_dir).ok_or_else(|| {
        format!(
            "no {MANIFEST_NAME} found from {}; run 'stasis init' first",
            start_dir.display()
        )
    })?;
    let root = canonical_workspace_root(&discovered_root)?;
    let bytes = fs::read(root.join(MANIFEST_NAME))
        .map_err(|error| format!("failed to read {MANIFEST_NAME}: {error}"))?;
    let mut manifest: ProjectManifest = serde_json::from_slice(&bytes)
        .map_err(|error| format!("invalid {MANIFEST_NAME}: {error}"))?;
    manifest.validate()?;
    if let Some(web) = manifest.web.as_ref() {
        if let Some(path) = web.loading_font.as_deref() {
            let normalized = normalize_web_loading_font_path(path)?;
            let font = root.join(&normalized);
            let assets_root = root.join("assets").canonicalize().ok();
            let resolved_font = font.canonicalize().ok();
            if !font.is_file()
                || !assets_root.as_deref().is_some_and(|assets| {
                    resolved_font
                        .as_deref()
                        .is_some_and(|resolved| resolved.starts_with(assets))
                })
            {
                return Err(format!(
                    "web.loading_font must name an existing file under assets: {}",
                    font.display()
                ));
            }
        }
    }
    if manifest.stdlib.as_deref() == Some("toolchain") {
        sync_toolchain_stdlib(&root)?;
    }
    if vendor_gate != VendorGate::Inspect {
        reconcile_project_vendor(&root, &mut manifest)?;
    }
    Ok(Workspace { root, manifest })
}

fn sync_toolchain_stdlib(workspace_root: &Path) -> Result<(), String> {
    let stdlib = bundled_stdlib_dir()?;
    let source = stdlib
        .parent()
        .ok_or_else(|| format!("bundled stdlib has no src parent: {}", stdlib.display()))?
        .to_path_buf();
    let fingerprint = directory_sha256(&source)?;
    let cache_root = workspace_root.join(".stasis_cache/toolchain");
    let target = cache_root.join("src");
    let marker = target.join(".toolchain-sha256");
    if fs::read_to_string(&marker)
        .ok()
        .is_some_and(|value| value.trim() == fingerprint)
    {
        return Ok(());
    }

    fs::create_dir_all(&cache_root)
        .map_err(|error| format!("failed to create {}: {error}", cache_root.display()))?;
    let suffix = std::process::id();
    let staging = cache_root.join(format!("src.sync-{suffix}"));
    let backup = cache_root.join(format!("src.previous-{suffix}"));
    for path in [&staging, &backup] {
        if path.exists() {
            fs::remove_dir_all(path)
                .map_err(|error| format!("failed to clear {}: {error}", path.display()))?;
        }
    }
    copy_dir_if_exists(&source, &staging)?;
    fs::write(
        staging.join(".toolchain-sha256"),
        format!("{fingerprint}\n"),
    )
    .map_err(|error| format!("failed to write stdlib fingerprint: {error}"))?;

    if target.exists() {
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("failed to inspect {}: {error}", target.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing to replace non-directory stdlib cache {}",
                target.display()
            ));
        }
        fs::rename(&target, &backup).map_err(|error| {
            format!(
                "failed to stage existing stdlib cache {}: {error}",
                target.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(&staging, &target) {
        if backup.exists() {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!(
            "failed to publish toolchain stdlib cache {}: {error}",
            target.display()
        ));
    }
    if backup.exists() {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("failed to clear {}: {error}", backup.display()))?;
    }
    Ok(())
}

fn collect_mapped_files(
    root: &Path,
    directory: &Path,
    prefix: &Path,
    files: &mut Vec<(PathBuf, PathBuf)>,
) -> Result<(), String> {
    let mut entries = fs::read_dir(directory)
        .map_err(|error| format!("failed to read {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to enumerate {}: {error}", directory.display()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let kind = entry
            .file_type()
            .map_err(|error| format!("failed to inspect {}: {error}", entry.path().display()))?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_mapped_files(root, &entry.path(), prefix, files)?;
        } else if kind.is_file() {
            let physical = entry.path();
            let relative = physical
                .strip_prefix(root)
                .map_err(|_| format!("file escaped mapped directory {}", root.display()))?;
            files.push((prefix.join(relative), physical));
        }
    }
    Ok(())
}

fn mapped_files_sha256(mut files: Vec<(PathBuf, PathBuf)>) -> Result<String, String> {
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let mut digest = Sha256::new();
    for (relative, physical) in files {
        digest.update(relative.to_string_lossy().replace('\\', "/").as_bytes());
        digest.update([0]);
        let bytes = fs::read(&physical)
            .map_err(|error| format!("failed to read {}: {error}", physical.display()))?;
        digest.update(bytes);
        digest.update([0]);
    }
    Ok(format!("{:x}", digest.finalize()))
}

fn directory_sha256(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_mapped_files(root, root, Path::new(""), &mut files)?;
    mapped_files_sha256(files)
}

fn bundled_vendor_sha256() -> Result<String, String> {
    let stdlib = bundled_stdlib_dir()?;
    let docs = bundled_knowledge_docs_dir()?;
    let mut files = Vec::new();
    collect_mapped_files(&stdlib, &stdlib, Path::new("stdlib"), &mut files)?;
    collect_mapped_files(&docs, &docs, Path::new("docs"), &mut files)?;
    mapped_files_sha256(files)
}

fn current_release_id() -> &'static str {
    option_env!("STASIS_RELEASE_ID").unwrap_or("development")
}

fn current_vendor_manifest() -> Result<VendorManifest, String> {
    Ok(VendorManifest {
        stasis: StasisVendorManifest {
            release_id: current_release_id().to_string(),
            sha256: bundled_vendor_sha256()?,
        },
    })
}

fn copy_bundled_vendor_package(destination: &Path) -> Result<(), String> {
    copy_dir_if_exists(&bundled_stdlib_dir()?, &destination.join("stdlib"))?;
    copy_dir_if_exists(&bundled_knowledge_docs_dir()?, &destination.join("docs"))
}

fn validate_vendor_sources(source_root: &Path) -> Result<(), String> {
    let mut files = Vec::new();
    collect_stasis_files(source_root, &mut files)?;
    for file in files {
        let source = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let formatted = format_source(&source)
            .map_err(|error| format!("failed to format {}: {error}", file.display()))?;
        if formatted != source {
            return Err(format!(
                "selected toolchain contains noncanonical vendored source {}",
                file.display()
            ));
        }
        let relative = file
            .strip_prefix(source_root)
            .map_err(|_| format!("vendor source escaped {}", source_root.display()))?;
        let logical = format!(
            "vendor/stasis/{}",
            relative.to_string_lossy().replace('\\', "/")
        );
        let imports = stasis_compiler::frontend::module_graph::parse_imports(&logical, &source)
            .map_err(|diagnostic| format!("invalid vendor import in {logical}: {diagnostic:?}"))?;
        for import in imports {
            let relative_target =
                import
                    .target
                    .strip_prefix("vendor/stasis/")
                    .ok_or_else(|| {
                        format!(
                            "vendor import '{}' from {logical} escapes the Stasis package",
                            import.path
                        )
                    })?;
            if !source_root.join(relative_target).is_file() {
                return Err(format!(
                    "vendor import '{}' from {logical} is missing its target",
                    import.path
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug)]
struct VendorStatus {
    recorded: Option<StasisVendorManifest>,
    installed: StasisVendorManifest,
    actual_sha256: Option<String>,
    local_changes: bool,
    update_available: bool,
}

fn inspect_project_vendor(
    workspace_root: &Path,
    manifest: &ProjectManifest,
) -> Result<VendorStatus, String> {
    let installed = current_vendor_manifest()?.stasis;
    let recorded = manifest.vendor.as_ref().map(|vendor| vendor.stasis.clone());
    let vendor_package = workspace_root.join("vendor/stasis");
    let actual_sha256 = if vendor_package.exists() {
        let metadata = fs::symlink_metadata(&vendor_package)
            .map_err(|error| format!("failed to inspect {}: {error}", vendor_package.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!(
                "refusing to inspect vendored sources through {}",
                vendor_package.display()
            ));
        }
        Some(directory_sha256(&vendor_package)?)
    } else {
        None
    };
    let local_changes = recorded
        .as_ref()
        .is_some_and(|recorded| actual_sha256.as_deref() != Some(recorded.sha256.as_str()));
    let update_available = recorded
        .as_ref()
        .is_none_or(|recorded| recorded.sha256 != installed.sha256);
    Ok(VendorStatus {
        recorded,
        installed,
        actual_sha256,
        local_changes,
        update_available,
    })
}

fn reconcile_project_vendor(
    workspace_root: &Path,
    manifest: &mut ProjectManifest,
) -> Result<(), String> {
    if manifest.vendor.is_none() {
        return Ok(());
    }
    let status = inspect_project_vendor(workspace_root, manifest)?;
    if !status.update_available
        && status.actual_sha256.as_deref() == Some(status.installed.sha256.as_str())
    {
        return Ok(());
    }
    update_vendor_snapshot(workspace_root, manifest)?;
    Ok(())
}

fn transaction_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos());
    format!("{}-{nanos}", std::process::id())
}

fn serialized_manifest(manifest: &ProjectManifest) -> Result<String, String> {
    let mut contents = serde_json::to_string_pretty(manifest)
        .map_err(|error| format!("failed to serialize manifest: {error}"))?;
    contents.push('\n');
    Ok(contents)
}

fn update_vendor_snapshot(
    workspace_root: &Path,
    manifest: &mut ProjectManifest,
) -> Result<bool, String> {
    let status = inspect_project_vendor(workspace_root, manifest)?;
    if !status.update_available
        && status.actual_sha256.as_deref() == Some(status.installed.sha256.as_str())
    {
        return Ok(false);
    }
    let vendor_root = workspace_root.join("vendor");
    fs::create_dir_all(&vendor_root)
        .map_err(|error| format!("failed to create {}: {error}", vendor_root.display()))?;
    let metadata = fs::symlink_metadata(&vendor_root)
        .map_err(|error| format!("failed to inspect {}: {error}", vendor_root.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(format!(
            "refusing to write vendored packages through {}",
            vendor_root.display()
        ));
    }

    let suffix = transaction_suffix();
    let target = vendor_root.join("stasis");
    let staging = vendor_root.join(format!(".stasis.sync-{suffix}"));
    let backup = vendor_root.join(format!(".stasis.previous-{suffix}"));
    let manifest_path = workspace_root.join(MANIFEST_NAME);
    let manifest_staging = workspace_root.join(format!("{MANIFEST_NAME}.vendor-sync-{suffix}"));
    let manifest_backup = workspace_root.join(format!("{MANIFEST_NAME}.vendor-previous-{suffix}"));

    if let Err(error) = copy_bundled_vendor_package(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    if let Err(error) = validate_vendor_sources(&staging) {
        let _ = fs::remove_dir_all(&staging);
        return Err(error);
    }
    let staged_hash = directory_sha256(&staging)?;
    if staged_hash != status.installed.sha256 {
        let _ = fs::remove_dir_all(&staging);
        return Err("staged Stasis vendor fingerprint does not match the toolchain".to_string());
    }

    manifest.vendor = Some(VendorManifest {
        stasis: StasisVendorManifest {
            release_id: status.installed.release_id.clone(),
            sha256: status.installed.sha256.clone(),
        },
    });
    fs::write(&manifest_staging, serialized_manifest(manifest)?)
        .map_err(|error| format!("failed to stage {MANIFEST_NAME}: {error}"))?;

    let had_target = target.exists();
    if had_target {
        let metadata = fs::symlink_metadata(&target)
            .map_err(|error| format!("failed to inspect {}: {error}", target.display()))?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(format!("refusing to replace {}", target.display()));
        }
        fs::rename(&target, &backup)
            .map_err(|error| format!("failed to stage {}: {error}", target.display()))?;
    }
    if let Err(error) = fs::rename(&staging, &target) {
        if had_target {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!("failed to publish {}: {error}", target.display()));
    }
    if let Err(error) = fs::rename(&manifest_path, &manifest_backup) {
        let _ = fs::remove_dir_all(&target);
        if had_target {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!("failed to stage {MANIFEST_NAME}: {error}"));
    }
    if let Err(error) = fs::rename(&manifest_staging, &manifest_path) {
        let _ = fs::rename(&manifest_backup, &manifest_path);
        let _ = fs::remove_dir_all(&target);
        if had_target {
            let _ = fs::rename(&backup, &target);
        }
        return Err(format!("failed to publish {MANIFEST_NAME}: {error}"));
    }
    if had_target {
        fs::remove_dir_all(&backup)
            .map_err(|error| format!("failed to clear {}: {error}", backup.display()))?;
    }
    fs::remove_file(&manifest_backup)
        .map_err(|error| format!("failed to clear {}: {error}", manifest_backup.display()))?;
    Ok(true)
}

fn vendor_command(workspace: &Workspace, command: VendorCommand) -> Result<CommandResult, String> {
    match command {
        VendorCommand::Status => {
            let status = inspect_project_vendor(&workspace.root, &workspace.manifest)?;
            let current = !status.update_available && !status.local_changes;
            Ok(CommandResult::success(
                if current {
                    "Stasis vendor is current".to_string()
                } else if status.local_changes {
                    "Stasis vendor has local changes".to_string()
                } else {
                    "Stasis vendor update is available".to_string()
                },
                json!({
                    "current": current,
                    "update_available": status.update_available,
                    "local_changes": status.local_changes,
                    "recorded": status.recorded,
                    "installed": status.installed,
                    "actual_sha256": status.actual_sha256,
                }),
            ))
        }
        VendorCommand::Update => {
            let mut manifest = workspace.manifest.clone();
            let changed = update_vendor_snapshot(&workspace.root, &mut manifest)?;
            Ok(CommandResult::success(
                if changed {
                    "updated vendor/stasis from the selected toolchain".to_string()
                } else {
                    "Stasis vendor is already current".to_string()
                },
                json!({
                    "changed": changed,
                    "release_id": manifest.vendor.as_ref().map(|vendor| &vendor.stasis.release_id),
                    "sha256": manifest.vendor.as_ref().map(|vendor| &vendor.stasis.sha256),
                }),
            ))
        }
    }
}

fn canonical_workspace_root(root: &Path) -> Result<PathBuf, String> {
    let canonical = root
        .canonicalize()
        .map_err(|error| format!("failed to resolve workspace root: {error}"))?;
    #[cfg(windows)]
    {
        let text = canonical.to_string_lossy();
        if text.starts_with(r"\\?\UNC\") {
            return Err("workspace root must not use a UNC path".to_string());
        }
        if let Some(path) = text.strip_prefix(r"\\?\") {
            return Ok(PathBuf::from(path));
        }
    }
    Ok(canonical)
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
    format_files(&workspace.root, files, check)
}

fn format_explicit_paths(paths: &[PathBuf], check: bool) -> Result<CommandResult, String> {
    let root =
        env::current_dir().map_err(|error| format!("failed to read current directory: {error}"))?;
    let mut files = Vec::new();
    for input in paths {
        let path = absolute_path(input)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| format!("failed to inspect {}: {error}", path.display()))?;
        if metadata.file_type().is_symlink() {
            return Err(format!("refusing to format symlink {}", path.display()));
        }
        if metadata.is_dir() {
            collect_stasis_files(&path, &mut files)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some("stasis") {
            files.push(path);
        } else {
            return Err(format!(
                "expected a .stasis file or directory: {}",
                path.display()
            ));
        }
    }
    format_files(&root, files, check)
}

fn format_files(
    root: &Path,
    mut files: Vec<PathBuf>,
    check: bool,
) -> Result<CommandResult, String> {
    files.sort();
    files.dedup();
    struct FormatChange {
        path: PathBuf,
        relative: String,
        original: String,
        formatted: String,
    }

    let mut changes = Vec::new();
    for file in files {
        let original = fs::read_to_string(&file)
            .map_err(|error| format!("failed to read {}: {error}", file.display()))?;
        let formatted = format_source(&original)
            .map_err(|error| format!("failed to format {}: {error}", file.display()))?;
        let reformatted = format_source(&formatted)
            .map_err(|error| format!("failed to verify {}: {error}", file.display()))?;
        if formatted != reformatted {
            return Err(format!(
                "formatter is not idempotent for {}",
                file.display()
            ));
        }
        if original != formatted {
            changes.push(FormatChange {
                relative: relative_display(root, &file),
                path: file,
                original,
                formatted,
            });
        }
    }
    let changed = changes
        .iter()
        .map(|change| change.relative.clone())
        .collect::<Vec<_>>();
    if check && !changed.is_empty() {
        return Err(format!("formatting required: {}", changed.join(", ")));
    }
    if !check {
        for (index, change) in changes.iter().enumerate() {
            if let Err(error) = fs::write(&change.path, &change.formatted) {
                let mut rollback_errors = Vec::new();
                for applied in changes[..=index].iter().rev() {
                    if let Err(rollback_error) = fs::write(&applied.path, &applied.original) {
                        rollback_errors
                            .push(format!("{}: {rollback_error}", applied.path.display()));
                    }
                }
                let rollback = if rollback_errors.is_empty() {
                    "previous writes were rolled back".to_string()
                } else {
                    format!("rollback also failed for {}", rollback_errors.join(", "))
                };
                return Err(format!(
                    "failed to write {}: {error}; {rollback}",
                    change.path.display()
                ));
            }
        }
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

fn check_workspace(workspace: &Workspace) -> Result<CommandResult, String> {
    let jit = compile_workspace_jit(workspace)?;
    validate_compiled_workspace_assets(workspace, &jit)?;
    Ok(CommandResult::success(
        format!("checked {}", workspace.manifest.name),
        json!({
            "name": workspace.manifest.name,
            "entry": workspace.manifest.entry,
            "functions_emitted": jit.artifacts().len(),
        }),
    ))
}

fn validate_compiled_workspace_assets(
    workspace: &Workspace,
    jit: &JitProcess,
) -> Result<Option<stasis_assets::ResolvedAssetManifest>, String> {
    let snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "asset validation compile produced no ProgramSnapshot".to_string())?;
    validate_program_snapshot_assets(workspace, snapshot)
}

fn validate_program_snapshot_assets(
    workspace: &Workspace,
    snapshot: &ProgramSnapshot,
) -> Result<Option<stasis_assets::ResolvedAssetManifest>, String> {
    let manifest = if workspace.root.join(DEFAULT_ASSET_MANIFEST_PATH).is_file() {
        Some(
            load_project_asset_manifest(&workspace.root, AssetLimits::default())
                .map_err(|error| format!("failed to resolve project assets: {error}"))?,
        )
    } else {
        None
    };
    crate::release_assets::validate_snapshot_assets(&workspace.root, snapshot, manifest.as_ref())?;
    Ok(manifest)
}

fn compile_workspace_jit(workspace: &Workspace) -> Result<JitProcess, String> {
    compile_workspace_jit_with_debug(workspace, false)
}

fn compile_workspace_jit_with_debug(
    workspace: &Workspace,
    debug_instrumentation: bool,
) -> Result<JitProcess, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    validate_workspace_destination(workspace, "entry", &entry)?;
    let files =
        load_workshop_edit_workspace(&workspace.root, Path::new(&workspace.manifest.entry))?;
    let files = workshop_reachable_files(&files, Path::new(&workspace.manifest.entry))?;
    let mut jit = JitProcess::new();
    jit.set_debug_instrumentation(debug_instrumentation)?;
    jit.set_project_root(display_path(&workspace.root))?;
    jit.set_required_emit_roots(&runtime_analysis_roots());
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

fn compile_workspace_mobile_costs(workspace: &Workspace) -> Result<(u64, u64), String> {
    let files =
        load_workshop_edit_workspace(&workspace.root, Path::new(&workspace.manifest.entry))?;
    let files = workshop_reachable_files(&files, Path::new(&workspace.manifest.entry))?;
    let mut aot = AotProcess::new();
    aot.set_project_root(display_path(&workspace.root))?;
    aot.set_target(AotTarget::android_arm64_default());
    aot.set_required_emit_roots(&runtime_analysis_roots());
    for file in files {
        let path = workspace.root.join(&file.path);
        let path = path.canonicalize().unwrap_or(path);
        aot.upsert_file(path.to_string_lossy().to_string(), file.source);
    }
    aot.compile().map_err(|error| format!("{error:?}"))?;
    let code_bytes = aot
        .artifacts()
        .iter()
        .try_fold(0u64, |total, artifact| {
            total.checked_add(artifact.object_bytes_len as u64)
        })
        .ok_or_else(|| "AOT object byte estimate overflow".to_string())?;
    let literal_bytes = aot
        .string_literals()
        .values()
        .try_fold(0u64, |total, literal| {
            total.checked_add(literal.len() as u64)
        })
        .ok_or_else(|| "literal data byte estimate overflow".to_string())?;
    Ok((code_bytes, literal_bytes))
}

fn runtime_analysis_roots() -> Vec<String> {
    ["main", "tick", "render", "on_code_swap"]
        .into_iter()
        .map(str::to_string)
        .collect()
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
    jit.set_project_root(display_path(&workspace.root))?;
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
    let manifest = if workspace.root.join(DEFAULT_ASSET_MANIFEST_PATH).is_file() {
        Some(
            load_project_asset_manifest(&workspace.root, AssetLimits::default())
                .map_err(|error| format!("failed to resolve test assets: {error}"))?,
        )
    } else {
        None
    };
    let data_binding_paths = resolve_play_data_binding_paths(
        &workspace.root.join(&workspace.manifest.entry),
        &workspace.root,
        None,
        None,
    )?;
    let mut session = StasisTestRunSession::new();
    let summary = stasis::run_jit_tests_in_directory_with_project_root_session_and_validator(
        &directory,
        &workspace.root,
        &mut session,
        |jit| {
            let snapshot = jit
                .program_snapshot()
                .ok_or_else(|| "test compilation did not publish a program snapshot".to_string())?;
            crate::release_assets::validate_snapshot_assets(
                &workspace.root,
                snapshot,
                manifest.as_ref(),
            )?;
            load_and_apply_play_data_bindings_for_test(&data_binding_paths, jit)
        },
    )?;
    let scenarios = headless::run_scenarios(workspace, &directory)?;
    let data = json!({
        "files_discovered": summary.files_discovered,
        "files_with_tests": summary.files_with_tests,
        "tests_discovered": summary.tests_discovered,
        "tests_run": summary.tests_run,
        "tests_passed": summary.tests_passed,
        "tests_failed": summary.tests_failed,
        "passed_tests": summary.passed_tests,
        "failures": summary.failures,
        "scenarios_discovered": scenarios.scenarios_discovered,
        "scenario_cases_run": scenarios.cases_run,
        "scenario_cases_passed": scenarios.cases_passed,
        "scenario_cases_failed": scenarios.cases_failed,
        "scenario_failures": scenarios.failures,
        "scenario_failure_receipts": scenarios.failure_receipts,
    });
    if summary.tests_failed > 0 || scenarios.cases_failed > 0 {
        return Err(format!(
            "{} test(s) and {} scenario case(s) failed: {}",
            summary.tests_failed,
            scenarios.cases_failed,
            summary
                .failures
                .iter()
                .chain(scenarios.failures.iter())
                .cloned()
                .collect::<Vec<_>>()
                .join(" | ")
        ));
    }
    Ok(CommandResult::success(
        format!(
            "{} test(s) passed in {} file(s); {} scenario case(s) passed",
            summary.tests_passed, summary.files_with_tests, scenarios.cases_passed
        ),
        data,
    ))
}

fn run_workspace(
    workspace: &Workspace,
    _headless: bool,
    ticks: u64,
    fast_forward: bool,
) -> Result<CommandResult, String> {
    let jit = compile_workspace_jit(workspace)?;
    let guest_exit = match jit.execute_i32_noarg_by_name("main") {
        Ok(value) => value,
        Err(error) if error.contains("not i32-returning") => {
            jit.execute_void_noarg_by_name("main")?;
            0
        }
        Err(error) => return Err(error),
    };
    let run = headless::run_ticks(&jit, ticks)?;
    Ok(CommandResult {
        code: guest_exit,
        human: format!(
            "program exited with code {guest_exit} after {} headless tick(s)",
            run.ticks_executed
        ),
        data: json!({
            "exit_code": guest_exit,
            "backend": "jit",
            "headless": true,
            "ticks_executed": run.ticks_executed,
            "fast_forward": fast_forward,
            "state_hash": run.state_hash,
        }),
    })
}

fn run_workspace_watch(workspace: &Workspace) -> Result<CommandResult, String> {
    let entry = workspace.root.join(&workspace.manifest.entry);
    run_play_in_process_with_window_title(
        &entry,
        Some(&workspace.root),
        None,
        None,
        16_000,
        None,
        &workspace.manifest.name,
    )?;
    Ok(CommandResult::success(
        "graphical watch session ended",
        json!({"backend": "jit", "headless": false, "watch": true}),
    ))
}

fn replay_workspace(
    workspace: &Workspace,
    recording: &Path,
    entry: Option<&Path>,
    tick_sleep_us: u64,
) -> Result<CommandResult, String> {
    let entry = entry.unwrap_or_else(|| Path::new(&workspace.manifest.entry));
    validate_relative_path("replay entry", entry)?;
    let entry = workspace.root.join(entry);
    validate_workspace_destination(workspace, "replay entry", &entry)?;
    let recording = absolute_path(recording)?;
    run_play_in_process_with_replay(
        &entry,
        Some(&workspace.root),
        None,
        None,
        None,
        tick_sleep_us,
        None,
        Some(&workspace.manifest.name),
        None,
        None,
        PlayReplayConfig::Replay(recording.clone()),
    )?;
    Ok(CommandResult::success(
        format!("replayed {}", recording.display()),
        json!({
            "backend": "jit",
            "recording": recording,
            "verified": true,
        }),
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
            live_tui::run_scripted_project_ai_with_cancel(&client, &ai_root, &prompt, &ai_canceled);
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        result
    });
    let config = LiveRunConfig::new(
        workspace.root.clone(),
        PathBuf::from(&workspace.manifest.entry),
        PathBuf::from(&workspace.manifest.output),
    )
    .with_window_title(&workspace.manifest.name);
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

#[allow(clippy::too_many_arguments)]
fn run_workspace_tui(
    workspace: &Workspace,
    entry: &Path,
    watch_dir: Option<&Path>,
    data_bind: &[PathBuf],
    script: Option<&Path>,
    json_lines: bool,
    stdio: bool,
    tick_sleep_micros: u64,
    max_ticks: Option<u64>,
) -> Result<CommandResult, String> {
    if !data_bind.is_empty() && data_bind.len() != 2 {
        return Err("--data-bind requires DATA_PATH and STRUCT_META_PATH".to_string());
    }
    let (entry_path, entry_relative) = resolve_tui_entry(workspace, entry)?;
    validate_optional_workspace_path(workspace, "watch directory", watch_dir)?;
    validate_optional_workspace_path(workspace, "live script", script)?;
    for path in data_bind {
        validate_relative_path("data binding", path)?;
        validate_workspace_destination(workspace, "data binding", &workspace.root.join(path))?;
    }
    let data_json = data_bind.first().map(|path| workspace.root.join(path));
    let data_meta = data_bind.get(1).map(|path| workspace.root.join(path));
    let watch_dir = watch_dir.map(|path| workspace.root.join(path));
    let (client, server) = live_session(stasis_runner::live::DEFAULT_LIVE_QUEUE_CAPACITY);
    let transport = if stdio {
        "stdio"
    } else if script.is_some() {
        "script"
    } else {
        "terminal"
    };
    let script = script.map(|path| workspace.root.join(path));
    let terminal_root = workspace.root.clone();
    let terminal = thread::spawn(move || {
        run_live_terminal(client, script.as_deref(), json_lines, stdio, &terminal_root)
    });
    let config = LiveRunConfig::new(
        workspace.root.clone(),
        entry_relative,
        PathBuf::from(&workspace.manifest.output),
    )
    .with_window_title(&workspace.manifest.name);
    let run_result = run_live_in_process_with_data(
        &entry_path,
        watch_dir.as_deref(),
        data_json.as_deref(),
        data_meta.as_deref(),
        tick_sleep_micros,
        max_ticks,
        server,
        config,
    );
    if !wait_for_live_terminal_shutdown(&terminal, run_result.is_ok()) {
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
        if stdio {
            String::new()
        } else {
            "interactive live session ended".to_string()
        },
        json!({
            "backend": "jit",
            "headless": false,
            "interactive": true,
            "transport": transport,
        }),
    ))
}

fn resolve_tui_entry(workspace: &Workspace, entry: &Path) -> Result<(PathBuf, PathBuf), String> {
    if entry.as_os_str().is_empty() {
        return Err("TUI entry must not be empty".to_string());
    }
    let workspace_candidate = if entry.is_absolute() {
        entry.to_path_buf()
    } else {
        workspace.root.join(entry)
    };
    let launch_candidate = absolute_path(entry)?;
    let entry_path = if workspace_candidate.is_file() {
        workspace_candidate
    } else {
        launch_candidate
    };
    validate_workspace_destination(workspace, "TUI entry", &entry_path)?;
    if !entry_path.is_file() {
        return Err(format!("TUI entry is not a file: {}", entry_path.display()));
    }
    let root = workspace
        .root
        .canonicalize()
        .map_err(|error| format!("failed to resolve workspace root: {error}"))?;
    let entry_path = entry_path
        .canonicalize()
        .map_err(|error| format!("failed to resolve TUI entry: {error}"))?;
    let entry_relative = entry_path
        .strip_prefix(&root)
        .map_err(|_| "TUI entry resolves outside the workspace".to_string())?
        .to_path_buf();
    Ok((entry_path, entry_relative))
}

fn run_live_terminal(
    client: stasis_runner::live::LiveSessionClient,
    script: Option<&Path>,
    json_lines: bool,
    stdio: bool,
    project_root: &Path,
) -> Result<(), String> {
    let result = run_live_terminal_inner(&client, script, json_lines, stdio, project_root);
    if result.is_err() {
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
    }
    result
}

fn run_live_terminal_inner(
    client: &stasis_runner::live::LiveSessionClient,
    script: Option<&Path>,
    json_lines: bool,
    stdio: bool,
    project_root: &Path,
) -> Result<(), String> {
    let mut terminal = TerminalBuffer::new();
    let mut saw_quit = false;
    let mut script_failure = None;
    if stdio {
        let stdin = io::stdin();
        for line in stdin.lock().lines() {
            let line =
                line.map_err(|error| format!("failed reading live command from stdin: {error}"))?;
            if let TerminalInput::Request(request) = terminal.feed_line(&line)? {
                saw_quit |= matches!(&request.command, LiveCommand::Quit);
                submit_and_print_live_response(client, request, true, true)?;
                io::stdout()
                    .flush()
                    .map_err(|error| format!("failed flushing live response: {error}"))?;
                if saw_quit {
                    break;
                }
            }
        }
        if terminal.cancel_pending() {
            return Err(
                "live stdin ended with unfinished multiline input; send :end or :abort".to_string(),
            );
        }
    } else if let Some(script) = script {
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
        "diagnostics" => format_live_diagnostics(data),
        "hover" => format_live_hover(data),
        "definition" => format_live_definition(data),
        "code_actions" => format_live_code_actions(data),
        "inlay_hints" => format_live_inlay_hints(data),
        "rename_preview" => format!(
            "rename {} -> {} ({} validated edit(s))",
            string_field(data, "old_name", "symbol"),
            string_field(data, "new_name", "symbol"),
            data.get("edits")
                .and_then(Value::as_array)
                .map_or(0, Vec::len)
        ),
        "completion" | "palette" if response.truncated => {
            "completion response exceeded the output bound; narrow the query".to_string()
        }
        "completion" | "palette" => format_live_completion(data),
        "inspection" => format_live_inspection(data),
        "state_inspection" => format_live_state_inspection(data),
        "runtime_validation" => format_live_runtime_validation(data),
        "print" | "evaluation" => format!(
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

fn format_live_diagnostics(data: &Value) -> String {
    let Some(diagnostics) = data.get("diagnostics").and_then(Value::as_array) else {
        return "diagnostics unavailable".to_string();
    };
    if diagnostics.is_empty() {
        return "no compiler diagnostics".to_string();
    }
    diagnostics
        .iter()
        .map(|diagnostic| {
            format!(
                "{}:{}..{}: {}: {}",
                string_field(diagnostic, "file", "source"),
                diagnostic.get("start").and_then(Value::as_u64).unwrap_or(0),
                diagnostic.get("end").and_then(Value::as_u64).unwrap_or(0),
                string_field(diagnostic, "severity", "error"),
                string_field(diagnostic, "message", "compiler diagnostic")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_live_hover(data: &Value) -> String {
    let Some(hover) = data.get("hover").filter(|hover| !hover.is_null()) else {
        return "no symbol at offset".to_string();
    };
    let symbol = string_field(hover, "symbol", "symbol");
    let type_name = hover.get("type_name").and_then(Value::as_str);
    let live_value = hover.get("live_value").and_then(Value::as_str);
    match (type_name, live_value) {
        (Some(type_name), Some(value)) => format!("{symbol}: {type_name} = {value}"),
        (Some(type_name), None) => format!("{symbol}: {type_name}"),
        (None, Some(value)) => format!("{symbol} = {value}"),
        (None, None) => symbol.to_string(),
    }
}

fn format_live_definition(data: &Value) -> String {
    let Some(locations) = data.get("locations").and_then(Value::as_array) else {
        return "definition unavailable".to_string();
    };
    if locations.is_empty() {
        return "definition not found".to_string();
    }
    locations
        .iter()
        .map(|location| {
            format!(
                "{}:{}..{}",
                string_field(location, "file", "source"),
                location.get("start").and_then(Value::as_u64).unwrap_or(0),
                location.get("end").and_then(Value::as_u64).unwrap_or(0)
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_live_code_actions(data: &Value) -> String {
    let Some(actions) = data.get("actions").and_then(Value::as_array) else {
        return "code actions unavailable".to_string();
    };
    if actions.is_empty() {
        return "no safe code actions available".to_string();
    }
    actions
        .iter()
        .map(|action| {
            let edits = action
                .get("edits")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!(
                "{} ({} edit(s), preview only)",
                string_field(action, "title", "code action"),
                edits
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn format_live_inlay_hints(data: &Value) -> String {
    let Some(hints) = data.get("hints").and_then(Value::as_array) else {
        return "inlay hints unavailable".to_string();
    };
    if hints.is_empty() {
        return "no inlay hints".to_string();
    }
    hints
        .iter()
        .map(|hint| {
            format!(
                "{} @ {}:{}..{} {}",
                string_field(hint, "kind", "hint"),
                string_field(data, "file", "source"),
                hint.get("start").and_then(Value::as_u64).unwrap_or(0),
                hint.get("end").and_then(Value::as_u64).unwrap_or(0),
                string_field(hint, "label", "")
            )
        })
        .collect::<Vec<_>>()
        .join("\n")
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
            let manifest = validate_compiled_workspace_assets(workspace, &jit)?;
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
            stage_workspace_assets(
                workspace,
                receipt.parent().ok_or_else(|| {
                    format!(
                        "development build receipt has no parent: {}",
                        receipt.display()
                    )
                })?,
                manifest.as_ref(),
            )?;
            fs::write(&receipt, contents)
                .map_err(|error| format!("failed to write {}: {error}", receipt.display()))?;
            Ok(CommandResult::success(
                format!("built JIT development image: {}", receipt.display()),
                json!({"backend": "jit", "receipt": display_path(&receipt)}),
            ))
        }
        BuildMode::Release => {
            let validation_jit = compile_workspace_jit(workspace)?;
            let manifest = validate_compiled_workspace_assets(workspace, &validation_jit)?;
            preflight_release_asset_preparation(workspace, &validation_jit, manifest.as_ref())?;
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
            let build_snapshot = summary.program_snapshot.as_ref().ok_or_else(|| {
                "release build did not publish its authoritative ProgramSnapshot".to_string()
            })?;
            let validation_snapshot = validation_jit.program_snapshot().ok_or_else(|| {
                "release asset preflight did not publish a ProgramSnapshot".to_string()
            })?;
            if build_snapshot.asset_references() != validation_snapshot.asset_references() {
                return Err(
                    "release build asset roots changed after successful preflight validation"
                        .to_string(),
                );
            }
            let retained = manifest
                .as_ref()
                .map(|resolved| {
                    crate::release_assets::retain_snapshot_assets(
                        &workspace.root,
                        build_snapshot,
                        resolved,
                    )
                })
                .transpose()?;
            stage_workspace_assets(
                workspace,
                summary.linked_image_path.parent().ok_or_else(|| {
                    format!(
                        "release output has no parent: {}",
                        summary.linked_image_path.display()
                    )
                })?,
                retained.as_ref(),
            )?;
            Ok(CommandResult::success(
                format!(
                    "built release executable: {}",
                    summary.linked_image_path.display()
                ),
                json!({
                    "backend": "aot",
                    "output": display_path(&summary.linked_image_path),
                    "source_files": summary.source_file_count,
                    "entry_symbol": summary.entry_symbol,
                }),
            ))
        }
    }
}

fn preflight_release_asset_preparation(
    workspace: &Workspace,
    jit: &JitProcess,
    resolved: Option<&stasis_assets::ResolvedAssetManifest>,
) -> Result<(), String> {
    let Some(resolved) = resolved else {
        return Ok(());
    };
    let snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "release asset preflight produced no ProgramSnapshot".to_string())?;
    let retained =
        crate::release_assets::retain_snapshot_assets(&workspace.root, snapshot, resolved)?;
    let stamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let destination = workspace
        .root
        .join(".stasis_cache")
        .join(format!("asset-preflight-{}-{stamp}", std::process::id()));
    let prepared = prepare_asset_bundle(
        &retained,
        &destination,
        workspace.root.join(".stasis_cache/assets"),
    )
    .map_err(|error| format!("release asset preparation failed: {error}"));
    let cleanup = if destination.exists() {
        fs::remove_dir_all(&destination).map_err(|error| {
            format!(
                "failed to remove asset preflight {}: {error}",
                destination.display()
            )
        })
    } else {
        Ok(())
    };
    prepared?;
    cleanup
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
    if package_root.exists() && !matches!(target, PackageTarget::Web) {
        return Err(format!(
            "package output already exists: {}",
            package_root.display()
        ));
    }
    if matches!(target, PackageTarget::Web) {
        return package_web_workspace(workspace, &package_root, development_build);
    }
    if !matches!(target, PackageTarget::Desktop) {
        return package_mobile_workspace(
            workspace,
            target,
            Path::new(&workspace.manifest.entry),
            &package_root,
            development_build,
            &[],
            0,
            0,
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
    let executable_file_name = executable_name(&workspace.manifest.name);
    let assembled = (|| -> Result<(), String> {
        let executable = staging_root.join(&executable_file_name);
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
        #[cfg(windows)]
        nest_windows_desktop_payload(&staging_root, &executable)?;
        let payload_root = if cfg!(windows) {
            staging_root.join(WINDOWS_DESKTOP_PAYLOAD_DIR)
        } else {
            staging_root.clone()
        };
        copy_file(
            &workspace.root.join(MANIFEST_NAME),
            &payload_root.join(MANIFEST_NAME),
        )?;
        if let Some(runtime) = installed_runtime_library() {
            copy_file(
                &runtime,
                &payload_root.join(runtime.file_name().unwrap_or_default()),
            )?;
        }
        write_json_file(&payload_root.join(PACKAGE_PROVENANCE_NAME), &provenance)?;
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
            "provenance": if cfg!(windows) {
                format!("{WINDOWS_DESKTOP_PAYLOAD_DIR}/{PACKAGE_PROVENANCE_NAME}")
            } else {
                PACKAGE_PROVENANCE_NAME.to_string()
            },
            "development_build": provenance["development_build"],
        }),
    ))
}

const WEB_INDEX_HTML: &str = include_str!("../../../runtime/web/index.html");
const WEB_RUNTIME_JS: &str = include_str!("../../../runtime/web/game.js");
const WEB_MINIMAL_RUNTIME_JS: &str = include_str!("../../../runtime/web/game_minimal.js");

struct WebWasmArtifact {
    bytes: Vec<u8>,
    optimized: bool,
    input_bytes: usize,
}

fn prepare_web_wasm(
    module_bytes: &[u8],
    staging_root: &Path,
    development_build: bool,
) -> Result<WebWasmArtifact, String> {
    if development_build {
        return Ok(WebWasmArtifact {
            bytes: module_bytes.to_vec(),
            optimized: false,
            input_bytes: module_bytes.len(),
        });
    }

    let configured = env::var_os("STASIS_WASM_OPT");
    let executables = configured.clone().map_or_else(
        || vec![OsString::from("wasm-opt"), OsString::from("wasmopt")],
        |executable| vec![executable],
    );
    let input_path = staging_root.join(".game.unoptimized.wasm");
    let output_path = staging_root.join(".game.optimized.wasm");
    fs::write(&input_path, module_bytes)
        .map_err(|error| format!("failed to stage {}: {error}", input_path.display()))?;
    let mut result = None;
    for executable in &executables {
        match Command::new(executable)
            .arg("-Oz")
            .arg(&input_path)
            .arg("-o")
            .arg(&output_path)
            .output()
        {
            Ok(output) => {
                result = Some((executable, output));
                break;
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound && configured.is_none() => {}
            Err(error) => {
                let _ = fs::remove_file(&input_path);
                return Err(format!(
                    "failed to run wasm-opt at {}: {error}",
                    Path::new(executable).display()
                ));
            }
        }
    }
    let _ = fs::remove_file(&input_path);

    let Some((_, output)) = result else {
        return Ok(WebWasmArtifact {
            bytes: module_bytes.to_vec(),
            optimized: false,
            input_bytes: module_bytes.len(),
        });
    };
    if !output.status.success() {
        let _ = fs::remove_file(&output_path);
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(format!(
            "wasm-opt failed with status {}{}",
            output.status,
            if stderr.is_empty() {
                String::new()
            } else {
                format!(": {stderr}")
            }
        ));
    }
    let bytes = fs::read(&output_path)
        .map_err(|error| format!("failed to read {}: {error}", output_path.display()))?;
    let _ = fs::remove_file(&output_path);
    if !bytes.starts_with(b"\0asm\x01\0\0\0") {
        return Err("wasm-opt produced an invalid WebAssembly module header".to_string());
    }
    Ok(WebWasmArtifact {
        bytes,
        optimized: true,
        input_bytes: module_bytes.len(),
    })
}

fn package_web_workspace(
    workspace: &Workspace,
    package_root: &Path,
    development_build: bool,
) -> Result<CommandResult, String> {
    let staging_name = format!(
        ".{}.staging",
        package_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stasis-web-package")
    );
    let staging_root = package_root.with_file_name(staging_name);
    if staging_root.exists() {
        return Err(format!(
            "package staging output already exists: {}",
            staging_root.display()
        ));
    }
    let provenance = resolve_package_provenance(development_build)?;
    let development_build = provenance["development_build"].as_bool() == Some(true);
    fs::create_dir_all(&staging_root)
        .map_err(|error| format!("failed to create {}: {error}", staging_root.display()))?;

    let assembled = (|| -> Result<(bool, usize, usize), String> {
        let web_entry = workspace
            .manifest
            .web
            .as_ref()
            .and_then(|web| (!web.entry.is_empty()).then_some(web.entry.as_str()))
            .unwrap_or(workspace.manifest.entry.as_str());
        let files = load_workshop_edit_workspace(&workspace.root, Path::new(web_entry))?;
        let files = workshop_reachable_files(&files, Path::new(web_entry))?;
        let mut process = WasmProcess::new();
        process.set_debug_symbols(development_build);
        process.set_project_root(display_path(&workspace.root))?;
        process.set_required_emit_roots(&[
            "main".to_string(),
            "tick".to_string(),
            "render".to_string(),
        ]);
        let mut sources = BTreeMap::new();
        for file in files {
            let path = workspace.root.join(&file.path);
            let path = path.canonicalize().unwrap_or(path);
            let compiler_path = path.to_string_lossy().to_string();
            sources.insert(compiler_path.clone(), file.source.clone());
            process.upsert_file(compiler_path, file.source);
        }
        process.compile().map_err(|error| {
            if let Some(diagnostic) = process.last_source_diagnostic() {
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
        let wasm = prepare_web_wasm(process.module_bytes(), &staging_root, development_build)?;

        let snapshot = process
            .program_snapshot()
            .ok_or_else(|| "web compile produced no ProgramSnapshot".to_string())?;
        let resolved = validate_program_snapshot_assets(workspace, snapshot)?;
        let retained = resolved
            .as_ref()
            .map(|manifest| {
                crate::release_assets::retain_snapshot_assets(&workspace.root, snapshot, manifest)
            })
            .transpose()?;
        stage_workspace_assets(workspace, &staging_root, retained.as_ref())?;
        stage_web_loading_font(workspace, &staging_root)?;
        let loading_font = workspace
            .manifest
            .web
            .as_ref()
            .and_then(|web| web.loading_font.as_deref())
            .map(normalize_web_loading_font_path)
            .transpose()?;
        let runtime_config = web_runtime_config(workspace, &process, development_build);
        let runtime_json = serde_json::to_string(&runtime_config)
            .map_err(|error| format!("failed to encode static web runtime metadata: {error}"))?
            .replace("</", "<\\/");
        let audio_enabled = process.imported_symbols().iter().any(|symbol| {
            symbol.starts_with("audio_") || symbol.contains("_audio_") || symbol == "web_play_tone"
        });
        let network_enabled = process
            .imported_symbols()
            .iter()
            .any(|symbol| symbol.starts_with("stasis_web_network_"));
        let linked_runtime = link_web_runtime(&process, audio_enabled, network_enabled);
        let runtime_bundle = format!("window.STASIS_GAME = {runtime_json};\n{linked_runtime}");
        let wasm_path = staging_root.join("game.wasm");
        fs::write(&wasm_path, &wasm.bytes)
            .map_err(|error| format!("failed to write {}: {error}", wasm_path.display()))?;
        fs::write(staging_root.join("game.js"), &runtime_bundle)
            .map_err(|error| format!("failed to write web runtime: {error}"))?;
        fs::write(
            staging_root.join("index.html"),
            web_index_html(
                &workspace.manifest.name,
                development_build,
                loading_font.as_deref(),
            ),
        )
        .map_err(|error| format!("failed to write web index: {error}"))?;

        if workspace
            .manifest
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.network)
        {
            let mut bundle_files = vec![
                stasis_network::BundleFile {
                    path: "index.html".to_string(),
                    mime: "text/html; charset=utf-8".to_string(),
                    bytes: fs::read(staging_root.join("index.html"))
                        .map_err(|error| format!("failed to read staged web index: {error}"))?,
                },
                stasis_network::BundleFile {
                    path: "game.js".to_string(),
                    mime: "text/javascript".to_string(),
                    bytes: runtime_bundle.as_bytes().to_vec(),
                },
                stasis_network::BundleFile {
                    path: "game.wasm".to_string(),
                    mime: "application/wasm".to_string(),
                    bytes: wasm.bytes.clone(),
                },
            ];
            if let Some(retained) = retained.as_ref() {
                let mut assets = retained.assets.iter().collect::<Vec<_>>();
                assets.sort_by(|left, right| left.entry.path.cmp(&right.entry.path));
                for asset in assets {
                    let staged_path = staging_root.join(&asset.entry.path);
                    let bytes = fs::read(&staged_path).map_err(|error| {
                        format!(
                            "failed to read staged network guest asset {}: {error}",
                            staged_path.display()
                        )
                    })?;
                    bundle_files.push(stasis_network::BundleFile {
                        path: asset.entry.path.clone(),
                        mime: network_guest_asset_mime(&asset.entry.format).to_string(),
                        bytes,
                    });
                }
            }
            let bundle = stasis_network::StaticBundle::new(bundle_files)
                .map_err(|error| format!("failed to create network guest bundle: {error}"))?;
            let encoded = bundle
                .encode()
                .map_err(|error| format!("failed to encode network guest bundle: {error}"))?;
            fs::write(staging_root.join("network_guest.bundle"), &encoded)
                .map_err(|error| format!("failed to write network guest bundle: {error}"))?;
            write_json_file(
                &staging_root.join("network_guest.bundle.json"),
                &json!({
                    "format": "stasis.static_bundle.v1",
                    "path": "network_guest.bundle",
                    "length": encoded.len(),
                    "sha256": format!("{:x}", Sha256::digest(&encoded)),
                }),
            )?;
        }

        write_json_file(&staging_root.join(PACKAGE_PROVENANCE_NAME), &provenance)?;
        Ok((wasm.optimized, wasm.input_bytes, wasm.bytes.len()))
    })();
    let (wasm_optimized, wasm_input_bytes, wasm_output_bytes) = match assembled {
        Ok(package) => package,
        Err(error) => {
            let _ = fs::remove_dir_all(&staging_root);
            return Err(error);
        }
    };
    publish_package_output(&staging_root, package_root)?;
    let optimization = if wasm_optimized {
        "wasm-opt -Oz"
    } else if development_build {
        "development Wasm"
    } else {
        "unoptimized Wasm; wasm-opt not found"
    };
    Ok(CommandResult::success(
        format!(
            "packaged web at {} ({optimization})",
            package_root.display()
        ),
        json!({
            "target": "web",
            "output": display_path(package_root),
            "play": "index.html",
            "wasm": "game.wasm",
            "wasm_optimized": wasm_optimized,
            "wasm_input_bytes": wasm_input_bytes,
            "wasm_output_bytes": wasm_output_bytes,
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
            "web_entry": workspace
                .manifest
                .web
                .as_ref()
                .and_then(|web| (!web.entry.is_empty()).then_some(web.entry.as_str()))
                .unwrap_or(workspace.manifest.entry.as_str()),
            "network_guest_bundle": workspace
                .manifest
                .capabilities
                .as_ref()
                .is_some_and(|capabilities| capabilities.network)
                .then_some("network_guest.bundle"),
        }),
    ))
}

fn publish_package_output(staging_root: &Path, package_root: &Path) -> Result<(), String> {
    let previous_root = package_root.with_file_name(format!(
        ".{}.previous",
        package_root
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("stasis-package")
    ));
    let replacing = package_root.exists();
    if replacing {
        if previous_root.exists() {
            let _ = fs::remove_dir_all(staging_root);
            return Err(format!(
                "package replacement backup already exists: {}",
                previous_root.display()
            ));
        }
        fs::rename(package_root, &previous_root).map_err(|error| {
            let _ = fs::remove_dir_all(staging_root);
            format!(
                "failed to prepare package replacement {}: {error}",
                package_root.display()
            )
        })?;
    }
    if let Err(error) = fs::rename(staging_root, package_root) {
        let rollback = if replacing {
            fs::rename(&previous_root, package_root)
                .map_err(|rollback_error| format!("; rollback failed: {rollback_error}"))
        } else {
            Ok(())
        };
        let _ = fs::remove_dir_all(staging_root);
        return Err(format!(
            "failed to publish package {}: {error}{}",
            package_root.display(),
            rollback.err().unwrap_or_default()
        ));
    }
    if replacing {
        fs::remove_dir_all(&previous_root).map_err(|error| {
            format!(
                "published package but failed to remove previous output {}: {error}",
                previous_root.display()
            )
        })?;
    }
    Ok(())
}

fn web_index_html(title: &str, development_build: bool, loading_font: Option<&str>) -> String {
    let (hud_style, hud) = if development_build {
        (
            "#stasis-hud { position: absolute; top: 10px; left: 10px; padding: 8px 10px; background: #000b; border: 1px solid #53d8fb88; line-height: 1.4; pointer-events: none; }",
            r#"<div id="stasis-hud" role="status">Starting Wasm...</div>"#,
        )
    } else {
        ("", "")
    };
    let (loading_font_face, loading_font_family) = loading_font
        .map(|path| {
            let (mime, format) = match Path::new(path)
                .extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
                .as_deref()
            {
                Some("ttf") => ("ttf", "truetype"),
                Some("otf") => ("otf", "opentype"),
                Some("woff") => ("woff", "woff"),
                Some("woff2") => ("woff2", "woff2"),
                _ => ("font", "font"),
            };
            (
                format!(
                    "<link rel=\"preload\" href=\"{path}\" as=\"font\" type=\"font/{mime}\" crossorigin>\n  <style>@font-face {{ font-family: \"StasisLoadingFont\"; src: url(\"{path}\") format(\"{format}\"); font-display: block; }}</style>"
                ),
                "\"StasisLoadingFont\", ".to_string(),
            )
        })
        .unwrap_or_else(|| (String::new(), String::new()));
    WEB_INDEX_HTML
        .replace("__STASIS_GAME_TITLE__", title)
        .replace("__STASIS_PERFORMANCE_HUD_STYLE__", hud_style)
        .replace("__STASIS_PERFORMANCE_HUD__", hud)
        .replace("__STASIS_LOADING_FONT_FACE__", &loading_font_face)
        .replace("__STASIS_LOADING_FONT_FAMILY__", &loading_font_family)
}

fn lean_web_runtime(process: &WasmProcess) -> Option<String> {
    if WEB_RUNTIME_BUFFERS
        .iter()
        .any(|path| process.memory_layout().contains_key(*path))
        || WEB_HOST_GLOBALS
            .iter()
            .any(|path| process.global_types().contains_key(*path))
    {
        return None;
    }
    let snippets = BTreeMap::from([
        ("cos_fast", "    cos_fast: value => Math.cos(value),"),
        ("print_char", "    print_char: value => console.log(String.fromCodePoint(value)),"),
        ("print_i32", "    print_i32: value => console.log(value),"),
        ("print_int", "    print_int: value => console.log(value),"),
        ("print_string", "    print_string: value => console.log(stringValue(value)),"),
        ("sin_fast", "    sin_fast: value => Math.sin(value),"),
        ("web_begin_frame", "    web_begin_frame: (r, g, b) => { commands.length = 0; commands.push([0, r, g, b]); },"),
        ("web_draw_rect", "    web_draw_rect: (x, y, width, height, r, g, b) => commands.push([1, x, y, width, height, r, g, b]),"),
        ("web_draw_text", "    web_draw_text: (x, y, value) => commands.push([2, x, y, value]),"),
    ]);
    let imports = process
        .imported_symbols()
        .iter()
        .map(|symbol| snippets.get(symbol.as_str()).copied())
        .collect::<Option<Vec<_>>>()?;
    Some(WEB_MINIMAL_RUNTIME_JS.replace("__STASIS_IMPORTS__", &imports.join("\n")))
}

fn link_web_runtime(process: &WasmProcess, audio_enabled: bool, network_enabled: bool) -> String {
    if let Some(runtime) = lean_web_runtime(process) {
        return runtime;
    }
    let runtime = strip_web_runtime_feature(WEB_RUNTIME_JS, "audio", audio_enabled);
    strip_web_runtime_feature(&runtime, "network", network_enabled)
}

fn strip_web_runtime_feature(source: &str, feature: &str, enabled: bool) -> String {
    let begin = format!("// @stasis-feature {feature} begin");
    let end = format!("// @stasis-feature {feature} end");
    let mut inside = false;
    source
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed == begin {
                inside = true;
                return None;
            }
            if trimmed == end {
                inside = false;
                return None;
            }
            (enabled || !inside).then_some(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

// Keep these aligned with the metadata reads in runtime/web/game.js. Development
// packages retain the complete reflection tables for inspection and tooling.
const WEB_RESOURCE_BINDING_FIELDS: [&str; 4] = ["handle", "width", "height", "font"];
const WEB_RUNTIME_BUFFERS: [&str; 5] = [
    "gfx_cmd_i32",
    "gfx_cmd_f32",
    "gfx_cmd_u8",
    "host_i32",
    "host_f32",
];
const WEB_HOST_GLOBALS: [&str; 4] = [
    "host_req_seq",
    "host_req_flags",
    "host_req_window_w_px",
    "host_req_window_h_px",
];

fn web_runtime_config(
    workspace: &Workspace,
    process: &WasmProcess,
    development_build: bool,
) -> Value {
    let strings = process
        .string_literals()
        .iter()
        .map(|(id, value)| (id.to_string(), Value::String(value.clone())))
        .collect::<serde_json::Map<_, _>>();
    let memory = process
        .memory_layout()
        .iter()
        .map(|(path, layout)| {
            (
                path.clone(),
                json!({
                    "hash": stasis_compiler::backend::wasm::wasm_global_hash(path),
                    "offset": layout.offset,
                    "type_id": layout.type_id,
                    "length": layout.length,
                    "stride": layout.stride,
                    "byte_backed": layout.byte_backed,
                }),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let views = process
        .struct_views()
        .iter()
        .map(|(base, fields)| (base.to_string(), json!(fields)))
        .collect::<serde_json::Map<_, _>>();
    let globals = process
        .global_types()
        .iter()
        .map(|(path, type_id)| {
            (
                path.clone(),
                json!({"hash": stasis_compiler::backend::wasm::wasm_global_hash(path), "type_id": type_id}),
            )
        })
        .collect::<serde_json::Map<_, _>>();
    let mut config = json!({
        "name": workspace.manifest.name,
        "strings": strings,
        "memory": memory,
        "views": views,
        "globals": globals,
        "assets": {},
    });
    if !development_build {
        prune_release_web_runtime_config(&mut config, process.imported_symbols());
    }
    config
}

fn prune_release_web_runtime_config(config: &mut Value, imported_symbols: &BTreeSet<String>) {
    let views = config["views"].as_object_mut().expect("generated views");
    views.retain(|_, fields| {
        let fields = fields.as_object_mut().expect("generated view fields");
        fields.retain(|field, _| WEB_RESOURCE_BINDING_FIELDS.contains(&field.as_str()));
        !fields.is_empty()
    });
    let retained_paths = views
        .values()
        .flat_map(|fields| {
            fields
                .as_object()
                .expect("generated view fields")
                .values()
                .filter_map(Value::as_str)
        })
        .map(str::to_string)
        .collect::<BTreeSet<_>>();
    config["memory"]
        .as_object_mut()
        .expect("generated memory layouts")
        .retain(|path, layout| {
            retained_paths.contains(path.as_str())
                || WEB_RUNTIME_BUFFERS.contains(&path.as_str())
                || (imported_symbols.contains("sys_memcpy_u8")
                    && layout["byte_backed"].as_bool() == Some(true))
                || (imported_symbols.contains("sys_memcpy_i32")
                    && layout["type_id"].as_i64() == Some(i64::from(TYPE_ID_I32)))
                || (imported_symbols.contains("sys_memcpy_f32")
                    && layout["type_id"].as_i64() == Some(i64::from(TYPE_ID_F32)))
        });
    config["globals"]
        .as_object_mut()
        .expect("generated globals")
        .retain(|path, _| {
            retained_paths.contains(path.as_str()) || WEB_HOST_GLOBALS.contains(&path.as_str())
        });
}

#[cfg(windows)]
fn nest_windows_desktop_payload(staging_root: &Path, executable: &Path) -> Result<(), String> {
    let payload_root = staging_root.join(WINDOWS_DESKTOP_PAYLOAD_DIR);
    fs::create_dir_all(&payload_root)
        .map_err(|error| format!("failed to create {}: {error}", payload_root.display()))?;
    let entries = fs::read_dir(staging_root)
        .map_err(|error| format!("failed to read {}: {error}", staging_root.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("failed to inspect {}: {error}", staging_root.display()))?;

    for entry in entries {
        let source = entry.path();
        if source == executable || source == payload_root {
            continue;
        }
        let destination = payload_root.join(entry.file_name());
        fs::rename(&source, &destination).map_err(|error| {
            format!(
                "failed to move Windows package payload {} to {}: {error}",
                source.display(),
                destination.display()
            )
        })?;
    }
    Ok(())
}

fn stage_workspace_assets(
    workspace: &Workspace,
    destination_root: &Path,
    resolved: Option<&stasis_assets::ResolvedAssetManifest>,
) -> Result<(), String> {
    let assets = workspace.root.join("assets");
    validate_workspace_destination(workspace, "assets directory", &assets)?;
    if let Some(resolved) = resolved {
        prepare_asset_bundle(
            resolved,
            destination_root,
            workspace.root.join(".stasis_cache/assets"),
        )
        .map_err(|error| format!("failed to prepare desktop build assets: {error}"))?;
    } else {
        copy_dir_if_exists(&assets, &destination_root.join("assets"))?;
    }
    Ok(())
}

fn network_guest_asset_mime(format: &AssetFormat) -> &'static str {
    match format {
        AssetFormat::Sprite { encoding, .. } => match encoding {
            SpriteEncoding::Png => "image/png",
            SpriteEncoding::Svg => "image/svg+xml",
            SpriteEncoding::Jpeg => "image/jpeg",
            SpriteEncoding::Webp => "image/webp",
        },
        AssetFormat::Audio { encoding, .. } => match encoding {
            AudioEncoding::Wav => "audio/wav",
            AudioEncoding::Ogg => "audio/ogg",
            AudioEncoding::Mp3 => "audio/mpeg",
            AudioEncoding::M4a => "audio/mp4",
        },
        AssetFormat::Font { encoding } => match encoding {
            FontEncoding::Ttf => "font/ttf",
            FontEncoding::Otf => "font/otf",
        },
    }
}

fn package_mobile_command(
    workspace: &Workspace,
    target: MobilePackageTarget,
    entry: Option<&Path>,
    output: Option<&Path>,
    development_build: bool,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
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
    package_mobile_workspace(
        workspace,
        target,
        entry,
        &package_root,
        development_build,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
    )
}

fn validate_mobile_network_guest_contract(
    manifest: &ProjectManifest,
    target: PackageTarget,
) -> Result<(), String> {
    if target.is_mobile()
        && manifest
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.network)
        && manifest
            .web
            .as_ref()
            .map_or(true, |web| web.entry.is_empty())
    {
        return Err(
            "network-enabled mobile projects must declare web.entry for the guest bundle"
                .to_string(),
        );
    }
    Ok(())
}

fn package_mobile_workspace(
    workspace: &Workspace,
    target: PackageTarget,
    entry: &Path,
    package_root: &Path,
    development_build: bool,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
) -> Result<CommandResult, String> {
    validate_mobile_network_guest_contract(&workspace.manifest, target)?;
    if matches!(target, PackageTarget::IosArm64)
        && workspace
            .manifest
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.network)
        && !cfg!(target_os = "macos")
    {
        return Err(
            "network-enabled iOS packaging requires a macOS host with Xcode and the iOS Rust toolchain"
                .to_string(),
        );
    }
    if matches!(target, PackageTarget::AndroidX86_64) && !development_build {
        return Err(
            "android-x86_64 is a test-only emulator target; pass --development-build".to_string(),
        );
    }
    if !profile_functions.is_empty() && !development_build {
        return Err("mobile function profiling requires --development-build".to_string());
    }
    if profile_functions.len() > 64 {
        return Err("mobile function profiling supports at most 64 functions".to_string());
    }
    if !profile_functions.is_empty() && profile_sample_frames == 0 {
        return Err("mobile function profiling requires at least one sample frame".to_string());
    }
    let unique_profile_functions: BTreeSet<&str> =
        profile_functions.iter().map(String::as_str).collect();
    if unique_profile_functions.len() != profile_functions.len() {
        return Err("mobile function profiling function names must be unique".to_string());
    }
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
        let mut child_command = Command::new(executable);
        child_command
            .arg("mobile-aot-bundle")
            .arg("--target")
            .arg(target.as_str())
            .arg("--project-dir")
            .arg(&workspace.root)
            .arg("--entry-file")
            .arg(entry)
            .arg("--out-dir")
            .arg(&aot_root);
        if !profile_functions.is_empty() {
            child_command
                .arg("--profile-functions")
                .arg(profile_functions.join(","))
                .arg("--profile-warmup-frames")
                .arg(profile_warmup_frames.to_string())
                .arg("--profile-sample-frames")
                .arg(profile_sample_frames.to_string());
        }
        let child = child_command
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
        let web_guest_bundle = if workspace
            .manifest
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.network)
        {
            let web_guest_root = staging_root.join("web-guest");
            package_web_workspace(workspace, &web_guest_root, development_build)?;
            Some(web_guest_root.join("network_guest.bundle"))
        } else {
            None
        };
        if workspace
            .manifest
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.network)
        {
            stage_mobile_network_library(&staging_root, target)?;
        }
        assemble_mobile_shell(
            workspace,
            target,
            &aot_root,
            &staging_root,
            &provenance,
            web_guest_bundle.as_deref(),
        )?;
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
                PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => "android",
                PackageTarget::IosArm64 => "ios/StasisMobile.xcodeproj",
                PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
            },
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
        }),
    ))
}

fn stage_mobile_network_library(staging_root: &Path, target: PackageTarget) -> Result<(), String> {
    match target {
        PackageTarget::IosArm64 => stage_ios_network_library(staging_root),
        PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
            stage_android_network_library(staging_root, target)
        }
        PackageTarget::Desktop | PackageTarget::Web => {
            Err("network static library requires a mobile target".to_string())
        }
    }
}

fn network_support_target(target: PackageTarget) -> Option<&'static str> {
    match target {
        PackageTarget::AndroidArm64 => Some("android-arm64"),
        PackageTarget::AndroidX86_64 => Some("android-x86_64"),
        PackageTarget::IosArm64 => Some("ios-arm64"),
        PackageTarget::Desktop | PackageTarget::Web => None,
    }
}

fn bundled_network_artifacts_for_executable(
    executable: &Path,
    target: PackageTarget,
) -> Result<Option<(PathBuf, PathBuf)>, String> {
    let target_name = network_support_target(target)
        .ok_or_else(|| "network static library requires a mobile target".to_string())?;
    let executable_dir = executable.parent().unwrap_or(Path::new("."));
    for support_root in [
        executable_dir.join("mobile/network"),
        executable_dir.join("../mobile/network"),
    ] {
        let library = support_root.join(target_name).join("libstasis_network.a");
        let header = support_root.join("include/stasis_network.h");
        if library.is_file() && header.is_file() {
            return Ok(Some((library, header)));
        }
    }
    Ok(None)
}

fn stage_network_artifacts(
    staging_root: &Path,
    target: PackageTarget,
    library: &Path,
    header: &Path,
) -> Result<(), String> {
    let destination = match target {
        PackageTarget::IosArm64 => staging_root.join("ios/network"),
        PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
            staging_root.join("android/app/src/main/cpp/network")
        }
        PackageTarget::Desktop | PackageTarget::Web => {
            return Err("network static library requires a mobile target".to_string())
        }
    };
    fs::create_dir_all(destination.join("include"))
        .map_err(|error| format!("failed to create network staging: {error}"))?;
    copy_file(library, &destination.join("libstasis_network.a"))?;
    copy_file(header, &destination.join("include/stasis_network.h"))?;
    Ok(())
}

fn source_network_workspace() -> Option<PathBuf> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)?
        .to_path_buf();
    (source_root.join("Cargo.toml").is_file()
        && source_root
            .join("crates/stasis_network/Cargo.toml")
            .is_file())
    .then_some(source_root)
}

fn stage_android_network_library(staging_root: &Path, target: PackageTarget) -> Result<(), String> {
    if let Some((library, header)) = bundled_network_artifacts_for_executable(
        &env::current_exe()
            .map_err(|error| format!("failed to locate stasis executable: {error}"))?,
        target,
    )? {
        return stage_network_artifacts(staging_root, target, &library, &header);
    }
    let source_root = source_network_workspace().ok_or_else(|| {
        "installed toolchain is missing prebuilt mobile/network network libraries; reinstall the complete release archive"
            .to_string()
    })?;
    let (rust_target, api_level) = match target {
        PackageTarget::AndroidArm64 => ("aarch64-linux-android", "aarch64-linux-android26"),
        PackageTarget::AndroidX86_64 => ("x86_64-linux-android", "x86_64-linux-android26"),
        _ => return Err("network static library requires an Android target".to_string()),
    };
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command.current_dir(&source_root).args([
        "build",
        "-p",
        "stasis_network",
        "--target",
        rust_target,
        "--release",
    ]);
    let Some(clang) = android_ndk_clang("clang") else {
        return Err(
            "network-enabled Android packaging requires an installed Android NDK clang".to_string(),
        );
    };
    let mut rustflags = env::var("RUSTFLAGS").unwrap_or_default();
    if !rustflags.is_empty() {
        rustflags.push(' ');
    }
    rustflags.push_str(&format!("-C link-arg=--target={api_level}"));
    let linker_key = format!(
        "CARGO_TARGET_{}_LINKER",
        rust_target.replace('-', "_").to_ascii_uppercase()
    );
    command
        .env(linker_key, &clang)
        .env(format!("CC_{rust_target}"), &clang)
        .env(format!("CXX_{rust_target}"), &clang)
        .env(
            format!("CFLAGS_{rust_target}"),
            format!("--target={api_level}"),
        )
        .env("RUSTFLAGS", rustflags);
    let output = command
        .output()
        .map_err(|error| format!("failed to build stasis_network for Android: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Android stasis_network build failed with exit code {}: stdout={} stderr={}",
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let library = source_root.join(format!("target/{rust_target}/release/libstasis_network.a"));
    if !library.is_file() {
        return Err(format!(
            "Android stasis_network build did not produce {}",
            library.display()
        ));
    }
    stage_network_artifacts(
        staging_root,
        target,
        &library,
        &source_root.join("crates/stasis_network/include/stasis_network.h"),
    )
}

fn stage_ios_network_library(staging_root: &Path) -> Result<(), String> {
    if !cfg!(target_os = "macos") {
        return Err(
            "network-enabled iOS packaging requires a macOS host with Xcode and the iOS Rust toolchain"
                .to_string(),
        );
    }
    if let Some((library, header)) = bundled_network_artifacts_for_executable(
        &env::current_exe()
            .map_err(|error| format!("failed to locate stasis executable: {error}"))?,
        PackageTarget::IosArm64,
    )? {
        return stage_network_artifacts(staging_root, PackageTarget::IosArm64, &library, &header);
    }
    let source_root = source_network_workspace().ok_or_else(|| {
        "installed toolchain is missing prebuilt mobile/network network libraries; reinstall the complete release archive"
            .to_string()
    })?;
    let rust_target = "aarch64-apple-ios";
    let xcrun = Command::new("xcrun")
        .args(["--sdk", "iphoneos", "--find", "clang"])
        .output()
        .map_err(|error| {
            format!(
                "network-enabled iOS packaging requires Xcode's iphoneos clang (run xcrun --sdk iphoneos --find clang): {error}"
            )
        })?;
    if !xcrun.status.success() {
        return Err(format!(
            "network-enabled iOS packaging requires Xcode's iphoneos clang (xcrun failed): {}",
            String::from_utf8_lossy(&xcrun.stderr).trim()
        ));
    }
    let clang = String::from_utf8_lossy(&xcrun.stdout).trim().to_string();
    if clang.is_empty() {
        return Err(
            "network-enabled iOS packaging requires Xcode's iphoneos clang (xcrun returned no path)"
                .to_string(),
        );
    }
    let cargo = env::var_os("CARGO").unwrap_or_else(|| OsString::from("cargo"));
    let mut command = Command::new(cargo);
    command
        .current_dir(&source_root)
        .args([
            "build",
            "-p",
            "stasis_network",
            "--target",
            rust_target,
            "--release",
        ])
        .env("CARGO_TARGET_AARCH64_APPLE_IOS_LINKER", &clang)
        .env("CC_aarch64_apple_ios", &clang)
        .env("CXX_aarch64_apple_ios", &clang);
    let output = command
        .output()
        .map_err(|error| format!("failed to build stasis_network for iOS: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "iOS stasis_network build failed with exit code {}: stdout={} stderr={}",
            output.status.code().unwrap_or(1),
            String::from_utf8_lossy(&output.stdout).trim(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let library = source_root.join(format!("target/{rust_target}/release/libstasis_network.a"));
    if !library.is_file() {
        return Err(format!(
            "iOS stasis_network build did not produce {}",
            library.display()
        ));
    }
    stage_network_artifacts(
        staging_root,
        PackageTarget::IosArm64,
        &library,
        &source_root.join("crates/stasis_network/include/stasis_network.h"),
    )
}

fn android_ndk_clang(executable: &str) -> Option<PathBuf> {
    let sdk = env::var_os("ANDROID_NDK_HOME")
        .or_else(|| env::var_os("ANDROID_NDK_ROOT"))
        .map(PathBuf::from)
        .or_else(|| {
            let sdk = env::var_os("ANDROID_SDK_ROOT")
                .or_else(|| env::var_os("ANDROID_HOME"))
                .map(PathBuf::from)?;
            let mut versions = fs::read_dir(sdk.join("ndk"))
                .ok()?
                .filter_map(Result::ok)
                .map(|entry| entry.path())
                .filter(|path| path.is_dir())
                .collect::<Vec<_>>();
            versions.sort();
            versions.pop()
        })?;
    let directory = sdk.join("toolchains/llvm/prebuilt/windows-x86_64/bin");
    let exe_path = directory.join(format!("{executable}.exe"));
    if exe_path.is_file() {
        return Some(exe_path);
    }
    let path = directory.join(executable);
    if path.is_file() {
        return Some(path);
    }
    let cmd_path = directory.join(format!("{executable}.cmd"));
    cmd_path.is_file().then_some(cmd_path)
}

fn assemble_mobile_shell(
    workspace: &Workspace,
    target: PackageTarget,
    aot_root: &Path,
    staging_root: &Path,
    provenance: &Value,
    web_guest_bundle: Option<&Path>,
) -> Result<(), String> {
    let mobile_assets = bundled_mobile_assets_dir()?;
    let runtime = bundled_mobile_runtime_dir()?;
    let platform = match target {
        PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => "android",
        PackageTarget::IosArm64 => "ios",
        PackageTarget::Desktop | PackageTarget::Web => {
            return Err("selected target is not a mobile package target".to_string())
        }
    };
    let common_destination = staging_root.join("common");
    let platform_destination = staging_root.join(platform);
    copy_required_dir(&mobile_assets.join("common"), &common_destination)?;
    copy_required_dir(&mobile_assets.join(platform), &platform_destination)?;
    copy_mobile_runtime(&runtime, &staging_root.join("runtime"))?;
    write_json_file(&staging_root.join(PACKAGE_PROVENANCE_NAME), provenance)?;
    write_mobile_provenance_header(&common_destination, provenance)?;

    let asset_source = match target {
        PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
            aot_root.join("apk_assets/stasis_game")
        }
        PackageTarget::IosArm64 => aot_root.join("ios_assets/stasis_game"),
        PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
    };
    let asset_destination = match target {
        PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
            staging_root.join("android/app/src/main/assets/stasis_game")
        }
        PackageTarget::IosArm64 => staging_root.join("ios/StasisMobile/stasis_game"),
        PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
    };
    let android_manifest = if target.is_android() {
        workspace.manifest.android.as_ref()
    } else {
        None
    };
    let package_id = android_manifest
        .map(|manifest| manifest.application_id.clone())
        .unwrap_or_else(|| mobile_package_id(&workspace.manifest.name));
    let app_name = android_manifest
        .map(|manifest| manifest.label.as_str())
        .unwrap_or(workspace.manifest.name.as_str());
    let android_orientation = android_manifest
        .map(|manifest| manifest.orientation.as_str())
        .unwrap_or("sensorLandscape");
    let android_version_code = android_manifest
        .map(|manifest| manifest.version_code)
        .unwrap_or(1)
        .to_string();
    let android_version_name = android_manifest
        .map(|manifest| manifest.version_name.as_str())
        .unwrap_or("1.0");
    let jni_package = package_id.replace('.', "_");
    let network_enabled = workspace
        .manifest
        .capabilities
        .as_ref()
        .is_some_and(|capabilities| capabilities.network);
    let local_network_usage = if network_enabled && matches!(target, PackageTarget::IosArm64) {
        format!(
            "    <key>NSLocalNetworkUsageDescription</key><string>{} uses your local network so nearby friends can join games hosted on this device.</string>\n",
            app_name
        )
    } else {
        String::new()
    };
    let replacements = [
        ("@STASIS_APP_NAME@", app_name),
        ("@STASIS_PACKAGE_ID@", package_id.as_str()),
        ("@STASIS_JNI_PACKAGE@", jni_package.as_str()),
        ("@STASIS_ASSET_BASE@", "."),
        ("@STASIS_ANDROID_ORIENTATION@", android_orientation),
        (
            "@STASIS_ANDROID_VERSION_CODE@",
            android_version_code.as_str(),
        ),
        ("@STASIS_ANDROID_VERSION_NAME@", android_version_name),
        ("@STASIS_ANDROID_ABI@", target.android_abi().unwrap_or("")),
        (
            "@STASIS_NETWORK_ENABLED@",
            if network_enabled { "1" } else { "0" },
        ),
        (
            "@STASIS_NETWORK_PERMISSION@",
            if network_enabled {
                "    <uses-permission android:name=\"android.permission.INTERNET\" />\n"
            } else {
                ""
            },
        ),
        ("@STASIS_LOCAL_NETWORK_USAGE@", local_network_usage.as_str()),
    ];
    replace_shell_tokens(&common_destination, &replacements)?;
    replace_shell_tokens(&platform_destination, &replacements)?;
    copy_required_dir(&asset_source, &asset_destination)?;
    if let Some(bundle) = web_guest_bundle {
        copy_file(bundle, &asset_destination.join("network_guest.bundle"))?;
        let metadata = bundle.with_extension("bundle.json");
        if metadata.is_file() {
            copy_file(
                &metadata,
                &asset_destination.join("network_guest.bundle.json"),
            )?;
        }
    }
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
        write_ios_object_config(
            aot_root,
            &staging_root.join("ios/StasisMobile.xcconfig"),
            network_enabled,
        )?;
    }
    let network_library = if network_enabled {
        Some(match target {
            PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
                "android/app/src/main/cpp/network/libstasis_network.a"
            }
            PackageTarget::IosArm64 => "ios/network/libstasis_network.a",
            PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
        })
    } else {
        None
    };
    let network_header = if network_enabled {
        Some(match target {
            PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
                "android/app/src/main/cpp/network/include/stasis_network.h"
            }
            PackageTarget::IosArm64 => "ios/network/include/stasis_network.h",
            PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
        })
    } else {
        None
    };
    let network_guest_bundle = if network_enabled {
        Some(match target {
            PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
                "android/app/src/main/assets/stasis_game/network_guest.bundle"
            }
            PackageTarget::IosArm64 => "ios/StasisMobile/stasis_game/network_guest.bundle",
            PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
        })
    } else {
        None
    };
    fs::write(
        staging_root.join("stasis_mobile_package.json"),
        serde_json::to_string_pretty(&json!({
            "schema": "stasis.mobile_package.v1",
            "target": target.as_str(),
            "name": workspace.manifest.name,
            "app_name": app_name,
            "package_id": package_id,
            "android_orientation": android_orientation,
            "android_version_code": android_version_code,
            "android_version_name": android_version_name,
            "aot_manifest": "aot/mobile_aot_bundle_manifest.json",
            "provenance": PACKAGE_PROVENANCE_NAME,
            "development_build": provenance["development_build"],
            "assets": match target {
                PackageTarget::AndroidArm64 | PackageTarget::AndroidX86_64 => {
                    "android/app/src/main/assets/stasis_game"
                }
                PackageTarget::IosArm64 => "ios/StasisMobile/stasis_game",
                PackageTarget::Desktop | PackageTarget::Web => unreachable!(),
            },
            "network": network_enabled,
            "network_library": network_library,
            "network_header": network_header,
            "network_guest_bundle": network_guest_bundle,
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

fn release_provenance_path() -> Result<Option<PathBuf>, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?;
    let directory = executable.parent().unwrap_or(Path::new("."));
    Ok([
        directory.join(RELEASE_PROVENANCE_NAME),
        directory.join("..").join(RELEASE_PROVENANCE_NAME),
    ]
    .into_iter()
    .find(|candidate| candidate.is_file()))
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
        || value["command_buffer"]["version"] != 4
    {
        return Err("release provenance is not a clean official gfx_cmd v4 build".to_string());
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
        || dependencies["sdl3"] != "3.4.10-static"
        || dependencies["sdl3_image"] != "3.4.4-static"
    {
        return Err(
            "release provenance must use SDL3 3.4.10-static and SDL3_image 3.4.4-static"
                .to_string(),
        );
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

fn local_provenance(development_build: bool) -> Result<Value, String> {
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
    let dirty_state = development_build
        || git_text(&["status", "--porcelain", "--untracked-files=no"])
            .map_or(true, |status| !status.is_empty());
    let build_class = if development_build {
        "development"
    } else {
        "local_release"
    };
    Ok(json!({
        "schema": "stasis.release_provenance.v1",
        "build_class": build_class,
        "release_tag": Value::Null,
        "source_commit": git_text(&["rev-parse", "HEAD"]).unwrap_or_else(|| "unknown".to_string()),
        "dirty_state": dirty_state,
        "development_build": development_build,
        "compiler": {
            "path": executable.file_name().unwrap_or_default().to_string_lossy(),
            "sha256": sha256_file(&executable)?,
        },
        "runtime_sources": sources,
        "mobile_shell_sources": content_hashes(&mobile_shells, "mobile/shells")?,
        "command_buffer": {"name": "gfx_cmd", "version": 4},
        "backends": ["sdl3"],
        "features": ["aot", "jit", "mobile-aot", "shared-renderer"],
        "dependencies": {
            "stasis": env!("CARGO_PKG_VERSION"),
            "toolchain": build_class,
            "sdl3": "3.4.10-static",
            "sdl3_image": "3.4.4-static",
        },
    }))
}

fn resolve_package_provenance(development_build: bool) -> Result<Value, String> {
    if development_build {
        local_provenance(true)
    } else if let Some(path) = release_provenance_path()? {
        verify_release_provenance(&path)
    } else {
        local_provenance(false)
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
    } else if provenance["build_class"] == "local_release" {
        "local release"
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

fn write_ios_object_config(
    aot_root: &Path,
    output: &Path,
    network_enabled: bool,
) -> Result<(), String> {
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
    let network_flags = if network_enabled {
        " STASIS_NETWORK_ENABLED=1"
    } else {
        ""
    };
    let network_headers = if network_enabled {
        " $(PROJECT_DIR)/network/include"
    } else {
        ""
    };
    let network_library = if network_enabled {
        " $(PROJECT_DIR)/network/libstasis_network.a"
    } else {
        ""
    };
    fs::write(
        output,
        format!(
            "GCC_PREPROCESSOR_DEFINITIONS = $(inherited) STASIS_GRAPHICS_SDL_ONLY=1{network_flags}\nFRAMEWORK_SEARCH_PATHS = $(inherited) $(STASIS_SDL_FRAMEWORKS)/SDL3.xcframework/ios-arm64 $(STASIS_SDL_FRAMEWORKS)/SDL3_image.xcframework/ios-arm64\nHEADER_SEARCH_PATHS = $(inherited) $(PROJECT_DIR)/../aot $(PROJECT_DIR)/../runtime $(STASIS_SDL_FRAMEWORKS)/SDL3.xcframework/ios-arm64/SDL3.framework/Headers $(STASIS_SDL_FRAMEWORKS)/SDL3_image.xcframework/ios-arm64/SDL3_image.framework/Headers{network_headers}\nLD_RUNPATH_SEARCH_PATHS = $(inherited) @executable_path/Frameworks\nOTHER_LDFLAGS = $(inherited) -framework SDL3 -framework SDL3_image{network_library} {object_flags}\n",
            network_flags = network_flags,
            network_headers = network_headers,
            network_library = network_library,
            object_flags = object_flags,
        ),
    )
    .map_err(|error| format!("failed to write {}: {error}", output.display()))
}

impl SymbolSelectorArgs {
    fn selector(&self) -> WorkshopSymbolSelector {
        WorkshopSymbolSelector {
            symbol_id: None,
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
            symbol_id: None,
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
            file: requested_files,
            owner,
            page,
            limit,
        } => {
            let limit = limit.clamp(1, 200);
            let mut items = workshop_source_items(&editable_files)?;
            if requested_files.len() > 16 {
                return Err("symbol list accepts at most 16 --file values".to_string());
            }
            let default_scope = requested_files.is_empty();
            let mut scope_files = if default_scope {
                vec![normalize_symbol_file(&workspace.manifest.entry)]
            } else {
                requested_files
                    .iter()
                    .map(|file| normalize_symbol_file(file))
                    .collect::<Vec<_>>()
            };
            if default_scope {
                scope_files.extend(workshop_direct_import_files(
                    &files,
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
                        workshop_direct_import_files(&files, Path::new(file))?,
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
    jit.set_project_root(display_path(&workspace.root))?;
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
    if !matches!(plan.schema_version, 1 | 2) {
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
    let (aot_object_code_bytes, literal_data_bytes) = compile_workspace_mobile_costs(workspace)?;
    let performance =
        jit.performance_cost_report(&memory, aot_object_code_bytes, literal_data_bytes)?;
    let mut data_flow = jit.function_data_flow_summaries().to_vec();
    let canonical_root = workspace
        .root
        .canonicalize()
        .unwrap_or_else(|_| workspace.root.clone());
    for summary in &mut data_flow {
        summary.file = relative_display(&canonical_root, Path::new(&summary.file));
    }
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
    if !data_flow.is_empty() {
        human.push("function data flow:".to_string());
        human.extend(data_flow.iter().map(|summary| {
            format!(
                "  {}: reads=[{}] writes=[{}] calls=[{}] host_calls=[{}] bounded_iterations={}",
                summary.function,
                summary.direct.reads.join(", "),
                summary.direct.writes.join(", "),
                summary.direct.calls.join(", "),
                summary.direct.host_calls.join(", "),
                summary.direct.bounded_iterations.len()
            )
        }));
    }
    if let Some(budget_us) = performance.tick_budget_us {
        human.push(format!(
            "tick budget: {budget_us} us (runtime average/p99 reported by play)"
        ));
    }
    if !performance.functions.is_empty() {
        human.push("bounded performance costs:".to_string());
        human.extend(performance.functions.iter().map(|function| {
            format!(
                "  {}: nested_product={} fields={} bytes_scanned={} pools={} host_calls=[{}]{}",
                function.function,
                function
                    .worst_nested_iteration_product
                    .map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                function.fields_scanned.len(),
                function.conservative_max_bytes_scanned,
                function.pools_iterated.len(),
                function
                    .host_calls
                    .iter()
                    .map(|call| format!(
                        "{}:{}",
                        call.function,
                        call.max_invocations
                            .map_or_else(|| "unknown".to_string(), |count| count.to_string())
                    ))
                    .collect::<Vec<_>>()
                    .join(", "),
                if function.structural_bound_complete {
                    ""
                } else {
                    " (incomplete bound)"
                }
            )
        }));
    }
    if !performance.layout_choices.is_empty() {
        human.push("layout choices (active lowering remains explicit SoA):".to_string());
        human.extend(performance.layout_choices.iter().map(|layout| {
            format!(
                "  {}: SoA={} bytes; AoS={} bytes (stride {}, padding {}/element); recommendation={}",
                layout.path,
                layout.soa_bytes,
                layout.aos_bytes,
                layout.aos_stride_bytes,
                layout.aos_padding_bytes_per_element,
                layout.recommendation
            )
        }));
    }
    human.push(format!(
        "mobile estimate: code={} data={} state={} buffers={} package={} peak_state_recommendation={}",
        performance.mobile.aot_object_code_bytes,
        performance.mobile.literal_data_bytes,
        performance.mobile.state_capacity_bytes,
        performance.mobile.command_buffer_bytes,
        performance.mobile.package_estimate_bytes,
        performance.mobile.peak_state_recommendation_bytes
    ));
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
            "function_data_flow": {
                "schema_version": stasis_compiler::data_flow::FUNCTION_DATA_FLOW_SCHEMA_VERSION,
                "functions": data_flow,
            },
            "performance": performance,
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

fn editor_info_result() -> Result<CommandResult, String> {
    let executable = env::current_exe()
        .map_err(|error| format!("failed to locate stasis executable: {error}"))?
        .canonicalize()
        .map_err(|error| format!("failed to resolve stasis executable: {error}"))?;
    let runtime = installed_runtime_library().ok_or_else(|| {
        format!(
            "the Stasis graphics runtime is not installed beside {}",
            executable.display()
        )
    })?;
    let runtime = runtime
        .canonicalize()
        .map_err(|error| format!("failed to resolve graphics runtime: {error}"))?;
    stasis_dynload::StasisGraphicsApi::load(&runtime)
        .map_err(|error| format!("the sibling graphics runtime is incompatible: {error}"))?;

    let version = env!("CARGO_PKG_VERSION");
    let release_id = option_env!("STASIS_RELEASE_ID").unwrap_or("development");
    let runtime_release_id = stasis_dynload::graphics_runtime_release_id(&runtime)?;
    if runtime_release_id != release_id {
        return Err(format!(
            "toolchain release mismatch: stasis is '{release_id}' but {} is '{runtime_release_id}'",
            runtime.display()
        ));
    }
    let source_commit = option_env!("STASIS_SOURCE_COMMIT").unwrap_or("development");
    let target = option_env!("STASIS_BUILD_TARGET")
        .map(str::to_string)
        .unwrap_or_else(|| format!("{}-{}", env::consts::ARCH, env::consts::OS));
    let data = json!({
        "schema": 1,
        "release_id": release_id,
        "version": version,
        "source_commit": source_commit,
        "target": target,
        "protocols": {
            "lsp": 1,
            "dap": 1,
            "live": 1,
            "graphics_abi": 1,
        },
        "executable": {
            "path": executable,
            "sha256": sha256_file(&executable)?,
        },
        "graphics_runtime": {
            "path": runtime,
            "release_id": runtime_release_id,
            "sha256": sha256_file(&runtime)?,
        },
    });
    Ok(CommandResult::success(
        format!("stasis editor toolchain {release_id} ({target})"),
        data,
    ))
}

fn wait_for_live_terminal_shutdown(
    terminal: &thread::JoinHandle<Result<(), String>>,
    runner_succeeded: bool,
) -> bool {
    if runner_succeeded {
        for _ in 0..200 {
            if terminal.is_finished() {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    terminal.is_finished()
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
        if candidate.join("stdlib.stasis").is_file()
            && candidate.join("internal/host_frame.stasis").is_file()
            && candidate.join("internal/gfx_cmd.stasis").is_file()
        {
            return Ok(candidate);
        }
    }
    Err("installed toolchain is missing the complete src/stdlib hierarchy; reinstall the complete release archive".to_string())
}

fn bundled_knowledge_docs_dir() -> Result<PathBuf, String> {
    let directory = bundled_toolchain_directory("docs/knowledge", "Stasis knowledge library")?;
    let missing: Vec<_> = KNOWLEDGE_FILES
        .iter()
        .filter(|document| !directory.join(document).is_file())
        .copied()
        .collect();
    if missing.is_empty() {
        Ok(directory)
    } else {
        Err(format!(
            "installed toolchain has an incomplete Stasis knowledge library at {} (missing {}); reinstall the complete release archive",
            directory.display(),
            missing.join(", ")
        ))
    }
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
        && name.trim_matches(' ') == name
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == ' ');
    if valid {
        Ok(())
    } else {
        Err(
            "project name must be 1-64 ASCII letters, digits, spaces, '-' or '_', without leading or trailing spaces"
                .to_string(),
        )
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

fn stage_web_loading_font(workspace: &Workspace, destination_root: &Path) -> Result<(), String> {
    let Some(path) = workspace
        .manifest
        .web
        .as_ref()
        .and_then(|web| web.loading_font.as_deref())
    else {
        return Ok(());
    };
    let path = normalize_web_loading_font_path(path)?;
    let source = workspace.root.join(&path);
    let destination = destination_root.join(&path);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    copy_file(&source, &destination)
}

fn normalize_web_loading_font_path(value: &str) -> Result<String, String> {
    let value = value.trim();
    let value = value.strip_prefix('/').unwrap_or(value);
    if value.is_empty() || value.contains('\\') {
        return Err(
            "web.loading_font must be an assets-relative path such as /assets/ui.ttf".to_string(),
        );
    }
    let path = Path::new(value);
    validate_relative_path("web.loading_font", path)?;
    let normalized = value.replace('\\', "/");
    if !normalized.starts_with("assets/") || normalized.len() == "assets/".len() {
        return Err("web.loading_font must point to a file under assets/".to_string());
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| "web.loading_font must have a web font extension".to_string())?;
    if !matches!(extension.as_str(), "ttf" | "otf" | "woff" | "woff2") {
        return Err(
            "web.loading_font must use a .ttf, .otf, .woff, or .woff2 extension".to_string(),
        );
    }
    Ok(normalized)
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

    #[test]
    fn generated_graphical_project_has_effect_boundaries_and_checks() {
        let root = temp_dir("effect_template");
        create_project(root.clone(), "effect_template".to_string()).expect("create project");
        let source = fs::read_to_string(root.join("src/main.stasis")).expect("read source");
        assert_eq!(
            source
                .matches("function @effects(state) tick(): i32")
                .count(),
            1
        );
        assert_eq!(
            source
                .matches("function @effects(graphics) render(): i32")
                .count(),
            1
        );
        assert!(!source.contains("function @effects(state, graphics)"));
        let workspace = load_workspace(Some(&root)).expect("load generated workspace");
        check_workspace(&workspace).expect("generated project checks");
        remove_temp(&root);
    }
    use stasis_ai::live_tool_specs;
    use stasis_compiler::frontend::types::TYPE_ID_U8;
    use std::collections::BTreeMap;

    #[test]
    fn release_web_index_omits_performance_hud() {
        let release = web_index_html("release-game", false, None);
        assert!(!release.contains("stasis-hud"));
        assert!(!release.contains("__STASIS_"));
        assert!(release.contains(r#"<h1 id="stasis-loading-title">release-game</h1>"#));
        assert!(release.contains(r#"id="stasis-loading-status">Preparing…</div>"#));
        assert!(release.contains(r#"id="stasis-loading" role="status" aria-live="polite""#));

        let development = web_index_html("development-game", true, None);
        assert!(development.contains(r#"id="stasis-hud""#));
        assert!(development.contains(r#"<h1 id="stasis-loading-title">development-game</h1>"#));
        for html in [&release, &development] {
            assert!(!html.contains("__STASIS_"));
            assert!(html.contains("#stasis-error { position: absolute;"));
            assert!(html.contains("inset: 0; margin: 0;"));
            assert!(html.contains("white-space: pre-wrap;"));
            assert!(html.contains("#stasis-error:empty { display: none; }"));
        }
    }

    #[test]
    fn configured_web_loading_font_is_preloaded_and_used_by_shell() {
        let html = web_index_html("font-game", false, Some("assets/fonts/ui.ttf"));
        assert!(html.contains(
            r#"<link rel="preload" href="assets/fonts/ui.ttf" as="font" type="font/ttf" crossorigin>"#
        ));
        assert!(html.contains(
            r#"@font-face { font-family: "StasisLoadingFont"; src: url("assets/fonts/ui.ttf") format("truetype");"#
        ));
        assert!(
            html.contains(r#"font-family: "StasisLoadingFont", Georgia, "Times New Roman", serif"#)
        );
        assert!(!html.contains("__STASIS_"));
    }

    #[test]
    fn web_loading_font_paths_accept_rooted_assets_and_reject_escape() {
        assert_eq!(
            normalize_web_loading_font_path("/assets/fonts/ui.ttf").unwrap(),
            "assets/fonts/ui.ttf"
        );
        assert_eq!(
            normalize_web_loading_font_path("assets/fonts/ui.woff2").unwrap(),
            "assets/fonts/ui.woff2"
        );
        for path in [
            "../assets/fonts/ui.ttf",
            "assets/../outside.ttf",
            "assets/fonts/ui.txt",
            "fonts/ui.ttf",
            "assets/fonts\\ui.ttf",
        ] {
            assert!(
                normalize_web_loading_font_path(path).is_err(),
                "accepted invalid web loading font path {path}"
            );
        }
    }

    #[test]
    fn lean_web_runtime_defers_window_mailbox_games_to_full_host() {
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&[
            "main".to_string(),
            "tick".to_string(),
            "render".to_string(),
        ]);
        process.upsert_file(
            "window.stasis",
            "global host_req_seq: i32; global host_req_flags: i32; global host_req_window_w_px: i32; global host_req_window_h_px: i32; function main(): i32 { host_req_flags = 1; host_req_window_w_px = 640; host_req_window_h_px = 360; host_req_seq += 1; return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile window mailbox game");
        assert!(lean_web_runtime(&process).is_none());
    }

    #[test]
    fn sprite_asset_tasks_survive_audio_feature_stripping() {
        let runtime = strip_web_runtime_feature(WEB_RUNTIME_JS, "audio", false);
        assert!(runtime.contains("const assetTasks = new Map()"));
        assert!(runtime.contains("const requestSprite = pathId =>"));
        assert!(runtime.contains("const releaseSprite = handle =>"));
        assert!(runtime.contains("stasis_jit_asset_request_sprite"));
        assert!(runtime.contains("stasis_jit_asset_task_poll"));
        assert!(runtime.contains("stasis_jit_gfx_release_sprite"));
        assert!(!runtime.contains("const requestAudio = pathId =>"));
        assert!(!runtime.contains("stasis_jit_asset_request_audio"));
        assert!(!runtime.contains("let audioContext"));
        assert!(!runtime.contains("@stasis-feature audio"));
    }

    #[test]
    fn release_web_runtime_keeps_only_required_host_interop() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/windows_launch_smoke");
        let workspace = load_workspace(Some(&root)).expect("load web sample workspace");
        let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let entry = Path::new("tests/stasis/seams/asset_extern_abi_probe.stasis");
        let files =
            load_workshop_edit_workspace(&source_root, entry).expect("load asset ABI sample files");
        let files = workshop_reachable_files(&files, entry).expect("prune asset ABI sample files");
        let mut process = WasmProcess::new();
        process
            .set_project_root(display_path(&source_root))
            .expect("set web sample root");
        process.set_required_emit_roots(&["main".to_string()]);
        for file in files {
            process.upsert_file(source_root.join(file.path).to_string_lossy(), file.source);
        }
        process.compile().expect("compile web sample");

        let release = web_runtime_config(&workspace, &process, false);
        let release_views = release["views"].as_object().expect("release views");
        assert!(!release_views.is_empty());
        assert!(release_views.values().all(|fields| fields
            .as_object()
            .expect("release view fields")
            .keys()
            .all(|field| WEB_RESOURCE_BINDING_FIELDS.contains(&field.as_str()))));
        assert!(release_views.values().any(|fields| fields
            .as_object()
            .expect("release view fields")
            .contains_key("handle")));
        let retained_view_paths = release_views
            .values()
            .flat_map(|fields| fields.as_object().expect("release view fields").values())
            .filter_map(Value::as_str)
            .collect::<BTreeSet<_>>();
        let release_memory = release["memory"].as_object().expect("release memory");
        assert!(release_memory
            .keys()
            .all(|path| retained_view_paths.contains(path.as_str())
                || WEB_RUNTIME_BUFFERS.contains(&path.as_str())));
        let release_globals = release["globals"].as_object().expect("release globals");
        assert!(release_globals.keys().all(|path| {
            WEB_HOST_GLOBALS.contains(&path.as_str()) || retained_view_paths.contains(path.as_str())
        }));
        for path in retained_view_paths {
            assert!(release_memory.contains_key(path) || release_globals.contains_key(path));
        }

        let development = web_runtime_config(&workspace, &process, true);
        assert!(development.get("views").is_some());
        assert!(development.get("globals").is_some());
        assert!(development.get("memory").is_some());
        assert!(
            development["views"]
                .as_object()
                .expect("development views")
                .len()
                >= release_views.len()
        );
        assert!(
            serde_json::to_vec(&release)
                .expect("encode release runtime")
                .len()
                < serde_json::to_vec(&development)
                    .expect("encode development runtime")
                    .len()
        );
    }

    #[test]
    fn release_web_runtime_retains_hashed_u8_layouts_for_memcpy() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/windows_launch_smoke");
        let workspace = load_workspace(Some(&root)).expect("load web sample workspace");
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&[
            "main".to_string(),
            "tick".to_string(),
            "render".to_string(),
        ]);
        process.upsert_file(
            "memcpy.stasis",
            "extern function sys_memcpy_u8(dst: u8[], dst_index: i32, src: u8[], src_index: i32, count: i32): void; global source: u8[4]; global destination: u8[4]; global source_utf8: utf8[4]; global destination_ascii: ascii[4]; global scratch: u8[2]; global unrelated: i32[4]; function main(): i32 { source[0] = 65; source_utf8[0] = 66; sys_memcpy_u8(destination, 0, source, 0, 4); sys_memcpy_u8(destination_ascii, 0, source_utf8, 0, 1); return destination[0]; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile web memcpy fixture");
        assert!(process.imported_symbols().contains("sys_memcpy_u8"));

        let release = web_runtime_config(&workspace, &process, false);
        let memory = release["memory"].as_object().expect("release memory");
        for path in ["source", "destination", "scratch"] {
            let layout = memory
                .get(path)
                .unwrap_or_else(|| panic!("release omitted u8 layout {path}"));
            assert_eq!(layout["type_id"], json!(TYPE_ID_U8));
            assert_eq!(
                layout["hash"],
                json!(stasis_compiler::backend::wasm::wasm_global_hash(path))
            );
        }
        for path in ["source_utf8", "destination_ascii"] {
            let layout = memory
                .get(path)
                .unwrap_or_else(|| panic!("release omitted byte-backed layout {path}"));
            assert_eq!(layout["byte_backed"], json!(true));
            assert_eq!(
                layout["hash"],
                json!(stasis_compiler::backend::wasm::wasm_global_hash(path))
            );
        }
        assert!(!memory.contains_key("unrelated"));

        let runtime = link_web_runtime(&process, false, false);
        assert!(runtime.contains("const sysMemcpyU8 ="));
        assert!(runtime.contains("sys_memcpy_u8: sysMemcpyU8"));
    }

    #[test]
    fn release_web_runtime_retains_typed_layouts_for_memcpy() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/windows_launch_smoke");
        let workspace = load_workspace(Some(&root)).expect("load web sample workspace");
        let mut process = WasmProcess::new();
        process.set_required_emit_roots(&[
            "main".to_string(),
            "tick".to_string(),
            "render".to_string(),
        ]);
        process.upsert_file(
            "typed_memcpy.stasis",
            "extern function sys_memcpy_i32(dst: i32[], dst_index: i32, src: i32[], src_index: i32, count: i32): void; extern function sys_memcpy_f32(dst: f32[], dst_index: i32, src: f32[], src_index: i32, count: i32): void; global i32_source: i32[4]; global i32_destination: i32[4]; global i32_scratch: i32[2]; global f32_source: f32[4]; global f32_destination: f32[4]; global f32_scratch: f32[2]; global unrelated: u8[4]; function main(): i32 { i32_source[0] = 41; f32_source[0] = 1.0; sys_memcpy_i32(i32_destination, 0, i32_source, 0, 4); sys_memcpy_f32(f32_destination, 0, f32_source, 0, 4); return i32_destination[0]; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
        );
        process.compile().expect("compile typed web memcpy fixture");
        assert!(process.imported_symbols().contains("sys_memcpy_i32"));
        assert!(process.imported_symbols().contains("sys_memcpy_f32"));

        let release = web_runtime_config(&workspace, &process, false);
        let memory = release["memory"].as_object().expect("release memory");
        for (paths, type_id) in [
            (
                ["i32_source", "i32_destination", "i32_scratch"],
                TYPE_ID_I32,
            ),
            (
                ["f32_source", "f32_destination", "f32_scratch"],
                TYPE_ID_F32,
            ),
        ] {
            for path in paths {
                let layout = memory
                    .get(path)
                    .unwrap_or_else(|| panic!("release omitted typed layout {path}"));
                assert_eq!(layout["type_id"], json!(type_id));
                assert_eq!(
                    layout["hash"],
                    json!(stasis_compiler::backend::wasm::wasm_global_hash(path))
                );
                assert_eq!(
                    layout["offset"],
                    json!(
                        process
                            .memory_layout()
                            .get(path)
                            .expect("typed layout")
                            .offset
                    )
                );
            }
        }
        assert!(!memory.contains_key("unrelated"));
    }

    #[test]
    fn successful_live_runner_allows_terminal_acknowledgement_to_finish() {
        let terminal = thread::spawn(|| {
            thread::sleep(Duration::from_millis(25));
            Ok(())
        });
        assert!(wait_for_live_terminal_shutdown(&terminal, true));
        terminal
            .join()
            .expect("join terminal")
            .expect("terminal result");
    }

    #[test]
    fn web_network_runtime_is_feature_stripped_for_normal_games() {
        let stripped = strip_web_runtime_feature(WEB_RUNTIME_JS, "network", false);
        assert!(!stripped.contains("stasis_web_network_connect"));
        assert!(!stripped.contains("stasis_web_network_checkpoint"));
        let linked = strip_web_runtime_feature(WEB_RUNTIME_JS, "network", true);
        assert!(linked.contains("stasis_web_network_connect"));
        assert!(linked.contains("stasis_web_network_checkpoint"));
    }

    #[test]
    fn inspect_exposes_compiler_function_data_flow() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../samples/function_data_flow");
        let workspace = load_workspace(Some(&root)).expect("load sample workspace");
        let result = inspect_workspace(&workspace, &[], MAX_STATE_SNAPSHOT_BYTES as u64)
            .expect("inspect sample workspace");
        let functions = result.data["function_data_flow"]["functions"]
            .as_array()
            .expect("data-flow functions");
        let tick = functions
            .iter()
            .find(|summary| summary["function"] == "tick")
            .expect("tick summary");

        assert_eq!(
            tick["schema_version"],
            stasis_compiler::data_flow::FUNCTION_DATA_FLOW_SCHEMA_VERSION
        );
        assert_eq!(tick["file"], "src/main.stasis");
        assert_eq!(
            tick["signature_hash"]
                .as_str()
                .expect("signature hash")
                .len(),
            16
        );
        assert_eq!(tick["direct"]["calls"], json!(["sum_enemy_health"]));
        assert_eq!(tick["direct"]["host_calls"], json!(["print_i32"]));
        assert_eq!(tick["direct"]["bounded_iterations"][0]["max_iterations"], 3);
        assert!(result.human.contains("function data flow:"));
    }

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
    fn human_live_output_formats_shared_language_queries() {
        let diagnostics = LiveResponse::success(8, 42, "diagnostics", json!({"diagnostics": []}));
        assert_eq!(
            format_live_response(&diagnostics),
            "no compiler diagnostics"
        );

        let hover = LiveResponse::success(
            9,
            43,
            "hover",
            json!({"hover": {
                "symbol": "score",
                "type_name": "i32",
                "live_value": "12 (tick 43)"
            }}),
        );
        assert_eq!(format_live_response(&hover), "score: i32 = 12 (tick 43)");

        let definition = LiveResponse::success(
            10,
            43,
            "definition",
            json!({"locations": [{
                "file": "src/main.stasis",
                "start": 7,
                "end": 12
            }]}),
        );
        assert_eq!(format_live_response(&definition), "src/main.stasis:7..12");

        let rename = LiveResponse::success(
            11,
            43,
            "rename_preview",
            json!({
                "old_name": "score",
                "new_name": "points",
                "edits": [{"file": "src/main.stasis"}, {"file": "src/ui.stasis"}]
            }),
        );
        assert_eq!(
            format_live_response(&rename),
            "rename score -> points (2 validated edit(s))"
        );

        let organize = LiveResponse::success(
            12,
            43,
            "code_actions",
            json!({"actions": [{
                "title": "Organize Stasis imports",
                "edits": [{"file": "src/main.stasis"}]
            }]}),
        );
        assert_eq!(
            format_live_response(&organize),
            "Organize Stasis imports (1 edit(s), preview only)"
        );

        let inlays = LiveResponse::success(
            13,
            43,
            "inlay_hints",
            json!({
                "file": "src/main.stasis",
                "hints": [{
                    "kind": "type",
                    "start": 21,
                    "end": 26,
                    "label": ": i32"
                }]
            }),
        );
        assert_eq!(
            format_live_response(&inlays),
            "type @ src/main.stasis:21..26 : i32"
        );
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

    #[test]
    fn elapsed_confirmation_uses_readable_units() {
        for (elapsed, expected) in [
            (Duration::from_millis(482), "built\nCompleted in 482ms."),
            (Duration::from_millis(1_250), "built\nCompleted in 1.25s."),
            (
                Duration::from_millis(301_500),
                "built\nCompleted in 5m 1.5s.",
            ),
        ] {
            let mut output = "built".to_string();
            append_elapsed_confirmation(&mut output, elapsed);
            assert_eq!(output, expected);
        }
    }

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
    fn development_web_wasm_keeps_diagnostic_module_unchanged() {
        let root = temp_dir("development_web_wasm");
        fs::create_dir_all(&root).expect("create optimizer fixture");
        let module = b"\0asm\x01\0\0\0diagnostic-names";
        let artifact = prepare_web_wasm(module, &root, true).expect("prepare development Wasm");
        assert!(!artifact.optimized);
        assert_eq!(artifact.input_bytes, module.len());
        assert_eq!(artifact.bytes, module);
        remove_temp(&root);
    }

    #[test]
    fn configured_wasm_opt_produces_a_valid_release_module() {
        if env::var_os("STASIS_WASM_OPT").is_none() {
            return;
        }
        let root = temp_dir("release_web_wasm");
        fs::create_dir_all(&root).expect("create optimizer fixture");
        let module = b"\0asm\x01\0\0\0";
        let artifact = prepare_web_wasm(module, &root, false).expect("optimize release Wasm");
        assert!(artifact.optimized);
        assert!(artifact.bytes.starts_with(b"\0asm\x01\0\0\0"));
        assert!(!root.join(".game.unoptimized.wasm").exists());
        assert!(!root.join(".game.optimized.wasm").exists());
        remove_temp(&root);
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

    #[cfg(unix)]
    #[test]
    fn workspace_alias_is_resolved_before_compiler_identity_is_created() {
        use std::os::unix::fs::symlink;

        let real_parent = temp_dir("real_workspace_parent");
        let real_root = real_parent.join("project");
        let alias_parent = temp_dir("workspace_alias_parent");
        let alias_root = alias_parent.join("project_alias");
        create_project(real_root.clone(), "alias_project".to_string()).expect("create project");
        fs::create_dir_all(&alias_parent).expect("create alias parent");
        symlink(&real_root, &alias_root).expect("create workspace alias");

        let workspace = load_workspace(Some(&alias_root)).expect("load workspace through alias");
        assert_eq!(workspace.root, real_root.canonicalize().expect("real root"));
        let jit = compile_workspace_jit(&workspace).expect("compile aliased workspace");
        let main = jit
            .program_snapshot()
            .expect("program snapshot")
            .functions()
            .iter()
            .find(|function| function.name == "main")
            .expect("main function");
        assert_eq!(
            main.symbol_id.canonical(),
            "v1|function|src/main.stasis|main|()"
        );

        fs::remove_file(&alias_root).expect("remove workspace alias");
        remove_temp(&alias_parent);
        remove_temp(&real_parent);
    }

    #[test]
    fn source_formatter_is_deterministic() {
        let source = "function main(): i32 {  \r\n    return 0;\t\r\n}\r\n\r\n";
        let expected = "function main(): i32 {\r\n    return 0;\r\n}\r\n";
        assert_eq!(format_source(source).expect("format"), expected);
        assert_eq!(format_source(expected).expect("format"), expected);
    }

    #[test]
    fn generated_project_checks_tests_and_runs_through_jit() {
        let root = temp_dir("smoke");
        create_project(root.clone(), "smoke".to_string()).expect("create project");
        let source = fs::read_to_string(root.join("src/main.stasis")).expect("read main source");
        for module in [
            "stdlib",
            "graphics",
            "audio",
            "collision",
            "flex_layout",
            "frame_timer",
            "hud_table",
            "sdl_scancodes",
            "storage",
            "ui_axis_layout",
            "ui_layout_audit",
            "ui_button_9slice",
        ] {
            assert!(
                source.contains(&format!("/vendor/stasis/stdlib/{module}.stasis")),
                "missing default {module} import"
            );
        }
        let workspace = load_workspace(Some(&root)).expect("load workspace");
        check_workspace(&workspace).expect("check project");
        test_workspace(&workspace, None).expect("test project");
        let run = run_workspace(&workspace, true, 0, false).expect("run project");
        assert_eq!(run.code, 0);
        remove_temp(&root);
    }

    fn write_data_binding_test_project(root: &Path, data: Option<&str>, metadata: Option<&str>) {
        create_project(root.to_path_buf(), "data_binding_test".to_string())
            .expect("create project");
        fs::write(
            root.join("src/main.stasis"),
            "struct Config { loaded: bool; scalar: i32; values: i32[2]; }\nstruct State { config: Config; }\nglobal state: State;\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        fs::write(
            root.join("tests/main.test.stasis"),
            "import \"../src/main.stasis\";\ntest `bound data reaches globals`(): bool { return state.config.loaded && state.config.scalar == 17 && state.config.values[0] == 4 && state.config.values[1] == 9; }\n",
        )
        .expect("write test");
        if let Some(data) = data {
            fs::create_dir_all(root.join("data")).expect("data directory");
            fs::write(root.join("data/gameplay.json"), data).expect("write data");
        }
        if let Some(metadata) = metadata {
            fs::create_dir_all(root.join("data")).expect("data directory");
            fs::write(root.join("data/gameplay.struct-meta.json"), metadata)
                .expect("write metadata");
        }
    }

    const DATA_BINDING_META: &str = r#"{
  "version": 1,
  "globalName": "state",
  "totalSize": 0,
  "fields": [
    {"name":"state__config__loaded","jsonPath":"config.loaded","offset":0,"size":1,"type":"bool","arrayCount":1},
    {"name":"state__config__scalar","jsonPath":"config.scalar","offset":4,"size":4,"type":"i32","arrayCount":1},
    {"name":"state__config__values","jsonPath":"config.values","offset":8,"size":8,"type":"i32","arrayCount":2}
  ]
}"#;

    #[test]
    fn workspace_tests_apply_project_json_scalar_and_array_bindings() {
        let root = temp_dir("test_data_binding");
        write_data_binding_test_project(
            &root,
            Some(r#"{"config":{"loaded":true,"scalar":17,"values":[4,9]}}"#),
            Some(DATA_BINDING_META),
        );
        let workspace = load_workspace(Some(&root)).expect("workspace");
        test_workspace(&workspace, None).expect("bound test project");
        remove_temp(&root);
    }

    #[test]
    fn workspace_tests_skip_project_bindings_outside_the_test_import_graph() {
        let root = temp_dir("test_data_binding_scoped_imports");
        write_data_binding_test_project(
            &root,
            Some(r#"{"config":{"loaded":true,"scalar":17,"values":[4,9]}}"#),
            Some(DATA_BINDING_META),
        );
        fs::write(
            root.join("tests/independent.test.stasis"),
            "global independent_value: i32;\ntest `independent test omits project globals`(): bool { return independent_value == 0; }\n",
        )
        .expect("write independent test");
        let workspace = load_workspace(Some(&root)).expect("workspace");
        let result = test_workspace(&workspace, None).expect("scoped binding test project");
        assert_eq!(result.data["tests_run"], 2);
        assert_eq!(result.data["tests_passed"], 2);
        remove_temp(&root);
    }

    #[test]
    fn workspace_tests_reject_invalid_project_json_binding_deterministically() {
        let root = temp_dir("test_data_binding_invalid");
        write_data_binding_test_project(
            &root,
            Some(r#"{"config":{"loaded":true,"scalar":17,"values":[4,9],"extra":1}}"#),
            Some(DATA_BINDING_META),
        );
        let workspace = load_workspace(Some(&root)).expect("workspace");
        let error = test_workspace(&workspace, None).expect_err("extra data property rejected");
        assert!(
            error.contains("binding source property config.extra"),
            "{error}"
        );
        remove_temp(&root);
    }

    #[test]
    fn workspace_tests_reject_malformed_and_missing_project_metadata() {
        let malformed_root = temp_dir("test_data_binding_malformed");
        write_data_binding_test_project(
            &malformed_root,
            Some("{not-json"),
            Some(DATA_BINDING_META),
        );
        let malformed_workspace = load_workspace(Some(&malformed_root)).expect("workspace");
        let malformed =
            test_workspace(&malformed_workspace, None).expect_err("malformed data rejected");
        assert!(
            malformed.contains("failed to parse data JSON"),
            "{malformed}"
        );
        remove_temp(&malformed_root);

        let missing_root = temp_dir("test_data_binding_missing_meta");
        write_data_binding_test_project(
            &missing_root,
            Some(r#"{"config":{"loaded":true,"scalar":17,"values":[4,9]}}"#),
            None,
        );
        let missing_workspace = load_workspace(Some(&missing_root)).expect("workspace");
        let missing =
            test_workspace(&missing_workspace, None).expect_err("missing metadata rejected");
        assert!(missing.contains("requires matching metadata"), "{missing}");
        remove_temp(&missing_root);
    }
    #[test]
    fn workspace_tests_without_project_data_keep_zero_initialized_globals() {
        let root = temp_dir("test_data_binding_none");
        create_project(root.clone(), "data_binding_none".to_string()).expect("create project");
        fs::write(
            root.join("src/main.stasis"),
            "global value: i32;\nfunction main(): i32 { return 0; }\n",
        )
        .expect("write source");
        fs::write(
            root.join("tests/main.test.stasis"),
            "import \"../src/main.stasis\";\ntest `no data leaves defaults`(): bool { return value == 0; }\n",
        )
        .expect("write test");
        let workspace = load_workspace(Some(&root)).expect("workspace");
        test_workspace(&workspace, None).expect("no-data test project");
        remove_temp(&root);
    }
    #[test]
    fn vendor_upgrade_only_rewrites_the_release_when_content_changes() {
        let root = temp_dir("vendor_upgrade");
        create_project(root.clone(), "vendor_upgrade".to_string()).expect("create project");
        let manifest_path = root.join(MANIFEST_NAME);
        let mut manifest: ProjectManifest =
            serde_json::from_slice(&fs::read(&manifest_path).expect("read generated manifest"))
                .expect("parse generated manifest");
        let vendor = manifest.vendor.as_mut().expect("tracked vendor");
        vendor.stasis.release_id = "older-toolchain".to_string();
        write_manifest(&manifest_path, &manifest).expect("record older vendor");
        let unchanged_manifest = fs::read(&manifest_path).expect("read manifest before load");
        let workspace = load_workspace(Some(&root)).expect("load with a new toolchain release");
        assert_eq!(
            workspace
                .manifest
                .vendor
                .expect("tracked vendor")
                .stasis
                .release_id,
            "older-toolchain"
        );
        assert_eq!(
            fs::read(&manifest_path).expect("read manifest after load"),
            unchanged_manifest,
            "a release-only toolchain rebuild must not rewrite the vendor manifest"
        );

        let older_source = root.join("vendor/stasis/stdlib/audio.stasis");
        let mut older_contents = fs::read_to_string(&older_source).expect("read vendor source");
        older_contents.push_str("// older clean snapshot\r\n");
        fs::write(&older_source, older_contents).expect("write older clean snapshot");
        let vendor = manifest.vendor.as_mut().expect("tracked vendor");
        vendor.stasis.sha256 =
            directory_sha256(&root.join("vendor/stasis")).expect("hash older clean snapshot");
        write_manifest(&manifest_path, &manifest).expect("record older content hash");
        let workspace = load_workspace(Some(&root)).expect("content-hash update");
        let updated = workspace.manifest.vendor.expect("updated vendor").stasis;
        assert_eq!(updated.release_id, current_release_id());
        assert_eq!(
            updated.sha256,
            directory_sha256(&root.join("vendor/stasis")).expect("hash updated vendor")
        );
        remove_temp(&root);
    }

    #[test]
    fn vendor_upgrade_flattens_the_legacy_src_directory() {
        let root = temp_dir("vendor_flatten");
        create_project(root.clone(), "vendor_flatten".to_string()).expect("create project");
        let vendor_root = root.join("vendor/stasis");
        let old_contents = root.join("vendor/stasis-old");
        fs::rename(&vendor_root, &old_contents).expect("stage flat vendor contents");
        fs::create_dir_all(&vendor_root).expect("recreate vendor root");
        fs::rename(&old_contents, vendor_root.join("src")).expect("create legacy layout");

        load_workspace(Some(&root)).expect("upgrade legacy vendor layout");

        assert!(vendor_root.join("stdlib/stdlib.stasis").is_file());
        assert!(vendor_root.join("docs/README.md").is_file());
        assert!(!vendor_root.join("src").exists());
        remove_temp(&root);
    }

    #[test]
    fn automatic_upgrade_replaces_vendor_edits_owned_by_stasis() {
        let root = temp_dir("vendor_local_edits");
        create_project(root.clone(), "vendor_local_edits".to_string()).expect("create project");
        let edited = root.join("vendor/stasis/stdlib/audio.stasis");
        fs::write(&edited, "// local vendor edit\n").expect("edit vendor source");
        let removed_doc =
            root.join("vendor/stasis/docs/a-little-stasis/03-a-tick-is-an-ordered-recipe.md");
        fs::remove_file(&removed_doc).expect("remove vendor knowledge document");

        let current = load_workspace(Some(&root)).expect("automatic vendor replacement");
        assert_ne!(
            fs::read_to_string(&edited).expect("read restored vendor"),
            "// local vendor edit\n"
        );
        assert!(removed_doc.is_file());
        assert_eq!(
            current
                .manifest
                .vendor
                .expect("tracked vendor")
                .stasis
                .release_id,
            current_release_id()
        );
        remove_temp(&root);
    }

    #[test]
    fn vendor_cli_parses_status_and_update() {
        for args in [
            vec!["stasis", "vendor", "status"],
            vec!["stasis", "vendor", "update"],
        ] {
            ToolchainCli::try_parse_from(args).expect("parse vendor command");
        }
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
            "--ticks",
            "3",
            "--fast-forward",
        ])
        .expect("parse run flags");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::Run {
                watch: true,
                headless: true,
                ticks: 3,
                fast_forward: true,
                ..
            }
        ));
    }

    #[test]
    fn record_accepts_exact_sixty_fps_and_frame_count() {
        let parsed = ToolchainCli::try_parse_from([
            "stasis",
            "--workspace",
            "demo",
            "record",
            "src/main.stasis",
            "--output",
            "artifacts/frames",
            "--width",
            "640",
            "--height",
            "360",
            "--fps",
            "60",
            "--frames",
            "12",
        ])
        .expect("parse recording command");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::Record { args }
                if args.entry == Some(PathBuf::from("src/main.stasis"))
                    && args.width == 640
                    && args.height == 360
                    && args.fps == 60
                    && args.frames == Some(12)
                    && args.duration.is_none()
        ));
    }

    #[test]
    fn record_requires_one_duration_or_frame_count() {
        assert!(ToolchainCli::try_parse_from([
            "stasis", "record", "--output", "frames", "--width", "1", "--height", "1", "--fps",
            "60"
        ])
        .is_err());
        assert!(ToolchainCli::try_parse_from([
            "stasis",
            "record",
            "--output",
            "frames",
            "--width",
            "1",
            "--height",
            "1",
            "--fps",
            "60",
            "--frames",
            "1",
            "--duration",
            "1"
        ])
        .is_err());
    }

    #[test]
    fn replay_accepts_a_recording_and_optional_entry() {
        let parsed = ToolchainCli::try_parse_from([
            "stasis",
            "--workspace",
            "demo",
            "replay",
            "runs/game.replay.json",
            "--entry",
            "src/alternate.stasis",
            "--tick-sleep-us",
            "0",
        ])
        .expect("parse replay command");
        assert!(matches!(
            parsed.command,
            ToolchainCommand::Replay {
                recording,
                entry: Some(entry),
                tick_sleep_us: 0,
            } if recording == PathBuf::from("runs/game.replay.json")
                && entry == PathBuf::from("src/alternate.stasis")
        ));
    }

    #[test]
    fn tui_entry_is_optional_and_accepts_an_override() {
        let manifest_entry =
            ToolchainCli::try_parse_from(["stasis", "tui"]).expect("parse manifest TUI entry");
        assert!(matches!(
            manifest_entry.command,
            ToolchainCommand::Tui { entry: None, .. }
        ));

        let explicit_entry = ToolchainCli::try_parse_from([
            "stasis",
            "tui",
            "samples/state_inspection/src/main.stasis",
        ])
        .expect("parse explicit TUI entry");
        assert!(matches!(
            explicit_entry.command,
            ToolchainCommand::Tui {
                entry: Some(ref entry),
                ..
            } if entry == Path::new("samples/state_inspection/src/main.stasis")
        ));
    }

    #[test]
    fn editor_transports_are_explicit_and_mutually_exclusive() {
        let formatted = ToolchainCli::try_parse_from(["stasis", "format", "--stdin"])
            .expect("parse stdin formatter");
        assert!(matches!(
            formatted.command,
            ToolchainCommand::Fmt {
                stdin: true,
                check: false,
                ref paths,
            } if paths.is_empty()
        ));
        assert!(
            ToolchainCli::try_parse_from(["stasis", "format", "--stdin", "src/main.stasis",])
                .is_err()
        );
        assert!(ToolchainCli::try_parse_from(["stasis", "format", "--stdin", "--check"]).is_err());

        let live = ToolchainCli::try_parse_from(["stasis", "tui", "--live-stdio"])
            .expect("parse live stdio");
        assert!(matches!(
            live.command,
            ToolchainCommand::Tui {
                live_stdio: true,
                live_script: None,
                ..
            }
        ));
        assert!(ToolchainCli::try_parse_from([
            "stasis",
            "tui",
            "--live-stdio",
            "--live-script",
            "commands.txt",
        ])
        .is_err());
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
                ..
            } if entry == Path::new("src/mobile.stasis") && out == Path::new("dist/ios")
        ));
    }

    #[test]
    fn local_release_provenance_keeps_release_behavior_without_claiming_official_status() {
        let provenance = local_provenance(false).expect("local release provenance");
        assert_eq!(provenance["build_class"], "local_release");
        assert_eq!(provenance["development_build"], false);
        assert!(provenance["release_tag"].is_null());
        assert!(provenance["compiler"]["sha256"].as_str().is_some());

        let root = temp_dir("local_release_provenance");
        fs::create_dir_all(&root).expect("local release header directory");
        write_mobile_provenance_header(&root, &provenance).expect("local release header");
        assert!(fs::read_to_string(root.join("stasis_package_provenance.h"))
            .expect("read local release header")
            .contains("local release"));
        remove_temp(&root);
    }

    #[test]
    fn development_provenance_is_always_marked_dirty() {
        let provenance = local_provenance(true).expect("development provenance");
        assert_eq!(provenance["build_class"], "development");
        assert_eq!(provenance["development_build"], true);
        assert_eq!(provenance["dirty_state"], true);
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
            "command_buffer": {"name": "gfx_cmd", "version": 4},
            "backends": ["sdl3"],
            "features": ["aot", "jit", "mobile-aot", "shared-renderer"],
            "dependencies": {
                "cargo_lock_sha256": "fixture",
                "cargo_packages": ["fixture 1.0.0 workspace"],
                "sdl3": "3.4.10-static",
                "sdl3_image": "3.4.4-static",
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
        assert_eq!(
            mobile_package_id("Chess TD"),
            "com.stasislang.gamechessx20td"
        );
        for name in ["mobile_game", "123-game", "Chess TD"] {
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
    fn project_names_allow_internal_ascii_spaces() {
        assert!(validate_project_name("Chess TD").is_ok());
        for invalid in [" Chess TD", "Chess TD ", "Chess\tTD", "Chess/TD"] {
            assert!(
                validate_project_name(invalid).is_err(),
                "accepted {invalid:?}"
            );
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
            manifest: ProjectManifest {
                android: Some(AndroidProjectManifest {
                    application_id: "com.example.mobile".to_string(),
                    label: "Mobile Smoke".to_string(),
                    orientation: "fullSensor".to_string(),
                    version_code: 7,
                    version_name: "2.1.0".to_string(),
                }),
                ..ProjectManifest::new("mobile_smoke".to_string())
            },
        };

        let android = root.join("android-package");
        fs::create_dir_all(&android).expect("create Android staging");
        let provenance = local_provenance(true).expect("development provenance");
        assemble_mobile_shell(
            &workspace,
            PackageTarget::AndroidArm64,
            &aot,
            &android,
            &provenance,
            None,
        )
        .expect("assemble Android shell");
        let android_cmake =
            fs::read_to_string(android.join("android/app/src/main/cpp/CMakeLists.txt"))
                .expect("read Android CMake");
        assert!(android_cmake.contains("stasis_mobile_runtime"));
        assert!(android_cmake.contains("published_aot_objects.cmake"));
        assert!(android_cmake.contains("STASIS_PUBLISHED_AOT_OBJECTS"));
        assert!(!android_cmake.contains("file(GLOB STASIS_AOT_OBJECTS"));
        assert!(android_cmake.contains("libmain.map"));
        assert!(!android_cmake.contains("stasis_dynload"));
        let android_gradle = fs::read_to_string(android.join("android/app/build.gradle"))
            .expect("read Android Gradle");
        assert!(android_gradle.contains("applicationId 'com.example.mobile'"));
        assert!(android_gradle.contains("versionCode 7"));
        assert!(android_gradle.contains("versionName '2.1.0'"));
        let android_manifest =
            fs::read_to_string(android.join("android/app/src/main/AndroidManifest.xml"))
                .expect("read Android manifest");
        assert!(android_manifest.contains("android:appCategory=\"game\""));
        assert!(android_manifest.contains(
            "android:name=\"android.window.PROPERTY_COMPAT_ALLOW_ORIENTATION_OVERRIDE\""
        ));
        assert!(android_manifest.contains(
            "android:name=\"android.window.PROPERTY_COMPAT_ALLOW_USER_ASPECT_RATIO_OVERRIDE\""
        ));
        assert!(android_manifest.matches("android:value=\"false\"").count() >= 2);
        assert!(android_manifest.contains("android:label=\"Mobile Smoke\""));
        assert!(android_manifest.contains("android:screenOrientation=\"fullSensor\""));
        let mobile_main = fs::read_to_string(android.join("common/stasis_mobile_main.c"))
            .expect("read shared mobile main")
            .replace("\r\n", "\n");
        assert!(mobile_main.contains("stasis_mobile_runtime_last_entry_result"));
        assert!(mobile_main.contains("Stasis seam:"));
        assert!(mobile_main.contains("seam_state_checksum"));
        assert!(mobile_main.contains("resource_state"));
        assert!(mobile_main.contains("renderer_generation"));
        assert!(mobile_main.contains("restore_failures"));
        assert!(mobile_main.contains("frame == 1"));
        assert!(mobile_main.contains("frame % 30 == 0"));
        assert!(mobile_main.contains(
            "} else {\n#if defined(__APPLE__) && !defined(__ANDROID__) && defined(STASIS_NETWORK_ENABLED)"
        ));
        assert!(mobile_main.contains("stasis_mobile_network_present_join_url();"));
        assert!(mobile_main.contains("if (seam_test_id != NULL && seam_test_id[0] != '\\0')"));
        assert!(!mobile_main.contains("}\n#if defined(STASIS_ENABLE_SEAM_TESTS)\n    else if"));
        let android_activity = fs::read_to_string(
            android.join("android/app/src/main/java/com/stasislang/game/MainActivity.java"),
        )
        .expect("read Android activity");
        assert!(android_activity.contains("stasis.seam_test_id"));
        assert!(android_activity.contains("BuildConfig.STASIS_SEAM_TESTS"));
        assert!(android_activity.contains("nativeSetSeamTestId"));
        assert!(android_activity
            .contains("private static final String STASIS_ANDROID_ORIENTATION = \"fullSensor\";"));
        assert!(!android_activity.contains("@STASIS_ANDROID_ORIENTATION@"));
        assert!(android_activity.contains("public void setOrientationBis"));
        assert!(android_activity.contains("ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE"));
        assert!(android_activity.contains("ActivityInfo.SCREEN_ORIENTATION_SENSOR_PORTRAIT"));
        assert!(android_activity.contains("ActivityInfo.SCREEN_ORIENTATION_FULL_SENSOR"));
        assert!(
            android_activity.contains("super.setOrientationBis(width, height, resizable, hint);")
        );

        let mut landscape_workspace = workspace.clone();
        landscape_workspace
            .manifest
            .android
            .as_mut()
            .expect("Android manifest")
            .orientation = "sensorLandscape".to_string();
        let android_landscape = root.join("android-landscape-package");
        fs::create_dir_all(&android_landscape).expect("create landscape Android staging");
        assemble_mobile_shell(
            &landscape_workspace,
            PackageTarget::AndroidArm64,
            &aot,
            &android_landscape,
            &provenance,
            None,
        )
        .expect("assemble landscape Android shell");
        let landscape_activity = fs::read_to_string(
            android_landscape
                .join("android/app/src/main/java/com/stasislang/game/MainActivity.java"),
        )
        .expect("read landscape Android activity");
        assert!(landscape_activity.contains(
            "private static final String STASIS_ANDROID_ORIENTATION = \"sensorLandscape\";"
        ));
        assert!(!landscape_activity.contains("@STASIS_ANDROID_ORIENTATION@"));
        let landscape_manifest =
            fs::read_to_string(android_landscape.join("android/app/src/main/AndroidManifest.xml"))
                .expect("read landscape Android manifest");
        assert!(landscape_manifest.contains("android:screenOrientation=\"sensorLandscape\""));

        let android_jni =
            fs::read_to_string(android.join("android/app/src/main/cpp/stasis_android_assets.c"))
                .expect("read Android JNI bridge");
        assert!(android_jni.contains("STASIS_ENABLE_TEST_INPUT"));
        let android_gradle = fs::read_to_string(android.join("android/app/build.gradle"))
            .expect("read Android Gradle build");
        assert!(android_gradle.contains("stasisSeamTests"));
        let android_cmake =
            fs::read_to_string(android.join("android/app/src/main/cpp/CMakeLists.txt"))
                .expect("read Android CMake project");
        assert!(android_cmake.contains("STASIS_ENABLE_SEAM_TESTS"));
        assert!(android_jni.contains("STASIS_SEAM_TEST_ID"));
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
            .join("runtime/stasis_performance_metrics.h")
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
        assert!(java.contains("event.getPointerCount() >= 3"));
        assert!(java.contains("nativeReadPerformanceMetrics"));
        assert!(java.contains("nativeSetPerformanceMetricsEnabled(show)"));
        assert!(java.contains("nativeReadRuntimeError"));
        assert!(java.contains("guest render"));
        assert!(java.contains("host replay"));
        assert!(java.contains("frame work"));
        assert!(java.contains("appendWorkload"));
        assert!(!java.contains("percentile("));
        assert!(java.contains("performanceHud.setSingleLine(false)"));
        assert!(java.contains("verifyAssetManifest(staging)"));
        assert!(java.contains("manifestVersion != 1 && manifestVersion != 2"));
        assert!(java.contains("Asset verification failed before runtime startup"));
        assert!(java.contains("setOnApplyWindowInsetsListener"));
        let jni =
            fs::read_to_string(android.join("android/app/src/main/cpp/stasis_android_assets.c"))
                .expect("read Android asset bridge");
        assert!(jni.contains("Java_com_example_mobile_MainActivity_nativeSetAssetRoot"));
        assert!(jni.contains("Java_com_example_mobile_MainActivity_nativeReadPerformanceMetrics"));
        assert!(
            jni.contains("Java_com_example_mobile_MainActivity_nativeSetPerformanceMetricsEnabled")
        );
        assert!(jni.contains("Java_com_example_mobile_MainActivity_nativeReadRuntimeError"));
        assert!(jni.contains("stasis_host_get_latest_performance_metrics_v1"));
        assert!(!jni.contains("@STASIS_"));
        let runtime_source = fs::read_to_string(android.join("runtime/stasis_mobile_runtime.c"))
            .expect("read shared mobile runtime source");
        assert!(runtime_source.contains("stasis_host_set_performance_metrics"));

        let ios = root.join("ios-package");
        fs::create_dir_all(&ios).expect("create iOS staging");
        assemble_mobile_shell(
            &workspace,
            PackageTarget::IosArm64,
            &aot,
            &ios,
            &provenance,
            None,
        )
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
        assert!(ios.join("runtime/stasis_performance_metrics.h").is_file());
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
        let ios_info = fs::read_to_string(ios.join("ios/StasisMobile/Info.plist"))
            .expect("read non-network iOS Info.plist");
        assert!(!ios_info.contains("NSLocalNetworkUsageDescription"));
        assert!(!config.contains("STASIS_NETWORK_ENABLED"));
        assert!(!config.contains("network/libstasis_network.a"));
        assert!(!project.contains("@STASIS_"));

        let mut network_workspace = workspace.clone();
        network_workspace.manifest.capabilities = Some(ProjectCapabilities { network: true });
        network_workspace.manifest.web = Some(WebProjectManifest {
            entry: "src/main.stasis".to_string(),
            loading_font: None,
        });
        let ios_network = root.join("ios-network-package");
        fs::create_dir_all(ios_network.join("ios/network/include"))
            .expect("create iOS network staging fixture");
        fs::write(
            ios_network.join("ios/network/libstasis_network.a"),
            b"fixture static library",
        )
        .expect("write iOS network library fixture");
        fs::write(
            ios_network.join("ios/network/include/stasis_network.h"),
            b"/* fixture network header */\n",
        )
        .expect("write iOS network header fixture");
        let guest_bundle = root.join("network_guest.bundle");
        fs::write(&guest_bundle, b"fixture guest bundle")
            .expect("write network guest bundle fixture");
        fs::write(
            guest_bundle.with_extension("bundle.json"),
            b"{\"schema\":\"stasis.network_guest_bundle.v1\"}\n",
        )
        .expect("write network guest metadata fixture");
        assemble_mobile_shell(
            &network_workspace,
            PackageTarget::IosArm64,
            &aot,
            &ios_network,
            &provenance,
            Some(&guest_bundle),
        )
        .expect("assemble network-enabled iOS shell");
        let network_config = fs::read_to_string(ios_network.join("ios/StasisMobile.xcconfig"))
            .expect("read network iOS config");
        assert!(network_config.contains("STASIS_NETWORK_ENABLED=1"));
        assert!(network_config.contains("$(PROJECT_DIR)/network/include"));
        assert!(network_config.contains("$(PROJECT_DIR)/network/libstasis_network.a"));
        let network_info = fs::read_to_string(ios_network.join("ios/StasisMobile/Info.plist"))
            .expect("read network iOS Info.plist");
        assert!(network_info.contains("NSLocalNetworkUsageDescription"));
        assert!(network_info.contains("mobile_smoke uses your local network"));
        let network_receipt: Value = serde_json::from_str(
            &fs::read_to_string(ios_network.join("stasis_mobile_package.json"))
                .expect("read network iOS package receipt"),
        )
        .expect("parse network iOS package receipt");
        assert_eq!(network_receipt["network"], true);
        assert_eq!(
            network_receipt["network_library"],
            "ios/network/libstasis_network.a"
        );
        assert_eq!(
            network_receipt["network_header"],
            "ios/network/include/stasis_network.h"
        );
        assert_eq!(
            network_receipt["network_guest_bundle"],
            "ios/StasisMobile/stasis_game/network_guest.bundle"
        );
        let network_asset_root = ios_network.join("ios/StasisMobile/stasis_game");
        assert!(network_asset_root.join("network_guest.bundle").is_file());
        assert!(network_asset_root
            .join("network_guest.bundle.json")
            .is_file());
        assert!(network_asset_root.join("assets/manifest.json").is_file());
        assert!(ios_network
            .join("ios/network/libstasis_network.a")
            .is_file());
        assert!(ios_network
            .join("ios/network/include/stasis_network.h")
            .is_file());
        let network_main = fs::read_to_string(ios_network.join("ios/StasisMobile/main.m"))
            .expect("read network iOS SDL3 main wrapper");
        assert_eq!(network_main.trim(), "#include <SDL3/SDL_main.h>");
        let network_presenter =
            fs::read_to_string(ios_network.join("ios/StasisMobile/stasis_ios_network.m"))
                .expect("read network iOS native presenter");
        assert!(network_presenter.contains("stasis_mobile_network_present_join_url"));
        assert!(network_presenter.contains("stasis_mobile_network_copy_join_url"));
        assert!(network_presenter.contains("alertControllerWithTitle:@\"mobile_smoke\""));
        assert!(network_presenter.contains("message:joinURL"));
        assert!(!network_presenter.contains("@STASIS_"));
        assert!(!network_presenter.contains("Join Maddox"));
        assert!(!network_presenter.contains("NSLog"));
        assert!(!network_presenter.contains("printf"));
        assert!(
            fs::read_to_string(ios_network.join("ios/StasisMobile.xcodeproj/project.pbxproj"))
                .expect("read network iOS Xcode project")
                .contains("stasis_ios_network.m in Sources")
        );

        let android_network = root.join("android-network-package");
        fs::create_dir_all(android_network.join("android/app/src/main/cpp/network/include"))
            .expect("create Android network staging fixture");
        fs::write(
            android_network.join("android/app/src/main/cpp/network/libstasis_network.a"),
            b"fixture Android static library",
        )
        .expect("write Android network library fixture");
        fs::write(
            android_network.join("android/app/src/main/cpp/network/include/stasis_network.h"),
            b"/* fixture Android network header */\n",
        )
        .expect("write Android network header fixture");
        assemble_mobile_shell(
            &network_workspace,
            PackageTarget::AndroidArm64,
            &aot,
            &android_network,
            &provenance,
            Some(&guest_bundle),
        )
        .expect("assemble network-enabled Android shell");
        let android_network_cmake =
            fs::read_to_string(android_network.join("android/app/src/main/cpp/CMakeLists.txt"))
                .expect("read network Android CMake");
        assert!(android_network_cmake.contains("STASIS_NETWORK_ENABLED 1"));
        let android_network_manifest =
            fs::read_to_string(android_network.join("android/app/src/main/AndroidManifest.xml"))
                .expect("read network Android manifest");
        assert!(android_network_manifest.contains("android.permission.INTERNET"));
        let android_network_receipt: Value = serde_json::from_str(
            &fs::read_to_string(android_network.join("stasis_mobile_package.json"))
                .expect("read network Android package receipt"),
        )
        .expect("parse network Android package receipt");
        assert_eq!(
            android_network_receipt["network_library"],
            "android/app/src/main/cpp/network/libstasis_network.a"
        );
        assert_eq!(
            android_network_receipt["network_header"],
            "android/app/src/main/cpp/network/include/stasis_network.h"
        );
        assert_eq!(
            android_network_receipt["network_guest_bundle"],
            "android/app/src/main/assets/stasis_game/network_guest.bundle"
        );
        assert!(android_network
            .join("android/app/src/main/assets/stasis_game/network_guest.bundle")
            .is_file());

        remove_temp(&root);
    }

    #[test]
    fn bundled_network_artifacts_resolve_from_a_relocated_toolchain_root() {
        let root = temp_dir("relocated_network_support");
        let executable = root.join("bin/stasis");
        let support = root.join("mobile/network");
        fs::create_dir_all(support.join("ios-arm64"))
            .expect("create relocated iOS support directory");
        fs::create_dir_all(support.join("include"))
            .expect("create relocated network include directory");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create relocated executable directory");
        fs::write(&executable, b"relocated stasis executable").expect("write executable fixture");
        fs::write(
            support.join("ios-arm64/libstasis_network.a"),
            b"relocated iOS network library",
        )
        .expect("write relocated network library");
        fs::write(
            support.join("include/stasis_network.h"),
            b"/* relocated network header */\n",
        )
        .expect("write relocated network header");
        let (library, header) =
            bundled_network_artifacts_for_executable(&executable, PackageTarget::IosArm64)
                .expect("resolve relocated network support")
                .expect("relocated support artifacts");
        assert_eq!(
            fs::canonicalize(library).expect("canonicalize relocated library"),
            fs::canonicalize(support.join("ios-arm64/libstasis_network.a"))
                .expect("canonicalize expected library")
        );
        assert_eq!(
            fs::canonicalize(header).expect("canonicalize relocated header"),
            fs::canonicalize(support.join("include/stasis_network.h"))
                .expect("canonicalize expected header")
        );
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
    fn manifest_accepts_only_the_active_toolchain_stdlib() {
        let mut manifest = ProjectManifest::new("demo".to_string());
        manifest.stdlib = Some("toolchain".to_string());
        assert!(manifest.validate().is_ok());
        manifest.stdlib = Some("nightly-20260730-162".to_string());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validates_vendor_identity_and_exclusive_source_mode() {
        let mut manifest = ProjectManifest::new("demo".to_string());
        manifest.vendor = Some(VendorManifest {
            stasis: StasisVendorManifest {
                release_id: "development".to_string(),
                sha256: "a".repeat(64),
            },
        });
        assert!(manifest.validate().is_ok());

        manifest.vendor.as_mut().unwrap().stasis.sha256 = "not-a-hash".to_string();
        assert!(manifest.validate().is_err());
        manifest.vendor.as_mut().unwrap().stasis.sha256 = "a".repeat(64);
        manifest.stdlib = Some("toolchain".to_string());
        assert!(manifest.validate().is_err());
    }

    #[test]
    fn loading_opted_in_workspace_materializes_the_active_toolchain_stdlib() {
        let root = temp_dir("toolchain_stdlib");
        create_project(root.clone(), "demo".to_string()).expect("create project");
        let mut manifest = ProjectManifest::new("demo".to_string());
        manifest.stdlib = Some("toolchain".to_string());
        write_manifest(&root.join(MANIFEST_NAME), &manifest).expect("enable toolchain stdlib");

        load_workspace(Some(&root)).expect("load workspace");
        let cached = root.join(".stasis_cache/toolchain/src/stdlib/storage.stasis");
        assert_eq!(
            fs::read(&cached).expect("read cached stdlib"),
            fs::read(
                bundled_stdlib_dir()
                    .expect("bundled stdlib")
                    .join("storage.stasis")
            )
            .expect("read bundled stdlib")
        );
        assert!(root
            .join(".stasis_cache/toolchain/src/.toolchain-sha256")
            .is_file());
        assert!(root
            .join(".stasis_cache/toolchain/src/stdlib/internal/gfx_cmd.stasis")
            .is_file());

        remove_temp(&root);
    }

    #[test]
    fn manifest_validates_android_release_identity() {
        let valid = ProjectManifest {
            android: Some(AndroidProjectManifest {
                application_id: "com.example.game".to_string(),
                label: "Example Game".to_string(),
                orientation: "unspecified".to_string(),
                version_code: 1,
                version_name: "1.0.0".to_string(),
            }),
            ..ProjectManifest::new("example_game".to_string())
        };
        assert!(valid.validate().is_ok());

        for orientation in [
            "unspecified",
            "sensorLandscape",
            "sensorPortrait",
            "fullSensor",
        ] {
            let mut oriented = valid.clone();
            oriented.android.as_mut().unwrap().orientation = orientation.to_string();
            assert!(oriented.validate().is_ok(), "rejected {orientation}");
        }

        let mut invalid_orientation = valid.clone();
        invalid_orientation.android.as_mut().unwrap().orientation = "sensor".to_string();
        assert!(invalid_orientation.validate().is_err());

        for application_id in ["game", "com.example.bad-name", "com.1game"] {
            let mut invalid = valid.clone();
            invalid.android.as_mut().unwrap().application_id = application_id.to_string();
            assert!(invalid.validate().is_err(), "accepted {application_id}");
        }

        let mut invalid_label = valid.clone();
        invalid_label.android.as_mut().unwrap().label = "Game & More".to_string();
        assert!(invalid_label.validate().is_err());

        let mut invalid_version = valid;
        invalid_version.android.as_mut().unwrap().version_name = "1.0'debug".to_string();
        assert!(invalid_version.validate().is_err());
    }

    #[test]
    fn manifest_validates_network_guest_entry_contract() {
        let mut manifest = ProjectManifest::new("network_game".to_string());
        manifest.capabilities = Some(ProjectCapabilities { network: true });
        assert!(manifest.validate().is_ok());
        assert!(
            validate_mobile_network_guest_contract(&manifest, PackageTarget::AndroidArm64).is_err()
        );
        assert!(
            validate_mobile_network_guest_contract(&manifest, PackageTarget::IosArm64).is_err()
        );
        assert!(validate_mobile_network_guest_contract(&manifest, PackageTarget::Web).is_ok());
        manifest.web = Some(WebProjectManifest {
            entry: "src/guest_main.stasis".to_string(),
            loading_font: None,
        });
        assert!(manifest.validate().is_ok());
        assert!(
            validate_mobile_network_guest_contract(&manifest, PackageTarget::AndroidArm64).is_ok()
        );
        manifest.web.as_mut().unwrap().entry = "../guest.stasis".to_string();
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
    fn init_preserves_other_vendors_and_preflights_the_stasis_package() {
        let root = temp_dir("vendor_preflight");
        fs::create_dir_all(root.join("vendor/example")).expect("create existing vendor");
        fs::write(root.join("vendor/example/keep.txt"), "keep\n").expect("write existing vendor");

        create_project(root.clone(), "demo".to_string()).expect("create alongside other vendor");
        assert_eq!(
            fs::read_to_string(root.join("vendor/example/keep.txt")).expect("read existing vendor"),
            "keep\n"
        );
        assert!(root.join("vendor/stasis/stdlib/stdlib.stasis").is_file());
        remove_temp(&root);

        let conflict = temp_dir("vendor_conflict");
        fs::create_dir_all(conflict.join("vendor/stasis")).expect("create package conflict");
        fs::write(conflict.join("vendor/stasis/keep.txt"), "keep\n")
            .expect("write package conflict");
        let error = create_project(conflict.clone(), "demo".to_string())
            .expect_err("reject existing stasis package");
        assert!(error.contains("vendor\\stasis") || error.contains("vendor/stasis"));
        assert!(!conflict.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(conflict.join("vendor/stasis/keep.txt"))
                .expect("read preserved package conflict"),
            "keep\n"
        );
        remove_temp(&conflict);
    }

    #[test]
    fn init_preserves_existing_vscode_settings_without_partial_writes() {
        let root = temp_dir("vscode_preflight");
        fs::create_dir_all(root.join(".vscode")).expect("create VS Code directory");
        fs::write(
            root.join(".vscode/settings.json"),
            "{\"editor.tabSize\": 2}\n",
        )
        .expect("write user editor settings");

        let error = create_project(root.clone(), "demo".to_string()).expect_err("reject conflict");
        assert!(error.contains(".vscode"));
        assert!(!root.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(root.join(".vscode/settings.json"))
                .expect("read user editor settings"),
            "{\"editor.tabSize\": 2}\n"
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
    fn init_preserves_existing_project_architecture_without_partial_writes() {
        let root = temp_dir("architecture_guide_preflight");
        fs::create_dir_all(&root).expect("create project directory");
        fs::write(root.join(PROJECT_ARCHITECTURE_NAME), "user guidance\n")
            .expect("write project architecture guide");

        let error = create_project(root.clone(), "demo".to_string()).expect_err("reject conflict");
        assert!(error.contains(PROJECT_ARCHITECTURE_NAME));
        assert!(!root.join(MANIFEST_NAME).exists());
        assert_eq!(
            fs::read_to_string(root.join(PROJECT_ARCHITECTURE_NAME))
                .expect("read project architecture guide"),
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
    fn windows_desktop_package_keeps_only_the_launcher_at_root() {
        let root = temp_dir("windows_desktop_payload");
        fs::create_dir_all(root.join("assets")).expect("create staged assets");
        let executable = root.join("demo.exe");
        fs::write(&executable, "launcher").expect("write launcher");
        fs::write(root.join("demo.exe.launch"), "dll=demo.dll").expect("write launch config");
        fs::write(root.join("demo.dll"), "game").expect("write game library");
        fs::write(root.join("stasis_graphics.dll"), "graphics").expect("write graphics runtime");

        nest_windows_desktop_payload(&root, &executable).expect("nest package payload");

        assert!(executable.is_file());
        assert_eq!(
            fs::read_dir(&root)
                .expect("read package root")
                .map(|entry| entry.expect("root entry").file_name())
                .collect::<BTreeSet<_>>(),
            [OsString::from("app"), OsString::from("demo.exe")]
                .into_iter()
                .collect()
        );
        for relative in [
            "assets",
            "demo.dll",
            "demo.exe.launch",
            "stasis_graphics.dll",
        ] {
            assert!(root
                .join(WINDOWS_DESKTOP_PAYLOAD_DIR)
                .join(relative)
                .exists());
        }
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
