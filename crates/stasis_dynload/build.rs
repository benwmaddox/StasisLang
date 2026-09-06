use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=STASIS_BUILD_FINGERPRINT");
    println!("cargo:rerun-if-env-changed=STASIS_RELEASE_ID");
    if let Ok(fingerprint) = std::env::var("STASIS_BUILD_FINGERPRINT") {
        if !fingerprint.trim().is_empty() {
            println!("cargo:rustc-env=STASIS_BUILD_FINGERPRINT={fingerprint}");
        }
    }
    if let Ok(release_id) = std::env::var("STASIS_RELEASE_ID") {
        if !release_id.trim().is_empty() {
            println!("cargo:rustc-env=STASIS_RELEASE_ID={release_id}");
        }
    }
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    cc::Build::new()
        .file(runtime.join("stasis_render_trace.c"))
        .file(runtime.join("stasis_platform_services.c"))
        .include(&runtime)
        .compile("stasis_render_trace_native");
    println!(
        "cargo:rerun-if-changed={}",
        runtime.join("stasis_render_trace.c").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        runtime.join("stasis_render_contract.h").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        runtime.join("stasis_platform_services.c").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        runtime.join("stasis_platform_services.h").display()
    );
}
