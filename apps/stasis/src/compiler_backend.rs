use stasis_runner::swap::contracts::{
    CompileRequest, CompileResult, Diagnostic, DiagnosticSeverity, FnId, FunctionPatch,
    FunctionPatchSet, LayoutHash,
};
use stasis_runner::swap::pipeline::CompilerBackend;
use std::collections::BTreeMap;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

const METRIC_STATUS: &str = "INC_STATUS=";
const METRIC_LAYOUT: &str = "INC_LAYOUT_HASH=";
const METRIC_FILE: &str = "INC_FILE_PATH=";
const METRIC_FN: &str = "INC_FN=";
const METRIC_ERR: &str = "INC_ERR=";
const BRIDGE_READY: &str = "BRIDGE_READY";
const BRIDGE_BEGIN: &str = "BRIDGE_BEGIN";
const BRIDGE_END: &str = "BRIDGE_END";
const BRIDGE_PROTOCOL_ERROR: &str = "BRIDGE_PROTOCOL_ERROR";

#[derive(serde::Serialize)]
struct BridgeCompileCommand<'a> {
    op: &'a str,
    request_id: u64,
    path: &'a str,
}

#[derive(serde::Serialize)]
struct BridgeQuitCommand<'a> {
    op: &'a str,
    request_id: u64,
}

struct BridgeSession {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

#[derive(Debug, Clone)]
struct IncrementalOutput {
    status: i32,
    layout_hash: i32,
    file_paths: Vec<String>,
    functions: Vec<FunctionMetric>,
    errors: Vec<ErrorMetric>,
}

#[derive(Debug, Clone)]
struct FunctionMetric {
    file_index: usize,
    ordinal: usize,
    id_hash: i32,
    sig_hash: i32,
    body_hash: i32,
}

#[derive(Debug, Clone)]
struct ErrorMetric {
    code: i32,
    pos: i32,
    detail_a: i32,
    detail_b: i32,
}

pub struct IncrementalCompilerBackend {
    repo_root: PathBuf,
    cli_exe_path: PathBuf,
    aot_helper_path: PathBuf,
    stable_temp_dir: PathBuf,
    clang_bin_dir: Option<PathBuf>,
    incremental_compiler_path: PathBuf,
    source_hash_by_path: BTreeMap<String, u64>,
    last_layout_hash_i32: i32,
    fn_id_by_signature: BTreeMap<String, FnId>,
    next_fn_id: u32,
    bridge: Option<BridgeSession>,
    next_bridge_request_id: u64,
}

impl IncrementalCompilerBackend {
    pub fn new() -> Self {
        let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let cli_exe_path = resolve_cli_exe_path(&repo_root);
        let aot_helper_path = repo_root
            .join("tools")
            .join("cranelift-aot")
            .join("target")
            .join("debug")
            .join("stasis-cranelift-aot.exe");
        let stable_temp_dir = repo_root.join(".stasis_cache").join("tmp");
        let incremental_compiler_path = repo_root
            .join("compiler")
            .join("incremental_compiler.stasis");
        Self {
            repo_root,
            cli_exe_path,
            aot_helper_path,
            stable_temp_dir,
            clang_bin_dir: detect_clang_bin_dir(),
            incremental_compiler_path,
            source_hash_by_path: BTreeMap::new(),
            last_layout_hash_i32: 0,
            fn_id_by_signature: BTreeMap::new(),
            next_fn_id: 1,
            bridge: None,
            next_bridge_request_id: 1,
        }
    }

    fn compile_request(&mut self, request: &CompileRequest) -> Result<IncrementalOutput, String> {
        if !cfg!(windows) {
            return Err("incremental compiler adapter currently supports Windows only".to_string());
        }
        if !self.cli_exe_path.exists() {
            return Err(format!(
                "missing CLI executable at {}",
                self.cli_exe_path.display()
            ));
        }
        if !self.incremental_compiler_path.exists() {
            return Err(format!(
                "missing incremental compiler source at {}",
                self.incremental_compiler_path.display()
            ));
        }
        if !self.aot_helper_path.exists() {
            return Err(format!(
                "missing Cranelift AOT helper at {}",
                self.aot_helper_path.display()
            ));
        }
        fs::create_dir_all(&self.stable_temp_dir).map_err(|error| {
            format!(
                "failed to create stable temp directory {}: {error}",
                self.stable_temp_dir.display()
            )
        })?;
        if request.changed_files.is_empty() {
            return Err("compile request had no changed files".to_string());
        }

        let mut files = request.changed_files.clone();
        files.sort();
        files.dedup();

        let mut sources = Vec::with_capacity(files.len());
        for path in files {
            let bytes = fs::read(&path)
                .map_err(|error| format!("failed reading {}: {error}", path.display()))?;
            let source = String::from_utf8_lossy(&bytes).to_string();
            let source = preprocess_for_incremental(&source);
            sources.push((normalize_path_key(&path), source));
        }

        let mut changed_sources = Vec::with_capacity(sources.len());
        for (path, source) in sources {
            let source_hash = hash_text(&source);
            let changed = self
                .source_hash_by_path
                .get(&path)
                .is_none_or(|existing| *existing != source_hash);
            self.source_hash_by_path.insert(path.clone(), source_hash);
            if changed {
                changed_sources.push((path, source));
            }
        }

        if changed_sources.is_empty() {
            return Ok(IncrementalOutput {
                status: 0,
                layout_hash: self.last_layout_hash_i32,
                file_paths: Vec::new(),
                functions: Vec::new(),
                errors: Vec::new(),
            });
        }

        let harness_path = write_harness_file(
            &self.repo_root,
            &self.incremental_compiler_path,
            &changed_sources,
        )?;
        let result = self.compile_harness_via_bridge(&harness_path).or_else(|first_error| {
            self.reset_bridge();
            self.compile_harness_via_bridge(&harness_path).map_err(|retry_error| {
                format!(
                    "incremental bridge compile failed after restart.\nfirst: {first_error}\nretry: {retry_error}"
                )
            })
        });
        let _ = fs::remove_file(&harness_path);
        result
    }

    fn fn_id_for_key(&mut self, key: &str) -> Result<FnId, String> {
        if let Some(existing) = self.fn_id_by_signature.get(key) {
            return Ok(*existing);
        }
        if self.next_fn_id == u32::MAX {
            return Err("function id space exhausted".to_string());
        }
        let next = FnId(self.next_fn_id);
        self.next_fn_id += 1;
        self.fn_id_by_signature.insert(key.to_string(), next);
        Ok(next)
    }

    fn compile_harness_via_bridge(
        &mut self,
        harness_path: &Path,
    ) -> Result<IncrementalOutput, String> {
        let request_id = self.next_bridge_request_id;
        self.next_bridge_request_id = self.next_bridge_request_id.wrapping_add(1).max(1);
        let harness_key = normalize_path_key(harness_path);

        let command = BridgeCompileCommand {
            op: "compile",
            request_id,
            path: &harness_key,
        };
        let payload = serde_json::to_string(&command)
            .map_err(|error| format!("failed to serialize bridge compile command: {error}"))?;

        let session = self.ensure_bridge_session()?;
        session
            .stdin
            .write_all(payload.as_bytes())
            .and_then(|_| session.stdin.write_all(b"\n"))
            .and_then(|_| session.stdin.flush())
            .map_err(|error| format!("failed writing compile command to bridge: {error}"))?;

        let mut in_request = false;
        let mut collected = String::new();
        loop {
            let line = read_bridge_line(&mut session.stdout)?;
            if !in_request {
                if let Some(begin_id) = parse_bridge_begin(&line) {
                    if begin_id == request_id {
                        in_request = true;
                    }
                }
                continue;
            }

            if let Some((end_id, exit_code)) = parse_bridge_end(&line) {
                if end_id != request_id {
                    continue;
                }
                if exit_code != 0 {
                    return Err(format!(
                        "bridge compile returned exit code {exit_code}.\noutput:\n{collected}"
                    ));
                }
                return parse_incremental_output(&collected, "");
            }

            if line.starts_with(BRIDGE_PROTOCOL_ERROR) {
                return Err(format!(
                    "bridge protocol error during compile request {request_id}: {line}"
                ));
            }

            collected.push_str(&line);
            collected.push('\n');
        }
    }

    fn ensure_bridge_session(&mut self) -> Result<&mut BridgeSession, String> {
        if self.bridge.is_none() {
            self.bridge = Some(self.spawn_bridge_session()?);
        }
        self.bridge
            .as_mut()
            .ok_or_else(|| "internal error: bridge session missing after spawn".to_string())
    }

    fn spawn_bridge_session(&self) -> Result<BridgeSession, String> {
        let mut command = Command::new(&self.cli_exe_path);
        command
            .arg("bridge")
            .env("STASIS_CRANELIFT_AOT", &self.aot_helper_path)
            .env("STASIS_TEMP_DIR", &self.stable_temp_dir)
            .current_dir(&self.repo_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some(clang_bin_dir) = &self.clang_bin_dir {
            let mut path_value = clang_bin_dir.to_string_lossy().into_owned();
            path_value.push(';');
            path_value.push_str(&std::env::var("PATH").unwrap_or_default());
            command.env("PATH", path_value);
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("failed to launch bridge process: {error}"))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "bridge process missing stdin pipe".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "bridge process missing stdout pipe".to_string())?;
        let mut stdout = BufReader::new(stdout);

        let mut startup = String::new();
        loop {
            let line = read_bridge_line(&mut stdout)?;
            if line.starts_with(BRIDGE_READY) {
                break;
            }
            startup.push_str(&line);
            startup.push('\n');
            if startup.len() > 16 * 1024 {
                return Err(format!(
                    "bridge startup did not emit {BRIDGE_READY}. output:\n{startup}"
                ));
            }
        }

        Ok(BridgeSession {
            child,
            stdin,
            stdout,
        })
    }

    fn reset_bridge(&mut self) {
        if let Some(mut session) = self.bridge.take() {
            let quit = BridgeQuitCommand {
                op: "quit",
                request_id: 0,
            };
            if let Ok(payload) = serde_json::to_string(&quit) {
                let _ = session.stdin.write_all(payload.as_bytes());
                let _ = session.stdin.write_all(b"\n");
                let _ = session.stdin.flush();
            }
            let _ = session.child.kill();
            let _ = session.child.wait();
        }
    }
}

fn resolve_cli_exe_path(repo_root: &Path) -> PathBuf {
    if let Ok(override_path) = std::env::var("STASIS_BOOTSTRAP_CLI") {
        let path = PathBuf::from(override_path);
        if path.exists() {
            return path;
        }
    }

    let release = repo_root
        .join("Stasis.Cli")
        .join("bin")
        .join("Release")
        .join("net9.0")
        .join("Stasis.Cli.exe");
    if release.exists() {
        return release;
    }

    let debug = repo_root
        .join("Stasis.Cli")
        .join("bin")
        .join("Debug")
        .join("net9.0")
        .join("Stasis.Cli.exe");
    if debug.exists() {
        return debug;
    }

    repo_root
        .join("bootstrap")
        .join("windows")
        .join("stasis-cli")
        .join("Stasis.Cli.exe")
}

impl Drop for IncrementalCompilerBackend {
    fn drop(&mut self) {
        self.reset_bridge();
    }
}

fn detect_clang_bin_dir() -> Option<PathBuf> {
    let llvm_clang = PathBuf::from(r"C:\Program Files\LLVM\bin\clang.exe");
    if llvm_clang.exists() {
        return llvm_clang.parent().map(Path::to_path_buf);
    }
    let vs_clang = PathBuf::from(
        r"C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\Llvm\x64\bin\clang.exe",
    );
    if vs_clang.exists() {
        return vs_clang.parent().map(Path::to_path_buf);
    }
    None
}

impl Default for IncrementalCompilerBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CompilerBackend for IncrementalCompilerBackend {
    fn compile(&mut self, request: CompileRequest) -> CompileResult {
        let parsed = match self.compile_request(&request) {
            Ok(result) => result,
            Err(message) => {
                return CompileResult::failed(
                    request.request_id,
                    vec![Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        message,
                        path: request.changed_files.first().cloned(),
                        line: None,
                        column: None,
                    }],
                );
            }
        };

        if parsed.status != 0 {
            let mut diagnostics = Vec::new();
            if parsed.errors.is_empty() {
                diagnostics.push(Diagnostic {
                    severity: DiagnosticSeverity::Error,
                    message: format!("incremental compiler failed with status {}", parsed.status),
                    path: request.changed_files.first().cloned(),
                    line: None,
                    column: None,
                });
            } else {
                for error in parsed.errors {
                    diagnostics.push(Diagnostic {
                        severity: DiagnosticSeverity::Error,
                        message: format_error_message(error.code, error.detail_a, error.detail_b),
                        path: None,
                        line: None,
                        column: Some(error.pos.max(0) as u32),
                    });
                }
            }
            return CompileResult::failed(request.request_id, diagnostics);
        }

        let mut functions = Vec::new();
        let mut hook_present = false;
        for metric in parsed.functions {
            let path = parsed
                .file_paths
                .get(metric.file_index)
                .cloned()
                .unwrap_or_else(|| "<unknown>".to_string());
            let key = format!(
                "{path}::{}::{}::{}::{}",
                metric.ordinal, metric.id_hash, metric.sig_hash, metric.body_hash
            );
            let fn_id = match self.fn_id_for_key(&key) {
                Ok(id) => id,
                Err(message) => {
                    return CompileResult::failed(
                        request.request_id,
                        vec![Diagnostic {
                            severity: DiagnosticSeverity::Error,
                            message,
                            path: None,
                            line: None,
                            column: None,
                        }],
                    );
                }
            };
            if metric.id_hash == hash_identifier("on_code_swap") {
                hook_present = true;
            }
            functions.push(FunctionPatch { fn_id });
        }

        self.last_layout_hash_i32 = parsed.layout_hash;
        let layout_hash = expand_layout_hash(parsed.layout_hash);
        let hook_symbol = if hook_present {
            Some("on_code_swap".to_string())
        } else {
            None
        };
        CompileResult::success_with_hook_symbol(
            request.request_id,
            layout_hash,
            FunctionPatchSet { functions },
            hook_symbol,
        )
    }
}

fn write_harness_file(
    repo_root: &Path,
    incremental_compiler_path: &Path,
    files: &[(String, String)],
) -> Result<PathBuf, String> {
    let cache_dir = repo_root.join(".stasis_cache");
    fs::create_dir_all(&cache_dir)
        .map_err(|error| format!("failed to create {}: {error}", cache_dir.display()))?;

    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("clock error: {error}"))?
        .as_nanos();
    let harness_path = cache_dir.join(format!("incremental_backend_{stamp}.stasis"));
    let import_path = normalize_path_key(incremental_compiler_path);

    let mut program = String::new();
    program.push_str(&format!(
        "import \"{}\";\n\n",
        escape_stasis_string(&import_path)
    ));
    program.push_str("function print_metric(name: ascii[], value: i32): void {\n");
    program.push_str("    print_string(name);\n");
    program.push_str("    print_int(value);\n");
    program.push_str("    print_char(10);\n");
    program.push_str("}\n\n");
    program.push_str("function main(): i32 {\n");
    program.push_str("    compiler_reset_state();\n");
    for (path, source) in files {
        if source.contains('"') {
            return Err("preprocessed source still contains quote character".to_string());
        }
        program.push_str(&format!(
            "    if (!compiler_upsert_file(\"{}\", \"{}\")) {{ return 90; }}\n",
            escape_stasis_string(path),
            source
        ));
    }
    program.push_str("    let status: i32 = run_incremental_compiler_with_main_entry();\n");
    program.push_str("    print_metric(\"INC_STATUS=\", status);\n");
    program.push_str("    print_metric(\"INC_LAYOUT_HASH=\", Compiler.layout_hash);\n");
    program.push_str("    print_metric(\"INC_FILE_COUNT=\", Compiler.file_count);\n");
    program.push_str("    print_metric(\"INC_ERROR_COUNT=\", Compiler.error_count);\n");
    program.push_str("    let i: i32 = 0;\n");
    program.push_str("    for (i = 0; i < Compiler.file_count; i = i + 1) {\n");
    program.push_str("        print_string(\"INC_FILE_PATH=\");\n");
    program.push_str("        print_int(i);\n");
    program.push_str("        print_char(44);\n");
    program.push_str("        print_string(Compiler.files[i].path);\n");
    program.push_str("        print_char(10);\n");
    program.push_str("        let j: i32 = 0;\n");
    program.push_str(
        "        for (j = 0; j < Compiler.files[i].tracked_function_count; j = j + 1) {\n",
    );
    program.push_str("            print_string(\"INC_FN=\");\n");
    program.push_str("            print_int(i);\n");
    program.push_str("            print_char(44);\n");
    program.push_str("            print_int(j);\n");
    program.push_str("            print_char(44);\n");
    program.push_str("            print_int(Compiler.files[i].function_id_hashes[j]);\n");
    program.push_str("            print_char(44);\n");
    program.push_str("            print_int(Compiler.files[i].function_sig_hashes[j]);\n");
    program.push_str("            print_char(44);\n");
    program.push_str("            print_int(Compiler.files[i].function_body_hashes[j]);\n");
    program.push_str("            print_char(10);\n");
    program.push_str("        }\n");
    program.push_str("    }\n");
    program.push_str("    for (i = 0; i < Compiler.error_count; i = i + 1) {\n");
    program.push_str("        print_string(\"INC_ERR=\");\n");
    program.push_str("        print_int(Compiler.errors[i].code);\n");
    program.push_str("        print_char(44);\n");
    program.push_str("        print_int(Compiler.errors[i].pos);\n");
    program.push_str("        print_char(44);\n");
    program.push_str("        print_int(Compiler.errors[i].detail_a);\n");
    program.push_str("        print_char(44);\n");
    program.push_str("        print_int(Compiler.errors[i].detail_b);\n");
    program.push_str("        print_char(10);\n");
    program.push_str("    }\n");
    program.push_str("    return status;\n");
    program.push_str("}\n");

    fs::write(&harness_path, program)
        .map_err(|error| format!("failed writing {}: {error}", harness_path.display()))?;
    Ok(harness_path)
}

fn read_bridge_line(reader: &mut BufReader<ChildStdout>) -> Result<String, String> {
    let mut line = String::new();
    let read = reader
        .read_line(&mut line)
        .map_err(|error| format!("failed reading bridge output: {error}"))?;
    if read == 0 {
        return Err("bridge process closed output stream unexpectedly".to_string());
    }
    Ok(line.trim_end_matches(&['\r', '\n'][..]).to_string())
}

fn parse_bridge_begin(line: &str) -> Option<u64> {
    let rest = line.strip_prefix(BRIDGE_BEGIN)?.trim();
    rest.parse::<u64>().ok()
}

fn parse_bridge_end(line: &str) -> Option<(u64, i32)> {
    let rest = line.strip_prefix(BRIDGE_END)?.trim();
    let mut parts = rest.split_whitespace();
    let request_id = parts.next()?.parse::<u64>().ok()?;
    let exit_code = parts.next()?.parse::<i32>().ok()?;
    Some((request_id, exit_code))
}

fn parse_incremental_output(stdout: &str, stderr: &str) -> Result<IncrementalOutput, String> {
    let mut status: Option<i32> = None;
    let mut layout_hash: Option<i32> = None;
    let mut file_paths: BTreeMap<usize, String> = BTreeMap::new();
    let mut functions = Vec::new();
    let mut errors = Vec::new();

    for line in stdout.lines().chain(stderr.lines()) {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix(METRIC_STATUS) {
            status = rest.trim().parse::<i32>().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix(METRIC_LAYOUT) {
            layout_hash = rest.trim().parse::<i32>().ok();
            continue;
        }
        if let Some(rest) = line.strip_prefix(METRIC_FILE) {
            let mut parts = rest.splitn(2, ',');
            let index = parts.next().and_then(|v| v.trim().parse::<usize>().ok());
            let path = parts.next().map(str::trim);
            if let (Some(index), Some(path)) = (index, path) {
                file_paths.insert(index, path.to_string());
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(METRIC_FN) {
            let parts: Vec<_> = rest.split(',').collect();
            if parts.len() == 5 {
                let parsed = (
                    parts[0].trim().parse::<usize>(),
                    parts[1].trim().parse::<usize>(),
                    parts[2].trim().parse::<i32>(),
                    parts[3].trim().parse::<i32>(),
                    parts[4].trim().parse::<i32>(),
                );
                if let (Ok(file_index), Ok(ordinal), Ok(id_hash), Ok(sig_hash), Ok(body_hash)) =
                    parsed
                {
                    functions.push(FunctionMetric {
                        file_index,
                        ordinal,
                        id_hash,
                        sig_hash,
                        body_hash,
                    });
                }
            }
            continue;
        }
        if let Some(rest) = line.strip_prefix(METRIC_ERR) {
            let parts: Vec<_> = rest.split(',').collect();
            if parts.len() == 4 {
                let parsed = (
                    parts[0].trim().parse::<i32>(),
                    parts[1].trim().parse::<i32>(),
                    parts[2].trim().parse::<i32>(),
                    parts[3].trim().parse::<i32>(),
                );
                if let (Ok(code), Ok(pos), Ok(detail_a), Ok(detail_b)) = parsed {
                    errors.push(ErrorMetric {
                        code,
                        pos,
                        detail_a,
                        detail_b,
                    });
                }
            }
        }
    }

    let status = status.ok_or_else(|| {
        format!("incremental harness did not emit status.\nstdout:\n{stdout}\nstderr:\n{stderr}")
    })?;
    let layout_hash = layout_hash.unwrap_or(0);
    let file_paths = file_paths.into_values().collect();

    Ok(IncrementalOutput {
        status,
        layout_hash,
        file_paths,
        functions,
        errors,
    })
}

fn format_error_message(code: i32, detail_a: i32, detail_b: i32) -> String {
    let head = match code {
        41 => "missing function main(): i32",
        42 => "invalid function main signature; expected function main(): i32",
        43 => "multiple main declarations",
        1001 => "unexpected character while lexing",
        1002 => "unterminated string literal",
        1003 => "token overflow while lexing",
        2001 => "expected top-level function/extern/global declaration",
        2002 => "expected function after extern",
        2003 => "expected identifier",
        2004 => "expected '('",
        2005 => "expected ')'",
        2006 => "expected ':'",
        2007 => "expected ';'",
        2008 => "expected '{'",
        2009 => "expected '}'",
        2010 => "expected expression",
        2011 => "expected expression after return",
        2012 => "top-level parse guard exhausted",
        2013 => "parameter parse guard exhausted",
        2014 => "argument parse guard exhausted",
        2015 => "body parse guard exhausted",
        2016 => "call parse guard exhausted",
        2017 => "let declaration requires '='",
        2018 => "expected ']'",
        2019 => "expected 'in' in foreach",
        3001 => "incremental file overflow",
        3002 => "incremental file path was empty",
        _ => "incremental compiler error",
    };
    format!("{head} (code={code}, detail_a={detail_a}, detail_b={detail_b})")
}

fn preprocess_for_incremental(source: &str) -> String {
    let mut out = String::new();
    let mut global_fields: Vec<String> = Vec::new();
    let mut skip_brace_depth: i32 = 0;
    for line in source.lines() {
        let trimmed = line.trim_start();

        if skip_brace_depth > 0 {
            skip_brace_depth += line.matches('{').count() as i32;
            skip_brace_depth -= line.matches('}').count() as i32;
            continue;
        }

        if trimmed.starts_with("import ") {
            continue;
        }
        if trimmed.starts_with("const ") {
            continue;
        }
        if trimmed.starts_with("struct ") || trimmed.starts_with("enum ") {
            skip_brace_depth += line.matches('{').count() as i32;
            skip_brace_depth -= line.matches('}').count() as i32;
            continue;
        }
        if let Some(global_rest) = trimmed.strip_prefix("global ") {
            let global_decl = global_rest.trim();
            if !global_decl.contains('{') && global_decl.ends_with(';') {
                global_fields.push(global_decl.to_string());
                continue;
            }
        }

        let normalized = line
            .replace("function @inline ", "function ")
            .replace("function @cold ", "function ");
        out.push_str(&normalized);
        out.push('\n');
    }

    let mut rewritten = String::new();
    if !global_fields.is_empty() {
        rewritten.push_str("global __stasis_globals {\n");
        for field in global_fields {
            rewritten.push_str("    ");
            rewritten.push_str(&field);
            rewritten.push('\n');
        }
        rewritten.push_str("}\n\n");
    }
    rewritten.push_str(&out);

    sanitize_for_harness_string_literal(&rewritten)
}

fn sanitize_for_harness_string_literal(source: &str) -> String {
    // The incremental lexer currently does not support string escapes.
    // To keep harness transport robust, strip comments and fold string literals to `0`.
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0usize;

    while i < bytes.len() {
        let b = bytes[i];

        if b == b'/' && i + 1 < bytes.len() && bytes[i + 1] == b'/' {
            i += 2;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'\n' {
                out.push('\n');
                i += 1;
            }
            continue;
        }

        if b == b'"' {
            out.push('0');
            i += 1;
            while i < bytes.len() && bytes[i] != b'"' {
                i += 1;
            }
            if i < bytes.len() && bytes[i] == b'"' {
                i += 1;
            }
            continue;
        }

        if b == b'\\' {
            out.push('/');
            i += 1;
            continue;
        }

        if b.is_ascii_digit() {
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j + 1 < bytes.len() && bytes[j] == b'.' && bytes[j + 1].is_ascii_digit() {
                out.push('0');
                j += 1;
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
                continue;
            }
            for &digit in &bytes[i..j] {
                out.push(char::from(digit));
            }
            i = j;
            continue;
        }

        out.push(char::from(b));
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

fn escape_stasis_string(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len() + 8);
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            _ => escaped.push(ch),
        }
    }
    escaped
}

fn expand_layout_hash(layout_hash: i32) -> LayoutHash {
    let as_u32 = layout_hash as u32;
    let mut out = [0u8; 32];
    out[0..4].copy_from_slice(&as_u32.to_le_bytes());
    out[4..8].copy_from_slice(&(as_u32.rotate_left(13)).to_le_bytes());
    out[8..12].copy_from_slice(&(as_u32.rotate_left(21)).to_le_bytes());
    out[12..16].copy_from_slice(&(as_u32.rotate_left(5)).to_le_bytes());
    out[16..20].copy_from_slice(&(as_u32.wrapping_mul(16777619)).to_le_bytes());
    out[20..24].copy_from_slice(&(as_u32 ^ 0x9e3779b9).to_le_bytes());
    out[24..28].copy_from_slice(&(as_u32.wrapping_add(0x85ebca6b)).to_le_bytes());
    out[28..32].copy_from_slice(&(as_u32 ^ 0xc2b2ae35).to_le_bytes());
    LayoutHash(out)
}

fn hash_identifier(name: &str) -> i32 {
    let mut hash: i32 = 216613626;
    for byte in name.bytes() {
        hash = hash
            .wrapping_mul(16777619)
            .wrapping_add(i32::from(byte) + 1);
    }
    hash
}

fn hash_text(value: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preprocess_strips_imports_structs_enums_and_consts() {
        let input = r#"
import "../../src/stdlib/stdlib.stasis";
const C: i32 = 1;
struct Item { value: i32; }
enum Kind { A, B }
function @inline main(): i32 { return C; }
"#;
        let output = preprocess_for_incremental(input);
        assert!(!output.contains("import "));
        assert!(!output.contains("const "));
        assert!(!output.contains("struct "));
        assert!(!output.contains("enum "));
        assert!(output.contains("function main(): i32"));
    }

    #[test]
    fn preprocess_sanitizes_comments_and_string_literals_for_harness() {
        let input = r#"
function main(): i32 {
    // comment with "quotes" and C:\path
    print_string("hello \"world\"");
    return 7;
}
"#;
        let output = preprocess_for_incremental(input);
        assert!(!output.contains("//"));
        assert!(!output.contains('"'));
        assert!(!output.contains('\\'));
        assert!(output.contains("print_string(0"));
    }

    #[test]
    fn preprocess_rewrites_global_fields_into_layout_block() {
        let input = r#"
global ticks: i32;
global values: f32[4];
function main(): i32 { return 0; }
"#;
        let output = preprocess_for_incremental(input);
        assert!(output.contains("global __stasis_globals"));
        assert!(output.contains("ticks: i32;"));
        assert!(output.contains("values: f32[4];"));
        assert!(output.contains("function main(): i32"));
    }

    #[test]
    fn identifier_hash_matches_incremental_function() {
        assert_eq!(hash_identifier("on_code_swap"), -663_287_521);
    }

    #[test]
    fn bridge_markers_parse() {
        assert_eq!(parse_bridge_begin("BRIDGE_BEGIN 42"), Some(42));
        assert_eq!(parse_bridge_begin("BRIDGE_BEGIN nope"), None);
        assert_eq!(parse_bridge_end("BRIDGE_END 42 0"), Some((42, 0)));
        assert_eq!(parse_bridge_end("BRIDGE_END 42"), None);
        assert_eq!(parse_bridge_end("not_a_marker"), None);
    }
}
