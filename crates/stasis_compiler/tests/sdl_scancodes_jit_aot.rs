use stasis_compiler::backend::{aot::AotProcess, jit::JitProcess};
use stasis_jit::{AotLinkConfig, AotTarget};
use std::path::{Path, PathBuf};
use std::process::Command;

const TEST_PATH: &str = "tests/stasis/sdl_scancodes_contract.test.stasis";
const SCANCODE_MODULE_SOURCE: &str = include_str!("../../../src/stdlib/sdl_scancodes.stasis");

// Independent reviewed oracle: every ordinary SDL 3.4.10 declaration in source order.
const EXPECTED_SCANCODES: &str = r#"
A=4 B=5 C=6 D=7 E=8
F=9 G=10 H=11 I=12 J=13
K=14 L=15 M=16 N=17 O=18
P=19 Q=20 R=21 S=22 T=23
U=24 V=25 W=26 X=27 Y=28
Z=29 DIGIT_1=30 DIGIT_2=31 DIGIT_3=32 DIGIT_4=33
DIGIT_5=34 DIGIT_6=35 DIGIT_7=36 DIGIT_8=37 DIGIT_9=38
DIGIT_0=39 RETURN=40 ESCAPE=41 BACKSPACE=42 TAB=43
SPACE=44 MINUS=45 EQUALS=46 LEFTBRACKET=47 RIGHTBRACKET=48
BACKSLASH=49 NONUSHASH=50 SEMICOLON=51 APOSTROPHE=52 GRAVE=53
COMMA=54 PERIOD=55 SLASH=56 CAPSLOCK=57 F1=58
F2=59 F3=60 F4=61 F5=62 F6=63
F7=64 F8=65 F9=66 F10=67 F11=68
F12=69 PRINTSCREEN=70 SCROLLLOCK=71 PAUSE=72 INSERT=73
HOME=74 PAGEUP=75 DELETE=76 END=77 PAGEDOWN=78
RIGHT=79 LEFT=80 DOWN=81 UP=82 NUMLOCKCLEAR=83
KP_DIVIDE=84 KP_MULTIPLY=85 KP_MINUS=86 KP_PLUS=87 KP_ENTER=88
KP_1=89 KP_2=90 KP_3=91 KP_4=92 KP_5=93
KP_6=94 KP_7=95 KP_8=96 KP_9=97 KP_0=98
KP_PERIOD=99 NONUSBACKSLASH=100 APPLICATION=101 POWER=102 KP_EQUALS=103
F13=104 F14=105 F15=106 F16=107 F17=108
F18=109 F19=110 F20=111 F21=112 F22=113
F23=114 F24=115 EXECUTE=116 HELP=117 MENU=118
SELECT=119 STOP=120 AGAIN=121 UNDO=122 CUT=123
COPY=124 PASTE=125 FIND=126 MUTE=127 VOLUMEUP=128
VOLUMEDOWN=129 KP_COMMA=133 KP_EQUALSAS400=134 INTERNATIONAL1=135 INTERNATIONAL2=136
INTERNATIONAL3=137 INTERNATIONAL4=138 INTERNATIONAL5=139 INTERNATIONAL6=140 INTERNATIONAL7=141
INTERNATIONAL8=142 INTERNATIONAL9=143 LANG1=144 LANG2=145 LANG3=146
LANG4=147 LANG5=148 LANG6=149 LANG7=150 LANG8=151
LANG9=152 ALTERASE=153 SYSREQ=154 CANCEL=155 CLEAR=156
PRIOR=157 RETURN2=158 SEPARATOR=159 OUT=160 OPER=161
CLEARAGAIN=162 CRSEL=163 EXSEL=164 KP_00=176 KP_000=177
THOUSANDSSEPARATOR=178 DECIMALSEPARATOR=179 CURRENCYUNIT=180 CURRENCYSUBUNIT=181 KP_LEFTPAREN=182
KP_RIGHTPAREN=183 KP_LEFTBRACE=184 KP_RIGHTBRACE=185 KP_TAB=186 KP_BACKSPACE=187
KP_A=188 KP_B=189 KP_C=190 KP_D=191 KP_E=192
KP_F=193 KP_XOR=194 KP_POWER=195 KP_PERCENT=196 KP_LESS=197
KP_GREATER=198 KP_AMPERSAND=199 KP_DBLAMPERSAND=200 KP_VERTICALBAR=201 KP_DBLVERTICALBAR=202
KP_COLON=203 KP_HASH=204 KP_SPACE=205 KP_AT=206 KP_EXCLAM=207
KP_MEMSTORE=208 KP_MEMRECALL=209 KP_MEMCLEAR=210 KP_MEMADD=211 KP_MEMSUBTRACT=212
KP_MEMMULTIPLY=213 KP_MEMDIVIDE=214 KP_PLUSMINUS=215 KP_CLEAR=216 KP_CLEARENTRY=217
KP_BINARY=218 KP_OCTAL=219 KP_DECIMAL=220 KP_HEXADECIMAL=221 LCTRL=224
LSHIFT=225 LALT=226 LGUI=227 RCTRL=228 RSHIFT=229
RALT=230 RGUI=231 MODE=257 SLEEP=258 WAKE=259
CHANNEL_INCREMENT=260 CHANNEL_DECREMENT=261 MEDIA_PLAY=262 MEDIA_PAUSE=263 MEDIA_RECORD=264
MEDIA_FAST_FORWARD=265 MEDIA_REWIND=266 MEDIA_NEXT_TRACK=267 MEDIA_PREVIOUS_TRACK=268 MEDIA_STOP=269
MEDIA_EJECT=270 MEDIA_PLAY_PAUSE=271 MEDIA_SELECT=272 AC_NEW=273 AC_OPEN=274
AC_CLOSE=275 AC_EXIT=276 AC_SAVE=277 AC_PRINT=278 AC_PROPERTIES=279
AC_SEARCH=280 AC_HOME=281 AC_BACK=282 AC_FORWARD=283 AC_STOP=284
AC_REFRESH=285 AC_BOOKMARKS=286 SOFTLEFT=287 SOFTRIGHT=288 CALL=289
ENDCALL=290
"#;

const FIXTURE_PREFIX: &str = r#"
import "../../src/stdlib/sdl_scancodes.stasis";
import "../../src/stdlib/host_frame.stasis";

global frame: HostFrame;

function main(): i32 {
    host_i32[32 + sdl_scancodes.SCANCODE.W] = 1;
    host_i32[32 + sdl_scancodes.SCANCODE.S] = 1;
    host_i32[32 + sdl_scancodes.SCANCODE.SPACE] = 1;
    host_i32[32 + sdl_scancodes.SCANCODE.UP] = 1;
    host_i32[32 + sdl_scancodes.SCANCODE.LEFT] = 1;
    frame.refresh();

    if (frame.keys[sdl_scancodes.SCANCODE.W] != 1) { return 1; }
    if (frame.keys[sdl_scancodes.SCANCODE.S] != 1) { return 2; }
    if (frame.keys[sdl_scancodes.SCANCODE.SPACE] != 1) { return 3; }
    if (frame.keys[sdl_scancodes.SCANCODE.UP] != 1) { return 4; }
    if (frame.keys[sdl_scancodes.SCANCODE.LEFT] != 1) { return 5; }

    if (sdl_scancodes.SCANCODE.RETURN != 40) { return 6; }
    if (sdl_scancodes.SCANCODE.ESCAPE != 41) { return 7; }
    if (sdl_scancodes.SCANCODE.DELETE != 76) { return 8; }
    if (sdl_scancodes.SCANCODE.END != 77) { return 9; }
    if (sdl_scancodes.SCANCODE.DIGIT_1 != 30) { return 10; }
    if (sdl_scancodes.SCANCODE.DIGIT_0 != 39) { return 11; }

    // Keep the existing enum source surface available during migration.
    if (sdl_scancodes.Scancode.W != 26) { return 12; }
    if (sdl_scancodes.Scancode.Left != 80) { return 13; }
"#;

fn expected_scancodes() -> Vec<(String, i32)> {
    EXPECTED_SCANCODES
        .split_whitespace()
        .map(|pair| {
            let (name, value) = pair.split_once('=').expect("oracle NAME=VALUE pair");
            (
                name.to_string(),
                value.parse().expect("oracle integer value"),
            )
        })
        .collect()
}

fn source_scancodes() -> Vec<(String, i32)> {
    let (_, group) = SCANCODE_MODULE_SOURCE
        .split_once("const SCANCODE {")
        .expect("generated SCANCODE group in stdlib module");
    let (group, _) = group.split_once('}').expect("closing SCANCODE group brace");
    group
        .lines()
        .filter_map(|line| {
            let line = line.split("//").next().unwrap_or_default().trim();
            if line.is_empty() {
                return None;
            }
            let declaration = line
                .strip_suffix(';')
                .expect("SCANCODE member ends in semicolon");
            let (name, value) = declaration
                .split_once(": i32 =")
                .expect("typed SCANCODE member declaration");
            Some((
                name.trim().to_string(),
                value.trim().parse().expect("SCANCODE integer value"),
            ))
        })
        .collect()
}

fn fixture_source() -> String {
    let assertions = expected_scancodes()
        .iter()
        .enumerate()
        .map(|(index, (name, value))| {
            format!(
                "    if (sdl_scancodes.SCANCODE.{name} != {value}) {{ return {}; }}\n",
                index + 100
            )
        })
        .collect::<String>();
    format!("{FIXTURE_PREFIX}{assertions}    return 0;\n}}\n")
}

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("canonical repository root")
}

fn configure_jit(process: &mut JitProcess) {
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set JIT project root");
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(TEST_PATH, fixture_source());
}

fn configure_aot(process: &mut AotProcess) {
    process
        .set_project_root(repository_root().to_string_lossy())
        .expect("set AOT project root");
    process.set_required_emit_roots(&["main".to_string()]);
    process.upsert_file(TEST_PATH, fixture_source());
}

fn run_linked_aot(aot: &AotProcess) -> i32 {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let target = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| repository_root().join("target"));
    let directory = target.join(format!("sdl-scancodes-aot-{}-{stamp}", std::process::id()));
    std::fs::create_dir_all(&directory).expect("create linked AOT directory");

    let deps = std::env::current_exe()
        .expect("test executable")
        .parent()
        .expect("Cargo deps directory")
        .to_path_buf();
    let (import, runtime) = [&deps, deps.parent().expect("Cargo profile directory")]
        .into_iter()
        .find_map(|candidate| {
            let (import_name, runtime_name) = if cfg!(windows) {
                ("stasis_dynload.dll.lib", "stasis_dynload.dll")
            } else if cfg!(target_os = "macos") {
                ("libstasis_dynload.dylib", "libstasis_dynload.dylib")
            } else {
                ("libstasis_dynload.so", "libstasis_dynload.so")
            };
            let import = candidate.join(import_name);
            let runtime = candidate.join(runtime_name);
            (import.is_file() && runtime.is_file()).then_some((import, runtime))
        })
        .expect("fresh dynload runtime and import library");
    std::fs::copy(
        &runtime,
        directory.join(runtime.file_name().expect("runtime library name")),
    )
    .expect("copy dynload runtime");

    #[cfg(windows)]
    let linker_path = Some(
        std::env::var_os("STASIS_AOT_LINKER")
            .map(PathBuf::from)
            .or_else(|| {
                cc::windows_registry::find_tool("x86_64-pc-windows-msvc", "link.exe")
                    .map(|tool| tool.path().to_path_buf())
            })
            .expect("MSVC link.exe is required for linked AOT execution"),
    );
    #[cfg(not(windows))]
    let linker_path = std::env::var_os("STASIS_AOT_LINKER").map(PathBuf::from);

    let executable = directory.join(format!(
        "sdl_scancodes_main{}",
        std::env::consts::EXE_SUFFIX
    ));
    aot.link_executable_for_i32_noarg_function(
        "main",
        &executable,
        &AotLinkConfig {
            linker_path,
            runtime_lib_paths: vec![import],
            target: AotTarget::Native,
        },
    )
    .expect("link native AOT executable");

    #[cfg(windows)]
    let status = Command::new(repository_root().join(".cargo/stasis-sign-and-run.cmd"))
        .arg(executable.file_name().expect("linked executable name"))
        .current_dir(&directory)
        .status()
        .expect("run linked AOT executable");
    #[cfg(not(windows))]
    let status = {
        let variable = if cfg!(target_os = "macos") {
            "DYLD_LIBRARY_PATH"
        } else {
            "LD_LIBRARY_PATH"
        };
        let mut paths = vec![directory.clone()];
        if let Some(existing) = std::env::var_os(variable) {
            paths.extend(std::env::split_paths(&existing));
        }
        Command::new(&executable)
            .current_dir(&directory)
            .env(
                variable,
                std::env::join_paths(paths).expect("join AOT runtime search path"),
            )
            .status()
            .expect("run linked AOT executable")
    };

    let _ = std::fs::remove_dir_all(&directory);
    status.code().expect("linked AOT process exit code")
}

#[test]
fn generated_sdl_scancode_group_matches_independent_header_oracle() {
    let expected = expected_scancodes();
    let source = source_scancodes();
    let expected_names = expected
        .iter()
        .map(|(name, _)| name.as_str())
        .collect::<std::collections::BTreeSet<_>>();
    let expected_values = expected
        .iter()
        .map(|(_, value)| *value)
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(expected.len(), 246, "ordinary SDL scancode count");
    assert_eq!(expected_names.len(), expected.len(), "unique member names");
    assert_eq!(expected_values.len(), expected.len(), "no numeric aliases");
    assert!(expected.iter().all(|(_, value)| (0..512).contains(value)));
    assert_eq!(
        source, expected,
        "generated source must match all names and values in SDL declaration order"
    );
    assert!(SCANCODE_MODULE_SOURCE.contains("enum Scancode"));
    for sentinel in ["UNKNOWN", "RESERVED", "COUNT"] {
        assert!(!source.iter().any(|(name, _)| name == sentinel));
    }
}

#[test]
fn imported_sdl_scancode_group_reads_host_frame_in_jit() {
    let mut jit = JitProcess::new();
    configure_jit(&mut jit);
    jit.compile()
        .expect("compile imported SDL scancode HostFrame JIT fixture");
    assert_eq!(
        jit.execute_i32_noarg_by_name("main")
            .expect("execute imported SDL scancode HostFrame JIT fixture"),
        0,
        "typed group indices select actual imported HostFrame key slots"
    );
}

#[test]
fn imported_sdl_scancode_group_reads_host_frame_in_linked_aot() {
    let mut aot = AotProcess::new();
    configure_aot(&mut aot);
    aot.compile()
        .expect("compile imported SDL scancode HostFrame AOT fixture");
    assert_eq!(
        run_linked_aot(&aot),
        0,
        "linked native executable reads HostFrame using imported typed scancodes"
    );
}
