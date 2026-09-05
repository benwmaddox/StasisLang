//! Deterministic cross-target characterization for the compiler boundary.
//!
//! This test intentionally records semantic and structural facts instead of raw
//! machine/module bytes.  The checked-in JSON is the review point for changes
//! to parser, reachability, lowering, and target emission.

use object::{Object, ObjectSymbol};
use serde_json::{json, Value};
use stasis_compiler::backend::aot::AotProcess;
use stasis_compiler::backend::jit::JitProcess;
use stasis_compiler::backend::patch_plan::{
    capture_accepted_program, plan_patch, FunctionKey, PatchReason, PatchReasonChain,
};
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;
use stasis_compiler::backend::wasm::WasmProcess;
use stasis_compiler::compiler::Compiler;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

const FIXTURE_PATH: &str = "tests/stasis/characterization/compiler_pipeline_v1.stasis";
const FIXTURE_RAW: &str =
    include_str!("../../../tests/stasis/characterization/compiler_pipeline_v1.stasis");
const ROOTS: [&str; 4] = ["main", "tick", "render", "on_code_swap"];

fn fixture() -> String {
    // .stasis files are checked out with the repository's CRLF policy on
    // Windows. Normalize before compiling so source ranges and hashes remain
    // identical in every characterization lane.
    FIXTURE_RAW.replace("\r\n", "\n").replace('\r', "\n")
}

fn roots() -> Vec<String> {
    ROOTS.iter().map(|root| (*root).to_string()).collect()
}

fn hex(bytes: impl AsRef<[u8]>) -> String {
    bytes
        .as_ref()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn key_json(key: &FunctionKey) -> Value {
    json!({"name": key.name, "symbol_id": key.symbol_id.canonical()})
}

fn reason_json(reason: &PatchReason) -> Value {
    match reason {
        PatchReason::ColdStart => json!("cold_start"),
        PatchReason::BodyChanged => json!("body_changed"),
        PatchReason::AddedOrSignatureChanged => json!("added_or_signature_changed"),
        PatchReason::BecameReachable => json!("became_reachable"),
        PatchReason::CompilerLayoutChanged => json!("compiler_layout_changed"),
        PatchReason::LoweredContractChanged => json!("lowered_contract_changed"),
        PatchReason::SccPeer { changed } => {
            json!({"kind": "scc_peer", "changed": key_json(changed)})
        }
        PatchReason::DirectCaller { callee } => {
            json!({"kind": "direct_caller", "callee": key_json(callee)})
        }
    }
}

fn reason_chain_json(chain: &PatchReasonChain) -> Value {
    json!({
        "function": key_json(&chain.function),
        "reason": reason_json(&chain.reason),
        "path_from_change": chain.path_from_change.iter().map(key_json).collect::<Vec<_>>(),
    })
}

fn key_list(values: &[FunctionKey]) -> Vec<Value> {
    values.iter().map(key_json).collect()
}

fn patch_json(plan: &stasis_compiler::backend::patch_plan::PatchPlan) -> Value {
    json!({
        "cold_start": plan.cold_start,
        "changed": key_list(&plan.changed),
        "re_jit_ids": plan.re_jit_ids,
        "re_jit": key_list(&plan.re_jit),
        "reused": key_list(&plan.reused),
        "retained_dependencies": key_list(&plan.retained_dependencies),
        "affected_host_entries": key_list(&plan.affected_host_entries),
        "removed": key_list(&plan.removed),
        "reasons": plan.reasons.iter().map(reason_chain_json).collect::<Vec<_>>(),
    })
}

fn function_json(function: &stasis_compiler::backend::program_snapshot::ProgramFunction) -> Value {
    json!({
        "id": function.id,
        "symbol_id": function.symbol_id.canonical(),
        "name": function.name,
        "module_alias": function.module_alias,
        "source_range": [function.source_range.start, function.source_range.end],
        "signature_range": [function.signature_range.start, function.signature_range.end],
        "signature_hash": function.signature_hash,
        "body_hash": function.body_hash,
        "params": function.params,
        "return_type": function.return_type,
        "dependencies": function.dependencies,
        "dependents": function.dependents,
    })
}

fn semantic_json(snapshot: &ProgramSnapshot) -> Value {
    let summaries = snapshot
        .data_flow_summaries()
        .iter()
        .map(|summary| {
            json!({
                "function": summary.function,
                "source": [summary.source_start, summary.source_end],
                "direct": {
                    "reads": summary.direct.reads,
                    "writes": summary.direct.writes,
                    "calls": summary.direct.calls,
                    "host_calls": summary.direct.host_calls,
                },
                "aggregate": {
                    "reads": summary.aggregate.reads,
                    "writes": summary.aggregate.writes,
                    "calls": summary.aggregate.calls,
                    "host_calls": summary.aggregate.host_calls,
                },
            })
        })
        .collect::<Vec<_>>();
    json!({
        "summary_schema": "stasis.data_flow.v3",
        "functions": summaries,
        "global_type_ids": snapshot.global_type_ids(),
        "collections": snapshot.collections().iter().map(|collection| json!({
            "path": collection.path,
            "capacity": collection.capacity,
            "element_type_id": collection.element_type_id,
            "field_type_ids": collection.field_type_ids,
        })).collect::<Vec<_>>(),
    })
}

fn semantic_snapshot_json(snapshot: &ProgramSnapshot) -> Value {
    json!({
        "source_revision": snapshot.source_revision(),
        "files": snapshot.files().iter().map(|file| json!({
            "path": file.path,
            "hash": file.hash,
        })).collect::<Vec<_>>(),
        "functions": snapshot.functions().iter().map(function_json).collect::<Vec<_>>(),
        "reachable_symbol_ids": snapshot.functions().iter()
            .filter(|function| snapshot.reachable_function_ids().contains(&function.id))
            .map(|function| function.symbol_id.canonical())
            .collect::<Vec<_>>(),
        "semantic": semantic_json(snapshot),
        "layout": {
            "digest": hex(snapshot.layout_digest()),
            "state": snapshot.state_layout(),
        },
        "literals": snapshot.literal_table(),
    })
}

fn normalized_clif(clif: &str) -> Value {
    let mut blocks = BTreeSet::new();
    let mut opcodes = BTreeSet::new();
    let mut call_targets = BTreeSet::new();
    const OPCODES: [&str; 28] = [
        "br", "brif", "call", "copy", "fadd", "fdiv", "fmul", "fsub", "iconst", "iadd", "iadd_imm",
        "band", "bor", "bxor", "imul", "isub", "isub_imm", "jump", "load", "store", "return",
        "sextend", "uextend", "ishl", "ishl_imm", "ushr", "ushr_imm", "trap",
    ];
    for line in clif.lines() {
        let trimmed = line.trim();
        if trimmed.ends_with(':') && !trimmed.contains(' ') {
            blocks.insert(trimmed.trim_end_matches(':').to_string());
            continue;
        }
        let operation = trimmed
            .split_once('=')
            .map_or(trimmed, |(_, right)| right.trim())
            .split_whitespace()
            .next()
            .unwrap_or_default();
        if operation.is_empty() || operation.starts_with(";") {
            continue;
        }
        let operation = operation.trim_end_matches(',');
        let Some(opcode) = OPCODES.iter().find(|candidate| {
            operation == **candidate || operation.starts_with(&format!("{}.", candidate))
        }) else {
            continue;
        };
        opcodes.insert((*opcode).to_string());
        if *opcode == "call" {
            let tokens = trimmed.split_whitespace().collect::<Vec<_>>();
            if let Some(index) = tokens.iter().position(|token| *token == "call") {
                if let Some(target) = tokens.get(index + 1) {
                    call_targets.insert(target.trim_end_matches(',').to_string());
                }
            }
        }
    }
    json!({
        "blocks": blocks.into_iter().collect::<Vec<_>>(),
        "opcodes": opcodes.into_iter().collect::<Vec<_>>(),
        "call_targets": call_targets.into_iter().collect::<Vec<_>>(),
    })
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, String> {
    let mut result = 0u32;
    let mut shift = 0;
    loop {
        let byte = *bytes
            .get(*cursor)
            .ok_or_else(|| "truncated wasm leb128".to_string())?;
        *cursor += 1;
        result |= u32::from(byte & 0x7f) << shift;
        if byte & 0x80 == 0 {
            return Ok(result);
        }
        shift += 7;
        if shift >= 32 {
            return Err("wasm leb128 overflows u32".to_string());
        }
    }
}

fn read_name(bytes: &[u8], cursor: &mut usize) -> Result<String, String> {
    let len = usize::try_from(read_u32(bytes, cursor)?).map_err(|_| "wasm name too long")?;
    let end = cursor
        .checked_add(len)
        .ok_or_else(|| "wasm name length overflow".to_string())?;
    let value = bytes
        .get(*cursor..end)
        .ok_or_else(|| "truncated wasm name".to_string())?;
    *cursor = end;
    String::from_utf8(value.to_vec()).map_err(|_| "wasm name is not utf8".to_string())
}

fn skip_limits(bytes: &[u8], cursor: &mut usize) -> Result<(), String> {
    let flags = *bytes
        .get(*cursor)
        .ok_or_else(|| "truncated wasm limits".to_string())?;
    *cursor += 1;
    read_u32(bytes, cursor)?;
    if flags & 1 != 0 {
        read_u32(bytes, cursor)?;
    }
    Ok(())
}

fn skip_valtype(bytes: &[u8], cursor: &mut usize) -> Result<(), String> {
    *cursor = cursor
        .checked_add(1)
        .ok_or_else(|| "wasm valtype overflow".to_string())?;
    if *cursor > bytes.len() {
        Err("truncated wasm valtype".to_string())
    } else {
        Ok(())
    }
}

fn opcode_family(opcode: u8) -> &'static str {
    match opcode {
        0x00..=0x01 => "control",
        0x02..=0x04 => "structured_control",
        0x05 => "else",
        0x0b..=0x0f => "control",
        0x10..=0x11 => "call",
        0x1a => "stack",
        0x20..=0x24 => "variable",
        0x28..=0x3e => "memory",
        0x3f..=0x40 => "memory_size_grow",
        0x41..=0x44 => "constant",
        0x45..=0xc4 => "numeric",
        0xd0..=0xd2 => "reference",
        0xfc => "extended",
        _ => "other",
    }
}

fn skip_instruction_immediate(bytes: &[u8], cursor: &mut usize, opcode: u8) -> Result<(), String> {
    match opcode {
        0x02..=0x04 | 0xd0 => {
            *cursor = cursor
                .checked_add(1)
                .ok_or_else(|| "wasm blocktype overflow".to_string())?;
        }
        0x0c | 0x0d | 0x10 | 0x20..=0x24 | 0x3f | 0x40 => {
            read_u32(bytes, cursor)?;
        }
        0x0e => {
            let count = read_u32(bytes, cursor)?;
            for _ in 0..=count {
                read_u32(bytes, cursor)?;
            }
        }
        0x11 => {
            read_u32(bytes, cursor)?;
            read_u32(bytes, cursor)?;
        }
        0x28..=0x3e => {
            read_u32(bytes, cursor)?;
            read_u32(bytes, cursor)?;
        }
        0x41 | 0x42 | 0xfc => {
            read_u32(bytes, cursor)?;
        }
        0x43 => *cursor = cursor.saturating_add(4),
        0x44 => *cursor = cursor.saturating_add(8),
        _ => {}
    }
    if *cursor > bytes.len() {
        Err("truncated wasm instruction immediate".to_string())
    } else {
        Ok(())
    }
}

fn code_opcode_families(bytes: &[u8]) -> Result<BTreeSet<String>, String> {
    let mut cursor = 0;
    let local_count = read_u32(bytes, &mut cursor)?;
    for _ in 0..local_count {
        read_u32(bytes, &mut cursor)?;
        skip_valtype(bytes, &mut cursor)?;
    }
    let mut families = BTreeSet::new();
    while cursor < bytes.len() {
        let opcode = bytes[cursor];
        cursor += 1;
        families.insert(opcode_family(opcode).to_string());
        skip_instruction_immediate(bytes, &mut cursor, opcode)?;
        if opcode == 0x0b && cursor == bytes.len() {
            break;
        }
    }
    Ok(families)
}

fn wasm_facts(module: &[u8]) -> Result<Value, String> {
    if module.get(..8) != Some(b"\0asm\x01\0\0\0") {
        return Err("invalid wasm header".to_string());
    }
    let mut cursor = 8;
    let mut sections = Vec::new();
    let mut imports = Vec::new();
    let mut exports = Vec::new();
    let mut functions = 0u32;
    let mut memories = 0u32;
    let mut opcode_families = BTreeSet::new();
    while cursor < module.len() {
        let id = module[cursor];
        cursor += 1;
        let size = usize::try_from(read_u32(module, &mut cursor)?)
            .map_err(|_| "wasm section is too large")?;
        let end = cursor
            .checked_add(size)
            .ok_or_else(|| "wasm section length overflow".to_string())?;
        let payload = module
            .get(cursor..end)
            .ok_or_else(|| "truncated wasm section".to_string())?;
        sections.push(match id {
            0 => "custom",
            1 => "type",
            2 => "import",
            3 => "function",
            5 => "memory",
            7 => "export",
            10 => "code",
            _ => "other",
        });
        let mut section_cursor = 0;
        match id {
            2 => {
                let count = read_u32(payload, &mut section_cursor)?;
                for _ in 0..count {
                    let module_name = read_name(payload, &mut section_cursor)?;
                    let name = read_name(payload, &mut section_cursor)?;
                    let kind = *payload
                        .get(section_cursor)
                        .ok_or_else(|| "truncated wasm import kind".to_string())?;
                    section_cursor += 1;
                    match kind {
                        0 => {
                            read_u32(payload, &mut section_cursor)?;
                        }
                        1 => {
                            skip_valtype(payload, &mut section_cursor)?;
                            skip_limits(payload, &mut section_cursor)?;
                        }
                        2 => skip_limits(payload, &mut section_cursor)?,
                        3 => {
                            skip_valtype(payload, &mut section_cursor)?;
                            skip_valtype(payload, &mut section_cursor)?;
                        }
                        _ => return Err("unknown wasm import kind".to_string()),
                    }
                    imports.push(json!({"module": module_name, "name": name, "kind": kind}));
                }
            }
            3 => functions = read_u32(payload, &mut section_cursor)?,
            5 => memories = read_u32(payload, &mut section_cursor)?,
            7 => {
                let count = read_u32(payload, &mut section_cursor)?;
                for _ in 0..count {
                    let name = read_name(payload, &mut section_cursor)?;
                    let kind = *payload
                        .get(section_cursor)
                        .ok_or_else(|| "truncated wasm export kind".to_string())?;
                    section_cursor += 1;
                    let index = read_u32(payload, &mut section_cursor)?;
                    exports.push(json!({"name": name, "kind": kind, "index": index}));
                }
            }
            10 => {
                let count = read_u32(payload, &mut section_cursor)?;
                for _ in 0..count {
                    let body_size = usize::try_from(read_u32(payload, &mut section_cursor)?)
                        .map_err(|_| "wasm body is too large")?;
                    let body_end = section_cursor
                        .checked_add(body_size)
                        .ok_or_else(|| "wasm body length overflow".to_string())?;
                    let body = payload
                        .get(section_cursor..body_end)
                        .ok_or_else(|| "truncated wasm body".to_string())?;
                    opcode_families.extend(code_opcode_families(body)?);
                    section_cursor = body_end;
                }
            }
            _ => {}
        }
        cursor = end;
    }
    sections.sort_unstable();
    imports.sort_by_key(|value| value.to_string());
    exports.sort_by_key(|value| value.to_string());
    Ok(json!({
        "sections": sections,
        "imports": imports,
        "exports": exports,
        "function_count": functions,
        "memory_count": memories,
        "opcode_families": opcode_families.into_iter().collect::<Vec<_>>(),
    }))
}

fn compiler_facts() -> Result<Value, String> {
    let required = roots();
    let fixture = fixture();
    let mut compiler = Compiler::new();
    compiler.set_analysis_required_roots(&required);
    compiler.upsert_file(FIXTURE_PATH, &fixture);
    compiler
        .check()
        .map_err(|error| format!("compiler check failed: {error:?}"))?;
    let accepted = capture_accepted_program(compiler.functions(), compiler.files(), &required)?;
    let cold = plan_patch(
        compiler.functions(),
        compiler.files(),
        &required,
        None,
        &BTreeSet::new(),
    )?;
    let edited = fixture.replace("value + 1", "value + 2");
    compiler.upsert_file(FIXTURE_PATH, edited);
    compiler
        .check()
        .map_err(|error| format!("edited compiler check failed: {error:?}"))?;
    let changed = plan_patch(
        compiler.functions(),
        compiler.files(),
        &required,
        Some(&accepted),
        &BTreeSet::new(),
    )?;
    let mut invalid = Compiler::new();
    invalid.upsert_file("invalid.stasis", "function broken(): i32 { return 1 }\n");
    let _error = invalid
        .check()
        .expect_err("invalid fixture must be rejected");
    let diagnostic = invalid
        .last_source_diagnostic()
        .ok_or_else(|| "invalid fixture did not produce a diagnostic".to_string())?;
    let reachable_symbol_ids = accepted
        .reachable
        .iter()
        .map(|key| json!(key.symbol_id.canonical()))
        .collect::<Vec<_>>();
    let declaration_files = compiler.files().to_vec();
    let declaration_functions = compiler.functions().to_vec();
    let declarations = declaration_functions
        .iter()
        .map(|function| {
            let file = &declaration_files[function.file_id as usize];
            json!({
                "path": file.path,
                "name": function.name,
                "source_range": [function.source_range.start, function.source_range.end],
                "signature_range": [function.signature_range.start, function.signature_range.end],
            })
        })
        .collect::<Vec<_>>();
    let facts = json!({
        "declarations": declarations,
        "invalid_diagnostic": {
            "path": diagnostic.path,
            "range": [diagnostic.start, diagnostic.end],
            "symbol": diagnostic.symbol,
            "code": diagnostic.code.as_str(),
            "message": diagnostic.message,
        },
        "patch": {"cold": patch_json(&cold), "edited": patch_json(&changed)},
        "reachable_symbol_ids": reachable_symbol_ids,
    });
    Ok(facts)
}

fn object_symbols(process: &mut AotProcess) -> Result<Value, String> {
    let root = std::env::temp_dir().join(format!(
        "stasis-characterization-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    if root.exists() {
        fs::remove_dir_all(&root).map_err(|error| format!("remove old object temp: {error}"))?;
    }
    let paths = process
        .write_object_files_by_id(&root)
        .map_err(|error| format!("write AOT characterization objects: {error}"))?;
    let mut symbols = BTreeSet::new();
    for path in paths.values().map(|(_, path)| path) {
        let bytes = fs::read(path).map_err(|error| format!("read AOT object: {error}"))?;
        let file = object::File::parse(bytes.as_slice())
            .map_err(|error| format!("parse AOT object: {error}"))?;
        symbols.extend(file.symbols().filter_map(|symbol| {
            symbol
                .name()
                .ok()
                .map(|name| name.trim_start_matches('_'))
                .filter(|name| {
                    name.starts_with("aot_fn_")
                        || name.starts_with("stasis_")
                        || name == &"score"
                        || name.starts_with("samples")
                })
                .map(str::to_string)
        }));
    }
    let _ = fs::remove_dir_all(&root);
    Ok(json!({"artifact_count": paths.len(), "symbols": symbols.into_iter().collect::<Vec<_>>() }))
}

fn target_facts() -> Result<Value, String> {
    let required = roots();
    let fixture = fixture();
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&required);
    jit.upsert_file(FIXTURE_PATH, &fixture);
    jit.compile()
        .map_err(|error| format!("JIT compile failed: {error:?}"))?;
    let jit_snapshot = jit
        .program_snapshot()
        .ok_or_else(|| "JIT snapshot missing".to_string())?
        .clone();
    let jit_artifacts = jit
        .artifacts()
        .iter()
        .map(|artifact| {
            json!({
                "function_id": artifact.function_id,
                "symbol_id": artifact.function_key.symbol_id.canonical(),
                "slot": artifact.slot,
                "body_hash": artifact.body_hash,
            })
        })
        .collect::<Vec<_>>();
    let jit_result = jit
        .execute_i32_noarg_by_name("main")
        .map_err(|error| format!("JIT execution failed: {error}"))?;
    let clif = normalized_clif(
        jit.clif_for_function_name("main")
            .ok_or_else(|| "JIT CLIF for main missing".to_string())?,
    );

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&required);
    aot.upsert_file(FIXTURE_PATH, &fixture);
    aot.compile()
        .map_err(|error| format!("AOT compile failed: {error:?}"))?;
    let aot_snapshot = aot
        .program_snapshot()
        .ok_or_else(|| "AOT snapshot missing".to_string())?
        .clone();
    let aot_artifacts = aot
        .artifacts()
        .iter()
        .map(|artifact| {
            json!({
                "function_id": artifact.function_id,
                "symbol_id": artifact.symbol_id.canonical(),
                "object_index": artifact.object_index,
                "body_hash": artifact.body_hash,
                "symbol_name": artifact.symbol_name,
            })
        })
        .collect::<Vec<_>>();
    let aot_objects = object_symbols(&mut aot)?;

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&required);
    wasm.upsert_file(FIXTURE_PATH, &fixture);
    wasm.compile()
        .map_err(|error| format!("Wasm compile failed: {error:?}"))?;
    let wasm_snapshot = wasm
        .program_snapshot()
        .ok_or_else(|| "Wasm snapshot missing".to_string())?
        .clone();

    let normalized_global_type_shapes = |snapshot: &ProgramSnapshot| {
        snapshot
            .global_type_ids()
            .iter()
            .map(|(path, type_id)| {
                (
                    path.clone(),
                    snapshot
                        .type_info(*type_id)
                        .map(|info| info.name.clone())
                        .unwrap_or_default(),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let parity = |other: &ProgramSnapshot| {
        json!({
            "functions": other.functions() == jit_snapshot.functions(),
            "reachable_symbol_ids": other.reachable_function_ids() == jit_snapshot.reachable_function_ids(),
            "layout_digest": other.layout_digest() == jit_snapshot.layout_digest(),
            "global_type_shapes": normalized_global_type_shapes(other) == normalized_global_type_shapes(&jit_snapshot),
            "collections": other.collections() == jit_snapshot.collections(),
        })
    };
    Ok(json!({
        "jit": {
            "snapshot": semantic_snapshot_json(&jit_snapshot),
            "artifacts": jit_artifacts,
            "result_main": jit_result,
            "clif_main": clif,
        },
        "aot": {
            "snapshot": semantic_snapshot_json(&aot_snapshot),
            "artifacts": aot_artifacts,
            "objects": aot_objects,
        },
        "wasm": {
            "snapshot": semantic_snapshot_json(&wasm_snapshot),
            "module": wasm_facts(wasm.module_bytes())?,
        },
        "program_snapshot_parity": {
            "jit_aot": parity(&aot_snapshot),
            "jit_wasm": parity(&wasm_snapshot),
        },
    }))
}

fn characterization() -> Result<Value, String> {
    let compiler = compiler_facts()?;
    Ok(json!({
        "schema": "stasis.compiler_characterization.v1",
        "fixture": FIXTURE_PATH,
        "compiler": compiler,
        "targets": target_facts()?,
    }))
}

#[test]
fn compiler_pipeline_matches_checked_in_characterization() {
    let actual = characterization().expect("compiler characterization");
    let golden_path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .join("tests/characterization/compiler_pipeline_v1.json");
    let golden: Value = serde_json::from_str(
        &fs::read_to_string(&golden_path).expect("read compiler characterization golden"),
    )
    .expect("parse compiler characterization golden");
    if std::env::var_os("STASIS_UPDATE_CHARACTERIZATION").is_some() {
        fs::write(
            &golden_path,
            serde_json::to_string_pretty(&actual).expect("serialize characterization") + "\n",
        )
        .expect("write compiler characterization golden");
    }
    assert_eq!(actual, golden);
}
