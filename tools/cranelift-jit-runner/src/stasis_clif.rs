use std::collections::HashMap;

use anyhow::{bail, Context, Result};
use cranelift_codegen::ir::condcodes::{FloatCC, IntCC};
use cranelift_codegen::ir::immediates::{Ieee32, Ieee64};
use cranelift_codegen::ir::{
    types, AbiParam, Function, GlobalValue, Inst, InstBuilder, Signature, StackSlotData, StackSlotKind,
};
use cranelift_codegen::isa::CallConv;
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};
use cranelift_module::{DataId, FuncId, Module};

pub(crate) struct ParsedModule
{
    pub(crate) globals: Vec<ParsedGlobal>,
    pub(crate) externals: Vec<ParsedExternal>,
    pub(crate) functions: Vec<ParsedFunction>,
}

#[derive(Clone)]
pub(crate) struct ParsedGlobal
{
    pub(crate) name: String,
    pub(crate) init_data: GlobalInitData,
    pub(crate) size_bytes: usize,
}

#[derive(Clone)]
pub(crate) enum GlobalInitData
{
    Zero,
    String(Vec<u8>), // Null-terminated string bytes
}

#[derive(Clone)]
pub(crate) struct ParsedExternal
{
    pub(crate) name: String,
    pub(crate) signature: Signature,
}

#[derive(Clone)]
pub(crate) struct ParsedFunction
{
    pub(crate) name: String,
    pub(crate) signature: Signature,
    blocks: Vec<ParsedBlock>,
}

#[derive(Clone)]
struct ParsedBlock
{
    name: String,
    param_value_ids: Vec<u32>,
    instructions: Vec<String>,
}

pub(crate) fn parse_stasis_clif(input: &str, default_cc: CallConv) -> Result<ParsedModule>
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

        // Parse global declarations.
        if line.starts_with("global ")
        {
            lines.next();
            let (name, _ty, init_data, size_bytes) = parse_global_decl(line)
                .with_context(|| format!("failed to parse global declaration: {line}"))?;
            globals.push(ParsedGlobal { name, init_data, size_bytes });
            continue;
        }

        // Parse external function declarations.
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

        // block0(v0: i32, v1: i32):
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
    // Example:
    // global cstr_0: i8[19] ; bytes: 1B 5B ...
    // global host_i32: i32[768]
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
                c =>
                {
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
    // external printf3(i64, i64, i64) -> i32 windows_fastcall
    // external stasis_sys_memcpy_u8(r64, i32, r64, i32, i32) windows_fastcall
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
        return Ok((name, sig));
    }

    // No explicit return type; optional callconv.
    let parts: Vec<_> = after.split_whitespace().collect();
    if !parts.is_empty()
    {
        sig.call_conv = parse_callconv(parts[0], default_cc)?;
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
        for p in param_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            // allow "i32" or "v0: i32" (we ignore names)
            let ty_str = if let Some((_v, ty)) = p.split_once(':') { ty.trim() } else { p };
            sig.params.push(AbiParam::new(parse_type(ty_str)?));
        }
    }

    if let Some((_, rest_after)) = after.split_once("->")
    {
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
    else
    {
        // No return type; parse optional callconv before '{'
        let after = after.trim_end_matches('{').trim();
        if !after.is_empty()
        {
            let parts: Vec<_> = after.split_whitespace().collect();
            if !parts.is_empty()
            {
                sig.call_conv = parse_callconv(parts[0], default_cc)?;
            }
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
    let s = s.trim().trim_end_matches(',').trim_end_matches(')').trim_end_matches(':').trim();
    let s = s.strip_prefix('v').context("expected value like v0")?;
    Ok(s.parse::<u32>().context("invalid value id")?)
}

pub(crate) fn build_function_ir<M: Module>(
    module: &mut M,
    function_ids: &HashMap<String, FuncId>,
    data_ids: &HashMap<String, DataId>,
    parsed: &ParsedFunction,
) -> Result<Function>
{
    let mut func = Function::with_name_signature(
        cranelift_codegen::ir::UserFuncName::testcase(&parsed.name),
        parsed.signature.clone(),
    );

    let mut func_ctx = FunctionBuilderContext::new();
    {
        let mut builder = FunctionBuilder::new(&mut func, &mut func_ctx);

        let mut blocks = HashMap::<String, cranelift_codegen::ir::Block>::new();
        let mut values = HashMap::<u32, cranelift_codegen::ir::Value>::new();
        let mut func_refs = HashMap::<String, cranelift_codegen::ir::FuncRef>::new();
        let mut global_values = HashMap::<String, GlobalValue>::new();

        for b in &parsed.blocks
        {
            let block = builder.create_block();
            blocks.insert(b.name.clone(), block);
        }

        // Treat the first block as the entry block.
        let entry_block = *blocks.get(&parsed.blocks[0].name).context("missing entry block")?;
        builder.switch_to_block(entry_block);

        builder.append_block_params_for_function_params(entry_block);
        let params = builder.block_params(entry_block).to_vec();
        if !parsed.blocks[0].param_value_ids.is_empty()
        {
            if parsed.blocks[0].param_value_ids.len() != params.len()
            {
                bail!(
                    "entry block param count mismatch: header has {} params but signature has {}",
                    parsed.blocks[0].param_value_ids.len(),
                    params.len()
                );
            }
            for (id, val) in parsed.blocks[0].param_value_ids.iter().copied().zip(params.into_iter())
            {
                values.insert(id, val);
            }
        }
        else
        {
            for (i, val) in params.into_iter().enumerate()
            {
                values.insert(i as u32, val);
            }
        }

        for b in &parsed.blocks
        {
            let block = *blocks.get(&b.name).context("missing block")?;
            builder.switch_to_block(block);

            // For now we only support entry block params (function params).
            if b.name != parsed.blocks[0].name && !b.param_value_ids.is_empty()
            {
                bail!("block parameters not supported yet ({})", b.name);
            }

            for inst_line in &b.instructions
            {
                lower_instruction(
                    module,
                    function_ids,
                    data_ids,
                    &mut builder,
                    &blocks,
                    &mut values,
                    &mut func_refs,
                    &mut global_values,
                    inst_line,
                )
                .with_context(|| format!("in {}: {inst_line}", parsed.name))?;
            }
        }

        builder.seal_all_blocks();
        builder.finalize();
    }

    Ok(func)
}

fn lower_instruction<M: Module>(
    module: &mut M,
    function_ids: &HashMap<String, FuncId>,
    data_ids: &HashMap<String, DataId>,
    builder: &mut FunctionBuilder,
    blocks: &HashMap<String, cranelift_codegen::ir::Block>,
    values: &mut HashMap<u32, cranelift_codegen::ir::Value>,
    func_refs: &mut HashMap<String, cranelift_codegen::ir::FuncRef>,
    global_values: &mut HashMap<String, GlobalValue>,
    line: &str,
) -> Result<()>
{
    let mut trimmed = line.trim();
    if let Some((code, _)) = trimmed.split_once(';')
    {
        trimmed = code.trim();
    }
    if trimmed.is_empty()
    {
        return Ok(());
    }

    // return v0 / return
    if trimmed == "return"
    {
        builder.ins().return_(&[]);
        return Ok(());
    }
    if let Some(rest) = trimmed.strip_prefix("return ")
    {
        let v = *values.get(&parse_value_id(rest.trim())?).context("unknown return value")?;
        builder.ins().return_(&[v]);
        return Ok(());
    }

    if let Some(rest) = trimmed.strip_prefix("jump ")
    {
        let target_name = rest.trim();
        let block = *blocks.get(target_name).context("unknown jump target block")?;
        builder.ins().jump(block, &[]);
        return Ok(());
    }

    if let Some(rest) = trimmed.strip_prefix("brif ")
    {
        // brif v0, block1, block2
        let parts: Vec<_> = rest.split(',').map(|s| s.trim()).collect();
        if parts.len() != 3
        {
            bail!("invalid brif syntax");
        }
        let cond = *values
            .get(&parse_value_id(parts[0])?)
            .context("unknown brif cond value")?;
        let then_block = *blocks.get(parts[1]).context("unknown then block")?;
        let else_block = *blocks.get(parts[2]).context("unknown else block")?;
        builder.ins().brif(cond, then_block, &[], else_block, &[]);
        return Ok(());
    }

    // call %foo(...)
    if let Some(rest) = trimmed.strip_prefix("call %")
    {
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
        let callee_id = *function_ids
            .get(callee)
            .with_context(|| format!("unknown callee {callee}"))?;
        let func_ref = if let Some(r) = func_refs.get(callee)
        {
            *r
        }
        else
        {
            let r = module.declare_func_in_func(callee_id, builder.func);
            func_refs.insert(callee.to_string(), r);
            r
        };
        builder.ins().call(func_ref, &arg_vals);
        return Ok(());
    }

    // v0 = ...
    let Some((dst, rhs)) = trimmed.split_once('=') else
    {
        // store v0, v1
        if let Some(rest) = trimmed.strip_prefix("store ")
        {
            let parts: Vec<_> = rest.split(',').map(|s| s.trim()).collect();
            if parts.len() != 2
            {
                bail!("invalid store: {trimmed}");
            }
            let src = *values.get(&parse_value_id(parts[0])?).context("unknown store src")?;
            let addr = *values.get(&parse_value_id(parts[1])?).context("unknown store addr")?;
            builder
                .ins()
                .store(cranelift_codegen::ir::MemFlags::new(), src, addr, 0);
            return Ok(());
        }

        bail!("unsupported instruction: {trimmed}");
    };

    let dst_id = parse_value_id(dst.trim())?;
    let rhs = rhs.trim();

    // iconst.i32 0
    if let Some(rest) = rhs.strip_prefix("iconst.")
    {
        let mut parts = rest.split_whitespace();
        let ty_str = parts.next().context("missing iconst type")?;
        let imm_str = parts.next().context("missing iconst value")?;
        let ty = parse_type(ty_str)?;
        let imm = imm_str.parse::<i64>().context("invalid iconst immediate")?;
        let v = builder.ins().iconst(ty, imm);
        values.insert(dst_id, v);
        return Ok(());
    }

    // f32const 1.0 / f64const 1.0
    if let Some(rest) = rhs.strip_prefix("f32const ")
    {
        let f = rest.trim().parse::<f32>().context("invalid f32const")?;
        let v = builder.ins().f32const(Ieee32::with_float(f));
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("f64const ")
    {
        let f = rest.trim().parse::<f64>().context("invalid f64const")?;
        let v = builder.ins().f64const(Ieee64::with_float(f));
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

    // unary conversions
    // - sextend.i64 v0
    // - uextend.i32 v0 / uextend.i64 v0
    // - bint.i32 v0 (stasis emits this; we treat it as zero-extend)
    for (op, ty_str) in [("sextend.", "i64"), ("uextend.", "i32"), ("uextend.", "i64")]
    {
        let prefix = format!("{op}{ty_str} ");
        if let Some(rest) = rhs.strip_prefix(&prefix)
        {
            let src = *values.get(&parse_value_id(rest.trim())?).context("unknown conversion src")?;
            let ty = parse_type(ty_str)?;
            let v = match op
            {
                "sextend." => builder.ins().sextend(ty, src),
                "uextend." => builder.ins().uextend(ty, src),
                _ => unreachable!(),
            };
            values.insert(dst_id, v);
            return Ok(());
        }
    }

    if let Some(rest) = rhs.strip_prefix("bint.i32 ")
    {
        let src = *values.get(&parse_value_id(rest.trim())?).context("unknown bint source")?;
        let v = builder.ins().uextend(types::I32, src);
        values.insert(dst_id, v);
        return Ok(());
    }

    if let Some(rest) = rhs.strip_prefix("bint.i64 ")
    {
        let src = *values.get(&parse_value_id(rest.trim())?).context("unknown bint source")?;
        let v = builder.ins().uextend(types::I64, src);
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

    // integer/fp arithmetic: iadd v0, v1 etc.
    for op in ["iadd", "isub", "imul", "sdiv", "srem", "band", "bor", "fadd", "fsub", "fmul", "fdiv"]
    {
        let prefix = format!("{op} ");
        if let Some(rest) = rhs.strip_prefix(&prefix)
        {
            let ops: Vec<_> = rest.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
            if ops.len() != 2
            {
                bail!("invalid {op} operands: {rhs}");
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

    // call %callee(v0, v1)
    if let Some(rest) = rhs.strip_prefix("call %")
    {
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
        let func_ref = if let Some(r) = func_refs.get(callee) { *r } else { let r = module.declare_func_in_func(callee_id, builder.func); func_refs.insert(callee.to_string(), r); r };
        let call: Inst = builder.ins().call(func_ref, &arg_vals);
        let results = builder.func.dfg.inst_results(call);
        if results.len() != 1
        {
            bail!("call result count != 1");
        }
        values.insert(dst_id, results[0]);
        return Ok(());
    }

    // stack_slot.i64
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
        let gv = if let Some(gv) = global_values.get(global_name) { *gv } else { let gv = module.declare_data_in_func(data_id, builder.func); global_values.insert(global_name.to_string(), gv); gv };
        let addr = builder.ins().global_value(types::I64, gv);
        values.insert(dst_id, addr);
        return Ok(());
    }

    // load.<ty> <addr>
    for ty_str in ["i8", "i16", "i32", "i64", "f32", "f64", "r64"]
    {
        let prefix = format!("load.{ty_str} ");
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

    // icmp <cc> v0, v1 / fcmp <cc> v0, v1
    if let Some(rest) = rhs.strip_prefix("icmp ")
    {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 3
        {
            bail!("invalid icmp: {rhs}");
        }
        let cc = parse_intcc(parts[0])?;
        let a = *values.get(&parse_value_id(parts[1])?).context("unknown icmp lhs")?;
        let b = *values.get(&parse_value_id(parts[2])?).context("unknown icmp rhs")?;
        let v = builder.ins().icmp(cc, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }
    if let Some(rest) = rhs.strip_prefix("fcmp ")
    {
        let parts: Vec<_> = rest.split_whitespace().collect();
        if parts.len() != 3
        {
            bail!("invalid fcmp: {rhs}");
        }
        let cc = parse_floatcc(parts[0])?;
        let a = *values.get(&parse_value_id(parts[1])?).context("unknown fcmp lhs")?;
        let b = *values.get(&parse_value_id(parts[2])?).context("unknown fcmp rhs")?;
        let v = builder.ins().fcmp(cc, a, b);
        values.insert(dst_id, v);
        return Ok(());
    }

    bail!("unsupported instruction: {rhs}")
}

fn parse_intcc(s: &str) -> Result<IntCC>
{
    Ok(match s
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
        other => bail!("unsupported intcc: {other}"),
    })
}

fn parse_floatcc(s: &str) -> Result<FloatCC>
{
    Ok(match s
    {
        "eq" => FloatCC::Equal,
        "ne" => FloatCC::NotEqual,
        "lt" => FloatCC::LessThan,
        "le" => FloatCC::LessThanOrEqual,
        "gt" => FloatCC::GreaterThan,
        "ge" => FloatCC::GreaterThanOrEqual,
        other => bail!("unsupported floatcc: {other}"),
    })
}

fn parse_block_target(s: &str) -> Result<(&str, Vec<u32>)>
{
    // block1(v2, v3) or block1
    let s = s.trim();
    if let Some((name, rest)) = s.split_once('(')
    {
        let args = rest.trim().trim_end_matches(')').trim();
        let mut out = Vec::new();
        if !args.is_empty()
        {
            for a in args.split(',').map(|x| x.trim()).filter(|x| !x.is_empty())
            {
                out.push(parse_value_id(a)?);
            }
        }
        Ok((name.trim(), out))
    }
    else
    {
        Ok((s, Vec::new()))
    }
}

fn resolve_values(values: &HashMap<u32, cranelift_codegen::ir::Value>, ids: &[u32]) -> Result<Vec<cranelift_codegen::ir::Value>>
{
    let mut out = Vec::with_capacity(ids.len());
    for &id in ids
    {
        out.push(*values.get(&id).with_context(|| format!("unknown value v{id}"))?);
    }
    Ok(out)
}

fn find_block(builder: &FunctionBuilder, name: &str) -> Result<cranelift_codegen::ir::Block>
{
    // Cranelift's FunctionBuilder doesn't provide a name->block map; Stasis CLIF uses stable block names
    // like "block0", so we can rely on block indices matching parse order by scanning layout.
    // As a compromise, we also check the SSA value table for block label annotations.
    let _ = builder;
    // The caller already creates blocks in parse order and switches to each; use that ordering:
    let idx = name
        .strip_prefix("block")
        .context("expected block name like block0")?
        .trim_end_matches(':')
        .parse::<usize>()
        .context("invalid block index")?;
    let mut n = 0usize;
    for b in builder.func.layout.blocks()
    {
        if n == idx
        {
            return Ok(b);
        }
        n += 1;
    }
    bail!("block index out of range: {name}")
}
