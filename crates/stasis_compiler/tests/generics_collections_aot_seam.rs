#[cfg(windows)]
use stasis_compiler::backend::aot::AotProcess;
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
#[cfg(windows)]
use std::{fs, path::PathBuf, process::Command};

#[cfg(windows)]
const ENTRY_PATH: &str = "samples/generics_collections/src/main.stasis";
#[cfg(windows)]
const ENTRY: &str = include_str!("../../../samples/generics_collections/src/main.stasis");
#[cfg(windows)]
const RIG2D_IMPORT: &str = "/.stasis_cache/toolchain/src/stdlib/rig2d.stasis";
#[cfg(windows)]
const ROOT: &str = "main";

#[cfg(windows)]
fn repository_root() -> PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

#[cfg(windows)]
fn repository_entry() -> String {
    ENTRY.replace(RIG2D_IMPORT, "../../../src/stdlib/rig2d.stasis")
}

#[cfg(windows)]
fn linker_path() -> PathBuf {
    if let Some(explicit) = std::env::var_os("STASIS_AOT_LINKER") {
        return PathBuf::from(explicit);
    }
    for candidate in ["link.exe", "lld-link.exe"] {
        let output = Command::new("where.exe")
            .arg(candidate)
            .output()
            .expect("locate Windows linker");
        if let Some(path) = output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout))
            .into_iter()
            .flat_map(|lines| {
                lines
                    .lines()
                    .map(str::trim)
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .find(|line| !line.is_empty())
        {
            return PathBuf::from(path);
        }
    }
    panic!("MSVC link.exe or lld-link.exe is required");
}

#[cfg(windows)]
fn dynload_artifacts() -> (PathBuf, PathBuf) {
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("deps directory")
        .to_path_buf();
    let artifacts = [&deps, deps.parent().expect("profile directory")]
        .into_iter()
        .find_map(|directory| {
            let import = directory.join("stasis_dynload.dll.lib");
            let runtime = directory.join("stasis_dynload.dll");
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("stasis_dynload artifacts");
    artifacts
}

#[cfg(windows)]
#[test]
fn generic_collection_sample_links_and_runs_in_aot() {
    let root = repository_root();
    let mut aot = AotProcess::new();
    aot.set_project_root(root.to_string_lossy())
        .expect("set sample AOT project root");
    aot.set_required_emit_roots(&[ROOT.to_string()]);
    aot.upsert_file(ENTRY_PATH, repository_entry());
    aot.compile()
        .expect("compile generic collection AOT sample");

    let output_dir = root
        .join("target")
        .join(format!("generic-collections-aot-{}", std::process::id()));
    let _ = fs::remove_dir_all(&output_dir);
    fs::create_dir_all(&output_dir).expect("create AOT evidence directory");
    let (import, runtime) = dynload_artifacts();
    fs::copy(runtime, output_dir.join("stasis_dynload.dll")).expect("copy AOT runtime");
    let linked = output_dir.join("generic_collections.exe");
    let config = AotLinkConfig {
        linker_path: Some(linker_path()),
        runtime_lib_paths: vec![import],
        target: AotTarget::Native,
    };
    aot.link_executable_for_i32_noarg_function(ROOT, &linked, &config)
        .expect("link generic collection AOT sample");
    let status = Command::new(root.join(".cargo/stasis-sign-and-run.cmd"))
        .arg(linked.file_name().expect("linked AOT executable name"))
        .current_dir(&output_dir)
        .status()
        .expect("run linked generic collection AOT sample");
    let aot_code = status.code().expect("AOT process exit code");
    let signed_execution_required =
        std::env::var_os("STASIS_REQUIRE_SIGNED_EXECUTION").is_some_and(|value| value == "1");
    if aot_code == 4551 && !signed_execution_required {
        eprintln!(
            "skipping linked AOT execution parity: Windows Application Control returned 4551 and signed execution is not required"
        );
        return;
    }
    assert_eq!(aot_code, 0, "linked generic collection AOT result");
    let _ = fs::remove_dir_all(output_dir);
}
