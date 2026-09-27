use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsString;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use flate2::write::DeflateEncoder;
use flate2::Compression;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use stasis_assets::{
    load_project_asset_manifest, resolve_project_asset_paths, AssetLimits, AssetPackageIdentity,
    ResolvedAssetManifest, ASSET_PACKAGE_IDENTITY_PATH, DEFAULT_ASSET_MANIFEST_PATH,
};
use stasis_compiler::backend::assets::{
    validate_asset_references, AssetDiagnostic, AssetValidationResult,
};
use stasis_compiler::backend::program_snapshot::ProgramSnapshot;
use stasis_compiler::backend::text_coverage::TextCoverageProof;

pub(crate) const ASSET_DIAGNOSTIC_PREFIX: &str = "stasis_asset_diagnostics:";

const FONT_SUBSET_REPORT_PATH: &str = "stasis_font_subsets.json";
const FONT_SUBSET_REPORT_SCHEMA: &str = "stasis.font_subsets";
const FONT_SUBSET_REPORT_VERSION: u32 = 1;
const FONTTOOLS_PACKAGE: &str = "fonttools==4.60.2";
const FONT_SUBSET_OPTIONS: &str = concat!(
    "fonttools-subset-v1;hinting=1;glyph_names=1;notdef_glyph=1;",
    "notdef_outline=1;recommended_glyphs=1;layout_features=*;layout_scripts=*;",
    "name_IDs=*;name_languages=*;legacy_cmap=1;preserve_gids=0;canonical_order=1;",
    "recalcTimestamp=0"
);
const FONT_SUBSET_DRIVER: &str = include_str!("../../../tools/font-subsetting/subset_font.py");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseFontSubsettingManifest {
    #[serde(default)]
    pub(crate) fonts: Vec<ReleaseFontSubsetEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReleaseFontSubsetEntry {
    pub(crate) path: String,
    pub(crate) license_path: String,
    pub(crate) modification_permitted: bool,
    pub(crate) reserved_names: Vec<String>,
    #[serde(default)]
    pub(crate) replacement_family: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FontSubsettingReport {
    pub(crate) schema: String,
    pub(crate) version: u32,
    pub(crate) enabled: bool,
    pub(crate) tool: Option<String>,
    pub(crate) options: String,
    pub(crate) fonts: Vec<FontSubsetDecision>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FontSubsetDecision {
    pub(crate) path: String,
    pub(crate) decision: String,
    pub(crate) reason: Option<String>,
    pub(crate) codepoints: Vec<u32>,
    pub(crate) unknown_reason_codes: Vec<String>,
    pub(crate) source_sha256: Option<String>,
    pub(crate) prepared_sha256: Option<String>,
    pub(crate) source_size: Option<FontSubsetSizeMetrics>,
    pub(crate) prepared_size: Option<FontSubsetSizeMetrics>,
    pub(crate) license_path: String,
    pub(crate) license_sha256: Option<String>,
    pub(crate) license_notice_staged_path: Option<String>,
    pub(crate) modification_permitted: bool,
    pub(crate) reserved_names: Vec<String>,
    pub(crate) replacement_family: Option<String>,
    pub(crate) cache_state: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub(crate) struct FontSubsetSizeMetrics {
    pub(crate) raw_bytes: u64,
    pub(crate) deflate_bytes: u64,
}

#[derive(Debug, Clone)]
struct PythonTool {
    executable: OsString,
    prefix_args: Vec<OsString>,
    identity: String,
}

#[derive(Debug, Serialize)]
struct FontSubsetCacheMaterial<'a> {
    source_sha256: &'a str,
    codepoints: &'a [u32],
    tool: &'a str,
    options: &'a str,
    driver_sha256: &'a str,
    legal: FontSubsetCacheLegalMaterial<'a>,
}

#[derive(Debug, Serialize)]
struct FontSubsetCacheLegalMaterial<'a> {
    license_path: &'a str,
    license_sha256: &'a str,
    modification_permitted: bool,
    reserved_names: &'a [String],
    replacement_family: &'a Option<String>,
}

pub(crate) fn validate_font_subsetting_manifest(
    project_root: Option<&Path>,
    config: &ReleaseFontSubsettingManifest,
) -> Result<(), String> {
    if config.fonts.is_empty() {
        return Err("release.font_subsetting.fonts must contain at least one font".to_string());
    }

    let mut paths = BTreeSet::new();
    for (index, font) in config.fonts.iter().enumerate() {
        validate_font_asset_path("path", &font.path)?;
        validate_project_relative_path("license_path", &font.license_path)?;
        if !font.license_path.starts_with("assets/") {
            return Err(
                "release.font_subsetting.license_path must point under assets/".to_string(),
            );
        }
        if !font.license_path.to_ascii_lowercase().ends_with(".txt") {
            return Err(format!(
                "release.font_subsetting.fonts[{index}].license_path must name a .txt license notice"
            ));
        }
        if !paths.insert(font.path.as_str()) {
            return Err(format!(
                "release.font_subsetting.fonts contains duplicate font path {}",
                font.path
            ));
        }
        if !font.modification_permitted {
            return Err(format!(
                "release.font_subsetting.fonts[{index}].modification_permitted must be true"
            ));
        }
        let mut names = BTreeSet::new();
        for name in &font.reserved_names {
            if !valid_font_name(name) {
                return Err(format!(
                    "release.font_subsetting.fonts[{index}].reserved_names must contain non-empty ASCII names"
                ));
            }
            if !names.insert(name.to_ascii_lowercase()) {
                return Err(format!(
                    "release.font_subsetting.fonts[{index}].reserved_names contains duplicate name {name}"
                ));
            }
        }
        if !font.reserved_names.is_empty() && font.replacement_family.is_none() {
            return Err(format!(
                "release.font_subsetting.fonts[{index}] must set replacement_family when reserved_names are declared"
            ));
        }
        if let Some(family) = &font.replacement_family {
            if !valid_font_name(family) {
                return Err(format!(
                    "release.font_subsetting.fonts[{index}].replacement_family must be a non-empty ASCII family name"
                ));
            }
            let family_key = normalized_name_key(family);
            if font.reserved_names.iter().any(|name| {
                !normalized_name_key(name).is_empty()
                    && family_key.contains(&normalized_name_key(name))
            }) {
                return Err(format!(
                    "release.font_subsetting.fonts[{index}].replacement_family must not contain a reserved name"
                ));
            }
        }
    }

    if let Some(project_root) = project_root {
        let canonical_root = project_root.canonicalize().map_err(|error| {
            format!(
                "failed to resolve font subsetting project root {}: {error}",
                project_root.display()
            )
        })?;
        let assets_root = canonical_root
            .join("assets")
            .canonicalize()
            .map_err(|error| {
                format!("failed to resolve project assets for font subsetting: {error}")
            })?;
        for font in &config.fonts {
            let font_path = canonical_project_file(&canonical_root, &font.path, "font path")?;
            if !font_path.starts_with(&assets_root) {
                return Err(format!(
                    "font subsetting path resolves outside project assets: {}",
                    font.path
                ));
            }
            let license_path = canonical_root.join(&font.license_path);
            let resolved_license = license_path.canonicalize().map_err(|error| {
                format!(
                    "failed to resolve font subsetting license_path {}: {error}",
                    font.license_path
                )
            })?;
            if !resolved_license.starts_with(&assets_root) || !resolved_license.is_file() {
                return Err(format!(
                    "font subsetting license_path must resolve to a file under assets/: {}",
                    font.license_path
                ));
            }
        }
    }
    Ok(())
}

#[allow(dead_code)]
pub(crate) fn load_release_font_subsetting_manifest(
    project_root: &Path,
) -> Result<Option<ReleaseFontSubsettingManifest>, String> {
    let manifest_path = project_root.join("stasis.json");
    let manifest_bytes = std::fs::read(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("failed to decode {}: {error}", manifest_path.display()))?;
    let Some(value) = manifest
        .get("release")
        .and_then(|release| release.get("font_subsetting"))
        .cloned()
    else {
        return Ok(None);
    };
    let config = serde_json::from_value(value)
        .map_err(|error| format!("invalid release.font_subsetting: {error}"))?;
    validate_font_subsetting_manifest(Some(project_root), &config)?;
    Ok(Some(config))
}

fn validate_font_asset_path(field: &str, value: &str) -> Result<(), String> {
    validate_project_relative_path(field, value)?;
    if !value.starts_with("assets/") {
        return Err(format!(
            "release.font_subsetting.{field} must point under assets/"
        ));
    }
    if !value.to_ascii_lowercase().ends_with(".ttf") {
        return Err(format!(
            "release.font_subsetting.{field} must name a TrueType .ttf file"
        ));
    }
    Ok(())
}

fn validate_project_relative_path(field: &str, value: &str) -> Result<(), String> {
    if value.is_empty()
        || !value.is_ascii()
        || value.starts_with('/')
        || value.contains('\\')
        || value.contains(':')
        || value
            .split('/')
            .any(|component| component.is_empty() || matches!(component, "." | ".."))
    {
        return Err(format!(
            "release.font_subsetting.{field} must be a normalized ASCII project-relative path"
        ));
    }
    Ok(())
}

fn valid_font_name(value: &str) -> bool {
    !value.is_empty()
        && value.is_ascii()
        && value.trim() == value
        && value.bytes().all(|byte| !byte.is_ascii_control())
}

fn normalized_name_key(value: &str) -> String {
    value
        .bytes()
        .filter(|byte| byte.is_ascii_alphanumeric())
        .map(|byte| byte.to_ascii_lowercase() as char)
        .collect()
}

fn canonical_project_file(root: &Path, relative: &str, field: &str) -> Result<PathBuf, String> {
    let path = root.join(relative).canonicalize().map_err(|error| {
        format!("failed to resolve font subsetting {field} {relative}: {error}")
    })?;
    if !path.starts_with(root) || !path.is_file() {
        return Err(format!(
            "font subsetting {field} must name an existing file under the project: {relative}"
        ));
    }
    Ok(path)
}

pub(crate) fn apply_release_font_subsetting(
    project_root: &Path,
    staging_root: &Path,
    cache_root: &Path,
    proof: &TextCoverageProof,
    config: Option<&ReleaseFontSubsettingManifest>,
    shell_title: Option<&str>,
    shell_font_path: Option<&str>,
) -> Result<FontSubsettingReport, String> {
    let Some(config) = config else {
        return Ok(empty_font_subsetting_report());
    };
    let tool = if matches!(proof, TextCoverageProof::Finite { .. }) {
        discover_fonttools()
    } else {
        None
    };
    apply_release_font_subsetting_with_tool(
        project_root,
        staging_root,
        cache_root,
        proof,
        config,
        shell_title,
        shell_font_path,
        tool,
    )
}

fn apply_release_font_subsetting_with_tool(
    project_root: &Path,
    staging_root: &Path,
    cache_root: &Path,
    proof: &TextCoverageProof,
    config: &ReleaseFontSubsettingManifest,
    shell_title: Option<&str>,
    shell_font_path: Option<&str>,
    tool: Option<PythonTool>,
) -> Result<FontSubsettingReport, String> {
    validate_font_subsetting_manifest(Some(project_root), config)?;
    let shell_font_path = shell_font_path.and_then(normalize_proof_font_path);
    let driver_sha256 = font_subset_driver_sha256();
    let mut fonts = config.fonts.iter().collect::<Vec<_>>();
    fonts.sort_by(|left, right| left.path.cmp(&right.path));
    let mut report = FontSubsettingReport {
        schema: FONT_SUBSET_REPORT_SCHEMA.to_string(),
        version: FONT_SUBSET_REPORT_VERSION,
        enabled: true,
        tool: tool.as_ref().map(|tool| tool.identity.clone()),
        options: format!("{FONT_SUBSET_OPTIONS};driver_sha256={driver_sha256}"),
        fonts: Vec::with_capacity(fonts.len()),
    };
    let mut replacements = Vec::<(String, Vec<u8>, String)>::new();
    let mut license_notices = BTreeMap::<PathBuf, Vec<u8>>::new();
    let mut license_notice_paths = BTreeMap::<String, String>::new();

    for font in fonts {
        let staged_font_path = staging_root.join(&font.path);
        let source_bytes = match std::fs::read(&staged_font_path) {
            Ok(bytes) => bytes,
            Err(_) => {
                report.fonts.push(retained_font_decision(
                    font,
                    "staged_font_missing",
                    Vec::new(),
                    None,
                    None,
                    None,
                    "not_used",
                ));
                continue;
            }
        };
        let source_sha256 = stasis_assets::sha256_bytes(&source_bytes);
        let source_size = font_size_metrics(&source_bytes).ok();
        let legal_notice = std::fs::read(project_root.join(&font.license_path)).ok();
        let license_sha256 = legal_notice.as_deref().map(stasis_assets::sha256_bytes);
        if let Some(license_bytes) = legal_notice
            .as_ref()
            .filter(|bytes| std::str::from_utf8(bytes).is_ok())
        {
            let license_sha256 = stasis_assets::sha256_bytes(license_bytes);
            let license_output =
                PathBuf::from("font_licenses").join(format!("{license_sha256}.txt"));
            license_notices
                .entry(license_output.clone())
                .or_insert_with(|| license_bytes.clone());
            license_notice_paths.insert(
                font.path.clone(),
                license_output.to_string_lossy().replace('\\', "/"),
            );
        }

        let TextCoverageProof::Finite {
            unicode_scalars,
            fonts: proof_fonts,
            ..
        } = proof
        else {
            let mut decision = retained_font_decision(
                font,
                "coverage_unknown",
                Vec::new(),
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            );
            if let TextCoverageProof::Unknown { reasons, .. } = proof {
                decision.unknown_reason_codes = reasons
                    .iter()
                    .map(|reason| reason.code.clone())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect();
            }
            report.fonts.push(decision);
            continue;
        };

        let shell_font_matches = shell_font_path.as_deref() == Some(font.path.as_str());
        let proof_evidence_matches = proof_fonts.iter().any(|evidence| {
            normalize_proof_font_path(&evidence.path).as_deref() == Some(font.path.as_str())
        });
        if !proof_evidence_matches && !shell_font_matches {
            report.fonts.push(retained_font_decision(
                font,
                "not_evidenced",
                Vec::new(),
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            ));
            continue;
        }

        let mut codepoints = unicode_scalars.iter().copied().collect::<BTreeSet<_>>();
        if codepoints
            .iter()
            .any(|scalar| char::from_u32(*scalar).is_none())
        {
            report.fonts.push(retained_font_decision(
                font,
                "invalid_codepoint",
                codepoints.into_iter().collect(),
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            ));
            continue;
        }
        if shell_font_matches {
            if let Some(title) = shell_title {
                codepoints.extend(title.chars().map(u32::from));
                codepoints.extend(title.chars().flat_map(char::to_uppercase).map(u32::from));
            }
        }
        let codepoints = codepoints.into_iter().collect::<Vec<_>>();
        if codepoints.is_empty() {
            report.fonts.push(retained_font_decision(
                font,
                "empty_coverage",
                codepoints,
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            ));
            continue;
        }

        let Some(license_bytes) = legal_notice else {
            report.fonts.push(retained_font_decision(
                font,
                "license_unavailable",
                codepoints,
                Some(&source_bytes),
                source_size,
                None,
                "not_used",
            ));
            continue;
        };
        if std::str::from_utf8(&license_bytes).is_err() {
            report.fonts.push(retained_font_decision(
                font,
                "license_invalid",
                codepoints,
                Some(&source_bytes),
                source_size,
                Some(stasis_assets::sha256_bytes(&license_bytes)),
                "not_used",
            ));
            continue;
        }
        let Some(tool) = tool.as_ref() else {
            report.fonts.push(retained_font_decision(
                font,
                "exact_tool_unavailable",
                codepoints,
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            ));
            continue;
        };
        if let Some(reason) = staged_font_manifest_reason(staging_root, &font.path, &source_sha256)
        {
            report.fonts.push(retained_font_decision(
                font,
                &reason,
                codepoints,
                Some(&source_bytes),
                source_size,
                license_sha256,
                "not_used",
            ));
            continue;
        }
        let license_sha256 = stasis_assets::sha256_bytes(&license_bytes);
        let reserved_names = {
            let mut names = font.reserved_names.clone();
            names.sort();
            names
        };
        let material = FontSubsetCacheMaterial {
            source_sha256: &source_sha256,
            codepoints: &codepoints,
            tool: &tool.identity,
            options: FONT_SUBSET_OPTIONS,
            driver_sha256: &driver_sha256,
            legal: FontSubsetCacheLegalMaterial {
                license_path: &font.license_path,
                license_sha256: &license_sha256,
                modification_permitted: font.modification_permitted,
                reserved_names: &reserved_names,
                replacement_family: &font.replacement_family,
            },
        };
        let material_bytes = serde_json::to_vec(&material)
            .map_err(|error| format!("failed to encode font subset cache key: {error}"))?;
        let cache_key = format!("{:x}", Sha256::digest(&material_bytes));
        let font_cache = cache_root.join("release-font-subsetting");
        if let Err(error) = std::fs::create_dir_all(&font_cache) {
            report.fonts.push(retained_font_decision(
                font,
                "cache_unavailable",
                codepoints,
                Some(&source_bytes),
                source_size,
                Some(license_sha256),
                "not_used",
            ));
            let _ = error;
            continue;
        }
        let cache_path = font_cache.join(format!("{cache_key}.ttf"));
        let mut cache_state = "miss";
        let mut prepared_bytes = None;
        if let Ok(cached) = std::fs::read(&cache_path) {
            match run_fonttools_driver(
                tool,
                &FontSubsetDriverRequest::validate(
                    &staged_font_path,
                    &cache_path,
                    &codepoints,
                    &reserved_names,
                    font.replacement_family.as_deref(),
                ),
            ) {
                Ok(()) => {
                    prepared_bytes = Some(cached);
                    cache_state = "hit";
                }
                Err(_) => cache_state = "rebuilt",
            }
        }

        if prepared_bytes.is_none() {
            let temporary_dir = font_cache.join(format!(
                ".{cache_key}.tmp-{}-{}",
                std::process::id(),
                NEXT_FONT_SUBSET_TEMP.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            if std::fs::create_dir(&temporary_dir).is_err() {
                report.fonts.push(retained_font_decision(
                    font,
                    "temporary_output_unavailable",
                    codepoints,
                    Some(&source_bytes),
                    source_size,
                    Some(license_sha256),
                    "not_used",
                ));
                continue;
            }
            let temporary_output = temporary_dir.join("subset.ttf");
            let transform = run_fonttools_driver(
                tool,
                &FontSubsetDriverRequest::subset(
                    &staged_font_path,
                    &temporary_output,
                    &codepoints,
                    &reserved_names,
                    font.replacement_family.as_deref(),
                ),
            );
            if let Err(reason) = transform {
                let _ = std::fs::remove_dir_all(&temporary_dir);
                report.fonts.push(retained_font_decision(
                    font,
                    &reason,
                    codepoints,
                    Some(&source_bytes),
                    source_size,
                    Some(license_sha256),
                    "miss",
                ));
                continue;
            }
            match std::fs::read(&temporary_output) {
                Ok(bytes) => prepared_bytes = Some(bytes),
                Err(_) => {
                    let _ = std::fs::remove_dir_all(&temporary_dir);
                    report.fonts.push(retained_font_decision(
                        font,
                        "transform_output_missing",
                        codepoints,
                        Some(&source_bytes),
                        source_size,
                        Some(license_sha256),
                        "miss",
                    ));
                    continue;
                }
            }
            let _ = std::fs::remove_dir_all(&temporary_dir);
        }

        let Some(prepared_bytes) = prepared_bytes else {
            report.fonts.push(retained_font_decision(
                font,
                "transform_output_missing",
                codepoints,
                Some(&source_bytes),
                source_size,
                Some(license_sha256),
                cache_state,
            ));
            continue;
        };
        let prepared_size = match font_size_metrics(&prepared_bytes) {
            Ok(metrics) => metrics,
            Err(_) => {
                report.fonts.push(retained_font_decision(
                    font,
                    "size_measurement_failed",
                    codepoints,
                    Some(&source_bytes),
                    source_size,
                    Some(license_sha256),
                    cache_state,
                ));
                continue;
            }
        };
        if source_size.is_none_or(|source| {
            prepared_size.raw_bytes >= source.raw_bytes
                || prepared_size.deflate_bytes >= source.deflate_bytes
        }) {
            report.fonts.push(retained_font_decision(
                font,
                "no_net_savings",
                codepoints,
                Some(&source_bytes),
                source_size,
                Some(license_sha256),
                cache_state,
            ));
            continue;
        }
        if cache_state != "hit" && atomic_write_bytes(&cache_path, &prepared_bytes).is_err() {
            report.fonts.push(retained_font_decision(
                font,
                "cache_write_failed",
                codepoints,
                Some(&source_bytes),
                source_size,
                Some(license_sha256),
                cache_state,
            ));
            continue;
        }

        replacements.push((
            font.path.clone(),
            prepared_bytes.clone(),
            source_sha256.clone(),
        ));
        report.fonts.push(FontSubsetDecision {
            path: font.path.clone(),
            decision: "subset".to_string(),
            reason: Some("finite_coverage".to_string()),
            codepoints,
            unknown_reason_codes: Vec::new(),
            source_sha256: Some(source_sha256),
            prepared_sha256: Some(stasis_assets::sha256_bytes(&prepared_bytes)),
            source_size,
            prepared_size: Some(prepared_size),
            license_path: font.license_path.clone(),
            license_sha256: Some(license_sha256),
            license_notice_staged_path: None,
            modification_permitted: font.modification_permitted,
            reserved_names,
            replacement_family: font.replacement_family.clone(),
            cache_state: cache_state.to_string(),
        });
    }

    for decision in &mut report.fonts {
        decision.license_notice_staged_path = license_notice_paths.get(&decision.path).cloned();
    }
    let license_notices = license_notices.into_iter().collect::<Vec<_>>();
    publish_font_subset_outputs(staging_root, &replacements, &license_notices, &mut report)?;
    Ok(report)
}

fn empty_font_subsetting_report() -> FontSubsettingReport {
    FontSubsettingReport {
        schema: FONT_SUBSET_REPORT_SCHEMA.to_string(),
        version: FONT_SUBSET_REPORT_VERSION,
        enabled: false,
        tool: None,
        options: format!(
            "{FONT_SUBSET_OPTIONS};driver_sha256={}",
            font_subset_driver_sha256()
        ),
        fonts: Vec::new(),
    }
}

fn font_subset_driver_sha256() -> String {
    let normalized = FONT_SUBSET_DRIVER.replace("\r\n", "\n").replace('\r', "\n");
    format!("{:x}", Sha256::digest(normalized.as_bytes()))
}

fn retained_font_decision(
    font: &ReleaseFontSubsetEntry,
    reason: &str,
    codepoints: Vec<u32>,
    original: Option<&[u8]>,
    sizes: Option<FontSubsetSizeMetrics>,
    license_sha256: Option<String>,
    cache_state: &str,
) -> FontSubsetDecision {
    let sha256 = original.map(stasis_assets::sha256_bytes);
    FontSubsetDecision {
        path: font.path.clone(),
        decision: "retained".to_string(),
        reason: Some(reason.to_string()),
        codepoints,
        unknown_reason_codes: Vec::new(),
        source_sha256: sha256.clone(),
        prepared_sha256: sha256,
        source_size: sizes,
        prepared_size: sizes,
        license_path: font.license_path.clone(),
        license_sha256,
        license_notice_staged_path: None,
        modification_permitted: font.modification_permitted,
        reserved_names: font.reserved_names.clone(),
        replacement_family: font.replacement_family.clone(),
        cache_state: cache_state.to_string(),
    }
}

fn font_size_metrics(bytes: &[u8]) -> Result<FontSubsetSizeMetrics, String> {
    let mut encoder = DeflateEncoder::new(Vec::new(), Compression::best());
    encoder
        .write_all(bytes)
        .map_err(|error| format!("failed to measure font DEFLATE size: {error}"))?;
    let deflate_bytes = encoder
        .finish()
        .map_err(|error| format!("failed to finish font DEFLATE measurement: {error}"))?
        .len();
    Ok(FontSubsetSizeMetrics {
        raw_bytes: bytes.len() as u64,
        deflate_bytes: deflate_bytes as u64,
    })
}

fn staged_font_manifest_reason(
    staging_root: &Path,
    font_path: &str,
    source_sha256: &str,
) -> Option<String> {
    let manifest_path = staging_root.join(DEFAULT_ASSET_MANIFEST_PATH);
    let bytes = match std::fs::read(manifest_path) {
        Ok(bytes) => bytes,
        Err(_) => return Some("manifest_metadata_missing".to_string()),
    };
    let manifest: serde_json::Value = match serde_json::from_slice(&bytes) {
        Ok(manifest) => manifest,
        Err(_) => return Some("manifest_metadata_invalid".to_string()),
    };
    let Some(entries) = manifest.get("assets").and_then(serde_json::Value::as_array) else {
        return Some("manifest_metadata_invalid".to_string());
    };
    let matching = entries
        .iter()
        .filter(|entry| entry.get("path").and_then(serde_json::Value::as_str) == Some(font_path))
        .collect::<Vec<_>>();
    if matching.len() != 1 {
        return Some("manifest_entry_missing_or_ambiguous".to_string());
    }
    let entry = matching[0];
    let format = entry.get("format");
    if format
        .and_then(|format| format.get("kind"))
        .and_then(serde_json::Value::as_str)
        != Some("font")
        || format
            .and_then(|format| format.get("encoding"))
            .and_then(serde_json::Value::as_str)
            != Some("ttf")
    {
        return Some("manifest_format_mismatch".to_string());
    }
    if entry
        .get("content_sha256")
        .and_then(serde_json::Value::as_str)
        != Some(source_sha256)
    {
        return Some("manifest_hash_mismatch".to_string());
    }
    None
}

fn normalize_proof_font_path(path: &str) -> Option<String> {
    let path = path.strip_prefix('/').unwrap_or(path);
    validate_font_asset_path("proof font path", path)
        .ok()
        .map(|()| path.to_string())
}

fn discover_fonttools() -> Option<PythonTool> {
    let candidates = python_candidates();
    for (executable, prefix_args) in candidates {
        let mut command = Command::new(&executable);
        command.args(&prefix_args).args([
            OsString::from("-c"),
            OsString::from(
                "import sys,fontTools; print(fontTools.__version__ + '|' + '.'.join(map(str, sys.version_info[:3])))",
            ),
        ]);
        let Ok(output) = command.output() else {
            continue;
        };
        if !output.status.success() {
            continue;
        }
        let Ok(version) = String::from_utf8(output.stdout) else {
            continue;
        };
        let version = version.trim();
        let Some((fonttools_version, python_version)) = version.split_once('|') else {
            continue;
        };
        if fonttools_version == "4.60.2" {
            return Some(PythonTool {
                executable,
                prefix_args,
                identity: format!("{FONTTOOLS_PACKAGE};python=={python_version}"),
            });
        }
    }
    None
}

#[cfg(windows)]
fn python_candidates() -> Vec<(OsString, Vec<OsString>)> {
    vec![
        (OsString::from("python"), Vec::new()),
        (OsString::from("python3"), Vec::new()),
        (OsString::from("py"), vec![OsString::from("-3")]),
    ]
}

#[cfg(not(windows))]
fn python_candidates() -> Vec<(OsString, Vec<OsString>)> {
    vec![
        (OsString::from("python3"), Vec::new()),
        (OsString::from("python"), Vec::new()),
    ]
}

#[derive(Debug, Serialize)]
struct FontSubsetDriverRequest<'a> {
    action: &'static str,
    input: String,
    output: Option<String>,
    original: Option<String>,
    scalars: &'a [u32],
    reserved_names: &'a [String],
    replacement_family: Option<&'a str>,
}

impl<'a> FontSubsetDriverRequest<'a> {
    fn subset(
        input: &Path,
        output: &Path,
        scalars: &'a [u32],
        reserved_names: &'a [String],
        replacement_family: Option<&'a str>,
    ) -> Self {
        Self {
            action: "subset",
            input: input.to_string_lossy().into_owned(),
            output: Some(output.to_string_lossy().into_owned()),
            original: None,
            scalars,
            reserved_names,
            replacement_family,
        }
    }

    fn validate(
        original: &Path,
        candidate: &Path,
        scalars: &'a [u32],
        reserved_names: &'a [String],
        replacement_family: Option<&'a str>,
    ) -> Self {
        Self {
            action: "validate",
            input: candidate.to_string_lossy().into_owned(),
            output: None,
            original: Some(original.to_string_lossy().into_owned()),
            scalars,
            reserved_names,
            replacement_family,
        }
    }
}

fn run_fonttools_driver(
    tool: &PythonTool,
    request: &FontSubsetDriverRequest<'_>,
) -> Result<(), String> {
    let request = serde_json::to_vec(request).map_err(|_| "driver_request_invalid".to_string())?;
    let mut command = Command::new(&tool.executable);
    command
        .args(&tool.prefix_args)
        .arg("-c")
        .arg(FONT_SUBSET_DRIVER)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|_| "exact_tool_unavailable".to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&request)
            .map_err(|_| "driver_request_failed".to_string())?;
    }
    let output = child
        .wait_with_output()
        .map_err(|_| "driver_process_failed".to_string())?;
    let response: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|_| "driver_response_invalid".to_string())?;
    if response.get("ok").and_then(serde_json::Value::as_bool) == Some(true) {
        return Ok(());
    }
    Err(response
        .get("reason")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("fonttools_transform_failed")
        .to_string())
}

static NEXT_FONT_SUBSET_TEMP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn publish_font_subset_outputs(
    staging_root: &Path,
    replacements: &[(String, Vec<u8>, String)],
    licenses: &[(PathBuf, Vec<u8>)],
    report: &mut FontSubsettingReport,
) -> Result<(), String> {
    let mut updates = Vec::<(PathBuf, Vec<u8>)>::new();
    for (path, bytes, _) in replacements {
        updates.push((PathBuf::from(path), bytes.clone()));
    }
    for (path, bytes) in licenses {
        updates.push((path.clone(), bytes.clone()));
    }

    if !replacements.is_empty() {
        let manifest_path = staging_root.join(DEFAULT_ASSET_MANIFEST_PATH);
        let manifest_bytes = std::fs::read(&manifest_path).map_err(|error| {
            format!("failed to read staged asset manifest for font subsetting: {error}")
        })?;
        let mut manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes)
            .map_err(|error| format!("failed to decode staged asset manifest: {error}"))?;
        let entries = manifest
            .get_mut("assets")
            .and_then(serde_json::Value::as_array_mut)
            .ok_or_else(|| "staged asset manifest is missing its assets array".to_string())?;
        for (path, prepared, source_sha256) in replacements {
            let matches = entries.iter_mut().filter(|entry| {
                entry.get("path").and_then(serde_json::Value::as_str) == Some(path)
            });
            let mut matched = 0usize;
            for entry in matches {
                let object = entry
                    .as_object_mut()
                    .ok_or_else(|| format!("staged font asset metadata is invalid for {path}"))?;
                object.insert(
                    "content_sha256".to_string(),
                    serde_json::Value::String(stasis_assets::sha256_bytes(prepared)),
                );
                object.insert(
                    "prepared_from_sha256".to_string(),
                    serde_json::Value::String(source_sha256.clone()),
                );
                matched += 1;
            }
            if matched != 1 {
                return Err(format!(
                    "staged asset manifest must contain exactly one entry for {path}"
                ));
            }
        }
        let mut encoded_manifest = serde_json::to_vec_pretty(&manifest)
            .map_err(|error| format!("failed to encode staged asset manifest: {error}"))?;
        encoded_manifest.push(b'\n');
        let identity = AssetPackageIdentity::from_manifest_bytes(&encoded_manifest);
        let identity_bytes = serde_json::to_vec_pretty(&identity)
            .map_err(|error| format!("failed to encode staged asset package identity: {error}"))?;
        updates.push((PathBuf::from(DEFAULT_ASSET_MANIFEST_PATH), encoded_manifest));
        updates.push((PathBuf::from(ASSET_PACKAGE_IDENTITY_PATH), identity_bytes));
    }

    let mut encoded_report = serde_json::to_vec_pretty(report)
        .map_err(|error| format!("failed to encode font subset report: {error}"))?;
    encoded_report.push(b'\n');
    updates.push((PathBuf::from(FONT_SUBSET_REPORT_PATH), encoded_report));

    let mut previous = Vec::<(PathBuf, Option<Vec<u8>>)>::new();
    for (relative, bytes) in updates {
        let destination = staging_root.join(&relative);
        let old = std::fs::read(&destination).ok();
        if let Err(error) = atomic_write_bytes(&destination, &bytes) {
            let mut rollback_failed = false;
            for (written, content) in previous.iter().rev() {
                let path = staging_root.join(written);
                let restored = match content {
                    Some(content) => atomic_write_bytes(&path, content).is_ok(),
                    None => std::fs::remove_file(&path).is_ok() || !path.exists(),
                };
                rollback_failed |= !restored;
            }
            if rollback_failed {
                return Err(format!(
                    "failed publishing font subset output ({error}); staged files could not all be restored"
                ));
            }
            for decision in &mut report.fonts {
                if decision.decision == "subset" {
                    decision.decision = "retained".to_string();
                    decision.reason = Some("stage_update_failed".to_string());
                    decision.prepared_sha256 = decision.source_sha256.clone();
                    decision.prepared_size = decision.source_size;
                    decision.license_notice_staged_path = None;
                }
            }
            let mut fallback_report = serde_json::to_vec_pretty(report).map_err(|serialize| {
                format!("failed to encode font subset fallback report: {serialize}")
            })?;
            fallback_report.push(b'\n');
            atomic_write_bytes(
                &staging_root.join(FONT_SUBSET_REPORT_PATH),
                &fallback_report,
            )
            .map_err(|write| {
                format!("failed publishing font subset fallback report after {error}: {write}")
            })?;
            return Ok(());
        }
        previous.push((relative, old));
    }
    Ok(())
}

fn atomic_write_bytes(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("failed creating {}: {error}", parent.display()))?;
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| format!("failed staging {}: {error}", path.display()))?;
    file.commit()
        .map_err(|error| format!("failed committing {}: {error}", path.display()))
}

fn snapshot_asset_source_dirs(snapshot: &ProgramSnapshot) -> Vec<PathBuf> {
    snapshot
        .module_graph()
        .roots()
        .iter()
        .filter_map(|root| Path::new(root).parent().map(Path::to_path_buf))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

pub(crate) fn validate_snapshot_assets(
    project_dir: &Path,
    snapshot: &ProgramSnapshot,
    resolved: Option<&ResolvedAssetManifest>,
) -> Result<AssetValidationResult, String> {
    let manifest_paths = resolved.map(|manifest| {
        manifest
            .assets
            .iter()
            .map(|asset| asset.entry.path.clone())
            .collect()
    });
    let dynamic_paths = resolved
        .map(|manifest| &manifest.dynamic_assets)
        .cloned()
        .unwrap_or_default();
    let source_base_dirs = snapshot_asset_source_dirs(snapshot);
    let validation = validate_asset_references(
        project_dir,
        &source_base_dirs,
        snapshot.asset_references(),
        manifest_paths.as_ref(),
        &dynamic_paths,
    );
    if validation.diagnostics.is_empty() {
        Ok(validation)
    } else {
        Err(format_asset_diagnostics(&validation.diagnostics))
    }
}

pub(crate) fn retain_snapshot_assets(
    project_dir: &Path,
    snapshot: &ProgramSnapshot,
    resolved: &ResolvedAssetManifest,
) -> Result<ResolvedAssetManifest, String> {
    let validation = validate_snapshot_assets(project_dir, snapshot, Some(resolved))?;
    Ok(resolved.retain_paths(&validation.resolved_paths))
}

pub(crate) fn resolve_snapshot_assets(
    project_dir: &Path,
    snapshot: &ProgramSnapshot,
) -> Result<ResolvedAssetManifest, String> {
    let source_manifest = project_dir.join(DEFAULT_ASSET_MANIFEST_PATH);
    if source_manifest.exists() {
        let resolved = load_project_asset_manifest(project_dir, AssetLimits::default())
            .map_err(|error| format!("failed to resolve mobile AOT assets: {error}"))?;
        return retain_snapshot_assets(project_dir, snapshot, &resolved);
    }

    let validation = validate_snapshot_assets(project_dir, snapshot, None)?;
    resolve_project_asset_paths(
        project_dir,
        &validation.resolved_paths,
        AssetLimits::default(),
    )
    .map_err(|error| format!("failed to infer mobile AOT assets: {error}"))
}

pub(crate) fn format_asset_diagnostics(diagnostics: &[AssetDiagnostic]) -> String {
    let json = serde_json::to_string(diagnostics)
        .unwrap_or_else(|_| "[{\"code\":\"asset_diagnostic_serialization_failed\"}]".to_string());
    format!("{ASSET_DIAGNOSTIC_PREFIX}{json}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_assets::{
        stable_asset_handle, AssetEntry, AssetFormat, ResolvedAsset, SpriteEncoding,
    };
    use stasis_compiler::backend::aot::AotProcess;
    use stasis_compiler::backend::text_coverage::{
        TextCoverageFontEvidence, TextCoverageUnknownReason,
    };

    const BASIC_FONT_PATH: &str = "assets/gauntlet-font/Basic-Regular.ttf";
    const BASIC_LICENSE_PATH: &str = "assets/gauntlet-font/OFL.txt";

    struct FontFixture {
        root: PathBuf,
        staging: PathBuf,
        cache: PathBuf,
        config: ReleaseFontSubsettingManifest,
        source_bytes: Vec<u8>,
        manifest_bytes: Vec<u8>,
    }

    fn font_fixture() -> FontFixture {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_release_font_subset_{stamp}"));
        let asset_dir = root.join("assets/gauntlet-font");
        std::fs::create_dir_all(&asset_dir).expect("create font asset directory");
        let fixture_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let font_bytes = std::fs::read(fixture_root.join("assets/gauntlet-font/Basic-Regular.ttf"))
            .expect("read Basic font fixture");
        let license_bytes = std::fs::read(fixture_root.join("assets/gauntlet-font/OFL.txt"))
            .expect("read Basic license fixture");
        std::fs::write(root.join(BASIC_FONT_PATH), &font_bytes).expect("write font fixture");
        std::fs::write(root.join(BASIC_LICENSE_PATH), &license_bytes)
            .expect("write license fixture");
        let source_sha256 = stasis_assets::sha256_bytes(&font_bytes);
        let manifest = serde_json::json!({
            "schema": "stasis-assets",
            "version": 2,
            "assets": [{
                "id": "basic",
                "path": BASIC_FONT_PATH,
                "content_sha256": source_sha256,
                "format": {"kind": "font", "encoding": "ttf"},
                "dependencies": []
            }]
        });
        let mut manifest_bytes = serde_json::to_vec_pretty(&manifest).expect("encode manifest");
        manifest_bytes.push(b'\n');
        std::fs::write(root.join(DEFAULT_ASSET_MANIFEST_PATH), &manifest_bytes)
            .expect("write source manifest");
        let staging = root.join("stage");
        stage_font_fixture(&staging, &font_bytes, &manifest_bytes);
        FontFixture {
            cache: root.join("cache"),
            root,
            staging,
            config: ReleaseFontSubsettingManifest {
                fonts: vec![ReleaseFontSubsetEntry {
                    path: BASIC_FONT_PATH.to_string(),
                    license_path: BASIC_LICENSE_PATH.to_string(),
                    modification_permitted: true,
                    reserved_names: vec!["Basic".to_string()],
                    replacement_family: Some("Stasis Gauntlet".to_string()),
                }],
            },
            source_bytes: font_bytes,
            manifest_bytes,
        }
    }

    fn stage_font_fixture(staging: &Path, font_bytes: &[u8], manifest_bytes: &[u8]) {
        std::fs::create_dir_all(staging.join("assets/gauntlet-font"))
            .expect("create staged font directory");
        std::fs::write(staging.join(BASIC_FONT_PATH), font_bytes).expect("write staged font");
        std::fs::write(staging.join(DEFAULT_ASSET_MANIFEST_PATH), manifest_bytes)
            .expect("write staged asset manifest");
        stasis_assets::write_asset_package_identity(staging).expect("write asset package identity");
    }

    fn finite_font_proof(codepoints: &[u32], evidence_path: &str) -> TextCoverageProof {
        TextCoverageProof::Finite {
            unicode_scalars: codepoints.to_vec(),
            fonts: vec![TextCoverageFontEvidence {
                path: evidence_path.to_string(),
            }],
            sinks: Vec::new(),
        }
    }

    fn resolved(root: &Path, id: &str, path: &str) -> ResolvedAsset {
        let entry = AssetEntry {
            id: id.to_string(),
            path: path.to_string(),
            content_sha256: "0".repeat(64),
            prepared_from_sha256: None,
            format: AssetFormat::Sprite {
                encoding: SpriteEncoding::Svg,
                width: 32,
                height: 32,
                layout: None,
            },
            prepare: None,
            dependencies: vec![],
        };
        ResolvedAsset {
            handle: stable_asset_handle(&entry),
            entry,
            absolute_path: root.join(path),
            byte_length: 1,
        }
    }

    fn retain_from_sources(
        root: &Path,
        sources: &[(String, String)],
        manifest: &ResolvedAssetManifest,
    ) -> ResolvedAssetManifest {
        let mut process = AotProcess::new();
        process
            .set_project_root(root.to_string_lossy().to_string())
            .expect("project root");
        for (path, source) in sources {
            process.upsert_file(
                root.join(path).to_string_lossy().to_string(),
                source.clone(),
            );
        }
        process.compile().expect("compile asset fixture");
        retain_snapshot_assets(
            root,
            process.program_snapshot().expect("program snapshot"),
            manifest,
        )
        .expect("retain snapshot assets")
    }

    #[test]
    fn font_subsetting_config_validates_paths_legal_assertions_and_reserved_names() {
        let fixture = font_fixture();
        assert!(validate_font_subsetting_manifest(Some(&fixture.root), &fixture.config).is_ok());

        std::fs::remove_file(fixture.root.join(BASIC_LICENSE_PATH))
            .expect("remove license fixture");
        let missing_license =
            validate_font_subsetting_manifest(Some(&fixture.root), &fixture.config)
                .expect_err("missing license notice must fail validation");
        assert!(
            missing_license.contains(BASIC_LICENSE_PATH),
            "{missing_license}"
        );

        let mut unsafe_path = fixture.config.clone();
        unsafe_path.fonts[0].path = "assets/../outside.ttf".to_string();
        assert!(validate_font_subsetting_manifest(None, &unsafe_path).is_err());

        let mut unapproved = fixture.config.clone();
        unapproved.fonts[0].modification_permitted = false;
        assert!(validate_font_subsetting_manifest(None, &unapproved).is_err());

        let mut no_rename = fixture.config.clone();
        no_rename.fonts[0].replacement_family = None;
        assert!(validate_font_subsetting_manifest(None, &no_rename).is_err());

        let missing_reserved_names =
            serde_json::from_value::<ReleaseFontSubsetEntry>(serde_json::json!({
                "path": BASIC_FONT_PATH,
                "license_path": BASIC_LICENSE_PATH,
                "modification_permitted": true,
                "replacement_family": "Stasis Gauntlet"
            }));
        assert!(missing_reserved_names.is_err());
        std::fs::remove_dir_all(fixture.root).ok();
    }

    #[test]
    fn unknown_text_coverage_retains_font_and_records_reason_codes() {
        let fixture = font_fixture();
        let proof = TextCoverageProof::Unknown {
            reasons: vec![
                TextCoverageUnknownReason {
                    code: "unknown_text_value".to_string(),
                    function_id: 7,
                    detail: "dynamic input".to_string(),
                },
                TextCoverageUnknownReason {
                    code: "unknown_font".to_string(),
                    function_id: 3,
                    detail: "runtime font".to_string(),
                },
                TextCoverageUnknownReason {
                    code: "unknown_text_value".to_string(),
                    function_id: 7,
                    detail: "dynamic input".to_string(),
                },
            ],
            fonts: vec![TextCoverageFontEvidence {
                path: BASIC_FONT_PATH.to_string(),
            }],
            sinks: Vec::new(),
        };
        let report = apply_release_font_subsetting_with_tool(
            &fixture.root,
            &fixture.staging,
            &fixture.cache,
            &proof,
            &fixture.config,
            Some("Game"),
            Some(BASIC_FONT_PATH),
            None,
        )
        .expect("report unknown coverage");
        let decision = &report.fonts[0];
        assert_eq!(decision.decision, "retained");
        assert_eq!(decision.reason.as_deref(), Some("coverage_unknown"));
        assert_eq!(
            decision.unknown_reason_codes,
            vec!["unknown_font", "unknown_text_value"]
        );
        assert_eq!(
            std::fs::read(fixture.staging.join(BASIC_FONT_PATH)).unwrap(),
            fixture.source_bytes
        );
        let license_staged = fixture
            .staging
            .join(decision.license_notice_staged_path.as_ref().unwrap());
        assert_eq!(
            std::fs::read(license_staged).unwrap(),
            std::fs::read(fixture.root.join(BASIC_LICENSE_PATH)).unwrap()
        );
        assert!(
            !fixture.staging.join(DEFAULT_ASSET_MANIFEST_PATH).exists()
                || std::fs::read(fixture.staging.join(DEFAULT_ASSET_MANIFEST_PATH)).unwrap()
                    == fixture.manifest_bytes
        );
        std::fs::remove_dir_all(fixture.root).ok();
    }

    #[test]
    fn finite_shell_title_counts_as_font_evidence_and_missing_tool_retains_original() {
        let fixture = font_fixture();
        let proof = TextCoverageProof::Finite {
            unicode_scalars: vec![u32::from('A')],
            fonts: Vec::new(),
            sinks: Vec::new(),
        };
        let report = apply_release_font_subsetting_with_tool(
            &fixture.root,
            &fixture.staging,
            &fixture.cache,
            &proof,
            &fixture.config,
            Some("Game ë"),
            Some("/assets/gauntlet-font/Basic-Regular.ttf"),
            None,
        )
        .expect("report missing tool");
        let decision = &report.fonts[0];
        assert_eq!(decision.reason.as_deref(), Some("exact_tool_unavailable"));
        assert!(decision.codepoints.contains(&u32::from('A')));
        assert!(decision.codepoints.contains(&u32::from('G')));
        assert!(decision.codepoints.contains(&u32::from('ë')));
        assert!(decision.codepoints.contains(&u32::from('Ë')));
        assert_eq!(
            std::fs::read(fixture.staging.join(BASIC_FONT_PATH)).unwrap(),
            fixture.source_bytes
        );
        std::fs::remove_dir_all(fixture.root).ok();
    }

    #[test]
    fn exact_fonttools_subsets_basic_and_records_deterministic_cache_provenance() {
        if discover_fonttools().is_none() {
            eprintln!("skipping exact fontTools test: fonttools==4.60.2 is unavailable");
            return;
        }
        let fixture = font_fixture();
        let proof = finite_font_proof(&[u32::from('A')], &format!("/{BASIC_FONT_PATH}"));
        let first = apply_release_font_subsetting(
            &fixture.root,
            &fixture.staging,
            &fixture.cache,
            &proof,
            Some(&fixture.config),
            Some("Game ë"),
            Some(&format!("/{BASIC_FONT_PATH}")),
        )
        .expect("subset Basic font");
        let first_decision = &first.fonts[0];
        assert_eq!(first_decision.decision, "subset", "{first_decision:?}");
        assert_eq!(first_decision.cache_state, "miss");
        assert!(first_decision.codepoints.contains(&u32::from('ë')));
        let subset_bytes =
            std::fs::read(fixture.staging.join(BASIC_FONT_PATH)).expect("read subset font");
        assert_ne!(subset_bytes, fixture.source_bytes);
        assert_eq!(
            first_decision.prepared_sha256.as_deref(),
            Some(stasis_assets::sha256_bytes(&subset_bytes).as_str())
        );
        assert!(first_decision
            .prepared_size
            .is_some_and(|size| size.raw_bytes < fixture.source_bytes.len() as u64));
        assert!(first_decision.prepared_size.is_some_and(
            |size| size.deflate_bytes < first_decision.source_size.unwrap().deflate_bytes
        ));

        let manifest_bytes = std::fs::read(fixture.staging.join(DEFAULT_ASSET_MANIFEST_PATH))
            .expect("read transformed manifest");
        let manifest: serde_json::Value = serde_json::from_slice(&manifest_bytes).unwrap();
        let asset = &manifest["assets"][0];
        assert_eq!(
            asset["content_sha256"],
            stasis_assets::sha256_bytes(&subset_bytes)
        );
        assert_eq!(
            asset["prepared_from_sha256"],
            stasis_assets::sha256_bytes(&fixture.source_bytes)
        );
        let identity: AssetPackageIdentity = serde_json::from_slice(
            &std::fs::read(fixture.staging.join(ASSET_PACKAGE_IDENTITY_PATH)).unwrap(),
        )
        .expect("decode asset package identity");
        assert_eq!(
            identity.manifest_sha256,
            stasis_assets::sha256_bytes(&manifest_bytes)
        );
        let license_staged = fixture
            .staging
            .join(first_decision.license_notice_staged_path.as_ref().unwrap());
        assert_eq!(
            std::fs::read(license_staged).unwrap(),
            std::fs::read(fixture.root.join(BASIC_LICENSE_PATH)).unwrap()
        );

        let second_staging = fixture.root.join("stage-second");
        stage_font_fixture(
            &second_staging,
            &fixture.source_bytes,
            &fixture.manifest_bytes,
        );
        let second = apply_release_font_subsetting(
            &fixture.root,
            &second_staging,
            &fixture.cache,
            &proof,
            Some(&fixture.config),
            Some("Game ë"),
            Some(BASIC_FONT_PATH),
        )
        .expect("reuse font cache");
        assert_eq!(second.fonts[0].decision, "subset");
        assert_eq!(second.fonts[0].cache_state, "hit");
        assert_eq!(
            second.fonts[0].prepared_sha256,
            first_decision.prepared_sha256
        );
        assert_eq!(
            std::fs::read(second_staging.join(BASIC_FONT_PATH)).unwrap(),
            subset_bytes
        );
        std::fs::remove_dir_all(fixture.root).ok();
    }

    #[test]
    fn exact_fonttools_retains_font_when_embedded_rfn_is_undeclared() {
        if discover_fonttools().is_none() {
            eprintln!("skipping exact fontTools test: fonttools==4.60.2 is unavailable");
            return;
        }
        let mut fixture = font_fixture();
        fixture.config.fonts[0].reserved_names.clear();
        fixture.config.fonts[0].replacement_family = None;
        let proof = finite_font_proof(&[u32::from('A')], BASIC_FONT_PATH);
        let report = apply_release_font_subsetting(
            &fixture.root,
            &fixture.staging,
            &fixture.cache,
            &proof,
            Some(&fixture.config),
            None,
            None,
        )
        .expect("retain Basic when its embedded RFN is undeclared");
        assert_eq!(report.fonts[0].decision, "retained");
        assert_eq!(
            report.fonts[0].reason.as_deref(),
            Some("reserved_name_metadata_ambiguous")
        );
        assert_eq!(
            std::fs::read(fixture.staging.join(BASIC_FONT_PATH)).unwrap(),
            fixture.source_bytes
        );
        std::fs::remove_dir_all(fixture.root).ok();
    }

    #[test]
    fn keeps_only_literal_assets_in_the_reachable_source_set() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_release_assets_{stamp}"));
        std::fs::create_dir_all(root.join("assets/svg")).unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("assets/svg/used.svg"), "u").unwrap();
        std::fs::write(root.join("assets/svg/unused.svg"), "x").unwrap();
        let manifest = ResolvedAssetManifest {
            manifest_path: root.join("assets/manifest.json"),
            dynamic_assets: Default::default(),
            assets: vec![
                resolved(&root, "used", "assets/svg/used.svg"),
                resolved(&root, "unused", "assets/svg/unused.svg"),
            ],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            concat!(
                "function @asset_path(path) request_sprite(path: string, max_w: i32, max_h: i32): bool { return true; } ",
                "function main(): void { ",
                "request_sprite(\"../assets/svg/used.svg\", 32, 32); ",
                "request_sprite(\"assets/svg/used.svg\", 32, 32); }"
            )
            .to_string(),
        )];

        let retained = retain_from_sources(&root, &sources, &manifest);
        assert_eq!(retained.assets.len(), 1);
        assert_eq!(retained.assets[0].entry.id, "used");
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn project_root_literal_skips_existing_non_asset_source_candidate() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_release_asset_shadow_{stamp}"));
        std::fs::create_dir_all(root.join("assets/svg")).unwrap();
        std::fs::create_dir_all(root.join("src/assets/svg")).unwrap();
        std::fs::write(root.join("assets/svg/used.svg"), "project asset").unwrap();
        std::fs::write(root.join("src/assets/svg/used.svg"), "source shadow").unwrap();
        let manifest = ResolvedAssetManifest {
            manifest_path: root.join("assets/manifest.json"),
            dynamic_assets: Default::default(),
            assets: vec![resolved(&root, "used", "assets/svg/used.svg")],
        };
        let sources = vec![(
            "src/main.stasis".to_string(),
            concat!(
                "function @asset_path(path) request_sprite(path: string, max_w: i32, max_h: i32): bool { return true; } ",
                "function main(): void { request_sprite(\"assets/svg/used.svg\", 32, 32); }"
            )
            .to_string(),
        )];

        let retained = retain_from_sources(&root, &sources, &manifest);
        assert_eq!(retained.assets.len(), 1);
        assert_eq!(retained.assets[0].entry.id, "used");
        std::fs::remove_dir_all(root).ok();
    }
}
