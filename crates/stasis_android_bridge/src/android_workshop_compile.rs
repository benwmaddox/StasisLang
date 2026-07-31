use std::env;
use std::path::Path;

use stasis_android_bridge::compile_android_workshop_project;

fn main() {
    let Some(project_root) = env::args_os().nth(1) else {
        eprintln!(
            "{}",
            serde_json::json!({"ok": false, "error": "project root argument is required"})
        );
        std::process::exit(2);
    };
    match compile_android_workshop_project(&project_root, Path::new("src/main.stasis")) {
        Ok(result) => println!(
            "{}",
            serde_json::json!({
                "ok": true,
                "status": result.status,
                "compiled_function_count": result.compiled_function_count,
                "manifest_path": result.manifest_path,
                "runtime_state_path": result.runtime_state_path,
            })
        ),
        Err(error) => {
            eprintln!("{}", serde_json::json!({"ok": false, "error": error}));
            std::process::exit(1);
        }
    }
}
