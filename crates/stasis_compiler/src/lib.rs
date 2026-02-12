#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalCompileOutput {
    pub status: i32,
    pub layout_hash: i32,
    pub file_paths: Vec<String>,
    pub functions: Vec<FunctionMetric>,
    pub errors: Vec<ErrorMetric>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionMetric {
    pub file_index: usize,
    pub ordinal: usize,
    pub id_hash: i32,
    pub sig_hash: i32,
    pub body_hash: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ErrorMetric {
    pub code: i32,
    pub pos: i32,
    pub detail_a: i32,
    pub detail_b: i32,
}

#[derive(Debug, Clone)]
struct FileState {
    source_hash: u64,
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
}

#[derive(Debug, Clone)]
struct ParsedFunction {
    ordinal: usize,
    id_hash: i32,
    sig_hash: i32,
    body_hash: i32,
}

#[derive(Debug, Clone)]
struct AnalysisResult {
    functions: Vec<ParsedFunction>,
    layout_hash: i32,
    main_decl_count: i32,
    main_valid_count: i32,
    main_invalid_count: i32,
    parse_errors: Vec<ErrorMetric>,
}

pub struct IncrementalCompilerHost {
    source_hash_by_path: BTreeMap<String, u64>,
    state_by_path: BTreeMap<String, FileState>,
    last_layout_hash_i32: i32,
}

impl IncrementalCompilerHost {
    pub fn new() -> Self {
        Self {
            source_hash_by_path: BTreeMap::new(),
            state_by_path: BTreeMap::new(),
            last_layout_hash_i32: 0,
        }
    }

    pub fn compile_changed_files(
        &mut self,
        changed_files: &[PathBuf],
    ) -> Result<IncrementalCompileOutput, String> {
        if changed_files.is_empty() {
            return Err("compile request had no changed files".to_string());
        }

        let mut files = changed_files.to_vec();
        files.sort();
        files.dedup();

        let mut changed_sources: Vec<(String, String)> = Vec::new();
        for path in files {
            let bytes = fs::read(&path)
                .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
            let source = String::from_utf8_lossy(&bytes).to_string();
            let path_key = normalize_path_key(&path);
            let source_hash = hash_text(&source);
            let changed = self
                .source_hash_by_path
                .get(&path_key)
                .is_none_or(|existing| *existing != source_hash);
            self.source_hash_by_path.insert(path_key.clone(), source_hash);
            if changed {
                changed_sources.push((path_key, source));
            }
        }

        if changed_sources.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 0,
                layout_hash: self.last_layout_hash_i32,
                file_paths: Vec::new(),
                functions: Vec::new(),
                errors: Vec::new(),
            });
        }

        let mut file_paths = Vec::with_capacity(changed_sources.len());
        let mut functions: Vec<FunctionMetric> = Vec::new();
        let mut errors: Vec<ErrorMetric> = Vec::new();
        let mut analyzed_by_path: BTreeMap<String, AnalysisResult> = BTreeMap::new();

        for (path_key, source) in &changed_sources {
            let analyzed = analyze_source(source);
            if !analyzed.parse_errors.is_empty() {
                errors.extend(analyzed.parse_errors.clone());
            }
            analyzed_by_path.insert(path_key.clone(), analyzed);
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                file_paths,
                functions,
                errors,
            });
        }

        for (path_key, analyzed) in &analyzed_by_path {
            let source_hash = *self
                .source_hash_by_path
                .get(path_key)
                .ok_or_else(|| format!("internal error: missing source hash for {path_key}"))?;
            self.state_by_path.insert(
                path_key.clone(),
                FileState {
                    source_hash,
                    layout_hash: analyzed.layout_hash,
                    main_decl_count: analyzed.main_decl_count,
                    main_valid_count: analyzed.main_valid_count,
                    main_invalid_count: analyzed.main_invalid_count,
                },
            );
        }

        let mut main_decl_total = 0;
        let mut main_valid_total = 0;
        let mut main_invalid_total = 0;
        for state in self.state_by_path.values() {
            main_decl_total += state.main_decl_count;
            main_valid_total += state.main_valid_count;
            main_invalid_total += state.main_invalid_count;
        }

        if main_decl_total == 0 {
            errors.push(ErrorMetric {
                code: 41,
                pos: 0,
                detail_a: 0,
                detail_b: 0,
            });
        } else if main_decl_total > 1 {
            errors.push(ErrorMetric {
                code: 43,
                pos: 0,
                detail_a: main_decl_total,
                detail_b: 0,
            });
        } else if main_valid_total != 1 || main_invalid_total > 0 {
            errors.push(ErrorMetric {
                code: 42,
                pos: 0,
                detail_a: main_valid_total,
                detail_b: main_invalid_total,
            });
        }

        if !errors.is_empty() {
            return Ok(IncrementalCompileOutput {
                status: 2,
                layout_hash: self.last_layout_hash_i32,
                file_paths,
                functions,
                errors,
            });
        }

        for (file_index, (path_key, _)) in changed_sources.iter().enumerate() {
            file_paths.push(path_key.clone());
            if let Some(analyzed) = analyzed_by_path.get(path_key) {
                for parsed in &analyzed.functions {
                    functions.push(FunctionMetric {
                        file_index,
                        ordinal: parsed.ordinal,
                        id_hash: parsed.id_hash,
                        sig_hash: parsed.sig_hash,
                        body_hash: parsed.body_hash,
                    });
                }
            }
        }

        let mut layout_acc = 216613626_i32;
        for state in self.state_by_path.values() {
            layout_acc = hash_mix(layout_acc, state.layout_hash);
            layout_acc = hash_mix(layout_acc, (state.source_hash & 0xffff_ffff) as i32);
        }
        self.last_layout_hash_i32 = layout_acc;

        Ok(IncrementalCompileOutput {
            status: 0,
            layout_hash: layout_acc,
            file_paths,
            functions,
            errors,
        })
    }

    pub fn backend_name(&self) -> &'static str {
        "native-rust"
    }
}

impl Default for IncrementalCompilerHost {
    fn default() -> Self {
        Self::new()
    }
}

fn analyze_source(source: &str) -> AnalysisResult {
    let sanitized = sanitize_for_scan(source);
    let mut functions = Vec::new();
    let mut parse_errors = Vec::new();

    let mut index = 0usize;
    let bytes = sanitized.as_bytes();
    let mut ordinal = 0usize;
    let mut main_decl_count = 0;
    let mut main_valid_count = 0;
    let mut main_invalid_count = 0;

    while index < bytes.len() {
        if !keyword_at(&sanitized, index, "function") {
            index += 1;
            continue;
        }
        index += "function".len();

        skip_ws(&sanitized, &mut index);
        while index < bytes.len() && bytes[index] == b'@' {
            index += 1;
            consume_ident(&sanitized, &mut index);
            skip_ws(&sanitized, &mut index);
        }

        let name_start = index;
        if !consume_ident(&sanitized, &mut index) {
            parse_errors.push(ErrorMetric {
                code: 2003,
                pos: name_start as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        }
        let name = &sanitized[name_start..index];

        skip_ws(&sanitized, &mut index);
        if !consume_char(&sanitized, &mut index, '(') {
            parse_errors.push(ErrorMetric {
                code: 2004,
                pos: index as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        }
        let params_start = index;
        let Some(params_end) = find_matching(&sanitized, params_start - 1, '(', ')') else {
            parse_errors.push(ErrorMetric {
                code: 2005,
                pos: params_start as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        };
        index = params_end + 1;

        skip_ws(&sanitized, &mut index);
        if !consume_char(&sanitized, &mut index, ':') {
            parse_errors.push(ErrorMetric {
                code: 2006,
                pos: index as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        }
        skip_ws(&sanitized, &mut index);
        let return_start = index;
        while index < bytes.len() {
            let ch = bytes[index] as char;
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '[' || ch == ']' {
                index += 1;
            } else {
                break;
            }
        }
        if return_start == index {
            parse_errors.push(ErrorMetric {
                code: 2003,
                pos: return_start as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        }
        let return_type = &sanitized[return_start..index];

        skip_ws(&sanitized, &mut index);
        if !consume_char(&sanitized, &mut index, '{') {
            parse_errors.push(ErrorMetric {
                code: 2008,
                pos: index as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        }
        let body_open = index - 1;
        let Some(body_close) = find_matching(&sanitized, body_open, '{', '}') else {
            parse_errors.push(ErrorMetric {
                code: 2009,
                pos: body_open as i32,
                detail_a: 0,
                detail_b: 0,
            });
            break;
        };
        index = body_close + 1;

        let signature_text = format!("{name}({}):{return_type}", sanitized[params_start..params_end].trim());
        let body_text = &sanitized[body_open + 1..body_close];

        if name == "main" {
            main_decl_count += 1;
            if sanitized[params_start..params_end].trim().is_empty() && return_type == "i32" {
                main_valid_count += 1;
            } else {
                main_invalid_count += 1;
            }
        }

        functions.push(ParsedFunction {
            ordinal,
            id_hash: hash_identifier(name),
            sig_hash: hash_i32(&signature_text),
            body_hash: hash_i32(body_text),
        });
        ordinal += 1;
    }

    let layout_hash = compute_layout_hash(&sanitized);

    AnalysisResult {
        functions,
        layout_hash,
        main_decl_count,
        main_valid_count,
        main_invalid_count,
        parse_errors,
    }
}

fn compute_layout_hash(sanitized: &str) -> i32 {
    let mut hash = 216613626_i32;
    let bytes = sanitized.as_bytes();
    let mut index = 0usize;

    while index < bytes.len() {
        if keyword_at(sanitized, index, "global") {
            hash = hash_mix(hash, hash_i32("global"));
            index += "global".len();
            skip_ws(sanitized, &mut index);
            let start = index;
            consume_ident(sanitized, &mut index);
            hash = hash_mix(hash, hash_i32(&sanitized[start..index]));
            skip_ws(sanitized, &mut index);
            if index < bytes.len() && bytes[index] == b'{' {
                if let Some(close) = find_matching(sanitized, index, '{', '}') {
                    hash = hash_mix(hash, hash_i32(&sanitized[index + 1..close]));
                    index = close + 1;
                    continue;
                }
            }
            while index < bytes.len() && bytes[index] != b';' && bytes[index] != b'\n' {
                index += 1;
            }
            hash = hash_mix(hash, hash_i32(&sanitized[start..index]));
            continue;
        }
        if keyword_at(sanitized, index, "struct") {
            hash = hash_mix(hash, hash_i32("struct"));
            index += "struct".len();
            skip_ws(sanitized, &mut index);
            let start = index;
            consume_ident(sanitized, &mut index);
            hash = hash_mix(hash, hash_i32(&sanitized[start..index]));
            skip_ws(sanitized, &mut index);
            if index < bytes.len() && bytes[index] == b'{' {
                if let Some(close) = find_matching(sanitized, index, '{', '}') {
                    hash = hash_mix(hash, hash_i32(&sanitized[index + 1..close]));
                    index = close + 1;
                    continue;
                }
            }
            continue;
        }
        index += 1;
    }

    hash
}

fn keyword_at(source: &str, start: usize, keyword: &str) -> bool {
    let bytes = source.as_bytes();
    let kw = keyword.as_bytes();
    if kw.is_empty() || start + kw.len() > bytes.len() {
        return false;
    }
    if &bytes[start..start + kw.len()] != kw {
        return false;
    }
    let left_ok = start == 0 || !is_ident(bytes[start - 1]);
    let right_ok = start + kw.len() == bytes.len() || !is_ident(bytes[start + kw.len()]);
    left_ok && right_ok
}

fn find_matching(source: &str, open_index: usize, open: char, close: char) -> Option<usize> {
    let bytes = source.as_bytes();
    if open_index >= bytes.len() || bytes[open_index] != open as u8 {
        return None;
    }
    let mut depth = 0i32;
    let mut i = open_index;
    while i < bytes.len() {
        let c = bytes[i] as char;
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
        i += 1;
    }
    None
}

fn consume_char(source: &str, index: &mut usize, expected: char) -> bool {
    let bytes = source.as_bytes();
    if *index < bytes.len() && bytes[*index] == expected as u8 {
        *index += 1;
        return true;
    }
    false
}

fn consume_ident(source: &str, index: &mut usize) -> bool {
    let bytes = source.as_bytes();
    if *index >= bytes.len() || !is_ident_start(bytes[*index]) {
        return false;
    }
    *index += 1;
    while *index < bytes.len() && is_ident(bytes[*index]) {
        *index += 1;
    }
    true
}

fn skip_ws(source: &str, index: &mut usize) {
    let bytes = source.as_bytes();
    while *index < bytes.len() {
        if (bytes[*index] as char).is_ascii_whitespace() {
            *index += 1;
        } else {
            break;
        }
    }
}

fn is_ident_start(b: u8) -> bool {
    (b as char).is_ascii_alphabetic() || b == b'_'
}

fn is_ident(b: u8) -> bool {
    (b as char).is_ascii_alphanumeric() || b == b'_'
}

fn sanitize_for_scan(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;

    while i < bytes.len() {
        if bytes[i] == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            while i < bytes.len() && bytes[i] != b'\n' {
                out.push(' ');
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'\n' {
                out.push('\n');
                i += 1;
            }
            continue;
        }

        if bytes[i] == b'"' {
            out.push('"');
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    out.push(' ');
                    i += 1;
                    if i < bytes.len() {
                        out.push(' ');
                        i += 1;
                    }
                    continue;
                }
                if bytes[i] == b'"' {
                    out.push('"');
                    i += 1;
                    break;
                }
                out.push(' ');
                i += 1;
            }
            continue;
        }

        out.push(bytes[i] as char);
        i += 1;
    }

    out
}

fn normalize_path_key(path: &Path) -> String {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .map(|cwd| cwd.join(path))
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let mut text = absolute.to_string_lossy().replace('\\', "/");
    if let Some(stripped) = text.strip_prefix("//?/") {
        text = stripped.to_string();
    }
    text
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn hash_mix(hash: i32, value: i32) -> i32 {
    hash.wrapping_mul(16777619).wrapping_add(value.wrapping_add(1))
}

fn hash_i32(value: &str) -> i32 {
    let mut hash: i32 = 216613626;
    for byte in value.bytes() {
        hash = hash_mix(hash, i32::from(byte));
    }
    hash
}

fn hash_identifier(name: &str) -> i32 {
    hash_i32(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backend_name_is_native_rust() {
        let host = IncrementalCompilerHost::new();
        assert_eq!(host.backend_name(), "native-rust");
    }

    #[test]
    fn parse_main_signature_validation() {
        let ok = analyze_source("function main(): i32 { return 0; }");
        assert_eq!(ok.main_decl_count, 1);
        assert_eq!(ok.main_valid_count, 1);
        assert_eq!(ok.main_invalid_count, 0);

        let bad = analyze_source("function main(x: i32): i32 { return x; }");
        assert_eq!(bad.main_decl_count, 1);
        assert_eq!(bad.main_valid_count, 0);
        assert_eq!(bad.main_invalid_count, 1);
    }

    #[test]
    fn parse_records_functions_and_hashes() {
        let analyzed = analyze_source(
            "function main(): i32 { return 0; }\nfunction on_code_swap(): void { return; }",
        );
        assert_eq!(analyzed.functions.len(), 2);
        assert_eq!(analyzed.functions[1].id_hash, -663_287_521);
    }

    #[test]
    fn compile_empty_change_set_is_error() {
        let mut host = IncrementalCompilerHost::new();
        let err = host.compile_changed_files(&[]).expect_err("expected error");
        assert!(err.contains("no changed files"));
    }
}
