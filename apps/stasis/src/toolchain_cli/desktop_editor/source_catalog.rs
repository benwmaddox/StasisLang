use serde_json::Value;
use stasis_compiler::frontend::{module_graph::parse_imports, parser::parse_top_level_type_layout};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

const MAX_INITIAL_CATALOG_BYTES: usize = 5_000;

fn files(sources: &[Value]) -> Result<BTreeMap<&str, Vec<(usize, &Value)>>, String> {
    let mut files = BTreeMap::<&str, Vec<(usize, &Value)>>::new();
    for (index, item) in sources.iter().enumerate() {
        let file = item["target"]["file"]
            .as_str()
            .ok_or("Missing source file")?;
        files.entry(file).or_default().push((index, item));
    }
    Ok(files)
}

fn global_names(item: &Value) -> Result<Vec<String>, String> {
    let source = item["source"].as_str().ok_or("Missing source text")?;
    let layout = parse_top_level_type_layout(source)?;
    Ok(layout
        .globals
        .iter()
        .map(|item| item.name.clone())
        .chain(layout.global_blocks.iter().map(|item| item.name.clone()))
        .chain(layout.constants.iter().map(|item| item.name.clone()))
        .collect())
}

fn shared_prefix(names: &[&str]) -> String {
    if names.len() < 2 {
        return String::new();
    }
    let Some(first) = names.first() else {
        return String::new();
    };
    let mut bytes = first.len();
    for name in &names[1..] {
        bytes = first
            .as_bytes()
            .iter()
            .zip(name.as_bytes())
            .take(bytes)
            .take_while(|(left, right)| left == right)
            .count();
    }
    while bytes > 0 && !first.is_char_boundary(bytes) {
        bytes -= 1;
    }
    let prefix = &first[..bytes];
    prefix
        .rfind('_')
        .map_or(String::new(), |end| prefix[..=end].to_string())
}

fn push(catalog: &mut String, line: &str) -> bool {
    if catalog.len() + line.len() + 1 > MAX_INITIAL_CATALOG_BYTES {
        return false;
    }
    writeln!(catalog, "{line}").unwrap();
    true
}

pub(super) fn render(sources: &[Value]) -> Result<String, String> {
    let files = files(sources)?;
    let mut catalog = String::from("files\n");
    let mut vendor = BTreeSet::new();
    for (file_id, (file, items)) in files.iter().enumerate() {
        let (mut imports, mut globals, mut structs, mut functions, mut tests) = (0, 0, 0, 0, 0);
        let (mut import_id, mut global_id) = (None, None);
        for &(index, item) in items {
            match item["target"]["kind"]
                .as_str()
                .ok_or("Missing source kind")?
            {
                "imports" => {
                    let parsed =
                        parse_imports(file, item["source"].as_str().ok_or("Missing source text")?)
                            .map_err(|error| error.message)?;
                    imports = parsed.len();
                    import_id = Some(index);
                    if file.starts_with("src/") {
                        for import in parsed {
                            if let Some(path) = import.path.strip_prefix("/vendor/") {
                                vendor.insert(path.to_string());
                            }
                        }
                    }
                }
                "globals" => {
                    globals = global_names(item)?.len();
                    global_id = Some(index);
                }
                "struct" => structs += 1,
                "function" => functions += 1,
                "test" => tests += 1,
                kind => return Err(format!("Unsupported source kind: {kind}")),
            }
        }
        let group = |count: usize, id: Option<usize>, kind: char| match id {
            Some(id) if count > 0 => format!(" {kind}{count}@{id}"),
            _ => format!(" {kind}{count}"),
        };
        let line = format!(
            "  f{file_id} {}{}{} s{structs} f{functions} t{tests}",
            file.escape_debug(),
            group(imports, import_id, 'i'),
            group(globals, global_id, 'g')
        );
        if !push(&mut catalog, &line) {
            push(&mut catalog, &format!("  +{} files", files.len() - file_id));
            return Ok(catalog);
        }
    }
    if !vendor.is_empty() {
        push(&mut catalog, "vendor");
        for path in vendor {
            if !push(&mut catalog, &format!("  {path}")) {
                break;
            }
        }
    }
    push(&mut catalog, "symbols");
    'details: for (file_id, (_, items)) in files.iter().enumerate() {
        for kind in ["struct", "function"] {
            let entries = items
                .iter()
                .filter(|(_, item)| item["target"]["kind"] == kind)
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            let entry_names = entries
                .iter()
                .map(|(_, item)| item["target"]["name"].as_str().unwrap_or_default())
                .collect::<Vec<_>>();
            let prefix = shared_prefix(&entry_names);
            let suffix = if prefix.is_empty() {
                String::new()
            } else {
                format!(" {prefix}")
            };
            if !push(&mut catalog, &format!("  f{file_id} {kind}s{suffix}")) {
                break 'details;
            }
            for (position, name) in entry_names.iter().enumerate() {
                let shown = name
                    .strip_prefix(&prefix)
                    .unwrap_or(name)
                    .escape_debug()
                    .to_string();
                if catalog.len() + shown.len() + 7 > MAX_INITIAL_CATALOG_BYTES {
                    push(
                        &mut catalog,
                        &format!("    +{}", entry_names.len() - position),
                    );
                    break 'details;
                }
                push(&mut catalog, &format!("    {shown}"));
            }
        }
    }
    Ok(catalog)
}

pub(super) fn file_for_id(sources: &[Value], id: usize) -> Option<&str> {
    files(sources).ok()?.keys().nth(id).copied()
}

pub(super) fn find_by_file_name(sources: &[Value], file_id: usize, name: &str) -> Vec<Value> {
    let Some(file) = file_for_id(sources, file_id) else {
        return Vec::new();
    };
    let items = sources
        .iter()
        .filter(|item| item["target"]["file"] == file)
        .collect::<Vec<_>>();
    items
        .into_iter()
        .filter(|item| {
            let Some(candidate) = item["target"]["name"].as_str() else {
                return false;
            };
            let kind = item["target"]["kind"].as_str().unwrap_or_default();
            let names = sources
                .iter()
                .filter(|other| other["target"]["file"] == file && other["target"]["kind"] == kind)
                .filter_map(|other| other["target"]["name"].as_str())
                .collect::<Vec<_>>();
            candidate == name || candidate == format!("{}{name}", shared_prefix(&names))
        })
        .cloned()
        .collect()
}

pub(super) fn search(
    sources: &[Value],
    query: &str,
    include_source: bool,
) -> Result<Value, String> {
    let terms = query
        .split_whitespace()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>();
    let files = files(sources)?;
    let mut matches = Vec::new();
    for (file_id, (file, items)) in files.iter().enumerate() {
        for &(index, item) in items {
            let mut searchable = vec![item["target"]["name"]
                .as_str()
                .unwrap_or_default()
                .to_string()];
            if item["target"]["kind"] == "globals" {
                searchable.extend(global_names(item)?);
            }
            let haystack = searchable.join(" ").to_ascii_lowercase();
            let score = terms
                .iter()
                .filter(|term| haystack.contains(term.as_str()))
                .count();
            if score == 0 {
                continue;
            }
            let mut result = serde_json::json!({"id":format!("@{index}"), "file_id":format!("f{file_id}"),
                "file":file, "kind":item["target"]["kind"], "name":item["target"]["name"]});
            if include_source {
                result["item"] = (*item).clone();
            }
            matches.push((score, result));
        }
    }
    matches.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| a.1.to_string().cmp(&b.1.to_string()))
    });
    Ok(Value::Array(
        matches
            .into_iter()
            .take(if include_source { 8 } else { 32 })
            .map(|(_, value)| value)
            .collect(),
    ))
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
