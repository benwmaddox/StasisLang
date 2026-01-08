use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use cranelift_codegen::ir::{AbiParam, Signature};
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_codegen::isa::CallConv;
use cranelift_module::{default_libcall_names, DataDescription, Linkage, Module};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_native;
use target_lexicon::Triple;

#[derive(Parser)]
#[command(name = "stasis-cranelift-jit-runner")]
#[command(about = "Run Stasis Cranelift CLIF in-process via cranelift-jit, with optional hot-swap over stdin.")]
struct Args
{
    #[arg(long)]
    server: bool,

    #[arg(long, value_name = "FPS", default_value = "60")]
    fps: u32,
}

fn main() -> Result<()>
{
    let args = Args::parse();
    if args.server
    {
        return run_server(args.fps);
    }

    bail!("non-server mode not implemented (use --server)");
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "win64" fn stasis_printf3(fmt: i64, a0: i64, _a1: i64) -> i32
{
    unsafe {
        let fmt = fmt as *const i8;
        let a0 = a0 as *const i8;
        libc::printf(fmt, a0)
    }
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn stasis_printf3(fmt: i64, a0: i64, _a1: i64) -> i32
{
    unsafe {
        let fmt = fmt as *const libc::c_char;
        let a0 = a0 as *const libc::c_char;
        libc::printf(fmt, a0)
    }
}

enum Request
{
    Init {
        module_name: String,
        entry_name: String,
        tick_name: String,
        clif: String,
    },
    Swap { clif: String },
    Quit,
}

fn run_server(fps: u32) -> Result<()>
{
    let mut stdout = io::stdout();

    writeln!(stdout, "READY")?;
    stdout.flush()?;

    let (tx, rx) = mpsc::channel::<Request>();
    spawn_request_reader(tx)?;

    let mut instance: Option<JitInstance> = None;

    loop
    {
        let req = rx.recv().context("request channel closed")?;
        match req
        {
            Request::Init { module_name, entry_name, tick_name, clif } =>
            {
                let mut new_instance = JitInstance::compile(&module_name, &entry_name, &tick_name, &clif)
                    .context("failed to compile initial CLIF")?;
                let rc = unsafe { (new_instance.entry_fn)() };
                if rc != 0
                {
                    writeln!(stdout, "ERR entry returned {rc}")?;
                    stdout.flush()?;
                    continue;
                }

                writeln!(stdout, "OK")?;
                stdout.flush()?;

                let tick_loop_result = run_tick_loop(fps, &mut new_instance, &rx, &mut stdout);
                instance = Some(new_instance);
                return tick_loop_result;
            }
            Request::Swap { .. } =>
            {
                writeln!(stdout, "ERR swap before init")?;
                stdout.flush()?;
            }
            Request::Quit =>
            {
                writeln!(stdout, "OK")?;
                stdout.flush()?;
                break;
            }
        }
    }

    Ok(())
}

fn run_tick_loop(fps: u32, instance: &mut JitInstance, rx: &mpsc::Receiver<Request>, stdout: &mut io::Stdout) -> Result<()>
{
    let target_dt = if fps == 0 { Duration::from_millis(16) } else { Duration::from_secs_f64(1.0 / (fps as f64)) };
    let mut last = Instant::now();

    loop
    {
        while let Ok(req) = rx.try_recv()
        {
            match req
            {
                Request::Swap { clif } =>
                {
                    let t0 = Instant::now();
                    let (missing_save, save_bytes) = instance.save_state();
                    let save_us = t0.elapsed().as_micros() as u64;

                    let t1 = Instant::now();
                    let mut new_instance = JitInstance::compile(&instance.module_name, &instance.entry_name, &instance.tick_name, &clif)
                        .context("failed to compile swapped CLIF")?;
                    let compile_us = t1.elapsed().as_micros() as u64;

                    let t2 = Instant::now();
                    let missing_restore = new_instance.restore_state(save_bytes);
                    let restore_us = t2.elapsed().as_micros() as u64;

                    let bytes: usize = new_instance.state_globals.iter().map(|(_, sz)| *sz).sum();
                    let symbols = new_instance.state_globals.len();
                    eprintln!(
                        "HOTSWAP ok: save={}us load={}us tick=0us restore={}us bytes={} symbols={}",
                        save_us, compile_us, restore_us, bytes, symbols
                    );
                    if missing_save > 0 || missing_restore > 0
                    {
                        eprintln!(
                            "HOTSWAP warning: state layout changed (missing save={} restore={}); consider restarting to resync state.",
                            missing_save, missing_restore
                        );
                    }

                    *instance = new_instance;
                    writeln!(stdout, "OK")?;
                    stdout.flush()?;
                }
                Request::Quit =>
                {
                    writeln!(stdout, "OK")?;
                    stdout.flush()?;
                    return Ok(());
                }
                Request::Init { .. } =>
                {
                    writeln!(stdout, "ERR already initialized")?;
                    stdout.flush()?;
                }
            }
        }

        let rc = unsafe { (instance.tick_fn)() };
        if rc != 0
        {
            return Ok(());
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(last);
        last = now;
        if elapsed < target_dt
        {
            thread::sleep(target_dt - elapsed);
        }
    }
}

fn spawn_request_reader(tx: mpsc::Sender<Request>) -> Result<()>
{
    let tx2 = tx.clone();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut line = String::new();
        loop
        {
            line.clear();
            let bytes = reader.read_line(&mut line).unwrap_or(0);
            if bytes == 0
            {
                let _ = tx2.send(Request::Quit);
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty()
            {
                continue;
            }

            if trimmed.eq_ignore_ascii_case("QUIT")
            {
                let _ = tx2.send(Request::Quit);
                break;
            }

            if trimmed.starts_with("INIT ")
            {
                let parts: Vec<_> = trimmed.split_whitespace().collect();
                if parts.len() != 5
                {
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let module_len = parts[1].parse::<usize>().unwrap_or(0);
                let entry_len = parts[2].parse::<usize>().unwrap_or(0);
                let tick_len = parts[3].parse::<usize>().unwrap_or(0);
                let clif_len = parts[4].parse::<usize>().unwrap_or(0);
                if module_len == 0 || entry_len == 0 || tick_len == 0 || clif_len == 0
                {
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let module_name = read_string(&mut reader, module_len).unwrap_or_default();
                let entry_name = read_string(&mut reader, entry_len).unwrap_or_default();
                let tick_name = read_string(&mut reader, tick_len).unwrap_or_default();
                let clif = read_string(&mut reader, clif_len).unwrap_or_default();
                let _ = tx2.send(Request::Init { module_name, entry_name, tick_name, clif });
                continue;
            }

            if trimmed.starts_with("SWAP ")
            {
                let parts: Vec<_> = trimmed.split_whitespace().collect();
                if parts.len() != 2
                {
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let clif_len = parts[1].parse::<usize>().unwrap_or(0);
                if clif_len == 0
                {
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let clif = read_string(&mut reader, clif_len).unwrap_or_default();
                let _ = tx2.send(Request::Swap { clif });
                continue;
            }
        }
    });

    Ok(())
}

fn read_string<R: Read>(reader: &mut R, len: usize) -> Result<String>
{
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let s = String::from_utf8(buf).context("invalid UTF-8")?;
    Ok(s)
}

struct JitInstance
{
    module_name: String,
    entry_name: String,
    tick_name: String,
    module: JITModule,
    entry_fn: extern "C" fn() -> i32,
    tick_fn: extern "C" fn() -> i32,
    state_globals: Vec<(String, usize)>,
    data_ptrs: HashMap<String, *mut u8>,
}

impl JitInstance
{
    fn compile(module_name: &str, entry_name: &str, tick_name: &str, clif: &str) -> Result<Self>
    {
        let triple = Triple::host();
        let flags = build_flags("speed", &triple)?;
        let isa = cranelift_native::builder()
            .map_err(|e| anyhow::anyhow!("failed to create native ISA builder: {e}"))?
            .finish(flags)
            .context("failed to finish native ISA")?;
        let default_cc = isa.default_call_conv();

        let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
        jit_builder.symbol("printf", stasis_printf3 as *const u8);
        jit_builder.symbol("stasis_printf3", stasis_printf3 as *const u8);

        let mut module = JITModule::new(jit_builder);

        let parsed = parse_stasis_clif(clif, default_cc).context("failed to parse stasis CLIF")?;

        let mut data_ptrs = HashMap::new();
        let mut state_globals = Vec::new();
        let mut data_ids = HashMap::new();
        for g in &parsed.globals
        {
            let id = module
                .declare_data(&g.name, Linkage::Export, true, false)
                .with_context(|| format!("declare_data failed for {}", g.name))?;
            data_ids.insert(g.name.clone(), id);

            let mut data_desc = DataDescription::new();
            match &g.init_data
            {
                GlobalInitData::Zero => data_desc.define_zeroinit(g.size_bytes),
                GlobalInitData::String(bytes) => data_desc.define(bytes.clone().into_boxed_slice()),
            }
            module
                .define_data(id, &data_desc)
                .with_context(|| format!("define_data failed for {}", g.name))?;

            if g.name.starts_with("state__")
            {
                state_globals.push((g.name.clone(), g.size_bytes));
            }
        }

        let mut function_ids = HashMap::new();
        for ext in &parsed.externals
        {
            let link_name = if ext.name == "printf_str" || ext.name == "printf3" { "printf" } else { &ext.name };
            let id = module
                .declare_function(link_name, Linkage::Import, &ext.signature)
                .with_context(|| format!("declare external function failed for {}", ext.name))?;
            function_ids.insert(ext.name.clone(), id);
        }

        for f in &parsed.functions
        {
            let id = module
                .declare_function(&f.name, Linkage::Export, &f.signature)
                .with_context(|| format!("declare_function failed for {}", f.name))?;
            function_ids.insert(f.name.clone(), id);
        }

        for f in parsed.functions
        {
            let mut context = module.make_context();
            context.func = build_function_ir(&mut module, &function_ids, &data_ids, &f)
                .with_context(|| format!("failed to build IR for {}", f.name))?;
            let id = *function_ids.get(&f.name).context("missing function id")?;
            module
                .define_function(id, &mut context)
                .with_context(|| format!("define_function failed for {}", f.name))?;
            module.clear_context(&mut context);
        }

        module.finalize_definitions().context("failed to finalize JIT definitions")?;

        for (name, id) in &data_ids
        {
            let ptr = module.get_finalized_data(*id);
            data_ptrs.insert(name.clone(), ptr.0 as *mut u8);
        }

        let entry_id = *function_ids.get(entry_name).with_context(|| format!("missing entry function {entry_name}"))?;
        let tick_id = *function_ids.get(tick_name).with_context(|| format!("missing tick function {tick_name}"))?;

        let entry_ptr = module.get_finalized_function(entry_id);
        let tick_ptr = module.get_finalized_function(tick_id);

        Ok(JitInstance {
            module_name: module_name.to_string(),
            entry_name: entry_name.to_string(),
            tick_name: tick_name.to_string(),
            module,
            entry_fn: unsafe { std::mem::transmute(entry_ptr) },
            tick_fn: unsafe { std::mem::transmute(tick_ptr) },
            state_globals,
            data_ptrs,
        })
    }

    fn save_state(&self) -> (u32, HashMap<String, Vec<u8>>)
    {
        let mut missing = 0u32;
        let mut out = HashMap::new();
        for (name, size) in &self.state_globals
        {
            let Some(ptr) = self.data_ptrs.get(name) else {
                missing += 1;
                continue;
            };
            unsafe {
                let slice = std::slice::from_raw_parts(*ptr as *const u8, *size);
                out.insert(name.clone(), slice.to_vec());
            }
        }
        (missing, out)
    }

    fn restore_state(&mut self, saved: HashMap<String, Vec<u8>>) -> u32
    {
        let mut missing = 0u32;
        for (name, size) in &self.state_globals
        {
            let Some(ptr) = self.data_ptrs.get(name) else {
                missing += 1;
                continue;
            };
            let Some(bytes) = saved.get(name) else {
                missing += 1;
                continue;
            };
            if bytes.len() != *size
            {
                missing += 1;
                continue;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), *ptr, *size);
            }
        }
        missing
    }
}

fn build_flags(opt_level: &str, target: &Triple) -> Result<settings::Flags>
{
    let mut flag_builder = settings::builder();

    match opt_level
    {
        "none" => flag_builder.set("opt_level", "none")?,
        "speed" => flag_builder.set("opt_level", "speed")?,
        "speed_and_size" => flag_builder.set("opt_level", "speed_and_size")?,
        other => bail!("invalid opt_level '{other}' (use none|speed|speed_and_size)"),
    }

    if target.operating_system.is_like_darwin()
    {
        flag_builder.set("is_pic", "true")?;
    }

    Ok(settings::Flags::new(flag_builder))
}

#[derive(Clone)]
struct ParsedModule
{
    globals: Vec<ParsedGlobal>,
    externals: Vec<ParsedExternal>,
    functions: Vec<ParsedFunction>,
}

#[derive(Clone)]
struct ParsedGlobal
{
    name: String,
    init_data: GlobalInitData,
    size_bytes: usize,
}

#[derive(Clone)]
enum GlobalInitData
{
    Zero,
    String(Vec<u8>),
}

#[derive(Clone)]
struct ParsedExternal
{
    name: String,
    signature: Signature,
}

#[derive(Clone)]
struct ParsedFunction
{
    name: String,
    signature: Signature,
    blocks: Vec<ParsedBlock>,
}

#[derive(Clone)]
struct ParsedBlock
{
    name: String,
    param_value_ids: Vec<u32>,
    instructions: Vec<String>,
}

fn parse_stasis_clif(input: &str, default_cc: CallConv) -> Result<ParsedModule>
{
    let mut globals = Vec::new();
    let mut externals = Vec::new();
    let mut funcs = Vec::new();
    let mut lines = input.lines().enumerate().peekable();

    while let Some((_, line)) = lines.peek().copied()
    {
        let line = line.trim();
        if line.is_empty() || line.starts_with(';')
        {
            lines.next();
            continue;
        }

        if line.starts_with("global ")
        {
            lines.next();
            let (name, _ty, init_data, size_bytes) = parse_global_decl(line)
                .with_context(|| format!("failed to parse global declaration: {line}"))?;
            globals.push(ParsedGlobal { name, init_data, size_bytes });
            continue;
        }

        if line.starts_with("external ")
        {
            lines.next();
            let (name, signature) = parse_external_decl(line, default_cc)
                .with_context(|| format!("failed to parse external declaration: {line}"))?;
            externals.push(ParsedExternal { name, signature });
            continue;
        }

        if !line.starts_with("function %")
        {
            lines.next();
            continue;
        }

        let (_, header_line) = lines.next().unwrap();
        let header = header_line.trim();

        let (name, signature) = parse_function_header(header, default_cc)
            .with_context(|| format!("failed to parse function header: {header}"))?;

        let mut body = Vec::new();
        loop
        {
            let Some((_, body_line)) = lines.next() else
            {
                bail!("unterminated function {name} (missing '}}')");
            };

            let t = body_line.trim_end();
            if t.trim() == "}"
            {
                break;
            }

            body.push(t.to_string());
        }

        let blocks = parse_blocks(&body).with_context(|| format!("failed to parse blocks for {name}"))?;
        funcs.push(ParsedFunction { name, signature, blocks });
    }

    if funcs.is_empty()
    {
        bail!("no functions found in input");
    }

    Ok(ParsedModule { globals, externals, functions: funcs })
}

fn parse_blocks(lines: &[String]) -> Result<Vec<ParsedBlock>>
{
    let mut blocks = Vec::new();
    let mut i = 0;
    while i < lines.len()
    {
        let line = lines[i].trim();
        if line.is_empty() || line.starts_with(';')
        {
            i += 1;
            continue;
        }

        if !line.starts_with("block")
        {
            bail!("expected block header, got: {line}");
        }

        let mut name = line.trim_end_matches(':').to_string();
        let mut param_ids = Vec::new();
        if let Some((hdr, params)) = line.split_once('(')
        {
            name = hdr.trim().trim_end_matches(':').to_string();
            let params = params.trim().trim_end_matches("):").trim_end_matches(')').trim();
            if !params.is_empty()
            {
                for p in params.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
                {
                    // v12: i64
                    let (v, _) = p.split_once(':').context("invalid block param")?;
                    let id = parse_value_id(v.trim())?;
                    param_ids.push(id);
                }
            }
        }

        i += 1;
        let mut insts = Vec::new();
        while i < lines.len()
        {
            let l = lines[i].trim();
            if l.starts_with("block")
            {
                break;
            }
            if !l.is_empty() && !l.starts_with(';')
            {
                insts.push(l.to_string());
            }
            i += 1;
        }

        blocks.push(ParsedBlock { name, param_value_ids: param_ids, instructions: insts });
    }
    Ok(blocks)
}

fn parse_global_decl(line: &str) -> Result<(String, cranelift_codegen::ir::Type, GlobalInitData, usize)>
{
    let rest = line.strip_prefix("global ").context("missing 'global ' prefix")?;

    if let Some((decl_part, comment_part)) = rest.split_once(';')
    {
        let (name, ty_str) = decl_part.split_once(':').context("missing ':' in global decl")?;
        let name = name.trim().to_string();
        let (ty, _) = parse_type_with_count(ty_str.trim())?;

        let comment = comment_part.trim();
        if let Some(bytes) = parse_bytes_literal_comment(comment)
        {
            let size_bytes = bytes.len();
            return Ok((name, ty, GlobalInitData::String(bytes), size_bytes));
        }
        if let Some(string_data) = parse_string_literal_comment(comment)
        {
            let size_bytes = string_data.len();
            return Ok((name, ty, GlobalInitData::String(string_data), size_bytes));
        }
    }

    let (name, ty_str) = rest.split_once(':').context("missing ':' in global decl")?;
    let name = name.trim().to_string();
    let (ty, count) = parse_type_with_count(ty_str.trim())?;
    let size_bytes = ty.bytes() as usize * count;
    Ok((name, ty, GlobalInitData::Zero, size_bytes))
}

fn parse_string_literal_comment(comment: &str) -> Option<Vec<u8>>
{
    let start = comment.find('"')?;
    let rest = &comment[start + 1..];
    let end = rest.find('"')?;
    let quoted = &rest[..end];

    let mut bytes = Vec::new();
    let mut chars = quoted.chars();
    while let Some(ch) = chars.next()
    {
        if ch == '\\'
        {
            match chars.next()?
            {
                'n' => bytes.push(b'\n'),
                'r' => bytes.push(b'\r'),
                't' => bytes.push(b'\t'),
                '\\' => bytes.push(b'\\'),
                '"' => bytes.push(b'"'),
                '0' => bytes.push(b'\0'),
                c => {
                    bytes.push(b'\\');
                    bytes.push(c as u8);
                }
            }
        }
        else
        {
            bytes.push(ch as u8);
        }
    }
    bytes.push(0);
    Some(bytes)
}

fn parse_bytes_literal_comment(comment: &str) -> Option<Vec<u8>>
{
    let start = comment.find("bytes:")?;
    let rest = &comment[start + "bytes:".len()..];
    let mut bytes = Vec::new();
    for token in rest.split_whitespace()
    {
        let t = token.trim().trim_end_matches(',');
        if t.is_empty()
        {
            continue;
        }
        let value = u8::from_str_radix(t, 16).ok()?;
        bytes.push(value);
    }
    if bytes.is_empty() { None } else { Some(bytes) }
}

fn parse_external_decl(line: &str, default_cc: CallConv) -> Result<(String, Signature)>
{
    let rest = line.strip_prefix("external ").context("missing 'external ' prefix")?;
    let open = rest.find('(').context("missing '(' in external decl")?;
    let close = rest.find(')').context("missing ')' in external decl")?;
    if close < open
    {
        bail!("invalid external decl parens");
    }

    let name = rest[..open].trim().to_string();
    let param_str = rest[open + 1..close].trim();
    let after = rest[close + 1..].trim();

    let mut sig = Signature::new(default_cc);
    if !param_str.is_empty()
    {
        for ty in param_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            sig.params.push(AbiParam::new(parse_type(ty)?));
        }
    }

    if after.starts_with("->")
    {
        let after = after.strip_prefix("->").unwrap().trim();
        let parts: Vec<_> = after.split_whitespace().collect();
        if parts.is_empty()
        {
            bail!("missing return type in external decl");
        }
        let ret_ty = parse_type(parts[0])?;
        sig.returns.push(AbiParam::new(ret_ty));
        if parts.len() > 1
        {
            sig.call_conv = parse_callconv(parts[1], default_cc)?;
        }
    }
    Ok((name, sig))
}

fn parse_function_header(line: &str, default_cc: CallConv) -> Result<(String, Signature)>
{
    // function %name(params) -> rets callconv {
    let rest = line.strip_prefix("function %").context("missing 'function %'")?;
    let open = rest.find('(').context("missing '('")?;
    let close = rest.find(')').context("missing ')'")?;
    let name = rest[..open].trim().to_string();
    let param_str = rest[open + 1..close].trim();
    let after = rest[close + 1..].trim();

    let mut sig = Signature::new(default_cc);
    if !param_str.is_empty()
    {
        for ty in param_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            sig.params.push(AbiParam::new(parse_type(ty)?));
        }
    }

    if let Some((arrow, rest_after)) = after.split_once("->")
    {
        let _ = arrow;
        let rest_after = rest_after.trim();
        let rest_after = rest_after.trim_end_matches('{').trim();
        let parts: Vec<_> = rest_after.split_whitespace().collect();
        if parts.is_empty()
        {
            bail!("missing return type");
        }
        let ret_ty = parse_type(parts[0])?;
        sig.returns.push(AbiParam::new(ret_ty));
        if parts.len() > 1
        {
            sig.call_conv = parse_callconv(parts[1], default_cc)?;
        }
    }

    Ok((name, sig))
}

fn parse_callconv(s: &str, default_cc: CallConv) -> Result<CallConv>
{
    match s
    {
        "windows_fastcall" => Ok(CallConv::WindowsFastcall),
        "system_v" => Ok(CallConv::SystemV),
        "fast" => Ok(CallConv::Fast),
        "cold" => Ok(CallConv::Cold),
        "tail" => Ok(CallConv::Tail),
        "default" => Ok(default_cc),
        other => bail!("unsupported callconv: {other}"),
    }
}

fn parse_type_with_count(s: &str) -> Result<(cranelift_codegen::ir::Type, usize)>
{
    if let Some((base, rest)) = s.split_once('[')
    {
        let rest = rest.trim_end_matches(']');
        let count = rest.parse::<usize>().context("invalid array count")?;
        Ok((parse_type(base.trim())?, count))
    }
    else
    {
        Ok((parse_type(s)?, 1))
    }
}

fn parse_type(s: &str) -> Result<cranelift_codegen::ir::Type>
{
    match s
    {
        "i8" => Ok(cranelift_codegen::ir::types::I8),
        "i16" => Ok(cranelift_codegen::ir::types::I16),
        "i32" => Ok(cranelift_codegen::ir::types::I32),
        "i64" => Ok(cranelift_codegen::ir::types::I64),
        "f32" => Ok(cranelift_codegen::ir::types::F32),
        "f64" => Ok(cranelift_codegen::ir::types::F64),
        other => bail!("unsupported type: {other}"),
    }
}

fn parse_value_id(v: &str) -> Result<u32>
{
    let v = v.trim().trim_end_matches(',').trim_end_matches(')');
    let v = v.strip_prefix('v').context("expected value id")?;
    Ok(v.parse::<u32>().context("invalid value id")?)
}

// The rest of the CLIF->IR lowering is non-trivial; for the hot-swap prototype we only support
// a small subset of Stasis-generated CLIF instructions.
fn build_function_ir<M: Module>(
    module: &mut M,
    function_ids: &HashMap<String, cranelift_module::FuncId>,
    data_ids: &HashMap<String, cranelift_module::DataId>,
    f: &ParsedFunction,
) -> Result<cranelift_codegen::ir::Function>
{
    // Minimal IR builder: supports the subset emitted by samples/hotstate_tick_watch.stasis.
    use cranelift_codegen::ir::{Function, InstBuilder};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    let mut func = Function::new();
    func.signature = f.signature.clone();
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut builder_ctx);
        let block = b.create_block();
        b.switch_to_block(block);
        b.seal_block(block);

        let mut vals: HashMap<u32, cranelift_codegen::ir::Value> = HashMap::new();
        for inst in &f.blocks[0].instructions
        {
            if let Some((lhs, rhs)) = inst.split_once('=')
            {
                let lhs_id = parse_value_id(lhs.trim())?;
                let rhs = rhs.trim();
                if rhs.starts_with("global_value ")
                {
                    let name = rhs.strip_prefix("global_value").unwrap().trim();
                    let data_id = *data_ids.get(name).with_context(|| format!("unknown global {name}"))?;
                    let gv = module.declare_data_in_func(data_id, b.func);
                    let gv = b.ins().global_value(cranelift_codegen::ir::types::I64, gv);
                    vals.insert(lhs_id, gv);
                    continue;
                }
                if rhs.starts_with("iconst.")
                {
                    // iconst.i64 8
                    let parts: Vec<_> = rhs.split_whitespace().collect();
                    let ty = parts[0].strip_prefix("iconst.").unwrap();
                    let imm = parts[1].parse::<i64>()?;
                    let v = b.ins().iconst(parse_type(ty)?, imm);
                    vals.insert(lhs_id, v);
                    continue;
                }
                if rhs.starts_with("iadd ")
                {
                    let parts: Vec<_> = rhs.split_whitespace().collect();
                    let a = vals.get(&parse_value_id(parts[1])?).copied().context("missing value")?;
                    let b2 = vals.get(&parse_value_id(parts[2])?).copied().context("missing value")?;
                    let v = b.ins().iadd(a, b2);
                    vals.insert(lhs_id, v);
                    continue;
                }
                if rhs.starts_with("call %")
                {
                    // call %printf3(v4, v2, v5)
                    let rhs = rhs.strip_prefix("call %").unwrap();
                    let open = rhs.find('(').context("invalid call")?;
                    let close = rhs.rfind(')').context("invalid call")?;
                    let callee = rhs[..open].trim();
                    let args_str = rhs[open + 1..close].trim();

                    let func_id = *function_ids.get(callee).with_context(|| format!("unknown function {callee}"))?;
                    let local_ref = module.declare_func_in_func(func_id, b.func);
                    let mut args = Vec::new();
                    if !args_str.is_empty()
                    {
                        for a in args_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
                        {
                            let v = vals.get(&parse_value_id(a)?).copied().context("missing value")?;
                            args.push(v);
                        }
                    }
                    let call = b.ins().call(local_ref, &args);
                    if let Some(first) = b.inst_results(call).get(0).copied()
                    {
                        vals.insert(lhs_id, first);
                    }
                    continue;
                }
                bail!("unsupported instruction: {rhs}");
            }
            if inst.starts_with("return ")
            {
                let parts: Vec<_> = inst.split_whitespace().collect();
                let v = vals.get(&parse_value_id(parts[1])?).copied().context("missing value")?;
                b.ins().return_(&[v]);
                continue;
            }
            bail!("unsupported instruction: {inst}");
        }
        b.finalize();
    }
    Ok(func)
}
