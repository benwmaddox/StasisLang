use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostSetProfile {
    Dev,
    Test,
    Prod,
}

impl HostSetProfile {
    pub fn as_str(self) -> &'static str {
        match self {
            HostSetProfile::Dev => "dev",
            HostSetProfile::Test => "test",
            HostSetProfile::Prod => "prod",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "dev" => Some(HostSetProfile::Dev),
            "test" => Some(HostSetProfile::Test),
            "prod" => Some(HostSetProfile::Prod),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostSetContract {
    pub host_set_id: String,
    pub host_set_hash: [u8; 32],
}

#[derive(Debug, Clone, Deserialize)]
struct HostSetRegistryFile {
    profiles: BTreeMap<String, HostSetRegistryProfileEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct HostSetRegistryProfileEntry {
    id: String,
    #[serde(default)]
    hash: Option<String>,
}

pub fn contract_hash_from_id(host_set_id: &str) -> [u8; 32] {
    // Deterministic placeholder hash until the contract includes export/phase metadata.
    let input = format!("stasis.host_set_contract.v1:{host_set_id}");
    let digest: [u8; 32] = Sha256::digest(input.as_bytes()).into();
    digest
}

pub fn resolve_profile_contract(
    profile: HostSetProfile,
    registry_file: Option<&Path>,
) -> Result<HostSetContract, String> {
    if let Some(path) = registry_file {
        let text = fs::read_to_string(path).map_err(|error| {
            format!(
                "failed reading host-set registry file {}: {error}",
                path.display()
            )
        })?;
        let registry: HostSetRegistryFile = serde_json::from_str(&text).map_err(|error| {
            format!(
                "invalid host-set registry JSON ({}): {error}",
                path.display()
            )
        })?;
        let key = profile.as_str().to_string();
        let entry = registry.profiles.get(&key).ok_or_else(|| {
            format!(
                "host-set registry {} missing profile entry for '{}'",
                path.display(),
                profile.as_str()
            )
        })?;

        let host_set_id = entry.id.trim().to_string();
        if host_set_id.is_empty() {
            return Err(format!(
                "host-set registry {} profile '{}' has empty id",
                path.display(),
                profile.as_str()
            ));
        }

        let host_set_hash = if let Some(hash) = entry.hash.as_deref() {
            parse_sha256_hex(hash).map_err(|message| {
                format!(
                    "host-set registry {} profile '{}' invalid hash: {message}",
                    path.display(),
                    profile.as_str()
                )
            })?
        } else {
            contract_hash_from_id(&host_set_id)
        };

        return Ok(HostSetContract {
            host_set_id,
            host_set_hash,
        });
    }

    let host_set_id = format!("stasis-{}", profile.as_str());
    Ok(HostSetContract {
        host_set_id: host_set_id.clone(),
        host_set_hash: contract_hash_from_id(&host_set_id),
    })
}

fn parse_sha256_hex(value: &str) -> Result<[u8; 32], String> {
    let trimmed = value.trim().trim_start_matches("0x");
    if trimmed.len() != 64 {
        return Err(format!("expected 64 hex chars, got {}", trimmed.len()));
    }
    let mut out = [0u8; 32];
    for (i, chunk) in trimmed.as_bytes().chunks(2).enumerate() {
        let s = std::str::from_utf8(chunk).map_err(|_| "hash is not valid utf-8".to_string())?;
        out[i] = u8::from_str_radix(s, 16).map_err(|_| format!("invalid hex byte '{s}'"))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn default_contract_hash_is_deterministic() {
        let a = resolve_profile_contract(HostSetProfile::Dev, None).expect("resolve");
        let b = resolve_profile_contract(HostSetProfile::Dev, None).expect("resolve");
        assert_eq!(a, b);
        assert_eq!(a.host_set_id, "stasis-dev");
        assert_ne!(a.host_set_hash, [0u8; 32]);
    }

    #[test]
    fn registry_file_can_override_profile_id_and_hash() {
        let tmp = std::env::temp_dir().join("stasis_host_set_registry_test.json");
        fs::write(
            &tmp,
            r#"{
  "profiles": {
    "dev": { "id": "editor-host", "hash": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f" }
  }
}"#,
        )
        .expect("write tmp");

        let contract = resolve_profile_contract(HostSetProfile::Dev, Some(&tmp)).expect("resolve");
        assert_eq!(contract.host_set_id, "editor-host");
        assert_eq!(contract.host_set_hash[0], 0);
        assert_eq!(contract.host_set_hash[31], 31);
    }

    #[test]
    fn registry_file_missing_profile_is_error() {
        let tmp: PathBuf = std::env::temp_dir().join("stasis_host_set_registry_missing.json");
        fs::write(
            &tmp,
            r#"{
  "profiles": {
    "prod": { "id": "stasis-prod" }
  }
}"#,
        )
        .expect("write tmp");

        let error = resolve_profile_contract(HostSetProfile::Dev, Some(&tmp)).unwrap_err();
        assert!(error.contains("missing profile entry"));
    }
}
