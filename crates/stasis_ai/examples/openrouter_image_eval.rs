use serde_json::json;
use sha2::{Digest, Sha256};
use stasis_ai::{
    ModelProvider, OpenRouterConfig, OpenRouterImageInput, OpenRouterProvider, RoutingConfig,
};
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() -> Result<(), String> {
    if std::env::var("STASIS_RUN_OPENROUTER_IMAGE_EVAL").as_deref() != Ok("1") {
        println!("skipped: set STASIS_RUN_OPENROUTER_IMAGE_EVAL=1, OPENROUTER_API_KEY, STASIS_AI_MODEL, STASIS_OPENROUTER_IMAGE_PATH, and STASIS_OPENROUTER_IMAGE_EXPECT to opt in");
        return Ok(());
    }
    let api_key = std::env::var("OPENROUTER_API_KEY")
        .map_err(|_| "OPENROUTER_API_KEY is required for the opt-in evaluation".to_string())?;
    let model = std::env::var("STASIS_AI_MODEL")
        .map_err(|_| "STASIS_AI_MODEL must name the capable model under evaluation".to_string())?;
    let image_path = PathBuf::from(
        std::env::var("STASIS_OPENROUTER_IMAGE_PATH")
            .map_err(|_| "STASIS_OPENROUTER_IMAGE_PATH is required".to_string())?,
    );
    let expected = std::env::var("STASIS_OPENROUTER_IMAGE_EXPECT")
        .map_err(|_| "STASIS_OPENROUTER_IMAGE_EXPECT is required".to_string())?;
    let file = std::fs::File::open(&image_path)
        .map_err(|error| format!("failed opening {}: {error}", image_path.display()))?;
    let mut bytes = Vec::new();
    file.take((stasis_ai::MAX_OPENROUTER_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("failed reading {}: {error}", image_path.display()))?;
    let mime_type = match image_path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        _ => return Err("evaluation image must have a PNG, JPG, or JPEG extension".to_string()),
    };
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let image = OpenRouterImageInput::new(mime_type, bytes, &sha256)?;
    let config = OpenRouterConfig {
        api_key,
        base_url: "https://openrouter.ai/api/v1".into(),
        model,
        routing: RoutingConfig::default(),
        timeout: Duration::from_secs(120),
    };
    let request = json!({
        "instruction": "Inspect the attached image pixels and describe the most distinctive visible fact in a done response. Do not use tools.",
        "tool_specs": stasis_ai::workshop_tool_specs(),
    })
    .to_string();
    let mut provider = OpenRouterProvider::new(config)?.with_image_inputs(vec![image])?;
    let capability = provider.refresh_image_input_capability(&AtomicBool::new(false))?;
    if !capability.supported {
        return Err(capability.reason);
    }
    let response = provider.respond(&request, &AtomicBool::new(false))?;
    let summary = match response {
        stasis_ai::ModelResponse::Done { summary, .. } => summary,
        other => return Err(format!("expected a done response, got {other:?}")),
    };
    if !summary.contains(&expected) {
        return Err(format!(
            "model response did not contain expected visible fact: {summary}"
        ));
    }
    println!("verified selected image sha256={sha256} model_response={summary}");
    Ok(())
}
