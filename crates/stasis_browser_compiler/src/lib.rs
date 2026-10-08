use std::cell::RefCell;
use std::collections::BTreeSet;

use serde::Deserialize;
use serde_json::{json, Map};
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;
use stasis_compiler::backend::{wasm::WasmProcess, ReachabilityPolicy};
use stasis_compiler::compiler::CompileError;
use stasis_compiler::frontend::types::{TYPE_ID_I32, TYPE_ID_VOID};

mod editor;

const ABI_VERSION: u32 = 1;
const FIXED_SAMPLE_PATH: &str = "memory/main.stasis";
const FIXED_SAMPLE_SOURCE: &str = "function main(): i32 { return 720; }";
const MAX_INPUT_JSON_BYTES: usize = 32 * 1024 * 1024;
const MAX_PROJECT_SOURCE_BYTES: usize = 4 * 1024 * 1024;
const MAX_SOURCE_FILE_BYTES: usize = 1024 * 1024;
const MAX_PROJECT_FILES: usize = 128;
const MAX_PROJECT_PATH_BYTES: usize = 1024;
const MAX_OUTPUT_MODULE_BYTES: usize = 32 * 1024 * 1024;
const MAX_OUTPUT_METADATA_BYTES: usize = 8 * 1024 * 1024;
const REQUIRED_LIFECYCLE_ROOTS: [&str; 4] = ["main", "tick", "render", "on_code_swap"];

#[derive(Default)]
struct CompileState {
    input: Vec<u8>,
    output: Vec<u8>,
    metadata: Vec<u8>,
    error: Vec<u8>,
    editor_input: Vec<u8>,
    editor_output: Vec<u8>,
    editor_error: Vec<u8>,
    last_good_editor_catalog: Option<editor::EditorCatalog>,
}

thread_local! {
    static COMPILE_STATE: RefCell<CompileState> = RefCell::new(CompileState::default());
}

#[derive(Debug)]
struct CompiledArtifact {
    module: Vec<u8>,
    metadata: Vec<u8>,
    editor_catalog: Option<editor::EditorCatalog>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompileProjectRequest {
    entry: String,
    files: Vec<CompileProjectFile>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CompileProjectFile {
    path: String,
    source: String,
}

fn compile_fixed_sample_bytes() -> Result<Vec<u8>, String> {
    let mut process = WasmProcess::new();
    process.set_reachability_policy(ReachabilityPolicy::Release);
    process.upsert_file(FIXED_SAMPLE_PATH, FIXED_SAMPLE_SOURCE);
    process.compile().map_err(|error| format!("{error:?}"))?;
    Ok(process.module_bytes().to_vec())
}

fn validate_project_path(path: &str) -> Result<(), String> {
    if path.is_empty() || path.len() > MAX_PROJECT_PATH_BYTES {
        return Err(format!(
            "project path must contain 1..={MAX_PROJECT_PATH_BYTES} UTF-8 bytes"
        ));
    }
    if !path.ends_with(".stasis") {
        return Err(format!("project path '{path}' must end in .stasis"));
    }
    if path.starts_with('/') || path.contains('\\') || path.contains(':') {
        return Err(format!(
            "project path '{path}' must be a relative path using '/' separators"
        ));
    }
    if path.chars().any(char::is_control) {
        return Err(format!(
            "project path '{path}' contains a control character"
        ));
    }
    if path
        .split('/')
        .any(|component| component.is_empty() || component == "." || component == "..")
    {
        return Err(format!(
            "project path '{path}' must not contain empty, '.' or '..' components"
        ));
    }
    Ok(())
}

fn validate_lifecycle(snapshot: &ProgramSnapshot) -> Result<bool, String> {
    for (name, expected_result) in [
        ("main", TYPE_ID_I32),
        ("tick", TYPE_ID_I32),
        ("render", TYPE_ID_I32),
    ] {
        let mut matches = snapshot
            .functions()
            .iter()
            .filter(|function| function.name == name);
        let function = matches
            .next()
            .ok_or_else(|| format!("project must define {name}(): i32"))?;
        if matches.next().is_some() {
            return Err(format!(
                "project must define exactly one {name} lifecycle function"
            ));
        }
        if !function.params.is_empty() || function.return_type != expected_result {
            return Err(format!("invalid {name} signature; expected {name}(): i32"));
        }
    }

    let mut hooks = snapshot
        .functions()
        .iter()
        .filter(|function| function.name == "on_code_swap");
    let Some(hook) = hooks.next() else {
        return Ok(false);
    };
    if hooks.next().is_some() {
        return Err("project must define at most one on_code_swap hook".to_string());
    }
    if !hook.params.is_empty() || hook.return_type != TYPE_ID_VOID {
        return Err("invalid on_code_swap signature; expected on_code_swap(): void".to_string());
    }
    Ok(true)
}

fn compile_diagnostic(process: &WasmProcess, error: CompileError) -> String {
    if let Some(diagnostic) = process.last_source_diagnostic() {
        return format!(
            "{}:{}..{}: {}",
            diagnostic.path, diagnostic.start, diagnostic.end, diagnostic.message
        );
    }
    format!("{error:?}")
}

fn validate_output_size(name: &str, length: usize, maximum: usize) -> Result<(), String> {
    if length > maximum {
        return Err(format!("{name} exceeds the {maximum}-byte output limit"));
    }
    Ok(())
}

fn compile_project_json(input: &[u8]) -> Result<CompiledArtifact, String> {
    if input.is_empty() || input.len() > MAX_INPUT_JSON_BYTES {
        return Err(format!(
            "compile-project JSON must contain 1..={MAX_INPUT_JSON_BYTES} bytes"
        ));
    }
    let text =
        std::str::from_utf8(input).map_err(|error| format!("request is not UTF-8: {error}"))?;
    let request: CompileProjectRequest = serde_json::from_str(text)
        .map_err(|error| format!("invalid compile-project request: {error}"))?;
    validate_project_path(&request.entry)?;
    if request.files.is_empty() || request.files.len() > MAX_PROJECT_FILES {
        return Err(format!(
            "compile-project must include 1..={MAX_PROJECT_FILES} source files"
        ));
    }

    let mut paths = BTreeSet::new();
    let mut total_source_bytes = 0usize;
    for file in &request.files {
        validate_project_path(&file.path)?;
        if !paths.insert(file.path.as_str()) {
            return Err(format!("duplicate project path '{}'", file.path));
        }
        if file.source.len() > MAX_SOURCE_FILE_BYTES {
            return Err(format!(
                "source file '{}' exceeds the {MAX_SOURCE_FILE_BYTES}-byte per-file limit",
                file.path
            ));
        }
        total_source_bytes = total_source_bytes
            .checked_add(file.source.len())
            .ok_or_else(|| "project source byte count overflowed".to_string())?;
        if total_source_bytes > MAX_PROJECT_SOURCE_BYTES {
            return Err(format!(
                "project sources exceed the {MAX_PROJECT_SOURCE_BYTES}-byte total limit"
            ));
        }
    }
    if !paths.contains(request.entry.as_str()) {
        return Err(format!(
            "entry '{}' must name one of the supplied project files",
            request.entry
        ));
    }

    let editor_files = request
        .files
        .iter()
        .map(|file| editor::EditorSourceFile {
            path: file.path.clone(),
            source: file.source.clone(),
        })
        .collect::<Vec<_>>();

    let mut process = WasmProcess::new();
    process.set_reachability_policy(ReachabilityPolicy::Development);
    let roots = REQUIRED_LIFECYCLE_ROOTS.map(str::to_string);
    process.set_required_emit_roots(&roots);
    for file in request.files {
        process.upsert_file(file.path, file.source);
    }
    if let Err(error) = process.compile() {
        return Err(compile_diagnostic(&process, error));
    }
    validate_output_size(
        "game WebAssembly module",
        process.module_bytes().len(),
        MAX_OUTPUT_MODULE_BYTES,
    )?;
    let snapshot = process
        .program_snapshot()
        .ok_or_else(|| "compiler completed without a semantic program snapshot".to_string())?;
    let hook_present = validate_lifecycle(snapshot)?;
    let snapshot_supported = process.replay_state_snapshot_supported();
    let mut replay_compatibility = snapshot.replay_compatibility();
    if snapshot_supported {
        replay_compatibility.state_snapshot.support = "canonical_bytes".to_string();
    }

    let string_literal_table = process
        .string_literal_metadata()
        .iter()
        .map(|(id, metadata)| {
            (
                id.to_string(),
                json!([metadata.offset, metadata.byte_length]),
            )
        })
        .collect::<Map<_, _>>();
    let memory = process
        .memory_layout()
        .iter()
        .map(|(path, layout)| {
            (
                path.clone(),
                json!({
                    "hash": stasis_compiler::backend::wasm::wasm_global_hash(path),
                    "handle": layout.handle,
                    "offset": layout.offset,
                    "type_id": layout.type_id,
                    "length": layout.length,
                    "stride": layout.stride,
                    "byte_backed": layout.byte_backed,
                }),
            )
        })
        .collect::<Map<_, _>>();
    let views = process
        .struct_views()
        .iter()
        .map(|(base, fields)| (base.to_string(), json!(fields)))
        .collect::<Map<_, _>>();
    let globals = process
        .global_types()
        .iter()
        .map(|(path, type_id)| {
            (
                path.clone(),
                json!({
                    "hash": stasis_compiler::backend::wasm::wasm_global_hash(path),
                    "type_id": type_id,
                }),
            )
        })
        .collect::<Map<_, _>>();
    let render_construction_lifecycle_version = snapshot
        .functions()
        .iter()
        .any(|function| function.name == "gfx_cmd_construction_reset")
        && snapshot
            .functions()
            .iter()
            .any(|function| function.name == "gfx_cmd_construction_finish");
    let render_construction_lifecycle_version = u8::from(render_construction_lifecycle_version);
    let host_exports = process.host_exports();
    host_exports
        .validate()
        .map_err(|error| format!("invalid host export metadata: {error}"))?;
    let imports = process
        .imported_symbols()
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    let state_layout = serde_json::to_value(snapshot.state_layout())
        .map_err(|error| format!("could not serialize state layout: {error}"))?;
    let layout_digest = snapshot
        .layout_digest()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let entrypoints = json!({
        "main": {"parameters": [], "returnType": "i32"},
        "tick": {"parameters": [], "returnType": "i32"},
        "render": {"parameters": [], "returnType": "i32"},
        "on_code_swap": if hook_present {
            Some(json!({"parameters": [], "returnType": "void"}))
        } else {
            None
        },
    });
    let config = json!({
        "name": "Browser Project",
        "strings": {},
        "stringLiteralTable": string_literal_table,
        "stringLiteralTableVersion": stasis_compiler::backend::wasm::STRING_LITERAL_TABLE_VERSION,
        "memory": memory,
        "views": views,
        "globals": globals,
        "assets": {},
        "collectionViewAbiVersion": stasis_compiler::backend::wasm::COLLECTION_VIEW_ABI_VERSION,
        "host_exports": host_exports,
        "renderContractVersion": if render_construction_lifecycle_version == 1 { 8 } else { 7 },
        "renderConstructionLifecycleVersion": render_construction_lifecycle_version,
        "spriteAtlasPageSize": 512,
        "replayCompatibility": replay_compatibility,
    });
    let metadata = json!({
        "schemaVersion": 1,
        "entry": request.entry,
        "config": config,
        "imports": imports,
        "entrypoints": entrypoints,
        "layoutDigest": layout_digest,
        "stateLayout": state_layout,
        "snapshotSupported": snapshot_supported,
        "replayCompatibility": replay_compatibility,
        "hookPresent": hook_present,
        "hook": if hook_present {
            Some(json!({"name": "on_code_swap", "parameters": [], "returnType": "void"}))
        } else {
            None
        },
        "provenance": {
            "compiler": "stasis_browser_compiler",
            "compilerVersion": env!("CARGO_PKG_VERSION"),
            "backend": "stasis_compiler",
            "backendVersion": env!("CARGO_PKG_VERSION"),
            "abiVersion": ABI_VERSION,
            "target": std::env::consts::ARCH,
            "profile": if cfg!(debug_assertions) { "debug" } else { "release" },
            "buildIdentity": format!(
                "stasis_browser_compiler@{};stasis_compiler@{};target={};profile={};abi={ABI_VERSION}",
                env!("CARGO_PKG_VERSION"),
                env!("CARGO_PKG_VERSION"),
                std::env::consts::ARCH,
                if cfg!(debug_assertions) { "debug" } else { "release" },
            ),
        },
    });
    let metadata = serde_json::to_vec(&metadata)
        .map_err(|error| format!("could not serialize compiler metadata: {error}"))?;
    validate_output_size(
        "compile metadata",
        metadata.len(),
        MAX_OUTPUT_METADATA_BYTES,
    )?;
    Ok(CompiledArtifact {
        module: process.module_bytes().to_vec(),
        metadata,
        editor_catalog: Some(editor::EditorCatalog::from_sources(&editor_files)),
    })
}

fn clear_outputs(state: &mut CompileState) {
    state.output.clear();
    state.metadata.clear();
    state.error.clear();
}

fn store_fixed_sample_result(result: Result<Vec<u8>, String>) -> i32 {
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        clear_outputs(&mut state);
        match result {
            Ok(output) => {
                state.output = output;
                0
            }
            Err(error) => {
                state.error = error.into_bytes();
                1
            }
        }
    })
}

fn store_project_result(result: Result<CompiledArtifact, String>) -> i32 {
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        clear_outputs(&mut state);
        state.input.clear();
        match result {
            Ok(artifact) => {
                state.output = artifact.module;
                state.metadata = artifact.metadata;
                state.last_good_editor_catalog = artifact.editor_catalog;
                0
            }
            Err(error) => {
                state.error = error.into_bytes();
                1
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_browser_compiler_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_fixed_sample() -> i32 {
    store_fixed_sample_result(compile_fixed_sample_bytes())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_project_input_resize(length: u32) -> i32 {
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        clear_outputs(&mut state);
        state.input.clear();
        let length = length as usize;
        if length == 0 || length > MAX_INPUT_JSON_BYTES {
            state.error =
                format!("compile-project request must contain 1..={MAX_INPUT_JSON_BYTES} bytes")
                    .into_bytes();
            return 1;
        }
        if state.input.try_reserve_exact(length).is_err() {
            state.error = "could not reserve compile-project input buffer"
                .to_string()
                .into_bytes();
            return 1;
        }
        state.input.resize(length, 0);
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_project_input_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().input.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_project() -> i32 {
    let result = COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        clear_outputs(&mut state);
        let result = compile_project_json(&state.input);
        state.input.clear();
        result
    });
    store_project_result(result)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_input_resize(length: u32) -> i32 {
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.editor_output.clear();
        state.editor_error.clear();
        state.editor_input.clear();
        let length = length as usize;
        if length == 0 || length > editor::MAX_EDITOR_JSON_BYTES {
            state.editor_error = format!(
                "analyze-editor request must contain 1..={} bytes",
                editor::MAX_EDITOR_JSON_BYTES
            )
            .into_bytes();
            return 1;
        }
        if state.editor_input.try_reserve_exact(length).is_err() {
            state.editor_error = b"could not reserve analyze-editor input buffer".to_vec();
            return 1;
        }
        state.editor_input.resize(length, 0);
        0
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_input_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().editor_input.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor() -> i32 {
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.editor_output.clear();
        state.editor_error.clear();
        let result = editor::analyze_editor_json(
            &state.editor_input,
            state.last_good_editor_catalog.as_ref(),
        );
        state.editor_input.clear();
        match result {
            Ok(output) => {
                state.editor_output = output;
                0
            }
            Err(error) => {
                state.editor_error = error.into_bytes();
                1
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_output_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().editor_output.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_output_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().editor_output.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_error_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().editor_error.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_analyze_editor_error_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().editor_error.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_output_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().output.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_output_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().output.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_metadata_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().metadata.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_metadata_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().metadata.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_error_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().error.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_error_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().error.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn project_json(entry_source: &str, additional_files: &[(&str, &str)]) -> Vec<u8> {
        let mut files = vec![json!({ "path": "src/main.stasis", "source": entry_source })];
        files.extend(
            additional_files
                .iter()
                .map(|(path, source)| json!({ "path": path, "source": source })),
        );
        serde_json::to_vec(&json!({ "entry": "src/main.stasis", "files": files }))
            .expect("serialize test request")
    }

    fn valid_project() -> Vec<u8> {
        project_json(
            "import \"helper.stasis\"; global score: i32; function main(): i32 { score = helper(); return score; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[("src/helper.stasis", "function helper(): i32 { return 720; }")],
        )
    }

    #[test]
    fn fixed_sample_compilation_is_deterministic_webassembly() {
        let first = compile_fixed_sample_bytes().expect("compile fixed browser sample");
        let second = compile_fixed_sample_bytes().expect("repeat fixed browser sample");
        assert_eq!(&first[..8], b"\0asm\x01\0\0\0");
        assert_eq!(first, second);
    }

    #[test]
    fn project_compilation_resolves_multiple_in_memory_files_and_emits_metadata() {
        let artifact = compile_project_json(&valid_project()).expect("compile project");
        assert_eq!(&artifact.module[..8], b"\0asm\x01\0\0\0");
        assert!(artifact.module.len() <= MAX_OUTPUT_MODULE_BYTES);
        assert!(artifact.metadata.len() <= MAX_OUTPUT_METADATA_BYTES);
        let metadata: Value = serde_json::from_slice(&artifact.metadata).expect("metadata JSON");
        assert_eq!(metadata["schemaVersion"], 1);
        assert_eq!(metadata["entry"], "src/main.stasis");
        assert_eq!(metadata["entrypoints"]["main"]["returnType"], "i32");
        assert_eq!(metadata["hookPresent"], false);
        assert_eq!(metadata["imports"], json!([]));
        assert_eq!(metadata["config"]["collectionViewAbiVersion"], 2);
        assert_eq!(metadata["config"]["stringLiteralTableVersion"], 1);
        assert_eq!(metadata["stateLayout"]["scalars"][0]["path"], "score");
        assert!(metadata["config"]["memory"].is_object());
        assert_eq!(
            metadata["config"]["globals"]["score"]["type_id"],
            TYPE_ID_I32
        );
        assert_eq!(metadata["layoutDigest"].as_str().unwrap().len(), 64);
        assert!(metadata["provenance"]["buildIdentity"].is_string());

        let changed = project_json(
            "import \"helper.stasis\"; function main(): i32 { return helper(); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[("src/helper.stasis", "function helper(): i32 { return 721; }")],
        );
        assert_ne!(
            artifact.module,
            compile_project_json(&changed).unwrap().module
        );
    }

    #[test]
    fn compiler_output_size_limits_accept_the_boundary_and_reject_overflow() {
        assert!(validate_output_size(
            "game WebAssembly module",
            MAX_OUTPUT_MODULE_BYTES,
            MAX_OUTPUT_MODULE_BYTES
        )
        .is_ok());
        assert!(validate_output_size(
            "game WebAssembly module",
            MAX_OUTPUT_MODULE_BYTES + 1,
            MAX_OUTPUT_MODULE_BYTES
        )
        .unwrap_err()
        .contains("game WebAssembly module"));

        assert!(validate_output_size(
            "compile metadata",
            MAX_OUTPUT_METADATA_BYTES,
            MAX_OUTPUT_METADATA_BYTES
        )
        .is_ok());
        assert!(validate_output_size(
            "compile metadata",
            MAX_OUTPUT_METADATA_BYTES + 1,
            MAX_OUTPUT_METADATA_BYTES
        )
        .unwrap_err()
        .contains("compile metadata"));
    }

    #[test]
    fn project_imports_are_reported_for_runtime_allowlisting() {
        let source = project_json(
            "import \"missing_symbol.stasis\"; function main(): i32 { return external_value(); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[],
        );
        let error = compile_project_json(&source).expect_err("missing module is rejected");
        assert!(error.contains("missing_symbol.stasis"), "{error}");
    }

    #[test]
    fn project_metadata_lists_the_exact_host_import_allowlist() {
        let request = project_json(
            "extern function print_i32(value: i32): void; function main(): i32 { print_i32(1); return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[],
        );
        let artifact = compile_project_json(&request).expect("compile imported host call");
        let metadata: Value = serde_json::from_slice(&artifact.metadata).expect("metadata JSON");
        assert_eq!(metadata["imports"], json!(["print_i32"]));
    }

    #[test]
    fn project_rejects_duplicate_paths_and_entry_mismatch() {
        let duplicate = project_json(
            "function main(): i32 { return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[("src/main.stasis", "function helper(): i32 { return 1; }")],
        );
        assert!(compile_project_json(&duplicate)
            .unwrap_err()
            .contains("duplicate project path"));

        let mismatched: Value = serde_json::from_slice(&valid_project()).unwrap();
        let mismatched = json!({ "entry": "helper.stasis", "files": mismatched["files"] });
        assert!(
            compile_project_json(&serde_json::to_vec(&mismatched).unwrap())
                .unwrap_err()
                .contains("entry 'helper.stasis'")
        );
    }

    #[test]
    fn project_rejects_unnormalized_or_oversized_paths_and_sources() {
        for path in [
            "../main.stasis",
            "./main.stasis",
            "folder//main.stasis",
            "C:/main.stasis",
            "folder\\main.stasis",
            "/main.stasis",
            "main.txt",
        ] {
            let request = json!({
                "entry": path,
                "files": [{ "path": path, "source": "" }]
            });
            assert!(
                compile_project_json(&serde_json::to_vec(&request).unwrap()).is_err(),
                "{path}"
            );
        }

        let long_path = format!("{}.stasis", "a".repeat(MAX_PROJECT_PATH_BYTES));
        let request = json!({
            "entry": long_path,
            "files": []
        });
        assert!(compile_project_json(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .contains("path must contain"));

        let large = "x".repeat(MAX_SOURCE_FILE_BYTES + 1);
        let request = json!({
            "entry": "main.stasis",
            "files": [{ "path": "main.stasis", "source": large }]
        });
        assert!(compile_project_json(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .contains("per-file limit"));

        let too_many_files = (0..=MAX_PROJECT_FILES)
            .map(|index| json!({ "path": format!("file-{index}.stasis"), "source": "" }))
            .collect::<Vec<_>>();
        let request = json!({ "entry": "file-0.stasis", "files": too_many_files });
        assert!(compile_project_json(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .contains("source files"));

        let source = "x".repeat(MAX_SOURCE_FILE_BYTES);
        let files = (0..5)
            .map(|index| json!({ "path": format!("file-{index}.stasis"), "source": source }))
            .collect::<Vec<_>>();
        let request = json!({ "entry": "file-0.stasis", "files": files });
        assert!(compile_project_json(&serde_json::to_vec(&request).unwrap())
            .unwrap_err()
            .contains("total limit"));
    }

    #[test]
    fn project_rejects_missing_or_invalid_lifecycle_signatures_and_hook() {
        let missing = project_json(
            "function main(): i32 { return 0; } function tick(): i32 { return 0; }",
            &[],
        );
        assert!(compile_project_json(&missing)
            .unwrap_err()
            .contains("render"));

        let bad_tick = project_json(
            "function main(): i32 { return 0; } function tick(value: i32): i32 { return value; } function render(): i32 { return 0; }",
            &[],
        );
        assert!(compile_project_json(&bad_tick)
            .unwrap_err()
            .contains("tick"));

        let bad_hook = project_json(
            "function main(): i32 { return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; } function on_code_swap(value: i32): i32 { return value; }",
            &[],
        );
        assert!(compile_project_json(&bad_hook)
            .unwrap_err()
            .contains("expected on_code_swap(): void"));

        let bad_hook_result = project_json(
            "function main(): i32 { return 0; } function tick(): i32 { return 0; } function render(): i32 { return 0; } function on_code_swap(): i32 { return 0; }",
            &[],
        );
        assert!(compile_project_json(&bad_hook_result)
            .unwrap_err()
            .contains("expected on_code_swap(): void"));
    }

    #[test]
    fn project_compile_errors_include_source_diagnostics() {
        let request = project_json(
            "function main(): i32 { return missing_value(); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[],
        );
        let error = compile_project_json(&request).expect_err("bad source is rejected");
        assert!(error.contains("src/main.stasis"), "{error}");
    }

    #[test]
    fn failed_c_abi_compile_clears_previous_success_outputs() {
        let valid = valid_project();
        assert_eq!(stasis_compile_project_input_resize(valid.len() as u32), 0);
        assert!(stasis_compile_project_input_ptr() > 0);
        COMPILE_STATE.with(|state| state.borrow_mut().input.copy_from_slice(&valid));
        assert_eq!(stasis_compile_project(), 0);
        assert!(stasis_compile_output_len() > 8);
        assert!(stasis_compile_metadata_len() > 0);

        let invalid = project_json(
            "function main(): i32 { return missing(); } function tick(): i32 { return 0; } function render(): i32 { return 0; }",
            &[],
        );
        assert_eq!(stasis_compile_project_input_resize(invalid.len() as u32), 0);
        COMPILE_STATE.with(|state| state.borrow_mut().input.copy_from_slice(&invalid));
        assert_eq!(stasis_compile_project(), 1);
        assert_eq!(stasis_compile_output_len(), 0);
        assert_eq!(stasis_compile_metadata_len(), 0);
        assert!(stasis_compile_error_len() > 0);

        assert_eq!(
            stasis_compile_project_input_resize((MAX_INPUT_JSON_BYTES + 1) as u32),
            1
        );
        assert_eq!(stasis_compile_output_len(), 0);
        assert_eq!(stasis_compile_metadata_len(), 0);
        assert!(stasis_compile_error_len() > 0);
    }

    #[test]
    fn editor_analysis_errors_do_not_clear_compiled_game_outputs() {
        let project = valid_project();
        assert_eq!(stasis_compile_project_input_resize(project.len() as u32), 0);
        COMPILE_STATE.with(|state| state.borrow_mut().input.copy_from_slice(&project));
        assert_eq!(stasis_compile_project(), 0);
        let (compiled_module, compiled_metadata) = COMPILE_STATE.with(|state| {
            let state = state.borrow();
            (state.output.clone(), state.metadata.clone())
        });
        assert!(!compiled_module.is_empty());
        assert!(!compiled_metadata.is_empty());

        let good_request = serde_json::to_vec(&json!({
            "path": "src/main.stasis",
            "source": "function main(): i32 { return 0; }",
            "files": [],
            "cursor": 10,
        }))
        .expect("serialize editor request");
        assert_eq!(
            stasis_analyze_editor_input_resize(good_request.len() as u32),
            0
        );
        COMPILE_STATE.with(|state| {
            state
                .borrow_mut()
                .editor_input
                .copy_from_slice(&good_request)
        });
        assert_eq!(stasis_analyze_editor(), 0);
        assert!(stasis_analyze_editor_output_len() > 0);

        let bad_request = serde_json::to_vec(&json!({
            "path": "../bad.stasis",
            "source": "",
            "files": [],
        }))
        .expect("serialize bad editor request");
        assert_eq!(
            stasis_analyze_editor_input_resize(bad_request.len() as u32),
            0
        );
        COMPILE_STATE.with(|state| {
            state
                .borrow_mut()
                .editor_input
                .copy_from_slice(&bad_request)
        });
        assert_eq!(stasis_analyze_editor(), 1);
        assert_eq!(stasis_analyze_editor_output_len(), 0);
        assert!(stasis_analyze_editor_error_len() > 0);
        assert_eq!(stasis_compile_error_len(), 0);
        COMPILE_STATE.with(|state| {
            let state = state.borrow();
            assert_eq!(state.output, compiled_module);
            assert_eq!(state.metadata, compiled_metadata);
        });
    }
}
