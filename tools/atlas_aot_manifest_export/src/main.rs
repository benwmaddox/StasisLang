use std::env;
use std::fs;
use std::path::PathBuf;

use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::{EngineEntrypoints, ReachabilityPolicy};
use stasis_compiler::frontend::module_graph::load_project_module_graph;

fn run() -> Result<(), String> {
    let mut args = env::args_os().skip(1);
    let project_root = args
        .next()
        .map(PathBuf::from)
        .ok_or("usage: atlas_aot_manifest_export <project-root> [entry.stasis] [output-dir]")?;
    let entry = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("src/main.stasis"));
    let output_dir = args
        .next()
        .map(PathBuf::from)
        .unwrap_or_else(|| project_root.join("build/atlas-affinity-aot-manifest"));
    if args.next().is_some() {
        return Err("too many arguments; usage: atlas_aot_manifest_export <project-root> [entry.stasis] [output-dir]".into());
    }
    let project_root = fs::canonicalize(&project_root)
        .map_err(|error| format!("cannot resolve project root: {error}"))?;
    let entry = if entry.is_absolute() { entry } else { project_root.join(entry) };
    let (graph, sources) = load_project_module_graph(&project_root, &entry)
        .map_err(|diagnostic| format!("module graph failed: {}", diagnostic.message))?;
    let mut process = AotProcess::new();
    process.set_reachability_policy(ReachabilityPolicy::Release);
    process.set_project_root(project_root.to_string_lossy().to_string())?;
    for (path, source) in sources {
        process.upsert_file(path, source);
    }
    process.compile().map_err(|error| format!("AOT compile failed: {error:?}"))?;
    let bundle = process
        .write_engine_bundle(
            &EngineEntrypoints { tick: "tick".into(), render: "render".into(), on_code_swap: None },
            &output_dir,
        )
        .map_err(|error| format!("engine bundle write failed: {error}"))?;
    println!(
        "manifest={} object_count={} modules={}",
        bundle.manifest_path.display(),
        bundle.object_paths().len(),
        graph.modules().len()
    );
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("atlas_aot_manifest_export: {error}");
        std::process::exit(1);
    }
}
