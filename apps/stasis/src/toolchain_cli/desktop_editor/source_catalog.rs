use super::MAX_SOURCE_CONTEXT_BYTES;
use serde_json::Value;
use stasis_compiler::frontend::{module_graph::parse_imports, parser::parse_top_level_type_layout};
use std::collections::BTreeMap;
use std::fmt::Write;

pub(super) fn render(sources: &[Value]) -> Result<String, String> {
    let mut files = BTreeMap::<&str, Vec<(usize, &Value)>>::new();
    for (index, item) in sources.iter().enumerate() {
        let file = item["target"]["file"]
            .as_str()
            .ok_or("Missing source file")?;
        files.entry(file).or_default().push((index, item));
    }
    let mut catalog = String::new();
    for (file, items) in files {
        writeln!(catalog, "{}", file.escape_debug()).unwrap();
        let mut last_kind = None;
        let tests = items
            .iter()
            .filter(|(_, item)| item["target"]["kind"] == "test")
            .collect::<Vec<_>>();
        for &(index, item) in &items {
            let target = &item["target"];
            let kind = target["kind"].as_str().ok_or("Missing source kind")?;
            let source = item["source"].as_str().ok_or("Missing source text")?;
            match kind {
                "test" => continue,
                "imports" => {
                    let imports = parse_imports(file, source).map_err(|error| error.message)?;
                    if imports.is_empty() {
                        continue;
                    }
                    writeln!(catalog, "  imports s{index}").unwrap();
                    for import in imports {
                        writeln!(catalog, "    {}", import.path.escape_debug()).unwrap();
                    }
                }
                "globals" => {
                    let layout = parse_top_level_type_layout(source)?;
                    let names = layout
                        .globals
                        .iter()
                        .map(|item| item.name.as_str())
                        .chain(layout.global_blocks.iter().map(|item| item.name.as_str()))
                        .chain(layout.constants.iter().map(|item| item.name.as_str()))
                        .map(|name| name.escape_debug().to_string())
                        .collect::<Vec<_>>();
                    if !names.is_empty() {
                        writeln!(catalog, "  globals/constants s{index}").unwrap();
                        for name in names {
                            writeln!(catalog, "    {name}").unwrap();
                        }
                    }
                }
                "struct" | "function" => {
                    let name = target["name"].as_str().ok_or("Missing source name")?;
                    if last_kind != Some(kind) {
                        writeln!(catalog, "  {kind}s").unwrap();
                        last_kind = Some(kind);
                    }
                    writeln!(catalog, "    s{index} {}", name.escape_debug()).unwrap();
                }
                _ => return Err(format!("Unsupported source kind: {kind}")),
            }
            if catalog.len() > MAX_SOURCE_CONTEXT_BYTES {
                return Err("Project symbol catalog exceeds 256 KiB; narrow the project before requesting edits.".into());
            }
        }
        if let Some((index, _)) = tests.first() {
            writeln!(
                catalog,
                "  tests t{index} ({}; read to list names)",
                tests.len()
            )
            .unwrap();
        }
    }
    if catalog.len() > MAX_SOURCE_CONTEXT_BYTES {
        return Err(
            "Project symbol catalog exceeds 256 KiB; narrow the project before requesting edits."
                .into(),
        );
    }
    Ok(catalog)
}

pub(super) fn test_names(sources: &[Value], index: usize) -> Option<Value> {
    let item = sources.get(index)?;
    if item["target"]["kind"] != "test" {
        return None;
    }
    let file = item["target"]["file"].as_str()?;
    let mut names = String::new();
    for (index, item) in sources
        .iter()
        .enumerate()
        .filter(|(_, item)| item["target"]["file"] == file && item["target"]["kind"] == "test")
    {
        writeln!(
            names,
            "  s{index} {}",
            item["target"]["name"].as_str()?.escape_debug()
        )
        .unwrap();
    }
    Some(serde_json::json!({"file":file, "tests":names}))
}
