use std::fs;
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{bail, Context, Result};
use clap::Parser;
use cranelift_codegen::isa;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_codegen::ir::{types, AbiParam, Function, Inst, InstBuilder, Signature};
use cranelift_codegen::isa::CallConv;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{default_libcall_names, Linkage, Module};
use cranelift_object::{ObjectBuilder, ObjectModule};
use target_lexicon::Triple;

#[derive(Parser)]
#[command(name = "stasis-cranelift-aot")]
#[command(about = "Compile Cranelift CLIF into a native object file (COFF on Windows).")]
struct Args
{
    /// Input CLIF file path.
    #[arg(long, value_name = "PATH")]
    input: PathBuf,

    /// Output object file path.
    #[arg(long, value_name = "PATH")]
    output: PathBuf,

    /// Target triple (default: x86_64-pc-windows-msvc).
    #[arg(long, value_name = "TRIPLE", default_value = "x86_64-pc-windows-msvc")]
    target: String,

    /// Module name embedded in the object.
    #[arg(long, value_name = "NAME", default_value = "stasis_module")]
    module_name: String,

    /// Optimization level (none|speed|speed_and_size).
    #[arg(long, value_name = "LEVEL", default_value = "none")]
    opt_level: String,
}

fn main() -> Result<()>
{
    let args = Args::parse();

    let clif = fs::read_to_string(&args.input)
        .with_context(|| format!("failed to read input file: {}", args.input.display()))?;

    let triple = Triple::from_str(&args.target)
        .map_err(|_| anyhow::anyhow!("invalid target triple: {}", args.target))?;

    let flags = build_flags(&args.opt_level)?;
    let isa = isa::lookup(triple.clone())
        .context("failed to look up ISA for target")?
        .finish(flags)
        .context("failed to finalize ISA")?;

    let builder = ObjectBuilder::new(isa, args.module_name, default_libcall_names())
        .context("failed to create ObjectBuilder")?;
    let mut module = ObjectModule::new(builder);

    let parsed = parse_stasis_clif(&clif).context("failed to parse stasis CLIF")?;

    // First declare all functions so intra-module calls can resolve.
    let mut function_ids = std::collections::HashMap::new();
    for f in &parsed
    {
        let id = module
            .declare_function(&f.name, Linkage::Export, &f.signature)
            .with_context(|| format!("declare_function failed for {}", f.name))?;
        function_ids.insert(f.name.clone(), id);
    }

    // Then define each function body.
    for f in parsed
    {
        let mut context = module.make_context();
        context.func = build_function_ir(&mut module, &function_ids, &f)
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

    fs::write(&args.output, obj_bytes)
        .with_context(|| format!("failed to write object file: {}", args.output.display()))?;

    Ok(())
}

fn build_flags(opt_level: &str) -> Result<settings::Flags>
{
    let mut flag_builder = settings::builder();

    match opt_level
    {
        "none" => flag_builder.set("opt_level", "none")?,
        "speed" => flag_builder.set("opt_level", "speed")?,
        "speed_and_size" => flag_builder.set("opt_level", "speed_and_size")?,
        other => bail!("invalid --opt-level '{other}' (use none|speed|speed_and_size)"),
    }

    Ok(settings::Flags::new(flag_builder))
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

fn parse_stasis_clif(input: &str) -> Result<Vec<ParsedFunction>>
{
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

        if !line.starts_with("function %")
        {
            lines.next();
            continue;
        }

        let (_, header_line) = lines.next().unwrap();
        let header = header_line.trim();

        let (name, signature) = parse_function_header(header)
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

    Ok(funcs)
}

fn parse_function_header(line: &str) -> Result<(String, Signature)>
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

    let mut sig = Signature::new(CallConv::WindowsFastcall);

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

fn parse_value_id(s: &str) -> Result<u32>
{
    let s = s.strip_prefix('v').context("expected value like v0")?;
    Ok(s.parse::<u32>().context("invalid value id")?)
}

fn build_function_ir(
    module: &mut ObjectModule,
    function_ids: &std::collections::HashMap<String, cranelift_module::FuncId>,
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
                emit_inst(module, function_ids, &mut builder, &blocks, &mut values, &mut func_refs, inst_line)
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
    builder: &mut FunctionBuilder,
    blocks: &std::collections::HashMap<String, cranelift_codegen::ir::Block>,
    values: &mut std::collections::HashMap<u32, cranelift_codegen::ir::Value>,
    func_refs: &mut std::collections::HashMap<String, cranelift_codegen::ir::FuncRef>,
    line: &str,
) -> Result<()>
{
    let line = line.trim();

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
    if let Some(rest) = rhs.strip_prefix("iconst.i64 ")
    {
        let imm = rest.trim().parse::<i64>().context("invalid iconst.i64 immediate")?;
        let v = builder.ins().iconst(types::I64, imm);
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
            other => bail!("unsupported icmp condition: {other}"),
        };
        let v = builder.ins().icmp(cc, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }

    for op in ["iadd", "isub", "imul", "sdiv", "srem"]
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

    bail!("unsupported instruction: {rhs}")
}
