use std::cell::RefCell;

use stasis_compiler::backend::{wasm::WasmProcess, ReachabilityPolicy};

const ABI_VERSION: u32 = 1;
const FIXED_SAMPLE_PATH: &str = "memory/main.stasis";
const FIXED_SAMPLE_SOURCE: &str = "function main(): i32 { return 720; }";

#[derive(Default)]
struct CompileState {
    output: Vec<u8>,
    error: Vec<u8>,
}

thread_local! {
    static COMPILE_STATE: RefCell<CompileState> = RefCell::new(CompileState::default());
}

fn compile_fixed_sample_bytes() -> Result<Vec<u8>, String> {
    let mut process = WasmProcess::new();
    process.set_reachability_policy(ReachabilityPolicy::Release);
    process.upsert_file(FIXED_SAMPLE_PATH, FIXED_SAMPLE_SOURCE);
    process.compile().map_err(|error| format!("{error:?}"))?;
    Ok(process.module_bytes().to_vec())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_browser_compiler_abi_version() -> u32 {
    ABI_VERSION
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_fixed_sample() -> i32 {
    let result = compile_fixed_sample_bytes();
    COMPILE_STATE.with(|state| {
        let mut state = state.borrow_mut();
        state.output.clear();
        state.error.clear();
        match result {
            Ok(output) => {
                state.output = output;
                0
            }
            Err(error) => {
                state.error = error.into_bytes();
                1
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_output_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().output.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_output_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().output.len())
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_error_ptr() -> usize {
    COMPILE_STATE.with(|state| state.borrow().error.as_ptr() as usize)
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_compile_error_len() -> usize {
    COMPILE_STATE.with(|state| state.borrow().error.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_sample_compilation_is_deterministic_webassembly() {
        let first = compile_fixed_sample_bytes().expect("compile fixed browser sample");
        let second = compile_fixed_sample_bytes().expect("repeat fixed browser sample");
        assert_eq!(&first[..8], b"\0asm\x01\0\0\0");
        assert_eq!(first, second);
    }
}
