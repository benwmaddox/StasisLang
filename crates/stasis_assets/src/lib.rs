use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Component, Path, PathBuf};

pub const ASSET_MANIFEST_SCHEMA: &str = "stasis-assets";
pub const ASSET_MANIFEST_VERSION: u32 = 2;
pub const DEFAULT_ASSET_MANIFEST_PATH: &str = "assets/manifest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssetLimits {
    pub max_manifest_bytes: u64,
    pub max_assets: usize,
    pub max_asset_bytes: u64,
}

impl Default for AssetLimits {
    fn default() -> Self {
        Self {
            max_manifest_bytes: 1024 * 1024,
            max_assets: 4096,
            max_asset_bytes: 64 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetManifest {
    pub schema: String,
    pub version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display: Option<AssetDisplay>,
    pub assets: Vec<AssetEntry>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetDisplay {
    pub logical_width: u32,
    pub logical_height: u32,
    pub max_physical_width: u32,
    pub max_physical_height: u32,
    #[serde(default)]
    pub scale_mode: AssetScaleMode,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetScaleMode {
    #[default]
    Fit,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AssetEntry {
    pub id: String,
    pub path: String,
    pub content_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepared_from_sha256: Option<String>,
    pub format: AssetFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prepare: Option<SpritePreparation>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dependencies: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpritePreparation {
    pub max_logical_width: u32,
    pub max_logical_height: u32,
    #[serde(default = "default_render_scale")]
    pub max_render_scale: f64,
}

fn default_render_scale() -> f64 {
    1.0
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AssetFormat {
    Sprite {
        encoding: SpriteEncoding,
        width: u32,
        height: u32,
    },
    Audio {
        encoding: AudioEncoding,
        sample_rate: u32,
        channels: u16,
        duration_frames: u64,
    },
    Font {
        encoding: FontEncoding,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpriteEncoding {
    Png,
    Svg,
    Jpeg,
    Webp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AudioEncoding {
    Wav,
    Ogg,
    Mp3,
    M4a,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontEncoding {
    Ttf,
    Otf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AssetHandle(u32);

impl AssetHandle {
    pub fn from_u32(value: u32) -> Option<Self> {
        (value != 0).then_some(Self(value))
    }

    pub fn from_i32(value: i32) -> Option<Self> {
        Self::from_u32(value as u32)
    }

    pub fn get(self) -> u32 {
        self.0
    }

    pub fn as_i32(self) -> i32 {
        self.0 as i32
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAsset {
    pub handle: AssetHandle,
    pub entry: AssetEntry,
    pub absolute_path: PathBuf,
    pub byte_length: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ResolvedAssetManifest {
    pub manifest_path: PathBuf,
    pub assets: Vec<ResolvedAsset>,
}

impl ResolvedAssetManifest {
    pub fn by_id(&self, id: &str) -> Option<&ResolvedAsset> {
        self.assets.iter().find(|asset| asset.entry.id == id)
    }

    pub fn by_handle(&self, handle: AssetHandle) -> Option<&ResolvedAsset> {
        self.assets.iter().find(|asset| asset.handle == handle)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetManifestError {
    pub code: &'static str,
    pub entry_id: Option<String>,
    pub path: Option<String>,
    pub detail: String,
}

impl fmt::Display for AssetManifestError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.code)?;
        if let Some(id) = &self.entry_id {
            write!(formatter, " [{id}]")?;
        }
        if let Some(path) = &self.path {
            write!(formatter, " ({path})")?;
        }
        write!(formatter, ": {}", self.detail)
    }
}

impl std::error::Error for AssetManifestError {}

pub fn stable_asset_handle(entry: &AssetEntry) -> AssetHandle {
    let kind = match entry.format {
        AssetFormat::Sprite { .. } => "sprite",
        AssetFormat::Audio { .. } => "audio",
        AssetFormat::Font { .. } => "font",
    };
    let mut hash = 2_166_136_261u32;
    for byte in kind.bytes().chain([b':']).chain(entry.id.bytes()) {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(16_777_619);
    }
    if hash == 0 {
        hash = 1;
    }
    AssetHandle(hash)
}

pub fn load_project_asset_manifest(
    project_root: impl AsRef<Path>,
    limits: AssetLimits,
) -> Result<ResolvedAssetManifest, AssetManifestError> {
    let root = project_root.as_ref().canonicalize().map_err(|error| {
        manifest_error(
            "asset_root_unavailable",
            None,
            Some(project_root.as_ref()),
            error.to_string(),
        )
    })?;
    let manifest_path = root
        .join(DEFAULT_ASSET_MANIFEST_PATH)
        .canonicalize()
        .map_err(|error| {
            manifest_error(
                "asset_manifest_missing",
                None,
                Some(Path::new(DEFAULT_ASSET_MANIFEST_PATH)),
                error.to_string(),
            )
        })?;
    if !manifest_path.starts_with(&root) || !manifest_path.is_file() {
        return Err(manifest_error(
            "asset_manifest_outside_project",
            None,
            Some(Path::new(DEFAULT_ASSET_MANIFEST_PATH)),
            "manifest must resolve to a file inside the project root",
        ));
    }
    let manifest_bytes = read_bounded(
        &manifest_path,
        limits.max_manifest_bytes,
        "asset_manifest_too_large",
    )?;
    let manifest: AssetManifest = serde_json::from_slice(&manifest_bytes).map_err(|error| {
        manifest_error(
            "asset_manifest_invalid_json",
            None,
            Some(&manifest_path),
            error.to_string(),
        )
    })?;
    validate_manifest_header(&manifest)?;
    validate_display(&manifest)?;
    if manifest.assets.len() > limits.max_assets {
        return Err(manifest_error(
            "asset_manifest_too_many_entries",
            None,
            Some(&manifest_path),
            format!(
                "{} entries exceeds limit {}",
                manifest.assets.len(),
                limits.max_assets
            ),
        ));
    }

    let mut ids = BTreeSet::new();
    let mut handles = BTreeMap::new();
    for entry in &manifest.assets {
        validate_entry(entry)?;
        if !ids.insert(entry.id.clone()) {
            return Err(entry_error(
                "asset_id_duplicate",
                entry,
                "asset IDs must be unique",
            ));
        }
        let handle = stable_asset_handle(entry);
        if let Some(other_id) = handles.insert(handle, entry.id.clone()) {
            return Err(entry_error(
                "asset_handle_collision",
                entry,
                format!("stable handle collides with asset {other_id}"),
            ));
        }
    }
    validate_dependencies(&manifest.assets, &ids)?;

    let mut resolved = Vec::with_capacity(manifest.assets.len());
    for entry in manifest.assets {
        let relative = validate_relative_asset_path(&entry.path)
            .map_err(|detail| entry_error("asset_path_invalid", &entry, detail))?;
        let candidate = root.join(relative);
        let absolute_path = candidate
            .canonicalize()
            .map_err(|error| entry_error("asset_file_missing", &entry, error.to_string()))?;
        if !absolute_path.starts_with(&root) || !absolute_path.is_file() {
            return Err(entry_error(
                "asset_path_outside_project",
                &entry,
                "asset must resolve to a file inside the project root",
            ));
        }
        let (content_sha256, byte_length) = sha256_file(&absolute_path, limits.max_asset_bytes)
            .map_err(|error| entry_error(error.0, &entry, error.1))?;
        if content_sha256 != entry.content_sha256 {
            return Err(entry_error(
                "asset_content_hash_mismatch",
                &entry,
                format!("expected {}, found {content_sha256}", entry.content_sha256),
            ));
        }
        resolved.push(ResolvedAsset {
            handle: stable_asset_handle(&entry),
            entry,
            absolute_path,
            byte_length,
        });
    }
    resolved.sort_by(|left, right| left.entry.id.cmp(&right.entry.id));
    Ok(ResolvedAssetManifest {
        manifest_path,
        assets: resolved,
    })
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

pub fn prepare_asset_bundle(
    resolved: &ResolvedAssetManifest,
    destination_root: impl AsRef<Path>,
    cache_root: impl AsRef<Path>,
) -> Result<PreparedAssetSummary, String> {
    let manifest_bytes = fs::read(&resolved.manifest_path)
        .map_err(|error| format!("failed to read source asset manifest: {error}"))?;
    let mut manifest: AssetManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|error| format!("failed to decode source asset manifest: {error}"))?;
    let destination_root = destination_root.as_ref();
    let cache_root = cache_root.as_ref();
    fs::create_dir_all(destination_root.join("assets"))
        .map_err(|error| format!("failed to create asset output: {error}"))?;
    fs::create_dir_all(cache_root)
        .map_err(|error| format!("failed to create asset cache: {error}"))?;

    let display_scale = manifest.display.as_ref().map(display_scale);
    let by_id = resolved
        .assets
        .iter()
        .map(|asset| (asset.entry.id.as_str(), asset))
        .collect::<BTreeMap<_, _>>();
    let mut summary = PreparedAssetSummary {
        copied_assets: 0,
        resized_assets: 0,
        cache_hits: 0,
    };

    for entry in &mut manifest.assets {
        let source = by_id
            .get(entry.id.as_str())
            .ok_or_else(|| format!("resolved asset missing from manifest: {}", entry.id))?;
        let destination = destination_root.join(&entry.path);
        fs::create_dir_all(destination.parent().expect("asset destination parent"))
            .map_err(|error| format!("failed to create asset directory: {error}"))?;
        let Some(prepare) = &entry.prepare else {
            fs::copy(&source.absolute_path, &destination)
                .map_err(|error| format!("failed to copy asset {}: {error}", entry.id))?;
            summary.copied_assets += 1;
            continue;
        };
        let Some(scale) = display_scale else {
            return Err(format!(
                "asset {} has prepare metadata without display metadata",
                entry.id
            ));
        };
        if !matches!(
            entry.format,
            AssetFormat::Sprite {
                encoding: SpriteEncoding::Png,
                ..
            }
        ) {
            fs::copy(&source.absolute_path, &destination)
                .map_err(|error| format!("failed to copy asset {}: {error}", entry.id))?;
            summary.copied_assets += 1;
            continue;
        }

        let target_width =
            prepared_axis(prepare.max_logical_width, scale, prepare.max_render_scale);
        let target_height =
            prepared_axis(prepare.max_logical_height, scale, prepare.max_render_scale);
        let (declared_width, declared_height) = match entry.format {
            AssetFormat::Sprite { width, height, .. } => (width, height),
            _ => unreachable!("PNG preparation requires a sprite entry"),
        };
        let ratio = f64::min(
            target_width as f64 / declared_width as f64,
            target_height as f64 / declared_height as f64,
        )
        .min(1.0);
        let output_width = ((declared_width as f64 * ratio).round() as u32).max(1);
        let output_height = ((declared_height as f64 * ratio).round() as u32).max(1);
        if output_width == declared_width && output_height == declared_height {
            validate_png_dimensions(source, declared_width, declared_height)?;
            fs::copy(&source.absolute_path, &destination)
                .map_err(|error| format!("failed to copy asset {}: {error}", entry.id))?;
            summary.copied_assets += 1;
            continue;
        }

        let cache_key = sha256_bytes(
            format!(
                "stasis-png-v2-linear-premultiplied-lanczos3:{}:{output_width}:{output_height}",
                source.entry.content_sha256
            )
            .as_bytes(),
        );
        let cached = cache_root.join(format!("{cache_key}.png"));
        if cached.is_file() {
            fs::copy(&cached, &destination)
                .map_err(|error| format!("failed to copy cached asset {}: {error}", entry.id))?;
            summary.cache_hits += 1;
        } else {
            let image = decode_png(source)?;
            if image.width() != declared_width || image.height() != declared_height {
                return Err(format!(
                    "PNG asset {} dimensions are {}x{}, manifest declares {declared_width}x{declared_height}",
                    entry.id,
                    image.width(),
                    image.height()
                ));
            }
            let resized = resize_png_high_quality(&image, output_width, output_height);
            let temporary = cache_root.join(format!("{cache_key}.{}.tmp", std::process::id()));
            strip_opaque_alpha(resized)
                .save_with_format(&temporary, image::ImageFormat::Png)
                .map_err(|error| format!("failed to encode PNG asset {}: {error}", entry.id))?;
            if cached.is_file() {
                fs::remove_file(&temporary).map_err(|error| {
                    format!(
                        "failed to remove duplicate cache file for {}: {error}",
                        entry.id
                    )
                })?;
            } else if let Err(error) = fs::rename(&temporary, &cached) {
                if cached.is_file() {
                    fs::remove_file(&temporary).map_err(|remove_error| {
                        format!(
                            "failed to resolve cache race for {} after {error}: {remove_error}",
                            entry.id
                        )
                    })?;
                } else {
                    return Err(format!(
                        "failed to publish cached asset {}: {error}",
                        entry.id
                    ));
                }
            }
            fs::copy(&cached, &destination)
                .map_err(|error| format!("failed to stage asset {}: {error}", entry.id))?;
        }
        let (output_hash, _) = sha256_file(&destination, u64::MAX)
            .map_err(|error| format!("failed to hash prepared asset {}: {}", entry.id, error.1))?;
        entry.prepared_from_sha256 = Some(entry.content_sha256.clone());
        entry.content_sha256 = output_hash;
        if let AssetFormat::Sprite { width, height, .. } = &mut entry.format {
            *width = output_width;
            *height = output_height;
        }
        summary.resized_assets += 1;
    }

    let generated = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("failed to encode prepared asset manifest: {error}"))?;
    fs::write(
        destination_root.join(DEFAULT_ASSET_MANIFEST_PATH),
        generated,
    )
    .map_err(|error| format!("failed to write prepared asset manifest: {error}"))?;
    Ok(summary)
}

fn decode_png(source: &ResolvedAsset) -> Result<image::DynamicImage, String> {
    image::ImageReader::open(&source.absolute_path)
        .map_err(|error| format!("failed to open PNG asset {}: {error}", source.entry.id))?
        .with_guessed_format()
        .map_err(|error| format!("failed to inspect PNG asset {}: {error}", source.entry.id))?
        .decode()
        .map_err(|error| format!("failed to decode PNG asset {}: {error}", source.entry.id))
}

fn validate_png_dimensions(
    source: &ResolvedAsset,
    declared_width: u32,
    declared_height: u32,
) -> Result<(), String> {
    let image = decode_png(source)?;
    if image.width() == declared_width && image.height() == declared_height {
        Ok(())
    } else {
        Err(format!(
            "PNG asset {} dimensions are {}x{}, manifest declares {declared_width}x{declared_height}",
            source.entry.id,
            image.width(),
            image.height()
        ))
    }
}

fn display_scale(display: &AssetDisplay) -> f64 {
    f64::min(
        display.max_physical_width as f64 / display.logical_width as f64,
        display.max_physical_height as f64 / display.logical_height as f64,
    )
}

fn prepared_axis(logical: u32, display_scale: f64, render_scale: f64) -> u32 {
    (logical as f64 * display_scale * render_scale)
        .ceil()
        .clamp(1.0, 16_384.0) as u32
}

fn resize_png_high_quality(
    image: &image::DynamicImage,
    output_width: u32,
    output_height: u32,
) -> image::DynamicImage {
    let source = image.to_rgba8();
    let linear_premultiplied =
        image::ImageBuffer::from_fn(source.width(), source.height(), |x, y| {
            let pixel = source.get_pixel(x, y);
            let alpha = f32::from(pixel[3]) / 255.0;
            image::Rgba([
                srgb_to_linear(f32::from(pixel[0]) / 255.0) * alpha,
                srgb_to_linear(f32::from(pixel[1]) / 255.0) * alpha,
                srgb_to_linear(f32::from(pixel[2]) / 255.0) * alpha,
                alpha,
            ])
        });
    let resized = image::imageops::resize(
        &linear_premultiplied,
        output_width,
        output_height,
        image::imageops::FilterType::Lanczos3,
    );
    let encoded = image::RgbaImage::from_fn(output_width, output_height, |x, y| {
        let pixel = resized.get_pixel(x, y);
        let alpha = pixel[3].clamp(0.0, 1.0);
        let encode = |channel: f32| {
            if alpha <= f32::EPSILON {
                0
            } else {
                normalized_to_u8(linear_to_srgb((channel / alpha).clamp(0.0, 1.0)))
            }
        };
        image::Rgba([
            encode(pixel[0]),
            encode(pixel[1]),
            encode(pixel[2]),
            normalized_to_u8(alpha),
        ])
    });
    image::DynamicImage::ImageRgba8(encoded)
}

fn srgb_to_linear(value: f32) -> f32 {
    if value <= 0.04045 {
        value / 12.92
    } else {
        ((value + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.powf(1.0 / 2.4) - 0.055
    }
}

fn normalized_to_u8(value: f32) -> u8 {
    (value * 255.0).round().clamp(0.0, 255.0) as u8
}

fn strip_opaque_alpha(image: image::DynamicImage) -> image::DynamicImage {
    if image.color().has_alpha() && image.to_rgba8().pixels().all(|pixel| pixel[3] == 255) {
        image::DynamicImage::ImageRgb8(image.to_rgb8())
    } else {
        image
    }
}

fn validate_manifest_header(manifest: &AssetManifest) -> Result<(), AssetManifestError> {
    if manifest.schema != ASSET_MANIFEST_SCHEMA {
        return Err(manifest_error(
            "asset_manifest_schema_unsupported",
            None,
            None,
            format!(
                "expected {ASSET_MANIFEST_SCHEMA}, found {}",
                manifest.schema
            ),
        ));
    }
    if !(1..=ASSET_MANIFEST_VERSION).contains(&manifest.version) {
        return Err(manifest_error(
            "asset_manifest_version_unsupported",
            None,
            None,
            format!(
                "expected version 1..={ASSET_MANIFEST_VERSION}, found {}",
                manifest.version
            ),
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreparedAssetSummary {
    pub copied_assets: usize,
    pub resized_assets: usize,
    pub cache_hits: usize,
}

fn validate_display(manifest: &AssetManifest) -> Result<(), AssetManifestError> {
    if manifest.version == 1
        && (manifest.display.is_some()
            || manifest.assets.iter().any(|entry| entry.prepare.is_some()))
    {
        return Err(manifest_error(
            "asset_display_requires_v2",
            None,
            None,
            "display preparation metadata requires manifest version 2",
        ));
    }
    if manifest.display.is_none() {
        if let Some(entry) = manifest.assets.iter().find(|entry| entry.prepare.is_some()) {
            return Err(entry_error(
                "asset_prepare_display_missing",
                entry,
                "sprite preparation requires top-level display metadata",
            ));
        }
    }
    if let Some(display) = &manifest.display {
        if display.logical_width == 0
            || display.logical_height == 0
            || display.max_physical_width == 0
            || display.max_physical_height == 0
            || display.max_physical_width > 16_384
            || display.max_physical_height > 16_384
        {
            return Err(manifest_error(
                "asset_display_dimensions_invalid",
                None,
                None,
                "display dimensions must be within 1..=16384",
            ));
        }
    }
    Ok(())
}

fn validate_entry(entry: &AssetEntry) -> Result<(), AssetManifestError> {
    if entry.id.is_empty()
        || entry.id.len() > 128
        || !entry
            .id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.'))
    {
        return Err(entry_error(
            "asset_id_invalid",
            entry,
            "ID must be 1-128 ASCII letters, digits, '.', '_' or '-'",
        ));
    }
    if entry.content_sha256.len() != 64
        || !entry
            .content_sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(entry_error(
            "asset_content_hash_invalid",
            entry,
            "content_sha256 must be 64 lowercase hexadecimal characters",
        ));
    }
    if let Some(hash) = &entry.prepared_from_sha256 {
        if hash.len() != 64
            || !hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(entry_error(
                "asset_prepared_source_hash_invalid",
                entry,
                "prepared_from_sha256 must be 64 lowercase hexadecimal characters",
            ));
        }
    }
    match entry.format {
        AssetFormat::Sprite {
            encoding,
            width,
            height,
        } => {
            if width == 0 || height == 0 || width > 16_384 || height > 16_384 {
                return Err(entry_error(
                    "asset_sprite_dimensions_invalid",
                    entry,
                    "sprite dimensions must be within 1..=16384",
                ));
            }
            validate_extension(entry, sprite_extension(encoding))?;
        }
        AssetFormat::Audio {
            encoding,
            sample_rate,
            channels,
            duration_frames,
        } => {
            if !(8_000..=384_000).contains(&sample_rate)
                || !(1..=8).contains(&channels)
                || duration_frames == 0
            {
                return Err(entry_error("asset_audio_metadata_invalid", entry, "audio requires sample rate 8000..=384000, channels 1..=8, and nonzero duration"));
            }
            validate_extension(entry, audio_extension(encoding))?;
        }
        AssetFormat::Font { encoding } => {
            validate_extension(entry, font_extension(encoding))?;
        }
    }
    if let Some(prepare) = &entry.prepare {
        if !matches!(entry.format, AssetFormat::Sprite { .. }) {
            return Err(entry_error(
                "asset_prepare_non_sprite",
                entry,
                "prepare metadata is only valid for sprites",
            ));
        }
        if prepare.max_logical_width == 0
            || prepare.max_logical_height == 0
            || !prepare.max_render_scale.is_finite()
            || !(1.0..=8.0).contains(&prepare.max_render_scale)
        {
            return Err(entry_error(
                "asset_prepare_invalid",
                entry,
                "logical dimensions must be nonzero and max_render_scale must be within 1.0..=8.0",
            ));
        }
    }
    Ok(())
}

fn validate_extension(entry: &AssetEntry, expected: &str) -> Result<(), AssetManifestError> {
    let actual = Path::new(&entry.path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("");
    if !(actual.eq_ignore_ascii_case(expected)
        || expected == "jpeg" && actual.eq_ignore_ascii_case("jpg"))
    {
        return Err(entry_error(
            "asset_format_extension_mismatch",
            entry,
            format!("declared format requires .{expected}"),
        ));
    }
    Ok(())
}

fn sprite_extension(encoding: SpriteEncoding) -> &'static str {
    match encoding {
        SpriteEncoding::Png => "png",
        SpriteEncoding::Svg => "svg",
        SpriteEncoding::Jpeg => "jpeg",
        SpriteEncoding::Webp => "webp",
    }
}

fn audio_extension(encoding: AudioEncoding) -> &'static str {
    match encoding {
        AudioEncoding::Wav => "wav",
        AudioEncoding::Ogg => "ogg",
        AudioEncoding::Mp3 => "mp3",
        AudioEncoding::M4a => "m4a",
    }
}

fn font_extension(encoding: FontEncoding) -> &'static str {
    match encoding {
        FontEncoding::Ttf => "ttf",
        FontEncoding::Otf => "otf",
    }
}

fn validate_relative_asset_path(path: &str) -> Result<PathBuf, &'static str> {
    if path.is_empty() || path.contains('\\') {
        return Err("asset path must use nonempty forward-slash project-relative syntax");
    }
    let parsed = Path::new(path);
    if parsed.is_absolute() || !path.starts_with("assets/") {
        return Err("asset path must be relative and remain under assets/");
    }
    if parsed
        .components()
        .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err("asset path cannot contain root, parent, or current-directory components");
    }
    Ok(parsed.to_path_buf())
}

fn validate_dependencies(
    entries: &[AssetEntry],
    ids: &BTreeSet<String>,
) -> Result<(), AssetManifestError> {
    let by_id = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect::<BTreeMap<_, _>>();
    for entry in entries {
        let mut unique = BTreeSet::new();
        for dependency in &entry.dependencies {
            if dependency == &entry.id || !unique.insert(dependency) {
                return Err(entry_error(
                    "asset_dependency_invalid",
                    entry,
                    format!("invalid dependency {dependency}"),
                ));
            }
            if !ids.contains(dependency) {
                return Err(entry_error(
                    "asset_dependency_missing",
                    entry,
                    format!("missing dependency {dependency}"),
                ));
            }
        }
    }
    let mut complete = BTreeSet::new();
    let mut active = BTreeSet::new();
    for id in &by_id {
        visit_dependencies(id.0, &by_id, &mut active, &mut complete)?;
    }
    Ok(())
}

fn visit_dependencies<'a>(
    id: &'a str,
    entries: &BTreeMap<&'a str, &'a AssetEntry>,
    active: &mut BTreeSet<&'a str>,
    complete: &mut BTreeSet<&'a str>,
) -> Result<(), AssetManifestError> {
    if complete.contains(id) {
        return Ok(());
    }
    if !active.insert(id) {
        return Err(entry_error(
            "asset_dependency_cycle",
            entries[id],
            "asset dependencies must be acyclic",
        ));
    }
    for dependency in &entries[id].dependencies {
        visit_dependencies(dependency, entries, active, complete)?;
    }
    active.remove(id);
    complete.insert(id);
    Ok(())
}

fn read_bounded(
    path: &Path,
    limit: u64,
    code: &'static str,
) -> Result<Vec<u8>, AssetManifestError> {
    let file = File::open(path).map_err(|error| {
        manifest_error(
            "asset_manifest_read_failed",
            None,
            Some(path),
            error.to_string(),
        )
    })?;
    let mut bytes = Vec::new();
    file.take(limit.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|error| {
            manifest_error(
                "asset_manifest_read_failed",
                None,
                Some(path),
                error.to_string(),
            )
        })?;
    if bytes.len() as u64 > limit {
        return Err(manifest_error(
            code,
            None,
            Some(path),
            format!("manifest exceeds limit {limit} bytes"),
        ));
    }
    Ok(bytes)
}

fn sha256_file(path: &Path, limit: u64) -> Result<(String, u64), (&'static str, String)> {
    let mut file =
        File::open(path).map_err(|error| ("asset_file_read_failed", error.to_string()))?;
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 32 * 1024];
    let mut length = 0u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| ("asset_file_read_failed", error.to_string()))?;
        if read == 0 {
            break;
        }
        length = length
            .checked_add(read as u64)
            .ok_or(("asset_file_too_large", "asset length overflow".to_string()))?;
        if length > limit {
            return Err((
                "asset_file_too_large",
                format!("asset exceeds {limit} bytes"),
            ));
        }
        digest.update(&buffer[..read]);
    }
    Ok((format!("{:x}", digest.finalize()), length))
}

fn entry_error(
    code: &'static str,
    entry: &AssetEntry,
    detail: impl Into<String>,
) -> AssetManifestError {
    manifest_error(code, Some(&entry.id), Some(Path::new(&entry.path)), detail)
}

fn manifest_error(
    code: &'static str,
    entry_id: Option<&str>,
    path: Option<&Path>,
    detail: impl Into<String>,
) -> AssetManifestError {
    AssetManifestError {
        code,
        entry_id: entry_id.map(str::to_string),
        path: path.map(|value| value.to_string_lossy().replace('\\', "/")),
        detail: detail.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn project(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("stasis_assets_{name}_{stamp}"));
        fs::create_dir_all(root.join("assets/images")).unwrap();
        fs::create_dir_all(root.join("assets/audio")).unwrap();
        fs::create_dir_all(root.join("assets/fonts")).unwrap();
        root
    }

    fn sprite(id: &str, path: &str, bytes: &[u8]) -> AssetEntry {
        AssetEntry {
            id: id.to_string(),
            path: path.to_string(),
            content_sha256: sha256_bytes(bytes),
            prepared_from_sha256: None,
            format: AssetFormat::Sprite {
                encoding: SpriteEncoding::Png,
                width: 2,
                height: 3,
            },
            prepare: None,
            dependencies: vec![],
        }
    }

    fn write_manifest(root: &Path, entries: Vec<AssetEntry>) {
        let manifest = AssetManifest {
            schema: ASSET_MANIFEST_SCHEMA.to_string(),
            version: ASSET_MANIFEST_VERSION,
            display: None,
            assets: entries,
        };
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
    }

    #[test]
    fn resolves_and_prepares_declared_font_assets() {
        let root = project("font");
        let bytes = b"test font bytes";
        fs::write(root.join("assets/fonts/ui.ttf"), bytes).unwrap();
        write_manifest(
            &root,
            vec![AssetEntry {
                id: "ui".to_string(),
                path: "assets/fonts/ui.ttf".to_string(),
                content_sha256: sha256_bytes(bytes),
                prepared_from_sha256: None,
                format: AssetFormat::Font {
                    encoding: FontEncoding::Ttf,
                },
                prepare: None,
                dependencies: vec![],
            }],
        );

        let resolved = load_project_asset_manifest(&root, AssetLimits::default()).unwrap();
        assert_eq!(resolved.assets[0].entry.id, "ui");
        assert_eq!(
            prepare_asset_bundle(&resolved, root.join("output"), root.join("cache"))
                .unwrap()
                .copied_assets,
            1
        );
        assert_eq!(
            fs::read(root.join("output/assets/fonts/ui.ttf")).unwrap(),
            bytes
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn resolves_sorted_assets_with_stable_handles_and_hashes() {
        let root = project("resolve");
        let ball = b"ball png";
        let paddle = b"paddle png";
        fs::write(root.join("assets/images/ball.png"), ball).unwrap();
        fs::write(root.join("assets/images/paddle.png"), paddle).unwrap();
        let mut paddle_entry = sprite("paddle", "assets/images/paddle.png", paddle);
        paddle_entry.dependencies.push("ball".to_string());
        let ball_entry = sprite("ball", "assets/images/ball.png", ball);
        let expected_handle = stable_asset_handle(&ball_entry);
        assert_eq!(expected_handle.get(), 0xa55f_97e3);
        assert_eq!(
            AssetHandle::from_i32(expected_handle.as_i32()),
            Some(expected_handle)
        );
        assert_eq!(AssetHandle::from_u32(0), None);
        write_manifest(&root, vec![paddle_entry, ball_entry]);

        let resolved = load_project_asset_manifest(&root, AssetLimits::default()).unwrap();
        assert_eq!(
            resolved
                .assets
                .iter()
                .map(|asset| asset.entry.id.as_str())
                .collect::<Vec<_>>(),
            vec!["ball", "paddle"]
        );
        assert_eq!(resolved.by_id("ball").unwrap().handle, expected_handle);
        assert_eq!(
            resolved.by_handle(expected_handle).unwrap().byte_length,
            ball.len() as u64
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_paths_outside_project_and_hash_mismatches() {
        let root = project("deny");
        let bytes = b"image";
        fs::write(root.join("assets/images/ball.png"), bytes).unwrap();
        let mut entry = sprite("ball", "../ball.png", bytes);
        write_manifest(&root, vec![entry.clone()]);
        assert_eq!(
            load_project_asset_manifest(&root, AssetLimits::default())
                .unwrap_err()
                .code,
            "asset_path_invalid"
        );

        entry.path = "assets/images/ball.png".to_string();
        entry.content_sha256 = "0".repeat(64);
        write_manifest(&root, vec![entry]);
        assert_eq!(
            load_project_asset_manifest(&root, AssetLimits::default())
                .unwrap_err()
                .code,
            "asset_content_hash_mismatch"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_missing_and_cyclic_dependencies() {
        let root = project("deps");
        let bytes = b"image";
        fs::write(root.join("assets/images/a.png"), bytes).unwrap();
        fs::write(root.join("assets/images/b.png"), bytes).unwrap();
        let mut a = sprite("a", "assets/images/a.png", bytes);
        a.dependencies.push("missing".to_string());
        write_manifest(&root, vec![a.clone()]);
        assert_eq!(
            load_project_asset_manifest(&root, AssetLimits::default())
                .unwrap_err()
                .code,
            "asset_dependency_missing"
        );

        let mut b = sprite("b", "assets/images/b.png", bytes);
        a.dependencies = vec!["b".to_string()];
        b.dependencies = vec!["a".to_string()];
        write_manifest(&root, vec![a, b]);
        assert_eq!(
            load_project_asset_manifest(&root, AssetLimits::default())
                .unwrap_err()
                .code,
            "asset_dependency_cycle"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn rejects_future_versions_and_oversized_assets() {
        let root = project("limits");
        let bytes = b"image";
        fs::write(root.join("assets/images/ball.png"), bytes).unwrap();
        let entry = sprite("ball", "assets/images/ball.png", bytes);
        write_manifest(&root, vec![entry]);
        let limits = AssetLimits {
            max_asset_bytes: 2,
            ..AssetLimits::default()
        };
        assert_eq!(
            load_project_asset_manifest(&root, limits).unwrap_err().code,
            "asset_file_too_large"
        );

        let mut json: serde_json::Value =
            serde_json::from_slice(&fs::read(root.join(DEFAULT_ASSET_MANIFEST_PATH)).unwrap())
                .unwrap();
        json["version"] = serde_json::json!(ASSET_MANIFEST_VERSION + 1);
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec(&json).unwrap(),
        )
        .unwrap();
        assert_eq!(
            load_project_asset_manifest(&root, AssetLimits::default())
                .unwrap_err()
                .code,
            "asset_manifest_version_unsupported"
        );
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prepares_png_for_logical_display_size_and_reuses_cache() {
        let root = project("prepare_png");
        let source_path = root.join("assets/images/hero.png");
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            800,
            800,
            image::Rgba([20, 40, 60, 255]),
        ))
        .save_with_format(&source_path, image::ImageFormat::Png)
        .unwrap();
        let source_bytes = fs::read(&source_path).unwrap();
        let mut hero = sprite("hero", "assets/images/hero.png", &source_bytes);
        hero.format = AssetFormat::Sprite {
            encoding: SpriteEncoding::Png,
            width: 800,
            height: 800,
        };
        hero.prepare = Some(SpritePreparation {
            max_logical_width: 50,
            max_logical_height: 50,
            max_render_scale: 1.0,
        });
        let manifest = AssetManifest {
            schema: ASSET_MANIFEST_SCHEMA.to_string(),
            version: ASSET_MANIFEST_VERSION,
            display: Some(AssetDisplay {
                logical_width: 100,
                logical_height: 200,
                max_physical_width: 400,
                max_physical_height: 1000,
                scale_mode: AssetScaleMode::Fit,
            }),
            assets: vec![hero],
        };
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();
        let resolved = load_project_asset_manifest(&root, AssetLimits::default()).unwrap();
        let output = root.join("output");
        let cache = root.join("cache");

        let first = prepare_asset_bundle(&resolved, &output, &cache).unwrap();
        assert_eq!(first.resized_assets, 1);
        assert_eq!(first.cache_hits, 0);
        let prepared = image::open(output.join("assets/images/hero.png")).unwrap();
        assert_eq!((prepared.width(), prepared.height()), (200, 200));
        assert_eq!(prepared.color(), image::ColorType::Rgb8);
        let generated: AssetManifest =
            serde_json::from_slice(&fs::read(output.join(DEFAULT_ASSET_MANIFEST_PATH)).unwrap())
                .unwrap();
        assert_eq!(
            generated.assets[0].prepared_from_sha256,
            Some(sha256_bytes(&source_bytes))
        );
        assert_ne!(
            generated.assets[0].content_sha256,
            sha256_bytes(&source_bytes)
        );

        let second = prepare_asset_bundle(&resolved, &output, &cache).unwrap();
        assert_eq!(second.resized_assets, 1);
        assert_eq!(second.cache_hits, 1);
        fs::remove_dir_all(root).ok();
    }

    #[test]
    fn prepared_png_resizes_in_linear_light() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            let value = if x == 0 { 0 } else { 255 };
            image::Rgba([value, value, value, 255])
        }));

        let resized = resize_png_high_quality(&source, 1, 1).to_rgba8();
        let value = resized.get_pixel(0, 0)[0];

        assert!(
            (180..=195).contains(&value),
            "linear-light midpoint should be near 188, found {value}"
        );
    }

    #[test]
    fn prepared_png_resizes_premultiplied_alpha_without_color_halos() {
        let source = image::DynamicImage::ImageRgba8(image::RgbaImage::from_fn(2, 1, |x, _| {
            if x == 0 {
                image::Rgba([255, 0, 0, 255])
            } else {
                image::Rgba([0, 0, 255, 0])
            }
        }));

        let resized = resize_png_high_quality(&source, 1, 1).to_rgba8();
        let pixel = resized.get_pixel(0, 0);

        assert!(pixel[0] >= 250, "opaque color should remain red: {pixel:?}");
        assert!(
            pixel[2] <= 5,
            "transparent blue must not bleed in: {pixel:?}"
        );
        assert!(
            (120..=135).contains(&pixel[3]),
            "alpha should average: {pixel:?}"
        );
    }
}
