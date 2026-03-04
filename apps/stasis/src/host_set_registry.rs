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
    pub extern_phase_classes: BTreeMap<String, HostExternPhaseClass>,
    pub budget_policy: HostSetBudgetPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostExternPhaseClass {
    TickSafe,
    CommitOnly,
    EffectQueued,
}

impl HostExternPhaseClass {
    pub fn as_str(self) -> &'static str {
        match self {
            HostExternPhaseClass::TickSafe => "tick_safe",
            HostExternPhaseClass::CommitOnly => "commit_only",
            HostExternPhaseClass::EffectQueued => "effect_queued",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "tick_safe" => Some(HostExternPhaseClass::TickSafe),
            "commit_only" => Some(HostExternPhaseClass::CommitOnly),
            "effect_queued" => Some(HostExternPhaseClass::EffectQueued),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSetBudgetPolicy {
    pub max_effect_calls_per_tick: u32,
    pub max_effect_bytes_per_tick: u32,
}

impl HostSetBudgetPolicy {
    fn default_for_profile(profile: HostSetProfile) -> Self {
        match profile {
            HostSetProfile::Dev => Self {
                max_effect_calls_per_tick: 10_000,
                max_effect_bytes_per_tick: 4 * 1024 * 1024,
            },
            HostSetProfile::Test => Self {
                max_effect_calls_per_tick: 2_000,
                max_effect_bytes_per_tick: 1024 * 1024,
            },
            HostSetProfile::Prod => Self {
                max_effect_calls_per_tick: 1_000,
                max_effect_bytes_per_tick: 512 * 1024,
            },
        }
    }
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
    #[serde(default)]
    phase_classes: BTreeMap<String, String>,
    #[serde(default)]
    budgets: Option<HostSetRegistryBudgetEntry>,
}

#[derive(Debug, Clone, Deserialize)]
struct HostSetRegistryBudgetEntry {
    #[serde(default)]
    max_effect_calls_per_tick: Option<u32>,
    #[serde(default)]
    max_effect_bytes_per_tick: Option<u32>,
}

fn contract_hash_from_contract(
    host_set_id: &str,
    extern_phase_classes: &BTreeMap<String, HostExternPhaseClass>,
    budget_policy: HostSetBudgetPolicy,
) -> [u8; 32] {
    let mut input = format!("stasis.host_set_contract.v2:{host_set_id}|");
    for (symbol, phase_class) in extern_phase_classes {
        input.push_str(symbol);
        input.push('=');
        input.push_str(phase_class.as_str());
        input.push(';');
    }
    input.push_str(&format!(
        "max_effect_calls_per_tick={};max_effect_bytes_per_tick={};",
        budget_policy.max_effect_calls_per_tick, budget_policy.max_effect_bytes_per_tick
    ));
    let digest: [u8; 32] = Sha256::digest(input.as_bytes()).into();
    digest
}

fn default_extern_phase_classes() -> BTreeMap<String, HostExternPhaseClass> {
    let mut map = BTreeMap::new();
    map.insert("print_i32".to_string(), HostExternPhaseClass::EffectQueued);
    map.insert(
        "stasis_jit_print_i32".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "print_string".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "stasis_jit_print_string".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "audio_push_f32_interleaved".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "stasis_audio_push_f32_interleaved".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "gfx_dump_bmp".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "stasis_gfx_dump_bmp".to_string(),
        HostExternPhaseClass::EffectQueued,
    );
    map.insert(
        "gfx_load_sprite".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_gfx_load_sprite".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("load_font".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_load_font".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("measure_text".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_measure_text".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("gfx_cache_text".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_gfx_cache_text".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "gfx_poll_reload".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_gfx_poll_reload".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "gfx_measure_text_cached".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_gfx_measure_text_cached".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("sleep_ms".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_jit_sleep_ms".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "audio_is_available".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_audio_is_available".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("audio_init".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_jit_audio_init".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert("audio_shutdown".to_string(), HostExternPhaseClass::TickSafe);
    map.insert(
        "stasis_jit_audio_shutdown".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "audio_get_sample_rate".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_audio_get_sample_rate".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "audio_get_channels".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_audio_get_channels".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "audio_get_queued_frames".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_audio_get_queued_frames".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "audio_get_underruns".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map.insert(
        "stasis_jit_audio_get_underruns".to_string(),
        HostExternPhaseClass::TickSafe,
    );
    map
}

fn parse_phase_class_overrides(
    profile: HostSetProfile,
    path: &Path,
    overrides: &BTreeMap<String, String>,
) -> Result<BTreeMap<String, HostExternPhaseClass>, String> {
    let mut classes = default_extern_phase_classes();
    for (raw_symbol, raw_phase_class) in overrides {
        let symbol = raw_symbol.trim().to_string();
        if symbol.is_empty() {
            return Err(format!(
                "host-set registry {} profile '{}' has empty phase class symbol",
                path.display(),
                profile.as_str()
            ));
        }
        let Some(phase_class) = HostExternPhaseClass::parse(raw_phase_class) else {
            return Err(format!(
                "host-set registry {} profile '{}' has invalid phase class '{}' for symbol '{}' (expected tick_safe|commit_only|effect_queued)",
                path.display(),
                profile.as_str(),
                raw_phase_class,
                symbol
            ));
        };
        classes.insert(symbol, phase_class);
    }
    Ok(classes)
}

fn resolve_budget_policy(
    profile: HostSetProfile,
    path: &Path,
    override_entry: Option<&HostSetRegistryBudgetEntry>,
) -> Result<HostSetBudgetPolicy, String> {
    let mut budget = HostSetBudgetPolicy::default_for_profile(profile);
    if let Some(entry) = override_entry {
        if let Some(value) = entry.max_effect_calls_per_tick {
            if value == 0 {
                return Err(format!(
                    "host-set registry {} profile '{}' max_effect_calls_per_tick must be > 0",
                    path.display(),
                    profile.as_str()
                ));
            }
            budget.max_effect_calls_per_tick = value;
        }
        if let Some(value) = entry.max_effect_bytes_per_tick {
            if value == 0 {
                return Err(format!(
                    "host-set registry {} profile '{}' max_effect_bytes_per_tick must be > 0",
                    path.display(),
                    profile.as_str()
                ));
            }
            budget.max_effect_bytes_per_tick = value;
        }
    }
    Ok(budget)
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

        let extern_phase_classes =
            parse_phase_class_overrides(profile, path, &entry.phase_classes)?;
        let budget_policy = resolve_budget_policy(profile, path, entry.budgets.as_ref())?;

        let host_set_hash = if let Some(hash) = entry.hash.as_deref() {
            parse_sha256_hex(hash).map_err(|message| {
                format!(
                    "host-set registry {} profile '{}' invalid hash: {message}",
                    path.display(),
                    profile.as_str()
                )
            })?
        } else {
            contract_hash_from_contract(&host_set_id, &extern_phase_classes, budget_policy)
        };

        return Ok(HostSetContract {
            host_set_id,
            host_set_hash,
            extern_phase_classes,
            budget_policy,
        });
    }

    let host_set_id = format!("stasis-{}", profile.as_str());
    let extern_phase_classes = default_extern_phase_classes();
    let budget_policy = HostSetBudgetPolicy::default_for_profile(profile);
    Ok(HostSetContract {
        host_set_id: host_set_id.clone(),
        host_set_hash: contract_hash_from_contract(
            &host_set_id,
            &extern_phase_classes,
            budget_policy,
        ),
        extern_phase_classes,
        budget_policy,
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
        assert_eq!(
            a.extern_phase_classes.get("print_i32"),
            Some(&HostExternPhaseClass::EffectQueued)
        );
        assert!(a.budget_policy.max_effect_calls_per_tick > 0);
        assert!(a.budget_policy.max_effect_bytes_per_tick > 0);
    }

    #[test]
    fn registry_file_can_override_profile_id_and_hash() {
        let tmp = std::env::temp_dir().join("stasis_host_set_registry_test.json");
        fs::write(
            &tmp,
            r#"{
  "profiles": {
    "dev": {
      "id": "editor-host",
      "hash": "000102030405060708090a0b0c0d0e0f101112131415161718191a1b1c1d1e1f",
      "phase_classes": { "print_i32": "commit_only" },
      "budgets": { "max_effect_calls_per_tick": 77, "max_effect_bytes_per_tick": 8888 }
    }
  }
}"#,
        )
        .expect("write tmp");

        let contract = resolve_profile_contract(HostSetProfile::Dev, Some(&tmp)).expect("resolve");
        assert_eq!(contract.host_set_id, "editor-host");
        assert_eq!(contract.host_set_hash[0], 0);
        assert_eq!(contract.host_set_hash[31], 31);
        assert_eq!(
            contract.extern_phase_classes.get("print_i32"),
            Some(&HostExternPhaseClass::CommitOnly)
        );
        assert_eq!(contract.budget_policy.max_effect_calls_per_tick, 77);
        assert_eq!(contract.budget_policy.max_effect_bytes_per_tick, 8888);
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

    #[test]
    fn registry_file_invalid_phase_class_is_error() {
        let tmp = std::env::temp_dir().join("stasis_host_set_registry_invalid_phase.json");
        fs::write(
            &tmp,
            r#"{
  "profiles": {
    "dev": {
      "id": "editor-host",
      "phase_classes": { "print_i32": "unknown_phase" }
    }
  }
}"#,
        )
        .expect("write tmp");

        let error = resolve_profile_contract(HostSetProfile::Dev, Some(&tmp)).unwrap_err();
        assert!(error.contains("invalid phase class"));
    }
}
