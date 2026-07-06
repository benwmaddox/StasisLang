use std::ffi::{c_char, CStr, CString};
use std::fs;
use std::path::{Path, PathBuf};

use stasis_compiler::frontend::workshop::{
    build_android_workshop_compile_plan, load_workshop_project, render_android_workshop_artifacts,
    AndroidWorkshopCompilePlan, AndroidWorkshopReload,
};
use stasis_compiler::IncrementalCompilerHost;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AndroidBridgeCompileResult {
    pub status: i32,
    pub reload: AndroidWorkshopReload,
    pub manifest_path: PathBuf,
    pub runtime_state_path: PathBuf,
    pub function_artifact_count: usize,
}

pub fn compile_android_workshop_project(
    project_root: impl AsRef<Path>,
    entry_file: impl AsRef<Path>,
) -> Result<AndroidBridgeCompileResult, String> {
    let project_root = project_root.as_ref();
    let entry_file = entry_file.as_ref();
    let files = load_workshop_project(project_root, entry_file)?;
    let changed_files = files
        .iter()
        .map(|file| project_root.join(&file.path))
        .collect::<Vec<_>>();

    let mut host = IncrementalCompilerHost::new();
    host.set_required_reachability_roots(&["tick", "render", "on_code_swap"]);
    let compile = host.compile_changed_files(&changed_files)?;
    let previous = read_previous_android_plan(project_root)?;
    let plan = build_android_workshop_compile_plan(&files, &compile, previous.as_ref())?;
    let artifacts = render_android_workshop_artifacts(&plan);

    let manifest_path = project_root.join(&artifacts.manifest_path);
    if let Some(parent) = manifest_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed creating Android manifest directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(&manifest_path, artifacts.manifest.as_bytes()).map_err(|error| {
        format!(
            "failed writing Android manifest {}: {error}",
            manifest_path.display()
        )
    })?;

    let runtime_state_path = project_root.join(&artifacts.runtime_state_path);
    if let Some(runtime_state) = artifacts.runtime_state.as_deref() {
        if let Some(parent) = runtime_state_path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed creating Android runtime state directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&runtime_state_path, runtime_state.as_bytes()).map_err(|error| {
            format!(
                "failed writing Android runtime state {}: {error}",
                runtime_state_path.display()
            )
        })?;
    }

    for artifact in &artifacts.function_artifacts {
        let path = project_root.join(&artifact.path);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "failed creating Android function artifact directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        fs::write(&path, artifact.source.as_bytes()).map_err(|error| {
            format!(
                "failed writing Android function artifact {}: {error}",
                path.display()
            )
        })?;
    }

    Ok(AndroidBridgeCompileResult {
        status: plan.status,
        reload: plan.reload,
        manifest_path,
        runtime_state_path,
        function_artifact_count: artifacts.function_artifacts.len(),
    })
}

fn read_previous_android_plan(
    project_root: &Path,
) -> Result<Option<AndroidWorkshopCompilePlan>, String> {
    let manifest_path = project_root.join("build/native_compile_manifest.txt");
    let manifest = match fs::read_to_string(&manifest_path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(format!(
                "failed reading previous Android manifest {}: {error}",
                manifest_path.display()
            ));
        }
    };
    let Some(project_hash) = parse_manifest_hex_i32(&manifest, "project_hash=") else {
        return Ok(None);
    };
    let Some(layout_hash) = parse_manifest_hex_i32(&manifest, "layout_hash=") else {
        return Ok(None);
    };
    Ok(Some(AndroidWorkshopCompilePlan {
        status: 0,
        reload: AndroidWorkshopReload::NoChange,
        reason: "Loaded from previous Android manifest.".to_string(),
        project_hash,
        layout_hash,
        entrypoints: Vec::new(),
        functions: Vec::new(),
        errors: Vec::new(),
    }))
}

fn parse_manifest_hex_i32(manifest: &str, key: &str) -> Option<i32> {
    let line = manifest.lines().find(|line| line.starts_with(key))?;
    let value = line[key.len()..].trim();
    u32::from_str_radix(value, 16)
        .ok()
        .map(|value| value as i32)
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_compile_project(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> *mut c_char {
    let result = unsafe { compile_project_from_c(project_root, entry_file) };
    let message = match result {
        Ok(result) => format!(
            "CompilePlanned: reload={:?} status={} functions={} manifest={}",
            result.reload,
            result.status,
            result.function_artifact_count,
            result.manifest_path.display()
        ),
        Err(error) => format!("CompileError: {error}"),
    };
    CString::new(message)
        .unwrap_or_else(|_| CString::new("CompileError: invalid bridge message").unwrap())
        .into_raw()
}

unsafe fn compile_project_from_c(
    project_root: *const c_char,
    entry_file: *const c_char,
) -> Result<AndroidBridgeCompileResult, String> {
    if project_root.is_null() || entry_file.is_null() {
        return Err("null project root or entry file".to_string());
    }
    let project_root = CStr::from_ptr(project_root)
        .to_str()
        .map_err(|error| format!("project root was not UTF-8: {error}"))?;
    let entry_file = CStr::from_ptr(entry_file)
        .to_str()
        .map_err(|error| format!("entry file was not UTF-8: {error}"))?;
    compile_android_workshop_project(project_root, entry_file)
}

#[no_mangle]
pub extern "C" fn stasis_android_bridge_free_string(value: *mut c_char) {
    if value.is_null() {
        return;
    }
    unsafe {
        drop(CString::from_raw(value));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_project(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_android_bridge_{name}_{stamp}"));
        fs::create_dir_all(root.join("src")).expect("create project");
        root
    }

    #[test]
    fn bridge_compiles_project_and_writes_compiler_rendered_artifacts() {
        let root = temp_project("compile");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 7; }\n",
        )
        .expect("write source");

        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("compile bridge");
        assert_eq!(result.status, 0);
        assert_eq!(result.reload, AndroidWorkshopReload::InitialCompile);
        assert!(result.function_artifact_count >= 2);

        let manifest = fs::read_to_string(root.join("build/native_compile_manifest.txt"))
            .expect("read manifest");
        assert!(manifest.contains("status=CompilePlanned"));
        assert!(manifest.contains("entrypoint=main"));
        assert!(manifest.contains("entrypoint=tick"));
        assert!(manifest.contains("signature=tick(): i32"));

        let state =
            fs::read_to_string(root.join("build/runtime_state.txt")).expect("read runtime state");
        assert!(state.contains("status=RuntimeStateReady"));
        assert!(state.contains("tick_count=0"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn bridge_preserves_runtime_state_for_fast_reload() {
        let root = temp_project("reload");
        let source = root.join("src/main.stasis");
        fs::write(
            &source,
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 1; }\n",
        )
        .expect("write first source");
        compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("first compile");
        fs::write(
            root.join("build/runtime_state.txt"),
            "status=RuntimeStateReady\ntick_count=41\n",
        )
        .expect("seed runtime state");

        fs::write(
            &source,
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 2; }\n",
        )
        .expect("write body change");
        let result = compile_android_workshop_project(&root, Path::new("src/main.stasis"))
            .expect("second compile");
        assert_eq!(result.reload, AndroidWorkshopReload::FastReload);
        let state = fs::read_to_string(root.join("build/runtime_state.txt"))
            .expect("read preserved runtime state");
        assert!(state.contains("tick_count=41"));
        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn c_bridge_returns_compile_message() {
        let root = temp_project("ffi");
        fs::write(
            root.join("src/main.stasis"),
            "function main(): i32 { return tick(); }\nfunction tick(): i32 { return 3; }\n",
        )
        .expect("write source");
        let root_c = CString::new(root.to_string_lossy().as_bytes()).expect("root cstr");
        let entry_c = CString::new("src/main.stasis").expect("entry cstr");
        let ptr = stasis_android_bridge_compile_project(root_c.as_ptr(), entry_c.as_ptr());
        assert!(!ptr.is_null());
        let message = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .expect("message utf8")
            .to_string();
        stasis_android_bridge_free_string(ptr);
        assert!(message.contains("CompilePlanned"));
        assert!(message.contains("functions="));
        fs::remove_dir_all(&root).ok();
    }
}
