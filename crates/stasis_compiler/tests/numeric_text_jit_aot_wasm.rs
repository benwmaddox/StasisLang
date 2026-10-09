use num_bigint::BigUint;
use object::{Object, ObjectSymbol};
use stasis_compiler::backend::{
    aot::AotProcess,
    jit::{JitProcess, JitScalarValue},
    wasm::WasmProcess,
};
use stasis_compiler::compiler::Compiler;
#[cfg(windows)]
use stasis_jit::{AotLinkConfig, AotTarget};
use std::fs;
#[cfg(windows)]
use std::path::{Path, PathBuf};
use std::process::Command;

const STDLIB: &str = include_str!("../../../src/stdlib/stdlib.stasis");
const MEMORY: &str = include_str!("../../../src/stdlib/memory.stasis");
const HARNESS: &str = r#"
import "stdlib.stasis";

global numeric_i32: i32;
global numeric_f32: f32;
global numeric_decimals: i32;
global numeric_width: i32;
global numeric_fill: u8;
global numeric_out: ascii[64];
global numeric_utf8_out: utf8[64];
global numeric_utf8_exact: utf8[12];
global numeric_utf8_short: utf8[11];
global numeric_utf8_nested: NumericTextState;
global numeric_ascii_out: ascii[64];
global numeric_i32_exact: ascii[12];
global numeric_i32_short: ascii[11];
global numeric_f32_exact: ascii[48];
global numeric_f32_short: ascii[47];
global numeric_small: ascii[8];

struct NumericTextState {
    text: utf8[12];
}

function numeric_from_i32(): i32 {
    return ascii_from_i32(numeric_out, numeric_i32);
}

function numeric_from_f32(): i32 {
    return ascii_from_f32_fixed(numeric_out, numeric_f32, numeric_decimals);
}

function numeric_utf8_receiver_i32(): i32 {
    return numeric_utf8_out.from_i32(numeric_i32);
}

function numeric_utf8_function_i32(): i32 {
    return utf8_from_i32(numeric_utf8_out, numeric_i32);
}

function numeric_utf8_exact_i32(): i32 {
    return numeric_utf8_exact.from_i32(numeric_i32);
}

function numeric_utf8_short_i32(): i32 {
    return numeric_utf8_short.from_i32(numeric_i32);
}

function numeric_utf8_nested_i32(): i32 {
    return numeric_utf8_nested.text.from_i32(numeric_i32);
}

function numeric_ascii_receiver_i32(): i32 {
    return numeric_ascii_out.from_i32(numeric_i32);
}

function numeric_utf8_receiver_from_ascii(): i32 {
    return numeric_utf8_out.from_ascii(numeric_ascii_out, 64);
}

function numeric_scalar_receiver_i32(): i32 {
    let converted: f32 = 0.0;
    let result: i32 = 0;
    converted.from_i32(numeric_i32);
    result.from_f32(converted);
    return result;
}

function numeric_append_i32(): i32 {
    return ascii_append_i32(numeric_out, numeric_i32);
}

function numeric_pad_left(): i32 {
    return ascii_pad_left(numeric_out, numeric_width, numeric_fill);
}

function numeric_i32_exact_capacity(): i32 {
    return ascii_from_i32(numeric_i32_exact, numeric_i32);
}

function numeric_i32_short_capacity(): i32 {
    return ascii_from_i32(numeric_i32_short, numeric_i32);
}

function numeric_f32_exact_capacity(): i32 {
    return ascii_from_f32_fixed(numeric_f32_exact, numeric_f32, 6);
}

function numeric_f32_short_capacity(): i32 {
    return ascii_from_f32_fixed(numeric_f32_short, numeric_f32, 6);
}

function numeric_small_append(): i32 {
    return ascii_append_i32(numeric_small, numeric_i32);
}
"#;

#[cfg(windows)]
fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn source_f32(value: f32) -> String {
    let bits = value.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let fraction = bits & 0x7f_ffff;
    let (mantissa, exp2) = if exponent == 0 {
        (fraction, -149)
    } else {
        ((1 << 23) + fraction, exponent as i32 - 150)
    };
    format!("numeric_make_f32({mantissa}, {exp2}, {})", bits >> 31 != 0)
}

fn backend_selfcheck_source() -> String {
    let mut source = String::from(
        r#"
import "stdlib.stasis";

global numeric_backend_out: ascii[64];
global numeric_backend_utf8: utf8[12];

function numeric_make_f32(mantissa: i32, exp2: i32, negative: bool): f32 {
    let value: f32 = i32_to_f32(mantissa);
    let i: i32 = 0;
    for (i = 0; i < 192; i = i + 1) {
        if (i < exp2) {
            value = value * 2.0;
        }
        if (i < 0 - exp2) {
            value = value / 2.0;
        }
    }
    if (negative) {
        return value * -1.0;
    }
    return value;
}

function numeric_is_negative_zero(value: f32): bool {
    return value == 0.0 && 1.0 / value < 0.0;
}

function main(): i32 {
    let status: i32 = 0;
"#,
    );
    let mut failure = 1;
    source.push_str(&format!(
        "    if (numeric_is_negative_zero(numeric_make_f32(0, -149, true)) == false) {{ return {failure}; }}\n"
    ));
    failure += 1;
    for (value, expected) in [
        (i32::MIN, "-2147483648"),
        (i32::MAX, "2147483647"),
        (0, "0"),
    ] {
        let expression = if value == i32::MIN {
            "0 - 2147483647 - 1".to_string()
        } else {
            value.to_string()
        };
        source.push_str(&format!(
            "    status = ascii_from_i32(numeric_backend_out, {expression});\n    if (status != {}) {{ return {failure}; }}\n",
            expected.len()
        ));
        failure += 1;
        for (index, byte) in expected.bytes().enumerate() {
            source.push_str(&format!(
                "    if (numeric_backend_out[{index}] != {byte}) {{ return {failure}; }}\n"
            ));
            failure += 1;
        }
        source.push_str(&format!(
            "    if (numeric_backend_out[{}] != 0) {{ return {failure}; }}\n",
            expected.len()
        ));
        failure += 1;
        source.push_str(&format!(
            "    status = numeric_backend_utf8.from_i32({expression});\n    if (status != {} || numeric_backend_utf8.length != {} || numeric_backend_utf8.char_length != {}) {{ return {failure}; }}\n",
            expected.len(),
            expected.len(),
            expected.len()
        ));
        failure += 1;
        for (index, byte) in expected.bytes().enumerate() {
            source.push_str(&format!(
                "    if (numeric_backend_utf8[{index}] != {byte}) {{ return {failure}; }}\n"
            ));
            failure += 1;
        }
        source.push_str(&format!(
            "    if (numeric_backend_utf8[{}] != 0) {{ return {failure}; }}\n",
            expected.len()
        ));
        failure += 1;
    }
    let mut cases = vec![
        (0.0_f32, 0_u32),
        (-0.0, 6),
        (f32::from_bits(1), 6),
        (-f32::from_bits(0x007f_ffff), 6),
        (f32::MIN_POSITIVE, 6),
        (f32::MAX, 6),
        (-f32::MAX, 6),
        (1.25, 1),
        (-1.25, 1),
        (9.999_999, 5),
        (-0.004, 2),
    ];
    for decimals in 0_u32..=6 {
        let tie = 2.0_f32.powi(-(decimals as i32 + 1));
        for raw in [tie.to_bits() - 1, tie.to_bits(), tie.to_bits() + 1] {
            cases.push((f32::from_bits(raw), decimals));
            cases.push((-f32::from_bits(raw), decimals));
        }
    }
    for (value, decimals) in cases {
        let expected = oracle_f32_fixed(value, decimals).expect("finite backend case");
        source.push_str(&format!(
            "    status = ascii_from_f32_fixed(numeric_backend_out, {}, {decimals});\n    if (status != {}) {{ return {failure}; }}\n",
            source_f32(value),
            expected.len()
        ));
        failure += 1;
        for (index, byte) in expected.bytes().enumerate() {
            source.push_str(&format!(
                "    if (numeric_backend_out[{index}] != {byte}) {{ return {failure}; }}\n"
            ));
            failure += 1;
        }
        source.push_str(&format!(
            "    if (numeric_backend_out[{}] != 0) {{ return {failure}; }}\n",
            expected.len()
        ));
        failure += 1;
    }
    source.push_str(&format!(
        "    ascii_copy(numeric_backend_out, \"keep\");\n    status = ascii_from_f32_fixed(numeric_backend_out, 1.0 / 0.0, 2);\n    if (status != -1 || numeric_backend_out[0] != 107 || numeric_backend_out[1] != 101 || numeric_backend_out[2] != 101 || numeric_backend_out[3] != 112 || numeric_backend_out[4] != 0) {{ return {failure}; }}\n"
    ));
    failure += 1;
    source.push_str(&format!(
        "    status = ascii_from_f32_fixed(numeric_backend_out, 0.0 / 0.0, 2);\n    if (status != -1 || numeric_backend_out[0] != 107 || numeric_backend_out[1] != 101 || numeric_backend_out[2] != 101 || numeric_backend_out[3] != 112 || numeric_backend_out[4] != 0) {{ return {failure}; }}\n"
    ));
    source.push_str("    return 0;\n}\n");
    source
}

fn compile_backend_selfcheck() -> (JitProcess, AotProcess, WasmProcess) {
    let roots = vec!["main".to_string()];
    let source = backend_selfcheck_source();
    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&roots);
    jit.upsert_file("memory.stasis", MEMORY);
    jit.upsert_file("stdlib.stasis", STDLIB);
    jit.upsert_file("numeric_backend.stasis", &source);
    jit.compile().expect("compile numeric-text self-check JIT");

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&roots);
    aot.upsert_file("memory.stasis", MEMORY);
    aot.upsert_file("stdlib.stasis", STDLIB);
    aot.upsert_file("numeric_backend.stasis", &source);
    aot.compile().expect("compile numeric-text native AOT");

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&roots);
    wasm.upsert_file("memory.stasis", MEMORY);
    wasm.upsert_file("stdlib.stasis", STDLIB);
    wasm.upsert_file("numeric_backend.stasis", &source);
    wasm.compile().expect("compile numeric-text Wasm");
    (jit, aot, wasm)
}

#[cfg(windows)]
fn linker_path() -> PathBuf {
    cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
        .map(|tool| tool.path().to_path_buf())
        .filter(|path| path.is_file())
        .expect("MSVC link.exe is required for linked numeric-text AOT execution")
}

#[cfg(windows)]
fn run_linked_aot(aot: &AotProcess, expected_exit_code: i32) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!("numeric-text-aot-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&directory).expect("create numeric-text AOT directory");
    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let (import, runtime) = [&deps, deps.parent().expect("Cargo profile directory")]
        .into_iter()
        .find_map(|candidate| {
            let import = candidate.join("stasis_dynload.dll.lib");
            let runtime = candidate.join("stasis_dynload.dll");
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh dynload artifacts");
    fs::copy(runtime, directory.join("stasis_dynload.dll")).expect("copy dynload runtime");
    let executable = directory.join("numeric_text_aot.exe");
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path: Some(linker_path()),
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link numeric-text AOT executable");
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .status()
        .expect("run linked numeric-text AOT executable");
    let _ = fs::remove_dir_all(directory);
    assert_eq!(
        status.code(),
        Some(expected_exit_code),
        "linked AOT self-check"
    );
}

fn run_executable_wasm(wasm: &WasmProcess, expected_result: i32) {
    let path = std::env::temp_dir().join(format!(
        "stasis_numeric_text_{}_{}.wasm",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    fs::write(&path, wasm.module_bytes()).expect("write numeric-text Wasm module");
    let output = Command::new("node")
        .args([
            "-e",
            "const fs=require('node:fs');const m=new WebAssembly.Module(fs.readFileSync(process.argv[1]));const imports=WebAssembly.Module.imports(m);if(imports.length!==0){throw new Error('unexpected imports: '+JSON.stringify(imports))}const i=new WebAssembly.Instance(m,{});process.stdout.write(String(i.exports.main()));",
        ])
        .arg(&path)
        .output()
        .expect("execute numeric-text Wasm module");
    let _ = fs::remove_file(path);
    assert!(
        output.status.success(),
        "Node failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        expected_result.to_string()
    );
}

fn assert_no_numeric_format_or_allocator_aot_imports(aot: &mut AotProcess) {
    let directory = std::env::temp_dir().join(format!(
        "stasis_numeric_text_symbols_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    let objects = aot
        .write_object_files_by_id(&directory)
        .expect("write numeric-text AOT objects for import audit");
    let mut undefined = std::collections::BTreeSet::new();
    for (_, path) in objects.values() {
        let bytes = fs::read(path).expect("read numeric-text AOT object");
        let object = object::File::parse(bytes.as_slice()).expect("parse numeric-text AOT object");
        undefined.extend(
            object
                .symbols()
                .filter(|symbol| symbol.is_undefined())
                .filter_map(|symbol| symbol.name().ok().map(str::to_string)),
        );
    }
    fs::remove_dir_all(&directory).expect("remove numeric-text AOT symbol evidence");
    println!("numeric-text native undefined symbols: {undefined:?}");
    for symbol in undefined {
        let lower = symbol.to_ascii_lowercase();
        for forbidden in [
            "format", "printf", "sprintf", "malloc", "calloc", "realloc", "free",
        ] {
            assert!(
                !lower.contains(forbidden),
                "numeric formatter introduced forbidden native import {symbol}"
            );
        }
    }
}

fn oracle_f32_fixed(value: f32, decimals: u32) -> Option<String> {
    if decimals > 6 || !value.is_finite() {
        return None;
    }
    let bits = value.to_bits();
    let exponent = (bits >> 23) & 0xff;
    let fraction = bits & 0x7f_ffff;
    let (mantissa, exp2) = if exponent == 0 {
        (fraction, -149)
    } else {
        ((1 << 23) + fraction, exponent as i32 - 150)
    };
    let mut magnitude = BigUint::from(mantissa) * BigUint::from(5_u32).pow(decimals);
    let shift = exp2 + decimals as i32;
    if shift >= 0 {
        magnitude <<= shift as usize;
    } else {
        let right = (-shift) as usize;
        if right > 0 {
            let quotient = &magnitude >> right;
            let remainder = &magnitude - (&quotient << right);
            let half = BigUint::from(1_u32) << (right - 1);
            magnitude = quotient + BigUint::from((remainder >= half) as u8);
        }
    }
    let mut digits = magnitude.to_str_radix(10);
    if decimals > 0 {
        let fractional = decimals as usize;
        if digits.len() <= fractional {
            digits = format!("{}{}", "0".repeat(fractional + 1 - digits.len()), digits);
        }
        digits.insert(digits.len() - fractional, '.');
    }
    if bits >> 31 != 0 && magnitude != BigUint::from(0_u8) {
        digits.insert(0, '-');
    }
    Some(digits)
}

fn process() -> JitProcess {
    let roots = [
        "numeric_from_i32".to_string(),
        "numeric_from_f32".to_string(),
        "numeric_utf8_receiver_i32".to_string(),
        "numeric_utf8_function_i32".to_string(),
        "numeric_utf8_exact_i32".to_string(),
        "numeric_utf8_short_i32".to_string(),
        "numeric_utf8_nested_i32".to_string(),
        "numeric_ascii_receiver_i32".to_string(),
        "numeric_utf8_receiver_from_ascii".to_string(),
        "numeric_scalar_receiver_i32".to_string(),
        "numeric_append_i32".to_string(),
        "numeric_pad_left".to_string(),
        "numeric_i32_exact_capacity".to_string(),
        "numeric_i32_short_capacity".to_string(),
        "numeric_f32_exact_capacity".to_string(),
        "numeric_f32_short_capacity".to_string(),
        "numeric_small_append".to_string(),
    ];
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&roots);
    process.upsert_file("memory.stasis", MEMORY);
    process.upsert_file("stdlib.stasis", STDLIB);
    process.upsert_file("numeric_text.stasis", HARNESS);
    process.compile().expect("compile numeric-text JIT harness");
    process
}

fn output(process: &JitProcess) -> Vec<u8> {
    collection_output(process, "numeric_out")
}

fn collection_output(process: &JitProcess, path: &str) -> Vec<u8> {
    let JitScalarValue::I32(length) = process
        .read_global_scalar(&format!("{path}.length"))
        .expect("read numeric output length")
    else {
        panic!("numeric output length must be i32");
    };
    (0..=length)
        .map(|index| {
            let JitScalarValue::U8(value) = process
                .read_global_collection_scalar(path, "", index)
                .expect("read numeric output byte")
            else {
                panic!("numeric output byte must be u8");
            };
            value
        })
        .collect()
}

fn collection_bytes(process: &JitProcess, path: &str, capacity: i32) -> Vec<u8> {
    (0..capacity)
        .map(|index| {
            let JitScalarValue::U8(value) = process
                .read_global_collection_scalar(path, "", index)
                .expect("read numeric collection byte")
            else {
                panic!("numeric collection byte must be u8");
            };
            value
        })
        .collect()
}

fn seed_utf8(process: &JitProcess, path: &str, capacity: i32, text: &[u8], fill: u8) {
    for index in 0..capacity {
        let value = if index as usize == text.len() {
            0
        } else {
            text.get(index as usize).copied().unwrap_or(fill)
        };
        process
            .write_global_collection_scalar(path, "", index, JitScalarValue::U8(value))
            .expect("seed UTF-8 collection byte");
    }
    process
        .write_global_scalar(
            &format!("{path}.length"),
            JitScalarValue::I32(text.len() as i32),
        )
        .expect("seed UTF-8 byte length");
    process
        .write_global_scalar(
            &format!("{path}.char_length"),
            JitScalarValue::I32(text.len() as i32),
        )
        .expect("seed UTF-8 character length");
}

#[test]
fn utf8_integer_receiver_overloads_write_directly_and_preserve_rejections() {
    let process = process();
    for (value, expected) in [
        (i32::MIN, "-2147483648"),
        (i32::MAX, "2147483647"),
        (-1, "-1"),
        (0, "0"),
        (1, "1"),
    ] {
        process
            .write_global_scalar("numeric_i32", JitScalarValue::I32(value))
            .expect("write UTF-8 receiver input");
        assert_eq!(
            process.execute_i32_noarg_by_name("numeric_utf8_receiver_i32"),
            Ok(expected.len() as i32),
            "receiver i32 value {value}"
        );
        let mut expected_bytes = expected.as_bytes().to_vec();
        expected_bytes.push(0);
        assert_eq!(
            collection_output(&process, "numeric_utf8_out"),
            expected_bytes
        );
        assert_eq!(
            process.read_global_scalar("numeric_utf8_out.char_length"),
            Ok(JitScalarValue::I32(expected.len() as i32))
        );
    }

    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(i32::MIN))
        .expect("write exact-capacity UTF-8 input");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_utf8_exact_i32"),
        Ok(11)
    );
    assert_eq!(
        collection_output(&process, "numeric_utf8_exact"),
        b"-2147483648\0"
    );

    seed_utf8(&process, "numeric_utf8_short", 11, b"keep", 0x53);
    process
        .write_global_collection_scalar("numeric_utf8_short", "", 10, JitScalarValue::U8(0x53))
        .expect("write UTF-8 rejection canary");
    let before = collection_bytes(&process, "numeric_utf8_short", 11);
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_utf8_short_i32"),
        Ok(-1)
    );
    assert_eq!(collection_bytes(&process, "numeric_utf8_short", 11), before);
    assert_eq!(
        process.read_global_scalar("numeric_utf8_short.length"),
        Ok(JitScalarValue::I32(4))
    );
    assert_eq!(
        process.read_global_scalar("numeric_utf8_short.char_length"),
        Ok(JitScalarValue::I32(4))
    );

    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(i32::MAX))
        .expect("write nested UTF-8 input");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_utf8_nested_i32"),
        Ok(10)
    );
    assert_eq!(
        collection_output(&process, "numeric_utf8_nested.text"),
        b"2147483647\0"
    );

    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(42))
        .expect("write ASCII overload input");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_ascii_receiver_i32"),
        Ok(2)
    );
    assert_eq!(collection_output(&process, "numeric_ascii_out"), b"42\0");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_utf8_receiver_from_ascii"),
        Ok(2)
    );
    assert_eq!(collection_output(&process, "numeric_utf8_out"), b"42\0");

    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_scalar_receiver_i32"),
        Ok(42),
        "numeric scalar conversion remains available"
    );
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_utf8_function_i32"),
        Ok(2),
        "function-form utf8_from_i32 remains available"
    );
    assert_eq!(collection_output(&process, "numeric_utf8_out"), b"42\0");
}

#[test]
fn conversion_receiver_overload_selects_user_defined_function() {
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(
        "user_overload.stasis",
        concat!(
            "struct Receiver { value: i32; }\n",
            "global receiver: Receiver;\n",
            "function from_i32(self: Receiver, value: i32): i32 { self.value = value; return self.value; }\n",
            "function main(): i32 { return receiver.from_i32(42); }\n",
        ),
    );
    process
        .compile()
        .expect("compile receiver overload through normal typed resolution");
    assert_eq!(
        process.execute_i32_noarg_by_name("main"),
        Ok(42),
        "user-defined from_i32 overload resolves by receiver type"
    );
}

#[test]
fn ascii_utf8_buffer_arguments_require_explicit_conversion_across_backends() {
    let source = concat!(
        "global text: utf8[16];\n",
        "function take_ascii(value: ascii[]): i32 { return value.length; }\n",
        "function main(): i32 { return take_ascii(text); }\n",
        "function tick(): i32 { return 0; }\n",
        "function render(): i32 { return 0; }\n",
    );

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file("strict_text_arguments.stasis", source);
    assert!(
        jit.compile().is_err(),
        "JIT must reject implicit UTF-8 to ASCII buffer passing"
    );

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&["main".to_string()]);
    aot.upsert_file("strict_text_arguments.stasis", source);
    assert!(
        aot.compile().is_err(),
        "native AOT must reject implicit UTF-8 to ASCII buffer passing"
    );

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&["main".to_string(), "tick".to_string(), "render".to_string()]);
    wasm.upsert_file("strict_text_arguments.stasis", source);
    assert!(
        wasm.compile().is_err(),
        "Wasm must reject implicit UTF-8 to ASCII buffer passing"
    );

    for (description, invalid_source) in [
        (
            "typed let",
            "global text: utf8[16]; function main(): i32 { let view: ascii[] = text; return 0; }",
        ),
        (
            "assignment",
            "global ascii_text: ascii[16]; global utf8_text: utf8[16]; function main(): i32 { ascii_text = utf8_text; return 0; }",
        ),
        (
            "return",
            "global text: utf8[16]; function as_ascii(): ascii[] { return text; } function main(): i32 { return 0; }",
        ),
    ] {
        let mut process = JitProcess::new();
        process.set_required_emit_roots(&["main".to_string()]);
        process.upsert_file("strict_text_assignment.stasis", invalid_source);
        assert!(
            process.compile().is_err(),
            "compiler must reject UTF-8 to ASCII {description}"
        );
    }

    let same_family = concat!(
        "global text: ascii[16];\n",
        "function take_ascii(value: ascii[]): i32 { return value.length; }\n",
        "function main(): i32 { return take_ascii(text); }\n",
    );
    let mut process = JitProcess::new();
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file("matching_text_family.stasis", same_family);
    process
        .compile()
        .expect("ASCII fixed storage remains compatible with an ASCII view");
}

#[test]
fn ascii_string_literals_are_contextual_but_non_ascii_literals_are_rejected() {
    let source = concat!(
        "global utf8_anchor: utf8[16];\n",
        "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
        "function take_ascii(value: ascii[]): i32 { return value.length; }\n",
        "function main(): i32 { return take_ascii(\"\"); }\n",
    );

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file("ascii_literal_context.stasis", source);
    jit.compile()
        .expect("ASCII string literal is contextually typed as ascii[]");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(0));

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&["main".to_string()]);
    wasm.upsert_file("ascii_literal_context.stasis", source);
    wasm.compile()
        .expect("Wasm retains contextual ASCII literal typing");
    run_executable_wasm(&wasm, 0);

    let contextual_source = concat!(
        "global utf8_anchor: utf8[16];\n",
        "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
        "function return_ascii_literal(): ascii[] { return \"return\"; }\n",
        "function main(): i32 { let local: ascii[] = \"let\"; let returned: ascii[] = return_ascii_literal(); if (returned.length != 6 || returned[0] != 114) { return -1; } return local.length; }\n",
    );

    let mut jit = JitProcess::new();
    jit.set_required_emit_roots(&["main".to_string()]);
    jit.upsert_file("ascii_typed_literal_context.stasis", contextual_source);
    jit.compile()
        .expect("typed ASCII lets and returns accept ASCII literals");
    assert_eq!(jit.execute_i32_noarg_by_name("main"), Ok(3));

    let mut aot = AotProcess::new();
    aot.set_required_emit_roots(&["main".to_string()]);
    aot.upsert_file("ascii_typed_literal_context.stasis", contextual_source);
    aot.compile()
        .expect("native AOT accepts contextual ASCII literals in typed lets and returns");
    #[cfg(windows)]
    run_linked_aot(&aot, 3);

    let mut wasm = WasmProcess::new();
    wasm.set_required_emit_roots(&["main".to_string()]);
    wasm.upsert_file("ascii_typed_literal_context.stasis", contextual_source);
    wasm.compile()
        .expect("Wasm accepts contextual ASCII literals in typed lets and returns");
    run_executable_wasm(&wasm, 3);

    let assignment_source = concat!(
        "global utf8_anchor: utf8[16];\n",
        "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
        "global ascii_storage: ascii[16];\n",
        "function main(): i32 { ascii_storage = \"assign\"; return 0; }\n",
    );
    let mut compiler = Compiler::new();
    compiler.upsert_file("ascii_typed_literal_assignment.stasis", assignment_source);
    compiler
        .check()
        .expect("semantic analysis accepts an ASCII literal in an ASCII assignment context");

    let non_ascii_sources = [
        (
            "non_ascii_literal_argument.stasis",
            concat!(
                "global utf8_anchor: utf8[16];\n",
                "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
                "function take_ascii(value: ascii[]): i32 { return value.length; }\n",
                "function main(): i32 { return take_ascii(\"é\"); }\n",
            ),
        ),
        (
            "non_ascii_literal_let.stasis",
            concat!(
                "global utf8_anchor: utf8[16];\n",
                "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
                "function main(): i32 { let invalid: ascii[] = \"é\"; return 0; }\n",
            ),
        ),
        (
            "non_ascii_literal_assignment.stasis",
            concat!(
                "global utf8_anchor: utf8[16];\n",
                "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
                "global ascii_storage: ascii[16];\n",
                "function main(): i32 { ascii_storage = \"é\"; return 0; }\n",
            ),
        ),
        (
            "non_ascii_literal_return.stasis",
            concat!(
                "global utf8_anchor: utf8[16];\n",
                "function take_utf8(value: utf8[]): i32 { return value.length; }\n",
                "function invalid_return(): ascii[] { return \"é\"; }\n",
                "function main(): i32 { return 0; }\n",
            ),
        ),
    ];
    for (path, source) in non_ascii_sources {
        let mut jit = JitProcess::new();
        jit.set_required_emit_roots(&["main".to_string()]);
        jit.upsert_file(path, source);
        assert!(jit.compile().is_err(), "JIT must reject non-ASCII {path}");

        let mut aot = AotProcess::new();
        aot.set_required_emit_roots(&["main".to_string()]);
        aot.upsert_file(path, source);
        assert!(
            aot.compile().is_err(),
            "native AOT must reject non-ASCII {path}"
        );

        let mut wasm = WasmProcess::new();
        wasm.set_required_emit_roots(&["main".to_string()]);
        wasm.upsert_file(path, source);
        assert!(wasm.compile().is_err(), "Wasm must reject non-ASCII {path}");
    }
}

fn seed_ascii(process: &JitProcess, path: &str, capacity: i32, text: &[u8], fill: u8) {
    for index in 0..capacity {
        let value = if index as usize == text.len() {
            0
        } else {
            text.get(index as usize).copied().unwrap_or(fill)
        };
        process
            .write_global_collection_scalar(path, "", index, JitScalarValue::U8(value))
            .expect("seed numeric collection byte");
    }
    process
        .write_global_scalar(
            &format!("{path}.length"),
            JitScalarValue::I32(text.len() as i32),
        )
        .expect("seed numeric collection length");
}

#[test]
fn numeric_text_formats_i32_and_classified_f32_values_in_jit() {
    let process = process();
    for (value, expected) in [
        (i32::MIN, "-2147483648"),
        (i32::MAX, "2147483647"),
        (-1, "-1"),
        (0, "0"),
        (1, "1"),
    ] {
        process
            .write_global_scalar("numeric_i32", JitScalarValue::I32(value))
            .expect("write i32 input");
        assert_eq!(
            process.execute_i32_noarg_by_name("numeric_from_i32"),
            Ok(expected.len() as i32)
        );
        let mut expected_bytes = expected.as_bytes().to_vec();
        expected_bytes.push(0);
        assert_eq!(output(&process), expected_bytes, "i32 value {value}");
    }

    let cases = [
        0.0_f32,
        -0.0,
        1.25,
        -1.25,
        f32::from_bits(1),
        -f32::from_bits(0x007f_ffff),
        f32::MIN_POSITIVE,
        f32::MAX,
        -f32::MAX,
    ];
    for value in cases {
        for decimals in 0..=6 {
            let expected = oracle_f32_fixed(value, decimals).expect("finite oracle value");
            process
                .write_global_scalar("numeric_f32", JitScalarValue::F32(value))
                .expect("write f32 input");
            process
                .write_global_scalar("numeric_decimals", JitScalarValue::I32(decimals as i32))
                .expect("write precision");
            assert_eq!(
                process.execute_i32_noarg_by_name("numeric_from_f32"),
                Ok(expected.len() as i32),
                "value={value:?} decimals={decimals}"
            );
            let mut expected_bytes = expected.into_bytes();
            expected_bytes.push(0);
            assert_eq!(
                output(&process),
                expected_bytes,
                "value={value:?} decimals={decimals}"
            );
        }
    }
}

#[test]
fn numeric_text_matches_independent_biguint_corpus_in_jit() {
    let process = process();
    let mut bits = std::collections::BTreeSet::new();
    for value in [
        0.0_f32,
        -0.0,
        f32::from_bits(1),
        f32::from_bits(0x007f_ffff),
        f32::MIN_POSITIVE,
        f32::from_bits(0x0080_0001),
        f32::from_bits(0x7f7f_fffe),
        f32::MAX,
    ] {
        bits.insert(value.to_bits());
        bits.insert((-value).to_bits());
    }
    for exponent in 1_u32..=254 {
        for fraction in [0_u32, 1 << 22, 0x7f_ffff] {
            let positive = (exponent << 23) | fraction;
            bits.insert(positive);
            bits.insert(positive | 0x8000_0000);
        }
    }
    for decimals in 0_u32..=6 {
        let tie = 2.0_f32.powi(-(decimals as i32 + 1));
        for candidate in [tie.to_bits() - 1, tie.to_bits(), tie.to_bits() + 1] {
            bits.insert(candidate);
            bits.insert(candidate | 0x8000_0000);
        }
    }
    for power in -45..=38 {
        let value = 10.0_f32.powi(power);
        if value.is_finite() && value > 0.0 {
            for candidate in [value.to_bits() - 1, value.to_bits(), value.to_bits() + 1] {
                let candidate_value = f32::from_bits(candidate);
                if candidate_value.is_finite() {
                    bits.insert(candidate);
                    bits.insert(candidate | 0x8000_0000);
                }
            }
        }
    }
    let mut state = 0x6d2b_79f5_u32;
    for _ in 0..1024 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        if (state >> 23) & 0xff != 0xff {
            bits.insert(state);
        }
    }

    for raw in bits {
        let value = f32::from_bits(raw);
        for decimals in 0_u32..=6 {
            let expected = oracle_f32_fixed(value, decimals).expect("finite corpus value");
            process
                .write_global_scalar("numeric_f32", JitScalarValue::F32(value))
                .expect("write corpus f32 input");
            process
                .write_global_scalar("numeric_decimals", JitScalarValue::I32(decimals as i32))
                .expect("write corpus precision");
            assert_eq!(
                process.execute_i32_noarg_by_name("numeric_from_f32"),
                Ok(expected.len() as i32),
                "raw=0x{raw:08x} decimals={decimals} expected={expected}"
            );
            let mut expected_bytes = expected.into_bytes();
            expected_bytes.push(0);
            assert_eq!(
                output(&process),
                expected_bytes,
                "raw=0x{raw:08x} decimals={decimals}"
            );
        }
    }
}

#[test]
fn numeric_text_capacity_append_padding_and_rejections_are_transactional() {
    let process = process();
    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(i32::MIN))
        .expect("write exact-capacity i32");
    process
        .write_global_scalar("numeric_f32", JitScalarValue::F32(-f32::MAX))
        .expect("write exact-capacity f32");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_i32_exact_capacity"),
        Ok(11)
    );
    assert_eq!(
        collection_output(&process, "numeric_i32_exact"),
        b"-2147483648\0"
    );
    seed_ascii(&process, "numeric_i32_short", 11, b"keep", 0x53);
    let before = collection_bytes(&process, "numeric_i32_short", 11);
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_i32_short_capacity"),
        Ok(-1)
    );
    assert_eq!(collection_bytes(&process, "numeric_i32_short", 11), before);
    assert_eq!(
        process.read_global_scalar("numeric_i32_short.length"),
        Ok(JitScalarValue::I32(4))
    );

    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_f32_exact_capacity"),
        Ok(47)
    );
    assert_eq!(
        collection_output(&process, "numeric_f32_exact"),
        b"-340282346638528859811704183484516925440.000000\0"
    );
    seed_ascii(&process, "numeric_f32_short", 47, b"keep", 0x54);
    let before = collection_bytes(&process, "numeric_f32_short", 47);
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_f32_short_capacity"),
        Ok(-1)
    );
    assert_eq!(collection_bytes(&process, "numeric_f32_short", 47), before);

    seed_ascii(&process, "numeric_out", 64, b"score:", 0x55);
    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(i32::MIN))
        .expect("write append value");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_append_i32"),
        Ok(17)
    );
    assert_eq!(output(&process), b"score:-2147483648\0");

    seed_ascii(&process, "numeric_out", 64, b"42", 0x56);
    process
        .write_global_collection_scalar("numeric_out", "", 2, JitScalarValue::U8(0x56))
        .expect("replace prior terminator with sentinel");
    process
        .write_global_scalar("numeric_width", JitScalarValue::I32(8))
        .expect("write padding width");
    process
        .write_global_scalar("numeric_fill", JitScalarValue::U8(b'0'))
        .expect("write padding fill");
    assert_eq!(process.execute_i32_noarg_by_name("numeric_pad_left"), Ok(8));
    assert_eq!(output(&process), b"00000042\0");

    seed_ascii(&process, "numeric_out", 64, b"keep", 0x57);
    let before = collection_bytes(&process, "numeric_out", 64);
    for (value, decimals) in [
        (f32::NAN, 2),
        (f32::INFINITY, 2),
        (f32::NEG_INFINITY, 2),
        (1.0, -1),
        (1.0, 7),
    ] {
        process
            .write_global_scalar("numeric_f32", JitScalarValue::F32(value))
            .expect("write rejected f32");
        process
            .write_global_scalar("numeric_decimals", JitScalarValue::I32(decimals))
            .expect("write rejected precision");
        assert_eq!(
            process.execute_i32_noarg_by_name("numeric_from_f32"),
            Ok(-1)
        );
        assert_eq!(collection_bytes(&process, "numeric_out", 64), before);
        assert_eq!(
            process.read_global_scalar("numeric_out.length"),
            Ok(JitScalarValue::I32(4))
        );
    }

    seed_ascii(&process, "numeric_small", 8, b"abc", 0x58);
    let before = collection_bytes(&process, "numeric_small", 8);
    process
        .write_global_scalar("numeric_i32", JitScalarValue::I32(12345))
        .expect("write rejected append value");
    assert_eq!(
        process.execute_i32_noarg_by_name("numeric_small_append"),
        Ok(-1)
    );
    assert_eq!(collection_bytes(&process, "numeric_small", 8), before);
}

#[test]
fn numeric_text_biguint_oracle_covers_rounding_and_full_finite_boundaries() {
    assert_eq!(oracle_f32_fixed(1.25, 1).as_deref(), Some("1.3"));
    assert_eq!(oracle_f32_fixed(-1.25, 1).as_deref(), Some("-1.3"));
    assert_eq!(oracle_f32_fixed(-0.004, 2).as_deref(), Some("0.00"));
    assert_eq!(
        oracle_f32_fixed(f32::MAX, 6).as_deref(),
        Some("340282346638528859811704183484516925440.000000")
    );
    assert_eq!(oracle_f32_fixed(f32::INFINITY, 0), None);
    assert_eq!(oracle_f32_fixed(f32::NAN, 0), None);
}

#[test]
fn numeric_text_matches_linked_native_aot_and_executable_wasm() {
    let (jit, mut aot, wasm) = compile_backend_selfcheck();
    assert_eq!(
        jit.execute_i32_noarg_by_name("main"),
        Ok(0),
        "generated exact-byte JIT self-check"
    );
    assert_no_numeric_format_or_allocator_aot_imports(&mut aot);
    #[cfg(windows)]
    run_linked_aot(&aot, 0);
    #[cfg(not(windows))]
    let _ = aot;
    run_executable_wasm(&wasm, 0);
}
