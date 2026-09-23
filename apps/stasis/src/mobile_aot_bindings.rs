use crate::build_aot_direct_storage_source;
use crate::compiler_backend::{
    append_replay_state_snapshot_bridge_source, replay_snapshot_bridge_can_emit,
};
use stasis_assets::{load_project_asset_manifest, AssetFormat, AssetLimits, ResolvedAssetManifest};
use stasis_compiler::backend::program_snapshot::ProgramReplayStateSnapshot;
use stasis_compiler::backend::state_layout::StateLayout;
use std::fs;
use std::path::Path;

pub fn write_mobile_aot_bindings_source(
    manifest: &serde_json::Value,
    state_layout: &StateLayout,
    project_dir: &Path,
    output_path: &Path,
) -> Result<(), String> {
    write_mobile_aot_bindings_source_with_profile(
        manifest,
        state_layout,
        project_dir,
        output_path,
        &[],
        0,
        0,
    )
}

pub fn write_mobile_aot_bindings_source_with_profile(
    manifest: &serde_json::Value,
    state_layout: &StateLayout,
    project_dir: &Path,
    output_path: &Path,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
) -> Result<(), String> {
    write_mobile_aot_bindings_source_with_profile_and_snapshot(
        manifest,
        state_layout,
        project_dir,
        output_path,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
        None,
    )
}

pub fn write_mobile_aot_bindings_source_with_profile_and_snapshot(
    manifest: &serde_json::Value,
    state_layout: &StateLayout,
    project_dir: &Path,
    output_path: &Path,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
    replay_state_snapshot: Option<&ProgramReplayStateSnapshot>,
) -> Result<(), String> {
    let assets = load_project_asset_manifest(project_dir, AssetLimits::default())
        .map_err(|error| format!("failed to resolve mobile AOT assets: {error}"))?;
    write_mobile_aot_bindings_source_with_profile_and_assets_and_snapshot(
        manifest,
        state_layout,
        &assets,
        output_path,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
        replay_state_snapshot,
    )
}

pub fn write_mobile_aot_bindings_source_with_profile_and_assets(
    manifest: &serde_json::Value,
    state_layout: &StateLayout,
    assets: &ResolvedAssetManifest,
    output_path: &Path,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
) -> Result<(), String> {
    write_mobile_aot_bindings_source_with_profile_and_assets_and_snapshot(
        manifest,
        state_layout,
        assets,
        output_path,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
        None,
    )
}

pub fn write_mobile_aot_bindings_source_with_profile_and_assets_and_snapshot(
    manifest: &serde_json::Value,
    state_layout: &StateLayout,
    assets: &ResolvedAssetManifest,
    output_path: &Path,
    profile_functions: &[String],
    profile_warmup_frames: u32,
    profile_sample_frames: u32,
    replay_state_snapshot: Option<&ProgramReplayStateSnapshot>,
) -> Result<(), String> {
    let functions = manifest
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing functions array".to_string())?;
    let render_lifecycle_version = mobile_aot_render_lifecycle_version(manifest, functions)?;
    let literals = manifest
        .get("string_literals")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing string_literals array".to_string())?;
    let mut out = String::from(
        "#include <stdint.h>\n#include <string.h>\n#include \"stasis_mobile_aot_runtime.h\"\n\n\
#if defined(_WIN32)\n#define STASIS_EXPORT __declspec(dllexport)\n#else\n#define STASIS_EXPORT __attribute__((visibility(\"default\")))\n#endif\n\n",
    );
    let (direct_storage_source, direct_storage_register_lines) =
        build_aot_direct_storage_source(state_layout)?;
    out.push_str(&direct_storage_source);
    if let Some(snapshot) =
        replay_state_snapshot.filter(|snapshot| replay_snapshot_bridge_can_emit(snapshot))
    {
        append_replay_state_snapshot_bridge_source(&mut out, snapshot)?;
    }
    for function in functions {
        let name = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing name".to_string())?;
        let symbol = function
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing symbol".to_string())?;
        let return_type = mobile_aot_c_return_type(function)?;
        if name == "gfx_cmd_construction_finish" {
            out.push_str(&format!("extern int32_t {symbol}(int32_t);\n"));
        } else {
            out.push_str(&format!("extern {return_type} {symbol}(void);\n"));
        }
    }
    for (name, wrapper) in [
        ("main", "stasis_mobile_main_entry"),
        ("tick", "stasis_mobile_tick_entry"),
        ("render", "stasis_mobile_render_entry"),
    ] {
        let (symbol, return_type) = mobile_aot_function_for(manifest, name)?;
        if name == "render" && render_lifecycle_version == 1 {
            let (reset, _) = mobile_aot_function_for(manifest, "gfx_cmd_construction_reset")?;
            let (finish, _) = mobile_aot_function_for(manifest, "gfx_cmd_construction_finish")?;
            if return_type == 0 {
                out.push_str(&format!(
                    "int32_t {wrapper}(void) {{ {reset}(); {symbol}(); return {finish}(0); }}\n"
                ));
            } else if return_type == 1 {
                out.push_str(&format!(
                    "int32_t {wrapper}(void) {{ {reset}(); return {finish}({symbol}()); }}\n"
                ));
            } else {
                return Err(format!(
                    "mobile AOT entry '{name}' must return void or i32, found type id {return_type}"
                ));
            }
        } else if return_type == 0 {
            out.push_str(&format!(
                "int32_t {wrapper}(void) {{ {symbol}(); return 0; }}\n"
            ));
        } else if return_type == 1 {
            out.push_str(&format!(
                "int32_t {wrapper}(void) {{ return {symbol}(); }}\n"
            ));
        } else {
            return Err(format!(
                "mobile AOT entry '{name}' must return void or i32, found type id {return_type}"
            ));
        }
    }
    for literal in literals {
        let id = literal
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "mobile AOT string literal missing id".to_string())?;
        let value = literal
            .get("value")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT string literal missing value".to_string())?;
        out.push_str(&format!(
            "static const char stasis_mobile_literal_{}[] = \"{}\";\n",
            id.unsigned_abs(),
            escape_mobile_c_string_literal(value)
        ));
    }
    out.push_str("\ntypedef struct { const char *path; int32_t handle; } StasisPublishedSprite;\n");
    out.push_str("static const StasisPublishedSprite stasis_published_sprites[] = {\n");
    for asset in assets
        .assets
        .iter()
        .filter(|asset| matches!(asset.entry.format, AssetFormat::Sprite { .. }))
    {
        out.push_str(&format!(
            "    {{\"{}\", {}}},\n",
            escape_mobile_c_string_literal(&asset.entry.path),
            asset.handle.as_i32()
        ));
    }
    out.push_str("    {0, 0},\n};\n");
    out.push_str(
        "int32_t stasis_published_sprite_handle_for_path(const char *path) {\n\
         \x20   if (path == 0) return 0;\n\
         \x20   while (path[0] == '.' && path[1] == '/') path += 2;\n\
         \x20   while (path[0] == '.' && path[1] == '.' && path[2] == '/') path += 3;\n\
         \x20   for (uintptr_t index = 0; index < sizeof(stasis_published_sprites) / sizeof(stasis_published_sprites[0]); index += 1) {\n\
         \x20       if (stasis_published_sprites[index].path != 0 && strcmp(path, stasis_published_sprites[index].path) == 0) return stasis_published_sprites[index].handle;\n\
         \x20   }\n\
         \x20   return 0;\n\
         }\n",
    );
    out.push_str("\nvoid stasis_aot_bind_runtime_globals(void) {\n");
    for line in direct_storage_register_lines {
        out.push_str(&format!("    {line}\n"));
    }
    out.push_str("    stasis_jit_clear_string_literal_table();\n");
    for literal in literals {
        let id = literal
            .get("id")
            .and_then(serde_json::Value::as_i64)
            .ok_or_else(|| "mobile AOT string literal missing id".to_string())?;
        out.push_str(&format!(
            "    stasis_jit_upsert_string_literal({id}, stasis_mobile_literal_{});\n",
            id.unsigned_abs()
        ));
    }
    for name in profile_functions {
        let function = functions
            .iter()
            .find(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(name))
            .ok_or_else(|| format!("mobile AOT profile function '{name}' was not emitted"))?;
        let function_id = function
            .get("function_id")
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("mobile AOT profile function '{name}' missing function_id"))?;
        out.push_str(&format!(
            "    stasis_jit_profile_register_function((int32_t)UINT32_C({function_id}), \"{}\");\n",
            escape_mobile_c_string_literal(name)
        ));
    }
    if !profile_functions.is_empty() {
        out.push_str(&format!(
            "    stasis_jit_profile_configure({profile_warmup_frames}, {profile_sample_frames});\n"
        ));
    }
    out.push_str("}\n");
    audit_mobile_aot_bindings(manifest, &out)?;
    fs::write(output_path, out).map_err(|error| {
        format!(
            "failed to write mobile AOT bindings source {}: {error}",
            output_path.display()
        )
    })
}

pub fn audit_mobile_aot_bindings(
    manifest: &serde_json::Value,
    bindings_source: &str,
) -> Result<(), String> {
    let functions = manifest
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing functions array".to_string())?;
    let render_lifecycle_version = mobile_aot_render_lifecycle_version(manifest, functions)?;
    for function in functions {
        let name = function
            .get("name")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing name".to_string())?;
        let symbol = function
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing symbol".to_string())?;
        let return_type = mobile_aot_c_return_type(function)?;
        let declaration = if name == "gfx_cmd_construction_finish" {
            format!("extern int32_t {symbol}(int32_t);")
        } else {
            format!("extern {return_type} {symbol}(void);")
        };
        if !bindings_source.contains(&declaration) {
            return Err(format!(
                "mobile AOT bindings missing declaration for generated symbol '{symbol}'"
            ));
        }
    }
    for (name, wrapper) in [
        ("main", "stasis_mobile_main_entry"),
        ("tick", "stasis_mobile_tick_entry"),
        ("render", "stasis_mobile_render_entry"),
    ] {
        let (symbol, return_type) = mobile_aot_function_for(manifest, name)?;
        let expected = if name == "render" && render_lifecycle_version == 1 {
            let (reset, _) = mobile_aot_function_for(manifest, "gfx_cmd_construction_reset")?;
            let (finish, _) = mobile_aot_function_for(manifest, "gfx_cmd_construction_finish")?;
            if return_type == 0 {
                format!("int32_t {wrapper}(void) {{ {reset}(); {symbol}(); return {finish}(0); }}")
            } else {
                format!("int32_t {wrapper}(void) {{ {reset}(); return {finish}({symbol}()); }}")
            }
        } else if return_type == 0 {
            format!("int32_t {wrapper}(void) {{ {symbol}(); return 0; }}")
        } else {
            format!("int32_t {wrapper}(void) {{ return {symbol}(); }}")
        };
        if !bindings_source.contains(&expected) {
            return Err(format!(
                "mobile AOT bindings wrapper '{wrapper}' does not target generated symbol '{symbol}'"
            ));
        }
    }
    if !bindings_source.contains("void stasis_aot_bind_runtime_globals(void)") {
        return Err("mobile AOT bindings missing runtime-global binding entry".to_string());
    }
    Ok(())
}

fn mobile_aot_render_lifecycle_version(
    manifest: &serde_json::Value,
    functions: &[serde_json::Value],
) -> Result<u64, String> {
    let version = manifest
        .get("render_construction_lifecycle_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if version > 1 {
        return Err(format!(
            "mobile AOT manifest has unsupported render construction lifecycle version {version}"
        ));
    }
    let has_reset = functions.iter().any(|entry| {
        entry.get("name").and_then(serde_json::Value::as_str) == Some("gfx_cmd_construction_reset")
    });
    let has_finish = functions.iter().any(|entry| {
        entry.get("name").and_then(serde_json::Value::as_str) == Some("gfx_cmd_construction_finish")
    });
    if has_reset != has_finish || (version == 1) != (has_reset && has_finish) {
        return Err(format!(
            "mobile AOT render construction lifecycle {version} does not match generated reset/finish helpers"
        ));
    }
    Ok(version)
}

pub fn mobile_aot_function_for(
    manifest: &serde_json::Value,
    function_name: &str,
) -> Result<(String, u64), String> {
    let functions = manifest
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing functions array".to_string())?;
    let function = functions
        .iter()
        .find(|entry| {
            entry.get("name").and_then(serde_json::Value::as_str) == Some(function_name)
                && (function_name != "tick"
                    || entry
                        .get("parameter_count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0)
                        == 0)
        })
        .ok_or_else(|| format!("mobile AOT manifest missing function '{function_name}'"))?;
    let symbol = function
        .get("symbol")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("mobile AOT function '{function_name}' missing symbol"))?;
    let return_type = function
        .get("return_type")
        .and_then(serde_json::Value::as_u64)
        .ok_or_else(|| format!("mobile AOT function '{function_name}' missing return_type"))?;
    Ok((symbol.to_string(), return_type))
}

fn mobile_aot_c_return_type(function: &serde_json::Value) -> Result<&'static str, String> {
    match function
        .get("return_type")
        .and_then(serde_json::Value::as_u64)
    {
        Some(0) => Ok("void"),
        Some(2) => Ok("float"),
        Some(4) => Ok("double"),
        Some(_) => Ok("int32_t"),
        None => Err("mobile AOT function missing return_type".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mobile_tick_lookup_uses_zero_argument_manifest_row() {
        let manifest = serde_json::json!({
            "functions": [
                {"name": "tick", "symbol": "tick_with_arg", "return_type": 1, "parameter_count": 1},
                {"name": "tick", "symbol": "tick_noarg", "return_type": 1, "parameter_count": 0}
            ]
        });

        assert_eq!(
            mobile_aot_function_for(&manifest, "tick").expect("zero-argument tick"),
            ("tick_noarg".to_string(), 1)
        );
    }
}

pub fn escape_mobile_c_string_literal(value: &str) -> String {
    let mut escaped = String::new();
    for byte in value.bytes() {
        match byte {
            b'\\' => escaped.push_str("\\\\"),
            b'"' => escaped.push_str("\\\""),
            b'\n' => escaped.push_str("\\n"),
            b'\r' => escaped.push_str("\\r"),
            b'\t' => escaped.push_str("\\t"),
            b' '..=b'~' => escaped.push(char::from(byte)),
            _ => escaped.push_str(&format!("\\{byte:03o}")),
        }
    }
    escaped
}

pub fn write_mobile_aot_replay_identity(
    compatibility: &serde_json::Value,
    snapshot: &ProgramReplayStateSnapshot,
    snapshot_supported: bool,
    header_path: &Path,
    source_path: &Path,
) -> Result<(), String> {
    let string_value = |name: &str| {
        compatibility
            .get(name)
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| format!("packaged replay compatibility missing string field '{name}'"))
    };
    let number_value = |name: &str| {
        compatibility
            .get(name)
            .and_then(serde_json::Value::as_u64)
            .ok_or_else(|| format!("packaged replay compatibility missing integer field '{name}'"))
    };
    let optional_string = |name: &str| {
        let value = compatibility.get(name);
        if value.is_none() || value.is_some_and(serde_json::Value::is_null) {
            Ok(None)
        } else {
            value
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
                .map(Some)
                .ok_or_else(|| {
                    format!("packaged replay compatibility field '{name}' must be text or null")
                })
        }
    };
    let stasis_version = string_value("stasis_version")?;
    let release_id = string_value("release_id")?;
    let source_sha256 = string_value("source_sha256")?;
    let state_layout_sha256 = string_value("state_layout_sha256")?;
    let compiler_layout_sha256 = string_value("compiler_layout_sha256")?;
    let input_usage_sha256 = string_value("input_usage_sha256")?;
    let hash_scope = string_value("hash_scope")?;
    let determinism_profile = string_value("determinism_profile")?;
    let asset_manifest_sha256 = optional_string("asset_manifest_sha256")?;
    let controller_schema_version = compatibility
        .get("controller_schema_version")
        .filter(|value| !value.is_null())
        .map(|value| {
            value.as_u64().ok_or_else(|| {
                "packaged replay controller_schema_version must be an integer or null".to_string()
            })
        })
        .transpose()?
        .unwrap_or(0);
    let host_schema_version = number_value("host_schema_version")?;
    let host_i32_count = number_value("host_i32_count")?;
    let host_f32_count = number_value("host_f32_count")?;
    let tick_rate_hz = number_value("tick_rate_hz")?;

    let descriptor_values = |name: &str| {
        compatibility
            .get(name)
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| format!("packaged replay compatibility missing array field '{name}'"))
    };
    let observed_i32 = descriptor_values("observed_i32")?;
    let observed_f32 = descriptor_values("observed_f32")?;
    let descriptor_source =
        |prefix: &str, values: &[serde_json::Value]| -> Result<String, String> {
            if values.is_empty() {
                return Ok(String::new());
            }
            let mut out = format!("static const StasisReplayInputDescriptor {prefix}[] = {{\n");
            for (index, value) in values.iter().enumerate() {
                let slot = value
                    .get("slot")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| format!("{prefix}[{index}] missing slot"))?;
                let field_index = value
                    .get("index")
                    .and_then(serde_json::Value::as_u64)
                    .ok_or_else(|| format!("{prefix}[{index}] missing index"))?;
                let path = value
                    .get("path")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("{prefix}[{index}] missing path"))?;
                let family = value
                    .get("family")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("{prefix}[{index}] missing family"))?;
                out.push_str(&format!(
                    "    {{UINT32_C({slot}), UINT32_C({field_index}), \"{}\", \"{}\"}},\n",
                    escape_mobile_c_string_literal(path),
                    escape_mobile_c_string_literal(family)
                ));
            }
            out.push_str("};\n");
            Ok(out)
        };
    let mut source = String::from("#include <stdint.h>\n#include \"stasis_replay_consumer.h\"\n\n");
    source.push_str(&descriptor_source(
        "stasis_published_replay_i32",
        observed_i32,
    )?);
    source.push_str(&descriptor_source(
        "stasis_published_replay_f32",
        observed_f32,
    )?);
    if !snapshot.entries.is_empty() {
        source.push_str(
            "static const StasisReplayStateEntry stasis_published_replay_state_entries[] = {\n",
        );
        for entry in &snapshot.entries {
            source.push_str(&format!(
                "    {{\"{}\", \"{}\", \"{}\", \"{}\", UINT64_C({}), UINT64_C({}), UINT8_C({})}},\n",
                escape_mobile_c_string_literal(&entry.kind),
                escape_mobile_c_string_literal(&entry.path),
                escape_mobile_c_string_literal(&entry.field),
                escape_mobile_c_string_literal(&entry.storage_type),
                entry.offset,
                entry.element_count,
                entry.element_bytes
            ));
        }
        source.push_str("};\n");
    }
    source.push_str(&format!(
        "static const char stasis_published_replay_stasis_version[] = \"{}\";\n\
static const char stasis_published_replay_release_id[] = \"{}\";\n\
static const char stasis_published_replay_source_sha256[] = \"{}\";\n\
static const char stasis_published_replay_state_layout_sha256[] = \"{}\";\n\
static const char stasis_published_replay_compiler_layout_sha256[] = \"{}\";\n\
static const char stasis_published_replay_input_usage_sha256[] = \"{}\";\n\
static const char stasis_published_replay_hash_scope[] = \"{}\";\n\
static const char stasis_published_replay_determinism_profile[] = \"{}\";\n",
        escape_mobile_c_string_literal(stasis_version),
        escape_mobile_c_string_literal(release_id),
        escape_mobile_c_string_literal(source_sha256),
        escape_mobile_c_string_literal(state_layout_sha256),
        escape_mobile_c_string_literal(compiler_layout_sha256),
        escape_mobile_c_string_literal(input_usage_sha256),
        escape_mobile_c_string_literal(hash_scope),
        escape_mobile_c_string_literal(determinism_profile),
    ));
    if let Some(asset_hash) = asset_manifest_sha256.as_deref() {
        source.push_str(&format!(
            "static const char stasis_published_replay_asset_manifest_sha256[] = \"{}\";\n",
            escape_mobile_c_string_literal(asset_hash)
        ));
    }
    source.push_str(&format!(
        "\nstatic const StasisReplayCompatibility stasis_published_replay_compatibility_value = {{\n\
    stasis_published_replay_stasis_version,\n\
    stasis_published_replay_release_id,\n\
    stasis_published_replay_source_sha256,\n\
    stasis_published_replay_state_layout_sha256,\n\
    stasis_published_replay_compiler_layout_sha256,\n\
    {},\n\
    UINT32_C({}),\n\
    UINT32_C({}),\n\
    UINT32_C({}),\n\
    stasis_published_replay_input_usage_sha256,\n\
    UINT32_C({}),\n\
    stasis_published_replay_hash_scope,\n\
    stasis_published_replay_determinism_profile,\n\
    UINT32_C({}),\n\
    {},\n\
    {},\n\
    {},\n\
    {},\n\
}};\n\
\nstatic const StasisReplayStateDescriptor stasis_published_replay_state_descriptor_value = {{\n\
    UINT64_C({}),\n\
    {},\n\
    {}\n\
}};\n",
        asset_manifest_sha256
            .as_deref()
            .map(|_| "stasis_published_replay_asset_manifest_sha256")
            .unwrap_or("NULL"),
        host_schema_version,
        host_i32_count,
        host_f32_count,
        tick_rate_hz,
        controller_schema_version,
        if observed_i32.is_empty() {
            "NULL"
        } else {
            "stasis_published_replay_i32"
        },
        observed_i32.len(),
        if observed_f32.is_empty() {
            "NULL"
        } else {
            "stasis_published_replay_f32"
        },
        observed_f32.len(),
        snapshot.required_bytes,
        if snapshot.entries.is_empty() {
            "NULL"
        } else {
            "stasis_published_replay_state_entries"
        },
        snapshot.entries.len(),
    ));
    if snapshot_supported {
        source.push_str(
            "extern int32_t stasis_replay_state_snapshot_size(void);\n\
extern int32_t stasis_replay_state_snapshot_write(uint8_t *, int32_t);\n\
extern int32_t stasis_replay_state_snapshot_restore(const uint8_t *, int32_t);\n",
        );
        source.push_str(
            "static int32_t stasis_published_replay_state_size(void *context) {\n\
    (void)context;\n\
    return stasis_replay_state_snapshot_size();\n\
}\n\
static int32_t stasis_published_replay_state_write(void *context, uint8_t *output, int32_t capacity) {\n\
    (void)context;\n\
    return stasis_replay_state_snapshot_write(output, capacity);\n\
}\n\
static int32_t stasis_published_replay_state_restore(void *context, const uint8_t *input, int32_t bytes) {\n\
    (void)context;\n\
    return stasis_replay_state_snapshot_restore(input, bytes);\n\
}\n",
        );
    }
    source.push_str(
        "\nconst StasisReplayCompatibility *stasis_published_replay_compatibility(void) {\n\
    return &stasis_published_replay_compatibility_value;\n\
}\n\
const StasisReplayStateDescriptor *stasis_published_replay_state_descriptor(void) {\n\
    return &stasis_published_replay_state_descriptor_value;\n\
}\n\
StasisReplayStateOps stasis_published_replay_state_ops(void) {\n",
    );
    if snapshot_supported {
        source.push_str(
            "    return (StasisReplayStateOps){NULL, stasis_published_replay_state_size,\n\
        stasis_published_replay_state_write, stasis_published_replay_state_restore};\n",
        );
    } else {
        source.push_str("    return (StasisReplayStateOps){0};\n");
    }
    source.push_str("}\n");

    let header = "#pragma once\n\n#include \"stasis_replay_consumer.h\"\n\n\
const StasisReplayCompatibility *stasis_published_replay_compatibility(void);\n\
const StasisReplayStateDescriptor *stasis_published_replay_state_descriptor(void);\n\
StasisReplayStateOps stasis_published_replay_state_ops(void);\n";
    if let Some(parent) = header_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create replay identity header directory {}: {error}",
                parent.display()
            )
        })?;
    }
    if let Some(parent) = source_path.parent() {
        fs::create_dir_all(parent).map_err(|error| {
            format!(
                "failed to create replay identity source directory {}: {error}",
                parent.display()
            )
        })?;
    }
    fs::write(header_path, header).map_err(|error| {
        format!(
            "failed to write replay identity header {}: {error}",
            header_path.display()
        )
    })?;
    fs::write(source_path, source).map_err(|error| {
        format!(
            "failed to write replay identity source {}: {error}",
            source_path.display()
        )
    })?;
    Ok(())
}
