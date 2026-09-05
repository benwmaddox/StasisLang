use std::collections::{BTreeMap, BTreeSet};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::patch_plan::PatchReason;
use stasis_compiler::backend::EngineEntrypoints;

struct Config {
    label: String,
    entry: PathBuf,
    edit_file: PathBuf,
    needle: String,
    replacement: String,
    cold_samples: usize,
    warmups: usize,
    samples: usize,
}

#[derive(Default)]
struct Samples {
    total_micros: Vec<u64>,
    plan_micros: Vec<u64>,
    codegen_micros: Vec<u64>,
    finalize_micros: Vec<u64>,
    package_micros: Vec<u64>,
    publication_micros: Vec<u64>,
}

fn required_arg(args: &[String], index: &mut usize, name: &str) -> Result<String, String> {
    *index += 1;
    args.get(*index)
        .cloned()
        .ok_or_else(|| format!("{name} requires a value"))
}

fn parse_args() -> Result<Config, String> {
    let args: Vec<String> = env::args().skip(1).collect();
    let mut label = None;
    let mut entry = None;
    let mut edit_file = None;
    let mut needle = None;
    let mut replacement = None;
    let mut cold_samples = 5;
    let mut warmups = 5;
    let mut samples = 30;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--label" => label = Some(required_arg(&args, &mut index, "--label")?),
            "--entry" => entry = Some(PathBuf::from(required_arg(&args, &mut index, "--entry")?)),
            "--edit-file" => {
                edit_file = Some(PathBuf::from(required_arg(
                    &args,
                    &mut index,
                    "--edit-file",
                )?))
            }
            "--needle" => needle = Some(required_arg(&args, &mut index, "--needle")?),
            "--replacement" => {
                replacement = Some(required_arg(&args, &mut index, "--replacement")?)
            }
            "--cold-samples" => {
                cold_samples = required_arg(&args, &mut index, "--cold-samples")?
                    .parse()
                    .map_err(|error| format!("invalid --cold-samples: {error}"))?
            }
            "--warmups" => {
                warmups = required_arg(&args, &mut index, "--warmups")?
                    .parse()
                    .map_err(|error| format!("invalid --warmups: {error}"))?
            }
            "--samples" => {
                samples = required_arg(&args, &mut index, "--samples")?
                    .parse()
                    .map_err(|error| format!("invalid --samples: {error}"))?
            }
            "--help" | "-h" => {
                println!("project_selective_jit_bench --label NAME --entry PATH --edit-file PATH --needle TEXT --replacement TEXT [--cold-samples 5 --warmups 5 --samples 30]");
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
        index += 1;
    }
    if cold_samples == 0 || samples == 0 {
        return Err("cold and measured sample counts must be positive".to_string());
    }
    Ok(Config {
        label: label.ok_or_else(|| "--label is required".to_string())?,
        entry: entry.ok_or_else(|| "--entry is required".to_string())?,
        edit_file: edit_file.ok_or_else(|| "--edit-file is required".to_string())?,
        needle: needle.ok_or_else(|| "--needle is required".to_string())?,
        replacement: replacement.ok_or_else(|| "--replacement is required".to_string())?,
        cold_samples,
        warmups,
        samples,
    })
}

fn canonical_text(path: &Path) -> String {
    path.canonicalize()
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .to_string()
}

fn import_paths(source: &str) -> Vec<PathBuf> {
    source
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with("import ") {
                return None;
            }
            let quoted = line.split_once('"')?.1;
            Some(PathBuf::from(quoted.split_once('"')?.0))
        })
        .collect()
}

fn load_project(entry: &Path) -> Result<BTreeMap<String, String>, String> {
    let entry = entry
        .canonicalize()
        .map_err(|error| format!("failed canonicalizing {}: {error}", entry.display()))?;
    let mut pending = vec![entry];
    let mut visited = BTreeSet::new();
    let mut sources = BTreeMap::new();
    while let Some(path) = pending.pop() {
        if !visited.insert(path.clone()) {
            continue;
        }
        let source = fs::read_to_string(&path)
            .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
        let parent = path.parent().unwrap_or(Path::new("."));
        for import in import_paths(&source) {
            let imported = parent.join(import).canonicalize().map_err(|error| {
                format!(
                    "failed resolving an import from {}: {error}",
                    path.display()
                )
            })?;
            pending.push(imported);
        }
        sources.insert(canonical_text(&path), source);
    }
    Ok(sources)
}

fn new_process(sources: &BTreeMap<String, String>) -> JitProcess {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&[
        "main".to_string(),
        "tick".to_string(),
        "render".to_string(),
        "on_code_swap".to_string(),
    ]);
    for (path, source) in sources {
        process.upsert_file(path.clone(), source.clone());
    }
    process
}

fn percentile_ms(samples: &[u64], percentile: usize) -> f64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let rank = (percentile * sorted.len()).div_ceil(100).saturating_sub(1);
    sorted[rank.min(sorted.len() - 1)] as f64 / 1000.0
}

fn elapsed_micros(started: Instant) -> u64 {
    u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX)
}

fn main() -> Result<(), String> {
    let config = parse_args()?;
    let sources = load_project(&config.entry)?;
    let edit_path = canonical_text(&config.edit_file);
    let base_edit_source = sources.get(&edit_path).cloned().ok_or_else(|| {
        format!(
            "edit file {} is not reachable from the entry imports",
            config.edit_file.display()
        )
    })?;
    if !base_edit_source.contains(&config.needle) {
        return Err(format!(
            "edit needle was not found in {}",
            config.edit_file.display()
        ));
    }
    let edited_source = base_edit_source.replacen(&config.needle, &config.replacement, 1);

    let mut cold = Vec::with_capacity(config.cold_samples);
    let mut reachable_functions = 0;
    for _ in 0..config.cold_samples {
        let mut process = new_process(&sources);
        let started = Instant::now();
        process
            .compile()
            .map_err(|error| format!("cold compile: {error:?}"))?;
        cold.push(elapsed_micros(started));
        reachable_functions = process.artifacts().len();
    }

    let mut process = new_process(&sources);
    process
        .compile()
        .map_err(|error| format!("baseline compile: {error:?}"))?;
    let baseline_package = process
        .build_engine_package(&EngineEntrypoints::runtime_default())
        .map_err(|error| format!("baseline package: {error}"))?;
    stasis_dynload::begin_jit_host_entry_session(baseline_package.host_entry_targets(1)?)?;
    let mut edited = false;
    let mut measured = Samples::default();
    let mut emitted_functions = 0;
    let mut reused_functions = 0;
    let mut changed_functions = 0;
    let mut affected_host_entries = 0;
    for sample in 0..(config.warmups + config.samples) {
        edited = !edited;
        process.upsert_file(
            edit_path.clone(),
            if edited {
                edited_source.clone()
            } else {
                base_edit_source.clone()
            },
        );
        let total_started = Instant::now();
        process
            .compile()
            .map_err(|error| format!("warm compile {sample}: {error:?}"))?;
        let total_micros = elapsed_micros(total_started);
        let metadata = process
            .generation_metadata()
            .ok_or_else(|| "warm compile metadata missing".to_string())?;
        emitted_functions = metadata.emitted_function_ids.len();
        reused_functions = metadata.reused_function_ids.len();
        changed_functions = metadata
            .patch_reasons
            .iter()
            .filter(|reason| {
                matches!(
                    reason.reason,
                    PatchReason::BodyChanged
                        | PatchReason::AddedOrSignatureChanged
                        | PatchReason::BecameReachable
                        | PatchReason::LoweredContractChanged
                        | PatchReason::CompilerLayoutChanged
                )
            })
            .count();
        affected_host_entries = metadata.affected_host_entries.len();
        let package_started = Instant::now();
        let package = process
            .build_engine_package(&EngineEntrypoints::runtime_default())
            .map_err(|error| format!("package {sample}: {error}"))?;
        let package_micros = elapsed_micros(package_started);
        let targets = package.host_entry_targets((sample + 2) as u64)?;
        let publication_started = Instant::now();
        stasis_dynload::publish_jit_host_entry_targets(targets)?;
        let publication_micros = elapsed_micros(publication_started);
        if sample >= config.warmups {
            measured.total_micros.push(total_micros);
            measured.plan_micros.push(metadata.plan_micros);
            measured.codegen_micros.push(metadata.codegen_micros);
            measured.finalize_micros.push(metadata.finalize_micros);
            measured.package_micros.push(package_micros);
            measured.publication_micros.push(publication_micros);
        }
    }

    println!(
        "result label={} reachable_functions={} changed_functions={} emitted_functions={} reused_functions={} affected_host_entries={} cold_ms_p50={:.3} cold_ms_p95={:.3} compile_ready_ms_p50={:.3} compile_ready_ms_p95={:.3} plan_ms_p50={:.3} plan_ms_p95={:.3} codegen_ms_p50={:.3} codegen_ms_p95={:.3} finalize_ms_p50={:.3} finalize_ms_p95={:.3} package_ms_p50={:.3} package_ms_p95={:.3} publication_ms_p50={:.3} publication_ms_p95={:.3}",
        config.label,
        reachable_functions,
        changed_functions,
        emitted_functions,
        reused_functions,
        affected_host_entries,
        percentile_ms(&cold, 50),
        percentile_ms(&cold, 95),
        percentile_ms(&measured.total_micros, 50),
        percentile_ms(&measured.total_micros, 95),
        percentile_ms(&measured.plan_micros, 50),
        percentile_ms(&measured.plan_micros, 95),
        percentile_ms(&measured.codegen_micros, 50),
        percentile_ms(&measured.codegen_micros, 95),
        percentile_ms(&measured.finalize_micros, 50),
        percentile_ms(&measured.finalize_micros, 95),
        percentile_ms(&measured.package_micros, 50),
        percentile_ms(&measured.package_micros, 95),
        percentile_ms(&measured.publication_micros, 50),
        percentile_ms(&measured.publication_micros, 95),
    );
    Ok(())
}
