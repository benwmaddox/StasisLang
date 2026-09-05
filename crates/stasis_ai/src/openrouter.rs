use crate::{
    decode_model_response, model_response_schema_for_request, ModelProvider, ModelResponse,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::future::Future;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-oss-120b";
const DEFAULT_OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Codex,
    Openrouter,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoutingSort {
    Price,
    Throughput,
    Latency,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PreferredThroughputPolicy {
    AllowBelow,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RoutingConfig {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub only: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub order: Vec<String>,
    pub allow_fallbacks: bool,
    pub sort: RoutingSort,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preferred_min_throughput: Option<f64>,
    pub preferred_throughput_policy: PreferredThroughputPolicy,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hard_min_throughput: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_price: Option<f64>,
}

impl Default for RoutingConfig {
    fn default() -> Self {
        Self {
            only: Vec::new(),
            order: Vec::new(),
            allow_fallbacks: true,
            sort: RoutingSort::Throughput,
            preferred_min_throughput: None,
            preferred_throughput_policy: PreferredThroughputPolicy::AllowBelow,
            hard_min_throughput: None,
            max_price: None,
        }
    }
}

#[derive(Clone)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub routing: RoutingConfig,
    pub timeout: Duration,
}

impl OpenRouterConfig {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("OPENROUTER_API_KEY").map_err(|_| {
            "OPENROUTER_API_KEY is required when STASIS_AI_PROVIDER=openrouter".to_string()
        })?;
        let config = Self {
            api_key,
            base_url: env_nonempty("STASIS_OPENROUTER_URL").unwrap_or_else(|| DEFAULT_OPENROUTER_URL.to_string()),
            model: env_nonempty("STASIS_AI_MODEL").unwrap_or_else(|| DEFAULT_OPENROUTER_MODEL.to_string()),
            routing: RoutingConfig {
                only: env_list("STASIS_AI_ROUTE_ONLY"),
                order: env_list("STASIS_AI_ROUTE_ORDER"),
                allow_fallbacks: env_bool("STASIS_AI_ALLOW_FALLBACKS", true)?,
                sort: match env_nonempty("STASIS_AI_ROUTE_SORT").as_deref().unwrap_or("throughput") {
                    "price" => RoutingSort::Price,
                    "throughput" => RoutingSort::Throughput,
                    "latency" => RoutingSort::Latency,
                    value => return Err(format!("STASIS_AI_ROUTE_SORT must be price, throughput, or latency; got {value}")),
                },
                preferred_min_throughput: env_f64("STASIS_AI_PREFERRED_MIN_THROUGHPUT")?,
                preferred_throughput_policy: match env_nonempty("STASIS_AI_PREFERRED_THROUGHPUT_POLICY").as_deref().unwrap_or("allow_below") {
                    "allow_below" => PreferredThroughputPolicy::AllowBelow,
                    "fail" => PreferredThroughputPolicy::Fail,
                    value => return Err(format!("STASIS_AI_PREFERRED_THROUGHPUT_POLICY must be allow_below or fail; got {value}")),
                },
                hard_min_throughput: env_f64("STASIS_AI_HARD_MIN_THROUGHPUT")?,
                max_price: env_f64("STASIS_AI_MAX_PRICE")?,
            },
            timeout: Duration::from_secs(env_u64("STASIS_AI_TIMEOUT_SECONDS")?.unwrap_or(120)),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.api_key.trim().is_empty() {
            return Err("OPENROUTER_API_KEY must not be empty".to_string());
        }
        if self.model.trim().is_empty() || !self.model.contains('/') {
            return Err("OpenRouter model must be a non-empty author/model identifier".to_string());
        }
        if self.model.len() > 256 || self.model.chars().any(char::is_control) {
            return Err("OpenRouter model must be at most 256 printable characters".to_string());
        }
        for (field, values) in [("only", &self.routing.only), ("order", &self.routing.order)] {
            if let Some(value) = values
                .iter()
                .find(|value| normalize_provider_slug(value).is_none())
            {
                return Err(format!(
                    "OpenRouter provider {field} value is not a valid slug: {value}"
                ));
            }
        }
        if self.timeout.is_zero() {
            return Err("OpenRouter timeout must be greater than zero".to_string());
        }
        for (name, value) in [
            (
                "preferred minimum throughput",
                self.routing.preferred_min_throughput,
            ),
            ("hard minimum throughput", self.routing.hard_min_throughput),
            ("maximum price", self.routing.max_price),
        ] {
            if value.is_some_and(|value| !value.is_finite() || value < 0.0) {
                return Err(format!(
                    "OpenRouter {name} must be a finite non-negative number"
                ));
            }
        }
        if self.routing.hard_min_throughput.is_some()
            && self.routing.preferred_min_throughput.is_some()
        {
            return Err(
                "configure either preferred or hard minimum throughput, not both".to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Clone)]
pub enum ProviderConfig {
    Codex,
    OpenRouter(OpenRouterConfig),
}

impl ProviderConfig {
    pub fn from_env() -> Result<Self, String> {
        match env_nonempty("STASIS_AI_PROVIDER")
            .as_deref()
            .unwrap_or("codex")
        {
            "codex" => Ok(Self::Codex),
            "openrouter" => Ok(Self::OpenRouter(OpenRouterConfig::from_env()?)),
            value => Err(format!(
                "STASIS_AI_PROVIDER must be codex or openrouter; got {value}"
            )),
        }
    }

    pub fn provider_name(&self) -> &'static str {
        match self {
            Self::Codex => "installed_codex_subscription",
            Self::OpenRouter(_) => "openrouter",
        }
    }

    pub fn model(&self) -> String {
        match self {
            Self::Codex => env_nonempty("STASIS_AI_MODEL")
                .unwrap_or_else(|| crate::DEFAULT_CODEX_MODEL.to_string()),
            Self::OpenRouter(config) => config.model.clone(),
        }
    }

    pub fn build(self) -> Result<ConfiguredProvider, String> {
        Ok(match self {
            Self::Codex => ConfiguredProvider::Codex(crate::CodexExecProvider::default()),
            Self::OpenRouter(config) => {
                ConfiguredProvider::OpenRouter(OpenRouterProvider::new(config)?)
            }
        })
    }
}

pub enum ConfiguredProvider {
    Codex(crate::CodexExecProvider),
    OpenRouter(OpenRouterProvider),
}

impl ConfiguredProvider {
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        match &mut self {
            Self::Codex(provider) => provider.request_timeout = Some(timeout),
            Self::OpenRouter(provider) => provider.config.timeout = timeout,
        }
        self
    }

    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        let model = model.into();
        match &mut self {
            Self::Codex(provider) => provider.model = model,
            Self::OpenRouter(provider) => provider.config.model = model,
        }
        self
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: impl Into<String>) -> Self {
        if let Self::Codex(provider) = &mut self {
            provider.reasoning_effort = reasoning_effort.into();
        }
        self
    }

    pub fn with_images(mut self, images: Vec<std::path::PathBuf>) -> Result<Self, String> {
        match &mut self {
            Self::Codex(provider) => provider.images = images,
            Self::OpenRouter(_) if !images.is_empty() => {
                return Err("OpenRouter transport does not support image attachments in this workspace flow".to_string());
            }
            Self::OpenRouter(_) => {}
        }
        Ok(self)
    }

    pub fn with_web_search(mut self, enabled: bool) -> Result<Self, String> {
        match &mut self {
            Self::Codex(provider) => provider.web_search = enabled,
            Self::OpenRouter(_) if enabled => {
                return Err(
                    "OpenRouter transport does not support Codex web search in this workspace flow"
                        .to_string(),
                );
            }
            Self::OpenRouter(_) => {}
        }
        Ok(self)
    }
    pub fn call_count(&self) -> u32 {
        match self {
            Self::Codex(provider) => provider.call_count(),
            Self::OpenRouter(provider) => provider.call_count,
        }
    }
}

impl ModelProvider for ConfiguredProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String> {
        match self {
            Self::Codex(provider) => provider.respond(request, canceled),
            Self::OpenRouter(provider) => provider.respond(request, canceled),
        }
    }
    fn take_usage(&mut self) -> Option<Value> {
        match self {
            Self::Codex(provider) => provider.take_usage().map(|usage| {
                json!({
                    "configured_provider": "codex",
                    "configured_model": provider.model,
                    "resolved_provider": "installed_codex_subscription",
                    "resolved_model": provider.model,
                    "route": "direct",
                    "fallback": false,
                    "tokens": usage,
                })
            }),
            Self::OpenRouter(provider) => provider.take_usage(),
        }
    }

    fn requires_action_ids(&self) -> bool {
        true
    }
}

pub struct OpenRouterProvider {
    config: OpenRouterConfig,
    client: Client,
    last_usage: Option<Value>,
    call_count: u32,
}

impl OpenRouterProvider {
    pub fn new(config: OpenRouterConfig) -> Result<Self, String> {
        config.validate()?;
        let client = Client::builder()
            .connect_timeout(config.timeout.min(Duration::from_secs(30)))
            .build()
            .map_err(|error| format!("failed configuring OpenRouter HTTPS client: {error}"))?;
        Ok(Self {
            config,
            client,
            last_usage: None,
            call_count: 0,
        })
    }

    #[cfg(test)]
    fn qualifying_endpoints(
        &self,
        minimum: f64,
        canceled: &AtomicBool,
    ) -> Result<(Vec<String>, Duration), String> {
        let started = Instant::now();
        let deadline = started + self.config.timeout;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed configuring OpenRouter async runtime: {error}"))?;
        runtime.block_on(self.qualifying_endpoints_async(minimum, deadline, canceled))
    }

    async fn qualifying_endpoints_async(
        &self,
        minimum: f64,
        deadline: Instant,
        canceled: &AtomicBool,
    ) -> Result<(Vec<String>, Duration), String> {
        let started = Instant::now();
        let timeout = remaining_timeout(deadline, "OpenRouter endpoint preflight")?;
        let url = format!(
            "{}/models/{}/endpoints",
            self.config.base_url.trim_end_matches('/'),
            self.config.model
        );
        let response = await_cancelable(
            self.client
                .get(url)
                .bearer_auth(&self.config.api_key)
                .timeout(timeout)
                .send(),
            canceled,
            deadline,
            "OpenRouter endpoint preflight",
        )
        .await?;
        let status = response.status();
        let value: Value = await_cancelable(
            response.json(),
            canceled,
            deadline,
            "OpenRouter endpoint preflight response",
        )
        .await?;
        if !status.is_success() {
            return Err(api_error(
                "OpenRouter endpoint preflight",
                status.as_u16(),
                &value,
                &self.config.api_key,
            ));
        }
        let endpoints = value
            .pointer("/data/endpoints")
            .or_else(|| value.get("data"))
            .or_else(|| value.get("endpoints"))
            .and_then(Value::as_array)
            .ok_or_else(|| "OpenRouter endpoint preflight omitted endpoint metadata".to_string())?;
        let mut tags = Vec::new();
        for endpoint in endpoints {
            let throughput = endpoint
                .pointer("/throughput_last_30m/p50")
                .and_then(Value::as_f64)
                .or_else(|| endpoint.get("throughput_last_30m").and_then(Value::as_f64))
                .or_else(|| endpoint.get("throughput").and_then(Value::as_f64))
                .or_else(|| {
                    endpoint
                        .pointer("/metrics/throughput")
                        .and_then(Value::as_f64)
                });
            let healthy = match endpoint.get("status") {
                Some(Value::Number(status)) => status.as_i64() == Some(0),
                Some(Value::String(status)) => matches!(
                    status.to_ascii_lowercase().as_str(),
                    "healthy" | "available" | "active" | "up"
                ),
                Some(_) => false,
                None => true,
            };
            if healthy && throughput.is_some_and(|value| value >= minimum) {
                if let Some(tag) = endpoint
                    .get("provider")
                    .or_else(|| endpoint.get("provider_slug"))
                    .or_else(|| endpoint.get("tag"))
                    .and_then(Value::as_str)
                    .and_then(normalize_provider_slug)
                {
                    tags.push(tag);
                }
            }
        }
        tags.sort();
        tags.dedup();
        if !self.config.routing.only.is_empty() {
            tags.retain(|tag| {
                self.config
                    .routing
                    .only
                    .iter()
                    .filter_map(|only| normalize_provider_slug(only))
                    .any(|only| only == *tag)
            });
        }
        if tags.is_empty() {
            return Err(format!("OpenRouter routing failed closed: no healthy endpoint for {} provides at least {minimum} tokens/s", self.config.model));
        }
        Ok((tags, started.elapsed()))
    }

    fn route_json(&self, hard_only: Option<Vec<String>>) -> Value {
        let routing = &self.config.routing;
        let mut value = json!({
            "allow_fallbacks": routing.allow_fallbacks,
            "sort": match routing.sort { RoutingSort::Price => "price", RoutingSort::Throughput => "throughput", RoutingSort::Latency => "latency" },
            "require_parameters": true,
        });
        let object = value.as_object_mut().expect("route object");
        let only = hard_only
            .unwrap_or_else(|| routing.only.clone())
            .into_iter()
            .filter_map(|value| normalize_provider_slug(&value))
            .collect::<Vec<_>>();
        if !only.is_empty() {
            object.insert("only".to_string(), json!(only));
        }
        if !routing.order.is_empty() {
            object.insert(
                "order".to_string(),
                json!(routing
                    .order
                    .iter()
                    .filter_map(|value| normalize_provider_slug(value))
                    .collect::<Vec<_>>()),
            );
        }
        if let Some(target) = routing.preferred_min_throughput {
            object.insert("preferred_min_throughput".to_string(), json!(target));
        }
        if let Some(max_price) = routing.max_price {
            object.insert("max_price".to_string(), json!({"completion": max_price}));
        }
        value
    }
}

impl ModelProvider for OpenRouterProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String> {
        let turn_started = Instant::now();
        let deadline = turn_started + self.config.timeout;
        self.call_count = self.call_count.saturating_add(1);
        self.last_usage = None;
        self.config.validate()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed configuring OpenRouter async runtime: {error}"))?;
        runtime.block_on(self.respond_async(request, canceled, turn_started, deadline))
    }

    fn take_usage(&mut self) -> Option<Value> {
        self.last_usage.take()
    }

    fn requires_action_ids(&self) -> bool {
        true
    }
}

impl OpenRouterProvider {
    async fn respond_async(
        &mut self,
        request: &str,
        canceled: &AtomicBool,
        turn_started: Instant,
        deadline: Instant,
    ) -> Result<ModelResponse, String> {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let (hard_only, metadata_time) = match self.config.routing.hard_min_throughput {
            Some(minimum) => {
                let (tags, elapsed) = self
                    .qualifying_endpoints_async(minimum, deadline, canceled)
                    .await?;
                (Some(tags), elapsed)
            }
            None => (None, Duration::ZERO),
        };
        if self.config.routing.preferred_throughput_policy == PreferredThroughputPolicy::Fail {
            if let Some(minimum) = self.config.routing.preferred_min_throughput {
                let (tags, elapsed) = self
                    .qualifying_endpoints_async(minimum, deadline, canceled)
                    .await?;
                return self
                    .send(
                        request,
                        Some(tags),
                        metadata_time + elapsed,
                        turn_started,
                        deadline,
                        canceled,
                    )
                    .await;
            }
        }
        self.send(
            request,
            hard_only,
            metadata_time,
            turn_started,
            deadline,
            canceled,
        )
        .await
    }

    async fn send(
        &mut self,
        request: &str,
        only: Option<Vec<String>>,
        metadata_time: Duration,
        turn_started: Instant,
        deadline: Instant,
        canceled: &AtomicBool,
    ) -> Result<ModelResponse, String> {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let timeout = remaining_timeout(deadline, "OpenRouter chat request")?;
        let route = self.route_json(only);
        let schema = model_response_schema_for_request(request)?;
        let body = json!({
            "model": self.config.model,
            "messages": [{"role": "user", "content": request}],
            "stream": true,
            "stream_options": {"include_usage": true},
            "response_format": {"type": "json_schema", "json_schema": {"name": "stasis_model_response", "strict": true, "schema": schema}},
            "provider": route,
        });
        let request_started = Instant::now();
        let mut response = await_cancelable(
            self.client
                .post(format!(
                    "{}/chat/completions",
                    self.config.base_url.trim_end_matches('/')
                ))
                .bearer_auth(&self.config.api_key)
                .json(&body)
                .timeout(timeout)
                .send(),
            canceled,
            deadline,
            "OpenRouter request",
        )
        .await?;
        let header_time = request_started.elapsed();
        let status = response.status();
        if !status.is_success() {
            let value = match await_cancelable(
                response.json::<Value>(),
                canceled,
                deadline,
                "OpenRouter request error response",
            )
            .await
            {
                Ok(value) => value,
                Err(error) if error.contains("timed out") || error == "AI request canceled" => {
                    return Err(error)
                }
                Err(_) => Value::Null,
            };
            return Err(api_error(
                "OpenRouter request",
                status.as_u16(),
                &value,
                &self.config.api_key,
            ));
        }
        let mut stream = SseDecoder::default();
        let mut content = String::new();
        let mut reasoning = String::new();
        let mut usage = Value::Null;
        let mut resolved_model = None;
        let mut resolved_provider = None;
        let mut first_reasoning_ms = None;
        let mut first_content_ms = None;
        let mut first_action_ms = None;
        let mut saw_done = false;
        loop {
            let Some(bytes) =
                await_cancelable(response.chunk(), canceled, deadline, "OpenRouter stream").await?
            else {
                break;
            };
            for event in stream.push(&bytes)? {
                if event == "[DONE]" {
                    saw_done = true;
                    continue;
                }
                let chunk: Value = serde_json::from_str(&event).map_err(|error| {
                    format!("OpenRouter stream returned invalid JSON event: {error}")
                })?;
                if let Some(error) = chunk.get("error") {
                    return Err(api_error(
                        "OpenRouter stream",
                        status.as_u16(),
                        error,
                        &self.config.api_key,
                    ));
                }
                resolved_model = chunk
                    .get("model")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(resolved_model);
                resolved_provider = chunk
                    .get("provider")
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .or(resolved_provider);
                if let Some(value) = chunk
                    .pointer("/choices/0/delta/reasoning")
                    .and_then(Value::as_str)
                {
                    if !value.is_empty() && first_reasoning_ms.is_none() {
                        first_reasoning_ms = Some(duration_ms(request_started.elapsed()));
                    }
                    reasoning.push_str(value);
                }
                if let Some(value) = chunk
                    .pointer("/choices/0/delta/content")
                    .and_then(Value::as_str)
                {
                    if !value.is_empty() && first_content_ms.is_none() {
                        first_content_ms = Some(duration_ms(request_started.elapsed()));
                    }
                    content.push_str(value);
                    if first_action_ms.is_none()
                        && (content.contains("\"action_id\"") || content.contains("\"tool_calls\""))
                    {
                        first_action_ms = Some(duration_ms(request_started.elapsed()));
                    }
                }
                if let Some(value) = chunk.get("usage") {
                    usage = value.clone();
                }
            }
        }
        for event in stream.finish()? {
            if event == "[DONE]" {
                saw_done = true;
            } else {
                return Err("OpenRouter stream ended with an incomplete event".to_string());
            }
        }
        if !saw_done {
            return Err("OpenRouter stream ended before the [DONE] marker".to_string());
        }
        let parsed = decode_model_response(&content, "OpenRouter")?;
        let resolved_model = resolved_model
            .as_deref()
            .map(sanitize_label)
            .unwrap_or_else(|| sanitize_label(&self.config.model));
        let resolved_provider = resolved_provider
            .as_deref()
            .and_then(normalize_provider_slug)
            .unwrap_or_else(|| "unknown".to_string());
        let fallback = self
            .config
            .routing
            .order
            .first()
            .and_then(|value| normalize_provider_slug(value))
            .is_some_and(|first| first != resolved_provider);
        let prompt_tokens = metric_number(usage.get("prompt_tokens"));
        let completion_tokens = metric_number(usage.get("completion_tokens"));
        let reasoning_tokens =
            metric_number(usage.pointer("/completion_tokens_details/reasoning_tokens"));
        let cache_tokens = metric_number(usage.pointer("/prompt_tokens_details/cached_tokens"));
        self.last_usage = Some(json!({
            "configured_provider": "openrouter", "configured_model": self.config.model,
            "resolved_provider": resolved_provider, "resolved_model": resolved_model,
            "route": route, "fallback": fallback,
            "timing_ms": {"metadata": duration_ms(metadata_time), "headers": duration_ms(header_time), "first_reasoning": first_reasoning_ms, "first_content": first_content_ms, "first_action": first_action_ms, "inference_total": duration_ms(request_started.elapsed()), "turn_total": duration_ms(turn_started.elapsed())},
            "tokens": {"prompt": prompt_tokens, "completion": completion_tokens, "reasoning": reasoning_tokens, "cache": cache_tokens},
            "cost": metric_number(usage.get("cost")),
            "throughput_tokens_per_second": throughput(&usage, request_started.elapsed()),
            "validation": {"structured_schema": "accepted", "repair_count": 0}
        }));
        Ok(parsed)
    }
}

#[derive(Default)]
struct SseDecoder {
    pending: Vec<u8>,
    data: Vec<String>,
}
impl SseDecoder {
    fn push(&mut self, bytes: &[u8]) -> Result<Vec<String>, String> {
        self.pending.extend_from_slice(bytes);
        let mut events = Vec::new();
        while let Some(position) = self.pending.iter().position(|byte| *byte == b'\n') {
            let mut line = self.pending.drain(..=position).collect::<Vec<_>>();
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            let line = std::str::from_utf8(&line)
                .map_err(|_| "OpenRouter stream contained invalid UTF-8".to_string())?;
            if line.is_empty() {
                if !self.data.is_empty() {
                    events.push(self.data.join("\n"));
                    self.data.clear();
                }
            } else if let Some(value) = line.strip_prefix("data:") {
                self.data
                    .push(value.strip_prefix(' ').unwrap_or(value).to_string());
            }
        }
        Ok(events)
    }
    fn finish(mut self) -> Result<Vec<String>, String> {
        let mut events = self.push(b"\n\n")?;
        if !self.pending.is_empty() || !self.data.is_empty() {
            return Err("OpenRouter stream ended mid-event".to_string());
        }
        Ok(std::mem::take(&mut events))
    }
}

fn remaining_timeout(deadline: Instant, context: &str) -> Result<Duration, String> {
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        Err(format!(
            "{context}: timed out before the request could continue"
        ))
    } else {
        Ok(remaining)
    }
}

async fn await_cancelable<F, T>(
    future: F,
    canceled: &AtomicBool,
    deadline: Instant,
    context: &str,
) -> Result<T, String>
where
    F: Future<Output = Result<T, reqwest::Error>>,
{
    tokio::pin!(future);
    loop {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let remaining = remaining_timeout(deadline, context)?;
        let poll_interval = remaining.min(Duration::from_millis(10));
        tokio::select! {
            result = &mut future => {
                return result.map_err(|error| sanitized_transport_error(context, &error));
            }
            _ = tokio::time::sleep(poll_interval) => {}
        }
    }
}

fn sanitize_label(value: &str) -> String {
    value
        .chars()
        .filter(|character| !character.is_control())
        .take(256)
        .collect()
}

fn metric_number(value: Option<&Value>) -> Value {
    match value {
        Some(Value::Number(number)) => Value::Number(number.clone()),
        _ => Value::Null,
    }
}
fn normalize_provider_slug(value: &str) -> Option<String> {
    let normalized = value.trim().to_ascii_lowercase();
    (!normalized.is_empty()
        && normalized
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')))
    .then_some(normalized)
}
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
fn env_list(name: &str) -> Vec<String> {
    env_nonempty(name)
        .map(|value| {
            value
                .split(',')
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}
fn env_bool(name: &str, default: bool) -> Result<bool, String> {
    match env_nonempty(name).as_deref() {
        None => Ok(default),
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        Some(value) => Err(format!("{name} must be true or false; got {value}")),
    }
}
fn env_f64(name: &str) -> Result<Option<f64>, String> {
    env_nonempty(name)
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| format!("{name} must be a number"))
        })
        .transpose()
}
fn env_u64(name: &str) -> Result<Option<u64>, String> {
    env_nonempty(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|_| format!("{name} must be an integer"))
        })
        .transpose()
}
fn duration_ms(value: Duration) -> u64 {
    u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
}
fn throughput(usage: &Value, elapsed: Duration) -> Value {
    usage
        .get("completion_tokens")
        .and_then(Value::as_f64)
        .filter(|_| !elapsed.is_zero())
        .map(|tokens| json!(tokens / elapsed.as_secs_f64()))
        .unwrap_or(Value::Null)
}
fn sanitized_transport_error(context: &str, error: &reqwest::Error) -> String {
    if error.is_timeout() {
        format!("{context}: timed out")
    } else if error.is_connect() {
        format!("{context}: connection failed")
    } else {
        format!("{context}: transport error")
    }
}
fn api_error(context: &str, status: u16, value: &Value, secret: &str) -> String {
    let message = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("request rejected");
    let bounded = message
        .chars()
        .filter(|ch| !ch.is_control())
        .take(300)
        .collect::<String>()
        .replace(secret, "[redacted]");
    format!("{context} failed with HTTP {status}: {bounded}")
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routing_serializes_all_knobs() {
        let routing = RoutingConfig {
            only: vec!["CeReBrAs".into()],
            order: vec!["CeReBrAs".into(), "OpenAI".into()],
            allow_fallbacks: false,
            sort: RoutingSort::Price,
            preferred_min_throughput: Some(1500.0),
            preferred_throughput_policy: PreferredThroughputPolicy::AllowBelow,
            hard_min_throughput: None,
            max_price: Some(0.8),
        };
        let config = OpenRouterConfig {
            api_key: "secret".into(),
            base_url: DEFAULT_OPENROUTER_URL.into(),
            model: DEFAULT_OPENROUTER_MODEL.into(),
            routing,
            timeout: Duration::from_secs(2),
        };
        let provider = OpenRouterProvider::new(config).expect("provider");
        assert_eq!(
            provider.route_json(None),
            json!({"only":["cerebras"], "order":["cerebras","openai"], "allow_fallbacks":false, "sort":"price", "require_parameters":true, "preferred_min_throughput":1500.0, "max_price":{"completion":0.8}})
        );
    }

    #[test]
    fn hard_and_preferred_thresholds_cannot_be_mixed() {
        let config = OpenRouterConfig {
            api_key: "secret".into(),
            base_url: DEFAULT_OPENROUTER_URL.into(),
            model: DEFAULT_OPENROUTER_MODEL.into(),
            routing: RoutingConfig {
                hard_min_throughput: Some(1.0),
                preferred_min_throughput: Some(2.0),
                ..RoutingConfig::default()
            },
            timeout: Duration::from_secs(2),
        };
        assert!(config
            .validate()
            .unwrap_err()
            .contains("either preferred or hard"));
    }

    #[test]
    fn sse_decoder_handles_split_events() {
        let mut decoder = SseDecoder::default();
        assert!(decoder.push(b"da").unwrap().is_empty());
        assert!(decoder
            .push(b"ta: {\"choices\":[{\"delta\":{")
            .unwrap()
            .is_empty());
        assert_eq!(
            decoder.push(b"\"content\":\"ok\"}}]}\r\n\r\n").unwrap(),
            vec![r#"{"choices":[{"delta":{"content":"ok"}}]}"#]
        );
        assert!(decoder.finish().unwrap().is_empty());
    }

    #[test]
    fn transport_errors_do_not_expose_secret_material() {
        let error = api_error(
            "OpenRouter request",
            401,
            &json!({"error":{"message":"unauthorized unit-secret"}}),
            "unit-secret",
        );
        assert_eq!(
            error,
            "OpenRouter request failed with HTTP 401: unauthorized [redacted]"
        );
        assert!(!error.contains("secret"));
    }
    fn mock_server(
        responses: Vec<String>,
    ) -> (
        String,
        std::sync::mpsc::Receiver<String>,
        std::thread::JoinHandle<()>,
    ) {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("mock listener");
        let address = listener.local_addr().expect("mock address");
        let (sent, received) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("mock accept");
                let mut request = Vec::new();
                let mut buffer = [0_u8; 2048];
                let header_end = loop {
                    let count = stream.read(&mut buffer).expect("mock read");
                    if count == 0 {
                        panic!("request ended before headers");
                    }
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n")
                    {
                        break index + 4;
                    }
                };
                let headers = String::from_utf8_lossy(&request[..header_end]);
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().ok())
                            .flatten()
                    })
                    .unwrap_or(0);
                while request.len() < header_end + content_length {
                    let count = stream.read(&mut buffer).expect("mock body read");
                    if count == 0 {
                        break;
                    }
                    request.extend_from_slice(&buffer[..count]);
                }
                sent.send(String::from_utf8_lossy(&request).into_owned())
                    .expect("capture request");
                stream
                    .write_all(response.as_bytes())
                    .expect("mock response");
            }
        });
        (format!("http://{address}"), received, worker)
    }

    fn http_response(content_type: &str, body: &str) -> String {
        format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len())
    }

    fn test_config(base_url: String) -> OpenRouterConfig {
        OpenRouterConfig {
            api_key: "unit-secret".into(),
            base_url,
            model: DEFAULT_OPENROUTER_MODEL.into(),
            routing: RoutingConfig::default(),
            timeout: Duration::from_secs(2),
        }
    }

    fn test_request() -> String {
        json!({"tool_specs": crate::workshop_tool_specs()}).to_string()
    }
    #[test]
    fn openrouter_stream_and_codex_fixture_decode_identically() {
        let read_id = crate::workshop_tool_specs()
            .into_iter()
            .find(|spec| spec.tool == "read_symbol")
            .expect("read spec")
            .action_id;
        let fixture = json!({
            "mode":"tool_calls", "working_notes":"Read the target.", "summary":"",
            "tool_calls":[{"action_id":read_id.clone(),"args":{"name":"tick"}}]
        });
        let chunk = json!({"model":DEFAULT_OPENROUTER_MODEL,"provider":"cerebras","choices":[{"delta":{"content":fixture.to_string()}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"completion_tokens_details":{"reasoning_tokens":1},"prompt_tokens_details":{"cached_tokens":2},"cost":0.001}});
        let body = format!("data: {}\n\ndata: [DONE]\n\n", chunk);
        let (base_url, requests, worker) =
            mock_server(vec![http_response("text/event-stream", &body)]);
        let mut provider = OpenRouterProvider::new(test_config(base_url)).expect("provider");
        let openrouter = provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect("stream response");
        let codex = crate::decode_codex_response(&fixture.to_string()).expect("Codex fixture");
        assert_eq!(openrouter, codex);
        let request = requests.recv().expect("captured request");
        let json_start = request.find("\r\n\r\n").expect("headers") + 4;
        let request: Value = serde_json::from_str(&request[json_start..]).expect("request JSON");
        assert_eq!(
            request.pointer("/response_format/json_schema/strict"),
            Some(&json!(true))
        );
        let variants = request
            .pointer("/response_format/json_schema/schema/properties/tool_calls/items/anyOf")
            .and_then(Value::as_array)
            .expect("per-action variants");
        let read_variant = variants
            .iter()
            .find(|variant| {
                variant.pointer("/properties/action_id/enum/0") == Some(&json!(read_id))
            })
            .expect("read-symbol variant");
        assert_eq!(
            read_variant.pointer("/properties/args/type"),
            Some(&json!("object"))
        );
        assert_eq!(
            read_variant.pointer("/properties/args/additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            request.pointer("/provider/require_parameters"),
            Some(&json!(true))
        );
        let usage = provider.take_usage().expect("usage");
        assert_eq!(usage["resolved_provider"], "cerebras");
        assert_eq!(usage["tokens"]["cache"], 2);
        assert!(usage["timing_ms"]["first_action"].is_number());
        assert!(usage["timing_ms"]["inference_total"].is_number());
        assert!(usage["timing_ms"]["turn_total"].is_number());
        assert!(usage["timing_ms"].get("total").is_none());
        worker.join().expect("mock worker");
    }

    #[test]
    fn official_endpoint_metadata_uses_p50_numeric_status_and_normalized_slug() {
        let metadata = json!({"data":{"endpoints":[
            {"provider":"CEREBRAS","provider_name":"Cerebras Display", "status":0,
             "throughput_last_30m":{"p50":1250.0,"p75":1400.0,"p90":1500.0,"p99":1600.0}},
            {"provider":"unhealthy","status":1,"throughput_last_30m":{"p50":9000.0}},
            {"provider_name":"Display Name Is Not A Slug","status":0,"throughput_last_30m":{"p50":9000.0}}
        ]}}).to_string();
        let (base_url, _requests, worker) =
            mock_server(vec![http_response("application/json", &metadata)]);
        let mut config = test_config(base_url);
        config.routing.only = vec!["CeReBrAs".into()];
        let provider = OpenRouterProvider::new(config).expect("provider");
        let (qualifying, _) = provider
            .qualifying_endpoints(1000.0, &AtomicBool::new(false))
            .expect("official metadata");
        assert_eq!(qualifying, vec!["cerebras"]);
        worker.join().expect("mock worker");
    }
    #[test]
    fn hard_throughput_preflight_fails_closed_without_chat_request() {
        let metadata = json!({"data":{"endpoints":[{"tag":"cerebras","status":"healthy","throughput":900.0}]}}).to_string();
        let (base_url, requests, worker) =
            mock_server(vec![http_response("application/json", &metadata)]);
        let mut config = test_config(base_url);
        config.routing.hard_min_throughput = Some(1000.0);
        let mut provider = OpenRouterProvider::new(config).expect("provider");
        let error = provider
            .respond("request", &AtomicBool::new(false))
            .expect_err("must fail closed");
        assert!(error.contains("no healthy endpoint"));
        let request = requests.recv().expect("preflight request");
        assert!(request.starts_with("GET "));
        worker.join().expect("mock worker");
    }

    #[test]
    fn preflight_and_generation_share_one_timeout_deadline() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let worker = std::thread::spawn(move || {
            let (mut metadata_stream, _) = listener.accept().expect("metadata accept");
            let mut buffer = [0_u8; 2048];
            let _ = metadata_stream.read(&mut buffer);
            std::thread::sleep(Duration::from_millis(150));
            let metadata = json!({"data":{"endpoints":[{
                "provider":"cerebras", "status":0,
                "throughput_last_30m":{"p50":1500.0}
            }]}})
            .to_string();
            metadata_stream
                .write_all(http_response("application/json", &metadata).as_bytes())
                .expect("metadata response");
            drop(metadata_stream);

            let (mut chat_stream, _) = listener.accept().expect("chat accept");
            let _ = chat_stream.read(&mut buffer);
            std::thread::sleep(Duration::from_millis(250));
        });

        let mut config = test_config(format!("http://{address}"));
        config.routing.hard_min_throughput = Some(1000.0);
        config.timeout = Duration::from_millis(250);
        let mut provider = OpenRouterProvider::new(config).expect("provider");
        let started = Instant::now();
        let error = provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect_err("shared timeout");
        let elapsed = started.elapsed();
        assert!(error.contains("timed out"));
        assert!(
            elapsed < Duration::from_millis(350),
            "preflight and generation exceeded one timeout budget: {elapsed:?}"
        );
        worker.join().expect("worker");
    }

    #[test]
    fn stalled_transport_respects_timeout_without_leaking_secret() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer);
            std::thread::sleep(Duration::from_millis(100));
            let _ = stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n");
        });
        let config = test_config(format!("http://{address}"));
        let provider = OpenRouterProvider::new(config).expect("provider");
        let mut provider =
            ConfiguredProvider::OpenRouter(provider).with_timeout(Duration::from_millis(20));
        let error = provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect_err("timeout");
        assert!(error.contains("timed out"));
        assert!(!error.contains("unit-secret"));
        worker.join().expect("worker");
    }
    #[test]
    fn canceled_request_never_starts_transport() {
        let mut provider =
            OpenRouterProvider::new(test_config("http://127.0.0.1:9".into())).expect("provider");
        let error = provider
            .respond("request", &AtomicBool::new(true))
            .expect_err("canceled");
        assert_eq!(error, "AI request canceled");
    }

    #[test]
    fn cancellation_drops_a_headers_then_stalled_stream_promptly() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        use std::sync::mpsc;
        use std::sync::Arc;

        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let (headers_sent, headers_received) = mpsc::channel();
        let (release_server, server_released) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept");
            let mut request = Vec::new();
            let mut buffer = [0_u8; 2048];
            let header_end = loop {
                let count = stream.read(&mut buffer).expect("request headers");
                request.extend_from_slice(&buffer[..count]);
                if let Some(index) = request.windows(4).position(|window| window == b"\r\n\r\n") {
                    break index + 4;
                }
            };
            let headers = String::from_utf8_lossy(&request[..header_end]);
            let content_length = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().ok())
                        .flatten()
                })
                .unwrap_or(0);
            while request.len() < header_end + content_length {
                let count = stream.read(&mut buffer).expect("request body");
                request.extend_from_slice(&buffer[..count]);
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: keep-alive\r\n\r\n")
                .expect("response headers");
            headers_sent.send(()).expect("header notification");
            let _ = server_released.recv_timeout(Duration::from_secs(2));
        });

        let canceled = Arc::new(AtomicBool::new(false));
        let worker_canceled = canceled.clone();
        let mut config = test_config(format!("http://{address}"));
        config.timeout = Duration::from_secs(30);
        let provider = OpenRouterProvider::new(config).expect("provider");
        let request = test_request();
        let started = Instant::now();
        let requester = std::thread::spawn(move || {
            let mut provider = provider;
            provider.respond(&request, &worker_canceled)
        });
        headers_received
            .recv_timeout(Duration::from_secs(2))
            .expect("headers received");
        canceled.store(true, Ordering::Release);
        let error = requester
            .join()
            .expect("request thread")
            .expect_err("canceled");
        assert_eq!(error, "AI request canceled");
        assert!(started.elapsed() < Duration::from_secs(1));
        release_server.send(()).expect("release server");
        worker.join().expect("server thread");
    }
}
