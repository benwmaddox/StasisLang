//! Opt-in live OpenRouter acceptance; skipped unless an evidence directory is configured.

#[cfg(target_os = "windows")]
use super::*;
#[cfg(target_os = "windows")]
use serde_json::json;
#[cfg(target_os = "windows")]
use std::collections::{BTreeMap, BTreeSet};
#[cfg(target_os = "windows")]
use std::path::{Path, PathBuf};
#[cfg(target_os = "windows")]
use std::sync::mpsc;
#[cfg(target_os = "windows")]
use std::time::{Duration, Instant};

#[cfg(target_os = "windows")]
#[test]
fn two_openrouter_tasks_share_one_live_game() {
    let Ok(_) = std::env::var("STASIS_EDITOR_LIVE_ACCEPTANCE_DIR") else {
        return;
    };
    run_live_acceptance().expect("live editor acceptance");
}

#[cfg(target_os = "windows")]
fn run_live_acceptance() -> Result<(), String> {
    use stasis::LiveRunConfig;
    use stasis_runner::live::{live_session, LiveCommand, LiveRequest};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    let repository = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .map_err(|error| format!("repository root: {error}"))?;
    let output = evidence_dir(&repository)?;
    clear_previous_evidence(&output)?;
    let source = PathBuf::from(
        std::env::var("STASIS_EDITOR_LIVE_SOURCE")
            .map_err(|_| "STASIS_EDITOR_LIVE_SOURCE must name the source game".to_string())?,
    );
    let workspace = repository.join("target/task-524-live-workspace");
    prepare_disposable_workspace(&repository, &source, &workspace)?;
    load_openrouter_key()?;
    std::env::set_var("STASIS_AI_PROVIDER", "openrouter");
    std::env::set_var(
        "STASIS_AI_MODEL",
        std::env::var("STASIS_EDITOR_OPENROUTER_MODEL")
            .unwrap_or_else(|_| "openai/gpt-5.6-luna".to_string()),
    );

    let manifest: Value = serde_json::from_slice(
        &std::fs::read(workspace.join("stasis.json"))
            .map_err(|error| format!("read disposable manifest: {error}"))?,
    )
    .map_err(|error| format!("parse disposable manifest: {error}"))?;
    let entry_relative = PathBuf::from(
        manifest["entry"]
            .as_str()
            .ok_or_else(|| "disposable manifest has no entry".to_string())?,
    );
    let entry = workspace.join(&entry_relative);
    let build = PathBuf::from(manifest["output"].as_str().unwrap_or("build"));
    let (client, server) = live_session(stasis_runner::live::DEFAULT_LIVE_QUEUE_CAPACITY);
    let shutdown = Arc::new(AtomicBool::new(false));
    let runtime_shutdown = Arc::clone(&shutdown);
    let runtime_workspace = workspace.clone();
    let runtime_entry = entry.clone();
    let runtime = std::thread::spawn(move || {
        let config = LiveRunConfig::new(runtime_workspace, entry_relative, build)
            .with_window_title("Task 524 live acceptance game");
        let result = stasis::run_live_in_process_with_data(
            &runtime_entry,
            None,
            None,
            None,
            16_000,
            None,
            server,
            config,
        );
        runtime_shutdown.store(true, Ordering::Release);
        result
    });
    let warmup_started = Instant::now();
    if let Err(error) = wait_for_playable_runtime(&client) {
        shutdown.store(true, Ordering::Release);
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        let _ = runtime.join();
        return Err(error);
    }
    let warmup_ms = warmup_started.elapsed().as_millis();
    let initial_probe = match probe_runtime(&client, 60_000, None) {
        Ok(probe) => probe,
        Err(error) => {
            shutdown.store(true, Ordering::Release);
            let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
            let _ = runtime.join();
            return Err(error);
        }
    };
    if std::env::var("STASIS_EDITOR_LIVE_LOCAL_APPLY_ONLY").as_deref() == Ok("1") {
        let preflight = run_local_apply_preflight(
            client.clone(),
            &workspace,
            Arc::clone(&shutdown),
            &initial_probe,
        );
        shutdown.store(true, Ordering::Release);
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        let runtime_result = runtime
            .join()
            .map_err(|_| "live runtime thread panicked".to_string())?;
        preflight?;
        return runtime_result;
    }
    if std::env::var("STASIS_EDITOR_LIVE_WARMUP_ONLY").as_deref() == Ok("1") {
        shutdown.store(true, Ordering::Release);
        let _ = client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
        return runtime
            .join()
            .map_err(|_| "live runtime thread panicked".to_string())?;
    }

    let (result_tx, result_rx) = mpsc::channel();
    let model = std::env::var("STASIS_AI_MODEL").unwrap_or_default();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stasis Editor - Task 524 live acceptance")
            .with_inner_size([1440.0, 900.0]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    let quit_client = client.clone();
    let app_output = output.clone();
    let app_workspace = workspace.clone();
    let app_shutdown = Arc::clone(&shutdown);
    let editor_result = eframe::run_native(
        "Stasis Editor live acceptance",
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_pixels_per_point(1.0);
            let windows = window_layout::WindowLayout::new(client.clone(), &app_workspace);
            let mut editor = DesktopEditor::new(client, app_workspace.clone(), app_shutdown);
            editor.windows = Some(windows);
            let (provider_error_tx, provider_errors) = mpsc::channel();
            let (provider_usage_tx, provider_usage) = mpsc::channel();
            let provider_root = app_workspace.clone();
            editor.controller = TaskController::new(move |request, canceled| {
                let task_id = request.task_id.clone();
                let result = run_reply_provider_observed(
                    request,
                    canceled,
                    provider_root.clone(),
                    |usage| {
                        let _ = provider_usage_tx.send(json!({
                            "task_id": task_id.clone(),
                            "usage": usage,
                        }));
                    },
                );
                if let Err(error) = &result {
                    let _ = provider_error_tx.send(error.clone());
                }
                result
            });
            let hold_seconds = std::env::var("STASIS_EDITOR_LIVE_HOLD_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            let final_hold_seconds = std::env::var("STASIS_EDITOR_LIVE_FINAL_HOLD_SECONDS")
                .ok()
                .and_then(|value| value.parse::<u64>().ok())
                .unwrap_or(0);
            Box::new(LiveAcceptanceApp {
                editor,
                output: app_output,
                workspace: app_workspace,
                model,
                state: AcceptanceState::HoldBeforeStart,
                state_started: Instant::now(),
                hold_until: Instant::now() + Duration::from_secs(hold_seconds),
                final_hold: Duration::from_secs(final_hold_seconds),
                failure: None,
                pending_editor_capture: None,
                task_records: Vec::new(),
                runtime_probes: vec![initial_probe],
                warmup_ms,
                provider_errors,
                provider_usage,
                usage_records: Vec::new(),
                acceptance_started: Instant::now(),
                apply_started: BTreeMap::new(),
                apply_wall_ms: BTreeMap::new(),
                motion_positions: None,
                repair_attempted: BTreeSet::new(),
                next_flow_capture: Instant::now(),
                flow_frames: 0,
                result: Some(result_tx),
            })
        }),
    )
    .map_err(|error| format!("live editor window: {error}"));
    shutdown.store(true, Ordering::Release);
    let _ = quit_client.submit(LiveRequest::new(u64::MAX, LiveCommand::Quit));
    let runtime_result = runtime
        .join()
        .map_err(|_| "live runtime thread panicked".to_string())?;
    let acceptance = result_rx
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| "live editor closed without an acceptance result".to_string())?;
    editor_result?;
    runtime_result?;
    acceptance?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn run_local_apply_preflight(
    client: stasis_runner::live::LiveSessionClient,
    workspace: &Path,
    shutdown: Arc<AtomicBool>,
    initial: &RuntimeProbe,
) -> Result<(), String> {
    let source = std::fs::read_to_string(workspace.join("src/main.stasis"))
        .map_err(|error| format!("read local apply source: {error}"))?;
    let start = source
        .find("function render(): i32")
        .ok_or_else(|| "local apply preflight could not find render".to_string())?;
    let end = source[start..]
        .find("// Compatible swaps")
        .map(|offset| start + offset)
        .ok_or_else(|| "local apply preflight could not bound render".to_string())?;
    let new_source = source[start..end]
        .trim_end()
        .replace("1.0, 0.30, 0.78", "0.35, 0.85, 1.0");
    let payload = json!({"schema_version": 1, "edits": [{
        "operation": "update",
        "target": {"name": "render", "file": "src/main.stasis"},
        "new_source": new_source,
    }]});
    let mut editor = DesktopEditor::new(client, workspace.to_path_buf(), shutdown);
    editor.state.objective = "Local state preservation preflight".to_string();
    editor
        .state
        .create_task()
        .map_err(|error| error.to_string())?;
    editor
        .state
        .session
        .active_task_mut()
        .map_err(|error| error.to_string())?
        .propose_action_with_payload(
            "local-render-accent",
            stasis_ai::ActionKind::Edit,
            "Update render accent",
            payload,
        )
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(30);
    while editor
        .state
        .reviewed_preview("task-1", "local-render-accent")
        .is_err()
    {
        editor.poll_semantic_previews();
        if Instant::now() >= deadline {
            return Err("local apply semantic preview timed out".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    editor
        .state
        .handle(TaskSessionCommand::AcceptAction)
        .map_err(|error| error.to_string())?;
    editor
        .state
        .handle(TaskSessionCommand::ApplyAction)
        .map_err(|error| error.to_string())?;
    editor.flush_intents();
    let deadline = Instant::now() + Duration::from_secs(120);
    while editor.busy_tasks.contains("task-1") {
        editor.poll_host();
        if Instant::now() >= deadline {
            return Err("local apply execution timed out".to_string());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let task = editor
        .state
        .session
        .task("task-1")
        .map_err(|error| error.to_string())?;
    if !matches!(task.validation, ValidationStatus::Passed { .. }) {
        return Err(format!(
            "local apply validation did not pass: {:?}",
            task.validation
        ));
    }
    let after = probe_runtime(&editor.client, 59_000, Some(initial.generation))?;
    if after.session_id != initial.session_id
        || after.generation <= initial.generation
        || after.tick != initial.tick
        || !same_scalar(&after.paddle_x, &initial.paddle_x)
        || !same_scalar(&after.bricks_left, &initial.bricks_left)
        || after.assets_ready != Value::Bool(true)
        || after.loader_failed != Value::Bool(false)
    {
        return Err(format!(
            "local apply did not preserve live state: before={} after={}",
            runtime_probe_json(initial),
            runtime_probe_json(&after)
        ));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
#[derive(Clone)]
struct RuntimeProbe {
    session_id: String,
    generation: u64,
    tick: u64,
    paddle_x: Value,
    bricks_left: Value,
    assets_ready: Value,
    loader_failed: Value,
}

#[cfg(target_os = "windows")]
fn request_live(
    client: &stasis_runner::live::LiveSessionClient,
    request_id: u64,
    command: stasis_runner::live::LiveCommand,
) -> Result<stasis_runner::live::LiveResponse, String> {
    client.submit(stasis_runner::live::LiveRequest::new(request_id, command))?;
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err("live runtime probe timed out".to_string());
        }
        match client.receive_timeout(remaining.min(Duration::from_millis(250))) {
            Ok(response) if response.request_id == request_id => {
                if response.ok {
                    return Ok(response);
                }
                return Err(response
                    .error
                    .unwrap_or_else(|| "live runtime probe failed".to_string()));
            }
            Ok(_) => {}
            Err(error) if error.contains("timed out") => {}
            Err(error) => return Err(error),
        }
    }
}

#[cfg(target_os = "windows")]
fn launch_ball_for_motion(client: &stasis_runner::live::LiveSessionClient) -> Result<(), String> {
    request_live(
        client,
        63_000,
        stasis_runner::live::LiveCommand::SetInputState {
            pointers: vec![stasis_runner::live::LivePointerInput {
                id: 0,
                x: 320,
                y: 320,
                is_down: true,
                went_down: true,
                went_up: false,
            }],
        },
    )?;
    request_live(
        client,
        63_001,
        stasis_runner::live::LiveCommand::Step { ticks: 1 },
    )?;
    request_live(
        client,
        63_002,
        stasis_runner::live::LiveCommand::SetInputState {
            pointers: Vec::new(),
        },
    )?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn probe_runtime(
    client: &stasis_runner::live::LiveSessionClient,
    request_id: u64,
    after_generation: Option<u64>,
) -> Result<RuntimeProbe, String> {
    if after_generation.is_none() {
        request_live(client, request_id, stasis_runner::live::LiveCommand::Pause)?;
    }
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut sequence = request_id.saturating_add(1);
    loop {
        let paddle = request_live(
            client,
            sequence,
            stasis_runner::live::LiveCommand::Inspect {
                path: "paddle_x".to_string(),
            },
        )?;
        sequence = sequence.saturating_add(1);
        let identity = paddle
            .runtime_identity
            .clone()
            .ok_or_else(|| "runtime probe omitted identity".to_string())?;
        if after_generation.is_some_and(|generation| identity.generation <= generation) {
            if Instant::now() >= deadline {
                return Err(format!(
                    "runtime generation did not advance beyond {}",
                    after_generation.unwrap_or_default()
                ));
            }
            std::thread::sleep(Duration::from_millis(100));
            continue;
        }
        let bricks = request_live(
            client,
            sequence,
            stasis_runner::live::LiveCommand::Inspect {
                path: "bricks_left".to_string(),
            },
        )?;
        sequence = sequence.saturating_add(1);
        let assets_ready = request_live(
            client,
            sequence,
            stasis_runner::live::LiveCommand::Inspect {
                path: "loader.assets_ready".to_string(),
            },
        )?;
        sequence = sequence.saturating_add(1);
        let loader_failed = request_live(
            client,
            sequence,
            stasis_runner::live::LiveCommand::Inspect {
                path: "loader.failed".to_string(),
            },
        )?;
        return Ok(RuntimeProbe {
            session_id: identity.session_id,
            generation: identity.generation,
            tick: paddle.tick,
            paddle_x: inspect_scalar(paddle.data),
            bricks_left: inspect_scalar(bricks.data),
            assets_ready: inspect_scalar(assets_ready.data),
            loader_failed: inspect_scalar(loader_failed.data),
        });
    }
}

#[cfg(target_os = "windows")]
fn inspect_scalar(data: Option<Value>) -> Value {
    data.as_ref()
        .and_then(|value| value.pointer("/value/value"))
        .cloned()
        .or_else(|| data.as_ref().and_then(|value| value.get("value")).cloned())
        .or(data)
        .unwrap_or(Value::Null)
}

#[cfg(target_os = "windows")]
fn wait_for_playable_runtime(
    client: &stasis_runner::live::LiveSessionClient,
) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut request_id = 58_000;
    loop {
        let ready = request_live(
            client,
            request_id,
            stasis_runner::live::LiveCommand::Inspect {
                path: "loader.assets_ready".to_string(),
            },
        )?;
        request_id += 1;
        let failed = request_live(
            client,
            request_id,
            stasis_runner::live::LiveCommand::Inspect {
                path: "loader.failed".to_string(),
            },
        )?;
        request_id += 1;
        if inspect_scalar(failed.data).as_bool() == Some(true) {
            return Err("game asset loader failed before live acceptance".to_string());
        }
        if inspect_scalar(ready.data).as_bool() == Some(true) {
            let bricks = request_live(
                client,
                request_id,
                stasis_runner::live::LiveCommand::Inspect {
                    path: "bricks_left".to_string(),
                },
            )?;
            let bricks = inspect_scalar(bricks.data);
            if bricks.as_i64() != Some(35) {
                return Err(format!(
                    "playable game initialized with unexpected bricks_left: {:?}",
                    bricks
                ));
            }
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err("game did not finish loading within 30 seconds".to_string());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(target_os = "windows")]
fn evidence_dir(repository: &Path) -> Result<PathBuf, String> {
    let configured = PathBuf::from(
        std::env::var("STASIS_EDITOR_LIVE_ACCEPTANCE_DIR")
            .map_err(|_| "live acceptance evidence directory is missing".to_string())?,
    );
    let output = if configured.is_absolute() {
        configured
    } else {
        repository.join(configured)
    };
    if output
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
        || !output.starts_with(repository)
    {
        return Err("evidence directory must be inside this worktree".to_string());
    }
    std::fs::create_dir_all(&output)
        .map_err(|error| format!("create evidence directory: {error}"))?;
    let output = output
        .canonicalize()
        .map_err(|error| format!("resolve evidence directory: {error}"))?;
    if !output.starts_with(repository) {
        return Err("evidence directory must be inside this worktree".to_string());
    }
    Ok(output)
}

#[cfg(target_os = "windows")]
fn clear_previous_evidence(output: &Path) -> Result<(), String> {
    for entry in
        std::fs::read_dir(output).map_err(|error| format!("read evidence directory: {error}"))?
    {
        let entry = entry.map_err(|error| format!("read evidence entry: {error}"))?;
        let metadata = entry
            .metadata()
            .map_err(|error| format!("inspect evidence entry: {error}"))?;
        if !metadata.is_file() {
            continue;
        }
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let generated = name.starts_with("flow-")
            || name.starts_with("game-motion-")
            || matches!(
                name.as_ref(),
                "failure-report.json"
                    | "report.json"
                    | "report.md"
                    | "first-proposal.png"
                    | "second-proposal.png"
                    | "final-editor.png"
                    | "game-frame.png"
                    | "editor-flow.mp4"
                    | "game-motion.mp4"
                    | "native-window-flow.png"
                    | "native-window-flow.mp4"
                    | "native-window-flow.json"
            );
        if generated {
            std::fs::remove_file(entry.path())
                .map_err(|error| format!("clear previous evidence {name}: {error}"))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn prepare_disposable_workspace(
    repository: &Path,
    source: &Path,
    target: &Path,
) -> Result<(), String> {
    if !target.starts_with(repository) || target == repository {
        return Err("unsafe disposable workspace path".to_string());
    }
    if target.exists() {
        let metadata = std::fs::symlink_metadata(target)
            .map_err(|error| format!("inspect disposable workspace: {error}"))?;
        if metadata.file_type().is_symlink()
            || !target
                .canonicalize()
                .map_err(|error| format!("resolve disposable workspace: {error}"))?
                .starts_with(repository)
        {
            return Err("unsafe existing disposable workspace path".to_string());
        }
        std::fs::remove_dir_all(target)
            .map_err(|error| format!("clear disposable workspace: {error}"))?;
    }
    copy_tree(source, target)?;
    let copied_stdlib = target.join("vendor/stasis/stdlib");
    if copied_stdlib.exists() {
        std::fs::remove_dir_all(&copied_stdlib)
            .map_err(|error| format!("replace disposable stdlib: {error}"))?;
    }
    copy_tree(&repository.join("src/stdlib"), &copied_stdlib)
}

#[cfg(target_os = "windows")]
fn copy_tree(source: &Path, target: &Path) -> Result<(), String> {
    std::fs::create_dir_all(target)
        .map_err(|error| format!("create disposable directory: {error}"))?;
    for entry in
        std::fs::read_dir(source).map_err(|error| format!("read source workspace: {error}"))?
    {
        let entry = entry.map_err(|error| format!("read source entry: {error}"))?;
        let name = entry.file_name();
        if matches!(
            name.to_str(),
            Some(".git" | ".stasis_cache" | "build" | "dist")
        ) {
            continue;
        }
        let destination = target.join(&name);
        let kind = entry
            .file_type()
            .map_err(|error| format!("inspect source entry: {error}"))?;
        if kind.is_symlink() {
            return Err(format!(
                "source workspace contains a symlink: {}",
                entry.path().display()
            ));
        }
        if kind.is_dir() {
            copy_tree(&entry.path(), &destination)?;
        } else if !name.to_string_lossy().starts_with(".env") {
            std::fs::copy(entry.path(), destination)
                .map_err(|error| format!("copy disposable file: {error}"))?;
        }
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn load_openrouter_key() -> Result<(), String> {
    if std::env::var_os("OPENROUTER_API_KEY").is_some() {
        return Ok(());
    }
    let path = PathBuf::from(std::env::var("STASIS_EDITOR_LIVE_ENV").map_err(|_| {
        "STASIS_EDITOR_LIVE_ENV must name the credential file when OPENROUTER_API_KEY is unset"
            .to_string()
    })?);
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("read configured credential file: {error}"))?;
    let value = contents
        .lines()
        .filter_map(|line| line.split_once('='))
        .find_map(|(name, value)| (name.trim() == "OPENROUTER_API_KEY").then(|| value.trim()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "configured credential file has no non-empty OpenRouter key".to_string())?;
    std::env::set_var("OPENROUTER_API_KEY", value);
    Ok(())
}

#[cfg(target_os = "windows")]
fn start_task(editor: &mut DesktopEditor, objective: &str, prompt: &str) -> Result<(), String> {
    create_openrouter_task(editor, objective)?;
    send_reply(editor, prompt)
}

#[cfg(target_os = "windows")]
fn create_openrouter_task(editor: &mut DesktopEditor, objective: &str) -> Result<(), String> {
    editor.state.objective = objective.to_string();
    editor.state.create_task()?;
    let config = selected_provider_config(Some(ProviderSelection::OpenRouter))?;
    let task = editor
        .state
        .session
        .active_task_mut()
        .map_err(|error| error.to_string())?;
    task.select_provider(ProviderSelection::OpenRouter)
        .and_then(|()| task.set_provider_state(configured_provider_state(&config)))
        .map_err(|error| error.to_string())?;
    Ok(())
}

#[cfg(target_os = "windows")]
fn send_reply(editor: &mut DesktopEditor, prompt: &str) -> Result<(), String> {
    editor.state.reply = prompt.to_string();
    editor
        .state
        .handle(TaskSessionCommand::SendReply)
        .map_err(|error| error.to_string())?;
    editor.flush_intents();
    Ok(())
}

#[cfg(target_os = "windows")]
#[derive(Clone, Copy, PartialEq, Eq)]
enum AcceptanceState {
    HoldBeforeStart,
    WaitFirstProposal,
    CaptureFirstProposal,
    SettleFirstProposal,
    WaitFirstApply,
    WaitFrame,
    WaitSecondProposal,
    CaptureSecondProposal,
    SettleSecondProposal,
    WaitSecondApply,
    ResumeMotion,
    CaptureFinal,
    HoldAfterCapture,
    Failed,
}

#[cfg(target_os = "windows")]
struct LiveAcceptanceApp {
    editor: DesktopEditor,
    output: PathBuf,
    workspace: PathBuf,
    model: String,
    state: AcceptanceState,
    state_started: Instant,
    hold_until: Instant,
    final_hold: Duration,
    failure: Option<String>,
    pending_editor_capture: Option<String>,
    task_records: Vec<Value>,
    runtime_probes: Vec<RuntimeProbe>,
    warmup_ms: u128,
    provider_errors: mpsc::Receiver<String>,
    provider_usage: mpsc::Receiver<Value>,
    usage_records: Vec<Value>,
    acceptance_started: Instant,
    apply_started: BTreeMap<String, Instant>,
    apply_wall_ms: BTreeMap<String, u128>,
    motion_positions: Option<(Value, Value)>,
    repair_attempted: BTreeSet<String>,
    next_flow_capture: Instant,
    flow_frames: usize,
    result: Option<mpsc::Sender<Result<(), String>>>,
}

#[cfg(target_os = "windows")]
impl LiveAcceptanceApp {
    fn transition(&mut self, state: AcceptanceState) {
        self.state = state;
        self.state_started = Instant::now();
    }

    fn fail(&mut self, context: &egui::Context, error: impl Into<String>) {
        let error = error.into();
        let tasks = self
            .editor
            .state
            .session
            .tasks()
            .map(task_summary)
            .collect::<Vec<_>>();
        let _ = std::fs::write(
            self.output.join("failure-report.json"),
            serde_json::to_vec_pretty(&json!({
                "result": "failed",
                "error": error,
                "model": self.model,
                "tasks": tasks,
                "provider_turn_usage": self.usage_records,
                "runtime_probes": self.runtime_probes.iter().map(runtime_probe_json).collect::<Vec<_>>(),
                "execution_receipts": self.editor.execution_receipts.iter().map(|((task_id, action_id), receipt)| json!({
                    "task_id": task_id,
                    "action_id": action_id,
                    "receipt": receipt,
                })).collect::<Vec<_>>(),
            }))
            .unwrap_or_default(),
        );
        self.failure = Some(error);
        self.state = AcceptanceState::Failed;
        context.send_viewport_cmd(egui::ViewportCommand::Close);
    }

    fn wait_guard(&mut self, context: &egui::Context) -> bool {
        if self.state_started.elapsed() > Duration::from_secs(180) {
            self.fail(context, "acceptance state exceeded its 180 second bound");
            false
        } else {
            true
        }
    }

    fn active_action_ready(&self) -> Option<String> {
        let task = self.editor.state.session.active_task().ok()?;
        task.actions.values().find_map(|action| {
            (matches!(action.state, ActionState::Proposed)
                && self
                    .editor
                    .state
                    .reviewed_preview(task.id.as_str(), action.id.as_str())
                    .is_ok())
            .then(|| action.id.to_string())
        })
    }

    fn record_active(&mut self, phase: &str) -> Result<(), String> {
        let task = self
            .editor
            .state
            .session
            .active_task()
            .map_err(|error| error.to_string())?;
        let action = task
            .actions
            .values()
            .next()
            .ok_or_else(|| "task has no semantic action".to_string())?;
        let preview = self
            .editor
            .state
            .reviewed_preview(task.id.as_str(), action.id.as_str())?;
        self.task_records.push(json!({
            "phase": phase,
            "reviewed_proposal_wall_ms": self.state_started.elapsed().as_millis(),
            "task_id": task.id,
            "objective": task.objective,
            "provider": task.provider.provider,
            "model": task.provider.model,
            "metrics": task.metrics,
            "action_id": action.id,
            "description": action.description,
            "state": format!("{:?}", action.state),
            "plan": preview.plan,
            "screenshots": task.screenshots.len(),
        }));
        Ok(())
    }

    fn save_editor_capture(&mut self, context: &egui::Context) -> Result<bool, String> {
        let Some(name) = self.pending_editor_capture.clone() else {
            return Ok(false);
        };
        let image = context.input(|input| {
            input.events.iter().find_map(|event| {
                if let egui::Event::Screenshot { image, .. } = event {
                    Some(image.clone())
                } else {
                    None
                }
            })
        });
        let Some(image) = image else {
            return Ok(false);
        };
        let bytes = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.to_array())
            .collect::<Vec<_>>();
        image::save_buffer(
            self.output.join(&name),
            &bytes,
            image.width() as u32,
            image.height() as u32,
            image::ColorType::Rgba8,
        )
        .map_err(|error| format!("save native editor screenshot: {error}"))?;
        if !name.starts_with("flow-") {
            image::save_buffer(
                self.output
                    .join(format!("flow-{:04}.png", self.flow_frames)),
                &bytes,
                image.width() as u32,
                image.height() as u32,
                image::ColorType::Rgba8,
            )
            .map_err(|error| format!("save milestone flow frame: {error}"))?;
            self.flow_frames += 1;
        }
        self.pending_editor_capture = None;
        Ok(true)
    }

    fn first_action_ms(&self, task_id: &str) -> Option<u64> {
        let mut prior_turn_ms = 0_u64;
        for record in &self.usage_records {
            if record.get("task_id").and_then(Value::as_str) != Some(task_id) {
                continue;
            }
            let usage = record.get("usage")?;
            if let Some(first_action) = usage
                .pointer("/timing_ms/first_action")
                .and_then(Value::as_u64)
            {
                return Some(prior_turn_ms.saturating_add(first_action));
            }
            prior_turn_ms = prior_turn_ms.saturating_add(
                usage
                    .pointer("/timing_ms/turn_total")
                    .and_then(Value::as_u64)
                    .unwrap_or(0),
            );
        }
        None
    }

    fn provider_totals(&self, task_id: &str) -> Value {
        let mut turns = 0_u64;
        let mut elapsed_ms = 0_u64;
        let mut prompt = 0_u64;
        let mut completion = 0_u64;
        let mut reasoning = 0_u64;
        let mut cache = 0_u64;
        let mut cost = 0.0_f64;
        for usage in self.usage_records.iter().filter_map(|record| {
            (record.get("task_id").and_then(Value::as_str) == Some(task_id))
                .then(|| record.get("usage"))
                .flatten()
        }) {
            turns += 1;
            elapsed_ms += usage
                .pointer("/timing_ms/turn_total")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            prompt += usage
                .pointer("/tokens/prompt")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            completion += usage
                .pointer("/tokens/completion")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            reasoning += usage
                .pointer("/tokens/reasoning")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            cache += usage
                .pointer("/tokens/cache")
                .and_then(Value::as_u64)
                .unwrap_or(0);
            cost += usage.get("cost").and_then(Value::as_f64).unwrap_or(0.0);
        }
        json!({
            "turns": turns,
            "elapsed_ms": elapsed_ms,
            "first_action_ms": self.first_action_ms(task_id),
            "tokens": {"prompt": prompt, "completion": completion, "reasoning": reasoning, "cache": cache},
            "cost_usd": cost,
        })
    }

    fn finish_report(&self) -> Result<(), String> {
        let first = self
            .editor
            .state
            .session
            .task("task-1")
            .map_err(|error| error.to_string())?;
        let second = self
            .editor
            .state
            .session
            .task("task-2")
            .map_err(|error| error.to_string())?;
        for task in [first, second] {
            if !task
                .actions
                .values()
                .any(|action| matches!(action.state, ActionState::Applied))
                || !matches!(task.validation, ValidationStatus::Passed { .. })
            {
                return Err(format!(
                    "{} did not preserve applied, passing state",
                    task.id
                ));
            }
        }
        let initial = self
            .runtime_probes
            .first()
            .ok_or_else(|| "missing initial runtime probe".to_string())?;
        if self.runtime_probes.len() != 3 {
            return Err("missing runtime probe after a semantic swap".to_string());
        }
        for pair in self.runtime_probes.windows(2) {
            let previous = &pair[0];
            let probe = &pair[1];
            if probe.session_id != initial.session_id
                || probe.generation <= previous.generation
                || probe.tick != initial.tick
                || !same_scalar(&probe.paddle_x, &initial.paddle_x)
                || !same_scalar(&probe.bricks_left, &initial.bricks_left)
                || probe.assets_ready != Value::Bool(true)
                || probe.loader_failed != Value::Bool(false)
            {
                return Err(
                    "paused guest state or runtime session was not preserved across swaps"
                        .to_string(),
                );
            }
        }
        let screenshot = second
            .screenshots
            .values()
            .next()
            .ok_or_else(|| "image task has no captured frame".to_string())?;
        let source = PathBuf::from(&screenshot.source);
        std::fs::copy(&source, self.output.join("game-frame.png"))
            .map_err(|error| format!("copy SDL game frame: {error}"))?;
        let prior_failed_provider_attempts = [
            "provider-failed-attempt.json",
            "provider-failed-attempt-gemini.json",
            "provider-failed-attempt-gemini-partial.json",
            "provider-failed-attempt-gemini-image.json",
        ]
        .into_iter()
        .filter_map(|name| std::fs::read(self.output.join(name)).ok())
        .filter_map(|bytes| serde_json::from_slice::<Value>(&bytes).ok())
        .collect::<Vec<_>>();
        let report = json!({
            "result": "passed",
            "provider": "openrouter",
            "initial_configured_model": self.model,
            "same_disposable_workspace": self.workspace,
            "warmup_ms": self.warmup_ms,
            "tasks": self.task_records,
            "total_wall_ms": self.acceptance_started.elapsed().as_millis(),
            "apply_wall_ms": self.apply_wall_ms,
            "provider_turn_usage": self.usage_records,
            "execution_receipts": self.editor.execution_receipts.iter().map(|((task_id, action_id), receipt)| json!({
                "task_id": task_id,
                "action_id": action_id,
                "receipt": receipt,
            })).collect::<Vec<_>>(),
            "runtime_probes": self.runtime_probes.iter().map(runtime_probe_json).collect::<Vec<_>>(),
            "first_action_ms": {
                "task-1": self.first_action_ms("task-1"),
                "task-2": self.first_action_ms("task-2"),
            },
            "provider_totals": {
                "task-1": self.provider_totals("task-1"),
                "task-2": self.provider_totals("task-2"),
            },
            "prior_failed_provider_attempts": prior_failed_provider_attempts,
            "final": [task_summary(first), task_summary(second)],
            "image_task": {"task_id": second.id, "frame": "game-frame.png", "sha256": screenshot.content_sha256},
            "motion_input": "one bounded pointer-down frame at (320,320), then input override cleared before resume",
            "motion_ball_positions": self.motion_positions,
            "visual_evidence": ["first-proposal.png", "second-proposal.png", "final-editor.png", "game-frame.png", "editor-flow.mp4", "game-motion.mp4", "native-window-flow.png", "native-window-flow.mp4"],
        });
        std::fs::write(
            self.output.join("report.json"),
            serde_json::to_vec_pretty(&report)
                .map_err(|error| format!("serialize report: {error}"))?,
        )
        .map_err(|error| format!("write report JSON: {error}"))?;
        let markdown = format!(
            "# Task 524 live acceptance\n\nTwo semantic tasks ran through the native desktop editor against one disposable running Asset Breakout workspace. They used OpenRouter models `{}` and `{}` and required explicit acceptance and apply. The second task attached the verified SDL game frame before its provider request.\n\n- Playable warmup: {} ms to `assets_ready=true`, `loader_failed=false`, and `bricks_left=35`.\n- Task 1: first action {} ms; provider total {} ms, {} input tokens, {} output tokens, ${:.6}; apply receipt {} ms.\n- Task 2: first action {} ms; provider total {} ms, {} input tokens, {} output tokens, ${:.6}; apply receipt {} ms.\n- State preservation: both actions remain applied and both focused test receipts remain passing. One runtime session spans two consecutive generation advances; paused tick, `paddle_x`, `bricks_left`, and asset-loader health are unchanged.\n- Timing receipts: `report.json` includes compile and test receipts, per-apply wall time, provider timings, and total wall time; `target/task-524-final-live.log` records watcher compile/package/commit timing.\n- Visual evidence: `first-proposal.png`, `second-proposal.png`, `final-editor.png`, `game-frame.png`, `editor-flow.mp4`, and `game-motion.mp4`. Native editor and SDL videos are separate task-surface captures.\n",
            first.provider.model.as_deref().unwrap_or("unknown"),
            second.provider.model.as_deref().unwrap_or("unknown"),
            self.warmup_ms,
            self.first_action_ms("task-1").map_or_else(|| "unavailable".to_string(), |value| value.to_string()),
            first.metrics.elapsed_ms,
            first.metrics.input_tokens,
            first.metrics.output_tokens,
            first.metrics.estimated_cost_micros as f64 / 1_000_000.0,
            self.apply_wall_ms.get("task-1").copied().unwrap_or_default(),
            self.first_action_ms("task-2").map_or_else(|| "unavailable".to_string(), |value| value.to_string()),
            second.metrics.elapsed_ms,
            second.metrics.input_tokens,
            second.metrics.output_tokens,
            second.metrics.estimated_cost_micros as f64 / 1_000_000.0,
            self.apply_wall_ms.get("task-2").copied().unwrap_or_default(),
        );
        std::fs::write(self.output.join("report.md"), markdown)
            .map_err(|error| format!("write report markdown: {error}"))?;
        Ok(())
    }

    fn encode_flow_video(&self) -> Result<(), String> {
        if self.flow_frames < 2 {
            return Err("too few native editor frames for flow video".to_string());
        }
        let status = std::process::Command::new("ffmpeg")
            .current_dir(&self.output)
            .args([
                "-y",
                "-framerate",
                "2",
                "-i",
                "flow-%04d.png",
                "-vf",
                "scale=934:1034:force_original_aspect_ratio=decrease,pad=934:1034:(ow-iw)/2:(oh-ih)/2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-r",
                "24",
                "editor-flow.mp4",
            ])
            .status()
            .map_err(|error| format!("start ffmpeg: {error}"))?;
        if status.success() {
            for frame in 0..self.flow_frames {
                let _ = std::fs::remove_file(self.output.join(format!("flow-{frame:04}.png")));
            }
            Ok(())
        } else {
            Err(format!("ffmpeg exited with {status}"))
        }
    }

    fn capture_game_motion(&mut self) -> Result<(), String> {
        let start_x = inspect_scalar(
            request_live(
                &self.editor.client,
                69_990,
                stasis_runner::live::LiveCommand::Inspect {
                    path: "ball_x".to_string(),
                },
            )?
            .data,
        );
        let start_y = inspect_scalar(
            request_live(
                &self.editor.client,
                69_991,
                stasis_runner::live::LiveCommand::Inspect {
                    path: "ball_y".to_string(),
                },
            )?
            .data,
        );
        for frame in 0..12_u64 {
            let evidence = capture_frame(
                &self.editor.client,
                70_000 + frame * 2,
                format!("task-524-motion-{frame:04}"),
                &AtomicBool::new(false),
                Duration::from_secs(10),
            )?;
            std::fs::write(
                self.output.join(format!("game-motion-{frame:04}.png")),
                evidence.bytes,
            )
            .map_err(|error| format!("write SDL motion frame: {error}"))?;
            std::thread::sleep(Duration::from_millis(100));
        }
        let end_x = inspect_scalar(
            request_live(
                &self.editor.client,
                70_100,
                stasis_runner::live::LiveCommand::Inspect {
                    path: "ball_x".to_string(),
                },
            )?
            .data,
        );
        let end_y = inspect_scalar(
            request_live(
                &self.editor.client,
                70_101,
                stasis_runner::live::LiveCommand::Inspect {
                    path: "ball_y".to_string(),
                },
            )?
            .data,
        );
        if same_scalar(&start_x, &end_x) && same_scalar(&start_y, &end_y) {
            return Err("ball position did not advance during motion capture".to_string());
        }
        self.motion_positions = Some((
            json!({"x": start_x, "y": start_y}),
            json!({"x": end_x, "y": end_y}),
        ));
        let status = std::process::Command::new("ffmpeg")
            .current_dir(&self.output)
            .args([
                "-y",
                "-framerate",
                "10",
                "-i",
                "game-motion-%04d.png",
                "-vf",
                "pad=ceil(iw/2)*2:ceil(ih/2)*2",
                "-c:v",
                "libx264",
                "-pix_fmt",
                "yuv420p",
                "-r",
                "30",
                "game-motion.mp4",
            ])
            .status()
            .map_err(|error| format!("start SDL motion ffmpeg: {error}"))?;
        let result = status
            .success()
            .then_some(())
            .ok_or_else(|| format!("SDL motion ffmpeg exited with {status}"));
        if result.is_ok() {
            for frame in 0..12_u64 {
                let _ =
                    std::fs::remove_file(self.output.join(format!("game-motion-{frame:04}.png")));
            }
        }
        result
    }
}

#[cfg(target_os = "windows")]
fn task_summary(task: &stasis_ai::Task) -> Value {
    json!({
        "task_id": task.id,
        "objective": task.objective,
        "metrics": task.metrics,
        "provider": task.provider,
        "validation": task.validation,
        "actions": task.actions,
    })
}

#[cfg(target_os = "windows")]
fn runtime_probe_json(probe: &RuntimeProbe) -> Value {
    json!({
        "session_id": probe.session_id,
        "generation": probe.generation,
        "tick": probe.tick,
        "paddle_x": probe.paddle_x,
        "bricks_left": probe.bricks_left,
        "assets_ready": probe.assets_ready,
        "loader_failed": probe.loader_failed,
    })
}

#[cfg(target_os = "windows")]
fn same_scalar(left: &Value, right: &Value) -> bool {
    match (left.as_f64(), right.as_f64()) {
        (Some(left), Some(right)) => (left - right).abs() < f64::EPSILON,
        _ => left == right,
    }
}

#[cfg(target_os = "windows")]
impl eframe::App for LiveAcceptanceApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        if let Err(error) = self.save_editor_capture(context) {
            self.fail(context, error);
            return;
        }
        if self.state == AcceptanceState::Failed {
            context.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        if !self.wait_guard(context) {
            return;
        }
        if matches!(
            self.state,
            AcceptanceState::WaitFirstProposal
                | AcceptanceState::SettleFirstProposal
                | AcceptanceState::WaitSecondProposal
                | AcceptanceState::SettleSecondProposal
        ) && self.active_action_ready().is_some()
        {
            // Set the persistent collapse state before the production UI renders
            // the frame that the native screenshot command will capture.
            semantic_diff::expand_for_evidence(context);
        }
        self.editor.ui(context);
        semantic_diff::clear_evidence(context);
        self.usage_records.extend(self.provider_usage.try_iter());
        if self.state != AcceptanceState::HoldBeforeStart
            && self.pending_editor_capture.is_none()
            && self.active_action_ready().is_none()
            && self.flow_frames < 240
            && Instant::now() >= self.next_flow_capture
        {
            self.pending_editor_capture = Some(format!("flow-{:04}.png", self.flow_frames));
            self.flow_frames += 1;
            self.next_flow_capture = Instant::now() + Duration::from_millis(500);
            context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        if let Ok(error) = self.provider_errors.try_recv() {
            let retryable = error.contains("invalid agent JSON")
                && matches!(
                    self.state,
                    AcceptanceState::WaitFirstProposal | AcceptanceState::WaitSecondProposal
                );
            if retryable {
                if let Ok(task) = self.editor.state.session.active_task() {
                    let task_id = task.id.clone();
                    let retries = self
                        .editor
                        .controller
                        .snapshot(&task_id)
                        .map_or(0, |request| request.retry_count);
                    if retries == 0
                        && self
                            .editor
                            .controller
                            .retry(&mut self.editor.state.session, &task_id)
                            .is_ok()
                    {
                        self.state_started = Instant::now();
                        context.request_repaint();
                        return;
                    }
                }
            }
            self.fail(context, format!("OpenRouter request failed: {error}"));
            return;
        }
        if let Some(error) = self.editor.state.notice.clone().filter(|notice| {
            notice.starts_with("Apply failed") || notice.starts_with("Screenshot failed")
        }) {
            self.fail(context, error);
            return;
        }
        if matches!(
            self.state,
            AcceptanceState::WaitFirstProposal | AcceptanceState::WaitSecondProposal
        ) {
            if let Ok(task) = self.editor.state.session.active_task() {
                let completed = self
                    .editor
                    .controller
                    .snapshot(&task.id)
                    .is_some_and(|request| request.state == stasis_ai::TaskRequestState::Completed);
                let errors = task
                    .actions
                    .values()
                    .map(|action| {
                        self.editor
                            .state
                            .reviewed_preview(task.id.as_str(), action.id.as_str())
                            .err()
                            .unwrap_or_else(|| "proposal was not current".to_string())
                    })
                    .collect::<Vec<_>>();
                let preview_pending = errors.iter().any(|error| {
                    error.contains("still loading") || error.contains("Review the semantic preview")
                });
                if completed && self.active_action_ready().is_none() && !preview_pending {
                    let detail = errors.join("; ");
                    let task_id = task.id.to_string();
                    if detail.contains("must define exactly one Function")
                        && self.repair_attempted.insert(task_id)
                    {
                        if let Err(error) = self
                            .editor
                            .state
                            .handle(TaskSessionCommand::RejectAction)
                            .map_err(|error| error.to_string())
                            .and_then(|()| {
                                send_reply(
                                    &mut self.editor,
                                    "Repair the rejected semantic edit. The compiler requires new_source to contain the complete `function render(): i32 { ... }` definition, with no surrounding code or partial-line replacement. Preserve CRLF line endings and the requested single visual change.",
                                )
                            })
                        {
                            self.fail(context, error);
                            return;
                        }
                        self.state_started = Instant::now();
                        return;
                    }
                    self.fail(
                        context,
                        format!(
                            "provider completed without a usable semantic preview: {}",
                            if detail.is_empty() {
                                "no semantic action was proposed"
                            } else {
                                &detail
                            }
                        ),
                    );
                    return;
                }
            }
        }
        match self.state {
            AcceptanceState::HoldBeforeStart => {
                if Instant::now() >= self.hold_until {
                    if let Some(windows) = self.editor.windows.as_mut() {
                        if let Err(error) = windows.focus_game() {
                            self.fail(context, format!("focus game before acceptance: {error}"));
                            return;
                        }
                        std::thread::sleep(Duration::from_secs(2));
                        windows.tile();
                    }
                    if let Err(error) = start_task(
                        &mut self.editor,
                        "Polish the hot-swap accent",
                        "Update only function render in src/main.stasis. In the presentation_variant accent fill_rect, change the RGB values from 1.0, 0.30, 0.78 to 0.35, 0.85, 1.0. The edit's new_source must contain the complete function definition beginning `function render(): i32 {` and no surrounding code. Preserve the original CRLF line endings inside that function and keep every other line identical. Propose one semantic edit and no other file changes.",
                    ) {
                        self.fail(context, error);
                        return;
                    }
                    self.transition(AcceptanceState::WaitFirstProposal);
                }
            }
            AcceptanceState::WaitFirstProposal => {
                if self.pending_editor_capture.is_none() && self.active_action_ready().is_some() {
                    if let Err(error) = self.record_active("reviewed") {
                        self.fail(context, error);
                        return;
                    }
                    semantic_diff::expand_for_evidence(context);
                    self.transition(AcceptanceState::SettleFirstProposal);
                }
            }
            AcceptanceState::SettleFirstProposal
                if self.state_started.elapsed() >= Duration::from_secs(1) =>
            {
                self.pending_editor_capture = Some("first-proposal.png".to_string());
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                self.transition(AcceptanceState::CaptureFirstProposal);
            }
            AcceptanceState::CaptureFirstProposal if self.pending_editor_capture.is_none() => {
                if let Err(error) = self
                    .editor
                    .state
                    .handle(TaskSessionCommand::AcceptAction)
                    .map_err(|error| error.to_string())
                {
                    self.fail(context, error);
                    return;
                }
                self.apply_started
                    .insert("task-1".to_string(), Instant::now());
                if let Err(error) = self
                    .editor
                    .state
                    .handle(TaskSessionCommand::ApplyAction)
                    .map_err(|error| error.to_string())
                {
                    self.fail(context, error);
                    return;
                }
                self.editor.flush_intents();
                self.transition(AcceptanceState::WaitFirstApply);
            }
            AcceptanceState::WaitFirstApply => {
                let applied = self
                    .editor
                    .state
                    .session
                    .task("task-1")
                    .ok()
                    .is_some_and(|task| {
                        task.actions
                            .values()
                            .any(|action| matches!(action.state, ActionState::Applied))
                            && matches!(task.validation, ValidationStatus::Passed { .. })
                    });
                if applied {
                    if let Some(started) = self.apply_started.remove("task-1") {
                        self.apply_wall_ms
                            .insert("task-1".to_string(), started.elapsed().as_millis());
                    }
                    let generation = self.runtime_probes.last().map(|probe| probe.generation);
                    match probe_runtime(&self.editor.client, 61_000, generation) {
                        Ok(probe) => self.runtime_probes.push(probe),
                        Err(error) => {
                            self.fail(context, error);
                            return;
                        }
                    }
                    std::env::set_var(
                        "STASIS_AI_MODEL",
                        std::env::var("STASIS_EDITOR_OPENROUTER_IMAGE_MODEL")
                            .unwrap_or_else(|_| "openai/gpt-5.6-luna".to_string()),
                    );
                    if let Err(error) = create_openrouter_task(
                        &mut self.editor,
                        "Improve ball readability from the live frame",
                    ) {
                        self.fail(context, error);
                        return;
                    }
                    if let Err(error) = self
                        .editor
                        .state
                        .handle(TaskSessionCommand::AttachScreenshot)
                        .map_err(|error| error.to_string())
                    {
                        self.fail(context, error);
                        return;
                    }
                    self.editor.flush_intents();
                    self.transition(AcceptanceState::WaitFrame);
                }
            }
            AcceptanceState::WaitFrame => {
                let captured = self.editor.capture.is_none()
                    && self
                        .editor
                        .state
                        .session
                        .active_task()
                        .ok()
                        .is_some_and(|task| !task.screenshots.is_empty());
                if captured {
                    if let Err(error) = send_reply(
                        &mut self.editor,
                        "Inspect the attached verified running-game frame and confirm the ball is visible against the arena. Then update only function render in src/main.stasis: immediately before the existing assets.ball.draw call, insert exactly `fill_rect(ball_x - 1.0, ball_y - 1.0, BALL_SIZE + 2.0, BALL_SIZE + 2.0, 1.0, 1.0, 1.0, 1.0);`. The edit's new_source must contain the complete function definition beginning `function render(): i32 {` and no surrounding code. Preserve the original CRLF line endings inside that function and every other line. Propose one semantic edit and no other file changes.",
                    ) {
                        self.fail(context, error);
                        return;
                    }
                    self.transition(AcceptanceState::WaitSecondProposal);
                }
            }
            AcceptanceState::WaitSecondProposal => {
                if self.pending_editor_capture.is_none() && self.active_action_ready().is_some() {
                    if let Err(error) = self.record_active("reviewed_with_image") {
                        self.fail(context, error);
                        return;
                    }
                    semantic_diff::expand_for_evidence(context);
                    self.transition(AcceptanceState::SettleSecondProposal);
                }
            }
            AcceptanceState::SettleSecondProposal
                if self.state_started.elapsed() >= Duration::from_secs(1) =>
            {
                self.pending_editor_capture = Some("second-proposal.png".to_string());
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                self.transition(AcceptanceState::CaptureSecondProposal);
            }
            AcceptanceState::CaptureSecondProposal if self.pending_editor_capture.is_none() => {
                if let Err(error) = self
                    .editor
                    .state
                    .handle(TaskSessionCommand::AcceptAction)
                    .map_err(|error| error.to_string())
                {
                    self.fail(context, error);
                    return;
                }
                self.apply_started
                    .insert("task-2".to_string(), Instant::now());
                if let Err(error) = self
                    .editor
                    .state
                    .handle(TaskSessionCommand::ApplyAction)
                    .map_err(|error| error.to_string())
                {
                    self.fail(context, error);
                    return;
                }
                self.editor.flush_intents();
                self.transition(AcceptanceState::WaitSecondApply);
            }
            AcceptanceState::WaitSecondApply => {
                let applied = self
                    .editor
                    .state
                    .session
                    .task("task-2")
                    .ok()
                    .is_some_and(|task| {
                        task.actions
                            .values()
                            .any(|action| matches!(action.state, ActionState::Applied))
                            && matches!(task.validation, ValidationStatus::Passed { .. })
                    });
                if applied && self.pending_editor_capture.is_none() {
                    if let Some(started) = self.apply_started.remove("task-2") {
                        self.apply_wall_ms
                            .insert("task-2".to_string(), started.elapsed().as_millis());
                    }
                    let generation = self.runtime_probes.last().map(|probe| probe.generation);
                    match probe_runtime(&self.editor.client, 62_000, generation) {
                        Ok(probe) => self.runtime_probes.push(probe),
                        Err(error) => {
                            self.fail(context, error);
                            return;
                        }
                    }
                    if let Err(error) = launch_ball_for_motion(&self.editor.client) {
                        self.fail(context, error);
                        return;
                    }
                    if let Err(error) = request_live(
                        &self.editor.client,
                        63_003,
                        stasis_runner::live::LiveCommand::Resume,
                    ) {
                        self.fail(context, error);
                        return;
                    }
                    self.transition(AcceptanceState::ResumeMotion);
                }
            }
            AcceptanceState::ResumeMotion => {
                if self.state_started.elapsed() >= Duration::from_secs(2)
                    && self.pending_editor_capture.is_none()
                {
                    if let Err(error) = self.capture_game_motion() {
                        self.fail(context, error);
                        return;
                    }
                    self.pending_editor_capture = Some("final-editor.png".to_string());
                    context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
                    self.transition(AcceptanceState::CaptureFinal);
                }
            }
            AcceptanceState::CaptureFinal if self.pending_editor_capture.is_none() => {
                if let Err(error) = self.encode_flow_video() {
                    self.fail(context, error);
                    return;
                }
                if let Err(error) = self.finish_report() {
                    self.fail(context, error);
                    return;
                }
                if !self.final_hold.is_zero() {
                    self.transition(AcceptanceState::HoldAfterCapture);
                    return;
                }
                if let Some(result) = self.result.take() {
                    let _ = result.send(Ok(()));
                }
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            AcceptanceState::HoldAfterCapture
                if self.state_started.elapsed() >= self.final_hold =>
            {
                if let Some(result) = self.result.take() {
                    let _ = result.send(Ok(()));
                }
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            AcceptanceState::Failed
            | AcceptanceState::SettleFirstProposal
            | AcceptanceState::SettleSecondProposal
            | AcceptanceState::CaptureFirstProposal
            | AcceptanceState::CaptureSecondProposal
            | AcceptanceState::CaptureFinal
            | AcceptanceState::HoldAfterCapture => {}
        }
        context.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if let Some(result) = self.result.take() {
            let _ = result
                .send(Err(self.failure.take().unwrap_or_else(|| {
                    "editor closed before acceptance completed".to_string()
                })));
        }
        self.editor.host.shutdown_and_join();
        let _ = self
            .editor
            .client
            .submit(stasis_runner::live::LiveRequest::new(
                u64::MAX,
                stasis_runner::live::LiveCommand::Quit,
            ));
    }
}
