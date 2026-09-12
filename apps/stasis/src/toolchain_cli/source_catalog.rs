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
    let mut catalog = String::from("files [id path]\n");
    let mut vendor = BTreeSet::new();
    for (file_id, (file, items)) in files.iter().enumerate() {
        for &(_, item) in items {
            match item["target"]["kind"]
                .as_str()
                .ok_or("Missing source kind")?
            {
                "imports" => {
                    let parsed =
                        parse_imports(file, item["source"].as_str().ok_or("Missing source text")?)
                            .map_err(|error| error.message)?;
                    if file.starts_with("src/") {
                        for import in parsed {
                            if let Some(path) = import.path.strip_prefix("/vendor/") {
                                vendor.insert(path.to_string());
                            }
                        }
                    }
                }
                "globals" | "struct" | "function" | "test" => {}
                kind => return Err(format!("Unsupported source kind: {kind}")),
            }
        }
        let line = format!("  f{file_id} {}", file.escape_debug());
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
    let entry = files
        .iter()
        .enumerate()
        .find(|(_, (_, items))| {
            items.iter().any(|(_, item)| {
                item["target"]["kind"] == "function" && item["target"]["name"] == "main"
            })
        })
        .or_else(|| {
            files
                .iter()
                .enumerate()
                .find(|(_, (file, _))| file.starts_with("src/"))
        });
    if let Some((file_id, (file, items))) = entry {
        push(
            &mut catalog,
            &format!("entry f{file_id} {}", file.escape_debug()),
        );
        for kind in ["imports", "globals", "struct", "function", "test"] {
            let entries = items
                .iter()
                .filter(|(_, item)| item["target"]["kind"] == kind)
                .collect::<Vec<_>>();
            if entries.is_empty() {
                continue;
            }
            let heading = if matches!(kind, "imports" | "globals") {
                format!("  {kind} s{}", entries[0].0)
            } else {
                format!("  {kind}s")
            };
            push(&mut catalog, &heading);
            if kind == "globals" {
                let names = global_names(entries[0].1)?;
                for (position, name) in names.iter().enumerate() {
                    if !push(&mut catalog, &format!("    {}", name.escape_debug())) {
                        push(&mut catalog, &format!("    +{}", names.len() - position));
                        break;
                    }
                }
            } else if !matches!(kind, "imports") {
                for (position, (index, item)) in entries.iter().enumerate() {
                    let name = item["target"]["name"].as_str().unwrap_or_default();
                    if !push(
                        &mut catalog,
                        &format!("    s{index} {}", name.escape_debug()),
                    ) {
                        push(&mut catalog, &format!("    +{}", entries.len() - position));
                        break;
                    }
                }
            }
        }
        let examples = items
            .iter()
            .filter(|(_, item)| {
                item["target"]["kind"] == "function"
                    && matches!(
                        item["target"]["name"].as_str(),
                        Some("main" | "on_code_swap")
                    )
                    && item["source"]
                        .as_str()
                        .is_some_and(|source| source.len() <= 1_200)
            })
            .collect::<Vec<_>>();
        if !examples.is_empty() {
            push(&mut catalog, "examples");
        }
        for (index, item) in examples.into_iter().take(2) {
            push(&mut catalog, &format!("  s{index}"));
            for line in item["source"].as_str().unwrap_or_default().lines() {
                if !push(&mut catalog, &format!("    {line}")) {
                    return Ok(catalog);
                }
            }
        }
    }
    Ok(catalog)
}

fn file_symbols(sources: &[Value], file_id: usize) -> Option<Value> {
    let file = file_for_id(sources, file_id)?;
    let mut listing = String::new();
    for (index, item) in sources
        .iter()
        .enumerate()
        .filter(|(_, item)| item["target"]["file"] == file)
    {
        let kind = item["target"]["kind"].as_str()?;
        if matches!(kind, "imports" | "globals") {
            writeln!(listing, "  s{index} {kind}").unwrap();
        } else {
            let name = item["target"]["name"].as_str()?.escape_debug();
            writeln!(listing, "  s{index} {kind} {name}").unwrap();
        }
    }
    Some(serde_json::json!({"file":file, "symbols":listing}))
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
            let mut result = serde_json::json!({"selector":format!("s{index}"), "file_id":format!("f{file_id}"),
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

pub(super) fn inspect(
    sources: &[Value],
    args: &Value,
    max_source_bytes: usize,
) -> Result<Value, String> {
    let selector = args
        .get("selector")
        .or_else(|| args.get("symbol_id"))
        .and_then(Value::as_str)
        .ok_or_else(|| "selector must be a string".to_string())?;
    if let Some(query) = selector.strip_prefix("search-source ") {
        return search(sources, query, true);
    }
    if let Some(query) = selector.strip_prefix("search ") {
        return search(sources, query, false);
    }
    if let Some(file_id) = selector
        .strip_prefix('f')
        .and_then(|value| value.parse::<usize>().ok())
    {
        return file_symbols(sources, file_id)
            .ok_or_else(|| format!("Unknown source file: f{file_id}"));
    }
    if let Some(query) = selector.strip_prefix("?+") {
        return search(sources, query, true);
    }
    if let Some(query) = selector.strip_prefix('?') {
        return search(sources, query, false);
    }
    if let Some(rest) = selector.strip_prefix("file ") {
        let (file_id, rest) = rest
            .split_once(' ')
            .ok_or_else(|| format!("Unknown source selector: {selector}"))?;
        let file_id = file_id
            .parse::<usize>()
            .map_err(|_| "invalid file selector")?;
        let file = file_for_id(sources, file_id)
            .ok_or_else(|| format!("Unknown source file: {file_id}"))?;
        let (kind, name) = rest.split_once(' ').unwrap_or((rest, ""));
        if kind == "symbols" && name.is_empty() {
            return file_symbols(sources, file_id)
                .ok_or_else(|| format!("Unknown source file: {file_id}"));
        }
        if kind == "tests" && name.is_empty() {
            return sources
                .iter()
                .enumerate()
                .find(|(_, item)| {
                    item["target"]["file"] == file && item["target"]["kind"] == "test"
                })
                .and_then(|(index, _)| test_names(sources, index))
                .ok_or_else(|| format!("Source file {file_id} has no tests"));
        }
        let group_kind = match (kind, name) {
            ("imports", "") => Some("imports"),
            ("globals", "") => Some("globals"),
            _ => None,
        };
        let matches: Vec<Value> = if let Some(kind) = group_kind {
            sources
                .iter()
                .filter(|item| item["target"]["file"] == file && item["target"]["kind"] == kind)
                .cloned()
                .collect()
        } else if matches!(kind, "function" | "struct" | "test" | "tests") && !name.is_empty() {
            find_by_file_name(sources, file_id, name)
                .into_iter()
                .filter(|item| {
                    item["target"]["kind"] == kind
                        || (kind == "tests" && item["target"]["kind"] == "test")
                })
                .collect()
        } else {
            return Err(format!("Unknown source selector: {selector}"));
        };
        return match matches.as_slice() {
            [] => Err(format!("Unknown source selector: {selector}")),
            [item] => Ok(item.clone()),
            _ => Ok(Value::Array(matches)),
        };
    }
    if let Some((file_id, name)) = selector
        .strip_prefix('f')
        .and_then(|rest| rest.split_once(':'))
    {
        let file_id = file_id
            .parse::<usize>()
            .map_err(|_| "invalid file selector")?;
        let file = file_for_id(sources, file_id)
            .ok_or_else(|| format!("Unknown source file: f{file_id}"))?;
        if name == "t" {
            return sources
                .iter()
                .enumerate()
                .find(|(_, item)| {
                    item["target"]["file"] == file && item["target"]["kind"] == "test"
                })
                .and_then(|(index, _)| test_names(sources, index))
                .ok_or_else(|| format!("Source file f{file_id} has no tests"));
        }
        let kind = match name {
            "i" => Some("imports"),
            "g" => Some("globals"),
            _ => None,
        };
        let matches = if let Some(kind) = kind {
            sources
                .iter()
                .filter(|item| item["target"]["file"] == file && item["target"]["kind"] == kind)
                .cloned()
                .collect()
        } else {
            find_by_file_name(sources, file_id, name)
        };
        return match matches.as_slice() {
            [] => Err(format!("Unknown source selector: {selector}")),
            [item] => Ok(item.clone()),
            _ => Ok(Value::Array(matches)),
        };
    }
    let symbol_id = selector
        .strip_prefix("source ")
        .map(|index| format!("s{index}"))
        .unwrap_or_else(|| selector.to_string());
    let symbol_id = symbol_id
        .strip_prefix('@')
        .map(|index| format!("s{index}"))
        .unwrap_or(symbol_id);
    let test_catalog = symbol_id
        .strip_prefix('t')
        .and_then(|index| index.parse::<usize>().ok())
        .filter(|index| symbol_id == format!("t{index}"))
        .and_then(|index| test_names(sources, index));
    let item = sources
        .iter()
        .find(|item| item["target"]["symbol_id"].as_str() == Some(symbol_id.as_str()))
        .or(test_catalog.as_ref())
        .or_else(|| {
            let index = symbol_id.strip_prefix('s')?.parse::<usize>().ok()?;
            (symbol_id == format!("s{index}"))
                .then(|| sources.get(index))
                .flatten()
        })
        .ok_or_else(|| format!("Unknown source symbol: {symbol_id}"))?;
    if serde_json::to_vec(item)
        .map_err(|error| error.to_string())?
        .len()
        > max_source_bytes
    {
        return Err(format!(
            "Source symbol {symbol_id} exceeds the {} KiB read limit.",
            max_source_bytes / 1024
        ));
    }
    Ok(item.clone())
}
