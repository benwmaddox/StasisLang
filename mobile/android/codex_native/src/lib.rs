use std::ffi::{c_char, CStr, CString};
use std::ffi::c_void;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::Duration;

use codex_login::{
    complete_device_code_login, load_auth_dot_json, oauth_client_id, request_device_code,
    AuthCredentialsStoreMode, AuthKeyringBackendKind, AuthManager, ServerOptions,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[no_mangle]
#[cfg(target_os = "android")]
pub extern "C" fn stasis_codex_android_initialize(
    raw_env: *mut c_void,
    raw_context: *mut c_void,
) -> i32 {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut unowned = unsafe {
            jni::EnvUnowned::from_raw(raw_env as *mut jni::sys::JNIEnv)
        };
        let outcome = unowned.with_env(|env| {
            let context = unsafe {
                jni::objects::JObject::from_raw(env, raw_context as jni::sys::jobject)
            };
            rustls_platform_verifier::android::init_with_env(env, context)
        });
        match outcome.into_outcome() {
            jni::Outcome::Ok(()) => Ok(()),
            jni::Outcome::Err(error) => Err(error.to_string()),
            jni::Outcome::Panic(_) => Err("Android verifier initialization panicked".to_string()),
        }
    }));
    match result {
        Ok(Ok(())) => 0,
        _ => -1,
    }
}

const UPSTREAM_CODEX_REVISION: &str = "5c19155cbd93bfa099016e7487259f61669823ff";
const CHATGPT_BASE_URL: &str = "https://chatgpt.com/backend-api";
const CHATGPT_USAGE_URL: &str = "https://chatgpt.com/backend-api/wham/usage";
const CHATGPT_CODEX_MODELS_URL: &str = "https://chatgpt.com/backend-api/codex/models";
const CHATGPT_CODEX_RESPONSES_URL: &str = "https://chatgpt.com/backend-api/codex/responses";
const MAX_CODEX_MODELS_BYTES: usize = 2 * 1024 * 1024;
const MAX_CODEX_RESPONSE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Clone)]
enum LoginState {
    Idle,
    AwaitingUser {
        verification_url: String,
        user_code: String,
    },
    SignedIn,
    Failed(String),
}

static LOGIN_STATE: LazyLock<Mutex<LoginState>> =
    LazyLock::new(|| Mutex::new(LoginState::Idle));
static LOGIN_GENERATION: AtomicU64 = AtomicU64::new(0);
static RESPONSE_GENERATION: AtomicU64 = AtomicU64::new(0);
static CODEX_MODELS: LazyLock<Mutex<Option<Vec<CodexModelInfo>>>> =
    LazyLock::new(|| Mutex::new(None));

fn server_options(codex_home: PathBuf) -> ServerOptions {
    ServerOptions::new(
        codex_home,
        oauth_client_id(),
        None,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
        None,
    )
}

fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Runtime::new().map_err(|error| error.to_string())
}

fn begin_device_login(codex_home: &Path) -> Result<Value, String> {
    std::fs::create_dir_all(codex_home).map_err(|error| error.to_string())?;
    let options = server_options(codex_home.to_path_buf());
    let device_code = runtime()?
        .block_on(request_device_code(&options))
        .map_err(|error| error.to_string())?;
    let verification_url = device_code.verification_url.clone();
    let user_code = device_code.user_code.clone();
    let generation = LOGIN_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    *LOGIN_STATE.lock().map_err(|error| error.to_string())? = LoginState::AwaitingUser {
        verification_url: verification_url.clone(),
        user_code: user_code.clone(),
    };

    thread::spawn(move || {
        let result = runtime().and_then(|runtime| {
            runtime
                .block_on(complete_device_code_login(options, device_code))
                .map_err(|error| error.to_string())
        });
        if LOGIN_GENERATION.load(Ordering::SeqCst) == generation {
            if let Ok(mut state) = LOGIN_STATE.lock() {
                *state = match result {
                    Ok(()) => LoginState::SignedIn,
                    Err(error) => LoginState::Failed(error),
                };
            }
        }
    });

    Ok(json!({
        "status": "awaiting_user",
        "verification_url": verification_url,
        "user_code": user_code,
        "upstream_revision": UPSTREAM_CODEX_REVISION,
    }))
}

fn account_status(codex_home: &Path) -> Result<Value, String> {
    let stored = load_auth_dot_json(
        codex_home,
        AuthCredentialsStoreMode::File,
        AuthKeyringBackendKind::Direct,
    )
    .map_err(|error| error.to_string())?;
    if let Some(auth) = stored {
        let plan = auth
            .tokens
            .as_ref()
            .and_then(|tokens| tokens.id_token.get_chatgpt_plan_type_raw());
        return Ok(json!({
            "status": "signed_in",
            "signed_in": true,
            "auth_mode": auth.auth_mode,
            "plan_type": plan,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }));
    }

    let state = LOGIN_STATE.lock().map_err(|error| error.to_string())?.clone();
    Ok(match state {
        LoginState::Idle => json!({
            "status": "signed_out",
            "signed_in": false,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }),
        LoginState::AwaitingUser {
            verification_url,
            user_code,
        } => json!({
            "status": "awaiting_user",
            "signed_in": false,
            "verification_url": verification_url,
            "user_code": user_code,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }),
        LoginState::SignedIn => json!({
            "status": "signed_in",
            "signed_in": true,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }),
        LoginState::Failed(error) => json!({
            "status": "error",
            "signed_in": false,
            "error": error,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }),
    })
}

#[no_mangle]
#[cfg(not(target_os = "android"))]
pub extern "C" fn stasis_codex_android_initialize(
    _raw_env: *mut c_void,
    _raw_context: *mut c_void,
) -> i32 {
    -1
}

#[derive(Deserialize)]
struct UsagePayload {
    rate_limit: Option<RateLimitDetails>,
}

#[derive(Deserialize)]
struct RateLimitDetails {
    primary_window: Option<RateLimitWindow>,
    secondary_window: Option<RateLimitWindow>,
}

#[derive(Deserialize)]
struct RateLimitWindow {
    used_percent: f64,
    limit_window_seconds: i64,
    reset_at: i64,
}

fn rate_limit_window(window: Option<RateLimitWindow>) -> Value {
    match window {
        Some(window) => json!({
            "used_percent": window.used_percent,
            "remaining_percent": (100.0 - window.used_percent).clamp(0.0, 100.0),
            "window_duration_mins": window.limit_window_seconds / 60,
            "resets_at": window.reset_at,
        }),
        None => Value::Null,
    }
}

fn account_rate_limits(codex_home: &Path) -> Result<Value, String> {
    let codex_home = codex_home.to_path_buf();
    runtime()?.block_on(async move {
        let auth_manager = AuthManager::new(
            codex_home,
            false,
            AuthCredentialsStoreMode::File,
            None,
            Some(CHATGPT_BASE_URL.to_string()),
            AuthKeyringBackendKind::Direct,
            None,
        )
        .await;
        let auth = auth_manager
            .auth()
            .await
            .ok_or_else(|| "ChatGPT sign-in is required to read Codex limits".to_string())?;
        if !auth.uses_codex_backend() {
            return Err("ChatGPT authentication is required to read Codex limits".to_string());
        }
        let token = auth.get_token().map_err(|error| error.to_string())?;
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| error.to_string())?;
        let mut request = client
            .get(CHATGPT_USAGE_URL)
            .bearer_auth(token)
            .header("User-Agent", "codex-cli");
        if let Some(account_id) = auth.get_account_id() {
            request = request.header("ChatGPT-Account-Id", account_id);
        }
        if auth.is_fedramp_account() {
            request = request.header("X-OpenAI-Fedramp", "true");
        }
        let response = request
            .send()
            .await
            .map_err(|error| error.to_string())?;
        if !response.status().is_success() {
            return Err(format!("Codex limits request failed: {}", response.status()));
        }
        let payload: UsagePayload = response.json().await.map_err(|error| error.to_string())?;
        let limits = payload
            .rate_limit
            .ok_or_else(|| "Codex limits response contained no rate-limit windows".to_string())?;
        Ok(json!({
            "status": "ok",
            "limit_id": "codex",
            "primary": rate_limit_window(limits.primary_window),
            "secondary": rate_limit_window(limits.secondary_window),
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }))
    })
}

#[derive(Clone, Debug, Deserialize)]
struct CodexModelInfo {
    slug: String,
    #[serde(default)]
    base_instructions: String,
    #[serde(default)]
    visibility: String,
    #[serde(default)]
    priority: i64,
}

#[derive(Deserialize)]
struct CodexModelsPayload {
    models: Vec<CodexModelInfo>,
}

async fn codex_auth(codex_home: PathBuf) -> Result<codex_login::CodexAuth, String> {
    let auth_manager = AuthManager::new(
        codex_home,
        false,
        AuthCredentialsStoreMode::File,
        None,
        Some(CHATGPT_BASE_URL.to_string()),
        AuthKeyringBackendKind::Direct,
        None,
    )
    .await;
    let auth = auth_manager
        .auth()
        .await
        .ok_or_else(|| "ChatGPT sign-in is required to run Codex".to_string())?;
    if !auth.uses_codex_backend() {
        return Err("ChatGPT authentication is required to run Codex".to_string());
    }
    Ok(auth)
}

fn add_codex_auth_headers(
    mut request: reqwest::RequestBuilder,
    auth: &codex_login::CodexAuth,
) -> Result<reqwest::RequestBuilder, String> {
    let token = auth.get_token().map_err(|error| error.to_string())?;
    request = request
        .bearer_auth(token)
        .header(
            "User-Agent",
            codex_login::default_client::get_codex_user_agent(),
        )
        .header(
            "originator",
            codex_login::default_client::originator().value,
        );
    if let Some(account_id) = auth.get_account_id() {
        request = request.header("ChatGPT-Account-Id", account_id);
    }
    if auth.is_fedramp_account() {
        request = request.header("X-OpenAI-Fedramp", "true");
    }
    Ok(request)
}

async fn bounded_response_bytes(
    mut response: reqwest::Response,
    limit: usize,
    label: &str,
) -> Result<Vec<u8>, String> {
    if response
        .content_length()
        .is_some_and(|length| length > limit as u64)
    {
        return Err(format!("{label} exceeded the {limit}-byte limit"));
    }
    let mut body = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|error| format!("{label} read failed: {error}"))?
    {
        if body.len().saturating_add(chunk.len()) > limit {
            return Err(format!("{label} exceeded the {limit}-byte limit"));
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

async fn codex_models(
    client: &reqwest::Client,
    auth: &codex_login::CodexAuth,
) -> Result<Vec<CodexModelInfo>, String> {
    if let Some(models) = CODEX_MODELS
        .lock()
        .map_err(|error| error.to_string())?
        .clone()
    {
        return Ok(models);
    }
    let request = client.get(format!(
        "{CHATGPT_CODEX_MODELS_URL}?client_version=0.0.0"
    ));
    let response = add_codex_auth_headers(request, auth)?
        .send()
        .await
        .map_err(|error| format!("Codex model discovery failed: {error}"))?;
    if !response.status().is_success() {
        return Err(format!("Codex model discovery failed: {}", response.status()));
    }
    let body = bounded_response_bytes(response, MAX_CODEX_MODELS_BYTES, "Codex model discovery")
        .await?;
    let models = serde_json::from_slice::<CodexModelsPayload>(&body)
        .map_err(|error| format!("Codex model discovery was invalid: {error}"))?
        .models;
    if models.is_empty() {
        return Err("Codex returned no available models".to_string());
    }
    *CODEX_MODELS
        .lock()
        .map_err(|error| error.to_string())? = Some(models.clone());
    Ok(models)
}

fn select_codex_model(
    mut models: Vec<CodexModelInfo>,
    requested_slug: &str,
) -> Result<CodexModelInfo, String> {
    models.sort_by_key(|model| model.priority);
    if !requested_slug.is_empty() {
        return models
            .into_iter()
            .find(|model| model.slug == requested_slug && model.visibility == "list")
            .ok_or_else(|| format!("Codex model is unavailable: {requested_slug}"));
    }
    models
        .iter()
        .find(|model| model.visibility == "list")
        .cloned()
        .or_else(|| models.first().cloned())
        .ok_or_else(|| "Codex returned no available models".to_string())
}

fn prepare_codex_payload(payload: &mut Value, model: &CodexModelInfo) -> Result<(), String> {
    let object = payload
        .as_object_mut()
        .ok_or_else(|| "Codex request must be a JSON object".to_string())?;
    object.insert("model".to_string(), Value::String(model.slug.clone()));
    object.insert("store".to_string(), Value::Bool(false));
    object.insert("stream".to_string(), Value::Bool(true));
    if !model.base_instructions.is_empty() {
        object.insert(
            "instructions".to_string(),
            Value::String(model.base_instructions.clone()),
        );
    }
    Ok(())
}

fn completed_response_from_sse(body: &str) -> Result<Value, String> {
    let mut output = Vec::new();
    let mut completed = None;
    for line in body.lines() {
        let Some(data) = line.strip_prefix("data:") else {
            continue;
        };
        let data = data.trim();
        if data.is_empty() || data == "[DONE]" {
            continue;
        }
        let event: Value = serde_json::from_str(data)
            .map_err(|error| format!("Codex stream contained invalid JSON: {error}"))?;
        match event.get("type").and_then(Value::as_str).unwrap_or("") {
            "response.output_item.done" => {
                if let Some(item) = event.get("item") {
                    output.push(item.clone());
                }
            }
            "response.completed" => completed = event.get("response").cloned(),
            "response.failed" | "error" => {
                let detail = event
                    .pointer("/response/error/message")
                    .or_else(|| event.pointer("/error/message"))
                    .or_else(|| event.get("message"))
                    .and_then(Value::as_str)
                    .unwrap_or("Codex response failed");
                return Err(detail.to_string());
            }
            _ => {}
        }
    }
    let mut response = completed.ok_or_else(|| "Codex stream ended without completion".to_string())?;
    let object = response
        .as_object_mut()
        .ok_or_else(|| "Codex completion was not an object".to_string())?;
    object.insert("output".to_string(), Value::Array(output));
    Ok(response)
}

fn begin_response_generation() -> u64 {
    RESPONSE_GENERATION.fetch_add(1, Ordering::SeqCst) + 1
}

fn cancel_response_generation() {
    RESPONSE_GENERATION.fetch_add(1, Ordering::SeqCst);
}

async fn cancel_on_generation_change<T>(
    generation: u64,
    response: impl Future<Output = Result<T, String>>,
) -> Result<T, String> {
    tokio::pin!(response);
    let cancelled = async move {
        while RESPONSE_GENERATION.load(Ordering::SeqCst) == generation {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::pin!(cancelled);
    tokio::select! {
        result = &mut response => result,
        _ = &mut cancelled => Err("Codex request cancelled".to_string()),
    }
}

fn codex_response(codex_home: &Path, request_json: &str, generation: u64) -> Result<Value, String> {
    let mut payload: Value = serde_json::from_str(request_json)
        .map_err(|error| format!("Codex request JSON was invalid: {error}"))?;
    let requested_model = payload
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    runtime()?.block_on(cancel_on_generation_change(generation, async move {
        let auth = codex_auth(codex_home.to_path_buf()).await?;
        let client = reqwest::Client::builder()
            .build()
            .map_err(|error| error.to_string())?;
        let model = select_codex_model(codex_models(&client, &auth).await?, &requested_model)?;
        prepare_codex_payload(&mut payload, &model)?;
        let request = client
            .post(CHATGPT_CODEX_RESPONSES_URL)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream")
            .json(&payload);
        let response = add_codex_auth_headers(request, &auth)?
            .send()
            .await
            .map_err(|error| format!("Codex request failed: {error}"))?;
        let status = response.status();
        let body = bounded_response_bytes(response, MAX_CODEX_RESPONSE_BYTES, "Codex response")
            .await?;
        let body = String::from_utf8(body)
            .map_err(|error| format!("Codex response was not UTF-8: {error}"))?;
        if !status.is_success() {
            let detail = serde_json::from_str::<Value>(&body)
                .ok()
                .and_then(|value| {
                    value
                        .pointer("/error/message")
                        .and_then(Value::as_str)
                        .map(str::to_string)
                })
                .unwrap_or_else(|| body.chars().take(500).collect());
            return Err(format!("Codex HTTP {status}: {detail}"));
        }
        Ok(json!({
            "status": "ok",
            "model": model.slug,
            "response": completed_response_from_sse(&body)?,
            "upstream_revision": UPSTREAM_CODEX_REVISION,
        }))
    }))
}

fn path_from_c(value: *const c_char) -> Result<PathBuf, String> {
    if value.is_null() {
        return Err("Codex home path was null".to_string());
    }
    let value = unsafe { CStr::from_ptr(value) }
        .to_str()
        .map_err(|error| error.to_string())?;
    if value.trim().is_empty() {
        return Err("Codex home path was empty".to_string());
    }
    Ok(PathBuf::from(value))
}

fn into_c_json(result: Result<Value, String>) -> *mut c_char {
    let value = match result {
        Ok(value) => value,
        Err(error) => json!({"status": "error", "error": error}),
    };
    CString::new(value.to_string())
        .unwrap_or_else(|_| CString::new("{\"status\":\"error\",\"error\":\"invalid native response\"}").unwrap())
        .into_raw()
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_begin_device_login(
    codex_home: *const c_char,
) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(|| {
        path_from_c(codex_home).and_then(|path| begin_device_login(&path))
    })) {
        Ok(result) => into_c_json(result),
        Err(_) => into_c_json(Err("Codex login bridge panicked".to_string())),
    }
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_account_status(
    codex_home: *const c_char,
) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(|| {
        path_from_c(codex_home).and_then(|path| account_status(&path))
    })) {
        Ok(result) => into_c_json(result),
        Err(_) => into_c_json(Err("Codex status bridge panicked".to_string())),
    }
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_account_rate_limits(
    codex_home: *const c_char,
) -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(|| {
        path_from_c(codex_home).and_then(|path| account_rate_limits(&path))
    })) {
        Ok(result) => into_c_json(result),
        Err(_) => into_c_json(Err("Codex rate-limit bridge panicked".to_string())),
    }
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_begin_response() -> u64 {
    begin_response_generation()
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_cancel_response() {
    cancel_response_generation();
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_ai_contract() -> *mut c_char {
    match catch_unwind(AssertUnwindSafe(|| Ok(stasis_ai::contract_json()))) {
        Ok(result) => into_c_json(result),
        Err(_) => into_c_json(Err("shared AI contract bridge panicked".to_string())),
    }
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_response(
    codex_home: *const c_char,
    request_json: *const c_char,
    generation: u64,
) -> *mut c_char {
    let result = catch_unwind(AssertUnwindSafe(|| {
        let home = path_from_c(codex_home)?;
        if request_json.is_null() {
            return Err("Codex request was null".to_string());
        }
        let request = unsafe { CStr::from_ptr(request_json) }
            .to_str()
            .map_err(|error| error.to_string())?;
        codex_response(&home, request, generation)
    }));
    match result {
        Ok(value) => into_c_json(value),
        Err(_) => into_c_json(Err("Codex response bridge panicked".to_string())),
    }
}

#[no_mangle]
pub extern "C" fn stasis_codex_android_free_string(value: *mut c_char) {
    if !value.is_null() {
        unsafe { drop(CString::from_raw(value)) };
    }
}

#[cfg(test)]
mod tests {
    use super::CodexModelInfo;
    use super::completed_response_from_sse;
    use super::prepare_codex_payload;
    use super::select_codex_model;
    use super::{begin_response_generation, cancel_on_generation_change, cancel_response_generation};
    use serde_json::json;

    #[test]
    fn cancellation_stops_an_active_response_future() {
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let generation = begin_response_generation();
        let result = runtime.block_on(async {
            tokio::spawn(async {
                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                cancel_response_generation();
            });
            cancel_on_generation_change(generation, async {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                Ok(())
            }).await
        });
        assert_eq!(result.unwrap_err(), "Codex request cancelled");
    }

    #[test]
    fn assembles_completed_response_items() {
        let body = concat!(
            "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"message\",\"content\":[{\"type\":\"output_text\",\"text\":\"ok\"}]}}\n\n",
            "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"resp_1\",\"usage\":{\"input_tokens\":2}}}\n\n"
        );
        let response = completed_response_from_sse(body).expect("valid completion");
        assert_eq!(response["id"], "resp_1");
        assert_eq!(response["output"][0]["content"][0]["text"], "ok");
        assert_eq!(response["usage"]["input_tokens"], 2);
    }

    #[test]
    fn returns_stream_failure_message() {
        let body = "data: {\"type\":\"response.failed\",\"response\":{\"error\":{\"message\":\"denied\"}}}\n\n";
        assert_eq!(
            completed_response_from_sse(body).expect_err("failure event"),
            "denied"
        );
    }

    #[test]
    fn selects_first_visible_model_by_priority() {
        let hidden = CodexModelInfo {
            slug: "hidden".to_string(),
            base_instructions: String::new(),
            visibility: "hide".to_string(),
            priority: 0,
        };
        let visible = CodexModelInfo {
            slug: "visible".to_string(),
            base_instructions: "base".to_string(),
            visibility: "list".to_string(),
            priority: 4,
        };
        let later = CodexModelInfo {
            slug: "later".to_string(),
            base_instructions: String::new(),
            visibility: "list".to_string(),
            priority: 8,
        };
        assert_eq!(
            select_codex_model(vec![later, visible, hidden], "")
                .expect("default model")
                .slug,
            "visible"
        );
    }

    #[test]
    fn selects_requested_visible_model() {
        let sol = CodexModelInfo {
            slug: "gpt-5.6-sol".to_string(),
            base_instructions: String::new(),
            visibility: "list".to_string(),
            priority: 1,
        };
        let luna = CodexModelInfo {
            slug: "gpt-5.6-luna".to_string(),
            base_instructions: "luna instructions".to_string(),
            visibility: "list".to_string(),
            priority: 3,
        };
        assert_eq!(
            select_codex_model(vec![sol, luna], "gpt-5.6-luna")
                .expect("requested model")
                .slug,
            "gpt-5.6-luna"
        );
    }

    #[test]
    fn rejects_unavailable_requested_model() {
        let sol = CodexModelInfo {
            slug: "gpt-5.6-sol".to_string(),
            base_instructions: String::new(),
            visibility: "list".to_string(),
            priority: 1,
        };
        assert_eq!(
            select_codex_model(vec![sol], "gpt-5.6-luna")
                .expect_err("unavailable model"),
            "Codex model is unavailable: gpt-5.6-luna"
        );
    }

    #[test]
    fn trusted_model_metadata_overrides_transport_fields() {
        let model = CodexModelInfo {
            slug: "gpt-test".to_string(),
            base_instructions: "trusted instructions".to_string(),
            visibility: "list".to_string(),
            priority: 0,
        };
        let mut payload = json!({
            "model": "untrusted",
            "instructions": "untrusted",
            "store": true,
            "stream": false,
            "input": []
        });
        prepare_codex_payload(&mut payload, &model).expect("prepared request");
        assert_eq!(payload["model"], "gpt-test");
        assert_eq!(payload["instructions"], "trusted instructions");
        assert_eq!(payload["store"], false);
        assert_eq!(payload["stream"], true);
    }
}
