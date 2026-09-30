use super::{CanonicalTarget, ProjectCapabilities};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use stasis_compiler::backend::program_snapshot::ProjectConfiguration;
use std::collections::{BTreeMap, BTreeSet};

pub(super) const CATALOG_SCHEMA: &str = "stasis.library_catalog.v1";
pub(super) const NETWORK_LIBRARY_ID: &str = "stasis.network";
pub(super) const NETWORK_CATALOG_RELEASE: &str = "stasis.network@1.0.0";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(deny_unknown_fields)]
pub(super) struct LibrariesManifest {
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) selections: BTreeMap<String, LibraryRequest>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) targets: BTreeMap<String, BTreeMap<String, LibraryRequest>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(super) struct LibraryRequest {
    #[serde(default = "enabled_by_default")]
    pub(super) enabled: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) features: Vec<String>,
}

fn enabled_by_default() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct ResolvedLibrarySet {
    pub(super) schema: &'static str,
    pub(super) catalog_version: &'static str,
    pub(super) catalog_sha256: String,
    pub(super) target: String,
    pub(super) library_set_sha256: String,
    pub(super) libraries: Vec<ResolvedLibrary>,
    pub(super) exclusions: Vec<LibraryExclusion>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct ResolvedLibrary {
    pub(super) id: String,
    pub(super) version: String,
    pub(super) selection: String,
    pub(super) features: Vec<String>,
    pub(super) capabilities: Vec<String>,
    pub(super) source_modules: Vec<AuthenticatedInput>,
    pub(super) artifact: CatalogArtifact,
    pub(super) dependencies: Vec<String>,
    pub(super) abi: String,
    pub(super) minimum_os: String,
    pub(super) toolchain: String,
    pub(super) license: AuthenticatedInput,
    pub(super) load_policy: String,
    pub(super) reasons: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct AuthenticatedInput {
    pub(super) path: String,
    pub(super) sha256: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct CatalogArtifact {
    pub(super) path: String,
    pub(super) kind: String,
    pub(super) authentication: String,
    pub(super) catalog_release: String,
    pub(super) source_set_sha256: String,
    pub(super) toolchain_sha256: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub(super) struct LibraryExclusion {
    pub(super) id: String,
    pub(super) reason: String,
}

#[derive(Debug, Clone)]
struct CatalogEntry {
    id: &'static str,
    version: &'static str,
    features: &'static [&'static str],
    dependencies: &'static [&'static str],
    conflicts: &'static [&'static str],
    abi: &'static str,
    load_policy: &'static str,
}

const NETWORK_ENTRY: CatalogEntry = CatalogEntry {
    id: NETWORK_LIBRARY_ID,
    version: "1.0.0",
    features: &["client", "host"],
    dependencies: &[],
    conflicts: &[],
    abi: "stasis.network.c.v1",
    load_policy: "normal-platform-dependency",
};

pub(super) fn validate_manifest(manifest: &LibrariesManifest) -> Result<(), String> {
    for target in manifest.targets.keys() {
        if CanonicalTarget::parse(target).is_none() {
            return Err(format!("unknown libraries target '{target}'"));
        }
    }
    for (scope, selections) in std::iter::once(("base", &manifest.selections)).chain(
        manifest
            .targets
            .iter()
            .map(|(target, selections)| (target.as_str(), selections)),
    ) {
        for (id, request) in selections {
            let entry = catalog_entry(id)
                .ok_or_else(|| format!("unknown library ID '{id}' in {scope} libraries"))?;
            let mut seen = BTreeSet::new();
            for feature in &request.features {
                if !entry.features.contains(&feature.as_str()) {
                    return Err(format!(
                        "unknown feature '{feature}' for library {id} in {scope} libraries"
                    ));
                }
                if !seen.insert(feature) {
                    return Err(format!(
                        "duplicate feature '{feature}' for library {id} in {scope} libraries"
                    ));
                }
            }
            if request.enabled && request.features.is_empty() {
                return Err(format!(
                    "enabled library {id} in {scope} libraries must select at least one feature"
                ));
            }
        }
    }
    Ok(())
}

pub(super) fn resolve(
    manifest_version: u32,
    manifest: Option<&LibrariesManifest>,
    capabilities: Option<&ProjectCapabilities>,
    target: CanonicalTarget,
) -> Result<ResolvedLibrarySet, String> {
    if let Some(manifest) = manifest {
        validate_manifest(manifest)?;
    }
    let capabilities = capabilities.cloned().unwrap_or_default();
    let (selection, request, reasons) = if manifest_version < 3 {
        if manifest.is_some() {
            return Err("included libraries require manifest_version 3".to_string());
        }
        let features = if capabilities.network {
            vec!["host".to_string()]
        } else if capabilities.network_client {
            vec!["client".to_string()]
        } else {
            Vec::new()
        };
        let request = (!features.is_empty()).then_some(LibraryRequest {
            enabled: true,
            features,
        });
        (
            "legacy-implicit",
            request,
            vec!["manifest-v1/v2 capability compatibility".to_string()],
        )
    } else {
        let manifest = manifest.cloned().unwrap_or_default();
        let base = manifest.selections.get(NETWORK_LIBRARY_ID).cloned();
        let override_request = manifest
            .targets
            .get(target.as_str())
            .and_then(|target| target.get(NETWORK_LIBRARY_ID))
            .cloned();
        let request = override_request.clone().or(base);
        let reasons = if override_request.is_some() {
            vec![format!("exact target override for {}", target.as_str())]
        } else if request.is_some() {
            vec!["base manifest selection".to_string()]
        } else {
            Vec::new()
        };
        if request.is_none() && (capabilities.network || capabilities.network_client) {
            return Err(format!(
                "manifest_version 3 network capabilities require an explicit {NETWORK_LIBRARY_ID} selection for {}",
                target.as_str()
            ));
        }
        ("explicit", request, reasons)
    };

    let mut libraries = Vec::new();
    let mut exclusions = Vec::new();
    match request {
        Some(request) if request.enabled => {
            let features = canonical_features(&request.features);
            validate_capability_grants(manifest_version, &features, &capabilities, target)?;
            libraries.push(resolve_network(target, selection, features, reasons)?);
        }
        Some(_) => exclusions.push(LibraryExclusion {
            id: NETWORK_LIBRARY_ID.to_string(),
            reason: format!("disabled for {}", target.as_str()),
        }),
        None => exclusions.push(LibraryExclusion {
            id: NETWORK_LIBRARY_ID.to_string(),
            reason: "not selected".to_string(),
        }),
    }
    libraries.sort_by(|left, right| left.id.cmp(&right.id));
    reject_catalog_conflicts(&libraries)?;
    let catalog_sha256 = catalog_digest();
    let library_set_sha256 = selection_digest(target, &libraries);
    Ok(ResolvedLibrarySet {
        schema: CATALOG_SCHEMA,
        catalog_version: "1",
        catalog_sha256,
        target: target.as_str().to_string(),
        library_set_sha256,
        libraries,
        exclusions,
    })
}

fn validate_capability_grants(
    manifest_version: u32,
    features: &[String],
    capabilities: &ProjectCapabilities,
    target: CanonicalTarget,
) -> Result<(), String> {
    if manifest_version < 3 {
        return Ok(());
    }
    for feature in features {
        let granted = match feature.as_str() {
            "host" => capabilities.network,
            "client" => capabilities.network_client,
            _ => false,
        };
        if !granted {
            return Err(format!(
                "library {NETWORK_LIBRARY_ID} feature '{feature}' requires capabilities.{} for {}",
                if feature == "host" {
                    "network"
                } else {
                    "network_client"
                },
                target.as_str()
            ));
        }
    }
    Ok(())
}

fn canonical_features(features: &[String]) -> Vec<String> {
    features
        .iter()
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn resolve_network(
    target: CanonicalTarget,
    selection: &str,
    features: Vec<String>,
    reasons: Vec<String>,
) -> Result<ResolvedLibrary, String> {
    let artifact = artifact_for(target).ok_or_else(|| {
        format!(
            "library {NETWORK_LIBRARY_ID} has no authenticated artifact for {}",
            target.as_str()
        )
    })?;
    let mut source_modules = Vec::new();
    if features.iter().any(|feature| feature == "host") {
        source_modules.push(authenticated_source(
            "crates/stasis_network/include/stasis_network.h",
            include_bytes!("../../../../crates/stasis_network/include/stasis_network.h"),
        ));
    }
    if features.iter().any(|feature| feature == "client") {
        source_modules.push(authenticated_source(
            "src/stdlib/network_client.stasis",
            include_bytes!("../../../../src/stdlib/network_client.stasis"),
        ));
    }
    source_modules.sort_by(|left, right| left.path.cmp(&right.path));
    let capabilities = features
        .iter()
        .map(|feature| match feature.as_str() {
            "host" => "network",
            "client" => "network_client",
            _ => unreachable!("features were validated against the catalog"),
        })
        .map(str::to_string)
        .collect();
    Ok(ResolvedLibrary {
        id: NETWORK_ENTRY.id.to_string(),
        version: NETWORK_ENTRY.version.to_string(),
        selection: selection.to_string(),
        features,
        capabilities,
        source_modules,
        artifact,
        dependencies: NETWORK_ENTRY
            .dependencies
            .iter()
            .map(|dependency| (*dependency).to_string())
            .collect(),
        abi: NETWORK_ENTRY.abi.to_string(),
        minimum_os: minimum_os(target).to_string(),
        toolchain: env!("CARGO_PKG_VERSION").to_string(),
        license: authenticated_source(
            "THIRD_PARTY_NOTICES.md",
            include_bytes!("../../../../THIRD_PARTY_NOTICES.md"),
        ),
        load_policy: NETWORK_ENTRY.load_policy.to_string(),
        reasons,
    })
}

fn artifact_for(target: CanonicalTarget) -> Option<CatalogArtifact> {
    let (path, kind, authentication) = match target {
        CanonicalTarget::Web => (
            "network_guest.bundle",
            "project-generated-wasm-js-bundle",
            "stasis_provenance.json#project_configuration.included_libraries",
        ),
        CanonicalTarget::WindowsX86_64 => (
            "desktop/network/windows-x86_64/stasis_network.dll",
            "pe-shared-library",
            "stasis_release_provenance.json#desktop_network_artifacts",
        ),
        CanonicalTarget::WindowsArm64 | CanonicalTarget::LinuxArm64 => return None,
        CanonicalTarget::LinuxX86_64 => (
            "desktop/network/linux-x86_64/libstasis_network.so",
            "elf-shared-library",
            "stasis_release_provenance.json#desktop_network_artifacts",
        ),
        CanonicalTarget::MacosX86_64 => (
            "desktop/network/macos-x86_64/libstasis_network.dylib",
            "mach-o-dylib",
            "catalog-release-source-set",
        ),
        CanonicalTarget::MacosArm64 => (
            "desktop/network/macos-arm64/libstasis_network.dylib",
            "mach-o-dylib",
            "catalog-release-source-set",
        ),
        CanonicalTarget::AndroidArm64 => (
            "mobile/network/android-arm64/libstasis_network_v1.so",
            "elf-shared-library",
            "stasis_release_provenance.json#mobile_network_artifacts",
        ),
        CanonicalTarget::AndroidX86_64 => (
            "mobile/network/android-x86_64/libstasis_network_v1.so",
            "elf-shared-library",
            "stasis_release_provenance.json#mobile_network_artifacts",
        ),
        CanonicalTarget::IosArm64 => (
            "mobile/network/ios-arm64/libstasis_network.a",
            "apple-static-archive",
            "catalog-release-source-set",
        ),
        CanonicalTarget::IosSimulatorArm64 => (
            "mobile/network/ios-simulator-arm64/libstasis_network.a",
            "apple-static-archive",
            "catalog-release-source-set",
        ),
    };
    Some(CatalogArtifact {
        path: path.to_string(),
        kind: kind.to_string(),
        authentication: authentication.to_string(),
        catalog_release: NETWORK_CATALOG_RELEASE.to_string(),
        source_set_sha256: source_set_digest(),
        toolchain_sha256: toolchain_digest(),
    })
}

pub(super) fn catalog_artifact(target: CanonicalTarget) -> Option<CatalogArtifact> {
    artifact_for(target)
}

pub(super) fn has_authenticated_artifact(target: CanonicalTarget) -> bool {
    artifact_for(target).is_some()
}

fn minimum_os(target: CanonicalTarget) -> &'static str {
    match target {
        CanonicalTarget::Web => "browser-wasm32",
        CanonicalTarget::WindowsX86_64 | CanonicalTarget::WindowsArm64 => "windows-10",
        CanonicalTarget::LinuxX86_64 | CanonicalTarget::LinuxArm64 => "glibc-2.31",
        CanonicalTarget::MacosX86_64 | CanonicalTarget::MacosArm64 => "macos-12",
        CanonicalTarget::AndroidArm64 | CanonicalTarget::AndroidX86_64 => {
            "android-api-26;ndk-r27;page-size-16384"
        }
        CanonicalTarget::IosArm64 | CanonicalTarget::IosSimulatorArm64 => "ios-15",
    }
}

fn authenticated_source(path: &str, bytes: &[u8]) -> AuthenticatedInput {
    AuthenticatedInput {
        path: path.to_string(),
        sha256: hex(Sha256::digest(bytes).into()),
    }
}

const NETWORK_SOURCE_INPUTS: &[(&str, &[u8])] = &[
    (
        "crates/stasis_network/Cargo.toml",
        include_bytes!("../../../../crates/stasis_network/Cargo.toml"),
    ),
    (
        "crates/stasis_network/include/stasis_network.h",
        include_bytes!("../../../../crates/stasis_network/include/stasis_network.h"),
    ),
    (
        "crates/stasis_network/src/client.rs",
        include_bytes!("../../../../crates/stasis_network/src/client.rs"),
    ),
    (
        "crates/stasis_network/src/lan.rs",
        include_bytes!("../../../../crates/stasis_network/src/lan.rs"),
    ),
    (
        "crates/stasis_network/src/lib.rs",
        include_bytes!("../../../../crates/stasis_network/src/lib.rs"),
    ),
    (
        "crates/stasis_network/src/realtime.rs",
        include_bytes!("../../../../crates/stasis_network/src/realtime.rs"),
    ),
    (
        "crates/stasis_network/src/supervision.rs",
        include_bytes!("../../../../crates/stasis_network/src/supervision.rs"),
    ),
];

const NETWORK_TOOLCHAIN_INPUTS: &[(&str, &[u8])] = &[
    ("Cargo.toml", include_bytes!("../../../../Cargo.toml")),
    ("Cargo.lock", include_bytes!("../../../../Cargo.lock")),
];

fn named_input_digest<'a>(inputs: impl IntoIterator<Item = (&'a str, &'a [u8])>) -> String {
    let mut digest = Sha256::new();
    digest.update(b"stasis.authenticated_source_set.v1\0");
    for (path, bytes) in inputs {
        digest.update((path.len() as u64).to_le_bytes());
        digest.update(path.as_bytes());
        digest.update((bytes.len() as u64).to_le_bytes());
        digest.update(bytes);
    }
    hex(digest.finalize().into())
}

fn source_set_digest() -> String {
    named_input_digest(NETWORK_SOURCE_INPUTS.iter().copied())
}

fn toolchain_digest() -> String {
    named_input_digest(NETWORK_TOOLCHAIN_INPUTS.iter().copied())
}

pub(super) fn authenticate_source_workspace(
    root: &std::path::Path,
    target: CanonicalTarget,
) -> Result<CatalogArtifact, String> {
    let artifact = artifact_for(target).ok_or_else(|| {
        format!(
            "library {NETWORK_LIBRARY_ID} has no authenticated artifact for {}",
            target.as_str()
        )
    })?;
    let read_inputs = |inputs: &[(&str, &[u8])]| -> Result<Vec<(String, Vec<u8>)>, String> {
        inputs
            .iter()
            .map(|(relative, _)| {
                std::fs::read(root.join(relative))
                    .map(|bytes| ((*relative).to_string(), bytes))
                    .map_err(|error| {
                        format!(
                            "failed to authenticate {NETWORK_LIBRARY_ID} source {}: {error}",
                            root.join(relative).display()
                        )
                    })
            })
            .collect()
    };
    let sources = read_inputs(NETWORK_SOURCE_INPUTS)?;
    let source_digest = named_input_digest(
        sources
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    );
    if source_digest != artifact.source_set_sha256 {
        return Err(format!(
            "{NETWORK_LIBRARY_ID} source set differs from authenticated catalog release {} for {}",
            artifact.catalog_release,
            target.as_str()
        ));
    }
    let toolchain = read_inputs(NETWORK_TOOLCHAIN_INPUTS)?;
    let actual_toolchain = named_input_digest(
        toolchain
            .iter()
            .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
    );
    if actual_toolchain != artifact.toolchain_sha256 {
        return Err(format!(
            "{NETWORK_LIBRARY_ID} toolchain inputs differ from authenticated catalog release {} for {}",
            artifact.catalog_release,
            target.as_str()
        ));
    }
    Ok(artifact)
}

fn catalog_entry(id: &str) -> Option<&'static CatalogEntry> {
    (id == NETWORK_ENTRY.id).then_some(&NETWORK_ENTRY)
}

fn reject_catalog_conflicts(libraries: &[ResolvedLibrary]) -> Result<(), String> {
    let selected = libraries
        .iter()
        .map(|library| library.id.as_str())
        .collect::<BTreeSet<_>>();
    for library in libraries {
        let entry = catalog_entry(&library.id).expect("resolved library came from catalog");
        if let Some(conflict) = entry
            .conflicts
            .iter()
            .find(|conflict| selected.contains(**conflict))
        {
            return Err(format!("library {} conflicts with {conflict}", library.id));
        }
    }
    Ok(())
}

fn catalog_digest() -> String {
    let targets = CanonicalTarget::ALL
        .iter()
        .map(|target| {
            (
                target.as_str(),
                json!({
                    "artifact": artifact_for(*target),
                    "minimum_os": minimum_os(*target),
                }),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let catalog = json!({
        "schema": CATALOG_SCHEMA,
        "version": 1,
        "entries": [{
            "id": NETWORK_ENTRY.id,
            "version": NETWORK_ENTRY.version,
            "features": NETWORK_ENTRY.features,
            "dependencies": NETWORK_ENTRY.dependencies,
            "conflicts": NETWORK_ENTRY.conflicts,
            "abi": NETWORK_ENTRY.abi,
            "load_policy": NETWORK_ENTRY.load_policy,
            "required_capabilities": {"host": "network", "client": "network_client"},
            "authenticated_inputs": {
                "host": authenticated_source(
                    "crates/stasis_network/include/stasis_network.h",
                    include_bytes!("../../../../crates/stasis_network/include/stasis_network.h"),
                ),
                "client": authenticated_source(
                    "src/stdlib/network_client.stasis",
                    include_bytes!("../../../../src/stdlib/network_client.stasis"),
                ),
                "license": authenticated_source(
                    "THIRD_PARTY_NOTICES.md",
                    include_bytes!("../../../../THIRD_PARTY_NOTICES.md"),
                ),
            },
            "toolchain": env!("CARGO_PKG_VERSION"),
            "targets": targets,
        }],
    });
    hex(Sha256::digest(canonical_json(&catalog).as_bytes()).into())
}

fn selection_digest(target: CanonicalTarget, libraries: &[ResolvedLibrary]) -> String {
    let value = json!({"target": target.as_str(), "libraries": libraries});
    hex(Sha256::digest(canonical_json(&value).as_bytes()).into())
}

fn canonical_json(value: &Value) -> String {
    serde_json::to_string(value).expect("catalog values serialize deterministically")
}

fn hex(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn decode_hex(value: &str) -> [u8; 32] {
    let mut out = [0; 32];
    for (index, byte) in out.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .expect("resolver emits lowercase SHA-256");
    }
    out
}

pub(super) fn apply_to_configuration(
    configuration: &mut ProjectConfiguration,
    resolved: &ResolvedLibrarySet,
) {
    configuration.libraries = resolved
        .libraries
        .iter()
        .map(|library| (library.id.clone(), library.features.clone()))
        .collect();
    configuration.libraries_digest = decode_hex(&resolved.library_set_sha256);
}

pub(super) fn generated_source(resolved: &ResolvedLibrarySet) -> String {
    let Some(network) = resolved
        .libraries
        .iter()
        .find(|library| library.id == NETWORK_LIBRARY_ID)
    else {
        return "function @inline project_library_stasis_network_available(): bool { return false; }\nfunction @inline project_library_stasis_network_feature_host(): bool { return false; }\nfunction @inline project_library_stasis_network_feature_client(): bool { return false; }\n".to_string();
    };
    format!(
        "function @inline project_library_stasis_network_available(): bool {{ return true; }}\nfunction @inline project_library_stasis_network_feature_host(): bool {{ return {}; }}\nfunction @inline project_library_stasis_network_feature_client(): bool {{ return {}; }}\n",
        network.features.iter().any(|feature| feature == "host"),
        network.features.iter().any(|feature| feature == "client"),
    )
}

pub(super) fn network_roles(resolved: &ResolvedLibrarySet) -> (bool, bool) {
    resolved
        .libraries
        .iter()
        .find(|library| library.id == NETWORK_LIBRARY_ID)
        .map(|library| {
            (
                library.features.iter().any(|feature| feature == "host"),
                library.features.iter().any(|feature| feature == "client"),
            )
        })
        .unwrap_or((false, false))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(host: bool, client: bool) -> ProjectCapabilities {
        ProjectCapabilities {
            network: host,
            network_client: client,
        }
    }

    fn manifest(features: &[&str]) -> LibrariesManifest {
        LibrariesManifest {
            selections: BTreeMap::from([(
                NETWORK_LIBRARY_ID.to_string(),
                LibraryRequest {
                    enabled: true,
                    features: features
                        .iter()
                        .map(|feature| (*feature).to_string())
                        .collect(),
                },
            )]),
            targets: BTreeMap::new(),
        }
    }

    #[test]
    fn exact_target_override_wins_and_digest_is_deterministic() {
        let mut fixture = manifest(&["host"]);
        fixture.targets.insert(
            "android-arm64".to_string(),
            BTreeMap::from([(
                NETWORK_LIBRARY_ID.to_string(),
                LibraryRequest {
                    enabled: true,
                    features: vec!["client".to_string()],
                },
            )]),
        );
        let first = resolve(
            3,
            Some(&fixture),
            Some(&capabilities(true, true)),
            CanonicalTarget::AndroidArm64,
        )
        .expect("resolve exact override");
        let second = resolve(
            3,
            Some(&fixture),
            Some(&capabilities(true, true)),
            CanonicalTarget::AndroidArm64,
        )
        .expect("resolve repeat");
        assert_eq!(first, second);
        assert_eq!(first.libraries[0].features, ["client"]);
        assert!(first.libraries[0].reasons[0].contains("exact target override"));
    }

    #[test]
    fn legacy_selection_preserves_conflict_and_role_mapping() {
        let host = resolve(
            2,
            None,
            Some(&capabilities(true, false)),
            CanonicalTarget::WindowsX86_64,
        )
        .expect("legacy host");
        assert_eq!(network_roles(&host), (true, false));
        let client = resolve(
            1,
            None,
            Some(&capabilities(false, true)),
            CanonicalTarget::AndroidArm64,
        )
        .expect("legacy client");
        assert_eq!(network_roles(&client), (false, true));
    }

    #[test]
    fn v3_allows_explicit_dual_role_with_both_capability_grants() {
        let resolved = resolve(
            3,
            Some(&manifest(&["host", "client"])),
            Some(&capabilities(true, true)),
            CanonicalTarget::LinuxX86_64,
        )
        .expect("dual role");
        assert_eq!(network_roles(&resolved), (true, true));
        assert_eq!(resolved.libraries[0].features, ["client", "host"]);
    }

    #[test]
    fn unknown_ids_features_targets_and_missing_grants_fail_closed() {
        let mut unknown_id = manifest(&["host"]);
        unknown_id.selections.insert(
            "vendor.foreign".to_string(),
            LibraryRequest {
                enabled: true,
                features: vec!["host".to_string()],
            },
        );
        assert!(validate_manifest(&unknown_id)
            .unwrap_err()
            .contains("unknown library ID"));
        let mut unknown_feature = manifest(&["server"]);
        assert!(validate_manifest(&unknown_feature)
            .unwrap_err()
            .contains("unknown feature"));
        unknown_feature = manifest(&["host"]);
        unknown_feature
            .targets
            .insert("android".to_string(), BTreeMap::new());
        assert!(validate_manifest(&unknown_feature)
            .unwrap_err()
            .contains("unknown libraries target"));
        assert!(resolve(
            3,
            Some(&manifest(&["host"])),
            Some(&capabilities(false, false)),
            CanonicalTarget::Web,
        )
        .unwrap_err()
        .contains("requires capabilities.network"));
    }

    #[test]
    fn v3_network_capabilities_require_a_complete_explicit_selection() {
        let absent = resolve(
            3,
            None,
            Some(&capabilities(true, false)),
            CanonicalTarget::Web,
        )
        .unwrap_err();
        assert!(absent.contains("require an explicit stasis.network selection for web"));

        let mut partial = LibrariesManifest::default();
        partial
            .targets
            .insert("web".to_string(), manifest(&["host"]).selections);
        let error = resolve(
            3,
            Some(&partial),
            Some(&capabilities(true, false)),
            CanonicalTarget::WindowsX86_64,
        )
        .unwrap_err();
        assert!(error.contains("require an explicit stasis.network selection for windows-x86_64"));

        let mut disabled = manifest(&[]);
        disabled
            .selections
            .get_mut(NETWORK_LIBRARY_ID)
            .unwrap()
            .enabled = false;
        let disabled = resolve(
            3,
            Some(&disabled),
            Some(&capabilities(true, false)),
            CanonicalTarget::Web,
        )
        .expect("explicitly disabled target remains unambiguous");
        assert!(disabled.libraries.is_empty());
    }

    #[test]
    fn catalog_authentication_matches_published_target_availability() {
        for target in [
            CanonicalTarget::WindowsX86_64,
            CanonicalTarget::LinuxX86_64,
            CanonicalTarget::AndroidArm64,
            CanonicalTarget::AndroidX86_64,
        ] {
            let artifact = artifact_for(target).expect("published target artifact");
            assert!(artifact.authentication.contains("provenance.json#"));
        }
        for target in [
            CanonicalTarget::MacosX86_64,
            CanonicalTarget::MacosArm64,
            CanonicalTarget::IosArm64,
            CanonicalTarget::IosSimulatorArm64,
        ] {
            let artifact = artifact_for(target).expect("catalog-authenticated target artifact");
            assert_eq!(artifact.authentication, "catalog-release-source-set");
            assert_eq!(artifact.catalog_release, NETWORK_CATALOG_RELEASE);
            assert_eq!(artifact.source_set_sha256.len(), 64);
            assert_eq!(artifact.toolchain_sha256.len(), 64);
        }
        for target in [CanonicalTarget::WindowsArm64, CanonicalTarget::LinuxArm64] {
            assert!(artifact_for(target).is_none());
            let error = resolve(
                3,
                Some(&manifest(&["host"])),
                Some(&capabilities(true, false)),
                target,
            )
            .unwrap_err();
            assert!(error.contains("has no authenticated artifact"));
        }
        assert_eq!(
            artifact_for(CanonicalTarget::Web)
                .expect("web package mapping")
                .authentication,
            "stasis_provenance.json#project_configuration.included_libraries"
        );
    }

    #[test]
    fn source_build_authentication_rejects_modified_network_sources() {
        let root = std::env::temp_dir().join(format!(
            "stasis-network-source-auth-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        for (path, bytes) in NETWORK_SOURCE_INPUTS
            .iter()
            .chain(NETWORK_TOOLCHAIN_INPUTS.iter())
        {
            let destination = root.join(path);
            std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
            std::fs::write(destination, bytes).unwrap();
        }
        let artifact = authenticate_source_workspace(&root, CanonicalTarget::MacosArm64)
            .expect("exact catalog source set authenticates");
        assert_eq!(artifact.catalog_release, NETWORK_CATALOG_RELEASE);
        std::fs::write(
            root.join("crates/stasis_network/src/lib.rs"),
            b"// modified after catalog publication\n",
        )
        .unwrap();
        let error = authenticate_source_workspace(&root, CanonicalTarget::IosArm64)
            .expect_err("modified source must fail before artifact build");
        assert!(error.contains("source set differs from authenticated catalog release"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
