//! Adapter for the repository-owned Windows signing policy.
//!
//! The legacy `STASIS_AOT_SIGN_TOOL` hook intentionally remains a one-argument
//! compatibility hook. Windows Authenticode policy lives in
//! `tools/windows/stasis-signing.ps1`.

use serde::{Deserialize, Serialize};
use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(test)]
use std::sync::{Mutex, OnceLock};

const SIGN_TOOL_ENV: &str = "STASIS_AOT_SIGN_TOOL";
const REQUIRE_SIGNED_ENV: &str = "STASIS_REQUIRE_SIGNED_EXECUTION";
const SIGNING_MODE_ENV: &str = "STASIS_SIGNING_MODE";
const CERTIFICATE_ENV: &str = "STASIS_SIGNING_CERTIFICATE";
const THUMBPRINT_ENV: &str = "STASIS_SIGNING_CERT_THUMBPRINT";
const LOCAL_RECORD_ENV: &str = "STASIS_SIGNING_LOCAL_RECORD";
const DEVELOPMENT_SUBJECT: &str = "CN=StasisLang Development Signing";
const SIGNING_SCRIPT_RELATIVE: &str = "tools/windows/stasis-signing.ps1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SigningStatus {
    pub platform: &'static str,
    pub required: bool,
    pub signer: Option<String>,
    pub signer_source: Option<&'static str>,
    pub certificate_configured: bool,
    pub local_development_certificate_configured: bool,
    pub production_credentials_configured: bool,
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ProvisionResult {
    pub subject: &'static str,
    pub store: &'static str,
    pub thumbprint: String,
    pub personal_store: String,
    pub trust_store: String,
    pub certificate: String,
    pub root_trust: String,
    pub record: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SigningOptions {
    pub tool: Option<PathBuf>,
    pub certificate: Option<PathBuf>,
    pub thumbprint: Option<String>,
    pub timestamp_url: Option<String>,
}

impl Default for SigningOptions {
    fn default() -> Self {
        Self {
            tool: None,
            certificate: None,
            thumbprint: None,
            timestamp_url: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignerSource {
    Explicit,
    Path,
    WindowsKit,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Signer {
    path: PathBuf,
    source: SignerSource,
}

impl SignerSource {
    fn label(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::Path => "path",
            Self::WindowsKit => "windows-kits",
        }
    }
}

fn env_nonempty(name: &str) -> Option<OsString> {
    env::var_os(name).filter(|value| !value.is_empty())
}

fn resolve_signing_script_for(
    current_exe: Option<&Path>,
    source_root: &Path,
) -> Result<PathBuf, String> {
    let mut candidates = Vec::new();
    if let Some(directory) = current_exe.and_then(Path::parent) {
        candidates.push(directory.join(SIGNING_SCRIPT_RELATIVE));
        if let Some(parent) = directory.parent() {
            candidates.push(parent.join(SIGNING_SCRIPT_RELATIVE));
        }
    }
    candidates.push(source_root.join(SIGNING_SCRIPT_RELATIVE));
    candidates.dedup();
    if let Some(path) = candidates.iter().find(|path| path.is_file()) {
        return Ok(path.clone());
    }
    Err(format!(
        "Windows signing policy script is missing; checked {}. Reinstall the complete Stasis toolchain or restore {SIGNING_SCRIPT_RELATIVE}",
        candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

fn signing_script() -> Result<PathBuf, String> {
    let source_root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    resolve_signing_script_for(env::current_exe().ok().as_deref(), &source_root)
}

fn script_command(action: &str) -> Result<Command, String> {
    let script = signing_script()?;
    let mut command = Command::new("powershell.exe");
    command.args([
        "-NoProfile",
        "-NonInteractive",
        "-ExecutionPolicy",
        "Bypass",
        "-File",
    ]);
    command.arg(script).arg(action);
    Ok(command)
}

fn add_signing_options(command: &mut Command, options: &SigningOptions) {
    for (name, value) in [
        ("-Tool", options.tool.as_deref()),
        ("-Certificate", options.certificate.as_deref()),
    ] {
        if let Some(value) = value {
            command.arg(name).arg(value);
        }
    }
    if let Some(value) = &options.thumbprint {
        command.arg("-Thumbprint").arg(value);
    }
    if let Some(value) = &options.timestamp_url {
        command.arg("-TimestampUrl").arg(value);
    }
}

fn run_script(mut command: Command, action: &str) -> Result<String, String> {
    let output = command.output().map_err(|error| {
        format!("failed to launch Windows signing policy for {action}: {error}")
    })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!(
            "Windows signing policy {action} failed with status {}: {detail}",
            output.status
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn production_mode() -> bool {
    env::var(SIGNING_MODE_ENV)
        .ok()
        .is_some_and(|value| value.eq_ignore_ascii_case("production"))
        || env::var("STASIS_SIGNING_PROFILE")
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("production"))
}

fn local_record_path() -> Option<PathBuf> {
    env_nonempty(LOCAL_RECORD_ENV)
        .map(PathBuf::from)
        .or_else(|| {
            env_nonempty("LOCALAPPDATA").map(PathBuf::from).map(|root| {
                root.join("Stasis")
                    .join("signing")
                    .join("development-thumbprint.txt")
            })
        })
}

fn read_local_development_thumbprint() -> Option<String> {
    if production_mode() {
        return None;
    }
    let path = local_record_path()?;
    let value = fs::read_to_string(path).ok()?.trim().to_string();
    (!value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit())).then_some(value)
}

pub fn signing_required() -> bool {
    env_nonempty(REQUIRE_SIGNED_ENV).is_some_and(|value| value == "1")
        || env::var(SIGNING_MODE_ENV)
            .ok()
            .is_some_and(|value| value.eq_ignore_ascii_case("required"))
}

fn is_signtool(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name.eq_ignore_ascii_case("signtool.exe") || name.eq_ignore_ascii_case("signtool")
        })
}

fn explicit_signer() -> Option<Signer> {
    env_nonempty(SIGN_TOOL_ENV).map(|path| Signer {
        path: PathBuf::from(path),
        source: SignerSource::Explicit,
    })
}

fn path_signer() -> Option<Signer> {
    let path = env::var_os("PATH")?;
    let mut entries: Vec<PathBuf> = env::split_paths(&path).collect();
    entries.sort_by(|left, right| left.to_string_lossy().cmp(&right.to_string_lossy()));
    entries.dedup();
    for entry in entries {
        for name in ["signtool.exe", "signtool"] {
            let candidate = entry.join(name);
            if candidate.is_file() {
                return Some(Signer {
                    path: candidate,
                    source: SignerSource::Path,
                });
            }
        }
    }
    None
}

fn windows_kit_signer() -> Option<Signer> {
    let roots = [
        PathBuf::from(r"C:\Program Files (x86)\Windows Kits\10\bin"),
        PathBuf::from(r"C:\Program Files\Windows Kits\10\bin"),
    ];
    let architectures = ["x64", "x86", "arm64", "arm"];
    let mut candidates: Vec<(String, usize, PathBuf)> = Vec::new();
    for root in roots {
        let Ok(entries) = fs::read_dir(root) else {
            continue;
        };
        for entry in entries.flatten() {
            let version = entry.file_name().to_string_lossy().into_owned();
            if !entry.path().is_dir() {
                continue;
            }
            for (rank, architecture) in architectures.iter().enumerate() {
                let candidate = entry.path().join(architecture).join("signtool.exe");
                if candidate.is_file() {
                    candidates.push((version.clone(), rank, candidate));
                }
            }
        }
    }
    candidates.sort_by(|left, right| {
        compare_versions(&right.0, &left.0)
            .then_with(|| left.1.cmp(&right.1))
            .then_with(|| left.2.to_string_lossy().cmp(&right.2.to_string_lossy()))
    });
    candidates.into_iter().next().map(|(_, _, path)| Signer {
        path,
        source: SignerSource::WindowsKit,
    })
}

fn compare_versions(left: &str, right: &str) -> std::cmp::Ordering {
    let left = left
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    let right = right
        .split('.')
        .map(|part| part.parse::<u64>().unwrap_or(0))
        .collect::<Vec<_>>();
    for index in 0..left.len().max(right.len()) {
        let ordering = left
            .get(index)
            .copied()
            .unwrap_or(0)
            .cmp(&right.get(index).copied().unwrap_or(0));
        if ordering != std::cmp::Ordering::Equal {
            return ordering;
        }
    }
    std::cmp::Ordering::Equal
}

fn discover_signer(explicit: Option<&Path>) -> Option<Signer> {
    explicit
        .map(|path| Signer {
            path: path.to_path_buf(),
            source: SignerSource::Explicit,
        })
        .or_else(explicit_signer)
        .or_else(path_signer)
        .or_else(windows_kit_signer)
}

fn signer_is_available(signer: &Signer) -> bool {
    if signer.path.components().count() > 1 || signer.path.is_absolute() {
        signer.path.is_file()
    } else {
        true
    }
}

fn configured_certificate(options: &SigningOptions) -> (Option<PathBuf>, Option<String>) {
    (
        options
            .certificate
            .clone()
            .or_else(|| env_nonempty(CERTIFICATE_ENV).map(PathBuf::from)),
        options.thumbprint.clone().or_else(|| {
            env_nonempty(THUMBPRINT_ENV)
                .map(|value| value.to_string_lossy().into_owned())
                .or_else(read_local_development_thumbprint)
        }),
    )
}

fn discovered_status() -> SigningStatus {
    let signer = discover_signer(None);
    let (certificate, thumbprint) = configured_certificate(&SigningOptions::default());
    let local_development_certificate_configured = read_local_development_thumbprint().is_some();
    let certificate_configured = certificate.as_ref().is_some_and(|path| path.is_file())
        || thumbprint.as_ref().is_some_and(|value| !value.is_empty());
    let production_credentials_configured = certificate.as_ref().is_some_and(|path| path.is_file())
        || env_nonempty(THUMBPRINT_ENV).is_some()
        || env_nonempty("STASIS_SIGNING_PFX_BASE64").is_some();
    let mut diagnostics = Vec::new();
    if signer.is_none() {
        diagnostics.push(
            "signtool.exe was not found; set STASIS_AOT_SIGN_TOOL, add it to PATH, or install the Windows SDK"
                .to_string(),
        );
    }
    if !certificate_configured {
        diagnostics.push(
            "no signing certificate is configured; provision a local test certificate explicitly with 'stasis signing provision' or set STASIS_SIGNING_CERT_THUMBPRINT/STASIS_SIGNING_CERTIFICATE for CI"
                .to_string(),
        );
    }
    SigningStatus {
        platform: if cfg!(windows) {
            "windows"
        } else {
            "non-windows"
        },
        required: signing_required(),
        signer: signer
            .as_ref()
            .map(|value| value.path.display().to_string()),
        signer_source: signer.as_ref().map(|value| value.source.label()),
        certificate_configured,
        local_development_certificate_configured,
        production_credentials_configured,
        diagnostics,
    }
}

#[derive(Deserialize)]
struct ScriptStatus {
    signer: Option<String>,
    signer_source: Option<String>,
    certificate_configured: bool,
    certificate_diagnostic: Option<String>,
    local_development_certificate_configured: bool,
    production_credentials_configured: bool,
    required: bool,
}

fn signer_source_label(source: Option<&str>) -> Option<&'static str> {
    match source {
        Some("explicit") => Some("explicit"),
        Some("path") => Some("path"),
        Some("windows-kits") => Some("windows-kits"),
        Some(_) => Some("script"),
        None => None,
    }
}

pub fn status() -> SigningStatus {
    if !cfg!(windows) {
        return discovered_status();
    }
    let result = script_command("status")
        .and_then(|command| run_script(command, "status"))
        .and_then(|json| {
            serde_json::from_str::<ScriptStatus>(&json).map_err(|error| {
                format!("Windows signing policy returned invalid status JSON: {error}")
            })
        });
    match result {
        Ok(script) => {
            let mut diagnostics = Vec::new();
            if script.signer.is_none() {
                diagnostics.push("signtool.exe was not found; set STASIS_AOT_SIGN_TOOL, add it to PATH, or install the Windows SDK".to_string());
            }
            if !script.certificate_configured {
                diagnostics.push(script.certificate_diagnostic.unwrap_or_else(|| "no signing certificate is configured; provision a local test certificate explicitly with 'stasis signing provision' or set STASIS_SIGNING_CERT_THUMBPRINT/STASIS_SIGNING_CERTIFICATE for CI".to_string()));
            }
            SigningStatus {
                platform: "windows",
                required: script.required,
                signer_source: signer_source_label(script.signer_source.as_deref()),
                signer: script.signer,
                certificate_configured: script.certificate_configured,
                local_development_certificate_configured: script
                    .local_development_certificate_configured,
                production_credentials_configured: script.production_credentials_configured,
                diagnostics,
            }
        }
        Err(error) => {
            let mut fallback = discovered_status();
            fallback.diagnostics.insert(0, error);
            fallback
        }
    }
}

#[derive(Deserialize)]
struct ScriptProvisionResult {
    subject: String,
    store: String,
    thumbprint: String,
    personal_store: String,
    trust_store: String,
    certificate: String,
    root_trust: String,
    record: String,
}

fn parse_provision_result(json: &str) -> Result<ProvisionResult, String> {
    let script: ScriptProvisionResult = serde_json::from_str(json).map_err(|error| {
        format!("Windows signing policy returned invalid provision JSON: {error}")
    })?;
    if script.subject != DEVELOPMENT_SUBJECT
        || script.store != "CurrentUser\\My"
        || script.personal_store != "CurrentUser\\My"
        || script.trust_store != "CurrentUser\\Root"
    {
        return Err(format!(
            "Windows signing policy returned unexpected certificate identity: subject={} personal_store={} trust_store={}",
            script.subject, script.personal_store, script.trust_store
        ));
    }
    if script.thumbprint.is_empty()
        || !script
            .thumbprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(
            "Windows signing policy returned an invalid certificate thumbprint".to_string(),
        );
    }
    Ok(ProvisionResult {
        subject: DEVELOPMENT_SUBJECT,
        store: "CurrentUser\\My",
        thumbprint: script.thumbprint,
        personal_store: script.personal_store,
        trust_store: script.trust_store,
        certificate: script.certificate,
        root_trust: script.root_trust,
        record: script.record,
    })
}

pub fn provision_local_certificate() -> Result<ProvisionResult, String> {
    if !cfg!(windows) {
        return Err(
            "local Windows signing certificate provisioning is only available on Windows"
                .to_string(),
        );
    }
    let json = run_script(script_command("provision")?, "provision")?;
    parse_provision_result(&json)
}

fn run_legacy_hook(path: &Path, artifact: &Path) -> Result<(), String> {
    if cfg!(windows) {
        let mut command = script_command("sign")?;
        add_legacy_hook_arguments(&mut command, path, artifact);
        return run_script(
            command,
            &format!("legacy-hook sign for {}", artifact.display()),
        )
        .map(|_| ());
    }
    let status = Command::new(path).arg(artifact).status().map_err(|error| {
        format!(
            "failed to launch configured signer {}: {error}",
            path.display()
        )
    })?;
    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "configured signer {} failed for {} with status {}",
            path.display(),
            artifact.display(),
            status
        ))
    }
}

fn add_legacy_hook_arguments(command: &mut Command, path: &Path, artifact: &Path) {
    command
        .arg("-Tool")
        .arg(path)
        .arg("-Artifact")
        .arg(artifact);
}

fn policy_signing_decision(
    platform_windows: bool,
    legacy_hook_configured: bool,
    certificate_configured: bool,
    required: bool,
) -> Result<bool, String> {
    if !platform_windows || legacy_hook_configured {
        return Ok(false);
    }
    if certificate_configured {
        return Ok(true);
    }
    if required {
        return Err(
            "required Windows signing has no configured certificate; provision local development signing explicitly or configure production credentials"
                .to_string(),
        );
    }
    Ok(false)
}

pub fn sign_artifact(artifact: &Path, options: &SigningOptions) -> Result<(), String> {
    if !artifact.is_file() {
        return Err(format!(
            "signing input does not exist: {}",
            artifact.display()
        ));
    }
    if options.tool.is_none() {
        if let Some(path) = env_nonempty(SIGN_TOOL_ENV)
            .map(PathBuf::from)
            .filter(|path| !is_signtool(path))
        {
            if !signer_is_available(&Signer {
                path: path.clone(),
                source: SignerSource::Explicit,
            }) {
                return Err(format!(
                    "configured signer tool {} does not exist",
                    path.display()
                ));
            }
            return run_legacy_hook(&path, artifact);
        }
    }
    if !cfg!(windows) {
        return Err(
            "Authenticode signing is only available for Windows artifacts on a Windows host"
                .to_string(),
        );
    }
    let mut command = script_command("sign")?;
    add_signing_options(&mut command, options);
    command.arg("-Artifact").arg(artifact);
    run_script(command, &format!("sign for {}", artifact.display())).map(|_| ())
}

pub fn sign_artifacts(artifacts: &[PathBuf], options: &SigningOptions) -> Result<(), String> {
    if artifacts.is_empty() {
        return Err("signing requires at least one explicit artifact path".to_string());
    }
    for artifact in artifacts {
        sign_artifact(artifact, options)?;
    }
    Ok(())
}

pub fn verify_artifact(artifact: &Path, tool: Option<&Path>) -> Result<(), String> {
    if !cfg!(windows) {
        return Err(
            "Authenticode verification is only available for Windows artifacts on a Windows host"
                .to_string(),
        );
    }
    if !artifact.is_file() {
        return Err(format!(
            "verification input does not exist: {}",
            artifact.display()
        ));
    }
    let mut command = script_command("verify")?;
    if let Some(tool) = tool {
        command.arg("-Tool").arg(tool);
    }
    command.arg("-Artifact").arg(artifact);
    run_script(command, &format!("verify for {}", artifact.display())).map(|_| ())
}

pub fn verify_artifacts(artifacts: &[PathBuf], tool: Option<&Path>) -> Result<(), String> {
    if artifacts.is_empty() {
        return Err("verification requires at least one explicit artifact path".to_string());
    }
    for artifact in artifacts {
        verify_artifact(artifact, tool)?;
    }
    Ok(())
}

pub fn sign_output_artifact_if_configured(artifact: &Path) -> Result<(), String> {
    let configured = env_nonempty(SIGN_TOOL_ENV).is_some();
    if !cfg!(windows) && !configured {
        return Ok(());
    }
    let legacy_hook = env_nonempty(SIGN_TOOL_ENV)
        .map(PathBuf::from)
        .filter(|path| !is_signtool(path));
    if let Some(path) = legacy_hook.as_ref() {
        if !signer_is_available(&Signer {
            path: path.clone(),
            source: SignerSource::Explicit,
        }) {
            if signing_required() {
                return Err(format!(
                    "configured signer tool {} does not exist",
                    path.display()
                ));
            }
            eprintln!(
                "warning: ignoring unavailable optional signer tool {}",
                path.display()
            );
            return Ok(());
        }
        return run_legacy_hook(&path, artifact);
    }
    let status = status();
    let should_attempt = policy_signing_decision(
        cfg!(windows),
        legacy_hook.is_some(),
        status.certificate_configured,
        signing_required(),
    )?;
    if !should_attempt {
        eprintln!(
            "warning: ignoring optional signing for {}: {}",
            artifact.display(),
            status.diagnostics.join("; ")
        );
        return Ok(());
    }
    if status.signer.is_none() {
        if signing_required() {
            return Err(status.diagnostics.join("; "));
        }
        eprintln!(
            "warning: ignoring optional signing for {}: {}",
            artifact.display(),
            status.diagnostics.join("; ")
        );
        return Ok(());
    }
    sign_artifact(artifact, &SigningOptions::default())
}

#[cfg(test)]
mod tests {
    use super::*;

    static TEST_ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    #[test]
    fn required_mode_accepts_legacy_requirement_switch() {
        let _guard = TEST_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let old = env::var_os(REQUIRE_SIGNED_ENV);
        env::set_var(REQUIRE_SIGNED_ENV, "1");
        assert!(signing_required());
        match old {
            Some(value) => env::set_var(REQUIRE_SIGNED_ENV, value),
            None => env::remove_var(REQUIRE_SIGNED_ENV),
        }
    }

    #[test]
    fn development_identity_is_explicit_and_non_exportable() {
        assert_eq!(DEVELOPMENT_SUBJECT, "CN=StasisLang Development Signing");
    }

    #[test]
    fn signing_script_resolves_source_and_installed_layouts() {
        let root = env::temp_dir().join(format!(
            "stasis-signing-script-layout-{}",
            std::process::id()
        ));
        let installed = root.join("installed");
        let script = installed.join(SIGNING_SCRIPT_RELATIVE);
        fs::create_dir_all(script.parent().unwrap()).unwrap();
        fs::write(&script, "# fixture").unwrap();

        assert_eq!(
            resolve_signing_script_for(Some(&installed.join("stasis.exe")), &root).unwrap(),
            script
        );
        assert_eq!(
            resolve_signing_script_for(Some(&installed.join("bin/stasis.exe")), &root).unwrap(),
            installed.join(SIGNING_SCRIPT_RELATIVE)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn missing_signing_script_has_actionable_diagnostic() {
        let root = env::temp_dir().join(format!(
            "stasis-missing-signing-script-{}",
            std::process::id()
        ));
        let error = resolve_signing_script_for(Some(&root.join("stasis.exe")), &root)
            .expect_err("missing signing policy must fail");
        assert!(error.contains(SIGNING_SCRIPT_RELATIVE));
        assert!(error.contains("Reinstall"));
    }

    #[test]
    fn provision_result_preserves_certificate_and_trust_changes() {
        let result = parse_provision_result(
            r#"{"subject":"CN=StasisLang Development Signing","store":"CurrentUser\\My","thumbprint":"ABCDEF123456","personal_store":"CurrentUser\\My","trust_store":"CurrentUser\\Root","certificate":"reused","root_trust":"already-present","record":"unchanged"}"#,
        )
        .expect("valid provision result");

        assert_eq!(result.thumbprint, "ABCDEF123456");
        assert_eq!(result.certificate, "reused");
        assert_eq!(result.root_trust, "already-present");
        assert_eq!(result.record, "unchanged");
    }

    #[test]
    fn windows_legacy_hook_arguments_route_one_artifact_through_policy() {
        let hook = Path::new(r"C:\signing tools\legacy-hook.cmd");
        let artifact = Path::new(r"C:\build output\game.exe");
        let mut command = Command::new("powershell.exe");
        add_legacy_hook_arguments(&mut command, hook, artifact);

        let arguments = command.get_args().collect::<Vec<_>>();
        assert_eq!(
            arguments,
            [
                std::ffi::OsStr::new("-Tool"),
                hook.as_os_str(),
                std::ffi::OsStr::new("-Artifact"),
                artifact.as_os_str(),
            ]
        );
    }

    #[test]
    fn windows_kit_versions_sort_numerically() {
        assert_eq!(
            compare_versions("10.0.26100.1", "10.0.22621.0"),
            std::cmp::Ordering::Greater
        );
        assert_eq!(
            compare_versions("10.0.9", "10.0.10"),
            std::cmp::Ordering::Less
        );
    }

    #[test]
    fn optional_policy_attempts_configured_certificate_without_legacy_hook() {
        assert_eq!(policy_signing_decision(true, false, true, false), Ok(true));
        assert_eq!(
            policy_signing_decision(true, false, false, false),
            Ok(false)
        );
        assert!(policy_signing_decision(true, false, false, true).is_err());
        assert_eq!(policy_signing_decision(false, false, true, true), Ok(false));
    }

    #[test]
    fn local_record_is_reused_for_development_signing_only() {
        let _guard = TEST_ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap();
        let path =
            env::temp_dir().join(format!("stasis-signing-record-{}.txt", std::process::id()));
        fs::write(&path, "ABCDEF123456\n").unwrap();
        let old_record = env::var_os(LOCAL_RECORD_ENV);
        let old_profile = env::var_os("STASIS_SIGNING_PROFILE");
        let old_thumbprint = env::var_os(THUMBPRINT_ENV);
        env::set_var(LOCAL_RECORD_ENV, &path);
        env::remove_var("STASIS_SIGNING_PROFILE");
        env::remove_var(THUMBPRINT_ENV);
        assert_eq!(
            configured_certificate(&SigningOptions::default())
                .1
                .as_deref(),
            Some("ABCDEF123456")
        );
        env::set_var("STASIS_SIGNING_PROFILE", "production");
        assert_eq!(read_local_development_thumbprint(), None);
        match old_record {
            Some(value) => env::set_var(LOCAL_RECORD_ENV, value),
            None => env::remove_var(LOCAL_RECORD_ENV),
        }
        match old_profile {
            Some(value) => env::set_var("STASIS_SIGNING_PROFILE", value),
            None => env::remove_var("STASIS_SIGNING_PROFILE"),
        }
        match old_thumbprint {
            Some(value) => env::set_var(THUMBPRINT_ENV, value),
            None => env::remove_var(THUMBPRINT_ENV),
        }
        let _ = fs::remove_file(path);
    }
}
