//! Repository-owned Windows development signing policy.
//!
//! The legacy `STASIS_AOT_SIGN_TOOL` hook intentionally remains a one-argument
//! compatibility hook.  Stasis-controlled signtool invocations use the
//! explicit SHA-256 and page-hash switches in this module.

use serde::Serialize;
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
const TIMESTAMP_ENV: &str = "STASIS_SIGNING_TIMESTAMP_URL";
const LOCAL_RECORD_ENV: &str = "STASIS_SIGNING_LOCAL_RECORD";
const DEVELOPMENT_SUBJECT: &str = "CN=StasisLang Development Signing";

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

fn write_local_development_thumbprint(thumbprint: &str) -> Result<(), String> {
    let path = local_record_path().ok_or_else(|| {
        "cannot persist local development certificate selection: LOCALAPPDATA is not set; set STASIS_SIGNING_LOCAL_RECORD explicitly".to_string()
    })?;
    let parent = path.parent().ok_or_else(|| {
        format!(
            "local signing record has no parent directory: {}",
            path.display()
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        format!(
            "failed to create local signing record directory {}: {error}",
            parent.display()
        )
    })?;
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    fs::write(&temporary, format!("{thumbprint}\n")).map_err(|error| {
        format!(
            "failed to write local signing record {}: {error}",
            temporary.display()
        )
    })?;
    if path.exists() {
        fs::remove_file(&path).map_err(|error| {
            let _ = fs::remove_file(&temporary);
            format!(
                "failed to replace stale local signing record {}: {error}",
                path.display()
            )
        })?;
    }
    fs::rename(&temporary, &path).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!(
            "failed to publish local signing record {}: {error}",
            path.display()
        )
    })
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

pub fn status() -> SigningStatus {
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

pub fn provision_local_certificate() -> Result<ProvisionResult, String> {
    if !cfg!(windows) {
        return Err(
            "local Windows signing certificate provisioning is only available on Windows"
                .to_string(),
        );
    }
    if production_mode() {
        return Err("production signing never provisions certificates; configure externally supplied CI credentials".to_string());
    }
    let command = format!(
        "$c = New-SelfSignedCertificate -Type CodeSigningCert -Subject '{DEVELOPMENT_SUBJECT}' -CertStoreLocation 'Cert:\\CurrentUser\\My' -KeyExportPolicy NonExportable -KeyLength 2048 -HashAlgorithm SHA256; $c.Thumbprint"
    );
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            &command,
        ])
        .output()
        .map_err(|error| {
            format!("failed to launch PowerShell for CurrentUser certificate provisioning: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "CurrentUser development certificate provisioning failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let thumbprint = String::from_utf8_lossy(&output.stdout)
        .lines()
        .rev()
        .map(str::trim)
        .find(|value| !value.is_empty() && value.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .unwrap_or_default()
        .to_string();
    if thumbprint.is_empty() {
        return Err("certificate provisioning returned no thumbprint".to_string());
    }
    write_local_development_thumbprint(&thumbprint)?;
    Ok(ProvisionResult {
        subject: DEVELOPMENT_SUBJECT,
        store: "CurrentUser\\My",
        thumbprint,
    })
}

fn signer_error(signer: Option<&Signer>, required: bool) -> Result<&Signer, String> {
    let Some(signer) = signer else {
        return Err("Windows signing is unavailable: signtool.exe was not found. Set STASIS_AOT_SIGN_TOOL, add signtool.exe to PATH, or install the Windows SDK. For local development run 'stasis signing provision'; for production configure an externally supplied certificate and signer.".to_string());
    };
    if !signer_is_available(signer) {
        let message = format!(
            "configured signer tool {} does not exist; set STASIS_AOT_SIGN_TOOL to signtool.exe or install the Windows SDK",
            signer.path.display()
        );
        if required {
            return Err(message);
        }
    }
    Ok(signer)
}

fn run_signtool_sign(
    signer: &Signer,
    artifact: &Path,
    options: &SigningOptions,
) -> Result<(), String> {
    let (certificate, thumbprint) = configured_certificate(options);
    if certificate.is_none() && thumbprint.is_none() {
        return Err("Windows signing requires a certificate. Set STASIS_SIGNING_CERT_THUMBPRINT or STASIS_SIGNING_CERTIFICATE; local development may provision an explicitly requested CurrentUser certificate with 'stasis signing provision'. Production credentials are never generated by Stasis.".to_string());
    }
    let mut command = Command::new(&signer.path);
    command.args(["sign", "/fd", "SHA256", "/ph"]);
    if let Some(path) = certificate {
        command.args(["/f", path.to_string_lossy().as_ref()]);
        if let Some(password) = env_nonempty("STASIS_SIGNING_PFX_PASSWORD") {
            command.args(["/p", password.to_string_lossy().as_ref()]);
        }
    } else if let Some(thumbprint) = thumbprint {
        command.args(["/sha1", &thumbprint]);
    }
    let timestamp = options
        .timestamp_url
        .clone()
        .or_else(|| env_nonempty(TIMESTAMP_ENV).map(|value| value.to_string_lossy().into_owned()));
    if let Some(timestamp) = timestamp {
        command.args(["/tr", timestamp.as_str(), "/td", "SHA256"]);
    }
    command.arg(artifact);
    let output = command.output().map_err(|error| {
        format!(
            "failed to launch signtool {} for {}: {error}",
            signer.path.display(),
            artifact.display()
        )
    })?;
    if !output.status.success() {
        return Err(format!(
            "signtool failed for {} with status {}: {}",
            artifact.display(),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

fn run_legacy_hook(path: &Path, artifact: &Path) -> Result<(), String> {
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
    if !cfg!(windows) {
        return Err(
            "Authenticode signing is only available for Windows artifacts on a Windows host"
                .to_string(),
        );
    }
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
    let signer = discover_signer(options.tool.as_deref());
    let signer = signer_error(signer.as_ref(), true)?;
    run_signtool_sign(signer, artifact, options)
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
    let signer = if let Some(tool) = tool {
        if !is_signtool(tool) {
            return Err(format!(
                "verification requires a real signtool.exe; configured legacy hook {} only supports signing",
                tool.display()
            ));
        }
        discover_signer(Some(tool))
    } else if let Some(configured) = env_nonempty(SIGN_TOOL_ENV).map(PathBuf::from) {
        if is_signtool(&configured) {
            discover_signer(Some(&configured))
        } else {
            path_signer().or_else(windows_kit_signer)
        }
    } else {
        path_signer().or_else(windows_kit_signer)
    };
    if signer.is_none()
        && env_nonempty(SIGN_TOOL_ENV)
            .map(PathBuf::from)
            .is_some_and(|path| !is_signtool(&path))
    {
        return Err(
            "signature verification cannot use STASIS_AOT_SIGN_TOOL because it is a legacy signing hook; install signtool.exe or pass --tool signtool.exe"
                .to_string(),
        );
    }
    let signer = signer_error(signer.as_ref(), true)?;
    let output = Command::new(&signer.path)
        .args(["verify", "/pa", "/all"])
        .arg(artifact)
        .output()
        .map_err(|error| format!("failed to launch signtool verification: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "signature verification failed for {}: {}",
            artifact.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
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
    if let Some(path) = env_nonempty(SIGN_TOOL_ENV).map(PathBuf::from) {
        if !is_signtool(&path) {
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
    }
    let status = status();
    let should_attempt = policy_signing_decision(
        cfg!(windows),
        configured,
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
        write_local_development_thumbprint("ABCDEF123456").unwrap();
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
