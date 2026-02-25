use std::path::Path;
use std::io::Write;
use std::sync::{Mutex, OnceLock};

#[cfg(windows)]
use std::ffi::{c_char, c_void, CString, OsStr};
#[cfg(windows)]
use std::os::windows::ffi::OsStrExt;

pub struct Library {
    #[cfg(windows)]
    handle: *mut c_void,
}

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
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> u64 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        let _ = address;
        Err("native no-arg invocation is only supported on windows in stasis_dynload".to_string())
    }
}

pub fn invoke_i32_i32_to_i32(address: usize, left: i32, right: i32) -> Result<i32, String> {
    if address == 0 {
        return Err("cannot invoke null function pointer".to_string());
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(left, right));
    }
    #[cfg(not(windows))]
    {
        let _ = left;
        let _ = right;
        Err(
            "native i32(i32,i32) invocation is only supported on windows in stasis_dynload"
                .to_string(),
        )
    }
}

pub fn replace_jit_i32_dispatch_table(entries: &[(u32, u8, usize)]) {
    let table = jit_i32_dispatch_table();
    let mut guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard.clear();
    for (fn_id, arity, code_ptr) in entries {
        guard.insert((*fn_id, *arity), *code_ptr);
    }
}

pub fn replace_jit_f32_dispatch_table(entries: &[(u32, u8, usize)]) {
    let table = jit_f32_dispatch_table();
    let mut guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard.clear();
    for (fn_id, arity, code_ptr) in entries {
        guard.insert((*fn_id, *arity), *code_ptr);
    }
}

pub fn replace_jit_code_ptr_table(entries: &[(u32, usize)]) {
    let table = jit_code_ptr_table();
    let mut guard = table.lock().expect("jit code ptr table mutex poisoned");
    guard.clear();
    for (fn_id, code_ptr) in entries {
        guard.insert(*fn_id, *code_ptr);
    }
}

pub fn clear_jit_string_literal_table() {
    let table = jit_string_literal_table();
    let mut guard = table.lock().expect("jit string literal table mutex poisoned");
    guard.clear();
}

pub fn upsert_jit_string_literal(id: i32, value: &str) {
    let table = jit_string_literal_table();
    let mut guard = table.lock().expect("jit string literal table mutex poisoned");
    guard.insert(id, value.to_string());
}

pub extern "C" fn stasis_jit_print_i32(value: i32) {
    print!("{value}");
    let _ = std::io::stdout().flush();
}

pub extern "C" fn stasis_jit_print_string(value_id: i32) {
    let table = jit_string_literal_table();
    let guard = table.lock().expect("jit string literal table mutex poisoned");
    if let Some(text) = guard.get(&value_id) {
        print!("{text}");
        let _ = std::io::stdout().flush();
    }
}

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

pub extern "C" fn stasis_jit_sin_fast(value: f32) -> f32 {
    value.sin()
}

pub extern "C" fn stasis_jit_cos_fast(value: f32) -> f32 {
    value.cos()
}

pub extern "C" fn stasis_jit_call_i32_0(fn_id_raw: i32) -> i32 {
    dispatch_i32_call0(fn_id_raw).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_1(fn_id_raw: i32, arg0: i32) -> i32 {
    dispatch_i32_call1(fn_id_raw, arg0).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_2(fn_id_raw: i32, arg0: i32, arg1: i32) -> i32 {
    dispatch_i32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_3(fn_id_raw: i32, arg0: i32, arg1: i32, arg2: i32) -> i32 {
    dispatch_i32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_4(
    fn_id_raw: i32,
    arg0: i32,
    arg1: i32,
    arg2: i32,
    arg3: i32,
) -> i32 {
    dispatch_i32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

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

pub extern "C" fn stasis_jit_call_i32_f32_1(fn_id_raw: i32, arg0: f32) -> i32 {
    dispatch_i32_f32_call1(fn_id_raw, arg0).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_f32_2(fn_id_raw: i32, arg0: f32, arg1: f32) -> i32 {
    dispatch_i32_f32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_f32_3(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
) -> i32 {
    dispatch_i32_f32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_f32_4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> i32 {
    dispatch_i32_f32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

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

pub extern "C" fn stasis_jit_call_f32_0(fn_id_raw: i32) -> f32 {
    dispatch_f32_call0(fn_id_raw).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_f32_1(fn_id_raw: i32, arg0: f32) -> f32 {
    dispatch_f32_call1(fn_id_raw, arg0).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_f32_2(fn_id_raw: i32, arg0: f32, arg1: f32) -> f32 {
    dispatch_f32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_f32_3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> f32 {
    dispatch_f32_call3(fn_id_raw, arg0, arg1, arg2).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_f32_4(
    fn_id_raw: i32,
    arg0: f32,
    arg1: f32,
    arg2: f32,
    arg3: f32,
) -> f32 {
    dispatch_f32_call4(fn_id_raw, arg0, arg1, arg2, arg3).unwrap_or_default()
}

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

pub extern "C" fn stasis_jit_call_f32_i32_1(fn_id_raw: i32, arg0: i32) -> f32 {
    dispatch_f32_call_i32_1(fn_id_raw, arg0).unwrap_or_default()
}

pub extern "C" fn stasis_jit_global_i32_load(path_hash: i32) -> i32 {
    let table = jit_i32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

pub extern "C" fn stasis_jit_global_i32_store(path_hash: i32, value: i32) {
    let table = jit_i32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

pub extern "C" fn stasis_jit_global_f32_load(path_hash: i32) -> f32 {
    let table = jit_f32_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard.get(&path_hash).copied().unwrap_or_default()
}

pub extern "C" fn stasis_jit_global_f32_store(path_hash: i32, value: f32) {
    let table = jit_f32_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert(path_hash, value);
}

pub extern "C" fn stasis_jit_global_i32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> i32 {
    let table = jit_i32_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

pub extern "C" fn stasis_jit_global_i32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: i32,
) {
    let table = jit_i32_array_global_table();
    let mut guard = table.lock().expect("jit global table mutex poisoned");
    guard.insert((collection_hash, field_hash, index), value);
}

pub extern "C" fn stasis_jit_global_f32_array_load(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
) -> f32 {
    let table = jit_f32_array_global_table();
    let guard = table.lock().expect("jit global table mutex poisoned");
    guard
        .get(&(collection_hash, field_hash, index))
        .copied()
        .unwrap_or_default()
}

pub extern "C" fn stasis_jit_global_f32_array_store(
    collection_hash: i32,
    field_hash: i32,
    index: i32,
    value: f32,
) {
    let table = jit_f32_array_global_table();
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

pub extern "C" fn stasis_jit_sys_memcpy_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_i32_array_lane(dst, dst_index, src, src_index, count);
}

pub extern "C" fn stasis_jit_sys_memcpy_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_i32_array_lane(dst, dst_index, src, src_index, count);
}

pub extern "C" fn stasis_jit_sys_memcpy_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_f32_array_lane(dst, dst_index, src, src_index, count);
}

pub extern "C" fn stasis_jit_sys_memmove_u8(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_i32_array_lane(dst, dst_index, src, src_index, count);
}

pub extern "C" fn stasis_jit_sys_memmove_i32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_i32_array_lane(dst, dst_index, src, src_index, count);
}

pub extern "C" fn stasis_jit_sys_memmove_f32(
    dst: i32,
    dst_index: i32,
    src: i32,
    src_index: i32,
    count: i32,
) {
    copy_f32_array_lane(dst, dst_index, src, src_index, count);
}

fn copy_i32_array_lane(dst: i32, dst_index: i32, src: i32, src_index: i32, count: i32) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<i32> = Vec::with_capacity(count as usize);
    {
        let table = jit_i32_array_global_table();
        let guard = table.lock().expect("jit global table mutex poisoned");
        for offset in 0..count {
            let index = src_index.saturating_add(offset);
            values.push(*guard.get(&(src, 0, index)).unwrap_or(&0));
        }
    }
    {
        let table = jit_i32_array_global_table();
        let mut guard = table.lock().expect("jit global table mutex poisoned");
        for (offset, value) in values.into_iter().enumerate() {
            let index = dst_index.saturating_add(offset as i32);
            guard.insert((dst, 0, index), value);
        }
    }
}

fn copy_f32_array_lane(dst: i32, dst_index: i32, src: i32, src_index: i32, count: i32) {
    if count <= 0 {
        return;
    }
    let mut values: Vec<f32> = Vec::with_capacity(count as usize);
    {
        let table = jit_f32_array_global_table();
        let guard = table.lock().expect("jit global table mutex poisoned");
        for offset in 0..count {
            let index = src_index.saturating_add(offset);
            values.push(*guard.get(&(src, 0, index)).unwrap_or(&0.0));
        }
    }
    {
        let table = jit_f32_array_global_table();
        let mut guard = table.lock().expect("jit global table mutex poisoned");
        for (offset, value) in values.into_iter().enumerate() {
            let index = dst_index.saturating_add(offset as i32);
            guard.insert((dst, 0, index), value);
        }
    }
}

fn dispatch_i32_call0(fn_id_raw: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 0)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=0"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        Err("jit i32 dispatch call0 is only supported on windows".to_string())
    }
}

fn dispatch_i32_call1(fn_id_raw: i32, arg0: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        Err("jit i32 dispatch call1 is only supported on windows".to_string())
    }
}

fn dispatch_i32_call2(fn_id_raw: i32, arg0: i32, arg1: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        Err("jit i32 dispatch call2 is only supported on windows".to_string())
    }
}

fn dispatch_i32_call3(fn_id_raw: i32, arg0: i32, arg1: i32, arg2: i32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        Err("jit i32 dispatch call3 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        Err("jit i32 dispatch call4 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        Err("jit i32 dispatch call5 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        Err("jit i32 dispatch call6 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        Err("jit i32 dispatch call7 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32, i32, i32, i32, i32, i32, i32, i32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        let _ = arg7;
        Err("jit i32 dispatch call8 is only supported on windows".to_string())
    }
}

fn dispatch_i32_f32_call1(fn_id_raw: i32, arg0: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        Err("jit i32 f32 dispatch call1 is only supported on windows".to_string())
    }
}

fn dispatch_i32_f32_call2(fn_id_raw: i32, arg0: f32, arg1: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32) -> i32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        Err("jit i32 f32 dispatch call2 is only supported on windows".to_string())
    }
}

fn dispatch_i32_f32_call3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> Result<i32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_i32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        Err("jit i32 f32 dispatch call3 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        Err("jit i32 f32 dispatch call4 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        Err("jit i32 f32 dispatch call5 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        Err("jit i32 f32 dispatch call6 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        Err("jit i32 f32 dispatch call7 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32, f32) -> i32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        let _ = arg7;
        Err("jit i32 f32 dispatch call8 is only supported on windows".to_string())
    }
}

fn dispatch_f32_call0(fn_id_raw: i32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 0)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=0"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn() -> f32 = unsafe { std::mem::transmute(address) };
        return Ok(callback());
    }
    #[cfg(not(windows))]
    {
        Err("jit f32 dispatch call0 is only supported on windows".to_string())
    }
}

fn dispatch_f32_call1(fn_id_raw: i32, arg0: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32) -> f32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        Err("jit f32 dispatch call1 is only supported on windows".to_string())
    }
}

fn dispatch_f32_call2(fn_id_raw: i32, arg0: f32, arg1: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 2)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=2"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32) -> f32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        Err("jit f32 dispatch call2 is only supported on windows".to_string())
    }
}

fn dispatch_f32_call3(fn_id_raw: i32, arg0: f32, arg1: f32, arg2: f32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 3)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=3"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        Err("jit f32 dispatch call3 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        Err("jit f32 dispatch call4 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        Err("jit f32 dispatch call5 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        Err("jit f32 dispatch call6 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        Err("jit f32 dispatch call7 is only supported on windows".to_string())
    }
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
    #[cfg(windows)]
    {
        let callback: extern "system" fn(f32, f32, f32, f32, f32, f32, f32, f32) -> f32 =
            unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0, arg1, arg2, arg3, arg4, arg5, arg6, arg7));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        let _ = arg1;
        let _ = arg2;
        let _ = arg3;
        let _ = arg4;
        let _ = arg5;
        let _ = arg6;
        let _ = arg7;
        Err("jit f32 dispatch call8 is only supported on windows".to_string())
    }
}

fn dispatch_f32_call_i32_1(fn_id_raw: i32, arg0: i32) -> Result<f32, String> {
    let fn_id = fn_id_raw as u32;
    let address = lookup_jit_f32_target(fn_id, 1)?;
    if address == 0 {
        return Err(format!("missing code pointer for fn_id={fn_id}, arity=1"));
    }
    #[cfg(windows)]
    {
        let callback: extern "system" fn(i32) -> f32 = unsafe { std::mem::transmute(address) };
        return Ok(callback(arg0));
    }
    #[cfg(not(windows))]
    {
        let _ = arg0;
        Err("jit f32 dispatch call(i32)->f32 is only supported on windows".to_string())
    }
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
type JitI32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), i32>;
type JitF32ArrayGlobalMap = std::collections::HashMap<(i32, i32, i32), f32>;
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

fn jit_i32_array_global_table() -> &'static Mutex<JitI32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitI32ArrayGlobalMap>> = OnceLock::new();
    TABLE.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
}

fn jit_f32_array_global_table() -> &'static Mutex<JitF32ArrayGlobalMap> {
    static TABLE: OnceLock<Mutex<JitF32ArrayGlobalMap>> = OnceLock::new();
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
}
