use std::path::PathBuf;

fn main() {
    let runtime = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    cc::Build::new()
        .file(runtime.join("stasis_render_trace.c"))
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
}
