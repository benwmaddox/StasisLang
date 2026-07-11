#![cfg_attr(not(debug_assertions), deny(warnings))]

use std::collections::HashMap;
use std::ffi::c_char;
use std::io::Write;
use std::path::Path;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use std::ffi::{c_void, CString, OsStr};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

pub struct Library {
    #[cfg(windows)]
    handle: *mut c_void,
}

// Library handles are process-wide OS resources and can be moved between threads.
unsafe impl Send for Library {}
// Loading a module and calling exports is thread-safe on Windows; the handle is immutable after load.
unsafe impl Sync for Library {}

impl Library {
    pub fn load(path: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let mut wide: Vec<u16> = os_str_to_wide(path.as_os_str());
            wide.push(0);
            let handle = unsafe { LoadLibraryW(wide.as_ptr()) };
            if handle.is_null() {
                return Err(format!("failed to load dynamic library {}", path.display()));
            }
            return Ok(Self { handle });
        }

        #[cfg(not(windows))]
        {
            let _ = path;
            Err("dynamic loading is only supported on windows in stasis_dynload".to_string())
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

        #[cfg(not(windows))]
        {
            let _ = symbol;
            Err(
                "dynamic symbol resolution is only supported on windows in stasis_dynload"
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
    }
}

pub fn invoke_noarg_u64(address: usize) -> Result<u64, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
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

pub fn invoke_i32_to_void(address: usize, arg0: i32) -> Result<(), String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
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

// ============================================================
// stasis_graphics host API (dev in-process runner)
// ============================================================

pub struct StasisGraphicsApi {
    _lib: Library,
    #[cfg(windows)]
    stasis_init_window: usize,
    #[cfg(windows)]
    stasis_host_get_frame: usize,
    #[cfg(windows)]
    stasis_host_bulk_apply_requests: usize,
    #[cfg(windows)]
    stasis_gfx_submit_u8: usize,
    #[cfg(windows)]
    stasis_sleep_ms: usize,
}

impl StasisGraphicsApi {
    pub fn load_default() -> Result<Self, String> {
        #[cfg(windows)]
        {
            for candidate in runtime_library_candidate_paths() {
                if !candidate.exists() {
                    continue;
                }
                if let Ok(api) = Self::load(&candidate) {
                    return Ok(api);
                }
            }
            Err("failed to load stasis_graphics runtime library (set STASIS_RUNTIME_DLL_PATH or build runtime)".to_string())
        }

        #[cfg(not(windows))]
        {
            Err(
                "stasis_graphics runtime loading is only supported on windows in stasis_dynload"
                    .to_string(),
            )
        }
    }

    pub fn load(path: &Path) -> Result<Self, String> {
        #[cfg(windows)]
        {
            let lib = Library::load(path)?;
            let stasis_init_window = lib.symbol_address("stasis_init_window")?;
            let stasis_host_get_frame = lib.symbol_address("stasis_host_get_frame")?;
            let stasis_host_bulk_apply_requests =
                lib.symbol_address("stasis_host_bulk_apply_requests")?;
            let stasis_gfx_submit_u8 = lib.symbol_address("stasis_gfx_submit_u8")?;
            let stasis_sleep_ms = lib.symbol_address("stasis_sleep_ms")?;
            Ok(Self {
                _lib: lib,
                stasis_init_window,
                stasis_host_get_frame,
                stasis_host_bulk_apply_requests,
                stasis_gfx_submit_u8,
                stasis_sleep_ms,
            })
        }

        #[cfg(not(windows))]
        {
            let _ = path;
            Err(
                "stasis_graphics runtime loading is only supported on windows in stasis_dynload"
                    .to_string(),
            )
        }
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
            let _ = width;
            let _ = height;
            let _ = title;
            Err(
                "stasis_graphics init_window is only supported on windows in stasis_dynload"
                    .to_string(),
            )
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
            let _ = out_i32;
            let _ = out_f32;
            Err(
                "stasis_graphics host_get_frame is only supported on windows in stasis_dynload"
                    .to_string(),
            )
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
            let _ = host_req_seq;
            let _ = host_req_flags;
            let _ = host_req_window_w_px;
            let _ = host_req_window_h_px;
            Err("stasis_graphics host_bulk_apply_requests is only supported on windows in stasis_dynload".to_string())
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
            let _ = cmd_i32;
            let _ = cmd_f32;
            let _ = cmd_u8;
            Err(
                "stasis_graphics gfx_submit_u8 is only supported on windows in stasis_dynload"
                    .to_string(),
            )
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
            let _ = ms;
            Err(
                "stasis_graphics sleep_ms is only supported on windows in stasis_dynload"
                    .to_string(),
            )
        }
    }
}

// ============================================================
// stasis_graphics asset API (JIT extern call bridge)
// ============================================================

#[cfg(windows)]
struct StasisGraphicsAssetsApi {
    _lib: Library,
    stasis_gfx_load_sprite: usize,
    stasis_gfx_dump_bmp: usize,
    stasis_gfx_poll_reload: usize,
    stasis_load_font: usize,
    stasis_measure_text: usize,
    stasis_gfx_cache_text: usize,
    stasis_gfx_measure_text_cached: usize,
    stasis_audio_load_music: usize,
    stasis_audio_load_effect: usize,
    stasis_audio_play_music: usize,
    stasis_audio_stop_music: usize,
    stasis_audio_play_effect: usize,
}

#[cfg(windows)]
impl StasisGraphicsAssetsApi {
    fn load_default() -> Result<Self, String> {
        for candidate in runtime_library_candidate_paths() {
            if !candidate.exists() {
                continue;
            }
            if let Ok(api) = Self::load(&candidate) {
                return Ok(api);
            }
        }
        Err("failed to load stasis_graphics runtime library for asset calls".to_string())
    }

    fn load(path: &Path) -> Result<Self, String> {
        let lib = Library::load(path)?;
        Ok(Self {
            stasis_gfx_load_sprite: lib.symbol_address("stasis_gfx_load_sprite")?,
            stasis_gfx_dump_bmp: lib.symbol_address("stasis_gfx_dump_bmp")?,
            stasis_gfx_poll_reload: lib.symbol_address("stasis_gfx_poll_reload")?,
            stasis_load_font: lib.symbol_address("stasis_load_font")?,
            stasis_measure_text: lib.symbol_address("stasis_measure_text")?,
            stasis_gfx_cache_text: lib.symbol_address("stasis_gfx_cache_text")?,
            stasis_gfx_measure_text_cached: lib.symbol_address("stasis_gfx_measure_text_cached")?,
            stasis_audio_load_music: lib.symbol_address("stasis_audio_load_music")?,
            stasis_audio_load_effect: lib.symbol_address("stasis_audio_load_effect")?,
            stasis_audio_play_music: lib.symbol_address("stasis_audio_play_music")?,
            stasis_audio_stop_music: lib.symbol_address("stasis_audio_stop_music")?,
            stasis_audio_play_effect: lib.symbol_address("stasis_audio_play_effect")?,
            _lib: lib,
        })
    }
}

#[cfg(windows)]
fn stasis_graphics_assets_api() -> Result<&'static StasisGraphicsAssetsApi, String> {
    static API: OnceLock<Result<StasisGraphicsAssetsApi, String>> = OnceLock::new();
    match API.get_or_init(StasisGraphicsAssetsApi::load_default) {
        Ok(api) => Ok(api),
        Err(error) => Err(error.clone()),
    }
}

pub fn runtime_library_candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(configured) = std::env::var_os("STASIS_RUNTIME_DLL_PATH") {
        out.push(PathBuf::from(configured));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            // Release-friendly default: ship the DLL next to the stasis binary.
            out.push(exe_dir.join("stasis_graphics.dll"));

            // Dev-friendly default: locate the runtime DLL built under the repo tree by
            // walking a few parents from the executable location.
            for ancestor in exe_dir.ancestors().take(6) {
                out.push(
                    ancestor
                        .join("runtime")
                        .join("build")
                        .join("bin")
                        .join("Release")
                        .join("stasis_graphics.dll"),
                );
                out.push(
                    ancestor
                        .join("runtime")
                        .join("build")
                        .join("bin")
                        .join("Debug")
                        .join("stasis_graphics.dll"),
                );
            }
        }
    }

    // Allow loading from the current working directory too (handy for ad-hoc runs).
    out.push(PathBuf::from("stasis_graphics.dll"));

    // Dev-friendly fallback: if the runtime DLL exists under this repo checkout, include it.
    // This helps when `CARGO_TARGET_DIR` points outside the workspace (so `current_exe()` ancestry
    // won't include the repo root).
    let repo_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
    let repo_release = repo_root
        .join("runtime")
        .join("build")
        .join("bin")
        .join("Release")
        .join("stasis_graphics.dll");
    if repo_release.exists() {
        out.push(repo_release);
    }
    let repo_debug = repo_root
        .join("runtime")
        .join("build")
        .join("bin")
        .join("Debug")
        .join("stasis_graphics.dll");
    if repo_debug.exists() {
        out.push(repo_debug);
    }
    out
}

pub fn replace_jit_i32_dispatch_table(entries: &[(u32, u8, usize)]) {
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let table = jit_i32_dispatch_table();
    let mut guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard.clear();
    for (fn_id, arity, code_ptr) in entries {
        guard.insert((*fn_id, *arity), *code_ptr);
    }
}

pub fn replace_jit_f32_dispatch_table(entries: &[(u32, u8, usize)]) {
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let table = jit_f32_dispatch_table();
    let mut guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard.clear();
    for (fn_id, arity, code_ptr) in entries {
        guard.insert((*fn_id, *arity), *code_ptr);
    }
}

pub fn replace_jit_code_ptr_table(entries: &[(u32, usize)]) {
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let table = jit_code_ptr_table();
    let mut guard = table.lock().expect("jit code ptr table mutex poisoned");
    guard.clear();
    for (fn_id, code_ptr) in entries {
        guard.insert(*fn_id, *code_ptr);
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_register_code_ptr(fn_id_raw: i32, code_ptr: i64) {
    if fn_id_raw < 0 || code_ptr == 0 {
        return;
    }
    let _dispatch_lock = jit_dispatch_lock()
        .lock()
        .expect("jit dispatch lock mutex poisoned");
    let table = jit_code_ptr_table();
    let mut guard = table.lock().expect("jit code ptr table mutex poisoned");
    guard.insert(fn_id_raw as u32, code_ptr as usize);
}

pub fn clear_jit_string_literal_table() {
    let table = jit_string_literal_table();
    let mut guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    guard.clear();
}

pub fn upsert_jit_string_literal(id: i32, value: &str) {
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

fn owned_f32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_i32_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<i32>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<i32>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn owned_f64_arrays() -> &'static Mutex<HashMap<ArrayKey, Vec<f64>>> {
    static TABLE: OnceLock<Mutex<HashMap<ArrayKey, Vec<f64>>>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn clear_registered_global_memory() {
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
}

pub fn register_global_i32_ptr(path_hash: i32, ptr: *mut i32) {
    let table = registered_i32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered i32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
}

pub fn register_global_f32_ptr(path_hash: i32, ptr: *mut f32) {
    let table = registered_f32_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f32 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
}

pub fn register_global_f64_ptr(path_hash: i32, ptr: *mut f64) {
    let table = registered_f64_ptrs();
    let mut guard = table
        .lock()
        .expect("registered f64 ptr table mutex poisoned");
    guard.insert(path_hash, ptr as usize);
}

pub fn register_global_i32_array(collection_hash: i32, field_hash: i32, ptr: *mut i32, len: usize) {
    let table = registered_i32_arrays();
    let mut guard = table
        .lock()
        .expect("registered i32 array table mutex poisoned");
    guard.insert((collection_hash, field_hash), (ptr as usize, len));
}

pub fn register_global_f32_array(collection_hash: i32, field_hash: i32, ptr: *mut f32, len: usize) {
    let table = registered_f32_arrays();
    let mut guard = table
        .lock()
        .expect("registered f32 array table mutex poisoned");
    guard.insert((collection_hash, field_hash), (ptr as usize, len));
}

pub fn register_global_f64_array(collection_hash: i32, field_hash: i32, ptr: *mut f64, len: usize) {
    let table = registered_f64_arrays();
    let mut guard = table
        .lock()
        .expect("registered f64 array table mutex poisoned");
    guard.insert((collection_hash, field_hash), (ptr as usize, len));
}

pub fn register_global_u8_array(collection_hash: i32, field_hash: i32, ptr: *mut u8, len: usize) {
    let table = registered_u8_arrays();
    let mut guard = table
        .lock()
        .expect("registered u8 array table mutex poisoned");
    guard.insert((collection_hash, field_hash), (ptr as usize, len));
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
pub extern "C" fn stasis_jit_global_i32_array_ptr(
    collection_hash: i32,
    field_hash: i32,
    len: i32,
) -> *mut i32 {
    if len <= 0 {
        return std::ptr::null_mut();
    }

    // Fast path: already registered (host-owned or previously allocated).
    {
        let table = registered_i32_arrays();
        let guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        if let Some((ptr, _)) = guard.get(&(collection_hash, field_hash)).copied() {
            return ptr as *mut i32;
        }
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);

    let ptr = {
        let mut owned_guard = owned_i32_arrays()
            .lock()
            .expect("owned i32 array table mutex poisoned");
        let array = owned_guard
            .entry(key)
            .or_insert_with(|| vec![0; requested_len]);
        if array.len() < requested_len {
            array.resize(requested_len, 0);
        }

        // Migrate any previously-stored values from the fallback hash map.
        {
            let table = jit_i32_array_global_table();
            let mut guard = table.lock().expect("jit global table mutex poisoned");
            for idx in 0..requested_len {
                let idx_i32 = idx as i32;
                if let Some(value) = guard.remove(&(collection_hash, field_hash, idx_i32)) {
                    array[idx] = value;
                }
            }
        }

        array.as_mut_ptr()
    };

    {
        let table = registered_i32_arrays();
        let mut guard = table
            .lock()
            .expect("registered i32 array table mutex poisoned");
        guard.insert(key, (ptr as usize, requested_len));
    }

    ptr
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

    // Fast path: already registered (host-owned or previously allocated).
    {
        let table = registered_f32_arrays();
        let guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        if let Some((ptr, _)) = guard.get(&(collection_hash, field_hash)).copied() {
            return ptr as *mut f32;
        }
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);

    // Allocate (or reuse) an owned backing array, then register it so subsequent helper calls
    // can skip the fallback hash map path.
    let ptr = {
        let mut owned_guard = owned_f32_arrays()
            .lock()
            .expect("owned f32 array table mutex poisoned");
        let array = owned_guard
            .entry(key)
            .or_insert_with(|| vec![0.0; requested_len]);
        if array.len() < requested_len {
            array.resize(requested_len, 0.0);
        }

        // Migrate any previously-stored values from the fallback hash map.
        {
            let table = jit_f32_array_global_table();
            let mut guard = table.lock().expect("jit global table mutex poisoned");
            for idx in 0..requested_len {
                let idx_i32 = idx as i32;
                if let Some(value) = guard.remove(&(collection_hash, field_hash, idx_i32)) {
                    array[idx] = value;
                }
            }
        }

        array.as_mut_ptr()
    };

    {
        let table = registered_f32_arrays();
        let mut guard = table
            .lock()
            .expect("registered f32 array table mutex poisoned");
        // Register with the requested len so helper loads/stores use the same bounds as foreach.
        guard.insert(key, (ptr as usize, requested_len));
    }

    ptr
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

    // Fast path: already registered (host-owned or previously allocated).
    {
        let table = registered_f64_arrays();
        let guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        if let Some((ptr, _)) = guard.get(&(collection_hash, field_hash)).copied() {
            return ptr as *mut f64;
        }
    }

    let requested_len = len as usize;
    let key = (collection_hash, field_hash);

    let ptr = {
        let mut owned_guard = owned_f64_arrays()
            .lock()
            .expect("owned f64 array table mutex poisoned");
        let array = owned_guard
            .entry(key)
            .or_insert_with(|| vec![0.0; requested_len]);
        if array.len() < requested_len {
            array.resize(requested_len, 0.0);
        }

        // Migrate any previously-stored values from the fallback hash map.
        {
            let table = jit_f64_array_global_table();
            let mut guard = table.lock().expect("jit global table mutex poisoned");
            for idx in 0..requested_len {
                let idx_i32 = idx as i32;
                if let Some(value) = guard.remove(&(collection_hash, field_hash, idx_i32)) {
                    array[idx] = value;
                }
            }
        }

        array.as_mut_ptr()
    };

    {
        let table = registered_f64_arrays();
        let mut guard = table
            .lock()
            .expect("registered f64 array table mutex poisoned");
        guard.insert(key, (ptr as usize, requested_len));
    }

    ptr
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_i32(value: i32) {
    print!("{value}");
    let _ = std::io::stdout().flush();
}

#[no_mangle]
pub extern "C" fn stasis_jit_print_string(value_id: i32) {
    if let Some(bytes) = jit_text_arg_bytes(value_id) {
        print!("{}", String::from_utf8_lossy(&bytes));
        let _ = std::io::stdout().flush();
        return;
    }

    let table = jit_string_literal_table();
    let guard = table
        .lock()
        .expect("jit string literal table mutex poisoned");
    if let Some(text) = guard.get(&value_id) {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

fn jit_text_buffer_is_registered(value_id: i32) -> bool {
    if stasis_jit_collection_i32_load(value_id, 2) > 0 {
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

#[cfg(windows)]
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
    #[cfg(windows)]
    {
        let Ok(path) = jit_text_arg_to_cstring(path_id) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char, i32, i32) -> i32 =
            unsafe { std::mem::transmute(api.stasis_gfx_load_sprite) };
        return callback(path.as_ptr(), max_w, max_h);
    }
    #[cfg(not(windows))]
    {
        let _ = path_id;
        let _ = max_w;
        let _ = max_h;
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_dump_bmp(path_id: i32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(path) = jit_text_arg_to_cstring(path_id) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char) -> i32 =
            unsafe { std::mem::transmute(api.stasis_gfx_dump_bmp) };
        return callback(path.as_ptr());
    }
    #[cfg(not(windows))]
    {
        let _ = path_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_load_font(path_id: i32, size: i32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(path) = jit_text_arg_to_cstring(path_id) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(*const c_char, i32) -> i32 =
            unsafe { std::mem::transmute(api.stasis_load_font) };
        return callback(path.as_ptr(), size);
    }
    #[cfg(not(windows))]
    {
        let _ = path_id;
        let _ = size;
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_measure_text(font: i32, text_id: i32) -> f32 {
    #[cfg(windows)]
    {
        let Ok(text) = jit_text_arg_to_cstring(text_id) else {
            return 0.0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0.0;
        };
        let callback: extern "system" fn(i32, *const c_char) -> f32 =
            unsafe { std::mem::transmute(api.stasis_measure_text) };
        return callback(font, text.as_ptr());
    }
    #[cfg(not(windows))]
    {
        let _ = font;
        let _ = text_id;
        0.0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_cache_text(font: i32, text_id: i32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(text) = jit_text_arg_to_cstring(text_id) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(i32, *const c_char) -> i32 =
            unsafe { std::mem::transmute(api.stasis_gfx_cache_text) };
        return callback(font, text.as_ptr());
    }
    #[cfg(not(windows))]
    {
        let _ = font;
        let _ = text_id;
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_poll_reload(handle: i32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(i32) -> i32 =
            unsafe { std::mem::transmute(api.stasis_gfx_poll_reload) };
        return callback(handle);
    }
    #[cfg(not(windows))]
    {
        let _ = handle;
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_gfx_measure_text_cached(run_handle: i32) -> f32 {
    #[cfg(windows)]
    {
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0.0;
        };
        let callback: extern "system" fn(i32) -> f32 =
            unsafe { std::mem::transmute(api.stasis_gfx_measure_text_cached) };
        return callback(run_handle);
    }
    #[cfg(not(windows))]
    {
        let _ = run_handle;
        0.0
    }
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
pub extern "C" fn stasis_gfx_poll_reload(handle: i32) -> i32 {
    stasis_jit_gfx_poll_reload(handle)
}

#[no_mangle]
pub extern "C" fn stasis_jit_lookup_code_ptr(fn_id_raw: i32) -> i64 {
    let fn_id = fn_id_raw as u32;
    let table = jit_code_ptr_table();
    let guard = table.lock().expect("jit code ptr table mutex poisoned");
    guard
        .get(&fn_id)
        .copied()
        .map(|value| value as i64)
        .unwrap_or_default()
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
pub extern "C" fn stasis_jit_call_i32_0(fn_id_raw: i32) -> i32 {
    dispatch_i32_call0(fn_id_raw).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_1(fn_id_raw: i32, arg0: i32) -> i32 {
    dispatch_i32_call1(fn_id_raw, arg0).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_2(fn_id_raw: i32, arg0: i32, arg1: i32) -> i32 {
    dispatch_i32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_3(fn_id_raw: i32, arg0: i32, arg1: i32, arg2: i32) -> i32 {
    dispatch_i32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_4(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
) -> i32 {
    dispatch_i32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_5(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
) -> i32 {
    dispatch_i32_call5(fn_id_raw, arg0, arg1, arg2, arg3, arg4).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_6(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
) -> i32 {
    dispatch_i32_call6(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_7(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
    arg6: i32,
) -> i32 {
    dispatch_i32_call7(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_8(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
    arg6: i32,
    arg7: i32,
) -> i32 {
    dispatch_i32_call8(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_1(fn_id_raw: i32, arg0: f32) -> i32 {
    dispatch_i32_f32_call1(fn_id_raw, arg0).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_2(fn_id_raw: i32, arg0: f32, arg1: f32) -> i32 {
    dispatch_i32_f32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_3(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
) -> i32 {
    dispatch_i32_f32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> i32 {
    dispatch_i32_f32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_5(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
) -> i32 {
    dispatch_i32_f32_call5(fn_id_raw, arg0, arg1, arg2, arg3, arg4).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_6(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
) -> i32 {
    dispatch_i32_f32_call6(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_7(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
) -> i32 {
    dispatch_i32_f32_call7(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_i32_f32_8(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
    arg7: f32,
) -> i32 {
    dispatch_i32_f32_call8(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_0(fn_id_raw: i32) -> f32 {
    dispatch_f32_call0(fn_id_raw).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_1(fn_id_raw: i32, arg0: f32) -> f32 {
    dispatch_f32_call1(fn_id_raw, arg0).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_2(fn_id_raw: i32, arg0: f32, arg1: f32) -> f32 {
    dispatch_f32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> f32 {
    dispatch_f32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> f32 {
    dispatch_f32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_5(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
) -> f32 {
    dispatch_f32_call5(fn_id_raw, arg0, arg1, arg2, arg3, arg4).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_6(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
) -> f32 {
    dispatch_f32_call6(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_7(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
) -> f32 {
    dispatch_f32_call7(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6).unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_8(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
    arg7: f32,
) -> f32 {
    dispatch_f32_call8(fn_id_raw, arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7)
        .unwrap_or_default()
}

#[no_mangle]
pub extern "C" fn stasis_jit_call_f32_i32_1(fn_id_raw: i32, arg0: i32) -> f32 {
    dispatch_f32_call_i32_1(fn_id_raw, arg0).unwrap_or_default()
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
    stasis_jit_global_i32_load(derived)
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
}

pub fn clear_jit_f32_global_table() {
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
}

pub fn clear_jit_f64_global_table() {
    let table = jit_f64_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
}

pub fn clear_jit_i32_array_global_table() {
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
}

pub fn clear_jit_f32_array_global_table() {
    let table = jit_f32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
}

pub fn clear_jit_f64_array_global_table() {
    let table = jit_f64_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.clear();
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

#[no_mangle]
pub extern "C" fn stasis_jit_audio_load_music(path_id: i32) -> i32 {
    audio_load_file(path_id, true)
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_load_effect(path_id: i32) -> i32 {
    audio_load_file(path_id, false)
}

fn audio_load_file(path_id: i32, music: bool) -> i32 {
    #[cfg(windows)]
    {
        let Ok(path) = jit_text_arg_to_cstring(path_id) else {
            return 0;
        };
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let address = if music {
            api.stasis_audio_load_music
        } else {
            api.stasis_audio_load_effect
        };
        let callback: extern "system" fn(*const c_char) -> i32 =
            unsafe { std::mem::transmute(address) };
        return callback(path.as_ptr());
    }
    #[cfg(not(windows))]
    {
        let _ = (path_id, music);
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_play_music(handle: i32, looping: i32, volume: f32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(i32, i32, f32) -> i32 =
            unsafe { std::mem::transmute(api.stasis_audio_play_music) };
        return callback(handle, looping, volume);
    }
    #[cfg(not(windows))]
    {
        let _ = (handle, looping, volume);
        0
    }
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_stop_music(handle: i32) {
    #[cfg(windows)]
    if let Ok(api) = stasis_graphics_assets_api() {
        let callback: extern "system" fn(i32) =
            unsafe { std::mem::transmute(api.stasis_audio_stop_music) };
        callback(handle);
    }
    #[cfg(not(windows))]
    let _ = handle;
}

#[no_mangle]
pub extern "C" fn stasis_jit_audio_play_effect(handle: i32, volume: f32) -> i32 {
    #[cfg(windows)]
    {
        let Ok(api) = stasis_graphics_assets_api() else {
            return 0;
        };
        let callback: extern "system" fn(i32, f32) -> i32 =
            unsafe { std::mem::transmute(api.stasis_audio_play_effect) };
        return callback(handle, volume);
    }
    #[cfg(not(windows))]
    {
        let _ = (handle, volume);
        0
    }
}

fn dispatch_i32_call0(fn_id_raw: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 0)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=0"));
    }
    let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
    Ok(callback())
}

fn dispatch_i32_call1(fn_id_raw: i32, arg0: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0))
}

fn dispatch_i32_call2(fn_id_raw: i32, arg0: i32, arg1: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    let callback: extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1))
}

fn dispatch_i32_call3(fn_id_raw: i32, arg0: i32, arg1: i32, arg2: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    let callback: extern "system" fn(i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2))
}

fn dispatch_i32_call4(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 4)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=4"));
    }
    let callback: extern "system" fn(i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3))
}

fn dispatch_i32_call5(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 5)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=5"));
    }
    let callback: extern "system" fn(i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4))
}

fn dispatch_i32_call6(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 6)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=6"));
    }
    let callback: extern "system" fn(i32, i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5))
}

fn dispatch_i32_call7(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
    arg6: i32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 7)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=7"));
    }
    let callback: extern "system" fn(i32, i32, i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6))
}

fn dispatch_i32_call8(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
    arg4: i32,
    arg5: i32,
    arg6: i32,
    arg7: i32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 8)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=8"));
    }
    let callback: extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7))
}

fn dispatch_i32_f32_call1(fn_id_raw: i32, arg0: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    let callback: extern "system" fn(f32) -> i32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0))
}

fn dispatch_i32_f32_call2(fn_id_raw: i32, arg0: f32, arg1: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    let callback: extern "system" fn(f32, f32) -> i32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1))
}

fn dispatch_i32_f32_call3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    let callback: extern "system" fn(f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2))
}

fn dispatch_i32_f32_call4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 4)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=4"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3))
}

fn dispatch_i32_f32_call5(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 5)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=5"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4))
}

fn dispatch_i32_f32_call6(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 6)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=6"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5))
}

fn dispatch_i32_f32_call7(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 7)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=7"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6))
}

fn dispatch_i32_f32_call8(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
    arg7: f32,
) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 8)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=8"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32, f32) -> i32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7))
}

fn dispatch_f32_call0(fn_id_raw: i32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 0)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=0"));
    }
    let callback: extern "system" fn() -> f32 = unsafe { std::mem::transmute(address) };
    Ok(callback())
}

fn dispatch_f32_call1(fn_id_raw: i32, arg0: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    let callback: extern "system" fn(f32) -> f32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0))
}

fn dispatch_f32_call2(fn_id_raw: i32, arg0: f32, arg1: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    let callback: extern "system" fn(f32, f32) -> f32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1))
}

fn dispatch_f32_call3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    let callback: extern "system" fn(f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2))
}

fn dispatch_f32_call4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 4)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=4"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3))
}

fn dispatch_f32_call5(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 5)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=5"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4))
}

fn dispatch_f32_call6(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 6)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=6"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5))
}

fn dispatch_f32_call7(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 7)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=7"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6))
}

fn dispatch_f32_call8(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
    arg4: f32,
    arg5: f32,
    arg6: f32,
    arg7: f32,
) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 8)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=8"));
    }
    let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32, f32) -> f32 =
        unsafe { std::mem::transmute(address) };
    Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7))
}

fn dispatch_f32_call_i32_1(fn_id_raw: i32, arg0: i32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    let callback: extern "system" fn(i32) -> f32 = unsafe { std::mem::transmute(address) };
    Ok(callback(arg0))
}

fn lookup_jit_i32_target(fn_id: u32, arity: u8) -> Result<usize, String> {
    let table = jit_i32_dispatch_table();
    let guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard
        .get(&(fn_id, arity))
        .copied()
        .ok_or_else(|| format!("missing jit dispatch entry for fn_id={fn_id}, arity={arity}"))
}

fn lookup_jit_f32_target(fn_id: u32, arity: u8) -> Result<usize, String> {
    let table = jit_f32_dispatch_table();
    let guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard
        .get(&(fn_id, arity))
        .copied()
        .ok_or_else(|| format!("missing jit dispatch entry for fn_id={fn_id}, arity={arity}"))
}

type JitDispatchMap = std::collections::HashMap<(u32, u8), usize>;
type JitCodePtrMap = std::collections::HashMap<u32, usize>;
type JitI32GlobalMap = std::collections::HashMap<i32, i32>;
type JitF32GlobalMap = std::collections::HashMap<i32, f32>;
type JitF64GlobalMap = std::collections::HashMap<i32, f64>;
type JitI32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), i32>;
type JitF32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f32>;
type JitF64ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f64>;
type JitStringLiteralMap = std::collections::HashMap<i32, String>;

fn jit_i32_dispatch_table() -> &'static Mutex<JitDispatchMap> {
    static TABLE: OnceLock<Mutex<JitDispatchMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_dispatch_table() -> &'static Mutex<JitDispatchMap> {
    static TABLE: OnceLock<Mutex<JitDispatchMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_code_ptr_table() -> &'static Mutex<JitCodePtrMap> {
    static TABLE: OnceLock<Mutex<JitCodePtrMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::MutexGuard;

    fn test_lock() -> MutexGuard<'static, ()> {
        jit_dispatch_lock()
            .lock()
            .expect("jit dispatch lock mutex poisoned")
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
    fn jit_text_arg_bytes_reads_string_literals() {
        let _lock = test_lock();
        clear_registered_global_memory();
        clear_jit_i32_global_table();
        clear_jit_i32_array_global_table();
        clear_jit_string_literal_table();

        upsert_jit_string_literal(1234, "hello");

        assert_eq!(jit_text_arg_bytes(1234), Some(b"hello".to_vec()));
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
    }
}
