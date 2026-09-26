use serde::de::{self, MapAccess, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer};
use serde_json::Value;
use sha2::{Digest, Sha256};
use stasis_compiler::backend::program_snapshot::{ProjectConfiguration, ProjectSettingValue};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

pub(super) const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub(crate) const GENERATED_SETTINGS_FILE: &str =
    stasis_compiler::frontend::module_graph::GENERATED_PROJECT_SETTINGS_PATH;
const MAX_SETTING_COUNT: usize = 128;
const MAX_SETTING_STRING_BYTES: usize = 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum CanonicalTarget {
    Web,
    WindowsX86_64,
    WindowsArm64,
    LinuxX86_64,
    LinuxArm64,
    MacosX86_64,
    MacosArm64,
    AndroidArm64,
    AndroidX86_64,
    IosArm64,
}

impl CanonicalTarget {
    pub(super) const ALL: [Self; 10] = [
        Self::Web,
        Self::WindowsX86_64,
        Self::WindowsArm64,
        Self::LinuxX86_64,
        Self::LinuxArm64,
        Self::MacosX86_64,
        Self::MacosArm64,
        Self::AndroidArm64,
        Self::AndroidX86_64,
        Self::IosArm64,
    ];

    pub(crate) fn host() -> Self {
        if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            Self::WindowsArm64
        } else if cfg!(target_os = "windows") {
            Self::WindowsX86_64
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            Self::MacosArm64
        } else if cfg!(target_os = "macos") {
            Self::MacosX86_64
        } else if cfg!(target_arch = "aarch64") {
            Self::LinuxArm64
        } else {
            Self::LinuxX86_64
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::Web => "web",
            Self::WindowsX86_64 => "windows-x86_64",
            Self::WindowsArm64 => "windows-arm64",
            Self::LinuxX86_64 => "linux-x86_64",
            Self::LinuxArm64 => "linux-arm64",
            Self::MacosX86_64 => "macos-x86_64",
            Self::MacosArm64 => "macos-arm64",
            Self::AndroidArm64 => "android-arm64",
            Self::AndroidX86_64 => "android-x86_64",
            Self::IosArm64 => "ios-arm64",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|target| target.as_str() == value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct ProjectSettingsManifest {
    pub(super) schema_version: u32,
    pub(super) definitions: BTreeMap<String, SettingDefinition>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) targets: BTreeMap<String, BTreeMap<String, Value>>,
}

use serde::Serialize;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub(super) struct SettingDefinition {
    #[serde(rename = "type")]
    kind: SettingKind,
    #[serde(default)]
    required: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    default: Option<Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    allowed: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    minimum: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    maximum: Option<f64>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum SettingKind {
    String,
    Bool,
    Number,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ResolvedProjectSettings {
    pub(crate) configuration: ProjectConfiguration,
    pub(crate) generated_source: String,
}

pub(super) fn parse_strict_json(bytes: &[u8]) -> Result<Value, String> {
    let mut deserializer = serde_json::Deserializer::from_slice(bytes);
    let strict = StrictValue::deserialize(&mut deserializer)
        .map_err(|error| format!("invalid stasis.json: {error}"))?;
    deserializer
        .end()
        .map_err(|error| format!("invalid stasis.json: {error}"))?;
    Ok(strict.0)
}

struct StrictValue(Value);

impl<'de> Deserialize<'de> for StrictValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(StrictValueVisitor)
    }
}

struct StrictValueVisitor;

impl<'de> Visitor<'de> for StrictValueVisitor {
    type Value = StrictValue;

    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("a JSON value")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Bool(value)))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Number(value.into())))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        serde_json::Number::from_f64(value)
            .map(Value::Number)
            .map(StrictValue)
            .ok_or_else(|| E::custom("JSON number must be finite"))
    }

    fn visit_str<E>(self, value: &str) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value.to_string())))
    }

    fn visit_string<E>(self, value: String) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::String(value)))
    }

    fn visit_none<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_unit<E>(self) -> Result<Self::Value, E> {
        Ok(StrictValue(Value::Null))
    }

    fn visit_seq<A>(self, mut values: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let mut output = Vec::new();
        while let Some(value) = values.next_element::<StrictValue>()? {
            output.push(value.0);
        }
        Ok(StrictValue(Value::Array(output)))
    }

    fn visit_map<A>(self, mut values: A) -> Result<Self::Value, A::Error>
    where
        A: MapAccess<'de>,
    {
        let mut output = serde_json::Map::new();
        while let Some(key) = values.next_key::<String>()? {
            if output.contains_key(&key) {
                return Err(de::Error::custom(format!("duplicate JSON key '{key}'")));
            }
            output.insert(key, values.next_value::<StrictValue>()?.0);
        }
        Ok(StrictValue(Value::Object(output)))
    }
}

pub(super) fn validate_manifest(settings: &ProjectSettingsManifest) -> Result<(), String> {
    if settings.schema_version != SETTINGS_SCHEMA_VERSION {
        return Err(format!(
            "unsupported settings.schema_version {}; expected {}",
            settings.schema_version, SETTINGS_SCHEMA_VERSION
        ));
    }
    if settings.definitions.len() > MAX_SETTING_COUNT {
        return Err(format!(
            "settings.definitions supports at most {MAX_SETTING_COUNT} entries"
        ));
    }
    for (key, definition) in &settings.definitions {
        validate_key(key)?;
        validate_definition(key, definition)?;
    }
    for (target, overrides) in &settings.targets {
        if CanonicalTarget::parse(target).is_none() {
            return Err(format!("unknown settings target '{target}'"));
        }
        for (key, value) in overrides {
            let definition = settings
                .definitions
                .get(key)
                .ok_or_else(|| format!("unknown setting key '{key}' in target '{target}'"))?;
            validate_value(key, definition, value)?;
        }
    }
    Ok(())
}

fn validate_key(key: &str) -> Result<(), String> {
    let valid = !key.is_empty()
        && key.len() <= 64
        && key.as_bytes()[0].is_ascii_lowercase()
        && key
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_');
    if !valid {
        return Err(format!(
            "setting key '{key}' must be 1..=64 lowercase snake_case characters"
        ));
    }
    let sensitive_parts = [
        "secret",
        "password",
        "credential",
        "api_key",
        "private_key",
        "signing_key",
        "token",
    ];
    if sensitive_parts.iter().any(|part| key.contains(part)) {
        return Err(format!(
            "setting key '{key}' is unsupported because project settings must not contain secrets or credentials"
        ));
    }
    Ok(())
}

fn validate_definition(key: &str, definition: &SettingDefinition) -> Result<(), String> {
    match definition.kind {
        SettingKind::String => {
            if definition.minimum.is_some() || definition.maximum.is_some() {
                return Err(format!(
                    "setting '{key}' uses string type and cannot declare minimum or maximum"
                ));
            }
            if definition.allowed.len() > 128 {
                return Err(format!(
                    "setting '{key}' supports at most 128 allowed values"
                ));
            }
            let mut unique = BTreeSet::new();
            for allowed in &definition.allowed {
                validate_string(key, allowed)?;
                if !unique.insert(allowed) {
                    return Err(format!("setting '{key}' has duplicate allowed value"));
                }
            }
        }
        SettingKind::Bool => {
            if !definition.allowed.is_empty()
                || definition.minimum.is_some()
                || definition.maximum.is_some()
            {
                return Err(format!(
                    "setting '{key}' uses bool type and cannot declare allowed, minimum, or maximum"
                ));
            }
        }
        SettingKind::Number => {
            if !definition.allowed.is_empty() {
                return Err(format!(
                    "setting '{key}' uses number type and cannot declare allowed"
                ));
            }
            let (Some(minimum), Some(maximum)) = (definition.minimum, definition.maximum) else {
                return Err(format!(
                    "setting '{key}' uses number type and must declare finite minimum and maximum"
                ));
            };
            if !minimum.is_finite() || !maximum.is_finite() || minimum > maximum {
                return Err(format!(
                    "setting '{key}' minimum and maximum must be finite and ordered"
                ));
            }
        }
    }
    if definition.required && definition.default.is_none() {
        // A required setting may intentionally be supplied by every target.
    }
    if let Some(value) = definition.default.as_ref() {
        validate_value(key, definition, value)?;
    }
    Ok(())
}

fn validate_string(key: &str, value: &str) -> Result<(), String> {
    if value.len() > MAX_SETTING_STRING_BYTES || value.chars().any(char::is_control) {
        return Err(format!(
            "setting '{key}' string must be at most {MAX_SETTING_STRING_BYTES} UTF-8 bytes and contain no control characters"
        ));
    }
    Ok(())
}

fn validate_value(key: &str, definition: &SettingDefinition, value: &Value) -> Result<(), String> {
    match definition.kind {
        SettingKind::String => {
            let value = value.as_str().ok_or_else(|| {
                format!(
                    "setting '{key}' expects string but received {}",
                    json_kind(value)
                )
            })?;
            validate_string(key, value)?;
            if !definition.allowed.is_empty()
                && !definition.allowed.iter().any(|allowed| allowed == value)
            {
                return Err(format!(
                    "setting '{key}' contains an unsupported string value"
                ));
            }
        }
        SettingKind::Bool => {
            if !value.is_boolean() {
                return Err(format!(
                    "setting '{key}' expects bool but received {}",
                    json_kind(value)
                ));
            }
        }
        SettingKind::Number => {
            let number = value.as_f64().ok_or_else(|| {
                format!(
                    "setting '{key}' expects number but received {}",
                    json_kind(value)
                )
            })?;
            let minimum = definition.minimum.expect("validated number minimum");
            let maximum = definition.maximum.expect("validated number maximum");
            if !number.is_finite() || number < minimum || number > maximum {
                return Err(format!(
                    "setting '{key}' number must be between {minimum} and {maximum}"
                ));
            }
        }
    }
    Ok(())
}

fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "bool",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "array",
        Value::Object(_) => "object",
    }
}

pub(super) fn resolve(
    settings: Option<&ProjectSettingsManifest>,
    target: CanonicalTarget,
    generated_api_enabled: bool,
) -> Result<ResolvedProjectSettings, String> {
    let mut values = BTreeMap::new();
    if let Some(settings) = settings {
        for (key, definition) in &settings.definitions {
            if let Some(value) = definition.default.as_ref() {
                values.insert(key.clone(), typed_value(definition, value));
            }
        }
        if let Some(overrides) = settings.targets.get(target.as_str()) {
            for (key, value) in overrides {
                let definition = &settings.definitions[key];
                values.insert(key.clone(), typed_value(definition, value));
            }
        }
        for (key, definition) in &settings.definitions {
            if definition.required && !values.contains_key(key) {
                return Err(format!(
                    "required setting '{key}' has no default or exact '{}' override",
                    target.as_str()
                ));
            }
        }
    }
    let digest = settings_digest(&values);
    let configuration = ProjectConfiguration {
        target: target.as_str().to_string(),
        settings: values,
        digest,
        generated_api_enabled,
    };
    let generated_source = if generated_api_enabled {
        generated_source(&configuration)
    } else {
        String::new()
    };
    Ok(ResolvedProjectSettings {
        configuration,
        generated_source,
    })
}

fn typed_value(definition: &SettingDefinition, value: &Value) -> ProjectSettingValue {
    match definition.kind {
        SettingKind::String => ProjectSettingValue::String(
            value
                .as_str()
                .expect("validated string setting")
                .to_string(),
        ),
        SettingKind::Bool => {
            ProjectSettingValue::Bool(value.as_bool().expect("validated bool setting"))
        }
        SettingKind::Number => {
            ProjectSettingValue::Number(value.as_f64().expect("validated number setting"))
        }
    }
}

fn settings_digest(values: &BTreeMap<String, ProjectSettingValue>) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(b"stasis.project_settings.v1\0");
    for (key, value) in values {
        digest.update((key.len() as u64).to_le_bytes());
        digest.update(key.as_bytes());
        digest.update(value.kind().as_bytes());
        match value {
            ProjectSettingValue::String(value) => {
                digest.update((value.len() as u64).to_le_bytes());
                digest.update(value.as_bytes());
            }
            ProjectSettingValue::Bool(value) => digest.update([u8::from(*value)]),
            ProjectSettingValue::Number(value) => digest.update(value.to_bits().to_le_bytes()),
        }
    }
    digest.finalize().into()
}

fn generated_source(configuration: &ProjectConfiguration) -> String {
    let mut source = format!(
        "function @inline project_target(): string {{ return {}; }}\n",
        serde_json::to_string(&configuration.target).expect("target string encodes")
    );
    for (key, value) in &configuration.settings {
        let (kind, literal) = match value {
            ProjectSettingValue::String(value) => (
                "string",
                serde_json::to_string(value).expect("setting string encodes"),
            ),
            ProjectSettingValue::Bool(value) => ("bool", value.to_string()),
            ProjectSettingValue::Number(value) => {
                let mut literal = value.to_string();
                if !literal.contains(['.', 'e', 'E']) {
                    literal.push_str(".0");
                }
                ("f64", literal)
            }
        };
        source.push_str(&format!(
            "function @inline project_setting_{key}(): {kind} {{ return {literal}; }}\n"
        ));
    }
    source
}

pub(crate) fn provenance_summary(configuration: &ProjectConfiguration) -> Value {
    let settings = configuration
        .settings
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.kind().to_string())))
        .collect::<serde_json::Map<_, _>>();
    serde_json::json!({
        "target": configuration.target,
        "settings_sha256": hex_digest(configuration.digest),
        "settings": settings,
    })
}

pub(super) fn hex_digest(digest: [u8; 32]) -> String {
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_compiler::backend::aot::AotProcess;
    use stasis_compiler::backend::jit::JitProcess;
    use stasis_compiler::backend::wasm::WasmProcess;

    fn fixture() -> ProjectSettingsManifest {
        serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "definitions": {
                "channel": {"type": "string", "required": true, "allowed": ["base", "web"]},
                "feature": {"type": "bool", "default": false},
                "scale": {"type": "number", "minimum": 0.5, "maximum": 2.0, "default": 1.0}
            },
            "targets": {"web": {"channel": "web", "feature": true}}
        }))
        .unwrap()
    }

    #[test]
    fn strict_json_rejects_nested_duplicate_keys() {
        let error = parse_strict_json(
            br#"{"manifest_version":2,"settings":{"schema_version":1,"schema_version":1}}"#,
        )
        .unwrap_err();
        assert!(error.contains("duplicate JSON key 'schema_version'"));
    }

    #[test]
    fn published_schema_and_runtime_validation_agree_on_structural_limits() {
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../docs/project_settings.schema.json"
        ))
        .expect("published project settings schema parses");
        assert_eq!(
            schema["properties"]["definitions"]["maxProperties"],
            MAX_SETTING_COUNT
        );
        assert_eq!(
            schema["$defs"]["settingName"]["allOf"][0]["pattern"],
            "^[a-z][a-z0-9_]{0,63}$"
        );
        assert_eq!(
            schema["properties"]["targets"]["additionalProperties"]["propertyNames"]["$ref"],
            "#/$defs/settingName"
        );
        assert_eq!(
            schema["$defs"]["definition"]["allOf"][2]["then"]["required"],
            serde_json::json!(["minimum", "maximum"])
        );

        let invalid = [
            (
                serde_json::json!({
                    "schema_version": 1,
                    "definitions": {"api_token": {"type": "string"}}
                }),
                "must not contain secrets",
            ),
            (
                serde_json::json!({
                    "schema_version": 1,
                    "definitions": {"scale": {"type": "number"}}
                }),
                "must declare finite minimum and maximum",
            ),
            (
                serde_json::json!({
                    "schema_version": 1,
                    "definitions": {"enabled": {"type": "bool", "allowed": ["yes"]}}
                }),
                "cannot declare allowed, minimum, or maximum",
            ),
        ];
        for (value, expected) in invalid {
            let settings: ProjectSettingsManifest = serde_json::from_value(value).unwrap();
            let error = validate_manifest(&settings).unwrap_err();
            assert!(error.contains(expected), "{error}");
        }

        let oversized = "é".repeat((MAX_SETTING_STRING_BYTES / 2) + 1);
        let settings: ProjectSettingsManifest = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "definitions": {"label": {"type": "string", "default": oversized}}
        }))
        .unwrap();
        let error = validate_manifest(&settings).unwrap_err();
        assert!(error.contains("1024 UTF-8 bytes"), "{error}");
    }

    #[test]
    fn exact_target_override_resolves_typed_source_and_digest() {
        let settings = fixture();
        validate_manifest(&settings).unwrap();
        let web = resolve(Some(&settings), CanonicalTarget::Web, true).unwrap();
        assert_eq!(web.configuration.target, "web");
        assert_eq!(
            web.configuration.settings["channel"],
            ProjectSettingValue::String("web".to_string())
        );
        assert!(web
            .generated_source
            .contains("project_setting_feature(): bool"));
        assert!(web.generated_source.contains("return true"));
        let repeated = resolve(Some(&settings), CanonicalTarget::Web, true).unwrap();
        assert_eq!(web.configuration.digest, repeated.configuration.digest);
    }

    #[test]
    fn required_setting_is_checked_for_the_selected_target() {
        let settings = fixture();
        let error = resolve(Some(&settings), CanonicalTarget::WindowsX86_64, true).unwrap_err();
        assert_eq!(
            error,
            "required setting 'channel' has no default or exact 'windows-x86_64' override"
        );
    }

    #[test]
    fn provenance_contains_no_values() {
        let resolved = resolve(Some(&fixture()), CanonicalTarget::Web, true).unwrap();
        let provenance = provenance_summary(&resolved.configuration);
        let text = provenance.to_string();
        assert!(!text.contains("\"web\":true"));
        assert!(!text.contains("\"channel\":\"web\""));
        assert_eq!(provenance["settings"]["channel"], "string");
    }

    #[test]
    fn canonical_target_table_is_complete_and_stable() {
        assert_eq!(
            CanonicalTarget::ALL.map(CanonicalTarget::as_str),
            [
                "web",
                "windows-x86_64",
                "windows-arm64",
                "linux-x86_64",
                "linux-arm64",
                "macos-x86_64",
                "macos-arm64",
                "android-arm64",
                "android-x86_64",
                "ios-arm64",
            ]
        );
        let expected_host = if cfg!(all(target_os = "windows", target_arch = "aarch64")) {
            "windows-arm64"
        } else if cfg!(target_os = "windows") {
            "windows-x86_64"
        } else if cfg!(all(target_os = "macos", target_arch = "aarch64")) {
            "macos-arm64"
        } else if cfg!(target_os = "macos") {
            "macos-x86_64"
        } else if cfg!(target_arch = "aarch64") {
            "linux-arm64"
        } else {
            "linux-x86_64"
        };
        assert_eq!(CanonicalTarget::host().as_str(), expected_host);
    }

    #[test]
    fn generated_api_and_configuration_are_identical_across_backends() {
        let resolved = resolve(Some(&fixture()), CanonicalTarget::Web, true).unwrap();
        let roots = [
            "project_target".to_string(),
            "project_setting_channel".to_string(),
            "project_setting_feature".to_string(),
            "project_setting_scale".to_string(),
        ];

        let mut jit = JitProcess::new();
        jit.set_project_configuration(resolved.configuration.clone());
        jit.set_required_emit_roots(&roots);
        jit.upsert_file(GENERATED_SETTINGS_FILE, resolved.generated_source.clone());
        jit.compile()
            .expect("compile generated project API for JIT");
        assert_eq!(
            jit.execute_string_noarg_by_name("project_target").unwrap(),
            "web"
        );
        assert_eq!(
            jit.execute_string_noarg_by_name("project_setting_channel")
                .unwrap(),
            "web"
        );
        assert!(jit
            .execute_bool_noarg_by_name("project_setting_feature")
            .unwrap());

        let mut aot = AotProcess::new();
        aot.set_project_configuration(resolved.configuration.clone());
        aot.set_required_emit_roots(&roots);
        aot.upsert_file(GENERATED_SETTINGS_FILE, resolved.generated_source.clone());
        aot.compile()
            .expect("compile generated project API for AOT");

        let mut wasm = WasmProcess::new();
        wasm.set_project_configuration(resolved.configuration.clone());
        wasm.set_required_emit_roots(&roots);
        wasm.upsert_file(GENERATED_SETTINGS_FILE, resolved.generated_source);
        wasm.compile()
            .expect("compile generated project API for Wasm");

        for snapshot in [
            jit.program_snapshot().unwrap(),
            aot.program_snapshot().unwrap(),
            wasm.program_snapshot().unwrap(),
        ] {
            assert_eq!(
                snapshot.project_configuration(),
                Some(&resolved.configuration)
            );
        }
    }

    #[test]
    fn synthetic_settings_edge_avoids_user_aliases_comments_and_stale_cache() {
        let resolved = resolve(Some(&fixture()), CanonicalTarget::Web, true).unwrap();
        let main_path = "src/main.stasis";
        let user_module = "src/project_settings.stasis";
        let source = "// import \"/.stasis/generated/__stasis_project_settings_v1.stasis\";\nfunction marker(): string { return \"import \\\"/.stasis/generated/__stasis_project_settings_v1.stasis\\\";\"; }\nfunction main(): i32 { if (project_setting_feature()) { return 0; } return 1; }\n";
        let mut jit = JitProcess::new();
        jit.set_project_configuration(resolved.configuration.clone());
        jit.set_required_emit_roots(&["main".to_string()]);
        jit.upsert_file(GENERATED_SETTINGS_FILE, resolved.generated_source);
        jit.upsert_file(
            user_module,
            "function user_project_settings(): i32 { return 7; }\n",
        );
        jit.upsert_file(main_path, source);
        jit.compile()
            .expect("synthetic edge must not collide with a user project_settings module");
        let accepted_source = jit
            .program_snapshot()
            .unwrap()
            .files()
            .iter()
            .find(|file| file.path == main_path)
            .unwrap();
        assert_eq!(accepted_source.original_content, source);

        let explicit_import = "import   \"/.stasis/generated/__stasis_project_settings_v1.stasis\";\nfunction main(): i32 { if (project_setting_feature()) { return 0; } return 1; }\n";
        jit.upsert_file(main_path, explicit_import);
        jit.compile()
            .expect("a parsed explicit import must suppress the synthetic graph edge");
        let explicitly_imported_source = jit
            .program_snapshot()
            .unwrap()
            .files()
            .iter()
            .find(|file| file.path == main_path)
            .unwrap();
        assert_eq!(explicitly_imported_source.original_content, explicit_import);

        jit.retain_files(&BTreeSet::from([
            main_path.to_string(),
            user_module.to_string(),
        ]));
        let replacement = "// import \"/.stasis/generated/__stasis_project_settings_v1.stasis\";\nfunction main(): i32 { return 0; }\n";
        jit.upsert_file(main_path, replacement);
        jit.compile()
            .expect("removing the generated module must not leave a stale synthetic import");
        let refreshed_source = jit
            .program_snapshot()
            .unwrap()
            .files()
            .iter()
            .find(|file| file.path == main_path)
            .unwrap();
        assert_eq!(refreshed_source.original_content, replacement);
    }

    #[test]
    fn generated_api_names_are_reserved_for_v2_guests() {
        let resolved = resolve(Some(&fixture()), CanonicalTarget::Web, true).unwrap();
        let mut jit = JitProcess::new();
        jit.set_project_configuration(resolved.configuration);
        jit.set_required_emit_roots(&["main".to_string()]);
        jit.upsert_file(GENERATED_SETTINGS_FILE, resolved.generated_source);
        jit.upsert_file(
            "src/main.stasis",
            "function project_setting_feature(): bool { return false; }\nfunction main(): i32 { return 0; }\n",
        );
        let error = format!("{:?}", jit.compile().unwrap_err());
        assert!(error.contains("name reserved by the manifest v2 project settings API"));
    }
}
