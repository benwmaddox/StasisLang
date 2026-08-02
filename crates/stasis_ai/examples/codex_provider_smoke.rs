//! Run with `cargo run -p stasis_ai --example codex_provider_smoke` from a
//! signed-in Codex environment.

use stasis_ai::{CodexExecProvider, ModelProvider, ModelResponse};
use std::sync::atomic::AtomicBool;

fn main() {
    let mut provider = CodexExecProvider::default();
    let response = provider
        .respond(
            r#"{"system":"Return a done response without calling tools.","user_prompt":"Confirm the Stasis AI provider is connected.","tool_specs":[],"transcript":[]}"#,
            &AtomicBool::new(false),
        )
        .unwrap_or_else(|error| {
            eprintln!("Codex provider smoke failed: {error}");
            std::process::exit(1);
        });

    match response {
        ModelResponse::Done { .. } => println!("Codex provider smoke passed"),
        ModelResponse::ToolCalls { .. } => {
            eprintln!("Codex provider smoke failed: expected a done response");
            std::process::exit(1);
        }
    }
}
