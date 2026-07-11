use std::ffi::{c_char, CStr, CString};
use std::ffi::c_void;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;

use codex_login::{
    complete_device_code_login, load_auth_dot_json, oauth_client_id, request_device_code,
    AuthCredentialsStoreMode, AuthKeyringBackendKind, AuthManager, ServerOptions,
};
use serde::Deserialize;
use serde_json::{json, Value};

#[no_mangle]
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
pub extern "C" fn stasis_codex_android_free_string(value: *mut c_char) {
    if !value.is_null() {
        unsafe { drop(CString::from_raw(value)) };
    }
}
