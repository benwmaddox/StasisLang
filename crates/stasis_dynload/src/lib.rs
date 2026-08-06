#![cfg_attr(not(debug_assertions), deny(warnings))]

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_void, CString};
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::ffi::CStr;
#[cfg(windows)]
use std::ffi::OsStr;
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

pub struct Library {
    #[cfg(windows)]
    handle: *mut c_void,
    #[cfg(unix)]
    handle: *mut c_void,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JitHostEntryTargets {
    pub revision: u64,
    pub main: usize,
    pub tick: usize,
    pub render: usize,
    pub on_code_swap: Option<usize>,
}

static JIT_HOST_ENTRY_TARGETS: AtomicUsize = AtomicUsize::new(0);

static JIT_DEBUG_ENABLED: AtomicBool = AtomicBool::new(false);

fn jit_output_capture() -> &'static Mutex<Option<String>> {
    static CAPTURE: OnceLock<Mutex<Option<String>>> = OnceLock::new();
    CAPTURE.get_or_init(|| Mutex::new(None))
}

pub fn set_jit_output_capture(enabled: bool) {
    *lock_unpoisoned(jit_output_capture()) = enabled.then(String::new);
}

pub fn drain_jit_output() -> String {
    let mut capture = lock_unpoisoned(jit_output_capture());
    capture.as_mut().map(std::mem::take).unwrap_or_default()
}

fn write_jit_output(text: &str) {
    let mut capture = lock_unpoisoned(jit_output_capture());
    if let Some(capture) = capture.as_mut() {
        capture.push_str(text);
    } else {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum JitDebugValue {
    I64 { type_tag: i32, value: i64 },
    F64 { type_tag: i32, value: f64 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct JitDebugFrame {
    pub function_id: u32,
    pub site_id: u32,
    pub values: HashMap<u32, JitDebugValue>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct JitDebugStop {
    pub sequence: u64,
    pub function_id: u32,
    pub site_id: u32,
    pub frames: Vec<JitDebugFrame>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JitDebugResume {
    Continue,
    StepIn,
    StepOver,
    StepOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JitDebugRunMode {
    Continue,
    StepIn,
    StepOver { depth: usize },
    StepOut { depth: usize },
}

struct JitDebugState {
    enabled: bool,
    breakpoints: HashSet<(u32, u32)>,
    frames: Vec<JitDebugFrame>,
    mode: JitDebugRunMode,
    stopped: Option<JitDebugStop>,
    sequence: u64,
}

impl Default for JitDebugState {
    fn default() -> Self {
        Self {
            enabled: false,
            breakpoints: HashSet::new(),
            frames: Vec::new(),
            mode: JitDebugRunMode::Continue,
            stopped: None,
            sequence: 0,
        }
    }
}

fn jit_debug_controller() -> &'static (Mutex<JitDebugState>, Condvar) {
    static CONTROLLER: OnceLock<(Mutex<JitDebugState>, Condvar)> = OnceLock::new();
    CONTROLLER.get_or_init(|| (Mutex::new(JitDebugState::default()), Condvar::new()))
}

fn lock_unpoisoned<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn enable_jit_debugger(breakpoints: impl IntoIterator<Item = (u32, u32)>) {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    state.enabled = true;
    state.breakpoints = breakpoints.into_iter().collect();
    state.frames.clear();
    state.mode = JitDebugRunMode::Continue;
    state.stopped = None;
    JIT_DEBUG_ENABLED.store(true, Ordering::Release);
    wake.notify_all();
}

pub fn set_jit_debug_breakpoints(breakpoints: impl IntoIterator<Item = (u32, u32)>) {
    let (state, _) = jit_debug_controller();
    lock_unpoisoned(state).breakpoints = breakpoints.into_iter().collect();
}

pub fn disable_jit_debugger() {
    JIT_DEBUG_ENABLED.store(false, Ordering::Release);
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    state.enabled = false;
    state.frames.clear();
    state.stopped = None;
    state.mode = JitDebugRunMode::Continue;
    wake.notify_all();
}

pub fn jit_debug_stop() -> Option<JitDebugStop> {
    let (state, _) = jit_debug_controller();
    lock_unpoisoned(state).stopped.clone()
}

pub fn wait_for_jit_debug_stop(after_sequence: u64, timeout: Duration) -> Option<JitDebugStop> {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(stop) = state
            .stopped
            .as_ref()
            .filter(|stop| stop.sequence > after_sequence)
        {
            return Some(stop.clone());
        }
        if !state.enabled {
            return None;
        }
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return None;
        }
        let (next, result) = wake
            .wait_timeout(state, remaining)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        state = next;
        if result.timed_out() {
            return state
                .stopped
                .as_ref()
                .filter(|stop| stop.sequence > after_sequence)
                .cloned();
        }
    }
}

pub fn resume_jit_debugger(resume: JitDebugResume) -> Result<(), String> {
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return Err("JIT debugger is not enabled".to_string());
    }
    let depth = state.frames.len();
    state.mode = match resume {
        JitDebugResume::Continue => JitDebugRunMode::Continue,
        JitDebugResume::StepIn => JitDebugRunMode::StepIn,
        JitDebugResume::StepOver => JitDebugRunMode::StepOver { depth },
        JitDebugResume::StepOut => JitDebugRunMode::StepOut { depth },
    };
    state.stopped = None;
    wake.notify_all();
    Ok(())
}

pub fn pause_jit_debugger() -> Result<(), String> {
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return Err("JIT debugger is not enabled".to_string());
    }
    state.mode = JitDebugRunMode::StepIn;
    Ok(())
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_frame_enter(function_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if state.enabled {
        state.frames.push(JitDebugFrame {
            function_id: function_id as u32,
            site_id: 0,
            values: HashMap::new(),
        });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_frame_leave(function_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return;
    }
    if state
        .frames
        .last()
        .is_some_and(|frame| frame.function_id == function_id as u32)
    {
        state.frames.pop();
    } else {
        state.frames.clear();
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_values_begin() {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame.values.clear();
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_value_i64(slot: i32, type_tag: i32, value: i64) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame
            .values
            .insert(slot as u32, JitDebugValue::I64 { type_tag, value });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_value_f64(slot: i32, type_tag: i32, value: f64) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, _) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if let Some(frame) = state.frames.last_mut() {
        frame
            .values
            .insert(slot as u32, JitDebugValue::F64 { type_tag, value });
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_debug_statement(function_id: i32, site_id: i32) {
    if !JIT_DEBUG_ENABLED.load(Ordering::Acquire) {
        return;
    }
    let (state, wake) = jit_debug_controller();
    let mut state = lock_unpoisoned(state);
    if !state.enabled {
        return;
    }
    let function_id = function_id as u32;
    let site_id = site_id as u32;
    let depth = state.frames.len();
    if let Some(frame) = state.frames.last_mut() {
        frame.site_id = site_id;
    }
    let should_stop = state.breakpoints.contains(&(function_id, site_id))
        || match state.mode {
            JitDebugRunMode::Continue => false,
            JitDebugRunMode::StepIn => true,
            JitDebugRunMode::StepOver { depth: target } => depth <= target,
            JitDebugRunMode::StepOut { depth: target } => depth < target,
        };
    if !should_stop {
        return;
    }
    state.sequence = state.sequence.saturating_add(1);
    state.mode = JitDebugRunMode::Continue;
    state.stopped = Some(JitDebugStop {
        sequence: state.sequence,
        function_id,
        site_id,
        frames: state.frames.clone(),
    });
    wake.notify_all();
    while state.enabled && state.stopped.is_some() {
        state = wake
            .wait(state)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }
}

pub fn publish_jit_host_entry_targets(targets: JitHostEntryTargets) -> Result<(), String> {
    validate_jit_host_entry_targets(&targets)?;
    if let Some(active) = jit_host_entry_targets() {
        if targets.revision <= active.revision {
            return Err(format!(
                "stale JIT host-entry revision {} cannot replace active revision {}",
                targets.revision, active.revision
            ));
        }
    }
    install_jit_host_entry_targets(targets);
    Ok(())
}

pub fn begin_jit_host_entry_session(targets: JitHostEntryTargets) -> Result<(), String> {
    validate_jit_host_entry_targets(&targets)?;
    install_jit_host_entry_targets(targets);
    Ok(())
}

fn install_jit_host_entry_targets(targets: JitHostEntryTargets) {
    let published = Box::into_raw(Box::new(targets)) as usize;
    JIT_HOST_ENTRY_TARGETS.store(published, Ordering::Release);
}

pub fn validate_jit_host_entry_targets(targets: &JitHostEntryTargets) -> Result<(), String> {
    if targets.main == 0 || targets.tick == 0 || targets.render == 0 {
        return Err(
            "JIT host-entry publication requires non-zero main, tick, and render targets"
                .to_string(),
        );
    }
    Ok(())
}

pub fn jit_host_entry_targets() -> Option<JitHostEntryTargets> {
    let published = JIT_HOST_ENTRY_TARGETS.load(Ordering::Acquire);
    if published == 0 {
        return None;
    }
    Some(unsafe { *(published as *const JitHostEntryTargets) })
}

pub fn jit_host_main_trampoline_ptr() -> usize {
    stasis_jit_host_main_trampoline as *const () as usize
}

pub fn jit_host_tick_trampoline_ptr() -> usize {
    stasis_jit_host_tick_trampoline as *const () as usize
}

pub fn jit_host_render_trampoline_ptr() -> usize {
    stasis_jit_host_render_trampoline as *const () as usize
}

pub fn jit_host_on_code_swap_trampoline_ptr() -> usize {
    stasis_jit_host_on_code_swap_trampoline as *const () as usize
}

fn active_jit_host_target(select: impl FnOnce(JitHostEntryTargets) -> Option<usize>) -> usize {
    jit_host_entry_targets().and_then(select).unwrap_or(0)
}

extern "C" fn stasis_jit_host_main_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.main)))
}

extern "C" fn stasis_jit_host_tick_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.tick)))
}

extern "C" fn stasis_jit_host_render_trampoline() -> i32 {
    call_jit_host_i32_target(active_jit_host_target(|targets| Some(targets.render)))
}

extern "C" fn stasis_jit_host_on_code_swap_trampoline() {
    if let Some(target) = jit_host_entry_targets().and_then(|targets| targets.on_code_swap) {
        call_jit_host_void_target(target);
    }
}

fn call_jit_host_i32_target(address: usize) -> i32 {
    if address == 0 {
        return -1;
    }
    #[cfg(windows)]
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
    callback()
}

fn call_jit_host_void_target(address: usize) {
    if address == 0 {
        return;
    }
    #[cfg(windows)]
    let callback: extern "system" fn() = unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn() = unsafe { std::mem::transmute(address) };
    callback();
}

// Library handles are process-wide OS resources and can be moved between threads.
unsafe impl Send for Library {}
// Loading a module and calling exports is thread-safe on supported desktop platforms; the handle
// is immutable after load.
unsafe impl Sync for Library {}

impl Library {
    pub fn load(path: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let mut wide: Vec<u16> = os_str_to_wide(path.as_os_str());
            wide.push(0);
            let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
            if handle.is_null() {
                return Err(format!(
                    "failed to load dynamic library {}: {}",
                    path.display(),
                    std::io::Error::last_os_error()
                ));
            }
            return Ok(Self { handle });
        }

        #[cfg(unix)]
        {
            let path = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
                format!(
                    "dynamic library path contains interior NUL byte: {}",
                    path.display()
                )
            })?;
            clear_dynamic_loading_error();
            let handle = unsafe { dlopen(path.as_ptr(), RTLD_NOW | RTLD_LOCAL) };
            if handle.is_null() {
                return Err(format!(
                    "failed to load dynamic library {}: {}",
                    path.to_string_lossy(),
                    dynamic_loading_error()
                ));
            }
            Ok(Self { handle })
        }

        #[cfg(not(any(windows, unix)))]
        {
            let _ = path;
            Err("dynamic loading is unsupported on this platform in stasis_dynload".to_string())
        }
    }

    pub fn symbol_address(&self, symbol: &str) -> Result<usize, String> {
        #[cfg(windows)]
        {
            let name = CString::new(symbol)
                .map_err(|_| format!("symbol name contains interior NUL byte: {symbol}"))?;
            let address = unsafe { GetProcAddress(self.handle, name.as_ptr()) };
            if address.is_null() {
                return Err(format!("failed to resolve symbol {symbol}"));
            }
            return Ok(address as usize);
        }

        #[cfg(unix)]
        {
            let name = CString::new(symbol)
                .map_err(|_| format!("symbol name contains interior NUL byte: {symbol}"))?;
            clear_dynamic_loading_error();
            let address = unsafe { dlsym(self.handle, name.as_ptr()) };
            let error = dynamic_loading_error_if_present();
            if let Some(error) = error {
                return Err(format!("failed to resolve symbol {symbol}: {error}"));
            }
            if address.is_null() {
                return Err(format!("failed to resolve symbol {symbol}: null address"));
            }
            Ok(address as usize)
        }

        #[cfg(not(any(windows, unix)))]
        {
            let _ = symbol;
            Err(
                "dynamic symbol resolution is unsupported on this platform in stasis_dynload"
                    .to_string(),
            )
        }
    }
}

impl Drop for Library {
    fn drop(&mut self) {
        #[cfg(windows)]
        {
            if !self.handle.is_null() {
                let _ = unsafe { FreeLibrary(self.handle) };
            }
        }
        #[cfg(unix)]
        {
            if !self.handle.is_null() {
                let _ = unsafe { dlclose(self.handle) };
            }
        }
    }
}

pub fn invoke_noarg_u64(address: usize) -> Result<u64, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> u64 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() -> u64 = unsafe { std::mem::transmute(address) };
        Ok(callback())
    }
}

pub fn invoke_noarg_i32(address: usize) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback())
    }
}

pub fn invoke_noarg_void(address: usize) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn() = unsafe { std::mem::transmute(address) };
        callback();
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn() = unsafe { std::mem::transmute(address) };
        callback();
        Ok(())
    }
}

thread_local! {
    static CODE_SWAP_REJECTION: RefCell<Option<String>> = const { RefCell::new(None) };
}

pub fn invoke_code_swap_hook(address: usize) -> Result<(), String> {
    let _ = CODE_SWAP_REJECTION.with(|rejection| rejection.borrow_mut().take());
    invoke_noarg_void(address)?;
    CODE_SWAP_REJECTION.with(|rejection| rejection.borrow_mut().take().map_or(Ok(()), Err))
}

#[no_mangle]
pub extern "C" fn stasis_jit_reject_code_swap() {
    CODE_SWAP_REJECTION.with(|rejection| {
        *rejection.borrow_mut() = Some("hook requested rejection".to_string());
    });
}

pub fn invoke_i32_to_void(address: usize, arg0: i32) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32) = unsafe { std::mem::transmute(address) };
        callback(arg0);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(address) };
        callback(arg0);
        Ok(())
    }
}

pub fn invoke_i32_i32_to_i32(address: usize, left: i32, right: i32) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(left, right));
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback(left, right))
    }
}

pub fn invoke_i32_i32_i32_to_i32(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2));
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        Ok(callback(arg0, arg1, arg2))
    }
}

pub fn invoke_i32_i32_i32_i32_to_void(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32) =
            unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32, i32) = unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        Ok(())
    }
}

pub fn invoke_i32_i32_i32_f32_to_void(
    address: usize,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: f32,
) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let _execution = JitExecutionGuard::enter();
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, f32) =
            unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        return Ok(());
    }
    #[cfg(not(windows))]
    {
        let callback: extern "C" fn(i32, i32, i32, f32) = unsafe { std::mem::transmute(address) };
        callback(arg0, arg1, arg2, arg3);
        Ok(())
    }
}

// ============================================================
// stasis_graphics host API (dev in-process runner)
// ============================================================

const STASIS_GRAPHICS_RUNTIME_ABI_VERSION: i32 = 1;

fn verify_graphics_runtime_abi(lib: &Library, path: &Path) -> Result<(), String> {
    let address = lib
        .symbol_address("stasis_graphics_runtime_abi_version")
        .map_err(|_| {
            format!(
                "incompatible stasis graphics runtime {}: missing ABI version",
                path.display()
            )
        })?;
    #[cfg(windows)]
    let actual = {
        let version: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
        version()
    };
    #[cfg(not(windows))]
    let actual = {
        let version: extern "C" fn() -> i32 = unsafe { std::mem::transmute(address) };
        version()
    };
    if actual != STASIS_GRAPHICS_RUNTIME_ABI_VERSION {
        return Err(format!(
            "incompatible stasis graphics runtime {}: expected ABI {}, found {}",
            path.display(),
            STASIS_GRAPHICS_RUNTIME_ABI_VERSION,
            actual
        ));
    }
    Ok(())
}

pub fn graphics_runtime_release_id(path: &Path) -> Result<String, String> {
    let lib = Library::load(path)?;
    verify_graphics_runtime_abi(&lib, path)?;
    let address = lib
        .symbol_address("stasis_graphics_release_id")
        .map_err(|_| {
            format!(
                "incompatible stasis graphics runtime {}: missing release identity",
                path.display()
            )
        })?;
    #[cfg(windows)]
    let value = {
        let identity: extern "system" fn() -> *const c_char =
            unsafe { std::mem::transmute(address) };
        identity()
    };
    #[cfg(not(windows))]
    let value = {
        let identity: extern "C" fn() -> *const c_char = unsafe { std::mem::transmute(address) };
        identity()
    };
    if value.is_null() {
        return Err(format!(
            "incompatible stasis graphics runtime {}: empty release identity",
            path.display()
        ));
    }
    let value = unsafe { std::ffi::CStr::from_ptr(value) }
        .to_str()
        .map_err(|_| {
            format!(
                "incompatible stasis graphics runtime {}: release identity is not UTF-8",
                path.display()
            )
        })?;
    Ok(value.to_string())
}

pub struct StasisGraphicsApi {
    _lib: Library,
    stasis_init_window: usize,
    stasis_host_get_frame: usize,
    stasis_host_bulk_init: usize,
    stasis_host_bulk_apply_requests: usize,
    stasis_host_set_performance_metrics: usize,
    stasis_gfx_submit_u8: usize,
    stasis_sleep_ms: usize,
}

impl StasisGraphicsApi {
    pub fn load_default() -> Result<Self, String> {
        let mut last_error = None;
        for candidate in runtime_library_candidate_paths() {
            if !candidate.exists() {
                continue;
            }
            match Self::load(&candidate) {
                Ok(api) => return Ok(api),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "failed to load stasis_graphics runtime library (set STASIS_RUNTIME_LIBRARY_PATH or build runtime)"
                .to_string()
        }))
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::load(path)?;
        verify_graphics_runtime_abi(&lib, path)?;
        let stasis_init_window = lib.symbol_address("stasis_init_window")?;
        let stasis_host_get_frame = lib.symbol_address("stasis_host_get_frame")?;
        let stasis_host_bulk_init = lib.symbol_address("stasis_host_bulk_init")?;
        let stasis_host_bulk_apply_requests =
            lib.symbol_address("stasis_host_bulk_apply_requests")?;
        let stasis_host_set_performance_metrics =
            lib.symbol_address("stasis_host_set_performance_metrics")?;
        let stasis_gfx_submit_u8 = lib.symbol_address("stasis_gfx_submit_u8")?;
        let stasis_sleep_ms = lib.symbol_address("stasis_sleep_ms")?;
        Ok(Self {
            _lib: lib,
            stasis_init_window,
            stasis_host_get_frame,
            stasis_host_bulk_init,
            stasis_host_bulk_apply_requests,
            stasis_host_set_performance_metrics,
            stasis_gfx_submit_u8,
            stasis_sleep_ms,
        })
    }

    pub fn init_window(&self, width: i32, height: i32, title: &str) -> Result<bool, String> {
        #[cfg(windows)]
        {
            let title = CString::new(title)
                .map_err(|_| "window title contains interior NUL byte".to_string())?;
            let callback: extern "system" fn(i32, i32, *const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_init_window) };
            let rc = callback(width, height, title.as_ptr());
            return Ok(rc != 0);
        }
        #[cfg(not(windows))]
        {
            let title = CString::new(title)
                .map_err(|_| "window title contains interior NUL byte".to_string())?;
            let callback: extern "C" fn(i32, i32, *const c_char) -> i32 =
                unsafe { std::mem::transmute(self.stasis_init_window) };
            Ok(callback(width, height, title.as_ptr()) != 0)
        }
    }

    pub fn host_get_frame(&self, out_i32: &mut [i32], out_f32: &mut [f32]) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*mut i32, *mut f32) =
                unsafe { std::mem::transmute(self.stasis_host_get_frame) };
            callback(out_i32.as_mut_ptr(), out_f32.as_mut_ptr());
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*mut i32, *mut f32) =
                unsafe { std::mem::transmute(self.stasis_host_get_frame) };
            callback(out_i32.as_mut_ptr(), out_f32.as_mut_ptr());
            Ok(())
        }
    }

    pub fn host_bulk_init(&self, host_req_seq: &i32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_init) };
            callback(host_req_seq as *const i32);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_init) };
            callback(host_req_seq as *const i32);
            Ok(())
        }
    }

    pub fn host_bulk_apply_requests(
        &self,
        host_req_seq: &i32,
        host_req_flags: &i32,
        host_req_window_w_px: &i32,
        host_req_window_h_px: &i32,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const i32, *const i32, *const i32, *const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_apply_requests) };
            callback(
                host_req_seq as *const i32,
                host_req_flags as *const i32,
                host_req_window_w_px as *const i32,
                host_req_window_h_px as *const i32,
            );
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const i32, *const i32, *const i32, *const i32) =
                unsafe { std::mem::transmute(self.stasis_host_bulk_apply_requests) };
            callback(
                host_req_seq as *const i32,
                host_req_flags as *const i32,
                host_req_window_w_px as *const i32,
                host_req_window_h_px as *const i32,
            );
            Ok(())
        }
    }

    pub fn host_set_performance_metrics(
        &self,
        tick_micros: u64,
        render_micros: u64,
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(u64, u64) =
                unsafe { std::mem::transmute(self.stasis_host_set_performance_metrics) };
            callback(tick_micros, render_micros);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(u64, u64) =
                unsafe { std::mem::transmute(self.stasis_host_set_performance_metrics) };
            callback(tick_micros, render_micros);
            Ok(())
        }
    }

    pub fn gfx_submit_u8(
        &self,
        cmd_i32: &[i32],
        cmd_f32: &[f32],
        cmd_u8: &[u8],
    ) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(*const i32, *const f32, *const u8) =
                unsafe { std::mem::transmute(self.stasis_gfx_submit_u8) };
            callback(cmd_i32.as_ptr(), cmd_f32.as_ptr(), cmd_u8.as_ptr());
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(*const i32, *const f32, *const u8) =
                unsafe { std::mem::transmute(self.stasis_gfx_submit_u8) };
            callback(cmd_i32.as_ptr(), cmd_f32.as_ptr(), cmd_u8.as_ptr());
            Ok(())
        }
    }

    pub fn sleep_ms(&self, ms: i32) -> Result<(), String> {
        #[cfg(windows)]
        {
            let callback: extern "system" fn(i32) =
                unsafe { std::mem::transmute(self.stasis_sleep_ms) };
            callback(ms);
            return Ok(());
        }
        #[cfg(not(windows))]
        {
            let callback: extern "C" fn(i32) = unsafe { std::mem::transmute(self.stasis_sleep_ms) };
            callback(ms);
            Ok(())
        }
    }
}

// ============================================================
// stasis_graphics asset API (JIT extern call bridge)
// ============================================================

struct StasisGraphicsAssetsApi {
    _lib: Library,
    stasis_gfx_load_sprite: usize,
    stasis_gfx_release_sprite: usize,
    stasis_gfx_dump_bmp: usize,
    stasis_gfx_dump_png: Option<usize>,
    stasis_host_schedule_screenshot: Option<usize>,
    stasis_gfx_poll_reload: usize,
    stasis_load_font: usize,
    stasis_measure_text: usize,
    stasis_gfx_cache_text: usize,
    stasis_gfx_measure_text_cached: usize,
    stasis_gfx_measure_text_cached_height: usize,
    #[cfg(windows)]
    stasis_storage_load_i32: Option<usize>,
    #[cfg(windows)]
    stasis_storage_save_i32: Option<usize>,
}

impl StasisGraphicsAssetsApi {
    fn load_default() -> Result<Self, String> {
        let mut last_error = None;
        for candidate in runtime_library_candidate_paths() {
            if !candidate.exists() {
                continue;
            }
            match Self::load(&candidate) {
                Ok(api) => return Ok(api),
                Err(error) => last_error = Some(error),
            }
        }
        Err(last_error.unwrap_or_else(|| {
            "failed to load stasis_graphics runtime library for asset calls".to_string()
        }))
    }

    fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::load(path)?;
        verify_graphics_runtime_abi(&lib, path)?;
        Ok(Self {
            stasis_gfx_load_sprite: lib.symbol_address("stasis_gfx_load_sprite")?,
            stasis_gfx_release_sprite: lib.symbol_address("stasis_gfx_release_sprite")?,
            stasis_gfx_dump_bmp: lib.symbol_address("stasis_gfx_dump_bmp")?,
            // PNG capture was added after the original asset ABI. Keep older runtimes usable for
            // all pre-existing calls and report PNG as unsupported.
            stasis_gfx_dump_png: lib.symbol_address("stasis_gfx_dump_png").ok(),
            stasis_host_schedule_screenshot: lib
                .symbol_address("stasis_host_schedule_screenshot")
                .ok(),
            stasis_gfx_poll_reload: lib.symbol_address("stasis_gfx_poll_reload")?,
            stasis_load_font: lib.symbol_address("stasis_load_font")?,
            stasis_measure_text: lib.symbol_address("stasis_measure_text")?,
            stasis_gfx_cache_text: lib.symbol_address("stasis_gfx_cache_text")?,
            stasis_gfx_measure_text_cached: lib.symbol_address("stasis_gfx_measure_text_cached")?,
            stasis_gfx_measure_text_cached_height: lib
                .symbol_address("stasis_gfx_measure_text_cached_height")?,
            #[cfg(windows)]
            stasis_storage_load_i32: lib.symbol_address("stasis_storage_load_i32").ok(),
            #[cfg(windows)]
            stasis_storage_save_i32: lib.symbol_address("stasis_storage_save_i32").ok(),
            _lib: lib,
        })
    }
}

pub fn schedule_runtime_screenshot(path: &Path) -> Result<(), String> {
    let api = stasis_graphics_assets_api()?;
    let address = api.stasis_host_schedule_screenshot.ok_or_else(|| {
        "the loaded graphics runtime does not support dynamic screenshots".to_string()
    })?;
    let path = CString::new(path.to_string_lossy().as_bytes())
        .map_err(|_| "screenshot path contains an interior NUL byte".to_string())?;
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    if callback(path.as_ptr()) == 0 {
        return Err("graphics runtime rejected the screenshot path".to_string());
    }
    Ok(())
}

fn stasis_graphics_assets_api() -> Result<&'static StasisGraphicsAssetsApi, String> {
    static API: OnceLock<Result<StasisGraphicsAssetsApi, String>> = OnceLock::new();
    match API.get_or_init(StasisGraphicsAssetsApi::load_default) {
        Ok(api) => Ok(api),
        Err(error) => Err(error.clone()),
    }
}

pub fn runtime_library_candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // A release bundle is one unit. Never let an environment override replace its sibling
            // runtime with a different build.
            for file_name in runtime_library_file_names() {
                out.push(exe_dir.join(file_name));
            }

            // Dev-friendly default: locate the runtime built under the repo tree by
            // walking a few parents from the executable location.
            for ancestor in exe_dir.ancestors().take(6) {
                for file_name in runtime_library_file_names() {
                    for configuration in [None, Some("Release"), Some("Debug")] {
                        let mut candidate = ancestor.join("runtime").join("build").join("bin");
                        if let Some(configuration) = configuration {
                            candidate.push(configuration);
                        }
                        candidate.push(file_name);
                        out.push(candidate);
                    }
                }
            }
        }
    }
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_LIBRARY_PATH") {
        out.push(PathBuf::from(configured));
    }
    // Preserve the original variable as a compatibility alias for existing Windows workflows.
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_DLL_PATH") {
        out.push(PathBuf::from(configured));
    }

    // Allow loading from the current working directory too (handy for ad-hoc runs).
    for file_name in runtime_library_file_names() {
        out.push(PathBuf::from(file_name));
    }

    // Dev-friendly fallback: if the runtime exists under this repo checkout, include it.
    // This helps when `CARGO_TARGET_DIR` points outside the workspace (so `current_exe()` ancestry
    // won't include the repo root).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    for file_name in runtime_library_file_names() {
        for configuration in [None, Some("Release"), Some("Debug")] {
            let mut candidate = repo_root.join("runtime").join("build").join("bin");
            if let Some(configuration) = configuration {
                candidate.push(configuration);
            }
            candidate.push(file_name);
            if candidate.exists() {
                out.push(candidate);
            }
        }
    }
    out
}

#[cfg(windows)]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["stasis_graphics.dll"]
}

#[cfg(target_os = "macos")]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["libstasis_graphics.dylib", "stasis_graphics.dylib"]
}

#[cfg(all(unix, not(target_os = "macos")))]
fn runtime_library_file_names() -> &'static [&'static str] {
    &["libstasis_graphics.so", "stasis_graphics.so"]
}

pub fn clear_jit_string_literal_table() {
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.clear();
}

pub fn begin_jit_string_literal_staging() -> Result<(), String> {
    JIT_STRING_LITERAL_STAGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_some() {
            return Err("JIT string literal staging is already active on this thread".to_string());
        }
        *slot = Some(JitStringLiteralStage::default());
        Ok(())
    })
}

pub fn finish_jit_string_literal_staging() -> Result<HashMap<i32, String>, String> {
    JIT_STRING_LITERAL_STAGE.with(|slot| {
        let stage = slot
            .borrow_mut()
            .take()
            .ok_or_else(|| "JIT string literal staging is not active".to_string())?;
        if let Some(error) = stage.collision {
            Err(error)
        } else {
            Ok(stage.literals)
        }
    })
}

pub fn replace_jit_string_literal_table(literals: &HashMap<i32, String>) {
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.clear();
    guard.extend(literals.iter().map(|(id, value)| (*id, value.clone())));
}

pub fn upsert_jit_string_literal(id: i32, value: &str) {
    let staged = JIT_STRING_LITERAL_STAGE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(stage) = slot.as_mut() else {
            return false;
        };
        if let Some(previous) = stage.literals.get(&id) {
            if previous != value && stage.collision.is_none() {
                stage.collision = Some(format!(
                    "JIT string literal hash collision for id {id}: '{previous}' vs '{value}'"
                ));
            }
        } else {
            stage.literals.insert(id, value.to_string());
        }
        true
    });
    if staged {
        return;
    }
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.insert(id, value.to_string());
}

#[no_mangle]
pub extern "C" fn stasis_jit_clear_string_literal_table() {
    clear_jit_string_literal_table();
}

#[no_mangle]
pub extern "C" fn stasis_jit_upsert_string_literal(id: i32, value: *const c_char) {
    if value.is_null() {
        return;
    }
    #[cfg(windows)]
    let text = unsafe { std::ffi::CStr::from_ptr(value) };
    #[cfg(not(windows))]
    let text = unsafe { std::ffi::CStr::from_ptr(value) };
    if let Ok(text) = text.to_str() {
        upsert_jit_string_literal(id, text);
    }
}

pub fn jit_string_literal_value(id: i32) -> Option<String> {
    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&id).cloned()
}

// ============================================================
// Registered global memory (in-process engine)
// ============================================================

type ArrayKey = (i32, i32); // (collection_hash, field_hash)

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JitStorageKind {
    I32,
    F32,
    F64,
    U8,
    U16,
}

#[repr(C)]
pub struct JitStorageSlot {
    data: usize,
    len: usize,
}

impl JitStorageSlot {
    pub const DATA_OFFSET: i32 = 0;
    pub const LEN_OFFSET: i32 = std::mem::size_of::<usize>() as i32;
}

// Slot fields are only rebound while no guest execution window is active. Generated code reads
// them directly, so the registry mutex is deliberately absent from the load/store hot path.
unsafe impl Send for JitStorageSlot {}
unsafe impl Sync for JitStorageSlot {}

type StorageKey = (JitStorageKind, i32, i32);

fn direct_storage_slots() -> &'static Mutex<HashMap<StorageKey, Box<JitStorageSlot>>> {
    static TABLE: OnceLock<Mutex<HashMap<StorageKey, Box<JitStorageSlot>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn guest_execution_count() -> &'static AtomicUsize {
    static COUNT: AtomicUsize = AtomicUsize::new(0);
    &COUNT
}

pub struct JitExecutionGuard;

impl JitExecutionGuard {
    pub fn enter() -> Self {
        guest_execution_count().fetch_add(1, Ordering::AcqRel);
        Self
    }
}

impl Drop for JitExecutionGuard {
    fn drop(&mut self) {
        guest_execution_count().fetch_sub(1, Ordering::AcqRel);
    }
}

fn ensure_rebind_allowed() -> Result<(), String> {
    (guest_execution_count().load(Ordering::Acquire) == 0)
        .then_some(())
        .ok_or_else(|| {
            "storage rebinding is only allowed between guest execution windows".to_string()
        })
}

fn acquire_rebind_guard() -> Result<MutexGuard<'static, ()>, String> {
    ensure_rebind_allowed()?;
    let guard = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    ensure_rebind_allowed()?;
    Ok(guard)
}

fn update_direct_storage_slot(key: StorageKey, data: usize, len: usize) {
    if let Some(slot) = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .get_mut(&key)
    {
        slot.data = data;
        slot.len = len;
    }
}

pub fn direct_scalar_storage_slot_address(
    kind: JitStorageKind,
    path_hash: i32,
) -> Result<usize, String> {
    direct_storage_slot_address(kind, path_hash, 0)
}

pub fn direct_array_storage_slot_address(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<usize, String> {
    direct_storage_slot_address(kind, collection_hash, field_hash)
}

fn direct_storage_slot_address(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<usize, String> {
    let key = (kind, collection_hash, field_hash);
    let current = registered_storage(kind, collection_hash, field_hash);
    let (data, actual_len) = current.unwrap_or((0, 0));
    let mut slots = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned");
    let slot = slots.entry(key).or_insert_with(|| {
        Box::new(JitStorageSlot {
            data,
            len: actual_len,
        })
    });
    Ok(slot.as_ref() as *const JitStorageSlot as usize)
}

pub fn provision_direct_scalar_storage(kind: JitStorageKind, path_hash: i32) -> Result<(), String> {
    let _rebind = acquire_rebind_guard()?;
    match kind {
        JitStorageKind::I32 => ensure_owned_i32_scalar(path_hash)?,
        JitStorageKind::F32 => ensure_owned_f32_scalar(path_hash)?,
        JitStorageKind::F64 => ensure_owned_f64_scalar(path_hash)?,
        JitStorageKind::U8 => ensure_jit_u8_array_capacity_unlocked(path_hash, 0, 1)?,
        JitStorageKind::U16 => ensure_jit_u16_array_capacity_unlocked(path_hash, 0, 1)?,
    }
    let (data, len) = registered_storage(kind, path_hash, 0)?;
    update_direct_storage_slot((kind, path_hash, 0), data, len);
    Ok(())
}

pub fn provision_direct_array_storage(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
    len: usize,
) -> Result<(), String> {
    let _rebind = acquire_rebind_guard()?;
    let key = (kind, collection_hash, field_hash);
    match kind {
        JitStorageKind::I32 => {
            ensure_jit_i32_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::F32 => {
            ensure_jit_f32_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::F64 => {
            ensure_jit_f64_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::U8 => {
            ensure_jit_u8_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
        JitStorageKind::U16 => {
            ensure_jit_u16_array_capacity_unlocked(collection_hash, field_hash, len)?
        }
    }
    let (data, actual_len) = registered_storage(kind, collection_hash, field_hash)?;
    update_direct_storage_slot(key, data, actual_len);
    Ok(())
}

fn registered_i32_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f32_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f64_ptrs() -> &'static Mutex<HashMap<i32, usize>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, usize>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_i32_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f32_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_f64_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_u8_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn registered_u16_arrays() -> &'static Mutex<HashMap<ArrayKey, (usize, usize)>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, (usize, usize)>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_i32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<i32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<i32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_u8_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<u8>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<u8>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_u16_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<u16>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<u16>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f64_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_i32_scalars() -> &'static Mutex<HashMap<i32, Box<i32>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<i32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f32_scalars() -> &'static Mutex<HashMap<i32, Box<f32>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<f32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f64_scalars() -> &'static Mutex<HashMap<i32, Box<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<i32, Box<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn ensure_owned_scalar<T: Copy + Default>(
    path_hash: i32,
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
    fallback: &Mutex<HashMap<i32, T>>,
) -> Result<(), String> {
    if registered
        .lock()
        .expect("registered scalar table mutex poisoned")
        .contains_key(&path_hash)
    {
        return Ok(());
    }
    ensure_rebind_allowed()?;
    let initial = fallback
        .lock()
        .expect("fallback scalar table mutex poisoned")
        .remove(&path_hash)
        .unwrap_or_default();
    let mut owned = owned.lock().expect("owned scalar table mutex poisoned");
    let value = owned.entry(path_hash).or_insert_with(|| Box::new(initial));
    let address = value.as_mut() as *mut T as usize;
    registered
        .lock()
        .expect("registered scalar table mutex poisoned")
        .insert(path_hash, address);
    Ok(())
}

fn ensure_owned_i32_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_i32_scalars(),
        registered_i32_ptrs(),
        jit_i32_global_table(),
    )
}

fn ensure_owned_f32_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_f32_scalars(),
        registered_f32_ptrs(),
        jit_f32_global_table(),
    )
}

fn ensure_owned_f64_scalar(path_hash: i32) -> Result<(), String> {
    ensure_owned_scalar(
        path_hash,
        owned_f64_scalars(),
        registered_f64_ptrs(),
        jit_f64_global_table(),
    )
}

fn registered_storage(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<(usize, usize), String> {
    let value = match kind {
        JitStorageKind::I32 if field_hash == 0 => registered_i32_ptrs()
            .lock()
            .expect("registered i32 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_i32_arrays()
                    .lock()
                    .expect("registered i32 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::I32 => registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::F32 if field_hash == 0 => registered_f32_ptrs()
            .lock()
            .expect("registered f32 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_f32_arrays()
                    .lock()
                    .expect("registered f32 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::F32 => registered_f32_arrays()
            .lock()
            .expect("registered f32 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::F64 if field_hash == 0 => registered_f64_ptrs()
            .lock()
            .expect("registered f64 ptr table mutex poisoned")
            .get(&collection_hash)
            .copied()
            .map(|ptr| (ptr, 1))
            .or_else(|| {
                registered_f64_arrays()
                    .lock()
                    .expect("registered f64 array table mutex poisoned")
                    .get(&(collection_hash, field_hash))
                    .copied()
            }),
        JitStorageKind::F64 => registered_f64_arrays()
            .lock()
            .expect("registered f64 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::U8 => registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
        JitStorageKind::U16 => registered_u16_arrays()
            .lock()
            .expect("registered u16 array table mutex poisoned")
            .get(&(collection_hash, field_hash))
            .copied(),
    };
    value.ok_or_else(|| "direct storage backing was not provisioned".to_string())
}

fn ensure_owned_array_capacity<T: Copy>(
    key: ArrayKey,
    requested_len: usize,
    default: T,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) -> Result<(), String> {
    let mut owned_guard = owned.lock().expect("owned array table mutex poisoned");
    if let Some(array) = owned_guard.get_mut(&key) {
        if let Some((registered_ptr, registered_len)) = registered
            .lock()
            .expect("registered array table mutex poisoned")
            .get(&key)
            .copied()
        {
            if registered_ptr != array.as_mut_ptr() as usize {
                return (registered_len >= requested_len)
                    .then_some(())
                    .ok_or_else(|| {
                        format!(
                            "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
                        )
                    });
            }
        }
        array.resize(array.len().max(requested_len), default);
        let mut registered_guard = registered
            .lock()
            .expect("registered array table mutex poisoned");
        registered_guard.insert(key, (array.as_mut_ptr() as usize, array.len()));
        return Ok(());
    }
    if let Some((_, registered_len)) = registered
        .lock()
        .expect("registered array table mutex poisoned")
        .get(&key)
        .copied()
    {
        return (registered_len >= requested_len)
            .then_some(())
            .ok_or_else(|| {
                format!(
                "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
            )
            });
    }
    let mut array = vec![default; requested_len];
    let ptr = array.as_mut_ptr() as usize;
    owned_guard.insert(key, array);
    registered
        .lock()
        .expect("registered array table mutex poisoned")
        .insert(key, (ptr, requested_len));
    Ok(())
}

fn preflight_owned_array_capacity<T>(
    key: ArrayKey,
    requested_len: usize,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) -> Result<(), String> {
    let owned_guard = owned.lock().expect("owned array table mutex poisoned");
    let registered_entry = registered
        .lock()
        .expect("registered array table mutex poisoned")
        .get(&key)
        .copied();
    if let Some(array) = owned_guard.get(&key) {
        if registered_entry
            .is_none_or(|(registered_ptr, _)| registered_ptr == array.as_ptr() as usize)
        {
            return Ok(());
        }
    }
    if let Some((_, registered_len)) = registered_entry {
        return (registered_len >= requested_len)
            .then_some(())
            .ok_or_else(|| {
                format!(
                    "cannot grow host-owned collection storage from {registered_len} to {requested_len}"
                )
            });
    }
    Ok(())
}

pub fn preflight_jit_i32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_i32_arrays(),
        registered_i32_arrays(),
    )
}

pub fn preflight_jit_f32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_f32_arrays(),
        registered_f32_arrays(),
    )
}

pub fn preflight_jit_f64_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_f64_arrays(),
        registered_f64_arrays(),
    )
}

fn migrate_fallback_array<T: Copy>(
    key: ArrayKey,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    fallback: &Mutex<HashMap<(i32, i32, i32), T>>,
) {
    let mut owned_guard = owned.lock().expect("owned array table mutex poisoned");
    let Some(array) = owned_guard.get_mut(&key) else {
        return;
    };
    let mut fallback = fallback
        .lock()
        .expect("fallback array table mutex poisoned");
    for (index, slot) in array.iter_mut().enumerate() {
        if let Some(value) = fallback.remove(&(key.0, key.1, index as i32)) {
            *slot = value;
        }
    }
}

fn direct_array_rebind_guard(
    kind: JitStorageKind,
    collection_hash: i32,
    field_hash: i32,
) -> Result<Option<MutexGuard<'static, ()>>, String> {
    let has_direct_slot = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .contains_key(&(kind, collection_hash, field_hash));
    has_direct_slot.then(acquire_rebind_guard).transpose()
}

pub fn ensure_jit_i32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::I32, collection_hash, field_hash)?;
    ensure_jit_i32_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_i32_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::I32);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_i32_arrays(),
        registered_i32_arrays(),
    )?;
    migrate_fallback_array(key, owned_i32_arrays(), jit_i32_array_global_table());
    if let Some((data, len)) = registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::I32, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_f32_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::F32, collection_hash, field_hash)?;
    ensure_jit_f32_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_f32_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0.0,
        owned_f32_arrays(),
        registered_f32_arrays(),
    )?;
    migrate_fallback_array(key, owned_f32_arrays(), jit_f32_array_global_table());
    if let Some((data, len)) = registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::F32, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_f64_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::F64, collection_hash, field_hash)?;
    ensure_jit_f64_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_f64_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0.0,
        owned_f64_arrays(),
        registered_f64_arrays(),
    )?;
    migrate_fallback_array(key, owned_f64_arrays(), jit_f64_array_global_table());
    if let Some((data, len)) = registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        update_direct_storage_slot((JitStorageKind::F64, key.0, key.1), data, len);
    }
    Ok(())
}

pub fn ensure_jit_u8_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::U8, collection_hash, field_hash)?;
    ensure_jit_u8_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_u8_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U8);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_u8_arrays(),
        registered_u8_arrays(),
    )?;
    let (data, len) = registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .copied()
        .ok_or_else(|| "u8 storage was not provisioned".to_string())?;
    update_direct_storage_slot((JitStorageKind::U8, key.0, key.1), data, len);
    Ok(())
}

pub fn preflight_jit_u8_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_u8_arrays(),
        registered_u8_arrays(),
    )
}

pub fn preflight_jit_u16_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    preflight_owned_array_capacity(
        (collection_hash, field_hash),
        requested_len,
        owned_u16_arrays(),
        registered_u16_arrays(),
    )
}

pub fn ensure_jit_u16_array_capacity(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let _rebind = direct_array_rebind_guard(JitStorageKind::U16, collection_hash, field_hash)?;
    ensure_jit_u16_array_capacity_unlocked(collection_hash, field_hash, requested_len)
}

fn ensure_jit_u16_array_capacity_unlocked(
    collection_hash: i32,
    field_hash: i32,
    requested_len: usize,
) -> Result<(), String> {
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U16);
    ensure_owned_array_capacity(
        key,
        requested_len,
        0,
        owned_u16_arrays(),
        registered_u16_arrays(),
    )?;
    let (data, len) = registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .get(&key)
        .copied()
        .ok_or_else(|| "u16 storage was not provisioned".to_string())?;
    update_direct_storage_slot((JitStorageKind::U16, key.0, key.1), data, len);
    Ok(())
}

pub fn clear_registered_global_memory() {
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    registered_i32_ptrs()
        .lock()
        .expect("registered i32 ptr table mutex poisoned")
        .clear();
    registered_f32_ptrs()
        .lock()
        .expect("registered f32 ptr table mutex poisoned")
        .clear();
    registered_f64_ptrs()
        .lock()
        .expect("registered f64 ptr table mutex poisoned")
        .clear();
    registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .clear();
    registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .clear();
    registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .clear();
    registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .clear();
    registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .clear();

    owned_f32_arrays()
        .lock()
        .expect("owned f32 array table mutex poisoned")
        .clear();
    owned_i32_arrays()
        .lock()
        .expect("owned i32 array table mutex poisoned")
        .clear();
    owned_f64_arrays()
        .lock()
        .expect("owned f64 array table mutex poisoned")
        .clear();
    owned_u8_arrays()
        .lock()
        .expect("owned u8 array table mutex poisoned")
        .clear();
    owned_u16_arrays()
        .lock()
        .expect("owned u16 array table mutex poisoned")
        .clear();
    owned_i32_scalars()
        .lock()
        .expect("owned i32 scalar table mutex poisoned")
        .clear();
    owned_f32_scalars()
        .lock()
        .expect("owned f32 scalar table mutex poisoned")
        .clear();
    owned_f64_scalars()
        .lock()
        .expect("owned f64 scalar table mutex poisoned")
        .clear();
    direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .clear();
}

#[derive(Debug, Clone)]
pub struct JitRuntimeStateSnapshot {
    i32_globals: JitI32GlobalMap,
    f32_globals: JitF32GlobalMap,
    f64_globals: JitF64GlobalMap,
    i32_array_globals: JitI32ArrayGlobalMap,
    f32_array_globals: JitF32ArrayGlobalMap,
    f64_array_globals: JitF64ArrayGlobalMap,
    owned_i32_scalars: HashMap<i32, i32>,
    owned_f32_scalars: HashMap<i32, f32>,
    owned_f64_scalars: HashMap<i32, f64>,
    owned_i32_arrays: HashMap<ArrayKey, Vec<i32>>,
    owned_f32_arrays: HashMap<ArrayKey, Vec<f32>>,
    owned_f64_arrays: HashMap<ArrayKey, Vec<f64>>,
    owned_u8_arrays: HashMap<ArrayKey, Vec<u8>>,
    owned_u16_arrays: HashMap<ArrayKey, Vec<u16>>,
}

/// Captures state owned by the Stasis runtime.
///
/// Host-registered pointers are borrowed FFI memory. They are deliberately excluded: the
/// registry cannot prove that those allocations still exist when a snapshot is restored.
pub fn snapshot_jit_runtime_state() -> JitRuntimeStateSnapshot {
    snapshot_jit_runtime_state_bounded(usize::MAX)
        .expect("unbounded JIT runtime snapshot cannot exceed usize")
}

pub fn snapshot_jit_runtime_state_bounded(
    max_bytes: usize,
) -> Result<JitRuntimeStateSnapshot, String> {
    let snapshot_bytes = runtime_snapshot_bytes()?;
    if snapshot_bytes > max_bytes {
        return Err(format!(
            "live runtime snapshot requires {snapshot_bytes} bytes; limit is {max_bytes} bytes"
        ));
    }
    Ok(JitRuntimeStateSnapshot {
        i32_globals: jit_i32_global_table()
            .lock()
            .expect("jit i32 global table mutex poisoned")
            .clone(),
        f32_globals: jit_f32_global_table()
            .lock()
            .expect("jit f32 global table mutex poisoned")
            .clone(),
        f64_globals: jit_f64_global_table()
            .lock()
            .expect("jit f64 global table mutex poisoned")
            .clone(),
        i32_array_globals: jit_i32_array_global_table()
            .lock()
            .expect("jit i32 array global table mutex poisoned")
            .clone(),
        f32_array_globals: jit_f32_array_global_table()
            .lock()
            .expect("jit f32 array global table mutex poisoned")
            .clone(),
        f64_array_globals: jit_f64_array_global_table()
            .lock()
            .expect("jit f64 array global table mutex poisoned")
            .clone(),
        owned_i32_scalars: snapshot_owned_scalars(owned_i32_scalars(), registered_i32_ptrs()),
        owned_f32_scalars: snapshot_owned_scalars(owned_f32_scalars(), registered_f32_ptrs()),
        owned_f64_scalars: snapshot_owned_scalars(owned_f64_scalars(), registered_f64_ptrs()),
        owned_i32_arrays: snapshot_owned_arrays(owned_i32_arrays()),
        owned_f32_arrays: snapshot_owned_arrays(owned_f32_arrays()),
        owned_f64_arrays: snapshot_owned_arrays(owned_f64_arrays()),
        owned_u8_arrays: snapshot_owned_arrays(owned_u8_arrays()),
        owned_u16_arrays: snapshot_owned_arrays(owned_u16_arrays()),
    })
}

fn snapshot_owned_scalars<T: Copy>(
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
) -> HashMap<i32, T> {
    let owned = owned.lock().expect("owned scalar table mutex poisoned");
    let registered = registered
        .lock()
        .expect("registered scalar table mutex poisoned");
    owned
        .iter()
        .filter(|(key, value)| registered.get(key) == Some(&(value.as_ref() as *const T as usize)))
        .map(|(key, value)| (*key, **value))
        .collect()
}

fn snapshot_owned_arrays<T: Clone>(
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
) -> HashMap<ArrayKey, Vec<T>> {
    owned
        .lock()
        .expect("owned array table mutex poisoned")
        .clone()
}

fn runtime_snapshot_bytes() -> Result<usize, String> {
    let mut bytes = 0usize;
    let mut add = |count: usize, item_bytes: usize| -> Result<(), String> {
        bytes = bytes
            .checked_add(
                count
                    .checked_mul(item_bytes)
                    .ok_or_else(|| "live runtime snapshot size overflow".to_string())?,
            )
            .ok_or_else(|| "live runtime snapshot size overflow".to_string())?;
        Ok(())
    };
    add(
        jit_i32_global_table()
            .lock()
            .expect("jit i32 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, i32)>(),
    )?;
    add(
        jit_f32_global_table()
            .lock()
            .expect("jit f32 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, f32)>(),
    )?;
    add(
        jit_f64_global_table()
            .lock()
            .expect("jit f64 global table mutex poisoned")
            .len(),
        std::mem::size_of::<(i32, f64)>(),
    )?;
    add(
        jit_i32_array_global_table()
            .lock()
            .expect("jit i32 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), i32)>(),
    )?;
    add(
        jit_f32_array_global_table()
            .lock()
            .expect("jit f32 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), f32)>(),
    )?;
    add(
        jit_f64_array_global_table()
            .lock()
            .expect("jit f64 array global table mutex poisoned")
            .len(),
        std::mem::size_of::<((i32, i32, i32), f64)>(),
    )?;
    add_owned_scalar_bytes(&mut add, owned_i32_scalars(), std::mem::size_of::<i32>())?;
    add_owned_scalar_bytes(&mut add, owned_f32_scalars(), std::mem::size_of::<f32>())?;
    add_owned_scalar_bytes(&mut add, owned_f64_scalars(), std::mem::size_of::<f64>())?;
    add_owned_array_bytes(&mut add, owned_i32_arrays(), std::mem::size_of::<i32>())?;
    add_owned_array_bytes(&mut add, owned_f32_arrays(), std::mem::size_of::<f32>())?;
    add_owned_array_bytes(&mut add, owned_f64_arrays(), std::mem::size_of::<f64>())?;
    add_owned_array_bytes(&mut add, owned_u8_arrays(), std::mem::size_of::<u8>())?;
    add_owned_array_bytes(&mut add, owned_u16_arrays(), std::mem::size_of::<u16>())?;
    Ok(bytes)
}

fn add_owned_scalar_bytes<T>(
    add: &mut impl FnMut(usize, usize) -> Result<(), String>,
    table: &Mutex<HashMap<i32, Box<T>>>,
    item_bytes: usize,
) -> Result<(), String> {
    add(
        table
            .lock()
            .expect("owned scalar table mutex poisoned")
            .len(),
        item_bytes,
    )
}

fn add_owned_array_bytes<T>(
    add: &mut impl FnMut(usize, usize) -> Result<(), String>,
    table: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    item_bytes: usize,
) -> Result<(), String> {
    let elements = table
        .lock()
        .expect("owned array table mutex poisoned")
        .values()
        .try_fold(0usize, |total, values| {
            total
                .checked_add(values.len())
                .ok_or_else(|| "live runtime snapshot size overflow".to_string())
        })?;
    add(elements, item_bytes)
}

pub fn restore_jit_runtime_state(snapshot: &JitRuntimeStateSnapshot) {
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    *jit_i32_global_table()
        .lock()
        .expect("jit i32 global table mutex poisoned") = snapshot.i32_globals.clone();
    *jit_f32_global_table()
        .lock()
        .expect("jit f32 global table mutex poisoned") = snapshot.f32_globals.clone();
    *jit_f64_global_table()
        .lock()
        .expect("jit f64 global table mutex poisoned") = snapshot.f64_globals.clone();
    *jit_i32_array_global_table()
        .lock()
        .expect("jit i32 array global table mutex poisoned") = snapshot.i32_array_globals.clone();
    *jit_f32_array_global_table()
        .lock()
        .expect("jit f32 array global table mutex poisoned") = snapshot.f32_array_globals.clone();
    *jit_f64_array_global_table()
        .lock()
        .expect("jit f64 array global table mutex poisoned") = snapshot.f64_array_globals.clone();
    restore_owned_scalars(
        &snapshot.owned_i32_scalars,
        owned_i32_scalars(),
        registered_i32_ptrs(),
    );
    restore_owned_scalars(
        &snapshot.owned_f32_scalars,
        owned_f32_scalars(),
        registered_f32_ptrs(),
    );
    restore_owned_scalars(
        &snapshot.owned_f64_scalars,
        owned_f64_scalars(),
        registered_f64_ptrs(),
    );
    restore_owned_arrays(
        &snapshot.owned_i32_arrays,
        owned_i32_arrays(),
        registered_i32_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_f32_arrays,
        owned_f32_arrays(),
        registered_f32_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_f64_arrays,
        owned_f64_arrays(),
        registered_f64_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_u8_arrays,
        owned_u8_arrays(),
        registered_u8_arrays(),
    );
    restore_owned_arrays(
        &snapshot.owned_u16_arrays,
        owned_u16_arrays(),
        registered_u16_arrays(),
    );
    refresh_direct_storage_slots();
}

fn refresh_direct_storage_slots() {
    let keys = direct_storage_slots()
        .lock()
        .expect("direct storage slot table mutex poisoned")
        .keys()
        .copied()
        .collect::<Vec<_>>();
    for (kind, collection_hash, field_hash) in keys {
        let (data, len) = registered_storage(kind, collection_hash, field_hash).unwrap_or((0, 0));
        update_direct_storage_slot((kind, collection_hash, field_hash), data, len);
    }
}

fn restore_owned_scalars<T: Copy>(
    snapshot: &HashMap<i32, T>,
    owned: &Mutex<HashMap<i32, Box<T>>>,
    registered: &Mutex<HashMap<i32, usize>>,
) {
    let mut owned = owned.lock().expect("owned scalar table mutex poisoned");
    let previous_addresses = owned
        .iter()
        .map(|(key, value)| (*key, value.as_ref() as *const T as usize))
        .collect::<HashMap<_, _>>();
    owned.retain(|key, _| snapshot.contains_key(key));
    for (key, value) in snapshot {
        **owned.entry(*key).or_insert_with(|| Box::new(*value)) = *value;
    }
    let mut registered = registered
        .lock()
        .expect("registered scalar table mutex poisoned");
    registered.retain(|key, address| {
        snapshot.contains_key(key) || previous_addresses.get(key) != Some(address)
    });
    for (key, value) in owned.iter_mut() {
        registered.insert(*key, value.as_mut() as *mut T as usize);
    }
}

fn restore_owned_arrays<T: Clone>(
    snapshot: &HashMap<ArrayKey, Vec<T>>,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
    registered: &Mutex<HashMap<ArrayKey, (usize, usize)>>,
) {
    let mut owned = owned.lock().expect("owned array table mutex poisoned");
    let previous_addresses = owned
        .iter()
        .map(|(key, values)| (*key, values.as_ptr() as usize))
        .collect::<HashMap<_, _>>();
    owned.retain(|key, _| snapshot.contains_key(key));
    for (key, values) in snapshot {
        let target = owned.entry(*key).or_default();
        target.clear();
        target.extend_from_slice(values);
    }
    let mut registered = registered
        .lock()
        .expect("registered array table mutex poisoned");
    registered.retain(|key, (address, _)| {
        snapshot.contains_key(key) || previous_addresses.get(key) != Some(address)
    });
    for (key, values) in owned.iter_mut() {
        registered.insert(*key, (values.as_mut_ptr() as usize, values.len()));
    }
}

pub fn register_global_i32_ptr(path_hash: i32, ptr: *mut i32) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_i32_scalars());
    let table = registered_i32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered i32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::I32, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_f32_ptr(path_hash: i32, ptr: *mut f32) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_f32_scalars());
    let table = registered_f32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::F32, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_f64_ptr(path_hash: i32, ptr: *mut f64) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    remove_replaced_owned_scalar(path_hash, ptr as usize, owned_f64_scalars());
    let table = registered_f64_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f64 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
    update_direct_storage_slot((JitStorageKind::F64, path_hash, 0), ptr as usize, 1);
}

pub fn register_global_i32_array(collection_hash: i32, field_hash: i32, ptr: *mut i32, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::I32);
    remove_replaced_owned_array(key, ptr as usize, owned_i32_arrays());
    let table = registered_i32_arrays();
    let mut guard = table
        .lock()
        .expect("registered i32 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::I32, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn register_global_f32_array(collection_hash: i32, field_hash: i32, ptr: *mut f32, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    let key = (collection_hash, field_hash);
    remove_replaced_owned_array(key, ptr as usize, owned_f32_arrays());
    let table = registered_f32_arrays();
    let mut guard = table
        .lock()
        .expect("registered f32 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::F32, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn register_global_f64_array(collection_hash: i32, field_hash: i32, ptr: *mut f64, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    let key = (collection_hash, field_hash);
    remove_replaced_owned_array(key, ptr as usize, owned_f64_arrays());
    let table = registered_f64_arrays();
    let mut guard = table
        .lock()
        .expect("registered f64 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::F64, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

fn remove_replaced_owned_array<T>(
    key: ArrayKey,
    registered_ptr: usize,
    owned: &Mutex<HashMap<ArrayKey, Vec<T>>>,
) {
    let mut guard = owned.lock().expect("owned array table mutex poisoned");
    if guard
        .get(&key)
        .is_some_and(|values| values.as_ptr() as usize != registered_ptr)
    {
        guard.remove(&key);
    }
}

fn remove_replaced_owned_scalar<T>(
    key: i32,
    registered_ptr: usize,
    owned: &Mutex<HashMap<i32, Box<T>>>,
) {
    let mut guard = owned.lock().expect("owned scalar table mutex poisoned");
    if guard
        .get(&key)
        .is_some_and(|value| value.as_ref() as *const T as usize != registered_ptr)
    {
        guard.remove(&key);
    }
}

fn discard_integer_array_lane(key: ArrayKey, retained: JitStorageKind) {
    if retained != JitStorageKind::I32 {
        registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .remove(&key);
        owned_i32_arrays()
            .lock()
            .expect("owned i32 array table mutex poisoned")
            .remove(&key);
    }
    if retained != JitStorageKind::U8 {
        registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .remove(&key);
        owned_u8_arrays()
            .lock()
            .expect("owned u8 array table mutex poisoned")
            .remove(&key);
    }
    if retained != JitStorageKind::U16 {
        registered_u16_arrays()
            .lock()
            .expect("registered u16 array table mutex poisoned")
            .remove(&key);
        owned_u16_arrays()
            .lock()
            .expect("owned u16 array table mutex poisoned")
            .remove(&key);
    }
}

pub fn register_global_u16_array(collection_hash: i32, field_hash: i32, ptr: *mut u16, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U16);
    remove_replaced_owned_array(key, ptr as usize, owned_u16_arrays());
    registered_u16_arrays()
        .lock()
        .expect("registered u16 array table mutex poisoned")
        .insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::U16, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

pub fn register_global_u8_array(collection_hash: i32, field_hash: i32, ptr: *mut u8, len: usize) {
    if ptr.is_null() {
        return;
    }
    let Ok(_rebind) = acquire_rebind_guard() else {
        return;
    };
    let key = (collection_hash, field_hash);
    discard_integer_array_lane(key, JitStorageKind::U8);
    remove_replaced_owned_array(key, ptr as usize, owned_u8_arrays());
    let table = registered_u8_arrays();
    let mut guard = table
        .lock()
        .expect("registered u8 array table mutex poisoned");
    guard.insert(key, (ptr as usize, len));
    update_direct_storage_slot(
        (JitStorageKind::U8, collection_hash, field_hash),
        ptr as usize,
        len,
    );
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_i32_ptr(path_hash: i32, ptr: *mut i32) {
    register_global_i32_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f32_ptr(path_hash: i32, ptr: *mut f32) {
    register_global_f32_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f64_ptr(path_hash: i32, ptr: *mut f64) {
    register_global_f64_ptr(path_hash, ptr);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_i32_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut i32,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_i32_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f32_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut f32,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_f32_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_f64_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut f64,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_f64_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_u8_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut u8,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_u8_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_global_u16_array(
    collection_hash: i32,
    field_hash: i32,
    ptr: *mut u16,
    len: i32,
) {
    if len <= 0 {
        return;
    }
    register_global_u16_array(collection_hash, field_hash, ptr, len as usize);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut i32 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut i32;
        }
    }
    if ensure_jit_i32_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut i32)
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut f32 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut f32;
        }
    }
    if ensure_jit_f32_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_f32_arrays()
        .lock()
        .expect("registered f32 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut f32)
}

fn global_u8_array_ptr(collection_hash: i32, field_hash: i32, len: i32) -> *mut u8 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut u8;
        }
    }
    if ensure_jit_u8_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut u8)
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut f64 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);
    if let Some((ptr, registered_len)) = registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .copied()
    {
        if registered_len >= requested_len {
            return ptr as *mut f64;
        }
    }
    if ensure_jit_f64_array_capacity(collection_hash, field_hash, requested_len).is_err() {
        return std::ptr::null_mut();
    }
    registered_f64_arrays()
        .lock()
        .expect("registered f64 array table mutex poisoned")
        .get(&key)
        .map_or(std::ptr::null_mut(), |(ptr, _)| *ptr as *mut f64)
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_i32(value: i32) {
    write_jit_output(&value.to_string());
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_string(value_id: i32) {
    if let Some(bytes) = jit_text_arg_bytes(value_id) {
        write_jit_output(&String::from_utf8_lossy(&bytes));
        return;
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    if let Some(text) = guard.get(&value_id) {
        write_jit_output(text);
    }
}

pub const STASIS_RENDER_I32_COUNT: usize = 18_464;
pub const STASIS_RENDER_F32_COUNT: usize = 108_676;
pub const STASIS_RENDER_U8_COUNT: usize = 65_536;
const STASIS_RENDER_MAGIC: i32 = 0x4758_4631;
const STASIS_RENDER_VERSION: i32 = 2;
const STASIS_RENDER_HEADER_I32_COUNT: usize = 10;
const STASIS_RENDER_SPRITE_BASE: usize = 32;
const STASIS_RENDER_MAX_LINES: usize = 10_000;
const STASIS_RENDER_LINE_STRIDE: usize = 8;
const STASIS_RENDER_MAX_SPRITES: usize = 4_096;
const STASIS_RENDER_SPRITE_STRIDE_I32: usize = 3;
const STASIS_RENDER_SPRITE_BASE_F32: usize = 80_004;
const STASIS_RENDER_SPRITE_STRIDE_F32: usize = 4;
const STASIS_RENDER_TEXT_BASE_I32: usize = 12_320;
const STASIS_RENDER_TEXT_BASE_F32: usize = 96_388;
const STASIS_RENDER_MAX_TEXT: usize = 2_048;
const STASIS_RENDER_TEXT_STRIDE_I32: usize = 3;
const STASIS_RENDER_TEXT_STRIDE_F32: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderV2ActiveCounts {
    pub lines: usize,
    pub sprites: usize,
    pub text: usize,
    pub text_bytes: usize,
}

fn global_path_hash(path: &str) -> i32 {
    let mut hash = 2_166_136_261u32;
    for byte in path.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

/// Copies only the active spans of the canonical JIT render buffers.
///
/// The destination retains production offsets so the Android GLES adapter consumes the exact
/// same ABI as the SDL renderer without copying unused command capacity.
pub fn copy_jit_render_v2_active(
    out_i32: &mut [i32],
    out_f32: &mut [f32],
    out_u8: &mut [u8],
) -> Result<RenderV2ActiveCounts, String> {
    if out_i32.len() < STASIS_RENDER_I32_COUNT
        || out_f32.len() < STASIS_RENDER_F32_COUNT
        || out_u8.len() < STASIS_RENDER_U8_COUNT
    {
        return Err("production render destination has the wrong capacity".to_string());
    }
    let i32_ptr = stasis_jit_global_i32_array_ptr(
        global_path_hash("gfx_cmd_i32"),
        0,
        STASIS_RENDER_I32_COUNT as i32,
    );
    let f32_ptr = stasis_jit_global_f32_array_ptr(
        global_path_hash("gfx_cmd_f32"),
        0,
        STASIS_RENDER_F32_COUNT as i32,
    );
    let u8_ptr = global_u8_array_ptr(
        global_path_hash("gfx_cmd_u8"),
        0,
        STASIS_RENDER_U8_COUNT as i32,
    );
    if i32_ptr.is_null() || f32_ptr.is_null() || u8_ptr.is_null() {
        return Err("production render buffers were not registered by the JIT".to_string());
    }
    let source_i32 = unsafe { std::slice::from_raw_parts(i32_ptr, STASIS_RENDER_I32_COUNT) };
    let source_f32 = unsafe { std::slice::from_raw_parts(f32_ptr, STASIS_RENDER_F32_COUNT) };
    let source_u8 = unsafe { std::slice::from_raw_parts(u8_ptr, STASIS_RENDER_U8_COUNT) };
    if source_i32[0] != STASIS_RENDER_MAGIC || source_i32[1] != STASIS_RENDER_VERSION {
        return Err("JIT frame is not production gfx_cmd v2".to_string());
    }

    let counts = RenderV2ActiveCounts {
        lines: source_i32[3].clamp(0, STASIS_RENDER_MAX_LINES as i32) as usize,
        sprites: source_i32[4].clamp(0, STASIS_RENDER_MAX_SPRITES as i32) as usize,
        text: source_i32[7].clamp(0, STASIS_RENDER_MAX_TEXT as i32) as usize,
        text_bytes: source_i32[9].clamp(0, STASIS_RENDER_U8_COUNT as i32) as usize,
    };
    out_i32[..STASIS_RENDER_HEADER_I32_COUNT]
        .copy_from_slice(&source_i32[..STASIS_RENDER_HEADER_I32_COUNT]);
    let sprite_end = STASIS_RENDER_SPRITE_BASE + counts.sprites * STASIS_RENDER_SPRITE_STRIDE_I32;
    out_i32[STASIS_RENDER_SPRITE_BASE..sprite_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_SPRITE_BASE..sprite_end]);
    let text_i32_end = STASIS_RENDER_TEXT_BASE_I32 + counts.text * STASIS_RENDER_TEXT_STRIDE_I32;
    out_i32[STASIS_RENDER_TEXT_BASE_I32..text_i32_end]
        .copy_from_slice(&source_i32[STASIS_RENDER_TEXT_BASE_I32..text_i32_end]);

    out_f32[..4].copy_from_slice(&source_f32[..4]);
    let line_end = 4 + counts.lines * STASIS_RENDER_LINE_STRIDE;
    out_f32[4..line_end].copy_from_slice(&source_f32[4..line_end]);
    let sprite_f32_end =
        STASIS_RENDER_SPRITE_BASE_F32 + counts.sprites * STASIS_RENDER_SPRITE_STRIDE_F32;
    out_f32[STASIS_RENDER_SPRITE_BASE_F32..sprite_f32_end]
        .copy_from_slice(&source_f32[STASIS_RENDER_SPRITE_BASE_F32..sprite_f32_end]);
    let text_f32_end = STASIS_RENDER_TEXT_BASE_F32 + counts.text * STASIS_RENDER_TEXT_STRIDE_F32;
    out_f32[STASIS_RENDER_TEXT_BASE_F32..text_f32_end]
        .copy_from_slice(&source_f32[STASIS_RENDER_TEXT_BASE_F32..text_f32_end]);
    out_u8[..counts.text_bytes].copy_from_slice(&source_u8[..counts.text_bytes]);
    Ok(counts)
}

unsafe extern "C" {
    fn stasis_render_v2_trace_native(
        cmd_i32: *const i32,
        cmd_f32: *const f32,
        cmd_u8: *const u8,
    ) -> u32;
}

#[no_mangle]
pub unsafe extern "C" fn stasis_jit_render_v2_trace(
    cmd_i32_id: i32,
    cmd_i32_len: i32,
    cmd_f32_id: i32,
    cmd_f32_len: i32,
    cmd_u8_id: i32,
    cmd_u8_len: i32,
) -> i32 {
    if cmd_i32_len != STASIS_RENDER_I32_COUNT as i32
        || cmd_f32_len != STASIS_RENDER_F32_COUNT as i32
        || cmd_u8_len != STASIS_RENDER_U8_COUNT as i32
    {
        return 0;
    }
    let cmd_i32 = stasis_jit_global_i32_array_ptr(cmd_i32_id, 0, STASIS_RENDER_I32_COUNT as i32);
    let cmd_f32 = stasis_jit_global_f32_array_ptr(cmd_f32_id, 0, STASIS_RENDER_F32_COUNT as i32);
    let cmd_u8 = global_u8_array_ptr(cmd_u8_id, 0, STASIS_RENDER_U8_COUNT as i32);
    if cmd_i32.is_null() || cmd_f32.is_null() || cmd_u8.is_null() {
        return 0;
    }
    stasis_render_v2_trace_native(cmd_i32, cmd_f32, cmd_u8) as i32
}

fn jit_text_buffer_is_registered(value_id: i32) -> bool {
    if jit_collection_runtime_metadata_is_registered(value_id) {
        return true;
    }
    if registered_u8_arrays()
        .lock()
        .expect("registered u8 array table mutex poisoned")
        .contains_key(&(value_id, 0))
    {
        return true;
    }
    if registered_i32_arrays()
        .lock()
        .expect("registered i32 array table mutex poisoned")
        .contains_key(&(value_id, 0))
    {
        return true;
    }
    jit_i32_array_global_table()
        .lock()
        .expect("jit i32 array global table mutex poisoned")
        .keys()
        .any(|(collection_hash, field_hash, _)| *collection_hash == value_id && *field_hash == 0)
}

fn jit_global_text_bytes(value_id: i32) -> Option<Vec<u8>> {
    if !jit_text_buffer_is_registered(value_id) {
        return None;
    }
    let byte_len = stasis_jit_collection_i32_load(value_id, 1);
    if byte_len < 0 {
        return None;
    }

    let mut bytes = Vec::with_capacity(byte_len as usize);
    for index in 0..byte_len {
        let byte = stasis_jit_global_i32_array_load(value_id, 0, index);
        let Ok(byte) = u8::try_from(byte) else {
            return None;
        };
        bytes.push(byte);
    }
    Some(bytes)
}

fn jit_text_arg_bytes(value_id: i32) -> Option<Vec<u8>> {
    if let Some(bytes) = jit_global_text_bytes(value_id) {
        return Some(bytes);
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&value_id).map(|text| text.as_bytes().to_vec())
}

thread_local! {
    static JIT_TEXT_SCRATCH: std::cell::RefCell<Vec<u8>> =
        const { std::cell::RefCell::new(Vec::new()) };
}

fn with_jit_text_arg_bytes<R>(value_id: i32, mut callback: impl FnMut(&[u8]) -> R) -> Option<R> {
    if jit_text_buffer_is_registered(value_id) {
        let byte_len = usize::try_from(stasis_jit_collection_i32_load(value_id, 1)).ok()?;
        return JIT_TEXT_SCRATCH.with(|scratch| {
            let mut bytes = scratch.borrow_mut();
            bytes.clear();
            bytes.try_reserve(byte_len).ok()?;
            for index in 0..byte_len {
                let byte = stasis_jit_global_i32_array_load(value_id, 0, index as i32);
                bytes.push(u8::try_from(byte).ok()?);
            }
            Some(callback(&bytes))
        });
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.get(&value_id).map(|text| callback(text.as_bytes()))
}

#[derive(Clone, Copy)]
pub struct EmbeddedGraphicsHost {
    pub load_sprite: fn(&[u8], i32, i32) -> i32,
    pub release_sprite: fn(i32),
    pub load_font: fn(&[u8], i32) -> i32,
    pub measure_text: fn(i32, &[u8]) -> f32,
    pub cache_text: fn(i32, &[u8]) -> i32,
    pub measure_text_cached: fn(i32) -> f32,
    pub measure_text_cached_height: fn(i32) -> f32,
    pub poll_reload: fn(i32) -> i32,
}

thread_local! {
    static EMBEDDED_GRAPHICS_HOST: std::cell::Cell<Option<EmbeddedGraphicsHost>> =
        const { std::cell::Cell::new(None) };
}

pub fn set_embedded_graphics_host(host: Option<EmbeddedGraphicsHost>) {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.set(host));
}

pub fn embedded_graphics_host_is_set() -> bool {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.get().is_some())
}

fn embedded_graphics_host() -> Option<EmbeddedGraphicsHost> {
    EMBEDDED_GRAPHICS_HOST.with(|slot| slot.get())
}

fn jit_text_arg_to_cstring(value_id: i32) -> Result<CString, String> {
    let bytes = jit_text_arg_bytes(value_id).ok_or_else(|| {
        format!("missing jit text handle for id={value_id} (neither literal nor utf8 buffer)")
    })?;
    CString::new(bytes)
        .map_err(|_| "string contains interior NUL byte; cannot pass to C runtime".to_string())
}

// JIT string->C bridge for startup/asset extern calls.
// The language-level `string` currently lowers to an i32 handle in the JIT path, so we must
// translate that into a stable `const char*` when calling the C runtime.
#[no_mangle]
pub extern "C" fn stasis_jit_gfx_load_sprite(path_id: i32, max_w: i32, max_h: i32) -> i32 {
    if let (Some(host), Some(path)) = (embedded_graphics_host(), jit_text_arg_bytes(path_id)) {
        return (host.load_sprite)(&path, max_w, max_h);
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        eprintln!("gfx_load_sprite failed: unknown string handle {path_id}");
        return 0;
    };
    let api = match stasis_graphics_assets_api() {
        Ok(api) => api,
        Err(error) => {
            eprintln!("gfx_load_sprite failed: {error}");
            return 0;
        }
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_load_sprite) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_load_sprite) };
    let handle = callback(path.as_ptr(), max_w, max_h);
    if handle == 0 {
        eprintln!("gfx_load_sprite failed for {}", path.to_string_lossy());
    }
    handle
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_release_sprite(handle: i32) {
    if let Some(host) = embedded_graphics_host() {
        (host.release_sprite)(handle);
        return;
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) =
        unsafe { std::mem::transmute(api.stasis_gfx_release_sprite) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) =
        unsafe { std::mem::transmute(api.stasis_gfx_release_sprite) };
    callback(handle);
}

fn configured_preference_storage_root() -> &'static Mutex<Option<PathBuf>> {
    static ROOT: OnceLock<Mutex<Option<PathBuf>>> = OnceLock::new();
    ROOT.get_or_init(|| Mutex::new(None))
}

#[cfg(not(windows))]
fn preference_storage_root() -> Option<PathBuf> {
    if let Some(root) = configured_preference_storage_root()
        .lock()
        .expect("preference storage root mutex poisoned")
        .clone()
    {
        return Some(root);
    }
    if let Some(root) = std::env::var_os("STASIS_STORAGE_DIR").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(root));
    }
    #[cfg(target_os = "macos")]
    {
        return std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join("Library/Application Support/StasisLang"));
    }
    #[cfg(not(target_os = "macos"))]
    {
        if let Some(root) = std::env::var_os("XDG_DATA_HOME").filter(|value| !value.is_empty()) {
            return Some(PathBuf::from(root).join("StasisLang"));
        }
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .map(|root| root.join(".local/share/StasisLang"))
    }
}

pub fn set_preference_storage_root(root: Option<PathBuf>) {
    *configured_preference_storage_root()
        .lock()
        .expect("preference storage root mutex poisoned") = root;
}

#[cfg(not(windows))]
fn preference_component_valid(value: &[u8]) -> bool {
    !value.is_empty()
        && value.len() <= 63
        && value
            .iter()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

#[cfg(not(windows))]
fn preference_i32_path(scope: &[u8], key: &[u8]) -> Option<PathBuf> {
    if !preference_component_valid(scope) || !preference_component_valid(key) {
        return None;
    }
    let scope = std::str::from_utf8(scope).ok()?;
    let key = std::str::from_utf8(key).ok()?;
    Some(
        preference_storage_root()?
            .join(scope)
            .join(format!("{key}.i32")),
    )
}

#[cfg(not(windows))]
fn portable_storage_load_i32(scope: &[u8], key: &[u8], fallback: i32) -> i32 {
    let Some(path) = preference_i32_path(scope, key) else {
        return fallback;
    };
    let Ok(bytes) = std::fs::read(path) else {
        return fallback;
    };
    if bytes.len() > 63 {
        return fallback;
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return fallback;
    };
    text.trim().parse::<i32>().unwrap_or(fallback)
}

#[cfg(not(windows))]
fn portable_storage_save_i32(scope: &[u8], key: &[u8], value: i32) -> bool {
    let Some(path) = preference_i32_path(scope, key) else {
        return false;
    };
    let Some(directory) = path.parent() else {
        return false;
    };
    if std::fs::create_dir_all(directory).is_err() {
        return false;
    }
    let temporary = path.with_extension("i32.tmp");
    let result = (|| -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temporary)?;
        writeln!(file, "{value}")?;
        file.sync_all()?;
        std::fs::rename(&temporary, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(temporary);
        return false;
    }
    true
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_load_i32(scope_id: i32, key_id: i32, fallback: i32) -> i32 {
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (
            jit_text_arg_to_cstring(scope_id),
            jit_text_arg_to_cstring(key_id),
        ) else {
            return fallback;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return fallback;
        };
        let Some(address) = api.stasis_storage_load_i32 else {
            return fallback;
        };
        let callback: extern "system" fn(*const c_char, *const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(scope.as_ptr(), key.as_ptr(), fallback);
    }
    #[cfg(not(windows))]
    {
        let (Some(scope), Some(key)) = (jit_text_arg_bytes(scope_id), jit_text_arg_bytes(key_id))
        else {
            return fallback;
        };
        portable_storage_load_i32(&scope, &key, fallback)
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_storage_save_i32(scope_id: i32, key_id: i32, value: i32) -> i32 {
    #[cfg(windows)]
    {
        let (Ok(scope), Ok(key)) = (
            jit_text_arg_to_cstring(scope_id),
            jit_text_arg_to_cstring(key_id),
        ) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let Some(address) = api.stasis_storage_save_i32 else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char, *const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(scope.as_ptr(), key.as_ptr(), value);
    }
    #[cfg(not(windows))]
    {
        let (Some(scope), Some(key)) = (jit_text_arg_bytes(scope_id), jit_text_arg_bytes(key_id))
        else {
            return 0;
        };
        portable_storage_save_i32(&scope, &key, value) as i32
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_dump_bmp(path_id: i32) -> i32 {
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_dump_bmp) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_dump_bmp) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_dump_png(path_id: i32) -> i32 {
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    let Some(address) = api.stasis_gfx_dump_png else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char) -> i32 =
        unsafe { std::mem::transmute(address) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char) -> i32 = unsafe { std::mem::transmute(address) };
    callback(path.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_load_font(path_id: i32, size: i32) -> i32 {
    if let (Some(host), Some(path)) = (embedded_graphics_host(), jit_text_arg_bytes(path_id)) {
        return (host.load_font)(&path, size);
    }
    let Ok(path) = jit_text_arg_to_cstring(path_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_load_font) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(*const c_char, i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_load_font) };
    callback(path.as_ptr(), size)
}

#[no_mangle]
pub extern "C" fn stasis_jit_measure_text(font: i32, text_id: i32) -> f32 {
    if let Some(host) = embedded_graphics_host() {
        if let Some(width) =
            with_jit_text_arg_bytes(text_id, |text| (host.measure_text)(font, text))
        {
            return width;
        }
    }
    let Ok(text) = jit_text_arg_to_cstring(text_id) else {
        return 0.0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, *const c_char) -> f32 =
        unsafe { std::mem::transmute(api.stasis_measure_text) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, *const c_char) -> f32 =
        unsafe { std::mem::transmute(api.stasis_measure_text) };
    callback(font, text.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_cache_text(font: i32, text_id: i32) -> i32 {
    if let (Some(host), Some(text)) = (embedded_graphics_host(), jit_text_arg_bytes(text_id)) {
        return (host.cache_text)(font, &text);
    }
    let Ok(text) = jit_text_arg_to_cstring(text_id) else {
        return 0;
    };
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_cache_text) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32, *const c_char) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_cache_text) };
    callback(font, text.as_ptr())
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_poll_reload(handle: i32) -> i32 {
    if let Some(host) = embedded_graphics_host() {
        return (host.poll_reload)(handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_poll_reload) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> i32 =
        unsafe { std::mem::transmute(api.stasis_gfx_poll_reload) };
    callback(handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_measure_text_cached(run_handle: i32) -> f32 {
    if let Some(host) = embedded_graphics_host() {
        return (host.measure_text_cached)(run_handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached) };
    callback(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_measure_text_cached_height(run_handle: i32) -> f32 {
    if let Some(host) = embedded_graphics_host() {
        return (host.measure_text_cached_height)(run_handle);
    }
    let Ok(api) = stasis_graphics_assets_api() else {
        return 0.0;
    };
    #[cfg(windows)]
    let callback: extern "system" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached_height) };
    #[cfg(not(windows))]
    let callback: extern "C" fn(i32) -> f32 =
        unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached_height) };
    callback(run_handle)
}

fn struct_field_path_hash(base_hash: i32, suffix: &str) -> i32 {
    let mut hash = base_hash as u32;
    hash ^= u32::from(b'.');
    hash = hash.wrapping_mul(16_777_619);
    for byte in suffix.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    hash as i32
}

fn struct_view_i32_load(base: i32, index: i32, suffix: &str) -> i32 {
    if index < 0 {
        stasis_jit_global_i32_load(struct_field_path_hash(base, suffix))
    } else {
        stasis_jit_global_i32_array_load(base, global_path_hash(suffix), index)
    }
}

fn struct_view_i32_store(base: i32, index: i32, len: i32, suffix: &str, value: i32) {
    if index < 0 {
        stasis_jit_global_i32_store(struct_field_path_hash(base, suffix), value);
    } else if index < len {
        stasis_jit_global_i32_array_store(base, global_path_hash(suffix), index, value);
    }
}

fn struct_view_f32_store(base: i32, index: i32, len: i32, suffix: &str, value: f32) {
    if index < 0 {
        stasis_jit_global_f32_store(struct_field_path_hash(base, suffix), value);
    } else if index < len {
        stasis_jit_global_f32_array_store(base, global_path_hash(suffix), index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sprite_load_from(
    base: i32,
    index: i32,
    len: i32,
    path_id: i32,
    width: i32,
    height: i32,
) -> i32 {
    if width <= 0 || height <= 0 || (index >= 0 && index >= len) {
        return 0;
    }
    let loaded_handle = stasis_jit_gfx_load_sprite(path_id, width, height);
    if loaded_handle == 0 {
        return 0;
    }
    let old_handle = struct_view_i32_load(base, index, "handle");
    struct_view_i32_store(base, index, len, "handle", loaded_handle);
    struct_view_i32_store(base, index, len, "width", width);
    struct_view_i32_store(base, index, len, "height", height);
    if old_handle != 0 && old_handle != loaded_handle {
        stasis_jit_gfx_release_sprite(old_handle);
    }
    1
}

#[no_mangle]
pub extern "C" fn stasis_jit_text_run_load_from(
    base: i32,
    index: i32,
    len: i32,
    font: i32,
    text_id: i32,
) -> i32 {
    if font <= 0 || (index >= 0 && index >= len) {
        return 0;
    }
    let loaded_handle = stasis_jit_gfx_cache_text(font, text_id);
    if loaded_handle <= 0 {
        return 0;
    }
    struct_view_i32_store(base, index, len, "font", font);
    struct_view_i32_store(base, index, len, "handle", loaded_handle);
    struct_view_f32_store(
        base,
        index,
        len,
        "width",
        stasis_jit_gfx_measure_text_cached(loaded_handle),
    );
    struct_view_f32_store(
        base,
        index,
        len,
        "height",
        stasis_jit_gfx_measure_text_cached_height(loaded_handle),
    );
    1
}

// AOT engine bundles may be linked and executed headlessly; keep this as a no-op so tests don't
// block on sleeps during deterministic quality-gate runs.
#[no_mangle]
pub extern "C" fn stasis_jit_sleep_ms(ms: i32) {
    let _ = ms;
}

// Runtime-compatible time APIs used by `extern function time()`/`time_us()` expansion.
#[no_mangle]
pub extern "C" fn stasis_get_time_ms() -> i32 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_millis() as i32,
        Err(_) => 0,
    }
}

#[no_mangle]
pub extern "C" fn stasis_get_time_us() -> i32 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(duration) => duration.as_micros() as i32,
        Err(_) => 0,
    }
}

// Some stdlib externs use explicit `stasis_gfx_*` symbol names. Provide aliases so AOT bundles can
// link against the same shim layer used by the JIT runner.
#[no_mangle]
pub extern "C" fn stasis_gfx_cache_text(font: i32, text_id: i32) -> i32 {
    stasis_jit_gfx_cache_text(font, text_id)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_measure_text_cached(run_handle: i32) -> f32 {
    stasis_jit_gfx_measure_text_cached(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_measure_text_cached_height(run_handle: i32) -> f32 {
    stasis_jit_gfx_measure_text_cached_height(run_handle)
}

#[no_mangle]
pub extern "C" fn stasis_gfx_poll_reload(handle: i32) -> i32 {
    stasis_jit_gfx_poll_reload(handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_sin_fast(value: f32) -> f32 {
    value.sin()
}

#[no_mangle]
pub extern "C" fn stasis_jit_cos_fast(value: f32) -> f32 {
    value.cos()
}

fn jit_dispatch_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_load(path_hash: i32) -> i32 {
    {
        let table = registered_i32_ptrs();
        let guard = table
            .lock()
            .expect("registered i32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut i32) };
        }
    }
    let table = jit_i32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_store(path_hash: i32, value: i32) {
    {
        let table = registered_i32_ptrs();
        let guard = table
            .lock()
            .expect("registered i32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut i32) = value };
            return;
        }
    }
    let table = jit_i32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

fn fnv1a_extend_u32(mut hash: u32, suffix: &[u8]) -> u32 {
    for byte in suffix {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(16777619);
    }
    hash
}

fn jit_i32_slot_is_registered(path_hash: i32) -> bool {
    if registered_i32_ptrs()
        .lock()
        .expect("registered i32 ptr table mutex poisoned")
        .contains_key(&path_hash)
    {
        return true;
    }
    jit_i32_global_table()
        .lock()
        .expect("jit global table mutex poisoned")
        .contains_key(&path_hash)
}

fn jit_collection_runtime_metadata_is_registered(collection_hash: i32) -> bool {
    let max_length_hash = fnv1a_extend_u32(collection_hash as u32, b".max_length") as i32;
    jit_i32_slot_is_registered(max_length_hash)
}

fn stasis_meta_suffix_bytes(meta_kind: i32) -> Option<&'static [u8]> {
    // NOTE: These are intentionally hardcoded so Stasis can access collection header-like
    // fields (length/max_length/char_length) via a simple i32 handle (the base path hash)
    // without exposing negative indexing in Stasis source.
    match meta_kind {
        1 => Some(b".length"),
        2 => Some(b".max_length"),
        3 => Some(b".char_length"),
        _ => None,
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_collection_i32_load(collection_hash: i32, meta_kind: i32) -> i32 {
    let Some(suffix) = stasis_meta_suffix_bytes(meta_kind) else {
        return 0;
    };
    let derived = fnv1a_extend_u32(collection_hash as u32, suffix) as i32;
    if jit_i32_slot_is_registered(derived) {
        return stasis_jit_global_i32_load(derived);
    }
    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    let Some(text) = guard.get(&collection_hash) else {
        return 0;
    };
    match meta_kind {
        1 | 2 => i32::try_from(text.len()).unwrap_or(i32::MAX),
        3 => i32::try_from(text.chars().count()).unwrap_or(i32::MAX),
        _ => 0,
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_collection_i32_store(
    collection_hash: i32,
    meta_kind: i32,
    value: i32,
) {
    let Some(suffix) = stasis_meta_suffix_bytes(meta_kind) else {
        return;
    };
    let derived = fnv1a_extend_u32(collection_hash as u32, suffix) as i32;
    stasis_jit_global_i32_store(derived, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_load(path_hash: i32) -> f32 {
    {
        let table = registered_f32_ptrs();
        let guard = table
            .lock()
            .expect("registered f32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut f32) };
        }
    }
    let table = jit_f32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_store(path_hash: i32, value: f32) {
    {
        let table = registered_f32_ptrs();
        let guard = table
            .lock()
            .expect("registered f32 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut f32) = value };
            return;
        }
    }
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_load(path_hash: i32) -> f64 {
    {
        let table = registered_f64_ptrs();
        let guard = table
            .lock()
            .expect("registered f64 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            return unsafe { *(ptr as *mut f64) };
        }
    }
    let table = jit_f64_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_store(path_hash: i32, value: f64) {
    {
        let table = registered_f64_ptrs();
        let guard = table
            .lock()
            .expect("registered f64 ptr table mutex poisoned");
        if let Some(ptr) = guard.get(&path_hash).copied() {
            // Safety: caller owns lifetime; this is a process-global registration.
            unsafe { *(ptr as *mut f64) = value };
            return;
        }
    }
    let table = jit_f64_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> i32 {
    if index < 0 {
        return 0;
    }

    let idx = index as usize;
    {
        let table = registered_i32_arrays();
        let guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut i32).add(idx)) };
            }
            return 0;
        }
    }
    {
        let table = registered_u8_arrays();
        let guard = table
            .lock()
            .expect("registered u8 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return i32::from(unsafe { *((ptr as *mut u8).add(idx)) });
            }
            return 0;
        }
    }
    {
        let table = registered_u16_arrays();
        let guard = table
            .lock()
            .expect("registered u16 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                return i32::from(unsafe { *((ptr as *mut u16).add(idx)) });
            }
            return 0;
        }
    }
    let table = jit_i32_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_i32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: i32,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_i32_arrays();
        let guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut i32).add(idx)) = value };
            }
            return;
        }
    }
    {
        let table = registered_u8_arrays();
        let guard = table
            .lock()
            .expect("registered u8 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut u8).add(idx)) = value as u8 };
            }
            return;
        }
    }
    {
        let table = registered_u16_arrays();
        let guard = table
            .lock()
            .expect("registered u16 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                unsafe { *((ptr as *mut u16).add(idx)) = value as u16 };
            }
            return;
        }
    }
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> f32 {
    if index < 0 {
        return 0.0;
    }

    let idx = index as usize;
    {
        let table = registered_f32_arrays();
        let guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut f32).add(idx)) };
            }
            return 0.0;
        }
    }
    let table = jit_f32_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: f32,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_f32_arrays();
        let guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut f32).add(idx)) = value };
            }
            return;
        }
    }
    let table = jit_f32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> f64 {
    if index < 0 {
        return 0.0;
    }

    let idx = index as usize;
    {
        let table = registered_f64_arrays();
        let guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                return unsafe { *((ptr as *mut f64).add(idx)) };
            }
            return 0.0;
        }
    }
    let table = jit_f64_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_global_f64_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: f64,
) {
    if index < 0 {
        return;
    }

    let idx = index as usize;
    {
        let table = registered_f64_arrays();
        let guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        if let Some((ptr, len)) = guard.get(&(collection_hash, field_hash)).copied() {
            if idx < len {
                // Safety: caller owns lifetime; this is a process-global registration.
                unsafe { *((ptr as *mut f64).add(idx)) = value };
            }
            return;
        }
    }
    let table = jit_f64_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

pub fn clear_jit_i32_global_table() {
    let table = jit_i32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_i32_scalars()
        .lock()
        .expect("owned i32 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0;
    }
}

pub fn clear_jit_f32_global_table() {
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_f32_scalars()
        .lock()
        .expect("owned f32 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0.0;
    }
}

pub fn clear_jit_f64_global_table() {
    let table = jit_f64_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for value in owned_f64_scalars()
        .lock()
        .expect("owned f64 scalar table mutex poisoned")
        .values_mut()
    {
        **value = 0.0;
    }
}

pub fn clear_jit_i32_array_global_table() {
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_i32_arrays()
        .lock()
        .expect("owned i32 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0);
    }
}

pub fn clear_jit_f32_array_global_table() {
    let table = jit_f32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_f32_arrays()
        .lock()
        .expect("owned f32 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0.0);
    }
}

pub fn clear_jit_f64_array_global_table() {
    let table = jit_f64_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
    for values in owned_f64_arrays()
        .lock()
        .expect("owned f64 array table mutex poisoned")
        .values_mut()
    {
        values.fill(0.0);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let literal_bytes = if jit_text_buffer_is_registered(src) {
        None
    } else {
        jit_string_literal_table()
            .lock()
            .expect("jit string literal table mutex poisoned")
            .get(&src)
            .map(|text| text.as_bytes().to_vec())
    };
    if let Some(bytes) = literal_bytes {
        for offset in 0..count {
            let source_index = src_index.saturating_add(offset);
            let value = usize::try_from(source_index)
                .ok()
                .and_then(|index| bytes.get(index))
                .copied()
                .unwrap_or_default();
            let index = dst_index.saturating_add(offset);
            stasis_jit_global_i32_array_store(dst, 0, index, i32::from(value));
        }
        return;
    }
    let mut values: Vec<i32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_i32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_i32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<i32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_i32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_i32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memcpy_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<f32> = Vec::with_capacity(count as usize);
    for offset in 0..count {
        let index = src_index.saturating_add(offset);
        values.push(stasis_jit_global_f32_array_load(src, 0, index));
    }
    for (offset, value) in values.into_iter().enumerate() {
        let index = dst_index.saturating_add(offset as i32);
        stasis_jit_global_f32_array_store(dst, 0, index, value);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_u8(dst, dst_index, src, src_index, count);
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_i32(dst, dst_index, src, src_index, count);
}

#[no_mangle]
pub extern "C" fn stasis_jit_sys_memmove_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    stasis_jit_sys_memcpy_f32(dst, dst_index, src, src_index, count);
}

// ============================================================
// Audio host API (JIT extern call bridge)
// ============================================================

// Current dev runner doesn't model audio as a command buffer yet.
// Brickout uses `audio_is_available()` as a gate; return false so the game runs without calling
// pointer-typed audio externs (e.g. `audio_push_f32_interleaved`).
#[no_mangle]
pub extern "C" fn stasis_jit_audio_init(
    _sample_rate: i32,
    _channels: i32,
    _target_latency_frames: i32,
) -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_shutdown() {}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_is_available() -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_sample_rate() -> i32 {
    // Sensible default for callers that don't guard on `audio_is_available`.
    48_000
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_channels() -> i32 {
    2
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_queued_frames() -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_get_underruns() -> i32 {
    0
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_push_f32_interleaved(_samples: i32, _frame_count: i32) -> i32 {
    0
}

type JitI32GlobalMap = std::collections::HashMap<i32, i32>;
type JitF32GlobalMap = std::collections::HashMap<i32, f32>;
type JitF64GlobalMap = std::collections::HashMap<i32, f64>;
type JitI32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), i32>;
type JitF32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f32>;
type JitF64ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f64>;
type JitStringLiteralMap = std::collections::HashMap<i32, String>;

#[derive(Default)]
struct JitStringLiteralStage {
    literals: JitStringLiteralMap,
    collision: Option<String>,
}

thread_local! {
    static JIT_STRING_LITERAL_STAGE: RefCell<Option<JitStringLiteralStage>> = const { RefCell::new(None) };
}

fn jit_i32_global_table() -> &'static Mutex<JitI32GlobalMap> {
    static TABLE: OnceLock<Mutex<JitI32GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_global_table() -> &'static Mutex<JitF32GlobalMap> {
    static TABLE: OnceLock<Mutex<JitF32GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f64_global_table() -> &'static Mutex<JitF64GlobalMap> {
    static TABLE: OnceLock<Mutex<JitF64GlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_i32_array_global_table() -> &'static Mutex<JitI32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitI32ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_array_global_table() -> &'static Mutex<JitF32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitF32ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f64_array_global_table() -> &'static Mutex<JitF64ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitF64ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_string_literal_table() -> &'static Mutex<JitStringLiteralMap> {
    static TABLE: OnceLock<Mutex<JitStringLiteralMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

#[cfg(windows)]
fn os_str_to_wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryW(path: *const u16) -> *mut c_void;
    fn FreeLibrary(handle: *mut c_void) -> i32;
    fn GetProcAddress(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
const RTLD_LOCAL: i32 = 0;
#[cfg(any(target_os = "macos", target_os = "ios"))]
const RTLD_LOCAL: i32 = 4;
#[cfg(unix)]
const RTLD_NOW: i32 = 2;

#[cfg(all(unix, not(any(target_os = "macos", target_os = "ios"))))]
#[link(name = "dl")]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
unsafe extern "C" {
    fn dlopen(path: *const c_char, flags: i32) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    fn dlclose(handle: *mut c_void) -> i32;
    fn dlerror() -> *const c_char;
}

#[cfg(unix)]
fn clear_dynamic_loading_error() {
    let _ = unsafe { dlerror() };
}

#[cfg(unix)]
fn dynamic_loading_error_if_present() -> Option<String> {
    let error = unsafe { dlerror() };
    (!error.is_null()).then(|| {
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    })
}

#[cfg(unix)]
fn dynamic_loading_error() -> String {
    dynamic_loading_error_if_present().unwrap_or_else(|| "unknown dynamic loader error".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn test_lock() -> MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .expect("dynload test lock mutex poisoned")
    }

    #[test]
    fn bundled_graphics_runtime_outranks_environment_overrides() {
        let executable = std::env::current_exe().expect("current test executable");
        let expected = executable
            .parent()
            .expect("test executable directory")
            .join(runtime_library_file_names()[0]);
        assert_eq!(runtime_library_candidate_paths().first(), Some(&expected));
    }

    #[cfg(not(windows))]
    #[test]
    fn portable_preferences_persist_and_fail_closed() {
        let _guard = test_lock();
        let root = std::env::temp_dir().join(format!(
            "stasis_dynload_preferences_{}_{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let _ = std::fs::remove_dir_all(&root);
        set_preference_storage_root(Some(root.clone()));

        assert_eq!(portable_storage_load_i32(b"game", b"tier", 1), 1);
        assert!(portable_storage_save_i32(b"game", b"tier", 4));
        assert_eq!(portable_storage_load_i32(b"game", b"tier", 1), 4);
        assert!(!portable_storage_save_i32(b"../game", b"tier", 5));
        clear_jit_string_literal_table();
        upsert_jit_string_literal(700, "game");
        upsert_jit_string_literal(701, "tier");
        assert_eq!(stasis_jit_storage_save_i32(700, 701, 6), 1);
        assert_eq!(stasis_jit_storage_load_i32(700, 701, 1), 6);
        std::fs::write(root.join("game/tier.i32"), "surprise\n").expect("write corrupt value");
        assert_eq!(portable_storage_load_i32(b"game", b"tier", 2), 2);

        clear_jit_string_literal_table();
        set_preference_storage_root(None);
        std::fs::remove_dir_all(root).expect("remove preference fixture");
    }

    #[cfg(windows)]
    #[test]
    fn can_load_kernel32_and_resolve_export() {
        let library = Library::load(Path::new("kernel32.dll")).expect("load kernel32");
        let address = library
            .symbol_address("GetTickCount")
            .expect("resolve GetTickCount");
        assert_ne!(address, 0);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn can_load_linux_libc_and_resolve_export() {
        let library = Library::load(Path::new("libc.so.6")).expect("load libc");
        let address = library.symbol_address("malloc").expect("resolve malloc");
        assert_ne!(address, 0);
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn can_load_macos_libsystem_and_resolve_export() {
        let library =
            Library::load(Path::new("/usr/lib/libSystem.B.dylib")).expect("load libSystem");
        let address = library.symbol_address("malloc").expect("resolve malloc");
        assert_ne!(address, 0);
    }

    #[cfg(windows)]
    #[test]
    fn can_invoke_get_tick_count_export() {
        let library = Library::load(Path::new("kernel32.dll")).expect("load kernel32");
        let address = library
            .symbol_address("GetTickCount")
            .expect("resolve GetTickCount");
        let value = invoke_noarg_u64(address).expect("invoke GetTickCount");
        assert!(value <= u64::from(u32::MAX));
    }

    #[test]
    fn jit_debugger_blocks_on_breakpoints_and_preserves_real_nested_frames() {
        let _lock = test_lock();
        disable_jit_debugger();
        enable_jit_debugger([(2, 20)]);
        let worker = std::thread::spawn(|| {
            stasis_jit_debug_frame_enter(1);
            stasis_jit_debug_value_i64(0, 1, 7);
            stasis_jit_debug_statement(1, 10);
            stasis_jit_debug_frame_enter(2);
            stasis_jit_debug_value_f64(0, 2, 1.5);
            stasis_jit_debug_statement(2, 20);
            stasis_jit_debug_frame_enter(3);
            stasis_jit_debug_statement(3, 30);
            stasis_jit_debug_frame_leave(3);
            stasis_jit_debug_statement(2, 21);
            stasis_jit_debug_frame_leave(2);
            stasis_jit_debug_statement(1, 11);
            stasis_jit_debug_frame_leave(1);
        });

        let first = wait_for_jit_debug_stop(0, Duration::from_secs(2)).expect("breakpoint stop");
        assert_eq!((first.function_id, first.site_id), (2, 20));
        assert_eq!(first.frames.len(), 2);
        assert_eq!(
            first.frames[0].values.get(&0),
            Some(&JitDebugValue::I64 {
                type_tag: 1,
                value: 7
            })
        );
        assert_eq!(
            first.frames[1].values.get(&0),
            Some(&JitDebugValue::F64 {
                type_tag: 2,
                value: 1.5
            })
        );

        resume_jit_debugger(JitDebugResume::StepIn).expect("step in");
        let second =
            wait_for_jit_debug_stop(first.sequence, Duration::from_secs(2)).expect("step-in stop");
        assert_eq!((second.function_id, second.site_id), (3, 30));
        assert_eq!(second.frames.len(), 3);

        resume_jit_debugger(JitDebugResume::StepOut).expect("step out");
        let third = wait_for_jit_debug_stop(second.sequence, Duration::from_secs(2))
            .expect("step-out stop");
        assert_eq!((third.function_id, third.site_id), (2, 21));
        assert_eq!(third.frames.len(), 2);

        resume_jit_debugger(JitDebugResume::StepOut).expect("step out to caller");
        let fourth = wait_for_jit_debug_stop(third.sequence, Duration::from_secs(2))
            .expect("caller step-out stop");
        assert_eq!((fourth.function_id, fourth.site_id), (1, 11));
        assert_eq!(fourth.frames.len(), 1);

        resume_jit_debugger(JitDebugResume::Continue).expect("continue");
        worker.join().expect("debug worker");
        disable_jit_debugger();
        assert!(jit_debug_stop().is_none());
    }

    #[test]
    fn i32_array_ptr_migrates_fallback_values_and_supports_direct_access() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_array_global_table();

        let collection_hash = 0x1357_2468i32;
        let field_hash = 0x2468_1357i32;
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 0, 11);
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 1, 22);
        stasis_jit_global_i32_array_store(collection_hash, field_hash, 2, 33);

        let ptr = stasis_jit_global_i32_array_ptr(collection_hash, field_hash, 4);
        assert!(!ptr.is_null());

        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 0),
            11
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 1),
            22
        );
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 2),
            33
        );

        // Safety: pointer comes from dynload-owned registration table for this test process.
        unsafe {
            *ptr.add(3) = 44;
        }
        assert_eq!(
            stasis_jit_global_i32_array_load(collection_hash, field_hash, 3),
            44
        );

        let ptr2 = stasis_jit_global_i32_array_ptr(collection_hash, field_hash, 4);
        assert_eq!(ptr, ptr2);
    }

    #[test]
    fn render_trace_rejects_non_contract_lengths_before_allocating() {
        let _lock = test_lock();
        clear_registered_global_memory();

        // Safety: invalid lengths must be rejected before any pointer is resolved.
        let trace = unsafe {
            stasis_jit_render_v2_trace(
                101,
                i32::MAX,
                102,
                STASIS_RENDER_F32_COUNT as i32,
                103,
                STASIS_RENDER_U8_COUNT as i32,
            )
        };

        assert_eq!(trace, 0);
        assert!(registered_i32_arrays()
            .lock()
            .expect("registered i32 array table mutex poisoned")
            .is_empty());
        assert!(registered_f32_arrays()
            .lock()
            .expect("registered f32 array table mutex poisoned")
            .is_empty());
        assert!(registered_u8_arrays()
            .lock()
            .expect("registered u8 array table mutex poisoned")
            .is_empty());
    }

    #[test]
    fn runtime_state_snapshot_restores_owned_memory_without_reading_borrowed_memory() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_f32_global_table();
        clear_jit_f64_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_f32_array_global_table();
        clear_jit_f64_array_global_table();

        register_global_i32_ptr(1, std::ptr::null_mut());
        register_global_u8_array(2, 0, std::ptr::null_mut(), 4);
        let mut scalar = 7i32;
        let mut bytes = vec![1u8, 2, 3, 4];
        register_global_i32_ptr(10, &mut scalar);
        register_global_u8_array(20, 0, bytes.as_mut_ptr(), bytes.len());
        register_global_i32_array(21, 0, 1usize as *mut i32, 4);
        stasis_jit_global_i32_store(30, 9);
        stasis_jit_global_f32_array_store(40, 2, 0, 1.5);
        assert!(snapshot_jit_runtime_state_bounded(1)
            .expect_err("snapshot should be bounded")
            .contains("limit"));
        let snapshot = snapshot_jit_runtime_state();

        scalar = 70;
        bytes.copy_from_slice(&[9, 9, 9, 9]);
        stasis_jit_global_i32_store(30, 90);
        stasis_jit_global_i32_store(31, 100);
        stasis_jit_global_f32_array_store(40, 2, 0, 8.5);
        restore_jit_runtime_state(&snapshot);

        assert_eq!(scalar, 70);
        assert_eq!(bytes, vec![9, 9, 9, 9]);
        assert_eq!(stasis_jit_global_i32_load(30), 9);
        assert_eq!(stasis_jit_global_i32_load(31), 0);
        assert_eq!(stasis_jit_global_f32_array_load(40, 2, 0), 1.5);
    }

    #[test]
    fn runtime_state_snapshot_does_not_reclaim_scalar_replaced_by_host_memory() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 76;
        ensure_owned_i32_scalar(key).expect("provision owned scalar");
        stasis_jit_global_i32_store(key, 7);

        let mut borrowed = 11;
        register_global_i32_ptr(key, &mut borrowed);
        let snapshot = snapshot_jit_runtime_state();
        borrowed = 13;
        restore_jit_runtime_state(&snapshot);

        assert_eq!(borrowed, 13);
        assert_eq!(stasis_jit_global_i32_load(key), 13);
        clear_registered_global_memory();
    }

    #[test]
    fn runtime_state_rollback_leaves_borrowed_storage_descriptor_unchanged() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 77;
        let mut original = [3, 5];
        register_global_i32_array(key, 0, original.as_mut_ptr(), original.len());
        let slot_address = direct_array_storage_slot_address(JitStorageKind::I32, key, 0)
            .expect("reserve direct slot");
        let snapshot = snapshot_jit_runtime_state();

        let mut replacement = [9, 8, 7, 6];
        register_global_i32_array(key, 0, replacement.as_mut_ptr(), replacement.len());
        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, replacement.as_ptr() as usize);
        assert_eq!(slot.len, replacement.len());

        restore_jit_runtime_state(&snapshot);
        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, replacement.as_ptr() as usize);
        assert_eq!(slot.len, replacement.len());
        assert_eq!(unsafe { *(slot.data as *const i32).add(1) }, 8);
        clear_registered_global_memory();
    }

    #[test]
    fn direct_array_growth_is_rejected_during_guest_execution() {
        let _lock = test_lock();
        clear_registered_global_memory();
        let key = 78;
        let mut values = [3, 5];
        register_global_i32_array(key, 0, values.as_mut_ptr(), values.len());
        let slot_address = direct_array_storage_slot_address(JitStorageKind::I32, key, 0)
            .expect("reserve direct slot");

        {
            let _execution = JitExecutionGuard::enter();
            let error = ensure_jit_i32_array_capacity(key, 0, 4)
                .expect_err("growth must not rebind direct storage during guest execution");
            assert!(error.contains("between guest execution windows"));
            assert!(stasis_jit_global_i32_array_ptr(key, 0, 4).is_null());
        }

        let slot = unsafe { &*(slot_address as *const JitStorageSlot) };
        assert_eq!(slot.data, values.as_ptr() as usize);
        assert_eq!(slot.len, values.len());
        assert_eq!(values, [3, 5]);
        clear_registered_global_memory();
    }

    #[test]
    fn jit_text_arg_bytes_reads_string_literals() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        upsert_jit_string_literal(1234, "hello");

        assert_eq!(jit_text_arg_bytes(1234), Some(b"hello".to_vec()));
        assert_eq!(stasis_jit_collection_i32_load(1234, 1), 5);
        assert_eq!(stasis_jit_collection_i32_load(1234, 2), 5);
        assert_eq!(stasis_jit_collection_i32_load(1234, 3), 5);
    }

    #[test]
    fn jit_memcpy_u8_copies_string_literal_bytes() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let source = 0x1234_5678i32;
        let destination = 0x2345_6789i32;
        let mut bytes = [0u8; 8];
        upsert_jit_string_literal(source, "field");
        register_global_u8_array(destination, 0, bytes.as_mut_ptr(), bytes.len());

        stasis_jit_sys_memcpy_u8(destination, 1, source, 1, 3);

        assert_eq!(&bytes, b"\0iel\0\0\0\0");
        clear_registered_global_memory();
        clear_jit_string_literal_table();
    }

    #[test]
    fn jit_memcpy_u8_preserves_raw_array_copying() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_string_literal_table();

        let source = 0x3456_789ai32;
        let destination = 0x4567_89abi32;
        let mut source_bytes = *b"raw-data";
        let mut destination_bytes = [0u8; 8];
        register_global_u8_array(source, 0, source_bytes.as_mut_ptr(), source_bytes.len());
        register_global_u8_array(
            destination,
            0,
            destination_bytes.as_mut_ptr(),
            destination_bytes.len(),
        );

        stasis_jit_sys_memcpy_u8(destination, 0, source, 0, 8);

        assert_eq!(&destination_bytes, b"raw-data");
        clear_registered_global_memory();
    }

    #[test]
    fn staged_string_literals_do_not_mutate_live_table_before_publish() {
        let _lock = test_lock();
        clear_jit_string_literal_table();
        upsert_jit_string_literal(1, "live");

        begin_jit_string_literal_staging().expect("begin staging");
        upsert_jit_string_literal(2, "candidate");
        assert_eq!(jit_string_literal_value(1).as_deref(), Some("live"));
        assert_eq!(jit_string_literal_value(2), None);
        let staged = finish_jit_string_literal_staging().expect("finish staging");

        replace_jit_string_literal_table(&staged);
        assert_eq!(jit_string_literal_value(1), None);
        assert_eq!(jit_string_literal_value(2).as_deref(), Some("candidate"));

        begin_jit_string_literal_staging().expect("begin collision staging");
        upsert_jit_string_literal(3, "first");
        upsert_jit_string_literal(3, "second");
        assert!(finish_jit_string_literal_staging()
            .expect_err("hash collision")
            .contains("collision"));
        assert_eq!(jit_string_literal_value(3), None);
    }

    #[test]
    fn jit_text_arg_bytes_reads_utf8_global_buffers() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();

        let collection_hash = 0x4455_6677i32;
        stasis_jit_collection_i32_store(collection_hash, 1, 4);
        stasis_jit_collection_i32_store(collection_hash, 2, 8);
        stasis_jit_collection_i32_store(collection_hash, 3, 4);
        stasis_jit_global_i32_array_store(collection_hash, 0, 0, i32::from(b'p'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 1, i32::from(b'a'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 2, i32::from(b't'));
        stasis_jit_global_i32_array_store(collection_hash, 0, 3, i32::from(b'h'));

        assert_eq!(jit_text_arg_bytes(collection_hash), Some(b"path".to_vec()));
    }

    #[test]
    fn jit_text_arg_bytes_prefers_runtime_buffer_over_literal_hash_collision() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        let shared_id = 0x2244_6688i32;
        upsert_jit_string_literal(shared_id, "literal");
        stasis_jit_collection_i32_store(shared_id, 1, 6);
        stasis_jit_collection_i32_store(shared_id, 2, 8);
        stasis_jit_collection_i32_store(shared_id, 3, 6);
        stasis_jit_global_i32_array_store(shared_id, 0, 0, i32::from(b'b'));
        stasis_jit_global_i32_array_store(shared_id, 0, 1, i32::from(b'u'));
        stasis_jit_global_i32_array_store(shared_id, 0, 2, i32::from(b'f'));
        stasis_jit_global_i32_array_store(shared_id, 0, 3, i32::from(b'f'));
        stasis_jit_global_i32_array_store(shared_id, 0, 4, i32::from(b'e'));
        stasis_jit_global_i32_array_store(shared_id, 0, 5, i32::from(b'r'));

        assert_eq!(jit_text_arg_bytes(shared_id), Some(b"buffer".to_vec()));
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 1), 6);
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 2), 8);
        assert_eq!(stasis_jit_collection_i32_load(shared_id, 3), 6);
    }

    extern "C" fn host_entry_one() -> i32 {
        1
    }

    extern "C" fn host_entry_two() -> i32 {
        2
    }

    #[test]
    fn host_entry_trampolines_publish_one_immutable_target_table() {
        let _lock = test_lock();
        let tick_trampoline = jit_host_tick_trampoline_ptr();
        let render_trampoline = jit_host_render_trampoline_ptr();
        begin_jit_host_entry_session(JitHostEntryTargets {
            revision: 41,
            main: host_entry_one as usize,
            tick: host_entry_one as usize,
            render: host_entry_one as usize,
            on_code_swap: None,
        })
        .expect("publish first host-entry table");
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(1));
        assert_eq!(invoke_noarg_i32(render_trampoline), Ok(1));

        publish_jit_host_entry_targets(JitHostEntryTargets {
            revision: 42,
            main: host_entry_two as usize,
            tick: host_entry_two as usize,
            render: host_entry_two as usize,
            on_code_swap: None,
        })
        .expect("publish second host-entry table");
        assert_eq!(jit_host_tick_trampoline_ptr(), tick_trampoline);
        assert_eq!(jit_host_render_trampoline_ptr(), render_trampoline);
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(2));
        assert_eq!(invoke_noarg_i32(render_trampoline), Ok(2));
        assert_eq!(jit_host_entry_targets().unwrap().revision, 42);
        assert!(publish_jit_host_entry_targets(JitHostEntryTargets {
            revision: 41,
            main: host_entry_one as usize,
            tick: host_entry_one as usize,
            render: host_entry_one as usize,
            on_code_swap: None,
        })
        .expect_err("stale publication")
        .contains("stale JIT host-entry revision"));
        assert_eq!(invoke_noarg_i32(tick_trampoline), Ok(2));
    }
}
