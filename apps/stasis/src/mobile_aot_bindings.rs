use crate::build_aot_direct_storage_source;
use stasis_assets::{load_project_asset_manifest, AssetFormat, AssetLimits, ResolvedAssetManifest};
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
    let assets = load_project_asset_manifest(project_dir, AssetLimits::default())
        .map_err(|error| format!("failed to resolve mobile AOT assets: {error}"))?;
    write_mobile_aot_bindings_source_with_profile_and_assets(
        manifest,
        state_layout,
        &assets,
        output_path,
        profile_functions,
        profile_warmup_frames,
        profile_sample_frames,
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
    let functions = manifest
        .get("functions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing functions array".to_string())?;
    let literals = manifest
        .get("string_literals")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "mobile AOT manifest missing string_literals array".to_string())?;
    let mut out = String::from(
        "#include <stdint.h>\n#include <string.h>\n#include \"stasis_mobile_aot_runtime.h\"\n\n",
    );
    let (direct_storage_source, direct_storage_register_lines) =
        build_aot_direct_storage_source(state_layout)?;
    out.push_str(&direct_storage_source);
    for function in functions {
        let symbol = function
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing symbol".to_string())?;
        let return_type = mobile_aot_c_return_type(function)?;
        out.push_str(&format!("extern {return_type} {symbol}(void);\n"));
    }
    for (name, wrapper) in [
        ("main", "stasis_mobile_main_entry"),
        ("tick", "stasis_mobile_tick_entry"),
        ("render", "stasis_mobile_render_entry"),
    ] {
        let (symbol, return_type) = mobile_aot_function_for(manifest, name)?;
        if return_type == 0 {
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
    for function in functions {
        let symbol = function
            .get("symbol")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| "mobile AOT function missing symbol".to_string())?;
        let return_type = mobile_aot_c_return_type(function)?;
        let declaration = format!("extern {return_type} {symbol}(void);");
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
        let expected = if return_type == 0 {
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
        .find(|entry| entry.get("name").and_then(serde_json::Value::as_str) == Some(function_name))
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

// Escape UTF-8 bytes with fixed-width octal so following digits cannot extend an escape.
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
