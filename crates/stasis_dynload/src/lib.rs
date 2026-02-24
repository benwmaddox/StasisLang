use std::path::Path;
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

pub extern "C" fn stasis_jit_call_i32_0(fn_id_raw: i32) -> i32 {
    dispatch_i32_call0(fn_id_raw).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_1(fn_id_raw: i32, arg0: i32) -> i32 {
    dispatch_i32_call1(fn_id_raw, arg0).unwrap_or_default()
}

pub extern "C" fn stasis_jit_call_i32_2(fn_id_raw: i32, arg0: i32, arg1: i32) -> i32 {
    dispatch_i32_call2(fn_id_raw, arg0, arg1).unwrap_or_default()
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

fn lookup_jit_i32_target(fn_id: u32, arity: u8) -> Result<usize, String> {
    let table = jit_i32_dispatch_table();
    let guard = table.lock().expect("jit dispatch table mutex poisoned");
    guard
        .get(&(fn_id, arity))
        .copied()
        .ok_or_else(|| format!("missing jit dispatch entry for fn_id={fn_id}, arity={arity}"))
}

type JitDispatchMap = std::collections::HashMap<(u32, u8), usize>;

fn jit_i32_dispatch_table() -> &'static Mutex<JitDispatchMap> {
    static TABLE: OnceLock<Mutex<JitDispatchMap>> = OnceLock::new();
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
