//! Opt-in network check: pass a workspace containing an OpenRouter .env.
//! Sends one read-only, no-tools request and prints only operational metrics.

use serde_json::json;
use stasis_ai::{ModelProvider, ModelResponse, OpenRouterConfig, OpenRouterProvider};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() {
    let root = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            eprintln!("Usage: openrouter_workspace_smoke <workspace>");
            std::process::exit(2);
        });
    let mut config = OpenRouterConfig::from_workspace(&root).unwrap_or_else(|_| {
        eprintln!("OpenRouter workspace configuration failed");
        std::process::exit(1);
    });
    config.timeout = config.timeout.min(Duration::from_secs(120));
    let hard_minimum = config.routing.hard_min_throughput;
    let mut provider = OpenRouterProvider::new(config).unwrap_or_else(|_| {
        eprintln!("OpenRouter client configuration failed");
        std::process::exit(1);
    });
    let result = provider.respond(
        r#"{"system":"Return a done response without calling tools.","user_prompt":"Confirm the Stasis AI provider is connected.","tool_specs":[],"transcript":[]}"#,
        &AtomicBool::new(false),
    );
    if let Some(usage) = provider.take_usage() {
        println!(
            "{}",
            json!({
                "resolved_model": usage["resolved_model"],
                "resolved_provider": usage["resolved_provider"],
                "qualified_endpoint_minimum_tokens_per_second": hard_minimum,
                "route": usage["route"],
                "timing_ms": usage["timing_ms"],
                "tokens": usage["tokens"],
                "observed_completion_tokens_per_second": usage["throughput_tokens_per_second"],
            })
        );
    }
    match result {
        Ok(ModelResponse::Done { .. }) => println!("OpenRouter workspace smoke passed"),
        Ok(_) => {
            eprintln!("OpenRouter smoke returned an unexpected tool request");
            std::process::exit(1);
        }
        Err(error) => {
            eprintln!(
                "{}",
                stasis_ai::task_controller::safe_provider_error(&error)
            );
            std::process::exit(1);
        }
    }
}
