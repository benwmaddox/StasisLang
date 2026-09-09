use serde_json::json;
use stasis_ai::{
    workshop_tool_specs, ModelProvider, ModelResponse, OpenRouterConfig, OpenRouterProvider,
    PreferredThroughputPolicy, ProviderProgress, RoutingConfig, RoutingSort,
};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() -> Result<(), String> {
    if std::env::var("STASIS_RUN_OPENROUTER_EVAL").as_deref() != Ok("1") {
        println!("skipped: set STASIS_RUN_OPENROUTER_EVAL=1 and OPENROUTER_API_KEY to opt in");
        return Ok(());
    }
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| "OPENROUTER_API_KEY is required for the opt-in evaluation".to_string())?;
    let config = OpenRouterConfig {
        api_key,
        base_url: "https://openrouter.ai/api/v1".into(),
        model: "openai/gpt-oss-120b".into(),
        routing: RoutingConfig {
            only: vec!["cerebras".into()],
            order: vec!["cerebras".into()],
            allow_fallbacks: false,
            sort: RoutingSort::Throughput,
            preferred_min_throughput: Some(1_000.0),
            preferred_throughput_policy: PreferredThroughputPolicy::AllowBelow,
            hard_min_throughput: None,
            max_price: None,
        },
        timeout: Duration::from_secs(120),
    };
    let request = json!({
        "instruction": "Return one tool_calls response selecting the offered read_symbol action for function tick. Use native JSON object args, concise working_notes, and no prose outside the schema.",
        "tool_specs": workshop_tool_specs(),
    })
    .to_string();
    let mut provider = OpenRouterProvider::new(config)?;
    let mut progress = Vec::new();
    let response =
        provider.respond_with_progress(&request, &AtomicBool::new(false), &mut |event| {
            progress.push(event)
        })?;
    if !matches!(
        response,
        ModelResponse::ToolCalls { ref tool_calls, .. } if !tool_calls.is_empty()
    ) {
        return Err("live evaluation did not return an action".to_string());
    }
    let usage = provider
        .take_usage()
        .ok_or_else(|| "live evaluation omitted usage audit data".to_string())?;
    let first_response_ms = progress.iter().find_map(|event| match event {
        ProviderProgress::FirstResponse { elapsed_ms } => Some(*elapsed_ms),
        _ => None,
    });
    let first_action_ms = progress.iter().find_map(|event| match event {
        ProviderProgress::FirstAction { elapsed_ms } => Some(*elapsed_ms),
        _ => None,
    });
    if first_response_ms
        != usage
            .pointer("/timing_ms/first_content")
            .and_then(|v| v.as_u64())
        || first_action_ms
            != usage
                .pointer("/timing_ms/first_action")
                .and_then(|v| v.as_u64())
    {
        return Err("live progress timestamps did not match the usage audit".to_string());
    }
    println!(
        "safe progress: {}",
        serde_json::to_string(&progress)
            .map_err(|error| format!("failed encoding progress: {error}"))?
    );
    println!("sanitized usage: {usage}");
    Ok(())
}
