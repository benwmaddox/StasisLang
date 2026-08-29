use std::ffi::{c_char, c_void, CString};
use std::path::Path;

/// Atomically rename `source` to `destination`, refusing an existing destination.
///
/// Recording publication uses this instead of a check-then-rename sequence so a
/// concurrent creator cannot be overwritten.
pub fn atomic_rename_no_replace(source: &Path, destination: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let source_display = source.display().to_string();
        let destination_display = destination.display().to_string();
        let source = source
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let destination = destination
            .as_os_str()
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        unsafe extern "system" {
            fn MoveFileExW(existing: *const u16, new: *const u16, flags: u32) -> i32;
        }
        const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
        let result = unsafe {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        };
        if result != 0 {
            return Ok(());
        }
        return Err(format!(
            "atomic no-replace rename {} -> {} failed: {}",
            source_display,
            destination_display,
            std::io::Error::last_os_error()
        ));
    }

    #[cfg(target_os = "linux")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source_display = source.display().to_string();
        let destination_display = destination.display().to_string();
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| "recording source path contains NUL".to_string())?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "recording destination path contains NUL".to_string())?;
        unsafe extern "C" {
            fn renameat2(
                olddirfd: i32,
                oldpath: *const c_char,
                newdirfd: i32,
                newpath: *const c_char,
                flags: u32,
            ) -> i32;
        }
        const AT_FDCWD: i32 = -100;
        const RENAME_NOREPLACE: u32 = 1;
        let result = unsafe {
            renameat2(
                AT_FDCWD,
                source.as_ptr(),
                AT_FDCWD,
                destination.as_ptr(),
                RENAME_NOREPLACE,
            )
        };
        if result == 0 {
            return Ok(());
        }
        return Err(format!(
            "atomic no-replace rename {} -> {} failed: {}",
            source_display,
            destination_display,
            std::io::Error::last_os_error()
        ));
    }

    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt;
        let source_display = source.display().to_string();
        let destination_display = destination.display().to_string();
        let source = CString::new(source.as_os_str().as_bytes())
            .map_err(|_| "recording source path contains NUL".to_string())?;
        let destination = CString::new(destination.as_os_str().as_bytes())
            .map_err(|_| "recording destination path contains NUL".to_string())?;
        unsafe extern "C" {
            fn renamex_np(from: *const c_char, to: *const c_char, flags: u32) -> i32;
        }
        const RENAME_EXCL: u32 = 0x4;
        let result = unsafe { renamex_np(source.as_ptr(), destination.as_ptr(), RENAME_EXCL) };
        if result == 0 {
            return Ok(());
        }
        return Err(format!(
            "atomic no-replace rename {} -> {} failed: {}",
            source_display,
            destination_display,
            std::io::Error::last_os_error()
        ));
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "macos")))]
    {
        let _ = (source, destination);
        Err("atomic no-replace rename is unsupported on this desktop platform".to_string())
    }
}

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

#[cfg(windows)]
fn os_str_to_wide(value: &OsStr) -> Vec<u16> {
    value.encode_wide().collect()
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
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
