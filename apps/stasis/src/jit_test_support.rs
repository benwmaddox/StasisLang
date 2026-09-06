use std::sync::{Mutex, MutexGuard};

static GLOBAL_TABLE_LOCK: Mutex<()> = Mutex::new(());

pub(crate) struct JitTestGuard {
    _lock: MutexGuard<'static, ()>,
}

// Declare this before fixture locals and retain it until all JITs and workers are dropped.
// Dependency crates do not enable their own cfg(test) JIT isolation in this test binary.
pub(crate) fn lock() -> JitTestGuard {
    let guard = JitTestGuard {
        // Cleanup also runs on unwind, so a failed assertion need not poison later fixtures.
        _lock: GLOBAL_TABLE_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()),
    };
    clear_runtime();
    guard
}

impl Drop for JitTestGuard {
    fn drop(&mut self) {
        clear_runtime();
    }
}

fn clear_runtime() {
    stasis_dynload::clear_registered_global_memory();
    stasis_dynload::clear_jit_i32_global_table();
    stasis_dynload::clear_jit_f32_global_table();
    stasis_dynload::clear_jit_f64_global_table();
    stasis_dynload::clear_jit_i32_array_global_table();
    stasis_dynload::clear_jit_f32_array_global_table();
    stasis_dynload::clear_jit_f64_array_global_table();
    stasis_dynload::clear_jit_string_literal_table();
}
