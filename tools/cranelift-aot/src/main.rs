use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::Parser;
use cranelift_codegen::isa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_codegen::ir::{types, AbiParam, Function, Inst, InstBuilder, Signature, GlobalValue, StackSlotData, StackSlotKind};
use cranelift_codegen::ir::immediates::{Ieee32, Ieee64};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module, DataDescription};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::{OperatingSystem, Triple};

#[derive(Parser)]
#[command(name = "stasis-cranelift-aot")]
#[command(about = "Compile Cranelift CLIF into a native object file (COFF on Windows).")]
struct Args
{
    /// Input CLIF file path.
    #[arg(long, value_name = "PATH", required_unless_present = "server")]
    input: Option<PathBuf>,

    /// Output object file path.
    #[arg(long, value_name = "PATH", required_unless_present = "server")]
    output: Option<PathBuf>,

    /// Target triple (default: x86_64-pc-windows-msvc).
    #[arg(long, value_name = "TRIPLE", default_value = "x86_64-pc-windows-msvc")]
    target: String,

    /// Module name embedded in the object.
    #[arg(long, value_name = "NAME", default_value = "stasis_module")]
    module_name: String,

    /// Optimization level (none|speed|speed_and_size).
    #[arg(long, value_name = "LEVEL", default_value = "none")]
    opt_level: String,

    /// Run in persistent server mode (reads requests from stdin).
    #[arg(long)]
    server: bool,
}

fn main() -> Result<()>
{
    let args = Args::parse();

    if args.server
    {
        return run_server();
    }

    let input = args.input.context("missing --input")?;
    let output = args.output.context("missing --output")?;
    let clif = fs::read_to_string(&input)
        .with_context(|| format!("failed to read input file: {}", input.display()))?;

    compile_clif(&clif, &output, &args.target, &args.module_name, &args.opt_level)
}

fn compile_clif(clif: &str, output: &PathBuf, target: &str, module_name: &str, opt_level: &str) -> Result<()>
{
    let triple = Triple::from_str(target)
        .map_err(|_| anyhow::anyhow!("invalid target triple: {target}"))?;
    let target_is_windows = matches!(triple.operating_system, OperatingSystem::Windows);

    let flags = build_flags(opt_level, &triple)?;
    let isa = isa::lookup(triple.clone())
        .context("failed to look up ISA for target")?
        .finish(flags)
        .context("failed to finalize ISA")?;
    let default_cc = isa.default_call_conv();

    let builder = ObjectBuilder::new(isa, module_name.to_string(), default_libcall_names())
        .context("failed to create ObjectBuilder")?;
    let mut module = ObjectModule::new(builder);

    let parsed = parse_stasis_clif(clif, default_cc).context("failed to parse stasis CLIF")?;

    // First declare all globals.
    let mut data_ids = std::collections::HashMap::new();
    for g in &parsed.globals
    {
        let id = module
            .declare_data(&g.name, Linkage::Export, true, false)
            .with_context(|| format!("declare_data failed for {}", g.name))?;
        data_ids.insert(g.name.clone(), id);

        // Define the global with appropriate initialization.
        let mut data_desc = DataDescription::new();
        match &g.init_data
        {
            GlobalInitData::Zero => {
                data_desc.define_zeroinit(g.size_bytes);
            }
            GlobalInitData::String(bytes) => {
                data_desc.define(bytes.clone().into_boxed_slice());
            }
        }
        module
            .define_data(id, &data_desc)
            .with_context(|| format!("define_data failed for {}", g.name))?;
    }

    // Declare external functions (imports from C runtime).
    let mut function_ids = std::collections::HashMap::new();
    for ext in &parsed.externals
    {
        // Alias printf3 to either printf (Windows) or a fixed-arity wrapper (SysV varargs can crash if called as non-variadic).
        let link_name =
            if ext.name == "printf_str" || ext.name == "printf3"
            {
                if target_is_windows { "printf" } else { "stasis_printf3" }
            }
            else
            {
                &ext.name
            };

        let id = module
            .declare_function(link_name, Linkage::Import, &ext.signature)
            .with_context(|| format!("declare external function failed for {}", ext.name))?;
        function_ids.insert(ext.name.clone(), id);
    }

    // Then declare all functions so intra-module calls can resolve.
    for f in &parsed.functions
    {
        let id = module
            .declare_function(&f.name, Linkage::Export, &f.signature)
            .with_context(|| format!("declare_function failed for {}", f.name))?;
        function_ids.insert(f.name.clone(), id);
    }

    // Finally define each function body.
    for f in parsed.functions
    {
        let mut context = module.make_context();
        context.func = build_function_ir(&mut module, &function_ids, &data_ids, &f)
            .with_context(|| format!("failed to build IR for {}", f.name))?;
        let id = *function_ids.get(&f.name).context("missing function id")?;
        module
            .define_function(id, &mut context)
            .with_context(|| format!("define_function failed for {}", f.name))?;
        module
            .clear_context(&mut context);
    }

    let product = module.finish();
    let obj_bytes = product.emit().context("failed to emit object")?;

    fs::write(output, obj_bytes)
        .with_context(|| format!("failed to write object file: {}", output.display()))?;

    Ok(())
}

fn run_server() -> Result<()>
{
    let stdin = io::stdin();
    let mut reader = BufReader::new(stdin.lock());
    let mut stdout = io::stdout();

    writeln!(stdout, "READY")?;
    stdout.flush()?;

    let mut line = String::new();
    loop
    {
        line.clear();
        let bytes = reader.read_line(&mut line)?;
        if bytes == 0
        {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty()
        {
            continue;
        }

        if trimmed.eq_ignore_ascii_case("QUIT")
        {
            break;
        }

        if !trimmed.starts_with("REQ ")
        {
            writeln!(stdout, "ERR invalid request header")?;
            stdout.flush()?;
            continue;
        }

        let parts: Vec<_> = trimmed.split_whitespace().collect();
        if parts.len() != 6
        {
            writeln!(stdout, "ERR invalid request header")?;
            stdout.flush()?;
            continue;
        }

        let out_len = parts[1].parse::<usize>().unwrap_or(0);
        let target_len = parts[2].parse::<usize>().unwrap_or(0);
        let module_len = parts[3].parse::<usize>().unwrap_or(0);
        let opt_len = parts[4].parse::<usize>().unwrap_or(0);
        let clif_len = parts[5].parse::<usize>().unwrap_or(0);

        if out_len == 0 || target_len == 0 || module_len == 0 || opt_len == 0 || clif_len == 0
        {
            writeln!(stdout, "ERR invalid request lengths")?;
            stdout.flush()?;
            continue;
        }

        let out_path = read_string(&mut reader, out_len)?;
        let target = read_string(&mut reader, target_len)?;
        let module_name = read_string(&mut reader, module_len)?;
        let opt_level = read_string(&mut reader, opt_len)?;
        let clif = read_string(&mut reader, clif_len)?;

        let output = PathBuf::from(out_path);
        let result = compile_clif(&clif, &output, &target, &module_name, &opt_level);

        match result
        {
            Ok(()) => {
                writeln!(stdout, "OK")?;
                stdout.flush()?;
            }
            Err(err) => {
                writeln!(stdout, "ERR {}", err)?;
                stdout.flush()?;
            }
        }
    }

    Ok(())
}

fn read_string<R: Read>(reader: &mut R, len: usize) -> Result<String>
{
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let s = String::from_utf8(buf).context("invalid UTF-8")?;
    Ok(s)
}

fn build_flags(opt_level: &str, target: &Triple) -> Result<settings::Flags>
{
    let mut flag_builder = settings::builder();

    match opt_level
    {
        "none" => flag_builder.set("opt_level", "none")?,
        "speed" => flag_builder.set("opt_level", "speed")?,
        "speed_and_size" => flag_builder.set("opt_level", "speed_and_size")?,
        other => bail!("invalid --opt-level '{other}' (use none|speed|speed_and_size)"),
    }

    if matches!(target.operating_system, OperatingSystem::Darwin(_) | OperatingSystem::MacOSX(_))
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
    String(Vec<u8>), // Null-terminated string bytes
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

        // Parse global declarations: "global gv0: i32" or "global str_0: i64 ; "string data""
        if line.starts_with("global ")
        {
            lines.next();
            let (name, _ty, init_data, size_bytes) = parse_global_decl(line)
                .with_context(|| format!("failed to parse global declaration: {line}"))?;
            globals.push(ParsedGlobal { name, init_data, size_bytes });
            continue;
        }

        // Parse external function declarations: "external printf(i64) -> i32 windows_fastcall"
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

        // Collect body lines until closing brace.
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

fn parse_global_decl(line: &str) -> Result<(String, cranelift_codegen::ir::Type, GlobalInitData, usize)>
{
    // Example: "global gv0: i32"
    // Or: "global str_0: i64 ; "Hello\n""
    // Or: "global numbers: i32[5]"
    let rest = line.strip_prefix("global ").context("missing 'global ' prefix")?;

    // Check for literal comment
    if let Some((decl_part, comment_part)) = rest.split_once(';')
    {
        let (name, ty_str) = decl_part.split_once(':').context("missing ':' in global decl")?;
        let name = name.trim().to_string();
        let (ty, _) = parse_type_with_count(ty_str.trim())?;

        // Parse literal data from comment
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

    // No comment or no string literal - parse as regular global
    let (name, ty_str) = rest.split_once(':').context("missing ':' in global decl")?;
    let name = name.trim().to_string();
    let (ty, count) = parse_type_with_count(ty_str.trim())?;
    let size_bytes = ty.bytes() as usize * count;
    Ok((name, ty, GlobalInitData::Zero, size_bytes))
}

fn parse_string_literal_comment(comment: &str) -> Option<Vec<u8>>
{
    // Comment format: "string data" or just "string data"
    // Find quoted string
    let start = comment.find('"')?;
    let rest = &comment[start + 1..];
    let end = rest.find('"')?;
    let quoted = &rest[..end];

    // Unescape and convert to bytes
    let mut bytes = Vec::new();
    let mut chars = quoted.chars();
    while let Some(ch) = chars.next()
    {
        if ch == '\\' {
            match chars.next()? {
                'n' => bytes.push(b'\n'),
                'r' => bytes.push(b'\r'),
                't' => bytes.push(b'\t'),
                '\\' => bytes.push(b'\\'),
                '"' => bytes.push(b'"'),
                '0' => bytes.push(b'\0'),
                c => {
                    // Unknown escape, keep literal
                    bytes.push(b'\\');
                    bytes.push(c as u8);
                }
            }
        } else {
            bytes.push(ch as u8);
        }
    }

    // Add null terminator for C strings
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
        if t.is_empty() {
            continue;
        }
        let value = u8::from_str_radix(t, 16).ok()?;
        bytes.push(value);
    }

    if bytes.is_empty() {
        return None;
    }

    Some(bytes)
}

fn parse_external_decl(line: &str, default_cc: CallConv) -> Result<(String, Signature)>
{
    // Example: "external printf(i64) -> i32 windows_fastcall"
    // Strip "external " prefix
    let rest = line.strip_prefix("external ").context("missing 'external ' prefix")?;

    // Parse similar to function header but without the % prefix and without { }
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

    // Parse optional "-> <ret>" before calling convention token.
    if after.starts_with("->")
    {
        let after = after.strip_prefix("->").unwrap().trim();
        let mut parts = after.split_whitespace();
        let ret_ty = parts.next().context("missing return type after '->'")?;
        sig.returns.push(AbiParam::new(parse_type(ret_ty)?));
    }

    Ok((name, sig))
}

fn parse_function_header(line: &str, default_cc: CallConv) -> Result<(String, Signature)>
{
    // Example:
    // function %main() -> i32 windows_fastcall {
    let line = line.trim();
    if !line.ends_with('{')
    {
        bail!("function header must end with '{{'");
    }

    let line = line.trim_end_matches('{').trim_end();
    let rest = line.strip_prefix("function %").context("missing 'function %' prefix")?;

    let open = rest.find('(').context("missing '('")?;
    let close = rest.find(')').context("missing ')'")?;
    if close < open
    {
        bail!("invalid function header parens");
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

    // Parse optional "-> <ret>" before calling convention token.
    if after.starts_with("->")
    {
        let after = after.strip_prefix("->").unwrap().trim();
        let mut parts = after.split_whitespace();
        let ret_ty = parts.next().context("missing return type after '->'")?;
        sig.returns.push(AbiParam::new(parse_type(ret_ty)?));
    }

    Ok((name, sig))
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
            i += 1;
            continue;
        }

        let (block_name, param_ids) = parse_block_header(line)
            .with_context(|| format!("failed to parse block header: {line}"))?;
        i += 1;

        let mut instrs = Vec::new();
        while i < lines.len()
        {
            let t = lines[i].trim();
            if t.starts_with("block")
            {
                break;
            }
            if !t.is_empty() && !t.starts_with(';')
            {
                instrs.push(t.to_string());
            }
            i += 1;
        }

        blocks.push(ParsedBlock { name: block_name, param_value_ids: param_ids, instructions: instrs });
    }

    if blocks.is_empty()
    {
        bail!("no blocks found");
    }

    Ok(blocks)
}

fn parse_block_header(line: &str) -> Result<(String, Vec<u32>)>
{
    // Examples:
    // block0():
    // block0(v0: i32, v1: i32):
    // block1:
    let line = line.trim();
    let colon = line.rfind(':').context("missing ':'")?;
    let head = line[..colon].trim();

    let (name, params_part) = if let Some(open) = head.find('(')
    {
        let close = head.rfind(')').context("missing ')'")?;
        (head[..open].trim(), Some(head[open + 1..close].trim()))
    }
    else
    {
        (head, None)
    };

    let mut ids = Vec::new();
    if let Some(p) = params_part
    {
        if !p.is_empty()
        {
            for item in p.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
            {
                // v0: i32
                let (v, _) = item.split_once(':').context("expected 'vN: ty'")?;
                let id = parse_value_id(v.trim())?;
                ids.push(id);
            }
        }
    }

    Ok((name.to_string(), ids))
}

fn parse_type(s: &str) -> Result<cranelift_codegen::ir::Type>
{
    Ok(match s
    {
        "i8" => types::I8,
        "i16" => types::I16,
        "i32" => types::I32,
        "i64" => types::I64,
        "f32" => types::F32,
        "f64" => types::F64,
        // Stasis "r64" is pointer-sized in the CLIF scaffolding; treat as i64 for now.
        "r64" => types::I64,
        other => bail!("unsupported type: {other}"),
    })
}

fn parse_type_with_count(s: &str) -> Result<(cranelift_codegen::ir::Type, usize)>
{
    if let Some(open) = s.find('[')
    {
        let close = s.rfind(']').context("missing ']' in array type")?;
        if close < open
        {
            bail!("invalid array type");
        }
        let base = s[..open].trim();
        let count_str = s[open + 1..close].trim();
        let count = count_str.parse::<usize>().context("invalid array length")?;
        let ty = parse_type(base)?;
        return Ok((ty, count));
    }

    Ok((parse_type(s)?, 1))
}

fn parse_value_id(s: &str) -> Result<u32>
{
    let s = s.strip_prefix('v').context("expected value like v0")?;
    Ok(s.parse::<u32>().context("invalid value id")?)
}

fn build_function_ir(
    module: &mut ObjectModule,
    function_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
    data_ids: &std::collections::HashMap<String, cranelift_module::DataId>,
    parsed: &ParsedFunction,
) -> Result<Function>
{
    let mut func = Function::with_name_signature(cranelift_codegen::ir::UserFuncName::testcase(&parsed.name), parsed.signature.clone());

    let mut func_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

        let mut blocks = std::collections::HashMap::<String, cranelift_codegen::ir::Block>::new();
        let mut values = std::collections::HashMap::<u32, cranelift_codegen::ir::Value>::new();
        let mut func_refs = std::collections::HashMap::<String, cranelift_codegen::ir::FuncRef>::new();
        let mut global_values = std::collections::HashMap::<String, GlobalValue>::new();

        for b in &parsed.blocks
        {
            let block = builder.create_block();
            blocks.insert(b.name.clone(), block);
        }

        // Treat the first block as the entry block.
        let entry_name = parsed.blocks[0].name.clone();
        let entry = *blocks.get(&entry_name).context("missing entry block")?;
        builder.switch_to_block(entry);

        // Map function parameters to v0..vN based on the block header.
        builder.append_block_params_for_function_params(entry);
        let params = builder.block_params(entry).to_vec();
        for (i, val) in params.into_iter().enumerate()
        {
            values.insert(i as u32, val);
        }

        for b in &parsed.blocks
        {
            let block = *blocks.get(&b.name).context("missing block")?;
            builder.switch_to_block(block);

            // For now we only support entry block params (function params).
            if b.name != entry_name && !b.param_value_ids.is_empty()
            {
                bail!("block parameters not supported yet ({})", b.name);
            }

            for inst_line in &b.instructions
            {
                emit_inst(module, function_ids, data_ids, &mut builder, &blocks, &mut values, &mut func_refs, &mut global_values, inst_line)
                    .with_context(|| format!("in {}: {inst_line}", parsed.name))?;
            }
        }

        builder.seal_all_blocks();
        builder.finalize();
    }

    Ok(func)
}

fn emit_inst(
    module: &mut ObjectModule,
    function_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
    data_ids: &std::collections::HashMap<String, cranelift_module::DataId>,
    builder: &mut FunctionBuilder,
    blocks: &std::collections::HashMap<String, cranelift_codegen::ir::Block>,
    values: &mut std::collections::HashMap<u32, cranelift_codegen::ir::Value>,
    func_refs: &mut std::collections::HashMap<String, cranelift_codegen::ir::FuncRef>,
    global_values: &mut std::collections::HashMap<String, GlobalValue>,
    line: &str,
) -> Result<()>
{
    let mut line = line.trim();
    if let Some((code, _)) = line.split_once(';')
    {
        line = code.trim();
    }
    if line.is_empty()
    {
        return Ok(());
    }

    if line == "return"
    {
        builder.ins().return_(&[]);
        return Ok(());
    }
    if let Some(rest) = line.strip_prefix("return ")
    {
        let id = parse_value_id(rest.trim())?;
        let v = *values.get(&id).context("unknown value in return")?;
        builder.ins().return_(&[v]);
        return Ok(());
    }

    if let Some(rest) = line.strip_prefix("jump ")
    {
        let target_name = rest.trim();
        let block = *blocks.get(target_name).context("unknown jump target block")?;
        builder.ins().jump(block, &[]);
        return Ok(());
    }

    if let Some(rest) = line.strip_prefix("brif ")
    {
        // brif v0, block1, block2
        let parts: Vec<_> = rest.split(',').map(|s| s.trim()).collect();
        if parts.len() != 3
        {
            bail!("invalid brif syntax");
        }
        let cond = *values.get(&parse_value_id(parts[0])?).context("unknown brif cond value")?;
        let then_block = *blocks.get(parts[1]).context("unknown then block")?;
        let else_block = *blocks.get(parts[2]).context("unknown else block")?;
        builder.ins().brif(cond, then_block, &[], else_block, &[]);
        return Ok(());
    }

    if let Some(rest) = line.strip_prefix("call %")
    {
        // call %foo(v0, v1)
        let open = rest.find('(').context("missing '(' in call")?;
        let close = rest.rfind(')').context("missing ')' in call")?;
        let callee = rest[..open].trim();
        let args_str = rest[open + 1..close].trim();
        let mut arg_vals = Vec::new();
        if !args_str.is_empty()
        {
            for a in args_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
            {
                let v = *values.get(&parse_value_id(a)?).context("unknown call arg")?;
                arg_vals.push(v);
            }
        }

        let callee_id = *function_ids.get(callee).with_context(|| format!("unknown callee {callee}"))?;
        let func_ref = *func_refs.entry(callee.to_string()).or_insert_with(|| module.declare_func_in_func(callee_id, builder.func));
        builder.ins().call(func_ref, &arg_vals);
        return Ok(());
    }

    // store <value>, <addr>
    if let Some(rest) = line.strip_prefix("store ")
    {
        let parts: Vec<_> = rest.split(',').map(|s| s.trim()).collect();
        if parts.len() != 2
        {
            bail!("invalid store syntax (expected: store vN, vM)");
        }
        let val_id = parse_value_id(parts[0])?;
        let addr_id = parse_value_id(parts[1])?;
        let val = *values.get(&val_id).context("unknown store value")?;
        let addr = *values.get(&addr_id).context("unknown store address")?;
        builder.ins().store(cranelift_codegen::ir::MemFlags::new(), val, addr, 0);
        return Ok(());
    }

    // vN = ...
    let (dst, rhs) = line.split_once('=').context("expected assignment")?;
    let dst_id = parse_value_id(dst.trim())?;
    let rhs = rhs.trim();

    if let Some(rest) = rhs.strip_prefix("iconst.i32 ")
    {
        let imm = rest.trim().parse::<i64>().context("invalid iconst.i32 immediate")?;
        let v = builder.ins().iconst(types::I32, imm);
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("iconst.i8 ")
    {
        let imm = rest.trim().parse::<i64>().context("invalid iconst.i8 immediate")?;
        let v = builder.ins().iconst(types::I8, imm);
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("iconst.i16 ")
    {
        let imm = rest.trim().parse::<i64>().context("invalid iconst.i16 immediate")?;
        let v = builder.ins().iconst(types::I16, imm);
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("iconst.i64 ")
    {
        let imm = rest.trim().parse::<i64>().context("invalid iconst.i64 immediate")?;
        let v = builder.ins().iconst(types::I64, imm);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("f32const ")
    {
        let imm = rest.trim().parse::<f32>().context("invalid f32const immediate")?;
        let v = builder.ins().f32const(Ieee32::with_float(imm));
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("f64const ")
    {
        let imm = rest.trim().parse::<f64>().context("invalid f64const immediate")?;
        let v = builder.ins().f64const(Ieee64::with_float(imm));
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("fcvt_from_sint.")
    {
        let ty_str = rest.split_whitespace().next().context("missing fcvt type")?;
        let value_str = rest[ty_str.len()..].trim();
        let src = *values.get(&parse_value_id(value_str)?).context("unknown fcvt source")?;
        let ty = parse_type(ty_str)?;
        let v = builder.ins().fcvt_from_sint(ty, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("fcvt_to_sint_sat.")
    {
        let ty_str = rest.split_whitespace().next().context("missing fcvt type")?;
        let value_str = rest[ty_str.len()..].trim();
        let src = *values.get(&parse_value_id(value_str)?).context("unknown fcvt_to_sint_sat source")?;
        let ty = parse_type(ty_str)?;
        let v = builder.ins().fcvt_to_sint_sat(ty, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("bint.i32 ")
    {
        let src = *values.get(&parse_value_id(rest.trim())?).context("unknown bint source")?;
        let v = builder.ins().uextend(types::I32, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("ireduce.")
    {
        let ty_str = rest.split_whitespace().next().context("missing ireduce type")?;
        let value_str = rest[ty_str.len()..].trim();
        let src = *values.get(&parse_value_id(value_str)?).context("unknown ireduce source")?;
        let ty = parse_type(ty_str)?;
        let v = builder.ins().ireduce(ty, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("uextend.i32 ")
    {
        let src = *values.get(&parse_value_id(rest.trim())?).context("unknown uextend source")?;
        let v = builder.ins().uextend(types::I32, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("select ")
    {
        let parts: Vec<_> = rest.split(',').map(|s| s.trim()).collect();
        if parts.len() != 3
        {
            bail!("invalid select syntax");
        }
        let cond = *values.get(&parse_value_id(parts[0])?).context("unknown select cond")?;
        let a = *values.get(&parse_value_id(parts[1])?).context("unknown select true")?;
        let b = *values.get(&parse_value_id(parts[2])?).context("unknown select false")?;
        let v = builder.ins().select(cond, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }

    // sextend.i64 <value>
    if let Some(rest) = rhs.strip_prefix("sextend.i64 ")
    {
        let src = *values.get(&parse_value_id(rest.trim())?).context("unknown sextend source")?;
        let v = builder.ins().sextend(types::I64, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("icmp ")
    {
        // icmp eq v0, v1
        let mut parts = rest.split_whitespace();
        let cc = parts.next().context("missing icmp condition")?;
        let remaining = parts.collect::<Vec<_>>().join(" ");
        let ops: Vec<_> = remaining.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if ops.len() != 2
        {
            bail!("invalid icmp operands");
        }
        let a = *values.get(&parse_value_id(ops[0])?).context("unknown icmp lhs")?;
        let b = *values.get(&parse_value_id(ops[1])?).context("unknown icmp rhs")?;
        let cc = match cc
        {
            "eq" => IntCC::Equal,
            "ne" => IntCC::NotEqual,
            "slt" => IntCC::SignedLessThan,
            "sle" => IntCC::SignedLessThanOrEqual,
            "sgt" => IntCC::SignedGreaterThan,
            "sge" => IntCC::SignedGreaterThanOrEqual,
            "ult" => IntCC::UnsignedLessThan,
            "ule" => IntCC::UnsignedLessThanOrEqual,
            "ugt" => IntCC::UnsignedGreaterThan,
            "uge" => IntCC::UnsignedGreaterThanOrEqual,
            other => bail!("unsupported icmp condition: {other}"),
        };
        let v = builder.ins().icmp(cc, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("fcmp ")
    {
        // fcmp lt v0, v1
        let mut parts = rest.split_whitespace();
        let cc = parts.next().context("missing fcmp condition")?;
        let remaining = parts.collect::<Vec<_>>().join(" ");
        let ops: Vec<_> = remaining.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if ops.len() != 2
        {
            bail!("invalid fcmp operands");
        }
        let a = *values.get(&parse_value_id(ops[0])?).context("unknown fcmp lhs")?;
        let b = *values.get(&parse_value_id(ops[1])?).context("unknown fcmp rhs")?;
        let cc = match cc
        {
            "eq" => FloatCC::Equal,
            "ne" => FloatCC::NotEqual,
            "lt" => FloatCC::LessThan,
            "le" => FloatCC::LessThanOrEqual,
            "gt" => FloatCC::GreaterThan,
            "ge" => FloatCC::GreaterThanOrEqual,
            other => bail!("unsupported fcmp condition: {other}"),
        };
        let v = builder.ins().fcmp(cc, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }

    for op in ["iadd", "isub", "imul", "sdiv", "srem", "band", "bor", "fadd", "fsub", "fmul", "fdiv"]
    {
        if let Some(rest) = rhs.strip_prefix(op)
        {
            let ops: Vec<_> = rest.trim().split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
            if ops.len() != 2
            {
                bail!("invalid {op} operands");
            }
            let a = *values.get(&parse_value_id(ops[0])?).context("unknown lhs")?;
            let c = *values.get(&parse_value_id(ops[1])?).context("unknown rhs")?;

            let v = match op
            {
                "iadd" => builder.ins().iadd(a, c),
                "isub" => builder.ins().isub(a, c),
                "imul" => builder.ins().imul(a, c),
                "sdiv" => builder.ins().sdiv(a, c),
                "srem" => builder.ins().srem(a, c),
                "band" => builder.ins().band(a, c),
                "bor" => builder.ins().bor(a, c),
                "fadd" => builder.ins().fadd(a, c),
                "fsub" => builder.ins().fsub(a, c),
                "fmul" => builder.ins().fmul(a, c),
                "fdiv" => builder.ins().fdiv(a, c),
                _ => unreachable!(),
            };

            values.insert(dst_id, v);
            return Ok(());
        }
    }

    if let Some(rest) = rhs.strip_prefix("call %")
    {
        // call %add(v0, v1)
        let open = rest.find('(').context("missing '(' in call")?;
        let close = rest.rfind(')').context("missing ')' in call")?;
        let callee = rest[..open].trim();
        let args_str = rest[open + 1..close].trim();
        let mut arg_vals = Vec::new();
        if !args_str.is_empty()
        {
            for a in args_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
            {
                let v = *values.get(&parse_value_id(a)?).context("unknown call arg")?;
                arg_vals.push(v);
            }
        }

        let callee_id = *function_ids.get(callee).with_context(|| format!("unknown callee {callee}"))?;
        let func_ref = *func_refs.entry(callee.to_string()).or_insert_with(|| module.declare_func_in_func(callee_id, builder.func));
        let call: Inst = builder.ins().call(func_ref, &arg_vals);
        let results = builder.func.dfg.inst_results(call);
        if results.len() != 1
        {
            bail!("call result count != 1");
        }
        values.insert(dst_id, results[0]);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("stack_slot.")
    {
        let ty_str = rest.trim();
        let ty = parse_type(ty_str)?;
        let size = ty.bytes() as u32;
        let align_shift = match size
        {
            1 => 0,
            2 => 1,
            4 => 2,
            8 => 3,
            16 => 4,
            _ => 0,
        };
        let slot = builder.create_sized_stack_slot(StackSlotData::new(StackSlotKind::ExplicitSlot, size, align_shift));
        let addr = builder.ins().stack_addr(types::I64, slot, 0);
        values.insert(dst_id, addr);
        return Ok(());
    }

    // global_value <global_name>
    if let Some(rest) = rhs.strip_prefix("global_value ")
    {
        let global_name = rest.trim();
        let data_id = *data_ids.get(global_name).with_context(|| format!("unknown global {global_name}"))?;
        let gv = *global_values.entry(global_name.to_string()).or_insert_with(|| {
            module.declare_data_in_func(data_id, builder.func)
        });
        let addr = builder.ins().global_value(types::I64, gv);
        values.insert(dst_id, addr);
        return Ok(());
    }

    // load.i32 <addr>
    // load.i64 <addr>
    // load.f32 <addr>
    // load.f64 <addr>
    // load.r64 <addr>
    for ty_str in ["i8", "i16", "i32", "i64", "f32", "f64", "r64"]
    {
        let prefix = format!("load.{} ", ty_str);
        if let Some(rest) = rhs.strip_prefix(&prefix)
        {
            let addr_id = parse_value_id(rest.trim())?;
            let addr = *values.get(&addr_id).context("unknown load address")?;
            let ty = parse_type(ty_str)?;
            let v = builder.ins().load(ty, cranelift_codegen::ir::MemFlags::new(), addr, 0);
            values.insert(dst_id, v);
            return Ok(());
        }
    }

    bail!("unsupported instruction: {rhs}")
}
