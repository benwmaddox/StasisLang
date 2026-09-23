use super::*;
use std::collections::HashSet;
use std::ffi::c_void;
use std::marker::PhantomData;
use std::path::Path;
use std::rc::Rc;
use std::sync::atomic::AtomicBool;
use std::thread::ThreadId;

static AOT_PROBE_SESSION_ACTIVE: AtomicBool = AtomicBool::new(false);
static AOT_PROBE_SESSION_OWNER: OnceLock<Mutex<Option<ThreadId>>> = OnceLock::new();

fn session_owner() -> &'static Mutex<Option<ThreadId>> {
    AOT_PROBE_SESSION_OWNER.get_or_init(|| Mutex::new(None))
}

pub(super) fn current_thread_owns_session() -> bool {
    if !AOT_PROBE_SESSION_ACTIVE.load(Ordering::Acquire) {
        return true;
    }
    session_owner()
        .lock()
        .map(|owner| owner.as_ref() == Some(&std::thread::current().id()))
        .unwrap_or(false)
}

#[no_mangle]
pub extern "C" fn stasis_jit_aot_probe_session_begin() -> i32 {
    if AOT_PROBE_SESSION_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return 0;
    }
    let dispatch = match jit_dispatch_lock().lock() {
        Ok(dispatch) => dispatch,
        Err(_) => {
            AOT_PROBE_SESSION_ACTIVE.store(false, Ordering::Release);
            return 0;
        }
    };
    if !runtime_probe_state_is_empty() {
        AOT_PROBE_SESSION_ACTIVE.store(false, Ordering::Release);
        return 0;
    }
    match session_owner().lock() {
        Ok(mut owner) => *owner = Some(std::thread::current().id()),
        Err(_) => {
            AOT_PROBE_SESSION_ACTIVE.store(false, Ordering::Release);
            return 0;
        }
    }
    drop(dispatch);
    1
}

#[no_mangle]
pub extern "C" fn stasis_jit_aot_probe_session_end() -> i32 {
    if !AOT_PROBE_SESSION_ACTIVE.load(Ordering::Acquire) || !current_thread_owns_session() {
        return 0;
    }
    let dispatch = match acquire_rebind_guard() {
        Ok(dispatch) => dispatch,
        Err(_) => return 0,
    };
    clear_registered_global_storage_unlocked();
    clear_jit_string_literal_table();
    if let Ok(mut owner) = session_owner().lock() {
        *owner = None;
    } else {
        return 0;
    }
    AOT_PROBE_SESSION_ACTIVE.store(false, Ordering::Release);
    drop(dispatch);
    1
}

fn runtime_probe_state_is_empty() -> bool {
    fn empty<T>(table: &Mutex<T>, is_empty: impl FnOnce(&T) -> bool) -> bool {
        table.lock().map(|value| is_empty(&value)).unwrap_or(false)
    }
    empty(registered_i32_ptrs(), HashMap::is_empty)
        && empty(registered_f32_ptrs(), HashMap::is_empty)
        && empty(registered_f64_ptrs(), HashMap::is_empty)
        && empty(registered_i32_arrays(), HashMap::is_empty)
        && empty(registered_f32_arrays(), HashMap::is_empty)
        && empty(registered_f64_arrays(), HashMap::is_empty)
        && empty(registered_u8_arrays(), HashMap::is_empty)
        && empty(registered_u16_arrays(), HashMap::is_empty)
        && empty(owned_i32_scalars(), HashMap::is_empty)
        && empty(owned_f32_scalars(), HashMap::is_empty)
        && empty(owned_f64_scalars(), HashMap::is_empty)
        && empty(owned_i32_arrays(), HashMap::is_empty)
        && empty(owned_f32_arrays(), HashMap::is_empty)
        && empty(owned_f64_arrays(), HashMap::is_empty)
        && empty(owned_u8_arrays(), HashMap::is_empty)
        && empty(owned_u16_arrays(), HashMap::is_empty)
        && empty(direct_storage_slots(), HashMap::is_empty)
        && empty(direct_array_required_lengths(), HashMap::is_empty)
        && empty(jit_string_literal_table(), HashMap::is_empty)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AotProbeStorageKind {
    I32,
    F32,
    F64,
    U8,
    U16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AotProbeStorageDescriptor {
    Scalar {
        symbol: String,
        path_hash: i32,
        kind: AotProbeStorageKind,
    },
    Array {
        symbol: String,
        collection_hash: i32,
        field_hash: i32,
        kind: AotProbeStorageKind,
        len: usize,
    },
}

impl AotProbeStorageDescriptor {
    pub fn scalar(symbol: impl Into<String>, path_hash: i32, kind: AotProbeStorageKind) -> Self {
        Self::Scalar {
            symbol: symbol.into(),
            path_hash,
            kind,
        }
    }

    pub fn array(
        symbol: impl Into<String>,
        collection_hash: i32,
        field_hash: i32,
        kind: AotProbeStorageKind,
        len: usize,
    ) -> Self {
        Self::Array {
            symbol: symbol.into(),
            collection_hash,
            field_hash,
            kind,
            len,
        }
    }

    fn details(&self) -> Result<(&str, AotProbeStorageKind, i32, i32, usize, bool), String> {
        match self {
            Self::Scalar {
                symbol,
                path_hash,
                kind,
            } => {
                if matches!(kind, AotProbeStorageKind::U8 | AotProbeStorageKind::U16) {
                    return Err(format!(
                        "scalar storage kind {kind:?} has no runtime pointer lane"
                    ));
                }
                Ok((symbol, *kind, *path_hash, 0, 1, false))
            }
            Self::Array {
                symbol,
                collection_hash,
                field_hash,
                kind,
                len,
            } => {
                if *len > i32::MAX as usize {
                    return Err(format!(
                        "array storage {symbol} length {len} exceeds the runtime ABI"
                    ));
                }
                Ok((symbol, *kind, *collection_hash, *field_hash, *len, true))
            }
        }
    }
}

struct RegistryApi {
    begin: unsafe extern "C" fn() -> i32,
    end: unsafe extern "C" fn() -> i32,
    clear_literals: unsafe extern "C" fn(),
    upsert_literal: unsafe extern "C" fn(i32, *const c_char),
    register_i32_ptr: unsafe extern "C" fn(i32, *mut i32),
    register_f32_ptr: unsafe extern "C" fn(i32, *mut f32),
    register_f64_ptr: unsafe extern "C" fn(i32, *mut f64),
    register_i32_array: unsafe extern "C" fn(i32, i32, *mut i32, i32),
    register_f32_array: unsafe extern "C" fn(i32, i32, *mut f32, i32),
    register_f64_array: unsafe extern "C" fn(i32, i32, *mut f64, i32),
    register_u8_array: unsafe extern "C" fn(i32, i32, *mut u8, i32),
    register_u16_array: unsafe extern "C" fn(i32, i32, *mut u16, i32),
}

impl RegistryApi {
    fn load(library: &Library) -> Result<Self, String> {
        macro_rules! symbol {
            ($name:literal, $ty:ty) => {{
                let address = library.symbol_address($name)?;
                // SAFETY: these named exports use the declared runtime ABI; the session retains the DLL.
                unsafe { std::mem::transmute::<usize, $ty>(address) }
            }};
        }
        Ok(Self {
            begin: symbol!(
                "stasis_jit_aot_probe_session_begin",
                unsafe extern "C" fn() -> i32
            ),
            end: symbol!(
                "stasis_jit_aot_probe_session_end",
                unsafe extern "C" fn() -> i32
            ),
            clear_literals: symbol!(
                "stasis_jit_clear_string_literal_table",
                unsafe extern "C" fn()
            ),
            upsert_literal: symbol!(
                "stasis_jit_upsert_string_literal",
                unsafe extern "C" fn(i32, *const c_char)
            ),
            register_i32_ptr: symbol!(
                "stasis_jit_register_global_i32_ptr",
                unsafe extern "C" fn(i32, *mut i32)
            ),
            register_f32_ptr: symbol!(
                "stasis_jit_register_global_f32_ptr",
                unsafe extern "C" fn(i32, *mut f32)
            ),
            register_f64_ptr: symbol!(
                "stasis_jit_register_global_f64_ptr",
                unsafe extern "C" fn(i32, *mut f64)
            ),
            register_i32_array: symbol!(
                "stasis_jit_register_global_i32_array",
                unsafe extern "C" fn(i32, i32, *mut i32, i32)
            ),
            register_f32_array: symbol!(
                "stasis_jit_register_global_f32_array",
                unsafe extern "C" fn(i32, i32, *mut f32, i32)
            ),
            register_f64_array: symbol!(
                "stasis_jit_register_global_f64_array",
                unsafe extern "C" fn(i32, i32, *mut f64, i32)
            ),
            register_u8_array: symbol!(
                "stasis_jit_register_global_u8_array",
                unsafe extern "C" fn(i32, i32, *mut u8, i32)
            ),
            register_u16_array: symbol!(
                "stasis_jit_register_global_u16_array",
                unsafe extern "C" fn(i32, i32, *mut u16, i32)
            ),
        })
    }
}

struct ResolvedStorage {
    kind: AotProbeStorageKind,
    address: usize,
    len: usize,
}

pub struct AotProbeSession {
    runtime: Option<Library>,
    guest: Option<Library>,
    registry: RegistryApi,
    storage: HashMap<String, ResolvedStorage>,
    active: bool,
    bootstrapped: bool,
    _thread_bound: PhantomData<Rc<()>>,
}

impl AotProbeSession {
    /// Owns the exact runtime DLL used by a trusted compiler-produced AOT probe.
    pub fn load(runtime_path: &Path, guest_path: &Path) -> Result<Self, String> {
        let runtime = Library::load(runtime_path)?;
        let registry = RegistryApi::load(&runtime)?;
        // SAFETY: the typed export belongs to the runtime DLL retained below.
        if unsafe { (registry.begin)() } == 0 {
            return Err(
                "runtime DLL registry is occupied or already has an AOT probe session".into(),
            );
        }
        let guest = match Library::load(guest_path) {
            Ok(guest) => guest,
            Err(error) => {
                // SAFETY: this thread claimed the session and the runtime DLL is still loaded.
                let _ = unsafe { (registry.end)() };
                return Err(error);
            }
        };
        Ok(Self {
            runtime: Some(runtime),
            guest: Some(guest),
            registry,
            storage: HashMap::new(),
            active: true,
            bootstrapped: false,
            _thread_bound: PhantomData,
        })
    }

    /// Validates all compiler-owned descriptors before publishing any registry pointer.
    pub fn bootstrap(
        &mut self,
        literals: &[(i32, String)],
        descriptors: &[AotProbeStorageDescriptor],
    ) -> Result<(), String> {
        if self.bootstrapped {
            return Err("AOT probe session is already bootstrapped".into());
        }
        let mut symbols = HashSet::new();
        let mut keys = HashSet::new();
        for descriptor in descriptors {
            let (symbol, kind, collection_hash, field_hash, len, _) = descriptor.details()?;
            if symbol.is_empty() || !symbols.insert(symbol.to_string()) {
                return Err(format!("empty or duplicate AOT storage symbol: {symbol:?}"));
            }
            if !keys.insert((kind, collection_hash, field_hash)) {
                return Err(format!("duplicate AOT storage registry key for {symbol}"));
            }
            len.checked_mul(storage_element_size(kind))
                .ok_or_else(|| format!("AOT storage byte length overflow for {symbol}"))?;
        }
        let c_literals = literals
            .iter()
            .map(|(id, value)| {
                CString::new(value.as_str())
                    .map(|value| (*id, value))
                    .map_err(|_| format!("AOT string literal {id} contains an interior NUL"))
            })
            .collect::<Result<Vec<_>, _>>()?;

        let guest = self.guest.as_ref().expect("session owns guest library");
        let mut resolved = HashMap::new();
        for descriptor in descriptors {
            let (symbol, kind, _, _, len, _) = descriptor.details()?;
            let address = guest
                .symbol_address(symbol)
                .map_err(|error| format!("resolve AOT storage {symbol}: {error}"))?;
            validate_image_storage(guest, symbol, address, kind, len)?;
            resolved.insert(symbol.to_string(), ResolvedStorage { kind, address, len });
        }

        // SAFETY: this thread owns the registry and retains its runtime DLL.
        unsafe { (self.registry.clear_literals)() };
        for (id, value) in &c_literals {
            // SAFETY: the owned CString remains valid for the callback, which copies its bytes.
            unsafe { (self.registry.upsert_literal)(*id, value.as_ptr()) };
        }
        for descriptor in descriptors {
            let (symbol, kind, collection_hash, field_hash, len, is_array) =
                descriptor.details()?;
            let address = resolved[symbol].address;
            if is_array {
                if len == 0 {
                    continue;
                }
                let len = i32::try_from(len)
                    .map_err(|_| format!("AOT storage length exceeds i32 for {symbol}"))?;
                // SAFETY: validated lane alignment and writable extent belong to the retained guest DLL.
                unsafe {
                    match kind {
                        AotProbeStorageKind::I32 => (self.registry.register_i32_array)(
                            collection_hash,
                            field_hash,
                            address as *mut i32,
                            len,
                        ),
                        AotProbeStorageKind::F32 => (self.registry.register_f32_array)(
                            collection_hash,
                            field_hash,
                            address as *mut f32,
                            len,
                        ),
                        AotProbeStorageKind::F64 => (self.registry.register_f64_array)(
                            collection_hash,
                            field_hash,
                            address as *mut f64,
                            len,
                        ),
                        AotProbeStorageKind::U8 => (self.registry.register_u8_array)(
                            collection_hash,
                            field_hash,
                            address as *mut u8,
                            len,
                        ),
                        AotProbeStorageKind::U16 => (self.registry.register_u16_array)(
                            collection_hash,
                            field_hash,
                            address as *mut u16,
                            len,
                        ),
                    }
                }
            } else {
                // SAFETY: validated lane alignment and writable extent belong to the retained guest DLL.
                unsafe {
                    match kind {
                        AotProbeStorageKind::I32 => {
                            (self.registry.register_i32_ptr)(collection_hash, address as *mut i32)
                        }
                        AotProbeStorageKind::F32 => {
                            (self.registry.register_f32_ptr)(collection_hash, address as *mut f32)
                        }
                        AotProbeStorageKind::F64 => {
                            (self.registry.register_f64_ptr)(collection_hash, address as *mut f64)
                        }
                        AotProbeStorageKind::U8 | AotProbeStorageKind::U16 => {
                            unreachable!("scalar lanes validated")
                        }
                    }
                }
            }
        }
        self.storage = resolved;
        self.bootstrapped = true;
        Ok(())
    }

    /// Runs trusted compiler-generated reset `void()`, probe `i32()`, and finish
    /// `i32(i32)` exports. Names must come from the matching compiler artifacts.
    pub fn invoke_frame(
        &mut self,
        reset_symbol: &str,
        root_symbol: &str,
        finish_symbol: &str,
    ) -> Result<(), String> {
        if !self.bootstrapped {
            return Err("AOT probe session has not been bootstrapped".into());
        }
        let reset = self.function_address(reset_symbol)?;
        let root = self.function_address(root_symbol)?;
        let finish = self.function_address(finish_symbol)?;
        invoke_noarg_void(reset)?;
        let result = invoke_noarg_i32(root)?;
        if result != 0 {
            return Err(format!("AOT probe {root_symbol} returned {result}"));
        }
        let result = invoke_i32_to_i32(finish, result)?;
        if result != 0 {
            return Err(format!(
                "AOT frame finish for {root_symbol} returned {result}"
            ));
        }
        Ok(())
    }

    pub fn read_i32(&mut self, symbol: &str) -> Result<Vec<i32>, String> {
        let storage = self.typed_storage(symbol, AotProbeStorageKind::I32)?;
        // SAFETY: bootstrap checked the image extent, lane alignment, and length.
        // The session retains both DLLs and serializes this copy with guest execution.
        Ok(
            unsafe { std::slice::from_raw_parts(storage.address as *const i32, storage.len) }
                .to_vec(),
        )
    }

    pub fn read_f32(&mut self, symbol: &str) -> Result<Vec<f32>, String> {
        let storage = self.typed_storage(symbol, AotProbeStorageKind::F32)?;
        // SAFETY: bootstrap checked the image extent, lane alignment, and length.
        // The session retains both DLLs and serializes this copy with guest execution.
        Ok(
            unsafe { std::slice::from_raw_parts(storage.address as *const f32, storage.len) }
                .to_vec(),
        )
    }

    pub fn read_f64(&mut self, symbol: &str) -> Result<Vec<f64>, String> {
        let storage = self.typed_storage(symbol, AotProbeStorageKind::F64)?;
        // SAFETY: bootstrap checked the image extent, lane alignment, and length.
        // The session retains both DLLs and serializes this copy with guest execution.
        Ok(
            unsafe { std::slice::from_raw_parts(storage.address as *const f64, storage.len) }
                .to_vec(),
        )
    }

    pub fn read_u8(&mut self, symbol: &str) -> Result<Vec<u8>, String> {
        let storage = self.typed_storage(symbol, AotProbeStorageKind::U8)?;
        // SAFETY: bootstrap checked the image extent, lane alignment, and length.
        // The session retains both DLLs and serializes this copy with guest execution.
        Ok(
            unsafe { std::slice::from_raw_parts(storage.address as *const u8, storage.len) }
                .to_vec(),
        )
    }

    pub fn read_u16(&mut self, symbol: &str) -> Result<Vec<u16>, String> {
        let storage = self.typed_storage(symbol, AotProbeStorageKind::U16)?;
        // SAFETY: bootstrap checked the image extent, lane alignment, and length.
        // The session retains both DLLs and serializes this copy with guest execution.
        Ok(
            unsafe { std::slice::from_raw_parts(storage.address as *const u16, storage.len) }
                .to_vec(),
        )
    }

    fn function_address(&self, symbol: &str) -> Result<usize, String> {
        let guest = self.guest.as_ref().expect("session owns guest library");
        let address = guest.symbol_address(symbol)?;
        if !guest.owns_address(address) {
            return Err(format!(
                "AOT entry symbol {symbol} does not belong to the owned probe DLL"
            ));
        }
        Ok(address)
    }

    fn typed_storage(
        &self,
        symbol: &str,
        expected: AotProbeStorageKind,
    ) -> Result<&ResolvedStorage, String> {
        if !self.bootstrapped {
            return Err("AOT probe session has not been bootstrapped".into());
        }
        let storage = self
            .storage
            .get(symbol)
            .ok_or_else(|| format!("AOT storage descriptor not found for {symbol}"))?;
        if storage.kind != expected {
            return Err(format!(
                "AOT storage {symbol} has type {:?}, requested {expected:?}",
                storage.kind
            ));
        }
        Ok(storage)
    }
}

impl Drop for AotProbeSession {
    fn drop(&mut self) {
        // SAFETY: the thread-bound session still owns both DLLs until teardown succeeds.
        let clean = !self.active || unsafe { (self.registry.end)() != 0 };
        self.active = false;
        if clean {
            drop(self.guest.take());
            drop(self.runtime.take());
        } else {
            // Keep the image mapped if teardown could not remove its registered storage pointers.
            if let Some(guest) = self.guest.take() {
                std::mem::forget(guest);
            }
            if let Some(runtime) = self.runtime.take() {
                std::mem::forget(runtime);
            }
        }
    }
}

fn storage_element_size(kind: AotProbeStorageKind) -> usize {
    match kind {
        AotProbeStorageKind::I32 => std::mem::size_of::<i32>(),
        AotProbeStorageKind::F32 => std::mem::size_of::<f32>(),
        AotProbeStorageKind::F64 => std::mem::size_of::<f64>(),
        AotProbeStorageKind::U8 => std::mem::size_of::<u8>(),
        AotProbeStorageKind::U16 => std::mem::size_of::<u16>(),
    }
}

fn storage_alignment(kind: AotProbeStorageKind) -> usize {
    match kind {
        AotProbeStorageKind::I32 => std::mem::align_of::<i32>(),
        AotProbeStorageKind::F32 => std::mem::align_of::<f32>(),
        AotProbeStorageKind::F64 => std::mem::align_of::<f64>(),
        AotProbeStorageKind::U8 => std::mem::align_of::<u8>(),
        AotProbeStorageKind::U16 => std::mem::align_of::<u16>(),
    }
}

fn validate_image_storage(
    guest: &Library,
    symbol: &str,
    address: usize,
    kind: AotProbeStorageKind,
    len: usize,
) -> Result<(), String> {
    if address == 0 || address % storage_alignment(kind) != 0 {
        return Err(format!(
            "AOT storage {symbol} has a null or misaligned address"
        ));
    }
    let bytes = len
        .checked_mul(storage_element_size(kind))
        .ok_or_else(|| format!("AOT storage byte length overflow for {symbol}"))?;
    let end = address
        .checked_add(bytes.max(1))
        .ok_or_else(|| format!("AOT storage address extent overflow for {symbol}"))?;
    let mut region = std::mem::MaybeUninit::<MemoryBasicInformation>::uninit();
    // SAFETY: VirtualQuery inspects the address and writes at most the supplied buffer size.
    let result = unsafe {
        VirtualQuery(
            address as *const c_void,
            region.as_mut_ptr(),
            std::mem::size_of::<MemoryBasicInformation>(),
        )
    };
    if result == 0 {
        return Err(format!(
            "query loaded image extent for AOT storage {symbol} failed"
        ));
    }
    // SAFETY: a successful VirtualQuery initialized the platform-layout structure.
    let region = unsafe { region.assume_init() };
    let region_start = region.base_address as usize;
    let region_end = region_start
        .checked_add(region.region_size)
        .ok_or_else(|| format!("loaded image region overflow for AOT storage {symbol}"))?;
    let protection = region.protect & 0xff;
    let writable = matches!(protection, 0x04 | 0x08 | 0x40 | 0x80);
    if !guest.owns_address(address)
        || !guest.owns_address(end - 1)
        || region.state != 0x1000
        || region.kind != 0x0100_0000
        || !writable
        || region.protect & 0x100 != 0
        || address < region_start
        || end > region_end
    {
        return Err(format!(
            "AOT storage {symbol} is outside writable image-backed storage"
        ));
    }
    Ok(())
}

#[repr(C)]
struct MemoryBasicInformation {
    base_address: *mut c_void,
    _allocation_base: *mut c_void,
    _allocation_protect: u32,
    #[cfg(target_pointer_width = "64")]
    _partition_id: u16,
    region_size: usize,
    state: u32,
    protect: u32,
    kind: u32,
}

#[link(name = "kernel32")]
unsafe extern "system" {
    fn VirtualQuery(
        address: *const c_void,
        buffer: *mut MemoryBasicInformation,
        length: usize,
    ) -> usize;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_lifecycle_rejects_occupied_state_and_serializes_literal_mutations() {
        let collection_hash = 0x5a71_5101;
        let occupied_literal_id = 0x5a71_5102;
        provision_direct_array_storage(JitStorageKind::I32, collection_hash, 0, 2)
            .expect("provision owned registry storage");
        upsert_jit_string_literal(occupied_literal_id, "occupied sentinel");

        assert_eq!(stasis_jit_aot_probe_session_begin(), 0);
        let (_, registered_len) = registered_storage(JitStorageKind::I32, collection_hash, 0)
            .expect("occupied storage remains registered after rejection");
        assert_eq!(registered_len, 2);
        assert_eq!(
            jit_string_literal_value(occupied_literal_id).as_deref(),
            Some("occupied sentinel"),
            "occupied literal remains after rejection"
        );

        clear_registered_global_memory();
        clear_jit_string_literal_table();
        assert_eq!(stasis_jit_aot_probe_session_begin(), 1);
        let owner_id = 0x5a71_5110;
        let foreign_id = 0x5a71_5111;
        upsert_jit_string_literal(owner_id, "owner sentinel");

        std::thread::spawn(move || {
            clear_jit_string_literal_table();
            upsert_jit_string_literal(foreign_id, "foreign upsert");
            let mut replacement = HashMap::new();
            replacement.insert(foreign_id, "foreign replacement".to_string());
            replace_jit_string_literal_table(&replacement);
        })
        .join()
        .expect("foreign literal mutations finish");

        assert_eq!(
            jit_string_literal_value(owner_id).as_deref(),
            Some("owner sentinel"),
            "foreign clear and replacement preserve the session owner's literal"
        );
        assert_eq!(
            jit_string_literal_value(foreign_id),
            None,
            "foreign upsert and replacement are rejected"
        );
        assert_eq!(stasis_jit_aot_probe_session_end(), 1);
        assert_eq!(jit_string_literal_value(owner_id), None);
        assert_eq!(stasis_jit_aot_probe_session_begin(), 1);
        assert_eq!(stasis_jit_aot_probe_session_end(), 1);
    }
}
