use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::EngineEntrypoints;

const DEFAULT_SAMPLES: usize = 3;
const DEFAULT_TICK_SLEEP_US: u64 = 16_666; // ~60fps
const DEFAULT_WARMUP_TICKS: u32 = 8;
const DEFAULT_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Clone)]
struct BenchConfig {
    samples: usize,
    tick_sleep_us: u64,
    warmup_ticks: u32,
    timeout_ms: u64,
}

#[derive(Debug, Clone, Copy)]
struct SampleMetrics {
    warm_update_total_ms: f64,
    compile_ms: f64,
    package_ms: f64,
    hook_ms: f64,
    tick_render_ms: f64,
}

#[derive(Debug, Clone, Copy)]
struct AggregatedMetrics {
    warm_update_total_ms_p50: f64,
    warm_update_total_ms_p95: f64,
    compile_ms_p50: f64,
    compile_ms_p95: f64,
    package_ms_p50: f64,
    package_ms_p95: f64,
    hook_ms_p50: f64,
    hook_ms_p95: f64,
    tick_render_ms_p50: f64,
    tick_render_ms_p95: f64,
}

fn parse_args() -> Result<BenchConfig, String> {
    let mut config = BenchConfig {
        samples: DEFAULT_SAMPLES,
        tick_sleep_us: DEFAULT_TICK_SLEEP_US,
        warmup_ticks: DEFAULT_WARMUP_TICKS,
        timeout_ms: DEFAULT_TIMEOUT_MS,
    };

    let args: Vec<String> = env::args().skip(1).collect();
    let mut index = 0usize;
    while index < args.len() {
        match args[index].as_str() {
            "--samples" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--samples requires a value".to_string());
                };
                config.samples = value
                    .parse::<usize>()
                    .map_err(|error| format!("invalid --samples '{value}': {error}"))?;
            }
            "--tick-sleep-us" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--tick-sleep-us requires a value".to_string());
                };
                config.tick_sleep_us = value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --tick-sleep-us '{value}': {error}"))?;
            }
            "--warmup-ticks" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--warmup-ticks requires a value".to_string());
                };
                config.warmup_ticks = value
                    .parse::<u32>()
                    .map_err(|error| format!("invalid --warmup-ticks '{value}': {error}"))?;
            }
            "--timeout-ms" => {
                index += 1;
                let Some(value) = args.get(index) else {
                    return Err("--timeout-ms requires a value".to_string());
                };
                config.timeout_ms = value
                    .parse::<u64>()
                    .map_err(|error| format!("invalid --timeout-ms '{value}': {error}"))?;
            }
            "--help" | "-h" => {
                print_help();
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
        index += 1;
    }

    if config.samples == 0 {
        return Err("--samples must be > 0".to_string());
    }
    if config.timeout_ms == 0 {
        return Err("--timeout-ms must be > 0".to_string());
    }
    Ok(config)
}

fn print_help() {
    println!("stasis engine hot-update benchmark (dev JIT)");
    println!("  --samples <usize>          default: 3");
    println!("  --tick-sleep-us <u64>      default: 16666 (~60fps)");
    println!("  --warmup-ticks <u32>       default: 8");
    println!("  --timeout-ms <u64>         default: 5000");
}

fn fixture_source(version: i32) -> String {
    // Tick/render return i32 so hosts can treat non-zero as exit. For benchmark we always return 0.
    format!(
        "global State {{ tick_version: i32; render_version: i32; }}\n\
         function tick(): i32 {{ State.tick_version = {version}; return 0; }}\n\
         function render(): i32 {{ State.render_version = {version}; return 0; }}\n\
         function on_code_swap(): void {{ return; }}\n"
    )
}

fn write_fixture(path: &Path, version: i32) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create fixture directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(path, fixture_source(version))
        .map_err(|error| format!("failed to write fixture {}: {error}", path.display()))
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

struct WatchService {
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
}

impl WatchService {
    fn start(root: &Path) -> Result<Self, String> {
        let (tx, rx) = channel::<notify::Result<Event>>();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )
        .map_err(|error| format!("failed to create notify watcher: {error}"))?;
        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|error| format!("failed to watch {}: {error}", root.display()))?;
        Ok(Self {
            _watcher: watcher,
            rx,
        })
    }

    fn drain_stasis_changes(&mut self) -> Vec<PathBuf> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Ok(event)) => {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        for path in event.paths {
                            if path
                                .extension()
                                .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
                            {
                                out.push(path);
                            }
                        }
                    }
                }
                Ok(Err(_)) => {}
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

fn percentile_ms(samples: &[f64], percentile: usize) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let rank = ((percentile as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[rank.min(sorted.len() - 1)]
}

fn aggregate(samples: &[SampleMetrics]) -> AggregatedMetrics {
    let warm_total: Vec<f64> = samples.iter().map(|s| s.warm_update_total_ms).collect();
    let compile: Vec<f64> = samples.iter().map(|s| s.compile_ms).collect();
    let package: Vec<f64> = samples.iter().map(|s| s.package_ms).collect();
    let hook: Vec<f64> = samples.iter().map(|s| s.hook_ms).collect();
    let tick_render: Vec<f64> = samples.iter().map(|s| s.tick_render_ms).collect();

    AggregatedMetrics {
        warm_update_total_ms_p50: percentile_ms(&warm_total, 50),
        warm_update_total_ms_p95: percentile_ms(&warm_total, 95),
        compile_ms_p50: percentile_ms(&compile, 50),
        compile_ms_p95: percentile_ms(&compile, 95),
        package_ms_p50: percentile_ms(&package, 50),
        package_ms_p95: percentile_ms(&package, 95),
        hook_ms_p50: percentile_ms(&hook, 50),
        hook_ms_p95: percentile_ms(&hook, 95),
        tick_render_ms_p50: percentile_ms(&tick_render, 50),
        tick_render_ms_p95: percentile_ms(&tick_render, 95),
    }
}

fn run_one_warm_update_sample(
    root: &Path,
    root_path_str: &str,
    watcher: &mut WatchService,
    jit: &mut JitProcess,
    tick_code_ptr: &mut u64,
    render_code_ptr: &mut u64,
    on_code_swap_code_ptr: Option<u64>,
    tick_sleep: Duration,
    expected_version: i32,
    timeout: Duration,
    tick_version_host: &mut i32,
    render_version_host: &mut i32,
) -> Result<SampleMetrics, String> {
    // Remove any queued events so this sample starts from a clean slate.
    let _ = watcher.drain_stasis_changes();

    // Trigger edit.
    write_fixture(root, expected_version)?;
    let change_start = Instant::now();

    let deadline = change_start + timeout;
    loop {
        if Instant::now() > deadline {
            return Err(format!(
                "timeout waiting for hot-update sample to complete after {}ms",
                timeout.as_millis()
            ));
        }

        let changed_paths = watcher.drain_stasis_changes();
        let needs_recompile = changed_paths.iter().any(|path| path == root);
        if needs_recompile {
            let root_text = fs::read_to_string(root)
                .map_err(|error| format!("failed to read {}: {error}", root.display()))?;
            jit.upsert_file(root_path_str.to_owned(), root_text);
            let _ = jit.refresh_imported_sources_from_disk(root_path_str);

            let t_compile = Instant::now();
            jit.compile()
                .map_err(|error| format!("jit compile failed: {error:?}"))?;
            let compile_ms = t_compile.elapsed().as_secs_f64() * 1000.0;

            let t_package = Instant::now();
            let package = jit
                .build_engine_package(&EngineEntrypoints::runtime_default())
                .map_err(|error| format!("build_engine_package failed: {error}"))?;
            let package_ms = t_package.elapsed().as_secs_f64() * 1000.0;

            let t_hook = Instant::now();
            if let Some(hook) = on_code_swap_code_ptr {
                stasis_dynload::invoke_noarg_void(hook as usize)
                    .map_err(|error| format!("on_code_swap hook failed: {error}"))?;
            }
            let hook_ms = t_hook.elapsed().as_secs_f64() * 1000.0;

            // Commit pointer swap.
            *tick_code_ptr = package.tick_code_ptr;
            *render_code_ptr = package.render_code_ptr;

            // First tick/render with updated code.
            let t_tick_render = Instant::now();
            let tick_rc = stasis_dynload::invoke_noarg_i32(*tick_code_ptr as usize)
                .map_err(|error| format!("tick invocation failed: {error}"))?;
            let render_rc = stasis_dynload::invoke_noarg_i32(*render_code_ptr as usize)
                .map_err(|error| format!("render invocation failed: {error}"))?;
            if tick_rc != 0 || render_rc != 0 {
                return Err(format!(
                    "expected tick/render to return 0 (got tick_rc={tick_rc} render_rc={render_rc})"
                ));
            }
            let tick_render_ms = t_tick_render.elapsed().as_secs_f64() * 1000.0;

            if *tick_version_host != expected_version || *render_version_host != expected_version {
                return Err(format!(
                    "updated tick/render did not publish expected versions: expected={} tick_version={} render_version={}",
                    expected_version, *tick_version_host, *render_version_host
                ));
            }

            let warm_update_total_ms = change_start.elapsed().as_secs_f64() * 1000.0;
            return Ok(SampleMetrics {
                warm_update_total_ms,
                compile_ms,
                package_ms,
                hook_ms,
                tick_render_ms,
            });
        }

        // Simulate steady-state tick/render loop work while waiting for the watcher to observe the edit.
        let tick_rc = stasis_dynload::invoke_noarg_i32(*tick_code_ptr as usize)
            .map_err(|error| format!("tick invocation failed: {error}"))?;
        let render_rc = stasis_dynload::invoke_noarg_i32(*render_code_ptr as usize)
            .map_err(|error| format!("render invocation failed: {error}"))?;
        if tick_rc != 0 || render_rc != 0 {
            return Err(format!(
                "expected tick/render to return 0 during steady-state (got tick_rc={tick_rc} render_rc={render_rc})"
            ));
        }

        if tick_sleep > Duration::from_micros(0) {
            std::thread::sleep(tick_sleep);
        }
    }
}

fn main() -> Result<(), String> {
    let config = parse_args()?;
    println!(
        "bench engine_hot_update samples={} tick_sleep_us={} warmup_ticks={} timeout_ms={}",
        config.samples, config.tick_sleep_us, config.warmup_ticks, config.timeout_ms
    );

    // Isolate from other host processes by clearing JIT global state.
    stasis_dynload::clear_jit_i32_global_table();
    stasis_dynload::clear_jit_f32_global_table();
    stasis_dynload::clear_jit_i32_array_global_table();
    stasis_dynload::clear_jit_f32_array_global_table();
    stasis_dynload::clear_jit_string_literal_table();

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_nanos();
    let temp_root = env::temp_dir().join(format!(
        "stasis_engine_hot_update_{}_{}",
        std::process::id(),
        stamp
    ));
    let source_path = temp_root.join("engine_hot_update_bench.stasis");
    write_fixture(&source_path, 0)?;

    let mut watcher = WatchService::start(&temp_root)?;

    let mut tick_version_host: i32 = -1;
    let mut render_version_host: i32 = -1;
    stasis_dynload::register_global_i32_ptr(
        hash_global_path("State.tick_version"),
        &mut tick_version_host,
    );
    stasis_dynload::register_global_i32_ptr(
        hash_global_path("State.render_version"),
        &mut render_version_host,
    );

    let mut jit = JitProcess::new();
    let root_path_str = source_path.to_string_lossy().to_string();
    let root_source = fs::read_to_string(&source_path)
        .map_err(|error| format!("failed to read {}: {error}", source_path.display()))?;
    jit.upsert_file(root_path_str.clone(), root_source);
    jit.compile()
        .map_err(|error| format!("initial jit compile failed: {error:?}"))?;
    let package = jit
        .build_engine_package(&EngineEntrypoints::runtime_default())
        .map_err(|error| format!("failed to build engine package: {error}"))?;
    let mut tick_code_ptr = package.tick_code_ptr;
    let mut render_code_ptr = package.render_code_ptr;
    let on_code_swap_code_ptr = package.on_code_swap_code_ptr;

    // Warmup: execute a few frames so the steady-state tick/render loop is representative.
    for _ in 0..config.warmup_ticks {
        let tick_rc = stasis_dynload::invoke_noarg_i32(tick_code_ptr as usize)
            .map_err(|error| format!("warmup tick failed: {error}"))?;
        let render_rc = stasis_dynload::invoke_noarg_i32(render_code_ptr as usize)
            .map_err(|error| format!("warmup render failed: {error}"))?;
        if tick_rc != 0 || render_rc != 0 {
            return Err(format!(
                "expected warmup tick/render to return 0 (got tick_rc={tick_rc} render_rc={render_rc})"
            ));
        }
        if config.tick_sleep_us > 0 {
            std::thread::sleep(Duration::from_micros(config.tick_sleep_us));
        }
    }

    let mut samples: Vec<SampleMetrics> = Vec::with_capacity(config.samples);
    for sample_index in 0..config.samples {
        let expected_version = i32::try_from(sample_index + 1).unwrap_or(i32::MAX);
        samples.push(run_one_warm_update_sample(
            &source_path,
            &root_path_str,
            &mut watcher,
            &mut jit,
            &mut tick_code_ptr,
            &mut render_code_ptr,
            on_code_swap_code_ptr,
            Duration::from_micros(config.tick_sleep_us),
            expected_version,
            Duration::from_millis(config.timeout_ms),
            &mut tick_version_host,
            &mut render_version_host,
        )?);
    }

    let aggregated = aggregate(&samples);
    println!(
        "result warm_update_total_ms_p50={:.3} warm_update_total_ms_p95={:.3} compile_ms_p50={:.3} compile_ms_p95={:.3} package_ms_p50={:.3} package_ms_p95={:.3} hook_ms_p50={:.3} hook_ms_p95={:.3} tick_render_ms_p50={:.3} tick_render_ms_p95={:.3}",
        aggregated.warm_update_total_ms_p50,
        aggregated.warm_update_total_ms_p95,
        aggregated.compile_ms_p50,
        aggregated.compile_ms_p95,
        aggregated.package_ms_p50,
        aggregated.package_ms_p95,
        aggregated.hook_ms_p50,
        aggregated.hook_ms_p95,
        aggregated.tick_render_ms_p50,
        aggregated.tick_render_ms_p95,
    );

    let _ = fs::remove_dir_all(&temp_root);
    Ok(())
}
