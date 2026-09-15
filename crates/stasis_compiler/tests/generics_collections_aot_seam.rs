use serde_json::json;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess};
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const ORACLE_PATH: &str = "tests/stasis/seams/generics_parity_oracle.stasis";
const ORACLE: &str = include_str!("../../../tests/stasis/seams/generics_parity_oracle.stasis");
const MODULE_PATH: &str = "tests/stasis/seams/generics_parity_module.stasis";
const MODULE: &str = include_str!("../../../tests/stasis/seams/generics_parity_module.stasis");
const EXPECTED_DIGEST: i32 = 685;
const STATUS_ROOT: &str = "generics_native_oracle_status";
const LOW_TRAP_ROOT: &str = "generics_native_bounds_low";
const HIGH_TRAP_ROOT: &str = "generics_native_bounds_high";
const TRAP_CHILD_ROOT: &str = "STASIS_GENERICS_TRAP_CHILD_ROOT";
#[cfg(windows)]
const TRAP_CHILD_EXE: &str = "STASIS_GENERICS_TRAP_CHILD_EXE";
#[cfg(windows)]
const NATIVE_EXE: &str = "STASIS_GENERICS_NATIVE_EXE";
const EVIDENCE_DIR: &str = "STASIS_GENERICS_DESKTOP_EVIDENCE_DIR";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(windows)]
const WINDOWS_ILLEGAL_INSTRUCTION: i32 = 0xC000001D_u32 as i32;
#[cfg(windows)]
const WINDOWS_ACCESS_VIOLATION: i32 = 0xC0000005_u32 as i32;

const NATIVE_WRAPPERS: &str = r#"
global generics_native_bounds_index: i32;

function generics_native_oracle_status(): i32 {
    if (main() != 685) { return 1; }
    if (tick() != 685) { return 2; }
    if (render(0) != 10 || render(2) != 10) { return 3; }
    return 0;
}

function generics_native_bounds_low(): i32 {
    generics_native_bounds_index = -1;
    return render(generics_native_bounds_index);
}

function generics_native_bounds_high(): i32 {
    generics_native_bounds_index = 3;
    return render(generics_native_bounds_index);
}
"#;

struct TestTree(PathBuf);

impl Drop for TestTree {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn source() -> String {
    format!("{ORACLE}\n{NATIVE_WRAPPERS}")
}

fn roots() -> Vec<String> {
    [
        "main",
        "tick",
        "render",
        STATUS_ROOT,
        LOW_TRAP_ROOT,
        HIGH_TRAP_ROOT,
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn configured_jit(required: &[String]) -> JitProcess {
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(required);
    jit.upsert_file(ORACLE_PATH, source());
    jit.upsert_file(MODULE_PATH, MODULE);
    jit.compile()
        .expect("compile shared generics oracle for JIT");
    jit
}

fn configured_aot() -> AotProcess {
    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots());
    aot.upsert_file(ORACLE_PATH, source());
    aot.upsert_file(MODULE_PATH, MODULE);
    aot.compile()
        .expect("compile shared generics oracle for native AOT");
    aot
}

fn dynload_artifacts() -> (PathBuf, PathBuf) {
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let profile = deps
        .parent()
        .expect("Cargo profile directory")
        .to_path_buf();
    let (link_name, runtime_name) = if cfg!(windows) {
        ("stasis_dynload.dll.lib", "stasis_dynload.dll")
    } else if cfg!(target_os = "macos") {
        ("libstasis_dynload.dylib", "libstasis_dynload.dylib")
    } else {
        ("libstasis_dynload.so", "libstasis_dynload.so")
    };
    let artifacts = [&deps, &profile]
        .into_iter()
        .find_map(|directory| {
            let link = directory.join(link_name);
            let runtime = directory.join(runtime_name);
            (link.is_file() && runtime.is_file()).then_some((link, runtime))
        })
        .unwrap_or_else(|| {
            panic!(
                "fresh stasis_dynload artifacts {link_name} and {runtime_name} were not found in {} or {}",
                deps.display(),
                profile.display()
            )
        });
    artifacts
}

fn executable_name(root: &str) -> String {
    format!("generics_{root}{}", std::env::consts::EXE_SUFFIX)
}

fn prepend_search_path(command: &mut Command, variable: &str, directory: &Path) {
    let mut paths = vec![directory.to_path_buf()];
    if let Some(existing) = std::env::var_os(variable) {
        paths.extend(std::env::split_paths(&existing));
    }
    command.env(
        variable,
        std::env::join_paths(paths).expect("join native runtime search path"),
    );
}

fn executable_command(executable: &Path) -> Command {
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-Command",
            "Add-Type 'using System; using System.Runtime.InteropServices; public static class StasisTrapMode { [DllImport(\"kernel32.dll\")] public static extern uint SetErrorMode(uint mode); }'; [StasisTrapMode]::SetErrorMode(3) | Out-Null; & $env:STASIS_GENERICS_NATIVE_EXE; exit $LASTEXITCODE",
        ]);
        command.env(NATIVE_EXE, executable);
        command
    };
    #[cfg(not(windows))]
    let mut command = Command::new(executable);
    let directory = executable.parent().expect("AOT executable directory");
    command.current_dir(directory);
    prepend_search_path(&mut command, "PATH", directory);
    if cfg!(target_os = "linux") {
        prepend_search_path(&mut command, "LD_LIBRARY_PATH", directory);
    }
    if cfg!(target_os = "macos") {
        prepend_search_path(&mut command, "DYLD_LIBRARY_PATH", directory);
    }
    command
}

fn link_executable(aot: &AotProcess, root: &str, directory: &Path, runtime_link: &Path) -> PathBuf {
    let executable = directory.join(executable_name(root));
    aot.link_executable_for_i32_noarg_function(
        root,
        &executable,
        &AotLinkConfig {
            linker_path: None,
            runtime_lib_paths: vec![runtime_link.to_path_buf()],
            target: AotTarget::Native,
        },
    )
    .unwrap_or_else(|error| panic!("link shared generics AOT root {root}: {error}"));
    executable
}

fn run(executable: &Path) -> Output {
    run_bounded(
        executable_command(executable),
        &format!("linked AOT executable {}", executable.display()),
    )
}

fn run_bounded(mut command: Command, description: &str) -> Output {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command.spawn().unwrap_or_else(|error| {
        if error.raw_os_error() == Some(4551) {
            panic!("{description} was blocked by Windows Application Control (error 4551)");
        }
        panic!("spawn {description}: {error}")
    });
    let mut stdout = child.stdout.take().expect("piped child stdout");
    let mut stderr = child.stderr.take().expect("piped child stderr");
    let stdout_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).expect("read child stdout");
        bytes
    });
    let stderr_reader = thread::spawn(move || {
        let mut bytes = Vec::new();
        stderr.read_to_end(&mut bytes).expect("read child stderr");
        bytes
    });
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let status = loop {
        if let Some(status) = child
            .try_wait()
            .unwrap_or_else(|error| panic!("wait for {description}: {error}"))
        {
            break status;
        }
        if Instant::now() >= deadline {
            #[cfg(windows)]
            let _ = Command::new("taskkill.exe")
                .args(["/PID", &child.id().to_string(), "/T", "/F"])
                .status();
            #[cfg(not(windows))]
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "{description} exceeded the {} second deadline",
                PROCESS_TIMEOUT.as_secs()
            );
        }
        thread::sleep(Duration::from_millis(25));
    };
    Output {
        status,
        stdout: stdout_reader.join().expect("join child stdout reader"),
        stderr: stderr_reader.join().expect("join child stderr reader"),
    }
}

fn isolated_jit_command(root: &str) -> Command {
    let test_executable = std::env::current_exe().expect("test executable");
    #[cfg(windows)]
    let mut command = {
        let mut command = Command::new("powershell.exe");
        command.args([
            "-NoProfile",
            "-Command",
            "Add-Type 'using System; using System.Runtime.InteropServices; public static class StasisTrapMode { [DllImport(\"kernel32.dll\")] public static extern uint SetErrorMode(uint mode); }'; [StasisTrapMode]::SetErrorMode(3) | Out-Null; & $env:STASIS_GENERICS_TRAP_CHILD_EXE --exact shared_generics_oracle_matches_jit_and_linked_native_aot_with_isolated_bounds_traps --nocapture; exit $LASTEXITCODE",
        ]);
        command.env(TRAP_CHILD_EXE, test_executable);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = Command::new(test_executable);
        command.args([
            "--exact",
            "shared_generics_oracle_matches_jit_and_linked_native_aot_with_isolated_bounds_traps",
            "--nocapture",
        ]);
        command
    };
    command.env(TRAP_CHILD_ROOT, root);
    command
}

fn is_native_bounds_trap(output: &Output) -> bool {
    #[cfg(windows)]
    {
        output.status.code() == Some(WINDOWS_ILLEGAL_INSTRUCTION)
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        output.status.signal() == Some(4)
    }
}

fn is_jit_bounds_trap(output: &Output) -> bool {
    #[cfg(windows)]
    {
        output.status.code().is_some_and(|code| {
            code == WINDOWS_ILLEGAL_INSTRUCTION || code == WINDOWS_ACCESS_VIOLATION
        })
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        output
            .status
            .signal()
            .is_some_and(|signal| signal == 4 || signal == 11)
    }
}

fn sha256(path: &Path) -> String {
    format!(
        "{:x}",
        Sha256::digest(fs::read(path).unwrap_or_else(|error| {
            panic!("read evidence artifact {}: {error}", path.display())
        }))
    )
}

fn write_evidence(
    output_dir: &Path,
    runtime_link: &Path,
    runtime: &Path,
    jit_results: [i32; 4],
    jit_traps: &[(&str, &Output)],
    executables: &[(&str, &Path)],
    outputs: &[(&str, &Output)],
) {
    let Some(evidence) = std::env::var_os(EVIDENCE_DIR).map(PathBuf::from) else {
        return;
    };
    fs::create_dir_all(&evidence).expect("create generics desktop evidence directory");
    for (_, executable) in executables {
        fs::copy(
            executable,
            evidence.join(executable.file_name().expect("evidence executable name")),
        )
        .expect("copy linked AOT evidence executable");
    }
    fs::copy(
        output_dir.join("stasis_provenance.json"),
        evidence.join("native-aot-provenance.json"),
    )
    .expect("copy native AOT provenance");
    let status = |output: &Output| {
        json!({
            "success": output.status.success(),
            "code": output.status.code(),
            "stdout": String::from_utf8_lossy(&output.stdout),
            "stderr": String::from_utf8_lossy(&output.stderr),
        })
    };
    let receipt = json!({
        "schema": "stasis.generics_desktop_execution.v1",
        "host": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH },
        "inputs": {
            "oracle": { "path": ORACLE_PATH, "sha256": format!("{:x}", Sha256::digest(source().as_bytes())) },
            "module": { "path": MODULE_PATH, "sha256": format!("{:x}", Sha256::digest(MODULE.as_bytes())) },
            "runtime_link_sha256": sha256(runtime_link),
            "runtime_sha256": sha256(runtime),
        },
        "expected_digest": EXPECTED_DIGEST,
        "jit": {
            "main": jit_results[0], "tick": jit_results[1],
            "render_0": jit_results[2], "render_2": jit_results[3],
            "low_trap": status(jit_traps[0].1), "high_trap": status(jit_traps[1].1),
        },
        "native_aot": {
            "oracle": status(outputs[0].1),
            "low_trap": status(outputs[1].1),
            "high_trap": status(outputs[2].1),
            "sha256": {
                "oracle": sha256(executables[0].1),
                "low_trap": sha256(executables[1].1),
                "high_trap": sha256(executables[2].1),
            }
        }
    });
    fs::write(
        evidence.join("execution-receipt.json"),
        serde_json::to_vec_pretty(&receipt).expect("serialize execution evidence"),
    )
    .expect("write execution evidence");
}

#[test]
fn shared_generics_oracle_matches_jit_and_linked_native_aot_with_isolated_bounds_traps() {
    if let Some(root) = std::env::var_os(TRAP_CHILD_ROOT) {
        let root = root.to_string_lossy().into_owned();
        let jit = configured_jit(&[root.clone()]);
        let _ = jit
            .execute_i32_noarg_by_name(&root)
            .expect("out-of-range generics JIT access must trap before returning");
        return;
    }
    if let Ok(expected_arch) = std::env::var("STASIS_EXPECTED_HOST_ARCH") {
        assert_eq!(
            std::env::consts::ARCH,
            expected_arch,
            "unexpected CI host architecture"
        );
    }

    let jit = configured_jit(&roots());
    let bounds_clif = jit
        .clif_for_function_name("oracle_read_bound")
        .expect("oracle_read_bound CLIF");
    for instruction in ["icmp_imm sge", "icmp ult", "trapz"] {
        assert!(
            bounds_clif.contains(instruction),
            "array-view bounds lowering omitted {instruction}:\n{bounds_clif}"
        );
    }
    let jit_main = jit
        .execute_i32_noarg_by_name("main")
        .expect("execute JIT main");
    let jit_tick = jit
        .execute_i32_noarg_by_name("tick")
        .expect("execute JIT tick");
    let jit_render_0 = jit
        .execute_i32_onearg_by_name("render", 0)
        .expect("execute JIT render(0)");
    let jit_render_2 = jit
        .execute_i32_onearg_by_name("render", 2)
        .expect("execute JIT render(2)");
    assert_eq!(jit_main, EXPECTED_DIGEST);
    assert_eq!(jit_tick, EXPECTED_DIGEST);
    assert_eq!(jit_render_0, 10);
    assert_eq!(jit_render_2, 10);
    let mut jit_traps = Vec::new();
    for root in [LOW_TRAP_ROOT, HIGH_TRAP_ROOT] {
        let output = run_bounded(
            isolated_jit_command(root),
            &format!("isolated JIT trap {root}"),
        );
        assert!(
            is_jit_bounds_trap(&output),
            "isolated JIT trap child {root} did not terminate with the native Cranelift bounds trap: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        jit_traps.push((root, output));
    }

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let tree = TestTree(target.join(format!(
        "generics-desktop-aot-{}-{stamp}",
        std::process::id()
    )));
    fs::create_dir_all(&tree.0).expect("create native AOT evidence directory");
    let (runtime_link, runtime) = dynload_artifacts();
    fs::copy(
        &runtime,
        tree.0
            .join(runtime.file_name().expect("native runtime name")),
    )
    .expect("copy native AOT runtime");
    fs::write(
        tree.0.join("stasis_provenance.json"),
        serde_json::to_vec_pretty(&json!({
            "schema": "stasis.generics_native_aot_provenance.v1",
            "oracle": ORACLE_PATH,
            "oracle_sha256": format!("{:x}", Sha256::digest(source().as_bytes())),
            "module": MODULE_PATH,
            "module_sha256": format!("{:x}", Sha256::digest(MODULE.as_bytes())),
            "expected_digest": EXPECTED_DIGEST,
            "host": { "os": std::env::consts::OS, "arch": std::env::consts::ARCH }
        }))
        .expect("serialize native AOT provenance"),
    )
    .expect("write native AOT provenance");

    let aot = configured_aot();
    let oracle = link_executable(&aot, STATUS_ROOT, &tree.0, &runtime_link);
    let low = link_executable(&aot, LOW_TRAP_ROOT, &tree.0, &runtime_link);
    let high = link_executable(&aot, HIGH_TRAP_ROOT, &tree.0, &runtime_link);
    let oracle_output = run(&oracle);
    assert!(
        oracle_output.status.success(),
        "linked native AOT oracle failed: status={} stdout={} stderr={}",
        oracle_output.status,
        String::from_utf8_lossy(&oracle_output.stdout),
        String::from_utf8_lossy(&oracle_output.stderr)
    );
    let low_output = run(&low);
    let high_output = run(&high);
    for (name, output) in [(LOW_TRAP_ROOT, &low_output), (HIGH_TRAP_ROOT, &high_output)] {
        assert!(
            is_native_bounds_trap(output),
            "linked native AOT trap child {name} did not terminate with the native Cranelift bounds trap: status={} stdout={} stderr={}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    write_evidence(
        &tree.0,
        &runtime_link,
        &runtime,
        [jit_main, jit_tick, jit_render_0, jit_render_2],
        &[
            (jit_traps[0].0, &jit_traps[0].1),
            (jit_traps[1].0, &jit_traps[1].1),
        ],
        &[
            (STATUS_ROOT, &oracle),
            (LOW_TRAP_ROOT, &low),
            (HIGH_TRAP_ROOT, &high),
        ],
        &[
            (STATUS_ROOT, &oracle_output),
            (LOW_TRAP_ROOT, &low_output),
            (HIGH_TRAP_ROOT, &high_output),
        ],
    );
}
