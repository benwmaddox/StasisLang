use crate::task_session::MAX_ATTRIBUTION_CHARS;
use base64::Engine;
use reqwest::Client;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const MAX_ENCODED_BYTES: usize = 12 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = MAX_ENCODED_BYTES + 64 * 1024;

#[derive(Clone)]
pub struct ImageGenerationConfig {
    pub provider: String,
    pub model: String,
    pub route: String,
    pub fallback: String,
    api_key: String,
    base_url: String,
    kind: ProviderKind,
    routing: Option<Value>,
    allow_fallbacks: bool,
}

#[derive(Debug, Clone, Copy)]
enum ProviderKind {
    OpenAi,
    OpenRouter,
}

#[derive(Debug, Clone)]
pub struct GeneratedImage {
    pub png: Vec<u8>,
    pub provider: String,
    pub model: String,
    pub route: String,
    pub fallback: String,
    pub cost_micros: Option<u64>,
}

impl ImageGenerationConfig {
    pub fn from_env() -> Result<Self, String> {
        let provider = env("STASIS_IMAGE_PROVIDER")
            .or_else(|| env("STASIS_AI_PROVIDER"))
            .unwrap_or_else(|| "codex".into());
        let config = match provider.as_str() {
            "openai" => Self {
                provider,
                model: env("STASIS_IMAGE_MODEL").unwrap_or_else(|| "gpt-image-1".into()),
                route: "openai:direct".into(),
                fallback: "disabled".into(),
                api_key: required("OPENAI_API_KEY", "STASIS_IMAGE_PROVIDER=openai")?,
                base_url: env("STASIS_OPENAI_URL")
                    .unwrap_or_else(|| "https://api.openai.com/v1".into()),
                kind: ProviderKind::OpenAi,
                routing: None,
                allow_fallbacks: false,
            },
            "openrouter" => {
                let mut configured = crate::openrouter::OpenRouterConfig::from_env()?;
                configured.model = required("STASIS_IMAGE_MODEL", "STASIS_IMAGE_PROVIDER=openrouter")?;
                configured.validate()?;
                validate_image_routing(&configured.routing)?;
                let only = &configured.routing.only;
                let order = &configured.routing.order;
                let fallback = configured.routing.allow_fallbacks;
                let route = if !only.is_empty() {
                    format!("only:{}", only.join(","))
                } else if !order.is_empty() {
                    format!("order:{}", order.join(","))
                } else {
                    format!("openrouter:{:?}", configured.routing.sort).to_ascii_lowercase()
                };
                let routing = crate::openrouter::route_json_for_config(&configured.routing, None);
                Self {
                    provider,
                    model: configured.model,
                    route,
                    fallback: if fallback { "enabled" } else { "disabled" }.into(),
                    api_key: configured.api_key,
                    base_url: configured.base_url,
                    kind: ProviderKind::OpenRouter,
                    routing: Some(routing),
                    allow_fallbacks: fallback,
                }
            }
            "codex" => return Err("the installed Codex subscription does not expose image generation to Stasis; set STASIS_IMAGE_PROVIDER to openai or openrouter".into()),
            value => return Err(format!("STASIS_IMAGE_PROVIDER must be openai or openrouter; got {value}")),
        };
        config.validate_attribution()?;
        Ok(config)
    }

    pub fn generate(&self, prompt: &str, canceled: &AtomicBool) -> Result<GeneratedImage, String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            return Err("image prompt is empty".into());
        }
        if prompt.chars().count() > 4_000 {
            return Err("image prompt exceeds 4,000 characters".into());
        }
        self.validate_attribution()?;
        if canceled.load(Ordering::Acquire) {
            return Err("image generation canceled".into());
        }
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| "failed configuring image provider runtime".to_string())?
            .block_on(self.generate_async(prompt, canceled))
    }

    async fn generate_async(
        &self,
        prompt: &str,
        canceled: &AtomicBool,
    ) -> Result<GeneratedImage, String> {
        let client = Client::builder()
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|_| "failed configuring image provider client".to_string())?;
        let (url, body) = match self.kind {
            ProviderKind::OpenAi => (
                format!("{}/images/generations", self.base_url.trim_end_matches('/')),
                json!({"model":self.model,"prompt":prompt,"size":"1024x1024","quality":"low"}),
            ),
            ProviderKind::OpenRouter => {
                let routing = self.routing.clone().expect("OpenRouter routing snapshot");
                (
                    format!("{}/chat/completions", self.base_url.trim_end_matches('/')),
                    json!({
                        "model":self.model,"messages":[{"role":"user","content":prompt}],
                        "modalities":["image","text"],"image_config":{"aspect_ratio":"1:1"},"provider":routing
                    }),
                )
            }
        };
        let request = client
            .post(url)
            .bearer_auth(&self.api_key)
            .json(&body)
            .send();
        tokio::pin!(request);
        let response = loop {
            tokio::select! {
                value = &mut request => break value.map_err(|_| "image provider request failed".to_string())?,
                _ = tokio::time::sleep(Duration::from_millis(50)) => if canceled.load(Ordering::Acquire) { return Err("image generation canceled".into()); }
            }
        };
        let status = response.status();
        if !status.is_success() {
            return Err(format!(
                "image provider request failed with status {}",
                status.as_u16()
            ));
        }
        if response
            .content_length()
            .is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
        {
            return Err("image provider response is oversized".into());
        }
        let mut response = response;
        let mut bytes = Vec::new();
        loop {
            let chunk = tokio::select! {
                value = response.chunk() => value.map_err(|_| "image provider response could not be read".to_string())?,
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if canceled.load(Ordering::Acquire) { return Err("image generation canceled".into()); }
                    continue;
                }
            };
            let Some(chunk) = chunk else { break };
            if bytes.len().saturating_add(chunk.len()) > MAX_RESPONSE_BYTES {
                return Err("image provider response is oversized".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let value: Value = serde_json::from_slice(&bytes)
            .map_err(|_| "image provider returned an invalid response".to_string())?;
        let encoded = match self.kind {
            ProviderKind::OpenAi => value.pointer("/data/0/b64_json").and_then(Value::as_str),
            ProviderKind::OpenRouter => value
                .pointer("/choices/0/message/images/0/image_url/url")
                .or_else(|| value.pointer("/choices/0/message/images/0/url"))
                .and_then(Value::as_str)
                .and_then(|url| url.strip_prefix("data:image/png;base64,")),
        }
        .ok_or_else(|| "image provider response contained no PNG".to_string())?;
        Ok(GeneratedImage {
            png: decode_base64(encoded)?,
            provider: resolved_attribution(
                value.get("provider").and_then(Value::as_str),
                &self.provider,
                "unknown (reported provider metadata invalid)",
            ),
            model: resolved_attribution(
                value.get("model").and_then(Value::as_str),
                &self.model,
                "unknown (reported model metadata invalid)",
            ),
            route: format!("configured {}", self.route),
            fallback: match value.get("fallback").and_then(Value::as_bool) {
                Some(true) => "used".into(),
                Some(false) => "not used".into(),
                None if self.allow_fallbacks => "allowed; actual use unknown".into(),
                None => "disabled".into(),
            },
            cost_micros: value
                .pointer("/usage/cost")
                .and_then(Value::as_f64)
                .filter(|v| v.is_finite() && *v >= 0.0)
                .map(|v| (v * 1_000_000.0).round() as u64),
        })
    }

    fn validate_attribution(&self) -> Result<(), String> {
        validate_attribution_field("image provider", &self.provider)?;
        validate_attribution_field("image model", &self.model)?;
        validate_attribution_field("image route", &self.route)?;
        validate_attribution_field("image fallback label", &self.fallback)?;
        for fallback in [
            "used",
            "not used",
            "allowed; actual use unknown",
            "disabled",
        ] {
            for cost in [
                "unknown cost".to_string(),
                format!("${:.4}", u64::MAX as f64 / 1_000_000.0),
            ] {
                let credit = format!(
                    "route configured {}; fallback {}; {cost}",
                    self.route, fallback
                );
                if credit.chars().count() > MAX_ATTRIBUTION_CHARS {
                    return Err(format!(
                        "image routing attribution exceeds {MAX_ATTRIBUTION_CHARS} characters"
                    ));
                }
            }
        }
        Ok(())
    }
}

fn validate_image_routing(routing: &crate::openrouter::RoutingConfig) -> Result<(), String> {
    if routing.hard_min_throughput.is_some() {
        return Err("STASIS_AI_HARD_MIN_THROUGHPUT is not supported for image routing because image endpoints do not publish token throughput".into());
    }
    if routing.preferred_min_throughput.is_some()
        && matches!(
            routing.preferred_throughput_policy,
            crate::openrouter::PreferredThroughputPolicy::Fail
        )
    {
        return Err("STASIS_AI_PREFERRED_THROUGHPUT_POLICY=fail is not supported for image routing because image endpoints do not publish token throughput".into());
    }
    Ok(())
}

fn decode_base64(value: &str) -> Result<Vec<u8>, String> {
    if value.len() > MAX_ENCODED_BYTES {
        return Err("image provider PNG encoding is invalid or oversized".into());
    }
    base64::engine::general_purpose::STANDARD
        .decode(value)
        .map_err(|_| "image provider PNG encoding is invalid".into())
}
fn validate_attribution_field(field: &str, value: &str) -> Result<(), String> {
    let count = value.trim().chars().count();
    if count == 0 {
        return Err(format!("{field} must not be empty"));
    }
    if count > MAX_ATTRIBUTION_CHARS {
        return Err(format!(
            "{field} exceeds {MAX_ATTRIBUTION_CHARS} characters"
        ));
    }
    Ok(())
}

fn resolved_attribution(value: Option<&str>, configured: &str, invalid: &str) -> String {
    let Some(value) = value else {
        return configured.to_string();
    };
    let value = value.trim();
    if value.is_empty()
        || value.chars().count() > MAX_ATTRIBUTION_CHARS
        || value.chars().any(char::is_control)
    {
        invalid.to_string()
    } else {
        value.to_string()
    }
}
fn env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
}
fn required(name: &str, context: &str) -> Result<String, String> {
    env(name).ok_or_else(|| format!("{name} is required when {context}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::mpsc;

    fn mock(
        status: u16,
        response: &'static str,
    ) -> (String, mpsc::Receiver<String>, std::thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            let mut buffer = [0_u8; 4096];
            loop {
                let read = stream.read(&mut buffer).unwrap();
                request.extend_from_slice(&buffer[..read]);
                let header_end = request
                    .windows(4)
                    .position(|value| value == b"\r\n\r\n")
                    .map(|value| value + 4);
                if let Some(header_end) = header_end {
                    let headers = String::from_utf8_lossy(&request[..header_end]);
                    let length = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .and_then(|value| value.parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if request.len() >= header_end + length {
                        break;
                    }
                }
            }
            tx.send(String::from_utf8_lossy(&request).into_owned())
                .unwrap();
            let reason = if status == 200 { "OK" } else { "Bad Request" };
            write!(stream, "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{response}", response.len()).unwrap();
        });
        (format!("http://{address}"), rx, worker)
    }

    fn config(kind: ProviderKind, base_url: String) -> ImageGenerationConfig {
        ImageGenerationConfig {
            provider: match kind {
                ProviderKind::OpenAi => "openai",
                ProviderKind::OpenRouter => "openrouter",
            }
            .into(),
            model: "fixture-model".into(),
            route: "fixture:direct".into(),
            fallback: "disabled".into(),
            api_key: "unit-secret".into(),
            base_url,
            kind,
            routing: Some(
                json!({"allow_fallbacks":false,"require_parameters":true,"sort":"price"}),
            ),
            allow_fallbacks: false,
        }
    }

    #[test]
    fn base64_is_strict() {
        assert_eq!(decode_base64("iVBORw==").unwrap(), b"\x89PNG");
        assert!(decode_base64("bad").is_err());
        assert!(decode_base64("!!!!").is_err());
    }

    #[test]
    fn openai_generation_uses_image_endpoint_and_reports_unknown_cost() {
        let (url, request, worker) = mock(200, r#"{"data":[{"b64_json":"iVBORw=="}]}"#);
        let generated = config(ProviderKind::OpenAi, url)
            .generate("small red ship", &AtomicBool::new(false))
            .unwrap();
        let request = request.recv().unwrap();
        worker.join().unwrap();
        assert!(request.starts_with("POST /images/generations "));
        assert!(request.contains("\"quality\":\"low\""));
        assert!(!request.contains("response_format"));
        assert_eq!(generated.png, b"\x89PNG");
        assert_eq!(generated.cost_micros, None);
    }

    #[test]
    fn openrouter_generation_sends_routing_and_reads_resolved_state() {
        let response = r#"{"provider":"resolved-fixture","model":"resolved-model","usage":{"cost":0.0125},"choices":[{"message":{"images":[{"image_url":{"url":"data:image/png;base64,iVBORw=="}}]}}]}"#;
        let (url, request, worker) = mock(200, response);
        let generated = config(ProviderKind::OpenRouter, url)
            .generate("unit", &AtomicBool::new(false))
            .unwrap();
        let request = request.recv().unwrap();
        worker.join().unwrap();
        assert!(request.starts_with("POST /chat/completions "));
        assert!(request.contains("\"modalities\":[\"image\",\"text\"]"));
        assert!(request.contains("\"require_parameters\":true"));
        assert_eq!(generated.provider, "resolved-fixture");
        assert_eq!(generated.model, "resolved-model");
        assert_eq!(generated.cost_micros, Some(12_500));
    }

    #[test]
    fn attribution_that_cannot_fit_is_rejected_before_provider_request() {
        let mut configured = config(ProviderKind::OpenAi, "http://127.0.0.1:9".into());
        configured.route = "x".repeat(MAX_ATTRIBUTION_CHARS);
        assert_eq!(
            configured
                .generate("unit", &AtomicBool::new(false))
                .unwrap_err(),
            format!("image routing attribution exceeds {MAX_ATTRIBUTION_CHARS} characters")
        );

        configured.route = "direct".into();
        configured.model = "x".repeat(MAX_ATTRIBUTION_CHARS + 1);
        assert_eq!(
            configured.validate_attribution().unwrap_err(),
            format!("image model exceeds {MAX_ATTRIBUTION_CHARS} characters")
        );
    }

    #[test]
    fn maximum_valid_route_leaves_room_for_fallback_and_cost() {
        let fallback = "allowed; actual use unknown";
        let cost = format!("${:.4}", u64::MAX as f64 / 1_000_000.0);
        let fixed = format!("route configured ; fallback {fallback}; {cost}")
            .chars()
            .count();
        let mut configured = config(ProviderKind::OpenAi, "http://127.0.0.1:9".into());
        configured.route = "x".repeat(MAX_ATTRIBUTION_CHARS - fixed);
        configured.validate_attribution().unwrap();
        let credit = format!(
            "route configured {}; fallback {fallback}; {cost}",
            configured.route
        );
        assert_eq!(credit.chars().count(), MAX_ATTRIBUTION_CHARS);

        configured.route.push('x');
        assert!(configured.validate_attribution().is_err());
    }

    #[test]
    fn invalid_resolved_attribution_keeps_the_valid_generated_image() {
        let oversized = "x".repeat(MAX_ATTRIBUTION_CHARS + 1);
        let response = Box::leak(
            format!(
                r#"{{"provider":"{oversized}","model":" ","choices":[{{"message":{{"images":[{{"image_url":{{"url":"data:image/png;base64,iVBORw=="}}}}]}}}}]}}"#
            )
            .into_boxed_str(),
        );
        let (url, _request, worker) = mock(200, response);
        let generated = config(ProviderKind::OpenRouter, url)
            .generate("unit", &AtomicBool::new(false))
            .unwrap();
        worker.join().unwrap();
        assert_eq!(generated.png, b"\x89PNG");
        assert_eq!(
            generated.provider,
            "unknown (reported provider metadata invalid)"
        );
        assert_eq!(generated.model, "unknown (reported model metadata invalid)");
        let cost = generated
            .cost_micros
            .map(|value| format!("${:.4}", value as f64 / 1_000_000.0))
            .unwrap_or_else(|| "unknown cost".into());
        crate::task_session::ImageAttribution::new(
            &generated.provider,
            Some(generated.model.clone()),
            Some(format!(
                "route {}; fallback {}; {cost}",
                generated.route, generated.fallback
            )),
        )
        .unwrap();
    }

    #[test]
    fn provider_errors_and_precanceled_requests_are_deterministic() {
        let (url, _request, worker) = mock(400, r#"{"error":{"message":"secret detail"}}"#);
        let error = config(ProviderKind::OpenAi, url)
            .generate("unit", &AtomicBool::new(false))
            .unwrap_err();
        worker.join().unwrap();
        assert_eq!(error, "image provider request failed with status 400");
        let canceled = AtomicBool::new(true);
        assert_eq!(
            config(ProviderKind::OpenAi, "http://127.0.0.1:9".into())
                .generate("unit", &canceled)
                .unwrap_err(),
            "image generation canceled"
        );
    }

    #[test]
    fn non_json_provider_failure_preserves_status_without_exposing_body() {
        let (url, _request, worker) = mock(429, "private gateway error details");
        let error = config(ProviderKind::OpenAi, url)
            .generate("unit", &AtomicBool::new(false))
            .unwrap_err();
        worker.join().unwrap();
        assert_eq!(error, "image provider request failed with status 429");

        let (url, _request, worker) = mock(200, "invalid successful response");
        let error = config(ProviderKind::OpenAi, url)
            .generate("unit", &AtomicBool::new(false))
            .unwrap_err();
        worker.join().unwrap();
        assert_eq!(error, "image provider returned an invalid response");
    }

    #[test]
    fn image_routing_rejects_thresholds_that_need_text_endpoint_preflight() {
        let mut routing = crate::openrouter::RoutingConfig::default();
        routing.hard_min_throughput = Some(1.0);
        assert!(validate_image_routing(&routing).is_err());
        routing.hard_min_throughput = None;
        routing.preferred_min_throughput = Some(1.0);
        routing.preferred_throughput_policy = crate::openrouter::PreferredThroughputPolicy::Fail;
        assert!(validate_image_routing(&routing).is_err());
        routing.preferred_throughput_policy =
            crate::openrouter::PreferredThroughputPolicy::AllowBelow;
        validate_image_routing(&routing).unwrap();
    }
}
