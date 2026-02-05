/*
 * stasis-cranelift-jit-runner (server mode)
 *
 * High-level job:
 * - Read Stasis CLIF (Cranelift IR in a simple text format) over stdin.
 * - JIT compile it in-process using cranelift_jit::JITModule.
 * - Execute `<module>__main` once, then repeatedly call `<module>__tick`.
 * - Support hot-swap by accepting replacement CLIF over stdin and swapping the active
 *   compiled instance while preserving global state.
 *
 * Protocol (stdin/stdout):
 * - On startup: prints `READY` on stdout.
 * - INIT: `INIT <moduleLen> <entryLen> <tickLen> <clifLen>\n<module><entry><tick><clif>`
 *     Compiles, runs entry, then enters tick loop. Replies `OK` or `ERR ...`.
 * - BIND: `BIND <jsonLen> <metaLen>\n<jsonPath><structMetaPath>`
 *     Loads compiler-emitted struct-meta.json and applies/polls a JSON file to update globals.
 *     Replies `OK` or `ERR ...`.
 * - SWAP: `SWAP <clifLen>\n<clif>`
 *     Compiles on a background worker thread. When finished, the tick loop swaps the active
 *     instance with a fast state copy and responds `OK ...` or `ERR ...`.
 * - QUIT: replies `OK` and exits.
 *
 * Hot-swap model (JIT path):
 * - Persisted state is the set of exported globals whose symbol names start with `state__`.
 * - On swap, we copy bytes for each `state__*` global from the old instance into a map, then
 *   restore those bytes into the new instance. This mirrors the disk/DLL runner but doesn't use
 *   a separate on-disk state map.
 *
 * Notes on differences vs the disk/DLL runner:
 * - No filesystem swap files and no OS dynamic loader; the compiled code lives in-process.
 * - Bulk host loop: `stasis_host_bulk_step` is supported but disabled by default (it has shown
 *   the ability to deadlock the tick loop on some click paths). The legacy loop
 *   (`host_get_frame` + `tick` + `gfx_submit_u8`) is the default for reliability.
 */

use std::collections::HashMap;
use std::ffi::c_void;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use clap::Parser;
use cranelift_codegen::settings;
use cranelift_codegen::settings::Configurable;
use cranelift_module::{default_libcall_names, DataDescription, Linkage, Module};
use cranelift_jit::{JITBuilder, JITModule};
use cranelift_native;
use libloading::Library;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use target_lexicon::Triple;

mod stasis_clif;

#[derive(Parser)]
#[command(name = "stasis-cranelift-jit-runner")]
#[command(about = "Run Stasis Cranelift CLIF in-process via cranelift-jit, with optional hot-swap over stdin.")]
struct Args
{
    #[arg(long)]
    server: bool,

    #[arg(long, value_name = "FPS", default_value = "60")]
    fps: u32,
}

fn main() -> Result<()>
{
    let args = Args::parse();
    if args.server
    {
        return run_server(args.fps);
    }

    bail!("non-server mode not implemented (use --server)");
}

fn now_ms() -> u64
{
    // Best-effort wall clock timestamp (used only for the hang watchdog).
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
    {
        Ok(d) => d.as_millis() as u64,
        Err(_) => 0,
    }
}

fn env_flag(name: &str) -> bool
{
    // Convention: most Stasis env toggles treat "1" as enabled.
    std::env::var(name).ok().as_deref() == Some("1")
}

fn env_u64(name: &str, default: u64) -> u64
{
    // Best-effort parse; keeps runner behavior stable even under unexpected env values.
    std::env::var(name).ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(default)
}

#[cfg(windows)]
#[unsafe(no_mangle)]
pub extern "win64" fn stasis_printf3(fmt: i64, a0: i64, _a1: i64) -> i32
{
    unsafe {
        stasis_printf3_impl(fmt as *const i8, a0, _a1)
    }
}

#[cfg(not(windows))]
#[unsafe(no_mangle)]
pub extern "C" fn stasis_printf3(fmt: i64, a0: i64, _a1: i64) -> i32
{
    unsafe {
        stasis_printf3_impl(fmt as *const libc::c_char, a0, _a1)
    }
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn stasis_printf3_impl(fmt: *const libc::c_char, arg1: i64, arg2: i64) -> i32
{
    if fmt.is_null()
    {
        return 0;
    }

    // Mirror runtime/stasis_sys.c behavior: support up to two format specifiers and pass
    // correctly-typed args to the C variadic printf implementation.
    //
    // This avoids ABI issues (Cranelift calls fixed-arity imports) and prevents UB where
    // integer args were previously treated as pointers.
    let mut spec_count = 0usize;
    let mut specs: [u8; 2] = [0, 0];
    let mut lens: [u8; 2] = [0, 0]; // 0=default, 1=l, 2=ll

    let mut p = fmt as *const u8;
    while *p != 0 && spec_count < 2
    {
        if *p != b'%'
        {
            p = p.add(1);
            continue;
        }

        p = p.add(1);
        if *p == 0
        {
            break;
        }
        if *p == b'%'
        {
            p = p.add(1);
            continue;
        }

        let mut len = 0u8;
        if *p == b'l'
        {
            len = 1;
            if *p.add(1) == b'l'
            {
                len = 2;
                p = p.add(1);
            }
            p = p.add(1);
        }

        let s = *p;
        if matches!(s, b'd' | b'i' | b'u' | b'x' | b'X' | b'c' | b's')
        {
            lens[spec_count] = len;
            specs[spec_count] = s;
            spec_count += 1;
        }

        p = p.add(1);
    }

    if spec_count == 0
    {
        return libc::printf(b"%s\0".as_ptr() as *const libc::c_char, fmt) as i32;
    }

    if spec_count == 1
    {
        match specs[0]
        {
            b's' => return libc::printf(fmt, arg1 as *const libc::c_char) as i32,
            b'c' => return libc::printf(fmt, arg1 as libc::c_int) as i32,
            b'u' | b'x' | b'X' =>
            {
                return match lens[0]
                {
                    2 => libc::printf(fmt, arg1 as libc::c_ulonglong) as i32,
                    1 => libc::printf(fmt, arg1 as libc::c_ulong) as i32,
                    _ => libc::printf(fmt, arg1 as libc::c_uint) as i32,
                };
            }
            b'd' | b'i' | _ =>
            {
                return match lens[0]
                {
                    2 => libc::printf(fmt, arg1 as libc::c_longlong) as i32,
                    1 => libc::printf(fmt, arg1 as libc::c_long) as i32,
                    _ => libc::printf(fmt, arg1 as libc::c_int) as i32,
                };
            }
        }
    }

    // spec_count == 2
    if specs[0] == b's' && specs[1] == b's'
    {
        return libc::printf(fmt, arg1 as *const libc::c_char, arg2 as *const libc::c_char) as i32;
    }
    if specs[0] == b's' && (specs[1] == b'd' || specs[1] == b'i')
    {
        return libc::printf(fmt, arg1 as *const libc::c_char, arg2 as libc::c_int) as i32;
    }
    if (specs[0] == b'd' || specs[0] == b'i') && (specs[1] == b'd' || specs[1] == b'i')
    {
        return libc::printf(fmt, arg1 as libc::c_int, arg2 as libc::c_int) as i32;
    }
    if specs[0] == b'c' && specs[1] == b'c'
    {
        return libc::printf(fmt, arg1 as libc::c_int, arg2 as libc::c_int) as i32;
    }

    libc::printf(fmt, arg1 as libc::c_longlong, arg2 as libc::c_longlong) as i32
}

#[unsafe(no_mangle)]
pub extern "C" fn stasis_sys_memcpy_u8(dst: i64, dst_index: i32, src: i64, src_index: i32, count: i32)
{
    if count <= 0
    {
        return;
    }

    unsafe {
        let dst = (dst as *mut u8).wrapping_add(dst_index.max(0) as usize);
        let src = (src as *const u8).wrapping_add(src_index.max(0) as usize);
        std::ptr::copy_nonoverlapping(src, dst, count as usize);
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn sinf(x: f32) -> f32
{
    x.sin()
}

#[unsafe(no_mangle)]
pub extern "C" fn cosf(x: f32) -> f32
{
    x.cos()
}

struct GraphicsApi
{
    _lib: Library,
    host_get_frame: Option<unsafe extern "C" fn(*mut i32, *mut f32)>,
    gfx_submit_u8: Option<unsafe extern "C" fn(*mut i32, *mut f32, *mut u8)>,
    host_bulk_init: Option<unsafe extern "C" fn(*const i32)>,
    host_bulk_apply_requests: Option<unsafe extern "C" fn(*const i32, *const i32, *const i32, *const i32)>,
    host_bulk_step: Option<
        unsafe extern "C" fn(
            *mut i32,
            *mut f32,
            *mut i32,
            *mut f32,
            *mut u8,
            *const i32,
            *const i32,
            *const i32,
            *const i32,
            extern "C" fn() -> i32,
        ) -> i32,
    >,
    set_window_size: Option<unsafe extern "C" fn(i32, i32)>,
    set_fullscreen: Option<unsafe extern "C" fn(i32) -> i32>,
    init_window: Option<unsafe extern "C" fn(i32, i32, *const i8) -> i32>,
}

fn try_load_stasis_graphics(jit_builder: &mut JITBuilder) -> Option<GraphicsApi>
{
    // In repo/dev runs, the CLI starts the runner with WorkingDirectory=repoRoot,
    // so loading by name resolves `.\stasis_graphics.dll` on Windows.
    let lib = unsafe { Library::new("stasis_graphics.dll") }.ok()?;

    fn try_get_fn<T: Copy>(lib: &Library, name: &str) -> Option<T>
    {
        // SAFETY: we only call through the function pointers with the expected signatures.
        let sym = unsafe { lib.get::<T>(name.as_bytes()) }.ok()?;
        Some(*sym)
    }

    // Register symbols for the JIT (for extern calls), and also capture a few host-side APIs.
    //
    // Important: a missing host symbol often only triggers on specific gameplay/UI paths
    // (e.g., audio init on click). Keep this list in sync with what the compiler emits.
    for name in [
        "stasis_host_get_frame",
        "stasis_gfx_submit_u8",
        "stasis_host_bulk_init",
        "stasis_host_bulk_apply_requests",
        "stasis_host_bulk_step",
        "stasis_set_window_size",
        "stasis_set_fullscreen",
        "stasis_init_window",
        "stasis_gfx_load_sprite",
        "stasis_load_font",
        "stasis_get_time_ms",
        "stasis_get_time_us",
        "stasis_audio_init",
        "stasis_audio_shutdown",
        "stasis_audio_is_available",
        "stasis_audio_get_sample_rate",
        "stasis_audio_get_channels",
        "stasis_audio_get_queued_frames",
        "stasis_audio_get_underruns",
        "stasis_audio_push_f32_interleaved",
    ]
    {
        unsafe {
            if let Ok(sym) = lib.get::<*const c_void>(name.as_bytes())
            {
                jit_builder.symbol(name, *sym as *const u8);
            }
        }
    }

    let api = GraphicsApi {
        host_get_frame: try_get_fn::<unsafe extern "C" fn(*mut i32, *mut f32)>(&lib, "stasis_host_get_frame"),
        gfx_submit_u8: try_get_fn::<unsafe extern "C" fn(*mut i32, *mut f32, *mut u8)>(&lib, "stasis_gfx_submit_u8"),
        host_bulk_init: try_get_fn::<unsafe extern "C" fn(*const i32)>(&lib, "stasis_host_bulk_init"),
        host_bulk_apply_requests: try_get_fn::<unsafe extern "C" fn(*const i32, *const i32, *const i32, *const i32)>(
            &lib,
            "stasis_host_bulk_apply_requests",
        ),
        host_bulk_step: try_get_fn::<
            unsafe extern "C" fn(
                *mut i32,
                *mut f32,
                *mut i32,
                *mut f32,
                *mut u8,
                *const i32,
                *const i32,
                *const i32,
                *const i32,
                extern "C" fn() -> i32,
            ) -> i32,
        >(&lib, "stasis_host_bulk_step"),
        set_window_size: try_get_fn::<unsafe extern "C" fn(i32, i32)>(&lib, "stasis_set_window_size"),
        set_fullscreen: try_get_fn::<unsafe extern "C" fn(i32) -> i32>(&lib, "stasis_set_fullscreen"),
        init_window: try_get_fn::<unsafe extern "C" fn(i32, i32, *const i8) -> i32>(&lib, "stasis_init_window"),
        _lib: lib,
    };

    Some(api)
}

enum Request
{
    Init {
        module_name: String,
        entry_name: String,
        tick_name: String,
        clif: String,
    },
    Bind {
        json_path: String,
        struct_meta_path: String,
    },
    Swap { clif: String },
    Quit,
}

#[derive(Clone)]
struct DataBinding
{
    json_path: String,
    fields: Vec<StructFieldMeta>,
    last_modified: Option<std::time::SystemTime>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StructMetaFile
{
    version: i32,
    fields: Vec<StructFieldMeta>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StructFieldMeta
{
    name: String,
    json_path: String,
    size: i32,
    #[serde(rename = "type")]
    field_type: String,
    array_count: i32,
}

impl DataBinding
{
    fn load(json_path: String, struct_meta_path: String) -> Result<Self>
    {
        let meta_bytes = std::fs::read(&struct_meta_path)
            .with_context(|| format!("failed to read struct meta: {struct_meta_path}"))?;
        // The CLI writes struct-meta as UTF-8 without BOM. Be tolerant anyway: some Windows tools
        // (or legacy CLI builds) may produce a BOM, which serde_json rejects.
        let meta_bytes = meta_bytes
            .strip_prefix(&[0xEF, 0xBB, 0xBF])
            .unwrap_or(meta_bytes.as_slice());
        let meta: StructMetaFile =
            serde_json::from_slice(&meta_bytes).with_context(|| format!("failed to parse struct meta: {struct_meta_path}"))?;
        if meta.version != 1
        {
            bail!("unsupported struct meta version {} (expected 1)", meta.version);
        }

        Ok(Self { json_path, fields: meta.fields, last_modified: None })
    }

    fn apply_if_changed(&mut self, instance: &mut JitInstance) -> Result<bool>
    {
        let meta = std::fs::metadata(&self.json_path).ok();
        let modified = meta.and_then(|m| m.modified().ok());

        if modified.is_none()
        {
            return Ok(false);
        }

        if self.last_modified.is_some() && modified == self.last_modified
        {
            return Ok(false);
        }

        self.last_modified = modified;
        self.apply(instance)?;
        Ok(true)
    }

    fn apply(&self, instance: &mut JitInstance) -> Result<()>
    {
        let json_bytes =
            std::fs::read(&self.json_path).with_context(|| format!("failed to read data file: {}", self.json_path))?;
        let root: JsonValue =
            serde_json::from_slice(&json_bytes).with_context(|| format!("failed to parse json: {}", self.json_path))?;

        for field in &self.fields
        {
            let Some(dest) = instance.data_ptrs.get(&field.name).copied() else {
                continue;
            };

            // Prefer jsonPath; fall back to flattened symbol name as key.
            let mut value = json_get_by_path(&root, &field.json_path);
            if value.is_none() && field.json_path != field.name
            {
                value = json_get_by_path(&root, &field.name);
            }

            if value.is_none() && field.array_count > 1
            {
                // AoS-style arrays: "asteroids.x" where "asteroids" is array of objects.
                if let Some((base_path, leaf)) = field.json_path.rsplit_once('.')
                {
                    if let Some(base_value) = json_get_by_path(&root, base_path)
                    {
                        if let Some(arr) = base_value.as_array()
                        {
                            let elem_bytes = field_element_bytes(field)?;
                            let limit = arr.len().min(field.array_count.max(0) as usize);
                            for i in 0..limit
                            {
                                if let Some(obj) = arr[i].as_object()
                                {
                                    if let Some(v) = obj.get(leaf)
                                    {
                                        unsafe {
                                            apply_scalar_to_dest(field, dest.add(i * elem_bytes), v);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
                continue;
            }

            let Some(value) = value else { continue };

            if field.array_count > 1 && value.is_array()
            {
                let elem_bytes = field_element_bytes(field)?;
                let arr = value.as_array().unwrap();
                let limit = arr.len().min(field.array_count.max(0) as usize);
                for i in 0..limit
                {
                    let v = &arr[i];
                    unsafe {
                        apply_scalar_to_dest(field, dest.add(i * elem_bytes), v);
                    }
                }
            }
            else
            {
                unsafe {
                    apply_scalar_to_dest(field, dest, value);
                }
            }
        }

        Ok(())
    }
}

fn json_get_by_path<'a>(root: &'a JsonValue, path: &str) -> Option<&'a JsonValue>
{
    let mut current = root;
    for part in path.split('.').map(|s| s.trim()).filter(|s| !s.is_empty())
    {
        match current
        {
            JsonValue::Object(map) => {
                current = map.get(part)?;
            }
            _ => return None,
        }
    }
    Some(current)
}

fn field_element_bytes(field: &StructFieldMeta) -> Result<usize>
{
    if field.array_count > 1 && field.size > 0
    {
        let elem = field.size / field.array_count;
        if elem > 0 && (elem * field.array_count) == field.size
        {
            return Ok(elem as usize);
        }
    }

    Ok(match field.field_type.as_str()
    {
        "bool" | "u8" => 1,
        "u16" => 2,
        "u32" | "i32" | "f32" => 4,
        "f64" => 8,
        // strings are special: treat element as 1 byte in array cases; scalar path handles full header+payload.
        "string" => 1,
        _ => 0,
    })
}

#[allow(unsafe_op_in_unsafe_fn)]
unsafe fn apply_scalar_to_dest(field: &StructFieldMeta, dest: *mut u8, value: &JsonValue)
{
    match field.field_type.as_str()
    {
        "bool" => {
            let v = if let Some(b) = value.as_bool() { b } else { value.as_i64().unwrap_or(0) != 0 };
            *(dest as *mut u8) = if v { 1 } else { 0 };
        }
        "u8" => {
            if let Some(n) = value.as_i64() { *(dest as *mut u8) = n as u8; }
        }
        "u16" => {
            if let Some(n) = value.as_i64() { *(dest as *mut u16) = n as u16; }
        }
        "u32" => {
            if let Some(n) = value.as_i64() { *(dest as *mut u32) = n as u32; }
        }
        "i32" => {
            if let Some(n) = value.as_i64() { *(dest as *mut i32) = n as i32; }
        }
        "f32" => {
            if let Some(n) = value.as_f64() { *(dest as *mut f32) = n as f32; }
        }
        "f64" => {
            if let Some(n) = value.as_f64() { *(dest as *mut f64) = n; }
        }
        "string" => {
            // Matches runtime/stasis_data.c layout:
            // ascii[N]: [byte_len:i32][u8[N]] (header_bytes=4)
            // utf8[N]: [byte_len:i32][char_len:i32][u8[N]] (header_bytes=8)
            let Some(s) = value.as_str() else { return; };
            if field.array_count <= 1 || field.size <= 0 || field.array_count > field.size
            {
                return;
            }
            let header_bytes = field.size - field.array_count;
            if header_bytes != 4 && header_bytes != 8
            {
                return;
            }
            let payload = dest.add(header_bytes as usize);
            let payload_cap = field.array_count as usize;
            if payload_cap == 0 { return; }

            let max_copy = payload_cap.saturating_sub(1);
            let bytes = s.as_bytes();

            if header_bytes == 4
            {
                let mut copy_len = 0usize;
                for &b in bytes.iter().take(max_copy)
                {
                    if b == 0 || b >= 0x80 { break; }
                    *payload.add(copy_len) = b;
                    copy_len += 1;
                }
                if copy_len < payload_cap { *payload.add(copy_len) = 0; }
                *(dest as *mut i32) = copy_len as i32;
                return;
            }

            // UTF-8: bounded copy, best-effort (assumes input is valid UTF-8).
            let mut copy_bytes = 0usize;
            for &b in bytes.iter().take(max_copy)
            {
                if b == 0 { break; }
                *payload.add(copy_bytes) = b;
                copy_bytes += 1;
            }
            if copy_bytes < payload_cap { *payload.add(copy_bytes) = 0; }
            *(dest as *mut i32) = copy_bytes as i32;
            *(dest.add(4) as *mut i32) = s.chars().count().min(i32::MAX as usize) as i32;
        }
        _ => {}
    }
}

struct SwapJob
{
    id: u64,
    module_name: String,
    entry_name: String,
    tick_name: String,
    clif: String,
}

enum SwapOutcome
{
    Ok { instance: JitInstance, compile_us: u64 },
    Err { message: String },
}

struct SwapResult
{
    id: u64,
    outcome: SwapOutcome,
}

fn run_server(fps: u32) -> Result<()>
{
    let mut stdout = io::stdout();

    writeln!(stdout, "READY")?;
    stdout.flush()?;

    let (tx, rx) = mpsc::channel::<Request>();
    spawn_request_reader(tx)?;

    loop
    {
        let req = rx.recv().context("request channel closed")?;
        match req
        {
            Request::Init { module_name, entry_name, tick_name, clif } =>
            {
                let compile_result = JitInstance::compile(&module_name, &entry_name, &tick_name, &clif);
                let mut new_instance = match compile_result
                {
                    Ok(v) => v,
                    Err(e) =>
                    {
                        writeln!(stdout, "ERR init compile failed: {:#}", e)?;
                        stdout.flush()?;
                        continue;
                    }
                };

                // Initialize bulk host request tracking before main(), matching stasis_runner behavior.
                if new_instance.bulk_active
                {
                    if let (Some(g), Some(init), Some(req_seq)) =
                        (new_instance.graphics.as_ref(), new_instance.graphics.as_ref().and_then(|x| x.host_bulk_init), new_instance.host_req_seq)
                    {
                        let _ = g;
                        unsafe {
                            init(req_seq as *const i32);
                        }
                    }
                }

                let rc = (new_instance.entry_fn)();
                if rc != 0
                {
                    writeln!(stdout, "ERR entry returned {rc}")?;
                    stdout.flush()?;
                    continue;
                }

                writeln!(stdout, "OK init")?;
                stdout.flush()?;

                let tick_loop_result = run_tick_loop(fps, &mut new_instance, &rx, &mut stdout);
                return tick_loop_result;
            }
            Request::Bind { .. } =>
            {
                writeln!(stdout, "ERR databind before init")?;
                stdout.flush()?;
            }
            Request::Swap { .. } =>
            {
                writeln!(stdout, "ERR swap before init")?;
                stdout.flush()?;
            }
            Request::Quit =>
            {
                writeln!(stdout, "OK quit")?;
                stdout.flush()?;
                break;
            }
        }
    }

    Ok(())
}

fn run_tick_loop(fps: u32, instance: &mut JitInstance, rx: &mpsc::Receiver<Request>, stdout: &mut io::Stdout) -> Result<()>
{
    let target_dt = if fps == 0 { Duration::from_millis(16) } else { Duration::from_secs_f64(1.0 / (fps as f64)) };
    let mut last = Instant::now();

    let watchdog_ms = env_u64("STASIS_JIT_WATCHDOG_MS", 0);

    let last_progress_ms = Arc::new(AtomicU64::new(now_ms()));
    let tick_counter = Arc::new(AtomicU64::new(0));
    let swap_counter = Arc::new(AtomicU64::new(0));
    let stage = Arc::new(AtomicU32::new(0));

    fn stage_name(stage: u32) -> &'static str
    {
        match stage
        {
            0 => "idle",
            1 => "rx",
            2 => "databind",
            3 => "host_get_frame",
            4 => "apply_window_requests",
            5 => "tick",
            6 => "gfx_submit",
            7 => "bulk_step",
            _ => "unknown",
        }
    }

    if watchdog_ms > 0
    {
        let last_progress_ms = Arc::clone(&last_progress_ms);
        let tick_counter = Arc::clone(&tick_counter);
        let stage = Arc::clone(&stage);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(250));
            let last_ms = last_progress_ms.load(Ordering::Relaxed);
            let now = now_ms();
            if now.saturating_sub(last_ms) > watchdog_ms
            {
                eprintln!(
                    "error: jit runner watchdog fired (no tick progress for {}ms; ticks={}; stage={}({})); exiting so watch can restart.",
                    now.saturating_sub(last_ms),
                    tick_counter.load(Ordering::Relaxed),
                    stage.load(Ordering::Relaxed),
                    stage_name(stage.load(Ordering::Relaxed))
                );
                std::process::exit(124);
            }
        });
    }

    let heartbeat_ms = env_u64("STASIS_JIT_HEARTBEAT_MS", 0);

    if heartbeat_ms > 0
    {
        let tick_counter = Arc::clone(&tick_counter);
        let swap_counter = Arc::clone(&swap_counter);
        let stage = Arc::clone(&stage);
        thread::spawn(move || loop {
            thread::sleep(Duration::from_millis(heartbeat_ms.max(50)));
            eprintln!(
                "HEARTBEAT ticks={} swaps={} stage={}({})",
                tick_counter.load(Ordering::Relaxed),
                swap_counter.load(Ordering::Relaxed),
                stage.load(Ordering::Relaxed),
                stage_name(stage.load(Ordering::Relaxed))
            );
        });
    }

    // Compile swaps off-thread so the tick loop stays responsive (UI/events continue to pump).
    // The tick loop only does a fast apply step + pointer swap when compilation finishes.
    let (swap_job_tx, swap_job_rx) = mpsc::channel::<SwapJob>();
    let (swap_result_tx, swap_result_rx) = mpsc::channel::<SwapResult>();
    let latest_requested_id = Arc::new(AtomicU64::new(0));
    let latest_requested_id_worker = Arc::clone(&latest_requested_id);
    thread::spawn(move || {
        while let Ok(job) = swap_job_rx.recv()
        {
            // Best-effort coalescing: if a newer swap request arrived before we started compiling this job,
            // skip the work entirely. This keeps iteration responsive when edits arrive faster than compile time.
            if job.id != latest_requested_id_worker.load(Ordering::Relaxed)
            {
                continue;
            }

            let t0 = Instant::now();
            let outcome = match JitInstance::compile(&job.module_name, &job.entry_name, &job.tick_name, &job.clif)
            {
                Ok(instance) => SwapOutcome::Ok { instance, compile_us: t0.elapsed().as_micros() as u64 },
                Err(e) => SwapOutcome::Err { message: format!("{:#}", e) },
            };
            let _ = swap_result_tx.send(SwapResult { id: job.id, outcome });
        }
    });

    // Drop old instances off-thread as well (tearing down the previous JIT module can be expensive).
    // This prevents sporadic pauses on swap when JITModule gets freed.
    let (drop_tx, drop_rx) = mpsc::channel::<JitInstance>();
    thread::spawn(move || {
        while let Ok(v) = drop_rx.recv()
        {
            drop(v);
        }
    });

    let mut next_swap_id: u64 = 1;
    let mut pending_swap_id: Option<u64> = None;
    let mut data_binding: Option<DataBinding> = None;
    let mut last_req_seq: i32 = 0;

    // Bulk host mode: create a default window if the graphics runtime is present.
    if instance.bulk_active
    {
        if let Some(g) = instance.graphics.as_ref()
        {
            if let Some(init_window) = g.init_window
            {
                let title = b"Stasis\0";
                unsafe {
                    let _ = init_window(640, 360, title.as_ptr() as *const i8);
                }
            }
        }

        let bulk_step_available = instance.graphics.as_ref().and_then(|g| g.host_bulk_step).is_some();
        let use_bulk_step = env_flag("STASIS_JIT_USE_HOST_BULK_STEP");

        if bulk_step_available && use_bulk_step
        {
            eprintln!("HOST bulk: active (stasis_host_bulk_step)");
        }
        else
        {
            if bulk_step_available && !use_bulk_step
            {
                eprintln!("HOST bulk: stasis_host_bulk_step available but disabled (set STASIS_JIT_USE_HOST_BULK_STEP=1 to enable)");
            }
            eprintln!("HOST bulk: active (host_get_frame + gfx_submit_u8)");
        }
    }

    let bulk_step_available = instance.graphics.as_ref().and_then(|g| g.host_bulk_step).is_some();
    let use_bulk_step = env_flag("STASIS_JIT_USE_HOST_BULK_STEP");

    loop
    {
        last_progress_ms.store(now_ms(), Ordering::Relaxed);

        while let Ok(result) = swap_result_rx.try_recv()
        {
            // If multiple results arrive (shouldn't happen with the watch protocol), apply the latest matching swap.
            if pending_swap_id != Some(result.id)
            {
                // Drop compiled instances we didn't apply.
                if let SwapOutcome::Ok { instance: compiled, .. } = result.outcome
                {
                    let _ = drop_tx.send(compiled);
                }
                continue;
            }

            match result.outcome
            {
                SwapOutcome::Ok { instance: mut new_instance, compile_us } =>
                {
                    let t0 = Instant::now();
                    let (missing_save, save_bytes) = instance.save_state();
                    let save_us = t0.elapsed().as_micros() as u64;

                    let t2 = Instant::now();
                    let missing_restore = new_instance.restore_state(save_bytes);
                    let restore_us = t2.elapsed().as_micros() as u64;

                    if let Some(b) = data_binding.as_ref()
                    {
                        if let Err(e) = b.apply(&mut new_instance)
                        {
                            eprintln!("DATABIND error: {:#}", e);
                        }
                    }

                    let bytes: usize = new_instance.state_globals.iter().map(|(_, sz)| *sz).sum();
                    let symbols = new_instance.state_globals.len();
                    eprintln!(
                        "HOTSWAP ok: save={}us load={}us tick=0us restore={}us bytes={} symbols={}",
                        save_us, compile_us, restore_us, bytes, symbols
                    );
                    if missing_save > 0 || missing_restore > 0
                    {
                        eprintln!(
                            "HOTSWAP warning: state layout changed (missing save={} restore={}); consider restarting to resync state.",
                            missing_save, missing_restore
                        );
                    }

                    let old = std::mem::replace(instance, new_instance);
                    let _ = drop_tx.send(old);
                    swap_counter.fetch_add(1, Ordering::Relaxed);

                    writeln!(
                        stdout,
                        "APPLIED id={} save_us={} compile_us={} restore_us={} missing_save={} missing_restore={}",
                        result.id, save_us, compile_us, restore_us, missing_save, missing_restore
                    )?;
                    stdout.flush()?;
                }
                SwapOutcome::Err { message } =>
                {
                    eprintln!("HOTSWAP error: {}", message);
                    writeln!(stdout, "ERR swap compile failed: {}", message.replace('\n', " "))?;
                    stdout.flush()?;
                }
            }

            pending_swap_id = None;
            last_progress_ms.store(now_ms(), Ordering::Relaxed);
        }

        while let Ok(req) = rx.try_recv()
        {
            match req
            {
                Request::Bind { json_path, struct_meta_path } =>
                {
                    match DataBinding::load(json_path.clone(), struct_meta_path.clone())
                    {
                        Ok(b) =>
                        {
                            if let Err(e) = b.apply(instance)
                            {
                                eprintln!("DATABIND error: {:#}", e);
                                writeln!(stdout, "ERR databind apply failed: {}", format!("{:#}", e).replace('\n', " "))?;
                                stdout.flush()?;
                                continue;
                            }

                            eprintln!("DATABIND: registered {} ({} fields)", json_path, b.fields.len());
                            data_binding = Some(b);
                            writeln!(stdout, "OK")?;
                            stdout.flush()?;
                        }
                        Err(e) =>
                        {
                            eprintln!("DATABIND error: {:#}", e);
                            writeln!(stdout, "ERR databind load failed: {}", format!("{:#}", e).replace('\n', " "))?;
                            stdout.flush()?;
                        }
                    }
                }
                Request::Swap { clif } =>
                {
                    let id = next_swap_id;
                    next_swap_id = next_swap_id.saturating_add(1);
                    let supersedes = pending_swap_id.replace(id).unwrap_or(0);
                    latest_requested_id.store(id, Ordering::Relaxed);

                    eprintln!("HOTSWAP queued: id={} bytes={} supersedes={}", id, clif.len(), supersedes);
                    let job = SwapJob {
                        id,
                        module_name: instance.module_name.clone(),
                        entry_name: instance.entry_name.clone(),
                        tick_name: instance.tick_name.clone(),
                        clif,
                    };
                    if swap_job_tx.send(job).is_err()
                    {
                        pending_swap_id = None;
                        writeln!(stdout, "ERR swap worker unavailable")?;
                        stdout.flush()?;
                        continue;
                    }

                    // Acknowledge receipt immediately; compilation happens asynchronously.
                    writeln!(stdout, "QUEUED id={} supersedes={}", id, supersedes)?;
                    stdout.flush()?;
                }
                Request::Quit =>
                {
                    writeln!(stdout, "OK")?;
                    stdout.flush()?;
                    return Ok(());
                }
                Request::Init { .. } =>
                {
                    writeln!(stdout, "ERR already initialized")?;
                    stdout.flush()?;
                }
            }
        }

        if let Some(b) = data_binding.as_mut()
        {
            stage.store(2, Ordering::Relaxed);
            if b.apply_if_changed(instance).unwrap_or(false)
            {
                eprintln!("DATABIND: reloaded {}", b.json_path);
            }
        }

        if instance.bulk_active
        {
            if bulk_step_available && use_bulk_step
            {
                if let (Some(host_bulk_step), Some(host_i32), Some(host_f32), Some(cmd_i32), Some(cmd_f32), Some(cmd_u8)) =
                    (
                        instance.graphics.as_ref().and_then(|x| x.host_bulk_step),
                        instance.host_i32,
                        instance.host_f32,
                        instance.gfx_cmd_i32,
                        instance.gfx_cmd_f32,
                        instance.gfx_cmd_u8,
                    )
                {
                    unsafe {
                        stage.store(7, Ordering::Relaxed);
                        last_progress_ms.store(now_ms(), Ordering::Relaxed);
                        let rc = host_bulk_step(
                            host_i32,
                            host_f32,
                            cmd_i32,
                            cmd_f32,
                            cmd_u8,
                            instance.host_req_seq.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                            instance.host_req_flags.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                            instance.host_req_window_w_px.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                            instance.host_req_window_h_px.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                            instance.tick_fn,
                        );
                        if rc != 0
                        {
                            eprintln!("HOST bulk: step returned {rc}; exiting.");
                            return Ok(());
                        }
                    }
                    tick_counter.fetch_add(1, Ordering::Relaxed);
                    last_progress_ms.store(now_ms(), Ordering::Relaxed);
                }
            }
            else
            {
                // Legacy bulk mode: mimic stasis_runner behavior using host_get_frame + submit.
                if let (Some(host_get_frame), Some(host_i32), Some(host_f32)) =
                    (instance.graphics.as_ref().and_then(|x| x.host_get_frame), instance.host_i32, instance.host_f32)
                {
                    unsafe {
                        stage.store(3, Ordering::Relaxed);
                        last_progress_ms.store(now_ms(), Ordering::Relaxed);
                        host_get_frame(host_i32, host_f32);
                        if *host_i32.add(9) != 0
                        {
                            return Ok(());
                        }
                    }
                }

                stage.store(4, Ordering::Relaxed);
                apply_window_requests(instance, &mut last_req_seq);

                stage.store(5, Ordering::Relaxed);
                last_progress_ms.store(now_ms(), Ordering::Relaxed);
                let rc = (instance.tick_fn)();
                if rc != 0
                {
                    eprintln!("tick returned {rc}; exiting.");
                    return Ok(());
                }

                if let (Some(gfx_submit_u8), Some(cmd_i32), Some(cmd_f32), Some(cmd_u8)) =
                    (instance.graphics.as_ref().and_then(|x| x.gfx_submit_u8), instance.gfx_cmd_i32, instance.gfx_cmd_f32, instance.gfx_cmd_u8)
                {
                    unsafe {
                        stage.store(6, Ordering::Relaxed);
                        last_progress_ms.store(now_ms(), Ordering::Relaxed);
                        gfx_submit_u8(cmd_i32, cmd_f32, cmd_u8);
                    }
                }
            }
        }
        else
        {
            stage.store(5, Ordering::Relaxed);
            last_progress_ms.store(now_ms(), Ordering::Relaxed);
            let rc = (instance.tick_fn)();
            if rc != 0
            {
                eprintln!("tick returned {rc}; exiting.");
                return Ok(());
            }
        }

        if !(instance.bulk_active && bulk_step_available && use_bulk_step)
        {
            tick_counter.fetch_add(1, Ordering::Relaxed);
            last_progress_ms.store(now_ms(), Ordering::Relaxed);
        }

        let now = Instant::now();
        let elapsed = now.saturating_duration_since(last);
        last = now;
        if elapsed < target_dt
        {
            thread::sleep(target_dt - elapsed);
        }
    }
}

fn apply_window_requests(instance: &mut JitInstance, last_req_seq: &mut i32)
{
    // Prefer the shared implementation in stasis_graphics.dll when available.
    if let Some(g) = instance.graphics.as_ref()
    {
        if let Some(apply) = g.host_bulk_apply_requests
        {
            unsafe {
                apply(
                    instance.host_req_seq.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                    instance.host_req_flags.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                    instance.host_req_window_w_px.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                    instance.host_req_window_h_px.map(|p| p as *const i32).unwrap_or(std::ptr::null()),
                );
            }
            return;
        }
    }

    const HOST_REQ_FLAG_WINDOWED: i32 = 1;
    const HOST_REQ_FLAG_FULLSCREEN: i32 = 2;

    if !instance.bulk_active
    {
        return;
    }

    let (Some(req_seq), Some(req_flags)) = (instance.host_req_seq, instance.host_req_flags) else { return };
    let Some(g) = instance.graphics.as_ref() else { return };
    let Some(set_fullscreen) = g.set_fullscreen else { return };

    unsafe {
        let seq = *req_seq;
        if seq == 0 || seq == *last_req_seq
        {
            return;
        }
        *last_req_seq = seq;

        let flags = *req_flags;
        if (flags & HOST_REQ_FLAG_WINDOWED) != 0
        {
            if let (Some(set_window_size), Some(wp), Some(hp)) = (g.set_window_size, instance.host_req_window_w_px, instance.host_req_window_h_px)
            {
                let _ = set_fullscreen(0);
                set_window_size(*wp, *hp);
            }
        }
        else if (flags & HOST_REQ_FLAG_FULLSCREEN) != 0
        {
            let _ = set_fullscreen(1);
        }
    }
}

fn spawn_request_reader(tx: mpsc::Sender<Request>) -> Result<()>
{
    let tx2 = tx.clone();

    thread::spawn(move || {
        let stdin = io::stdin();
        let mut reader = BufReader::new(stdin.lock());
        let mut line = String::new();
        loop
        {
            line.clear();
            let bytes = reader.read_line(&mut line).unwrap_or(0);
            if bytes == 0
            {
                eprintln!("stasis-cranelift-jit-runner: stdin EOF (request reader exiting)");
                let _ = tx2.send(Request::Quit);
                break;
            }

            let trimmed = line.trim();
            if trimmed.is_empty()
            {
                continue;
            }

            if trimmed.eq_ignore_ascii_case("QUIT")
            {
                let _ = tx2.send(Request::Quit);
                break;
            }

            if trimmed.starts_with("INIT ")
            {
                let parts: Vec<_> = trimmed.split_whitespace().collect();
                if parts.len() != 5
                {
                    eprintln!("stasis-cranelift-jit-runner: bad INIT header (expected 5 parts): '{trimmed}'");
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let module_len = parts[1].parse::<usize>().unwrap_or(0);
                let entry_len = parts[2].parse::<usize>().unwrap_or(0);
                let tick_len = parts[3].parse::<usize>().unwrap_or(0);
                let clif_len = parts[4].parse::<usize>().unwrap_or(0);
                if module_len == 0 || entry_len == 0 || tick_len == 0 || clif_len == 0
                {
                    eprintln!(
                        "stasis-cranelift-jit-runner: bad INIT lengths module={module_len} entry={entry_len} tick={tick_len} clif={clif_len} (line='{trimmed}')"
                    );
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let module_name = read_string(&mut reader, module_len).unwrap_or_default();
                let entry_name = read_string(&mut reader, entry_len).unwrap_or_default();
                let tick_name = read_string(&mut reader, tick_len).unwrap_or_default();
                let clif = read_string(&mut reader, clif_len).unwrap_or_default();
                let _ = tx2.send(Request::Init { module_name, entry_name, tick_name, clif });
                continue;
            }

            if trimmed.starts_with("SWAP ")
            {
                let parts: Vec<_> = trimmed.split_whitespace().collect();
                if parts.len() != 2
                {
                    eprintln!("stasis-cranelift-jit-runner: bad SWAP header (expected 2 parts): '{trimmed}'");
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let clif_len = parts[1].parse::<usize>().unwrap_or(0);
                if clif_len == 0
                {
                    eprintln!("stasis-cranelift-jit-runner: bad SWAP length clif={clif_len} (line='{trimmed}')");
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let clif = read_string(&mut reader, clif_len).unwrap_or_default();
                let _ = tx2.send(Request::Swap { clif });
                continue;
            }

            if trimmed.starts_with("BIND ")
            {
                let parts: Vec<_> = trimmed.split_whitespace().collect();
                if parts.len() != 3
                {
                    eprintln!("stasis-cranelift-jit-runner: bad BIND header (expected 3 parts): '{trimmed}'");
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let json_len = parts[1].parse::<usize>().unwrap_or(0);
                let meta_len = parts[2].parse::<usize>().unwrap_or(0);
                if json_len == 0 || meta_len == 0
                {
                    eprintln!(
                        "stasis-cranelift-jit-runner: bad BIND lengths json={json_len} meta={meta_len} (line='{trimmed}')"
                    );
                    let _ = tx2.send(Request::Quit);
                    break;
                }
                let json_path = read_string(&mut reader, json_len).unwrap_or_default();
                let struct_meta_path = read_string(&mut reader, meta_len).unwrap_or_default();
                let _ = tx2.send(Request::Bind { json_path, struct_meta_path });
                continue;
            }
        }
    });

    Ok(())
}

fn read_string<R: Read>(reader: &mut R, len: usize) -> Result<String>
{
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    let s = String::from_utf8(buf).context("invalid UTF-8")?;
    Ok(s)
}

struct JitInstance
{
    module_name: String,
    entry_name: String,
    tick_name: String,
    graphics: Option<GraphicsApi>,
    _module: JITModule,
    entry_fn: extern "C" fn() -> i32,
    tick_fn: extern "C" fn() -> i32,
    state_globals: Vec<(String, usize)>,
    data_ptrs: HashMap<String, *mut u8>,

    // Bulk host mode: host writes HostFrame globals and submits gfx_cmd buffers (via stasis_graphics bulk API).
    bulk_active: bool,
    host_i32: Option<*mut i32>,
    host_f32: Option<*mut f32>,
    gfx_cmd_i32: Option<*mut i32>,
    gfx_cmd_f32: Option<*mut f32>,
    gfx_cmd_u8: Option<*mut u8>,
    host_req_seq: Option<*mut i32>,
    host_req_flags: Option<*mut i32>,
    host_req_window_w_px: Option<*mut i32>,
    host_req_window_h_px: Option<*mut i32>,
}

// SAFETY: JitInstance contains raw pointers to JIT-owned memory. We only move whole instances
// across threads (via channels) and ensure the active instance is used on a single thread at a time.
unsafe impl Send for JitInstance {}

impl JitInstance
{
    fn compile(module_name: &str, entry_name: &str, tick_name: &str, clif: &str) -> Result<Self>
    {
        let triple = Triple::host();
        let flags = build_flags("speed", &triple)?;
        let isa = cranelift_native::builder()
            .map_err(|e| anyhow::anyhow!("failed to create native ISA builder: {e}"))?
            .finish(flags)
            .context("failed to finish native ISA")?;
        let default_cc = isa.default_call_conv();

        let mut jit_builder = JITBuilder::with_isa(isa, default_libcall_names());
        jit_builder.symbol("printf", stasis_printf3 as *const u8);
        jit_builder.symbol("stasis_printf3", stasis_printf3 as *const u8);
        jit_builder.symbol("stasis_sys_memcpy_u8", stasis_sys_memcpy_u8 as *const u8);
        jit_builder.symbol("sinf", sinf as *const u8);
        jit_builder.symbol("cosf", cosf as *const u8);

        let graphics = try_load_stasis_graphics(&mut jit_builder);

        let mut module = JITModule::new(jit_builder);

        let parsed = stasis_clif::parse_stasis_clif(clif, default_cc).context("failed to parse stasis CLIF")?;

        let mut data_ptrs = HashMap::new();
        let mut state_globals = Vec::new();
        let mut data_ids = HashMap::new();
        for g in &parsed.globals
        {
            let id = module
                .declare_data(&g.name, Linkage::Export, true, false)
                .with_context(|| format!("declare_data failed for {}", g.name))?;
            data_ids.insert(g.name.clone(), id);

            let mut data_desc = DataDescription::new();
            match &g.init_data
            {
                stasis_clif::GlobalInitData::Zero => data_desc.define_zeroinit(g.size_bytes),
                stasis_clif::GlobalInitData::String(bytes) => data_desc.define(bytes.clone().into_boxed_slice()),
            }
            module
                .define_data(id, &data_desc)
                .with_context(|| format!("define_data failed for {}", g.name))?;

            if g.name.starts_with("state__")
            {
                state_globals.push((g.name.clone(), g.size_bytes));
            }
        }

        let mut function_ids = HashMap::new();
        for ext in &parsed.externals
        {
            let link_name = if ext.name == "printf_str" || ext.name == "printf3" { "printf" } else { &ext.name };
            let id = module
                .declare_function(link_name, Linkage::Import, &ext.signature)
                .with_context(|| format!("declare external function failed for {}", ext.name))?;
            function_ids.insert(ext.name.clone(), id);
        }

        for f in &parsed.functions
        {
            let id = module
                .declare_function(&f.name, Linkage::Export, &f.signature)
                .with_context(|| format!("declare_function failed for {}", f.name))?;
            function_ids.insert(f.name.clone(), id);
        }

        for f in parsed.functions
        {
            let mut context = module.make_context();
            context.func = stasis_clif::build_function_ir(&mut module, &function_ids, &data_ids, &f)
                .with_context(|| format!("failed to build IR for {}", f.name))?;
            let id = *function_ids.get(&f.name).context("missing function id")?;
            module
                .define_function(id, &mut context)
                .with_context(|| format!("define_function failed for {}", f.name))?;
            module.clear_context(&mut context);
        }

        module.finalize_definitions().context("failed to finalize JIT definitions")?;

        for (name, id) in &data_ids
        {
            let ptr = module.get_finalized_data(*id);
            data_ptrs.insert(name.clone(), ptr.0 as *mut u8);
        }

        let host_i32 = data_ptrs.get("host_i32").copied().map(|p| p as *mut i32);
        let host_f32 = data_ptrs.get("host_f32").copied().map(|p| p as *mut f32);
        let gfx_cmd_i32 = data_ptrs.get("gfx_cmd_i32").copied().map(|p| p as *mut i32);
        let gfx_cmd_f32 = data_ptrs.get("gfx_cmd_f32").copied().map(|p| p as *mut f32);
        let gfx_cmd_u8 = data_ptrs.get("gfx_cmd_u8").copied().map(|p| p as *mut u8);
        let host_req_seq = data_ptrs.get("host_req_seq").copied().map(|p| p as *mut i32);
        let host_req_flags = data_ptrs.get("host_req_flags").copied().map(|p| p as *mut i32);
        let host_req_window_w_px = data_ptrs.get("host_req_window_w_px").copied().map(|p| p as *mut i32);
        let host_req_window_h_px = data_ptrs.get("host_req_window_h_px").copied().map(|p| p as *mut i32);

        let bulk_active =
            host_i32.is_some()
            && host_f32.is_some()
            && gfx_cmd_i32.is_some()
            && gfx_cmd_f32.is_some()
            && gfx_cmd_u8.is_some()
            && (
                graphics.as_ref().and_then(|g| g.host_bulk_step).is_some()
                || (graphics.as_ref().and_then(|g| g.host_get_frame).is_some() && graphics.as_ref().and_then(|g| g.gfx_submit_u8).is_some())
            );

        let entry_id = *function_ids.get(entry_name).with_context(|| format!("missing entry function {entry_name}"))?;
        let tick_id = *function_ids.get(tick_name).with_context(|| format!("missing tick function {tick_name}"))?;

        let entry_ptr = module.get_finalized_function(entry_id);
        let tick_ptr = module.get_finalized_function(tick_id);

        Ok(JitInstance {
            module_name: module_name.to_string(),
            entry_name: entry_name.to_string(),
            tick_name: tick_name.to_string(),
            graphics,
            _module: module,
            entry_fn: unsafe { std::mem::transmute(entry_ptr) },
            tick_fn: unsafe { std::mem::transmute(tick_ptr) },
            state_globals,
            data_ptrs,
            bulk_active,
            host_i32,
            host_f32,
            gfx_cmd_i32,
            gfx_cmd_f32,
            gfx_cmd_u8,
            host_req_seq,
            host_req_flags,
            host_req_window_w_px,
            host_req_window_h_px,
        })
    }

    fn save_state(&self) -> (u32, HashMap<String, Vec<u8>>)
    {
        let mut missing = 0u32;
        let mut out = HashMap::new();
        for (name, size) in &self.state_globals
        {
            let Some(ptr) = self.data_ptrs.get(name) else {
                missing += 1;
                continue;
            };
            unsafe {
                let slice = std::slice::from_raw_parts(*ptr as *const u8, *size);
                out.insert(name.clone(), slice.to_vec());
            }
        }
        (missing, out)
    }

    fn restore_state(&mut self, saved: HashMap<String, Vec<u8>>) -> u32
    {
        let mut missing = 0u32;
        for (name, size) in &self.state_globals
        {
            let Some(ptr) = self.data_ptrs.get(name) else {
                missing += 1;
                continue;
            };
            let Some(bytes) = saved.get(name) else {
                missing += 1;
                continue;
            };
            if bytes.len() != *size
            {
                missing += 1;
                continue;
            }
            unsafe {
                std::ptr::copy_nonoverlapping(bytes.as_ptr(), *ptr, *size);
            }
        }
        missing
    }
}

fn build_flags(opt_level: &str, target: &Triple) -> Result<settings::Flags>
{
    let mut flag_builder = settings::builder();

    match opt_level
    {
        "none" => flag_builder.set("opt_level", "none")?,
        "speed" => flag_builder.set("opt_level", "speed")?,
        "speed_and_size" => flag_builder.set("opt_level", "speed_and_size")?,
        other => bail!("invalid opt_level '{other}' (use none|speed|speed_and_size)"),
    }

    if target.operating_system.is_like_darwin()
    {
        flag_builder.set("is_pic", "true")?;
    }

    Ok(settings::Flags::new(flag_builder))
}

/*
Legacy CLIF parser prototype (deprecated). Kept for reference only.

#[derive(Clone)]
struct ParsedModule
{
    globals: Vec<ParsedGlobal>,
    externals: Vec<ParsedExternal>,
    functions: Vec<ParsedFunction>,
}

#[derive(Clone)]
struct ParsedGlobal
{
    name: String,
    init_data: GlobalInitData,
    size_bytes: usize,
}

#[derive(Clone)]
enum GlobalInitData
{
    Zero,
    String(Vec<u8>),
}

#[derive(Clone)]
struct ParsedExternal
{
    name: String,
    signature: Signature,
}

#[derive(Clone)]
struct ParsedFunction
{
    name: String,
    signature: Signature,
    blocks: Vec<ParsedBlock>,
}

#[derive(Clone)]
struct ParsedBlock
{
    name: String,
    param_value_ids: Vec<u32>,
    instructions: Vec<String>,
}

fn parse_stasis_clif(input: &str, default_cc: CallConv) -> Result<ParsedModule>
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

        if line.starts_with("global ")
        {
            lines.next();
            let (name, _ty, init_data, size_bytes) = parse_global_decl(line)
                .with_context(|| format!("failed to parse global declaration: {line}"))?;
            globals.push(ParsedGlobal { name, init_data, size_bytes });
            continue;
        }

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
                    // v12: i64
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
                c => {
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
        for ty in param_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
        {
            sig.params.push(AbiParam::new(parse_type(ty)?));
        }
    }

    if let Some((arrow, rest_after)) = after.split_once("->")
    {
        let _ = arrow;
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
    match s
    {
        "i8" => Ok(cranelift_codegen::ir::types::I8),
        "i16" => Ok(cranelift_codegen::ir::types::I16),
        "i32" => Ok(cranelift_codegen::ir::types::I32),
        "i64" => Ok(cranelift_codegen::ir::types::I64),
        "f32" => Ok(cranelift_codegen::ir::types::F32),
        "f64" => Ok(cranelift_codegen::ir::types::F64),
        other => bail!("unsupported type: {other}"),
    }
}

fn parse_value_id(v: &str) -> Result<u32>
{
    let v = v.trim().trim_end_matches(',').trim_end_matches(')');
    let v = v.strip_prefix('v').context("expected value id")?;
    Ok(v.parse::<u32>().context("invalid value id")?)
}

// The rest of the CLIF->IR lowering is non-trivial; for the hot-swap prototype we only support
// a small subset of Stasis-generated CLIF instructions.
fn build_function_ir<M: Module>(
    module: &mut M,
    function_ids: &HashMap<String, cranelift_module::FuncId>,
    data_ids: &HashMap<String, cranelift_module::DataId>,
    f: &ParsedFunction,
) -> Result<cranelift_codegen::ir::Function>
{
    // Minimal IR builder: supports the subset emitted by samples/hotstate_tick_watch.stasis.
    use cranelift_codegen::ir::{Function, InstBuilder};
    use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext};

    let mut func = Function::new();
    func.signature = f.signature.clone();
    let mut builder_ctx = FunctionBuilderContext::new();
    {
        let mut b = FunctionBuilder::new(&mut func, &mut builder_ctx);
        let block = b.create_block();
        b.switch_to_block(block);
        b.seal_block(block);

        let mut vals: HashMap<u32, cranelift_codegen::ir::Value> = HashMap::new();
        for inst in &f.blocks[0].instructions
        {
            if let Some((lhs, rhs)) = inst.split_once('=')
            {
                let lhs_id = parse_value_id(lhs.trim())?;
                let rhs = rhs.trim();
                if rhs.starts_with("global_value ")
                {
                    let name = rhs.strip_prefix("global_value").unwrap().trim();
                    let data_id = *data_ids.get(name).with_context(|| format!("unknown global {name}"))?;
                    let gv = module.declare_data_in_func(data_id, b.func);
                    let gv = b.ins().global_value(cranelift_codegen::ir::types::I64, gv);
                    vals.insert(lhs_id, gv);
                    continue;
                }
                if rhs.starts_with("iconst.")
                {
                    // iconst.i64 8
                    let parts: Vec<_> = rhs.split_whitespace().collect();
                    let ty = parts[0].strip_prefix("iconst.").unwrap();
                    let imm = parts[1].parse::<i64>()?;
                    let v = b.ins().iconst(parse_type(ty)?, imm);
                    vals.insert(lhs_id, v);
                    continue;
                }
                if rhs.starts_with("iadd ")
                {
                    let parts: Vec<_> = rhs.split_whitespace().collect();
                    let a = vals.get(&parse_value_id(parts[1])?).copied().context("missing value")?;
                    let b2 = vals.get(&parse_value_id(parts[2])?).copied().context("missing value")?;
                    let v = b.ins().iadd(a, b2);
                    vals.insert(lhs_id, v);
                    continue;
                }
                if rhs.starts_with("call %")
                {
                    // call %printf3(v4, v2, v5)
                    let rhs = rhs.strip_prefix("call %").unwrap();
                    let open = rhs.find('(').context("invalid call")?;
                    let close = rhs.rfind(')').context("invalid call")?;
                    let callee = rhs[..open].trim();
                    let args_str = rhs[open + 1..close].trim();

                    let func_id = *function_ids.get(callee).with_context(|| format!("unknown function {callee}"))?;
                    let local_ref = module.declare_func_in_func(func_id, b.func);
                    let mut args = Vec::new();
                    if !args_str.is_empty()
                    {
                        for a in args_str.split(',').map(|s| s.trim()).filter(|s| !s.is_empty())
                        {
                            let v = vals.get(&parse_value_id(a)?).copied().context("missing value")?;
                            args.push(v);
                        }
                    }
                    let call = b.ins().call(local_ref, &args);
                    if let Some(first) = b.inst_results(call).get(0).copied()
                    {
                        vals.insert(lhs_id, first);
                    }
                    continue;
                }
                bail!("unsupported instruction: {rhs}");
            }
            if inst.starts_with("return ")
            {
                let parts: Vec<_> = inst.split_whitespace().collect();
                let v = vals.get(&parse_value_id(parts[1])?).copied().context("missing value")?;
                b.ins().return_(&[v]);
                continue;
            }
            bail!("unsupported instruction: {inst}");
        }
        b.finalize();
    }
    Ok(func)
}
*/
