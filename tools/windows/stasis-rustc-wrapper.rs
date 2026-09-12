use std::env;
use std::process::{self, Command};

fn required_environment(name: &str) -> std::ffi::OsString {
    env::var_os(name).unwrap_or_else(|| {
        eprintln!("[stasis-rustc-wrapper] {name} is not configured");
        process::exit(2);
    })
}

fn main() {
    let python = required_environment("STASIS_RUSTC_WRAPPER_PYTHON");
    let script = required_environment("STASIS_RUSTC_WRAPPER_SCRIPT");
    let status = Command::new(python)
        .arg(script)
        .args(env::args_os().skip(1))
        .status()
        .unwrap_or_else(|error| {
            eprintln!("[stasis-rustc-wrapper] failed to start Python policy: {error}");
            process::exit(2);
        });
    process::exit(status.code().unwrap_or(1));
}
