use crate::{
    decode_model_response, model_response_schema_for_request, ModelProvider, ModelResponse,
};
use base64::Engine as _;
use reqwest::{header::RETRY_AFTER, Client, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::future::Future;
use std::io::Read;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

pub const DEFAULT_OPENROUTER_MODEL: &str = "openai/gpt-oss-120b";
pub const APPROVED_OPENROUTER_MODELS: &[&str] = &[DEFAULT_OPENROUTER_MODEL];
pub const DEFAULT_OPENROUTER_MIN_THROUGHPUT: f64 = 400.0;
pub const DEFAULT_OPENROUTER_MAX_LATENCY_SECONDS: f64 = 2.0;
const DEFAULT_OPENROUTER_URL: &str = "https://openrouter.ai/api/v1";
pub const MAX_OPENROUTER_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_OPENROUTER_IMAGES: usize = crate::task_session::MAX_SCREENSHOTS_PER_REQUEST;
const MAX_OPENROUTER_RATE_LIMIT_RETRIES: u32 = 2;
const DEFAULT_OPENROUTER_RETRY_DELAY: Duration = Duration::from_millis(250);
const MAX_OPENROUTER_RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_APPROVED_OPENROUTER_MODELS: usize = 8;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ProjectAiConfig {
    #[serde(default)]
    pub provider: Option<ProjectProvider>,
    #[serde(default)]
    pub openrouter: ProjectOpenRouterConfig,
    #[serde(default)]
    pub editor: ProjectEditorConfig,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProjectProvider {
    Codex,
    OpenRouter,
}

impl ProjectProvider {
    fn label(self) -> &'static str {
        match self {
            Self::Codex => "codex",
            Self::OpenRouter => "openrouter",
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectEditorConfig {
    #[serde(default)]
    pub auto_persist_html_transcripts: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectOpenRouterConfig {
    #[serde(default = "default_approved_openrouter_models")]
    pub approved_models: Vec<String>,
    #[serde(default = "default_openrouter_min_throughput")]
    pub min_throughput_tokens_per_second: u32,
    #[serde(default = "default_openrouter_max_latency_seconds")]
    pub max_p50_latency_seconds: f64,
}

impl Default for ProjectOpenRouterConfig {
    fn default() -> Self {
        Self {
            approved_models: default_approved_openrouter_models(),
            min_throughput_tokens_per_second: default_openrouter_min_throughput(),
            max_p50_latency_seconds: default_openrouter_max_latency_seconds(),
        }
    }
}

fn default_approved_openrouter_models() -> Vec<String> {
    APPROVED_OPENROUTER_MODELS
        .iter()
        .map(|model| (*model).to_string())
        .collect()
}

fn default_openrouter_min_throughput() -> u32 {
    DEFAULT_OPENROUTER_MIN_THROUGHPUT as u32
}

fn default_openrouter_max_latency_seconds() -> f64 {
    DEFAULT_OPENROUTER_MAX_LATENCY_SECONDS
}

#[derive(Deserialize)]
struct ProjectAiManifest {
    #[serde(default)]
    ai: ProjectAiConfig,
}

impl ProjectAiConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.openrouter.validate()
    }

    pub fn from_workspace(root: &Path) -> Result<Self, String> {
        let path = root.join("stasis.json");
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("failed to read {}: {error}", path.display()))?;
        let manifest: ProjectAiManifest = serde_json::from_slice(&bytes)
            .map_err(|error| format!("invalid stasis.json AI configuration: {error}"))?;
        Ok(manifest.ai)
    }
}

impl ProjectOpenRouterConfig {
    fn validate(&self) -> Result<(), String> {
        if self.approved_models.is_empty() {
            return Err("ai.openrouter.approved_models must not be empty".to_string());
        }
        if self.approved_models.len() > MAX_APPROVED_OPENROUTER_MODELS {
            return Err(format!(
                "ai.openrouter.approved_models must contain at most {MAX_APPROVED_OPENROUTER_MODELS} models"
            ));
        }
        let mut unique = std::collections::BTreeSet::new();
        for model in &self.approved_models {
            if model.trim().is_empty()
                || !model.contains('/')
                || model.len() > 256
                || model.chars().any(char::is_control)
            {
                return Err(format!(
                    "ai.openrouter.approved_models contains an invalid author/model identifier: {model}"
                ));
            }
            if !unique.insert(model) {
                return Err("ai.openrouter.approved_models must not contain duplicates".to_string());
            }
        }
        if self.min_throughput_tokens_per_second == 0 {
            return Err(
                "ai.openrouter.min_throughput_tokens_per_second must be greater than zero"
                    .to_string(),
            );
        }
        if !self.max_p50_latency_seconds.is_finite() || self.max_p50_latency_seconds <= 0.0 {
            return Err(
                "ai.openrouter.max_p50_latency_seconds must be a finite number greater than zero"
                    .to_string(),
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRouterImageInput {
    mime_type: &'static str,
    bytes: Arc<[u8]>,
    sha256: String,
}

impl OpenRouterImageInput {
    pub fn new(mime_type: &str, bytes: Vec<u8>, expected_sha256: &str) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > MAX_OPENROUTER_IMAGE_BYTES {
            return Err(format!(
                "image must contain between 1 and {MAX_OPENROUTER_IMAGE_BYTES} bytes"
            ));
        }
        let mime_type = match mime_type.trim().to_ascii_lowercase().as_str() {
            "image/png" if bytes.starts_with(b"\x89PNG\r\n\x1a\n") => "image/png",
            "image/jpeg" if bytes.starts_with(&[0xff, 0xd8, 0xff]) => "image/jpeg",
            "image/png" => return Err("image bytes do not contain a PNG signature".to_string()),
            "image/jpeg" => return Err("image bytes do not contain a JPEG signature".to_string()),
            _ => return Err("image MIME type must be image/png or image/jpeg".to_string()),
        };
        let actual_sha256 = format!("{:x}", Sha256::digest(&bytes));
        if expected_sha256.len() != 64
            || !expected_sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !actual_sha256.eq_ignore_ascii_case(expected_sha256)
        {
            return Err("image bytes changed after selection (SHA-256 mismatch)".to_string());
        }
        Ok(Self {
            mime_type,
            bytes: bytes.into(),
            sha256: actual_sha256,
        })
    }

    pub fn mime_type(&self) -> &'static str {
        self.mime_type
    }
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn sha256(&self) -> &str {
        &self.sha256
    }

    fn data_url(&self) -> String {
        format!(
            "data:{};base64,{}",
            self.mime_type,
            base64::engine::general_purpose::STANDARD.encode(&self.bytes)
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageInputCapability {
    pub model: String,
    pub supported: bool,
    pub reason: String,
}

impl ImageInputCapability {
    fn unknown(model: &str) -> Self {
        Self {
            model: model.to_string(),
            supported: false,
            reason: "image support has not been verified for the configured model".to_string(),
        }
    }
}

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
    pub preferred_max_latency_seconds: Option<f64>,
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
            preferred_max_latency_seconds: None,
            max_price: None,
        }
    }
}

#[derive(Clone)]
pub struct OpenRouterConfig {
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub approved_models: Box<[String]>,
    pub routing: RoutingConfig,
    pub timeout: Duration,
}

impl OpenRouterConfig {
    pub fn from_env() -> Result<Self, String> {
        Self::from_lookup(
            &|name| std::env::var(name).ok(),
            &ProjectOpenRouterConfig::default(),
        )
    }

    /// Read project AI policy from stasis.json and secrets/transport from this workspace's .env.
    pub fn from_workspace(root: &Path) -> Result<Self, String> {
        let settings = WorkspaceSettings::read(root)?;
        let project = ProjectAiConfig::from_workspace(root)?;
        Self::from_lookup(
            &|name| settings.get(name, &|key| std::env::var(key).ok()),
            &project.openrouter,
        )
    }

    fn from_lookup(
        lookup: &dyn Fn(&str) -> Option<String>,
        project: &ProjectOpenRouterConfig,
    ) -> Result<Self, String> {
        project.validate()?;
        let api_key = lookup("OPENROUTER_API_KEY").ok_or_else(|| {
            "OPENROUTER_API_KEY is required when STASIS_AI_PROVIDER=openrouter".to_string()
        })?;
        let config = Self {
            api_key,
            base_url: setting_nonempty(lookup, "STASIS_OPENROUTER_URL")
                .unwrap_or_else(|| DEFAULT_OPENROUTER_URL.to_string()),
            model: project.approved_models.first().cloned().unwrap_or_default(),
            approved_models: project.approved_models.clone().into_boxed_slice(),
            routing: RoutingConfig {
                only: setting_list(lookup, "STASIS_AI_ROUTE_ONLY"),
                order: Vec::new(),
                allow_fallbacks: setting_bool(lookup, "STASIS_AI_ALLOW_FALLBACKS", true)?,
                sort: RoutingSort::Price,
                preferred_min_throughput: Some(project.min_throughput_tokens_per_second.into()),
                preferred_throughput_policy: PreferredThroughputPolicy::AllowBelow,
                hard_min_throughput: None,
                preferred_max_latency_seconds: Some(project.max_p50_latency_seconds),
                max_price: setting_f64(lookup, "STASIS_AI_MAX_PRICE")?,
            },
            timeout: Duration::from_secs(
                setting_u64(lookup, "STASIS_AI_TIMEOUT_SECONDS")?.unwrap_or(120),
            ),
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
        if self.approved_models.is_empty() {
            return Err("OpenRouter approved_models must contain at least one model".to_string());
        }
        if self.approved_models.len() > MAX_APPROVED_OPENROUTER_MODELS {
            return Err(format!(
                "OpenRouter approved_models must contain at most {MAX_APPROVED_OPENROUTER_MODELS} models"
            ));
        }
        for model in &self.approved_models {
            if model.trim().is_empty() || !model.contains('/') {
                return Err(format!(
                    "approved OpenRouter model must be an author/model identifier: {model}"
                ));
            }
            if model.len() > 256 || model.chars().any(char::is_control) {
                return Err(
                    "approved OpenRouter models must be at most 256 printable characters"
                        .to_string(),
                );
            }
        }
        let unique_models = self
            .approved_models
            .iter()
            .collect::<std::collections::BTreeSet<_>>();
        if unique_models.len() != self.approved_models.len() {
            return Err("OpenRouter approved_models must not contain duplicates".to_string());
        }
        if !self.approved_models.contains(&self.model) {
            return Err("selected OpenRouter model must be in approved_models".to_string());
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
            (
                "preferred maximum latency in seconds",
                self.routing.preferred_max_latency_seconds,
            ),
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
#[allow(clippy::large_enum_variant)] // Keep the public provider-config shape source-compatible.
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

    /// Resolve this workspace's provider without changing global environment.
    pub fn from_workspace(root: &Path) -> Result<Self, String> {
        Self::from_workspace_lookup(root, &|name| std::env::var(name).ok())
    }

    fn from_workspace_lookup(
        root: &Path,
        environment: &dyn Fn(&str) -> Option<String>,
    ) -> Result<Self, String> {
        let settings = WorkspaceSettings::read(root)?;
        let project = if root.join("stasis.json").is_file() {
            ProjectAiConfig::from_workspace(root)?
        } else {
            ProjectAiConfig::default()
        };
        let lookup = |name: &str| settings.get(name, environment);
        let selected = setting_nonempty(&lookup, "STASIS_AI_PROVIDER");
        let default = if setting_nonempty(&lookup, "OPENROUTER_API_KEY").is_some() {
            "openrouter"
        } else {
            "codex"
        };
        let configured = project.provider.map(ProjectProvider::label);
        match selected.as_deref().or(configured).unwrap_or(default) {
            "codex" => Ok(Self::Codex),
            "openrouter" => Ok(Self::OpenRouter(OpenRouterConfig::from_lookup(
                &lookup,
                &project.openrouter,
            )?)),
            _ => Err("STASIS_AI_PROVIDER must be codex or openrouter".into()),
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

    /// Returns true only when this exact configured transport/model pairing is
    /// known to accept local image inputs in the repository's Codex flow.
    pub fn supports_image_input(&self) -> bool {
        match self {
            Self::Codex => codex_model_supports_image_input(&self.model()),
            Self::OpenRouter(_) => false,
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

pub(crate) fn codex_model_supports_image_input(model: &str) -> bool {
    // The Gauntlet visual and gameplay critics exercise image input with this
    // explicit model. Unknown aliases stay disabled so capture fails closed.
    matches!(model.trim(), "gpt-5.6-sol")
}

pub enum ConfiguredProvider {
    Codex(crate::CodexExecProvider),
    OpenRouter(OpenRouterProvider),
}

impl ConfiguredProvider {
    pub fn cached_image_input_capability(&self) -> ImageInputCapability {
        match self {
            Self::Codex(provider) => {
                let supported = codex_model_supports_image_input(&provider.model);
                ImageInputCapability {
                    model: provider.model.clone(),
                    supported,
                    reason: if supported {
                        "Codex model is verified for image input".to_string()
                    } else {
                        "Codex model is not verified for image input".to_string()
                    },
                }
            }
            Self::OpenRouter(provider) => provider.cached_image_input_capability().clone(),
        }
    }

    pub fn refresh_image_input_capability(
        &mut self,
        canceled: &AtomicBool,
    ) -> Result<ImageInputCapability, String> {
        match self {
            Self::Codex(_) => Ok(self.cached_image_input_capability()),
            Self::OpenRouter(provider) => provider.refresh_image_input_capability(canceled),
        }
    }

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
            Self::OpenRouter(provider) => {
                provider.config.model = model.clone();
                provider.image_capability = ImageInputCapability::unknown(&model);
            }
        }
        self
    }

    pub fn with_reasoning_effort(mut self, reasoning_effort: impl Into<String>) -> Self {
        let reasoning_effort = reasoning_effort.into();
        match &mut self {
            Self::Codex(provider) => provider.reasoning_effort = reasoning_effort,
            Self::OpenRouter(provider) => provider.reasoning_effort = Some(reasoning_effort),
        }
        self
    }

    pub fn with_session_id(mut self, session_id: impl Into<String>) -> Result<Self, String> {
        let session_id = session_id.into();
        if session_id.trim().is_empty()
            || session_id.len() > 256
            || session_id.chars().any(char::is_control)
        {
            return Err(
                "AI provider session_id must contain 1..=256 printable characters".to_string(),
            );
        }
        if let Self::OpenRouter(provider) = &mut self {
            provider.session_id = Some(session_id);
        }
        Ok(self)
    }

    pub fn with_images(mut self, images: Vec<std::path::PathBuf>) -> Result<Self, String> {
        match &mut self {
            Self::Codex(provider)
                if !images.is_empty() && !codex_model_supports_image_input(&provider.model) =>
            {
                return Err(format!(
                    "Codex model {} does not support image input",
                    provider.model
                ));
            }
            Self::Codex(provider) => provider.images = images,
            Self::OpenRouter(_) if !images.is_empty() => {
                return Err("OpenRouter transport does not support image attachments in this workspace flow".to_string());
            }
            Self::OpenRouter(_) => {}
        }
        Ok(self)
    }

    pub fn with_openrouter_image_inputs(
        mut self,
        images: Vec<OpenRouterImageInput>,
    ) -> Result<Self, String> {
        match &mut self {
            Self::OpenRouter(provider) => {
                if images.len() > MAX_OPENROUTER_IMAGES {
                    return Err(format!(
                        "at most {MAX_OPENROUTER_IMAGES} images may be sent"
                    ));
                }
                provider.images = images;
            }
            Self::Codex(_) if !images.is_empty() => {
                return Err("OpenRouter image inputs require the OpenRouter provider".to_string())
            }
            Self::Codex(_) => {}
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
    fn respond_with_progress(
        &mut self,
        request: &str,
        canceled: &AtomicBool,
        progress: &mut dyn FnMut(crate::ProviderProgress),
    ) -> Result<ModelResponse, String> {
        match self {
            Self::Codex(provider) => provider.respond_with_progress(request, canceled, progress),
            Self::OpenRouter(provider) => {
                provider.respond_with_progress(request, canceled, progress)
            }
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

    fn observe_tool_results(&mut self, observations: &[crate::ToolObservation]) {
        if let Self::OpenRouter(provider) = self {
            provider.observe_tool_results(observations);
        }
    }
}

pub struct OpenRouterProvider {
    config: OpenRouterConfig,
    client: Client,
    last_usage: Option<Value>,
    call_count: u32,
    images: Vec<OpenRouterImageInput>,
    image_capability: ImageInputCapability,
    reasoning_effort: Option<String>,
    session_id: Option<String>,
}

impl OpenRouterProvider {
    pub fn new(config: OpenRouterConfig) -> Result<Self, String> {
        config.validate()?;
        let client = Client::builder()
            .connect_timeout(config.timeout.min(Duration::from_secs(30)))
            .build()
            .map_err(|error| format!("failed configuring OpenRouter HTTPS client: {error}"))?;
        let image_capability = ImageInputCapability::unknown(&config.model);
        Ok(Self {
            config,
            client,
            last_usage: None,
            call_count: 0,
            reasoning_effort: None,
            session_id: None,
            images: Vec::new(),
            image_capability,
        })
    }

    pub fn cached_image_input_capability(&self) -> &ImageInputCapability {
        &self.image_capability
    }

    pub fn with_image_inputs(mut self, images: Vec<OpenRouterImageInput>) -> Result<Self, String> {
        if images.len() > MAX_OPENROUTER_IMAGES {
            return Err(format!(
                "at most {MAX_OPENROUTER_IMAGES} images may be sent"
            ));
        }
        self.images = images;
        Ok(self)
    }

    pub fn refresh_image_input_capability(
        &mut self,
        canceled: &AtomicBool,
    ) -> Result<ImageInputCapability, String> {
        let deadline = Instant::now() + self.config.timeout;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed configuring OpenRouter async runtime: {error}"))?;
        let capability = runtime.block_on(self.image_input_capability_async(deadline, canceled))?;
        self.image_capability = capability.clone();
        Ok(capability)
    }

    async fn image_input_capability_async(
        &self,
        deadline: Instant,
        canceled: &AtomicBool,
    ) -> Result<ImageInputCapability, String> {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let response = await_cancelable(
            self.client
                .get(format!(
                    "{}/models",
                    self.config.base_url.trim_end_matches('/')
                ))
                .bearer_auth(&self.config.api_key)
                .timeout(remaining_timeout(
                    deadline,
                    "OpenRouter model capability lookup",
                )?)
                .send(),
            canceled,
            deadline,
            "OpenRouter model capability lookup",
        )
        .await?;
        let status = response.status();
        let value: Value = await_cancelable(
            response.json(),
            canceled,
            deadline,
            "OpenRouter model capability response",
        )
        .await?;
        if !status.is_success() {
            return Err(api_error(
                "OpenRouter model capability lookup",
                status.as_u16(),
                &value,
                &self.config.api_key,
            ));
        }
        let models = value
            .get("data")
            .and_then(Value::as_array)
            .ok_or_else(|| "OpenRouter model metadata omitted the model list".to_string())?;
        let Some(model) = models
            .iter()
            .find(|model| model.get("id").and_then(Value::as_str) == Some(&self.config.model))
        else {
            return Ok(ImageInputCapability {
                model: self.config.model.clone(),
                supported: false,
                reason: "configured model was absent from OpenRouter model metadata".to_string(),
            });
        };
        let modalities = model
            .pointer("/architecture/input_modalities")
            .or_else(|| model.get("input_modalities"))
            .and_then(Value::as_array);
        let supported = modalities
            .is_some_and(|values| values.iter().any(|value| value.as_str() == Some("image")));
        Ok(ImageInputCapability {
            model: self.config.model.clone(),
            supported,
            reason: if supported {
                "OpenRouter reports image input for the configured model".to_string()
            } else {
                "OpenRouter does not report image input for the configured model".to_string()
            },
        })
    }

    fn route_json(&self) -> Value {
        let routing = &self.config.routing;
        let mut value = json!({
            "allow_fallbacks": routing.allow_fallbacks,
            "sort": {
                "by": match routing.sort {
                    RoutingSort::Price => "price",
                    RoutingSort::Throughput => "throughput",
                    RoutingSort::Latency => "latency",
                },
                "partition": "none"
            },
            "require_parameters": true,
        });
        let object = value.as_object_mut().expect("route object");
        let only = routing
            .only
            .clone()
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
        if let Some(target) = routing
            .preferred_min_throughput
            .or(routing.hard_min_throughput)
        {
            object.insert(
                "preferred_min_throughput".to_string(),
                json!({"p50": target}),
            );
        }
        if let Some(target) = routing.preferred_max_latency_seconds {
            object.insert("preferred_max_latency".to_string(), json!({"p50": target}));
        }
        if let Some(max_price) = routing.max_price {
            object.insert("max_price".to_string(), json!({"completion": max_price}));
        }
        value
    }

    fn observe_tool_results(&mut self, observations: &[crate::ToolObservation]) {
        let rejected = observations
            .iter()
            .any(|observation| observation.error.is_some());
        if rejected
            && self
                .reasoning_effort
                .as_deref()
                .is_some_and(|effort| matches!(effort, "minimal" | "low"))
        {
            self.reasoning_effort = Some("medium".to_string());
        }
    }
}

impl ModelProvider for OpenRouterProvider {
    fn respond(&mut self, request: &str, canceled: &AtomicBool) -> Result<ModelResponse, String> {
        self.respond_with_progress(request, canceled, &mut |_| {})
    }

    fn respond_with_progress(
        &mut self,
        request: &str,
        canceled: &AtomicBool,
        progress: &mut dyn FnMut(crate::ProviderProgress),
    ) -> Result<ModelResponse, String> {
        let turn_started = Instant::now();
        let deadline = turn_started + self.config.timeout;
        self.call_count = self.call_count.saturating_add(1);
        self.last_usage = None;
        self.config.validate()?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|error| format!("failed configuring OpenRouter async runtime: {error}"))?;
        let images = std::mem::take(&mut self.images);
        runtime.block_on(self.respond_async(
            request,
            &images,
            canceled,
            turn_started,
            deadline,
            progress,
        ))
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
        images: &[OpenRouterImageInput],
        canceled: &AtomicBool,
        turn_started: Instant,
        deadline: Instant,
        progress: &mut dyn FnMut(crate::ProviderProgress),
    ) -> Result<ModelResponse, String> {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        progress(crate::ProviderProgress::ContactingProvider);
        if !images.is_empty() {
            let capability = self
                .image_input_capability_async(deadline, canceled)
                .await?;
            self.image_capability = capability.clone();
            if !capability.supported {
                return Err(capability.reason);
            }
        }

        self.send(request, images, turn_started, deadline, canceled, progress)
            .await
    }

    async fn send(
        &mut self,
        request: &str,
        images: &[OpenRouterImageInput],
        turn_started: Instant,
        deadline: Instant,
        canceled: &AtomicBool,
        progress: &mut dyn FnMut(crate::ProviderProgress),
    ) -> Result<ModelResponse, String> {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let route = self.route_json();
        let mut schema = model_response_schema_for_request(request)?;
        // Keep Gemini structured-output state bounded; local admission enforces array limits.
        strip_array_max_items(&mut schema);
        let content =
            if images.is_empty() {
                Value::String(request.to_string())
            } else {
                let mut content = vec![json!({"type": "text", "text": request})];
                content.extend(images.iter().map(
                    |image| json!({"type": "image_url", "image_url": {"url": image.data_url()}}),
                ));
                Value::Array(content)
            };
        let mut body = json!({
            "models": self.config.approved_models,
            "messages": [{"role": "user", "content": content}],
            "stream": true,
            "stream_options": {"include_usage": true},
            "response_format": {"type": "json_schema", "json_schema": {"name": "stasis_model_response", "strict": true, "schema": schema}},
            "provider": route,
        });
        let body_object = body.as_object_mut().expect("OpenRouter request body");
        if let Some(reasoning_effort) = self.reasoning_effort.as_deref() {
            body_object.insert("reasoning".to_string(), json!({"effort": reasoning_effort}));
        }
        if let Some(session_id) = self.session_id.as_deref() {
            body_object.insert("session_id".to_string(), json!(session_id));
        }
        let request_started = Instant::now();
        let mut transport_attempts = 0_u32;
        let mut response = loop {
            transport_attempts = transport_attempts.saturating_add(1);
            let timeout = remaining_timeout(deadline, "OpenRouter chat request")?;
            let response = await_cancelable(
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
            if response.status() != StatusCode::TOO_MANY_REQUESTS
                || transport_attempts > MAX_OPENROUTER_RATE_LIMIT_RETRIES
            {
                break response;
            }
            let delay = openrouter_retry_delay(
                response
                    .headers()
                    .get(RETRY_AFTER)
                    .and_then(|value| value.to_str().ok()),
                transport_attempts,
            );
            sleep_cancelable(delay, canceled, deadline, "OpenRouter rate-limit retry").await?;
            progress(crate::ProviderProgress::ContactingProvider);
        };
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
            let context = if status == StatusCode::TOO_MANY_REQUESTS && transport_attempts > 1 {
                "OpenRouter request after bounded retries"
            } else {
                "OpenRouter request"
            };
            return Err(api_error(
                context,
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
        let mut finish_reason = None;
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
                finish_reason = chunk
                    .pointer("/choices/0/finish_reason")
                    .and_then(Value::as_str)
                    .map(sanitize_label)
                    .or(finish_reason);
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
                        let elapsed_ms = duration_ms(request_started.elapsed());
                        first_content_ms = Some(elapsed_ms);
                        progress(crate::ProviderProgress::FirstResponse { elapsed_ms });
                    }
                    content.push_str(value);
                    if first_action_ms.is_none() && has_started_tool_call(&content) {
                        let elapsed_ms = duration_ms(request_started.elapsed());
                        first_action_ms = Some(elapsed_ms);
                        progress(crate::ProviderProgress::FirstAction { elapsed_ms });
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
        let parsed = decode_model_response(&content, "OpenRouter").map_err(|error| {
            format!(
                "{error} (finish_reason={}, response_bytes={})",
                finish_reason.as_deref().unwrap_or("unknown"),
                content.len()
            )
        });
        let resolved_model = resolved_model
            .as_deref()
            .map(sanitize_label)
            .unwrap_or_else(|| sanitize_label(&self.config.model));
        let resolved_provider_evidence = resolved_provider
            .as_deref()
            .and_then(normalize_provider_slug);
        let fallback = resolved_provider_evidence.as_ref().is_some_and(|resolved| {
            self.config
                .routing
                .order
                .first()
                .and_then(|value| normalize_provider_slug(value))
                .is_some_and(|first| first != *resolved)
        });
        let resolved_provider = resolved_provider_evidence.unwrap_or_else(|| "unknown".to_string());
        if fallback {
            progress(crate::ProviderProgress::Fallback);
        }
        let prompt_tokens = metric_number(usage.get("prompt_tokens"));
        let completion_tokens = metric_number(usage.get("completion_tokens"));
        let reasoning_tokens =
            metric_number(usage.pointer("/completion_tokens_details/reasoning_tokens"));
        let cache_tokens = metric_number(usage.pointer("/prompt_tokens_details/cached_tokens"));
        self.last_usage = Some(json!({
            "configured_provider": "openrouter", "configured_model": self.config.model,
            "approved_models": self.config.approved_models,
            "resolved_provider": resolved_provider, "resolved_model": resolved_model,
            "route": route, "fallback": fallback,
            "timing_ms": {"metadata": 0, "headers": duration_ms(header_time), "first_reasoning": first_reasoning_ms, "first_content": first_content_ms, "first_action": first_action_ms, "inference_total": duration_ms(request_started.elapsed()), "turn_total": duration_ms(turn_started.elapsed())},
            "transport_attempts": transport_attempts,
            "tokens": {"prompt": prompt_tokens, "completion": completion_tokens, "reasoning": reasoning_tokens, "cache": cache_tokens},
            "cost": metric_number(usage.get("cost")),
            "throughput_tokens_per_second": throughput(&usage, request_started.elapsed()),
            "validation": {"structured_schema": if parsed.is_ok() { "accepted" } else { "rejected" }, "repair_count": 0}
        }));
        parsed
    }
}

fn strip_array_max_items(value: &mut Value) {
    match value {
        Value::Object(object) => {
            object.remove("maxItems");
            for child in object.values_mut() {
                strip_array_max_items(child);
            }
        }
        Value::Array(array) => {
            for child in array {
                strip_array_max_items(child);
            }
        }
        _ => {}
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

fn openrouter_retry_delay(retry_after: Option<&str>, retry_number: u32) -> Duration {
    let server_delay = retry_after
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map(Duration::from_secs);
    server_delay
        .unwrap_or_else(|| {
            DEFAULT_OPENROUTER_RETRY_DELAY
                .saturating_mul(1_u32 << retry_number.saturating_sub(1).min(3))
        })
        .min(MAX_OPENROUTER_RETRY_DELAY)
}

async fn sleep_cancelable(
    delay: Duration,
    canceled: &AtomicBool,
    deadline: Instant,
    context: &str,
) -> Result<(), String> {
    let wake = Instant::now() + delay;
    loop {
        if canceled.load(Ordering::Acquire) {
            return Err("AI request canceled".to_string());
        }
        let remaining = remaining_timeout(deadline, context)?;
        let until_wake = wake.saturating_duration_since(Instant::now());
        if until_wake.is_zero() {
            return Ok(());
        }
        tokio::time::sleep(remaining.min(until_wake).min(Duration::from_millis(10))).await;
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
    (normalized.len() <= 256
        && normalized.split('/').all(|part| {
            !part.is_empty()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        }))
    .then_some(normalized)
}
fn env_nonempty(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .filter(|value| !value.trim().is_empty())
}
fn setting_nonempty(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Option<String> {
    lookup(name).filter(|value| !value.trim().is_empty())
}

struct WorkspaceSettings(BTreeMap<String, String>);

impl WorkspaceSettings {
    fn read(root: &Path) -> Result<Self, String> {
        const MAX_BYTES: u64 = 64 * 1024;
        let file = match std::fs::File::open(root.join(".env")) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self(BTreeMap::new()));
            }
            Err(_) => return Err("Cannot read workspace .env settings".into()),
        };
        let mut bytes = Vec::new();
        file.take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| "Cannot read workspace .env settings".to_string())?;
        if bytes.len() > MAX_BYTES as usize {
            return Err("Workspace .env exceeds 64 KiB".into());
        }
        // Windows editors commonly write a UTF-8 byte-order mark.
        let source = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]).unwrap_or(&bytes);
        let mut settings = BTreeMap::new();
        for item in dotenvy::from_read_iter(source) {
            let (key, value) = item.map_err(|_| "Invalid workspace .env syntax".to_string())?;
            settings.entry(key).or_insert(value);
        }
        Ok(Self(settings))
    }

    fn get(&self, name: &str, environment: &dyn Fn(&str) -> Option<String>) -> Option<String> {
        environment(name).or_else(|| self.0.get(name).cloned())
    }
}

fn setting_list(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Vec<String> {
    setting_nonempty(lookup, name)
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
fn setting_bool(
    lookup: &dyn Fn(&str) -> Option<String>,
    name: &str,
    default: bool,
) -> Result<bool, String> {
    match setting_nonempty(lookup, name).as_deref() {
        None => Ok(default),
        Some("1" | "true") => Ok(true),
        Some("0" | "false") => Ok(false),
        Some(_) => Err(format!("{name} must be true or false")),
    }
}
fn setting_f64(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Result<Option<f64>, String> {
    setting_nonempty(lookup, name)
        .map(|value| {
            value
                .parse::<f64>()
                .map_err(|_| format!("{name} must be a number"))
        })
        .transpose()
}
fn setting_u64(lookup: &dyn Fn(&str) -> Option<String>, name: &str) -> Result<Option<u64>, String> {
    setting_nonempty(lookup, name)
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
fn has_started_tool_call(content: &str) -> bool {
    let bytes = content.as_bytes();
    let mut index = 0;
    let mut depth = 0_u32;
    while index < bytes.len() {
        match bytes[index] {
            b'{' | b'[' => {
                depth = depth.saturating_add(1);
                index += 1;
            }
            b'}' | b']' => {
                depth = depth.saturating_sub(1);
                index += 1;
            }
            b'"' => {
                let start = index + 1;
                index = start;
                let mut escaped = false;
                while index < bytes.len() {
                    match bytes[index] {
                        b'\\' => {
                            escaped = true;
                            index = index.saturating_add(2);
                        }
                        b'"' => break,
                        _ => index += 1,
                    }
                }
                if index >= bytes.len() {
                    return false;
                }
                let end = index;
                index += 1;
                if depth != 1 || escaped || &bytes[start..end] != b"tool_calls" {
                    continue;
                }
                while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                    index += 1;
                }
                if bytes.get(index) != Some(&b':') {
                    continue;
                }
                index += 1;
                // A required but empty tool_calls array is not an action.
                for delimiter in [b'[', b'{'] {
                    while index < bytes.len() && bytes[index].is_ascii_whitespace() {
                        index += 1;
                    }
                    if bytes.get(index) != Some(&delimiter) {
                        return false;
                    }
                    index += 1;
                }
                return true;
            }
            _ => index += 1,
        }
    }
    false
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
    let primary = value
        .pointer("/error/message")
        .or_else(|| value.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("request rejected");
    let nested = value
        .pointer("/error/metadata/raw")
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|raw| {
            raw.pointer("/error/message")
                .or_else(|| raw.get("message"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });
    let message = nested.as_deref().unwrap_or(primary);
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
    struct WorkspaceFixture(std::path::PathBuf);

    impl WorkspaceFixture {
        fn new(source: &str) -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "stasis-provider-settings-{}-{}-{}",
                std::process::id(),
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            std::fs::write(path.join(".env"), source).unwrap();
            std::fs::write(path.join("stasis.json"), r#"{"manifest_version":1}"#).unwrap();
            Self(path)
        }
    }

    impl Drop for WorkspaceFixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn workspace_ai_policy_defaults_to_approved_model_and_performance_gates() {
        let fixture = WorkspaceFixture::new("OPENROUTER_API_KEY=test-key\n");
        assert!(
            !ProjectAiConfig::from_workspace(&fixture.0)
                .unwrap()
                .editor
                .auto_persist_html_transcripts
        );
        let ProviderConfig::OpenRouter(config) =
            ProviderConfig::from_workspace_lookup(&fixture.0, &|_| None).unwrap()
        else {
            panic!("expected OpenRouter");
        };
        assert_eq!(config.routing.preferred_min_throughput, Some(400.0));
        assert_eq!(config.routing.hard_min_throughput, None);
        assert_eq!(config.routing.preferred_max_latency_seconds, Some(2.0));
        assert_eq!(config.approved_models.as_ref(), [DEFAULT_OPENROUTER_MODEL]);
    }

    #[test]
    fn workspace_manifest_can_choose_openrouter_as_the_project_default() {
        let fixture = WorkspaceFixture::new("OPENROUTER_API_KEY=test-key\n");
        std::fs::write(
            fixture.0.join("stasis.json"),
            r#"{"manifest_version":1,"ai":{"provider":"openrouter"}}"#,
        )
        .unwrap();
        assert!(matches!(
            ProviderConfig::from_workspace_lookup(&fixture.0, &|_| None).unwrap(),
            ProviderConfig::OpenRouter(_)
        ));
    }

    #[test]
    fn workspace_manifest_owns_approved_models_and_performance_constraints() {
        let fixture = WorkspaceFixture::new("\u{feff}# secrets and transport only\nexport OPENROUTER_API_KEY='test-key'\nSTASIS_AI_MODEL=ignored/model\nSTASIS_AI_HARD_MIN_THROUGHPUT=9999\nSTASIS_AI_ROUTE_ONLY=cerebras,groq\nSTASIS_AI_ROUTE_ORDER=groq,cerebras\nSTASIS_AI_ALLOW_FALLBACKS=false\nSTASIS_AI_ROUTE_SORT=throughput\nSTASIS_AI_MAX_PRICE=2\nSTASIS_AI_TIMEOUT_SECONDS=45\n");
        std::fs::write(
            fixture.0.join("stasis.json"),
            r#"{"manifest_version":1,"ai":{"openrouter":{"approved_models":["openai/gpt-oss-120b","future/approved"],"min_throughput_tokens_per_second":450,"max_p50_latency_seconds":1.75},"editor":{"auto_persist_html_transcripts":true}}}"#,
        )
        .unwrap();
        let ProviderConfig::OpenRouter(config) =
            ProviderConfig::from_workspace_lookup(&fixture.0, &|_| None).unwrap()
        else {
            panic!("expected OpenRouter");
        };
        assert_eq!(config.api_key, "test-key");
        assert_eq!(config.model, "openai/gpt-oss-120b");
        assert_eq!(
            config.approved_models.as_ref(),
            ["openai/gpt-oss-120b", "future/approved"]
        );
        assert_eq!(config.routing.preferred_min_throughput, Some(450.0));
        assert_eq!(config.routing.hard_min_throughput, None);
        assert_eq!(config.routing.preferred_max_latency_seconds, Some(1.75));
        assert_eq!(config.routing.only, ["cerebras", "groq"]);
        assert!(config.routing.order.is_empty());
        assert!(matches!(config.routing.sort, RoutingSort::Price));
        assert!(!config.routing.allow_fallbacks);
        assert_eq!(config.routing.max_price, Some(2.0));
        assert_eq!(config.timeout, Duration::from_secs(45));
        assert!(
            ProjectAiConfig::from_workspace(&fixture.0)
                .unwrap()
                .editor
                .auto_persist_html_transcripts
        );
    }

    #[test]
    fn approved_model_list_is_bounded() {
        let mut project = ProjectOpenRouterConfig {
            approved_models: (0..=MAX_APPROVED_OPENROUTER_MODELS)
                .map(|index| format!("approved/model-{index}"))
                .collect(),
            ..ProjectOpenRouterConfig::default()
        };
        assert_eq!(
            project.validate().unwrap_err(),
            "ai.openrouter.approved_models must contain at most 8 models"
        );
        project.approved_models.pop();
        project.validate().unwrap();
    }

    #[test]
    fn workspace_environment_overrides_file_and_explicit_codex_wins() {
        let fixture = WorkspaceFixture::new("OPENROUTER_API_KEY=file-key\nSTASIS_AI_PROVIDER=openrouter\nSTASIS_AI_MODEL=openai/gpt-oss-120b\n");
        assert!(matches!(
            ProviderConfig::from_workspace_lookup(&fixture.0, &|name| (name
                == "STASIS_AI_PROVIDER")
                .then(|| "codex".into()))
            .unwrap(),
            ProviderConfig::Codex
        ));
        let ProviderConfig::OpenRouter(config) =
            ProviderConfig::from_workspace_lookup(&fixture.0, &|name| match name {
                "OPENROUTER_API_KEY" => Some("environment-key".into()),
                "STASIS_AI_MODEL" => Some("other/model".into()),
                _ => None,
            })
            .unwrap()
        else {
            panic!("expected OpenRouter");
        };
        assert_eq!(config.api_key, "environment-key");
        assert_eq!(config.model, DEFAULT_OPENROUTER_MODEL);
        let fixture =
            WorkspaceFixture::new("STASIS_AI_PROVIDER=codex\nOPENROUTER_API_KEY=file-key\n");
        assert!(matches!(
            ProviderConfig::from_workspace_lookup(&fixture.0, &|_| None).unwrap(),
            ProviderConfig::Codex
        ));
    }

    #[test]
    fn workspace_settings_do_not_search_parents_or_expose_parse_secrets() {
        let fixture = WorkspaceFixture::new("OPENROUTER_API_KEY=parent-key\n");
        let child = fixture.0.join("child");
        std::fs::create_dir(&child).unwrap();
        assert!(matches!(
            ProviderConfig::from_workspace_lookup(&child, &|_| None).unwrap(),
            ProviderConfig::Codex
        ));
        std::fs::write(
            child.join(".env"),
            "OPENROUTER_API_KEY=\"private-credential\n",
        )
        .unwrap();
        let error = ProviderConfig::from_workspace_lookup(&child, &|_| None)
            .err()
            .unwrap();
        assert_eq!(error, "Invalid workspace .env syntax");
        assert!(!error.contains("private-credential"));
    }
    use super::*;
    #[test]
    fn provider_error_surfaces_bounded_nested_message() {
        let error = api_error(
            "OpenRouter request",
            400,
            &json!({"error":{"message":"Provider returned error","metadata":{"raw":"{\"error\":{\"message\":\"response schema unsupported\"}}"}}}),
            "unit-secret",
        );
        assert_eq!(
            error,
            "OpenRouter request failed with HTTP 400: response schema unsupported"
        );
    }

    #[test]
    fn openrouter_schema_removes_provider_state_exploding_array_maxima() {
        let mut schema = json!({
            "type": "array",
            "maxItems": 50,
            "items": {"type": "object", "properties": {"edits": {
                "type": "array", "maxItems": 64, "items": {"type": "string"}
            }}}
        });
        strip_array_max_items(&mut schema);
        assert!(schema.pointer("/maxItems").is_none());
        assert!(schema.pointer("/items/properties/edits/maxItems").is_none());
        assert_eq!(
            schema.pointer("/items/properties/edits/items/type"),
            Some(&json!("string"))
        );
    }
    static ENVIRONMENT_TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct EnvironmentRestore(Vec<(&'static str, Option<std::ffi::OsString>)>);

    impl Drop for EnvironmentRestore {
        fn drop(&mut self) {
            for (name, value) in self.0.drain(..) {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    #[test]
    fn openrouter_environment_configuration_is_accepted_without_serializing_key() {
        let _lock = ENVIRONMENT_TEST_LOCK.lock().unwrap();
        let names = [
            "STASIS_AI_PROVIDER",
            "OPENROUTER_API_KEY",
            "STASIS_AI_MODEL",
            "STASIS_AI_ROUTE_ONLY",
            "STASIS_AI_ALLOW_FALLBACKS",
            "STASIS_AI_ROUTE_SORT",
            "STASIS_AI_TIMEOUT_SECONDS",
        ];
        let _restore = EnvironmentRestore(
            names
                .into_iter()
                .map(|name| (name, std::env::var_os(name)))
                .collect(),
        );
        let secret = "environment-test-key-not-a-real-credential";
        for (name, value) in [
            ("STASIS_AI_PROVIDER", "openrouter"),
            ("OPENROUTER_API_KEY", secret),
            ("STASIS_AI_MODEL", "ignored/model"),
            ("STASIS_AI_ROUTE_ONLY", "cerebras,openai"),
            ("STASIS_AI_ALLOW_FALLBACKS", "false"),
            ("STASIS_AI_ROUTE_SORT", "latency"),
            ("STASIS_AI_TIMEOUT_SECONDS", "45"),
        ] {
            std::env::set_var(name, value);
        }

        let ProviderConfig::OpenRouter(config) = ProviderConfig::from_env().unwrap() else {
            panic!("OpenRouter environment selected a different provider");
        };
        assert_eq!(config.api_key, secret);
        assert_eq!(config.model, "openai/gpt-oss-120b");
        assert_eq!(config.routing.only, ["cerebras", "openai"]);
        assert!(!config.routing.allow_fallbacks);
        assert!(matches!(config.routing.sort, RoutingSort::Price));
        assert_eq!(config.timeout, Duration::from_secs(45));
        let route = OpenRouterProvider::new(config).unwrap().route_json();
        assert!(!route.to_string().contains(secret));
    }

    #[test]
    fn action_progress_requires_a_nonempty_top_level_tool_calls_array() {
        for content in [
            r#"{"mode":"tool_calls"}"#,
            r#"{"working_notes":"say \"tool_calls\": [] and {ignored}"}"#,
            r#"{"nested":{"tool_calls":[{}]}}"#,
            r#"{"mode":"done","tool_calls":[]}"#,
            r#"{"tool_calls": [  ]}"#,
            r#"{"tool_calls": null}"#,
        ] {
            assert!(!has_started_tool_call(content), "{content}");
        }
        let partial = r#"{"working_notes":"ok","tool_calls" : [  {"#;
        for end in 0..partial.len() {
            assert!(!has_started_tool_call(&partial[..end]));
        }
        assert!(has_started_tool_call(partial));
    }

    #[test]
    fn streamed_done_reply_never_reports_first_action() {
        let fragments = [
            r#"{"mode":"done","working_notes":"","summary":"Done","tool_calls" :"#,
            " [ ",
            "]}",
        ];
        let mut body = String::new();
        for content in fragments {
            let chunk = json!({"choices":[{"delta":{"content":content}}]});
            body.push_str(&format!("data: {chunk}\n\n"));
        }
        body.push_str("data: [DONE]\n\n");
        let (base_url, _requests, worker) =
            mock_server(vec![http_response("text/event-stream", &body)]);
        let mut provider = OpenRouterProvider::new(test_config(base_url)).unwrap();
        let mut progress = Vec::new();
        let response = provider
            .respond_with_progress(&test_request(), &AtomicBool::new(false), &mut |event| {
                progress.push(event)
            })
            .unwrap();
        assert!(matches!(response, ModelResponse::Done { .. }));
        assert!(!progress
            .iter()
            .any(|event| matches!(event, crate::ProviderProgress::FirstAction { .. })));
        assert!(provider.take_usage().unwrap()["timing_ms"]["first_action"].is_null());
        worker.join().unwrap();
    }

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
            preferred_max_latency_seconds: None,
            max_price: Some(0.8),
        };
        let config = OpenRouterConfig {
            api_key: "secret".into(),
            base_url: DEFAULT_OPENROUTER_URL.into(),
            model: DEFAULT_OPENROUTER_MODEL.into(),
            approved_models: default_approved_openrouter_models().into_boxed_slice(),
            routing,
            timeout: Duration::from_secs(2),
        };
        let provider = OpenRouterProvider::new(config).expect("provider");
        assert_eq!(
            provider.route_json(),
            json!({"only":["cerebras"], "order":["cerebras","openai"], "allow_fallbacks":false, "sort":{"by":"price","partition":"none"}, "require_parameters":true, "preferred_min_throughput":{"p50":1500.0}, "max_price":{"completion":0.8}})
        );
    }

    #[test]
    fn throughput_routing_and_rejection_escalation_are_turn_aware() {
        let mut provider = OpenRouterProvider::new(OpenRouterConfig {
            api_key: "secret".into(),
            base_url: DEFAULT_OPENROUTER_URL.into(),
            model: DEFAULT_OPENROUTER_MODEL.into(),
            approved_models: default_approved_openrouter_models().into_boxed_slice(),
            routing: RoutingConfig::default(),
            timeout: Duration::from_secs(2),
        })
        .expect("provider");
        provider.reasoning_effort = Some("low".to_string());
        assert_eq!(
            provider.route_json()["sort"],
            json!({"by":"throughput", "partition":"none"})
        );

        provider.observe_tool_results(&[crate::ToolObservation::error(
            "write_symbol",
            "compile rejected",
        )]);
        assert_eq!(provider.reasoning_effort.as_deref(), Some("medium"));
    }

    #[test]
    fn hard_and_preferred_thresholds_cannot_be_mixed() {
        let config = OpenRouterConfig {
            api_key: "secret".into(),
            base_url: DEFAULT_OPENROUTER_URL.into(),
            model: DEFAULT_OPENROUTER_MODEL.into(),
            approved_models: default_approved_openrouter_models().into_boxed_slice(),
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

    fn http_error_response(status: &str, retry_after: Option<&str>, body: &str) -> String {
        let retry_after = retry_after
            .map(|value| format!("Retry-After: {value}\r\n"))
            .unwrap_or_default();
        format!(
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\n{retry_after}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    fn test_config(base_url: String) -> OpenRouterConfig {
        OpenRouterConfig {
            api_key: "unit-secret".into(),
            base_url,
            model: DEFAULT_OPENROUTER_MODEL.into(),
            approved_models: default_approved_openrouter_models().into_boxed_slice(),
            routing: RoutingConfig::default(),
            timeout: Duration::from_secs(2),
        }
    }

    fn test_request() -> String {
        json!({"tool_specs": crate::workshop_tool_specs()}).to_string()
    }

    fn test_png() -> OpenRouterImageInput {
        let source = image::RgbaImage::from_pixel(1, 1, image::Rgba([17, 34, 51, 255]));
        let mut bytes = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(source)
            .write_to(&mut bytes, image::ImageFormat::Png)
            .expect("encode test PNG");
        let bytes = bytes.into_inner();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        OpenRouterImageInput::new("image/png", bytes, &hash).expect("test image")
    }

    fn done_stream(summary: &str) -> String {
        let fixture = json!({
            "mode":"done", "working_notes":"Inspected the selected image.", "summary":summary
        });
        let chunk = json!({
            "model":DEFAULT_OPENROUTER_MODEL,"provider":"openai",
            "choices":[{"delta":{"content":fixture.to_string()}}]
        });
        format!("data: {chunk}\n\ndata: [DONE]\n\n")
    }

    #[test]
    fn rate_limit_retries_preserve_native_route_request_and_then_succeed() {
        let limited = json!({"error":{"message":"temporarily rate limited"}}).to_string();
        let stream = done_stream("completed after retry");
        let (base_url, requests, worker) = mock_server(vec![
            http_error_response("429 Too Many Requests", Some("0"), &limited),
            http_error_response("429 Too Many Requests", Some("0"), &limited),
            http_response("text/event-stream", &stream),
        ]);
        let mut config = test_config(base_url);
        config.approved_models = vec![
            "openai/gpt-oss-120b".to_string(),
            "qwen/qwen3-coder".to_string(),
        ]
        .into_boxed_slice();
        config.routing.preferred_min_throughput = Some(400.0);
        config.routing.preferred_max_latency_seconds = Some(2.0);
        config.routing.sort = RoutingSort::Price;
        let mut provider = OpenRouterProvider::new(config).expect("provider");
        provider.session_id = Some("stasis-task-sticky".to_string());
        let reply = provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect("bounded retry succeeds");
        assert!(matches!(
            reply,
            ModelResponse::Done { summary, .. } if summary == "completed after retry"
        ));
        for _ in 0..3 {
            let request = requests.recv().expect("chat request");
            let json_start = request.find("\r\n\r\n").expect("headers") + 4;
            let body: Value = serde_json::from_str(&request[json_start..]).expect("body");
            assert_eq!(
                body.get("models"),
                Some(&json!(["openai/gpt-oss-120b", "qwen/qwen3-coder"]))
            );
            assert!(body.get("model").is_none());
            assert_eq!(
                body.pointer("/provider/sort"),
                Some(&json!({"by":"price", "partition":"none"}))
            );
            assert_eq!(
                body.pointer("/provider/preferred_min_throughput/p50"),
                Some(&json!(400.0))
            );
            assert_eq!(
                body.pointer("/provider/preferred_max_latency/p50"),
                Some(&json!(2.0))
            );
            assert_eq!(body.get("session_id"), Some(&json!("stasis-task-sticky")));
        }
        assert!(requests.try_recv().is_err());
        assert_eq!(provider.take_usage().unwrap()["transport_attempts"], 3);
        worker.join().expect("server");
    }

    #[test]
    fn persistent_rate_limit_stops_after_bounded_retries() {
        let limited = json!({"error":{"message":"temporarily rate limited"}}).to_string();
        let (base_url, requests, worker) = mock_server(vec![
            http_error_response("429 Too Many Requests", Some("0"), &limited),
            http_error_response("429 Too Many Requests", Some("0"), &limited),
            http_error_response("429 Too Many Requests", Some("0"), &limited),
        ]);
        let mut provider = OpenRouterProvider::new(test_config(base_url)).expect("provider");
        let error = provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect_err("persistent rate limit");
        assert!(error.contains("HTTP 429"));
        assert!(error.contains("after bounded retries"));
        for _ in 0..3 {
            assert!(requests.recv().expect("chat request").starts_with("POST "));
        }
        worker.join().expect("server");
    }

    #[test]
    fn immutable_image_input_enforces_mime_bound_and_hash() {
        let bytes = b"\x89PNG\r\n\x1a\nselected-pixels".to_vec();
        let hash = format!("{:x}", Sha256::digest(&bytes));
        let image = OpenRouterImageInput::new("image/png", bytes.clone(), &hash)
            .expect("valid PNG snapshot");
        assert_eq!(image.bytes(), bytes);
        assert_eq!(image.sha256(), hash);
        assert!(
            OpenRouterImageInput::new("image/jpeg", bytes.clone(), &hash)
                .unwrap_err()
                .contains("JPEG signature")
        );
        assert!(
            OpenRouterImageInput::new("image/png", bytes, &"0".repeat(64))
                .unwrap_err()
                .contains("changed after selection")
        );
        let oversized = vec![0; MAX_OPENROUTER_IMAGE_BYTES + 1];
        assert!(
            OpenRouterImageInput::new("image/png", oversized, &"0".repeat(64))
                .unwrap_err()
                .contains("between 1")
        );
    }

    #[test]
    fn metadata_capability_gates_and_sends_exact_selected_bytes() {
        let metadata = json!({"data":[{
            "id":DEFAULT_OPENROUTER_MODEL,
            "architecture":{"input_modalities":["text","image"]}
        }]})
        .to_string();
        let stream = done_stream("selected pixels received");
        let (base_url, requests, worker) = mock_server(vec![
            http_response("application/json", &metadata),
            http_response("text/event-stream", &stream),
            http_response("text/event-stream", &stream),
        ]);
        let image = test_png();
        let expected_url = image.data_url();
        let mut provider = OpenRouterProvider::new(test_config(base_url))
            .expect("provider")
            .with_image_inputs(vec![image])
            .expect("image inputs");
        provider.reasoning_effort = Some("low".to_string());
        provider.session_id = Some("stasis-desktop-task-image".to_string());
        provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect("image response");
        let metadata_request = requests.recv().expect("metadata request");
        assert!(metadata_request.starts_with("GET /models "));
        let chat_request = requests.recv().expect("chat request");
        let json_start = chat_request.find("\r\n\r\n").expect("headers") + 4;
        let body: Value = serde_json::from_str(&chat_request[json_start..]).expect("body");
        assert_eq!(body.pointer("/reasoning/effort"), Some(&json!("low")));
        assert_eq!(
            body.get("session_id"),
            Some(&json!("stasis-desktop-task-image"))
        );
        assert_eq!(
            body.pointer("/messages/0/content/1/image_url/url"),
            Some(&Value::String(expected_url))
        );
        let encoded = body
            .pointer("/messages/0/content/1/image_url/url")
            .and_then(Value::as_str)
            .and_then(|url| url.strip_prefix("data:image/png;base64,"))
            .expect("PNG data URL");
        let delivered = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("payload base64");
        let decoded = image::load_from_memory_with_format(&delivered, image::ImageFormat::Png)
            .expect("delivered PNG")
            .to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [17, 34, 51, 255]);
        assert!(provider.cached_image_input_capability().supported);
        provider
            .respond(&test_request(), &AtomicBool::new(false))
            .expect("next text-only turn");
        let next_request = requests.recv().expect("next chat request");
        let next_json_start = next_request.find("\r\n\r\n").expect("headers") + 4;
        let next_body: Value =
            serde_json::from_str(&next_request[next_json_start..]).expect("next body");
        assert!(next_body
            .pointer("/messages/0/content")
            .is_some_and(Value::is_string));
        worker.join().expect("server");
    }

    #[test]
    fn missing_image_modality_fails_before_chat_and_model_change_clears_cache() {
        let metadata = json!({"data":[{
            "id":DEFAULT_OPENROUTER_MODEL,
            "architecture":{"input_modalities":["text"]}
        }]})
        .to_string();
        let (base_url, requests, worker) =
            mock_server(vec![http_response("application/json", &metadata)]);
        let provider = OpenRouterProvider::new(test_config(base_url))
            .expect("provider")
            .with_image_inputs(vec![test_png()])
            .expect("image inputs");
        let mut configured = ConfiguredProvider::OpenRouter(provider);
        let error = configured
            .respond(&test_request(), &AtomicBool::new(false))
            .expect_err("unsupported model");
        assert!(error.contains("does not report image input"));
        assert!(requests
            .recv()
            .expect("metadata request")
            .starts_with("GET /models "));
        configured = configured.with_model("vendor/new-model");
        let ConfiguredProvider::OpenRouter(provider) = configured else {
            unreachable!()
        };
        assert_eq!(
            provider.cached_image_input_capability().model,
            "vendor/new-model"
        );
        assert!(!provider.cached_image_input_capability().supported);
        assert!(provider
            .cached_image_input_capability()
            .reason
            .contains("not been verified"));
        worker.join().expect("server");
    }

    #[test]
    fn canceled_image_request_never_fetches_metadata_or_sends_chat() {
        let mut provider = OpenRouterProvider::new(test_config("http://127.0.0.1:9".into()))
            .expect("provider")
            .with_image_inputs(vec![test_png()])
            .expect("image inputs");
        assert_eq!(
            provider
                .respond(&test_request(), &AtomicBool::new(true))
                .unwrap_err(),
            "AI request canceled"
        );
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
        let mut config = test_config(base_url);
        config.routing.order = vec!["openai".to_string(), "cerebras".to_string()];
        let mut provider = OpenRouterProvider::new(config).expect("provider");
        provider.reasoning_effort = Some("low".to_string());
        provider.session_id = Some("stasis-test-session".to_string());
        let mut progress = Vec::new();
        let openrouter = provider
            .respond_with_progress(&test_request(), &AtomicBool::new(false), &mut |event| {
                progress.push(event)
            })
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
            read_variant.pointer("/properties/args/anyOf/0/type"),
            Some(&json!("object"))
        );
        assert_eq!(
            read_variant.pointer("/properties/args/anyOf/0/additionalProperties"),
            Some(&json!(false))
        );
        assert_eq!(
            request.pointer("/provider/require_parameters"),
            Some(&json!(true))
        );
        assert_eq!(request.pointer("/reasoning/effort"), Some(&json!("low")));
        assert_eq!(
            request.get("session_id"),
            Some(&json!("stasis-test-session"))
        );
        let usage = provider.take_usage().expect("usage");
        assert_eq!(usage["resolved_provider"], "cerebras");
        assert_eq!(usage["fallback"], true);
        assert_eq!(usage["tokens"]["cache"], 2);
        assert!(usage["timing_ms"]["first_action"].is_number());
        assert!(usage["timing_ms"]["inference_total"].is_number());
        assert!(usage["timing_ms"]["turn_total"].is_number());
        assert!(usage["timing_ms"].get("total").is_none());
        assert_eq!(
            progress,
            vec![
                crate::ProviderProgress::ContactingProvider,
                crate::ProviderProgress::FirstResponse {
                    elapsed_ms: usage["timing_ms"]["first_content"]
                        .as_u64()
                        .expect("first content timing"),
                },
                crate::ProviderProgress::FirstAction {
                    elapsed_ms: usage["timing_ms"]["first_action"]
                        .as_u64()
                        .expect("first action timing"),
                },
                crate::ProviderProgress::Fallback,
            ]
        );
        worker.join().expect("mock worker");
    }

    #[test]
    fn missing_resolved_provider_does_not_claim_fallback() {
        let fixture = json!({
            "mode": "done",
            "working_notes": "Request complete.",
            "summary": "complete"
        });
        let chunk = json!({
            "model": DEFAULT_OPENROUTER_MODEL,
            "choices": [{"delta": {"content": fixture.to_string()}}],
            "usage": {"prompt_tokens": 4, "completion_tokens": 3}
        });
        let body = format!("data: {}\n\ndata: [DONE]\n\n", chunk);
        let (base_url, _requests, worker) =
            mock_server(vec![http_response("text/event-stream", &body)]);
        let mut config = test_config(base_url);
        config.routing.order = vec!["cerebras".to_string(), "openai".to_string()];
        let mut provider = OpenRouterProvider::new(config).expect("provider");
        let mut progress = Vec::new();
        provider
            .respond_with_progress(&test_request(), &AtomicBool::new(false), &mut |event| {
                progress.push(event)
            })
            .expect("stream response");

        let usage = provider.take_usage().expect("usage");
        assert_eq!(usage["resolved_provider"], "unknown");
        assert_eq!(usage["fallback"], false);
        assert!(!progress
            .iter()
            .any(|event| matches!(event, crate::ProviderProgress::Fallback)));
        worker.join().expect("mock worker");
    }

    #[test]
    fn malformed_completed_response_retains_usage() {
        let chunk = json!({"choices":[{"delta":{"content":"{invalid"}}],"usage":{"prompt_tokens":10,"completion_tokens":5,"cost":0.001}});
        let body = format!("data: {}\n\ndata: [DONE]\n\n", chunk);
        let (base_url, requests, worker) =
            mock_server(vec![http_response("text/event-stream", &body)]);
        let mut provider = OpenRouterProvider::new(test_config(base_url)).unwrap();
        assert!(provider
            .respond(&test_request(), &AtomicBool::new(false))
            .is_err());
        let usage = provider.take_usage().expect("failed response usage");
        assert_eq!(usage["cost"], 0.001);
        assert_eq!(usage["tokens"]["prompt"], 10);
        assert_eq!(usage["tokens"]["completion"], 5);
        assert_eq!(usage["validation"]["structured_schema"], "rejected");
        assert!(provider.take_usage().is_none());
        requests.recv().unwrap();
        worker.join().unwrap();
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
        let mut progress = Vec::new();
        let error = provider
            .respond_with_progress("request", &AtomicBool::new(true), &mut |event| {
                progress.push(event)
            })
            .expect_err("canceled");
        assert_eq!(error, "AI request canceled");
        assert!(progress.is_empty());
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

    #[test]
    fn image_input_capability_is_explicit_and_fails_closed() {
        assert!(codex_model_supports_image_input("gpt-5.6-sol"));
        assert!(!codex_model_supports_image_input("gpt-5.6-luna"));
        assert!(!codex_model_supports_image_input("latest"));
        assert!(!codex_model_supports_image_input("unknown-model"));

        let openrouter = ProviderConfig::OpenRouter(OpenRouterConfig {
            api_key: "unit-secret".to_string(),
            base_url: "http://127.0.0.1:9".to_string(),
            model: DEFAULT_OPENROUTER_MODEL.to_string(),
            approved_models: default_approved_openrouter_models().into_boxed_slice(),
            routing: RoutingConfig::default(),
            timeout: Duration::from_secs(1),
        });
        assert!(!openrouter.supports_image_input());

        let unsupported = ProviderConfig::Codex
            .build()
            .expect("Codex provider")
            .with_model("gpt-5.6-luna")
            .with_images(vec![std::path::PathBuf::from("frame.png")]);
        assert!(
            matches!(unsupported, Err(error) if error.contains("does not support image input"))
        );

        let mut switched_after_images = ProviderConfig::Codex
            .build()
            .expect("Codex provider")
            .with_model("gpt-5.6-sol")
            .with_images(vec![std::path::PathBuf::from("frame.png")])
            .expect("supported image model")
            .with_model("unknown-model");
        let error = switched_after_images
            .respond("request", &AtomicBool::new(false))
            .expect_err("dispatch must recheck image capability");
        assert!(error.contains("does not support image input"));
    }
}
