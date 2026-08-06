use image::{DynamicImage, ImageFormat, Rgba, RgbaImage};
use serde_json::Value;
use sha2::{Digest, Sha256};
use stasis_ai::ToolCall;
use stasis_assets::{
    load_project_asset_manifest, AssetEntry, AssetFormat, AssetLimits, AssetManifest,
    AudioEncoding, SpriteEncoding, DEFAULT_ASSET_MANIFEST_PATH,
};
use std::collections::BTreeSet;
use std::fs;
use std::io::Cursor;
use std::path::{Component, Path, PathBuf};

const MAX_TEXT_ASSET_BYTES: usize = 256 * 1024;
const MAX_PNG_ASSET_BYTES: u64 = 16 * 1024 * 1024;

pub(crate) struct AppliedAssetTransaction {
    backups: Vec<(PathBuf, Option<Vec<u8>>)>,
}

impl AppliedAssetTransaction {
    pub(crate) fn rollback(self) -> Result<(), String> {
        for (path, prior) in self.backups.into_iter().rev() {
            match prior {
                Some(bytes) => fs::write(&path, bytes)
                    .map_err(|error| format!("failed restoring {}: {error}", path.display()))?,
                None if path.exists() => fs::remove_file(&path)
                    .map_err(|error| format!("failed removing {}: {error}", path.display()))?,
                None => {}
            }
        }
        Ok(())
    }
}

pub(crate) fn apply_asset_calls(
    project_root: &Path,
    calls: &[&ToolCall],
) -> Result<AppliedAssetTransaction, String> {
    let manifest_path = project_root.join(DEFAULT_ASSET_MANIFEST_PATH);
    let manifest_source = fs::read(&manifest_path)
        .map_err(|error| format!("failed reading asset manifest: {error}"))?;
    let mut manifest: AssetManifest = serde_json::from_slice(&manifest_source)
        .map_err(|error| format!("invalid asset manifest: {error}"))?;
    let mut writes = Vec::<(PathBuf, Vec<u8>)>::new();
    let mut deletes = Vec::<PathBuf>::new();
    let mut touched = BTreeSet::new();
    for call in calls {
        let args = call
            .args
            .as_object()
            .ok_or_else(|| "asset args must be an object".to_string())?;
        let relative = required_string(args, "path")?;
        let path = controlled_asset_path(project_root, &relative, call.tool.as_str())?;
        if !touched.insert(path.clone()) {
            return Err(format!("asset path appears more than once: {relative}"));
        }
        match call.tool.as_str() {
            "write_svg_asset" => {
                let id = controlled_id(&required_string(args, "id")?)?;
                let source = required_string(args, "source")?;
                let width = required_u32(args, "width", 1, 4096)?;
                let height = required_u32(args, "height", 1, 4096)?;
                validate_svg(&source)?;
                let bytes = source.into_bytes();
                upsert_entry(
                    &mut manifest,
                    AssetEntry {
                        id,
                        path: relative.replace('\\', "/"),
                        content_sha256: sha256(&bytes),
                        prepared_from_sha256: None,
                        format: AssetFormat::Sprite {
                            encoding: SpriteEncoding::Svg,
                            width,
                            height,
                        },
                        prepare: None,
                        dependencies: Vec::new(),
                    },
                );
                writes.push((path, bytes));
            }
            "write_png_asset" => {
                let id = controlled_id(&required_string(args, "id")?)?;
                let width = required_u32(args, "width", 1, 2048)?;
                let height = required_u32(args, "height", 1, 2048)?;
                if u64::from(width) * u64::from(height) > 4_194_304 {
                    return Err("PNG asset exceeds the 4,194,304-pixel limit".to_string());
                }
                let background = parse_color(&required_string(args, "background")?)?;
                let shapes = args
                    .get("shapes")
                    .and_then(Value::as_array)
                    .ok_or_else(|| "PNG asset shapes must be an array".to_string())?;
                if shapes.len() > 512 {
                    return Err("PNG asset supports at most 512 shapes".to_string());
                }
                let bytes = render_png(width, height, background, shapes)?;
                upsert_entry(
                    &mut manifest,
                    AssetEntry {
                        id,
                        path: relative.replace('\\', "/"),
                        content_sha256: sha256(&bytes),
                        prepared_from_sha256: None,
                        format: AssetFormat::Sprite {
                            encoding: SpriteEncoding::Png,
                            width,
                            height,
                        },
                        prepare: None,
                        dependencies: Vec::new(),
                    },
                );
                writes.push((path, bytes));
            }
            "import_png_asset" => {
                let id = controlled_id(&required_string(args, "id")?)?;
                let source = required_string(args, "source_path")?;
                let (source_bytes, source_width, source_height) =
                    load_imagegen_png(project_root, &source)?;
                let source_hash = sha256(&source_bytes);
                let (bytes, width, height, transformed) =
                    transform_imported_png(args, source_bytes, source_width, source_height)?;
                upsert_entry(
                    &mut manifest,
                    AssetEntry {
                        id,
                        path: relative.replace('\\', "/"),
                        content_sha256: sha256(&bytes),
                        prepared_from_sha256: transformed.then_some(source_hash),
                        format: AssetFormat::Sprite {
                            encoding: SpriteEncoding::Png,
                            width,
                            height,
                        },
                        prepare: None,
                        dependencies: Vec::new(),
                    },
                );
                writes.push((path, bytes));
            }
            "delete_asset" => {
                let normalized = relative.replace('\\', "/");
                let id = args
                    .get("id")
                    .map(|value| {
                        value
                            .as_str()
                            .ok_or_else(|| "delete_asset id must be a string".to_string())
                            .and_then(controlled_id)
                    })
                    .transpose()?;
                if let Some(id) = id.as_ref() {
                    let entry = manifest
                        .assets
                        .iter()
                        .find(|entry| &entry.id == id)
                        .ok_or_else(|| format!("asset manifest has no id: {id}"))?;
                    if entry.path != normalized {
                        return Err(format!(
                            "asset id {id} points to {}, not {normalized}",
                            entry.path
                        ));
                    }
                    manifest.assets.retain(|entry| &entry.id != id);
                } else {
                    manifest.assets.retain(|entry| entry.path != normalized);
                }
                let metadata = path
                    .symlink_metadata()
                    .map_err(|error| format!("failed reading obsolete asset: {error}"))?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err("delete_asset requires a regular non-symlink file".to_string());
                }
                deletes.push(path);
            }
            "write_data_asset" => {
                let source = required_string(args, "source")?;
                if source.len() > MAX_TEXT_ASSET_BYTES {
                    return Err("data asset exceeds 256 KiB".to_string());
                }
                if path
                    .extension()
                    .is_some_and(|value| value.eq_ignore_ascii_case("json"))
                {
                    serde_json::from_str::<Value>(&source)
                        .map_err(|error| format!("invalid JSON data asset: {error}"))?;
                }
                writes.push((path, source.into_bytes()));
            }
            "write_procedural_wav" => {
                let id = controlled_id(&required_string(args, "id")?)?;
                let frequency = required_u32(args, "frequency_hz", 20, 8_000)?;
                let duration_ms = required_u32(args, "duration_ms", 20, 5_000)?;
                let (bytes, frames) = procedural_wav(frequency, duration_ms);
                upsert_entry(
                    &mut manifest,
                    AssetEntry {
                        id,
                        path: relative.replace('\\', "/"),
                        content_sha256: sha256(&bytes),
                        prepared_from_sha256: None,
                        format: AssetFormat::Audio {
                            encoding: AudioEncoding::Wav,
                            sample_rate: 44_100,
                            channels: 1,
                            duration_frames: frames,
                        },
                        prepare: None,
                        dependencies: Vec::new(),
                    },
                );
                writes.push((path, bytes));
            }
            _ => return Err(format!("unsupported Gauntlet asset tool: {}", call.tool)),
        }
    }
    let mut manifest_bytes = serde_json::to_vec_pretty(&manifest)
        .map_err(|error| format!("failed encoding asset manifest: {error}"))?;
    manifest_bytes.push(b'\n');
    writes.push((manifest_path, manifest_bytes));
    let mut backups = Vec::with_capacity(writes.len() + deletes.len());
    for (path, bytes) in &writes {
        if path
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            restore(&backups)?;
            return Err(format!(
                "refusing to write through asset symlink: {}",
                path.display()
            ));
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed creating asset directory: {error}"))?;
        }
        let prior = fs::read(path).ok();
        backups.push((path.clone(), prior));
        if let Err(error) = fs::write(path, bytes) {
            restore(&backups)?;
            return Err(format!("failed staging asset {}: {error}", path.display()));
        }
    }
    for path in &deletes {
        let prior = fs::read(path).map_err(|error| {
            format!("failed reading obsolete asset {}: {error}", path.display())
        })?;
        backups.push((path.clone(), Some(prior)));
        if let Err(error) = fs::remove_file(path) {
            restore(&backups)?;
            return Err(format!(
                "failed deleting obsolete asset {}: {error}",
                path.display()
            ));
        }
    }
    if let Err(error) = load_project_asset_manifest(project_root, AssetLimits::default()) {
        restore(&backups)?;
        return Err(format!("asset transaction validation failed: {error}"));
    }
    if let Err(error) = sync_prepared_assets(project_root, &writes, &mut backups) {
        restore(&backups)?;
        return Err(error);
    }
    if let Err(error) = sync_deleted_assets(project_root, &deletes, &mut backups) {
        restore(&backups)?;
        return Err(error);
    }
    Ok(AppliedAssetTransaction { backups })
}

fn sync_deleted_assets(
    project_root: &Path,
    deletes: &[PathBuf],
    backups: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) -> Result<(), String> {
    let prepared = project_root.join(".stasis_cache/play-assets");
    if !prepared.is_dir() {
        return Ok(());
    }
    for source in deletes {
        let relative = source
            .strip_prefix(project_root)
            .map_err(|_| "asset transaction escaped the project".to_string())?;
        let destination = prepared.join(relative);
        if !destination.exists() {
            continue;
        }
        let metadata = destination
            .symlink_metadata()
            .map_err(|error| format!("failed reading prepared obsolete asset: {error}"))?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err("prepared obsolete asset must be a regular non-symlink file".to_string());
        }
        backups.push((
            destination.clone(),
            Some(
                fs::read(&destination).map_err(|error| {
                    format!("failed backing up prepared obsolete asset: {error}")
                })?,
            ),
        ));
        fs::remove_file(&destination)
            .map_err(|error| format!("failed deleting prepared obsolete asset: {error}"))?;
    }
    Ok(())
}

fn sync_prepared_assets(
    project_root: &Path,
    writes: &[(PathBuf, Vec<u8>)],
    backups: &mut Vec<(PathBuf, Option<Vec<u8>>)>,
) -> Result<(), String> {
    let prepared = project_root.join(".stasis_cache/play-assets");
    if !prepared.is_dir() {
        return Ok(());
    }
    for (source, bytes) in writes {
        let relative = source
            .strip_prefix(project_root)
            .map_err(|_| "asset transaction escaped the project".to_string())?;
        let destination = prepared.join(relative);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed creating prepared asset directory: {error}"))?;
        }
        backups.push((destination.clone(), fs::read(&destination).ok()));
        fs::write(&destination, bytes)
            .map_err(|error| format!("failed synchronizing prepared asset: {error}"))?;
    }
    Ok(())
}

fn restore(backups: &[(PathBuf, Option<Vec<u8>>)]) -> Result<(), String> {
    for (path, prior) in backups.iter().rev() {
        match prior {
            Some(bytes) => fs::write(path, bytes)
                .map_err(|error| format!("failed rolling back {}: {error}", path.display()))?,
            None if path.exists() => fs::remove_file(path)
                .map_err(|error| format!("failed rolling back {}: {error}", path.display()))?,
            None => {}
        }
    }
    Ok(())
}

fn controlled_asset_path(root: &Path, relative: &str, tool: &str) -> Result<PathBuf, String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !relative.starts_with("assets/generated")
    {
        return Err("Gauntlet assets must be normal paths under assets/generated".to_string());
    }
    let valid_extension = match tool {
        "write_svg_asset" => relative
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("svg")),
        "write_png_asset" | "import_png_asset" => relative
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("png")),
        "write_data_asset" => relative.extension().is_some_and(|value| {
            value.eq_ignore_ascii_case("json") || value.eq_ignore_ascii_case("csv")
        }),
        "write_procedural_wav" => relative
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("wav")),
        "delete_asset" => relative.extension().is_some_and(|value| {
            value.eq_ignore_ascii_case("svg")
                || value.eq_ignore_ascii_case("png")
                || value.eq_ignore_ascii_case("json")
                || value.eq_ignore_ascii_case("csv")
                || value.eq_ignore_ascii_case("wav")
        }),
        _ => false,
    };
    if !valid_extension {
        return Err(format!("invalid file extension for {tool}"));
    }
    Ok(root.join(relative))
}

pub(crate) fn load_imagegen_png(
    root: &Path,
    relative: &str,
) -> Result<(Vec<u8>, u32, u32), String> {
    let relative = Path::new(relative);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
        || !(relative.starts_with("build/ai-assets/imagegen")
            || relative.starts_with("build/gauntlet/imagegen"))
        || !relative
            .extension()
            .is_some_and(|value| value.eq_ignore_ascii_case("png"))
    {
        return Err(
            "ImageGen input must be a normal PNG path under build/ai-assets/imagegen or build/gauntlet/imagegen"
                .to_string(),
        );
    }
    let path = root.join(relative);
    let metadata = path
        .symlink_metadata()
        .map_err(|error| format!("failed reading ImageGen PNG {}: {error}", path.display()))?;
    if !metadata.is_file()
        || metadata.file_type().is_symlink()
        || metadata.len() > MAX_PNG_ASSET_BYTES
    {
        return Err(
            "ImageGen PNG must be a regular non-symlink file no larger than 16 MiB".to_string(),
        );
    }
    let bytes = fs::read(&path)
        .map_err(|error| format!("failed reading ImageGen PNG {}: {error}", path.display()))?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    limits.max_alloc = Some(32 * 1024 * 1024);
    let mut reader = image::ImageReader::with_format(Cursor::new(&bytes), ImageFormat::Png);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("invalid or oversized ImageGen PNG: {error}"))?;
    let width = decoded.width();
    let height = decoded.height();
    if u64::from(width) * u64::from(height) > 4_194_304 {
        return Err("ImageGen PNG exceeds the 4,194,304-pixel limit".to_string());
    }
    Ok((bytes, width, height))
}

fn transform_imported_png(
    args: &serde_json::Map<String, Value>,
    bytes: Vec<u8>,
    source_width: u32,
    source_height: u32,
) -> Result<(Vec<u8>, u32, u32, bool), String> {
    let crop_names = ["crop_x", "crop_y", "crop_width", "crop_height"];
    let crop_count = crop_names
        .iter()
        .filter(|name| args.contains_key(**name))
        .count();
    if !matches!(crop_count, 0 | 4) {
        return Err(
            "PNG crop requires crop_x, crop_y, crop_width, and crop_height together".to_string(),
        );
    }
    let transparent = args.get("transparent_color").is_some();
    if !transparent && args.contains_key("transparent_tolerance") {
        return Err("transparent_tolerance requires transparent_color".to_string());
    }
    if crop_count == 0 && !transparent {
        return Ok((bytes, source_width, source_height, false));
    }
    let decoded = image::load_from_memory_with_format(&bytes, ImageFormat::Png)
        .map_err(|error| format!("failed decoding PNG for transformation: {error}"))?;
    let mut image = decoded.to_rgba8();
    if crop_count == 4 {
        let x = required_u32(args, "crop_x", 0, source_width.saturating_sub(1))?;
        let y = required_u32(args, "crop_y", 0, source_height.saturating_sub(1))?;
        let width = required_u32(args, "crop_width", 1, source_width)?;
        let height = required_u32(args, "crop_height", 1, source_height)?;
        if x.saturating_add(width) > source_width || y.saturating_add(height) > source_height {
            return Err("PNG crop rectangle exceeds the source bounds".to_string());
        }
        image = image::imageops::crop_imm(&image, x, y, width, height).to_image();
    }
    if transparent {
        let key = parse_color(
            args.get("transparent_color")
                .and_then(Value::as_str)
                .ok_or_else(|| "transparent_color must be #RRGGBB".to_string())?,
        )?;
        let tolerance = args
            .get("transparent_tolerance")
            .map(|_| required_u32(args, "transparent_tolerance", 0, 255))
            .transpose()?
            .unwrap_or(12) as u8;
        let tolerance = adaptive_chroma_tolerance(&image, key, tolerance);
        let mut transparent_pixels = 0_u64;
        let mut opaque_pixels = 0_u64;
        for pixel in image.pixels_mut() {
            if chroma_distance(*pixel, key) <= tolerance {
                pixel[3] = 0;
            }
            if pixel[3] == 0 {
                transparent_pixels += 1;
            } else {
                opaque_pixels += 1;
            }
        }
        let pixel_count = u64::from(image.width()) * u64::from(image.height());
        let minimum_coverage = (pixel_count / 100).max(1);
        let border_is_transparent = (0..image.width()).all(|x| {
            image.get_pixel(x, 0)[3] == 0 && image.get_pixel(x, image.height() - 1)[3] == 0
        }) && (0..image.height()).all(|y| {
            image.get_pixel(0, y)[3] == 0 && image.get_pixel(image.width() - 1, y)[3] == 0
        });
        if transparent_pixels < minimum_coverage || !border_is_transparent {
            return Err(format!(
                "PNG background removal left the isolated-subject border opaque; increase transparent_tolerance or request a flatter chroma background (effective tolerance {tolerance})"
            ));
        }
        if opaque_pixels < minimum_coverage {
            return Err(
                "PNG background removal erased nearly the entire subject; use a chroma color absent from the subject or lower transparent_tolerance"
                    .to_string(),
            );
        }
    }
    let width = image.width();
    let height = image.height();
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| format!("failed encoding transformed PNG asset: {error}"))?;
    Ok((output.into_inner(), width, height, true))
}

fn chroma_distance(pixel: Rgba<u8>, key: Rgba<u8>) -> u8 {
    pixel[0]
        .abs_diff(key[0])
        .max(pixel[1].abs_diff(key[1]))
        .max(pixel[2].abs_diff(key[2]))
}

fn adaptive_chroma_tolerance(image: &RgbaImage, key: Rgba<u8>, requested: u8) -> u8 {
    let width = image.width();
    let height = image.height();
    let mut border_distances = Vec::with_capacity(((width + height) * 2) as usize);
    for x in 0..width {
        border_distances.push(chroma_distance(*image.get_pixel(x, 0), key));
        if height > 1 {
            border_distances.push(chroma_distance(*image.get_pixel(x, height - 1), key));
        }
    }
    for y in 1..height.saturating_sub(1) {
        border_distances.push(chroma_distance(*image.get_pixel(0, y), key));
        if width > 1 {
            border_distances.push(chroma_distance(*image.get_pixel(width - 1, y), key));
        }
    }
    let border_tolerance = border_distances.into_iter().max().unwrap_or(requested);
    if border_tolerance <= 64 {
        requested.max(border_tolerance.saturating_add(8))
    } else {
        requested
    }
}

fn render_png(
    width: u32,
    height: u32,
    background: Rgba<u8>,
    shapes: &[Value],
) -> Result<Vec<u8>, String> {
    let mut image = RgbaImage::from_pixel(width, height, background);
    for (index, shape) in shapes.iter().enumerate() {
        let shape = shape
            .as_object()
            .ok_or_else(|| format!("PNG shape {index} must be an object"))?;
        let kind = shape
            .get("kind")
            .and_then(Value::as_str)
            .ok_or_else(|| format!("PNG shape {index} requires kind"))?;
        let color = parse_color(
            shape
                .get("color")
                .and_then(Value::as_str)
                .ok_or_else(|| format!("PNG shape {index} requires color"))?,
        )?;
        match kind {
            "rect" => draw_rect(
                &mut image,
                shape_i32(shape, "x", index)?,
                shape_i32(shape, "y", index)?,
                shape_u32(shape, "width", index, 1, 4096)?,
                shape_u32(shape, "height", index, 1, 4096)?,
                color,
            ),
            "circle" => draw_circle(
                &mut image,
                shape_i32(shape, "x", index)?,
                shape_i32(shape, "y", index)?,
                shape_u32(shape, "radius", index, 1, 2048)? as i32,
                color,
            ),
            "line" => draw_line(
                &mut image,
                shape_i32(shape, "x1", index)?,
                shape_i32(shape, "y1", index)?,
                shape_i32(shape, "x2", index)?,
                shape_i32(shape, "y2", index)?,
                shape_u32(shape, "thickness", index, 1, 128)? as i32,
                color,
            ),
            _ => return Err(format!("PNG shape {index} has unsupported kind {kind}")),
        }
    }
    let mut output = Cursor::new(Vec::new());
    DynamicImage::ImageRgba8(image)
        .write_to(&mut output, ImageFormat::Png)
        .map_err(|error| format!("failed encoding PNG asset: {error}"))?;
    Ok(output.into_inner())
}

fn draw_rect(image: &mut RgbaImage, x: i32, y: i32, width: u32, height: u32, color: Rgba<u8>) {
    for py in y..y.saturating_add(height as i32) {
        for px in x..x.saturating_add(width as i32) {
            put_pixel_clipped(image, px, py, color);
        }
    }
}

fn draw_circle(image: &mut RgbaImage, x: i32, y: i32, radius: i32, color: Rgba<u8>) {
    let radius_squared = i64::from(radius) * i64::from(radius);
    for py in y.saturating_sub(radius)..=y.saturating_add(radius) {
        for px in x.saturating_sub(radius)..=x.saturating_add(radius) {
            let dx = i64::from(px) - i64::from(x);
            let dy = i64::from(py) - i64::from(y);
            if dx * dx + dy * dy <= radius_squared {
                put_pixel_clipped(image, px, py, color);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_line(
    image: &mut RgbaImage,
    x1: i32,
    y1: i32,
    x2: i32,
    y2: i32,
    thickness: i32,
    color: Rgba<u8>,
) {
    let dx = x2.saturating_sub(x1);
    let dy = y2.saturating_sub(y1);
    let steps = dx.unsigned_abs().max(dy.unsigned_abs()).max(1);
    for step in 0..=steps {
        let t = f64::from(step) / f64::from(steps);
        let x = (f64::from(x1) + f64::from(dx) * t).round() as i32;
        let y = (f64::from(y1) + f64::from(dy) * t).round() as i32;
        draw_circle(image, x, y, (thickness / 2).max(1), color);
    }
}

fn put_pixel_clipped(image: &mut RgbaImage, x: i32, y: i32, color: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
        image.put_pixel(x as u32, y as u32, color);
    }
}

fn parse_color(value: &str) -> Result<Rgba<u8>, String> {
    let hex = value.strip_prefix('#').unwrap_or(value);
    if !matches!(hex.len(), 6 | 8) || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("PNG colors must be #RRGGBB or #RRGGBBAA".to_string());
    }
    let channel =
        |start| u8::from_str_radix(&hex[start..start + 2], 16).map_err(|error| error.to_string());
    Ok(Rgba([
        channel(0)?,
        channel(2)?,
        channel(4)?,
        if hex.len() == 8 { channel(6)? } else { 255 },
    ]))
}

fn shape_i32(
    shape: &serde_json::Map<String, Value>,
    name: &str,
    index: usize,
) -> Result<i32, String> {
    shape
        .get(name)
        .and_then(Value::as_i64)
        .and_then(|value| i32::try_from(value).ok())
        .filter(|value| (-4096..=4096).contains(value))
        .ok_or_else(|| format!("PNG shape {index} arg {name} must be between -4096 and 4096"))
}

fn shape_u32(
    shape: &serde_json::Map<String, Value>,
    name: &str,
    index: usize,
    min: u32,
    max: u32,
) -> Result<u32, String> {
    shape
        .get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (*value >= min) && (*value <= max))
        .ok_or_else(|| format!("PNG shape {index} arg {name} must be between {min} and {max}"))
}

fn controlled_id(value: &str) -> Result<String, String> {
    if value.is_empty()
        || value.len() > 80
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return Err("asset id must contain 1..=80 ASCII letters, digits, '_' or '-'".to_string());
    }
    Ok(value.to_string())
}

fn validate_svg(source: &str) -> Result<(), String> {
    if source.len() > MAX_TEXT_ASSET_BYTES || !source.trim_start().starts_with("<svg") {
        return Err("SVG must start with <svg and be no larger than 256 KiB".to_string());
    }
    let lower = source.to_ascii_lowercase();
    for forbidden in ["<script", "<!doctype", "href=\"http", "href='http"] {
        if lower.contains(forbidden) {
            return Err(format!("SVG contains forbidden content: {forbidden}"));
        }
    }
    Ok(())
}

fn upsert_entry(manifest: &mut AssetManifest, entry: AssetEntry) {
    if let Some(existing) = manifest
        .assets
        .iter_mut()
        .find(|existing| existing.id == entry.id)
    {
        *existing = entry;
    } else {
        manifest.assets.push(entry);
    }
    manifest.assets.sort_by(|a, b| a.id.cmp(&b.id));
}

fn required_string(args: &serde_json::Map<String, Value>, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .ok_or_else(|| format!("asset tool requires non-empty string arg: {name}"))
}

fn required_u32(
    args: &serde_json::Map<String, Value>,
    name: &str,
    min: u32,
    max: u32,
) -> Result<u32, String> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| (*value >= min) && (*value <= max))
        .ok_or_else(|| format!("asset arg {name} must be between {min} and {max}"))
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn procedural_wav(frequency: u32, duration_ms: u32) -> (Vec<u8>, u64) {
    const RATE: u32 = 44_100;
    let frames = u64::from(RATE) * u64::from(duration_ms) / 1_000;
    let data_bytes = u32::try_from(frames.saturating_mul(2)).unwrap_or(u32::MAX);
    let mut out = Vec::with_capacity(44 + data_bytes as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_bytes).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt \x10\0\0\0\x01\0\x01\0");
    out.extend_from_slice(&RATE.to_le_bytes());
    out.extend_from_slice(&(RATE * 2).to_le_bytes());
    out.extend_from_slice(&2_u16.to_le_bytes());
    out.extend_from_slice(&16_u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_bytes.to_le_bytes());
    for frame in 0..frames {
        let phase =
            2.0 * std::f64::consts::PI * f64::from(frequency) * frame as f64 / f64::from(RATE);
        let envelope = 1.0 - frame as f64 / frames.max(1) as f64;
        let sample = (phase.sin() * envelope * 10_000.0) as i16;
        out.extend_from_slice(&sample.to_le_bytes());
    }
    (out, frames)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn svg_rejects_active_and_remote_content() {
        assert!(validate_svg("<svg><path d='M0 0'/></svg>").is_ok());
        assert!(validate_svg("<svg><script>alert(1)</script></svg>").is_err());
        assert!(validate_svg("<svg><image href=\"https://example.com/a.png\"/></svg>").is_err());
    }

    #[test]
    fn wav_is_bounded_and_has_a_real_header() {
        let (wav, frames) = procedural_wav(440, 100);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(frames, 4_410);
        assert_eq!(wav.len(), 44 + frames as usize * 2);
    }

    #[test]
    fn png_renderer_produces_a_decodable_source_asset() {
        let shapes = vec![serde_json::json!({
            "kind": "circle",
            "x": 8,
            "y": 8,
            "radius": 4,
            "color": "#ff8844ff"
        })];
        let png = render_png(16, 16, parse_color("#102030").unwrap(), &shapes).unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (16, 16));
    }

    #[test]
    fn imagegen_png_import_is_bounded_and_decodable() {
        let root =
            std::env::temp_dir().join(format!("stasis_gauntlet_imagegen_{}", std::process::id()));
        let source = root.join("build/gauntlet/imagegen/ship.png");
        fs::create_dir_all(source.parent().expect("imagegen parent")).expect("imagegen dir");
        let png = render_png(32, 24, parse_color("#102030").unwrap(), &[]).unwrap();
        fs::write(&source, &png).expect("imagegen input");
        let (imported, width, height) =
            load_imagegen_png(&root, "build/gauntlet/imagegen/ship.png").expect("import");
        assert_eq!((width, height), (32, 24));
        assert_eq!(imported, png);
        assert!(load_imagegen_png(&root, "../ship.png").is_err());
        let assisted = root.join("build/ai-assets/imagegen/ship.png");
        fs::create_dir_all(assisted.parent().expect("assisted parent")).expect("assisted dir");
        fs::write(&assisted, &png).expect("assisted input");
        assert!(load_imagegen_png(&root, "build/ai-assets/imagegen/ship.png").is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn imagegen_png_import_can_crop_and_remove_a_flat_background() {
        let mut source = RgbaImage::from_pixel(8, 6, parse_color("#00ff00").unwrap());
        source.put_pixel(3, 2, parse_color("#d08040").unwrap());
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let args = serde_json::json!({
            "crop_x": 2,
            "crop_y": 1,
            "crop_width": 4,
            "crop_height": 3,
            "transparent_color": "#00ff00",
            "transparent_tolerance": 0
        });
        let (png, width, height, transformed) =
            transform_imported_png(args.as_object().unwrap(), encoded.into_inner(), 8, 6).unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgba8();

        assert!(transformed);
        assert_eq!((width, height), (4, 3));
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        assert_eq!(decoded.get_pixel(1, 1), &parse_color("#d08040").unwrap());
    }

    #[test]
    fn imagegen_png_import_adapts_to_a_near_flat_generated_background() {
        let mut source = RgbaImage::from_pixel(12, 10, parse_color("#ea0de6").unwrap());
        for x in 0..12 {
            source.put_pixel(x, 0, parse_color("#ef12e8").unwrap());
            source.put_pixel(x, 9, parse_color("#e723e1").unwrap());
        }
        source.put_pixel(5, 4, parse_color("#108878").unwrap());
        source.put_pixel(6, 4, parse_color("#d6a22f").unwrap());
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let args = serde_json::json!({
            "transparent_color": "#ff00ff",
            "transparent_tolerance": 18
        });
        let (png, _, _, transformed) =
            transform_imported_png(args.as_object().unwrap(), encoded.into_inner(), 12, 10)
                .unwrap();
        let decoded = image::load_from_memory_with_format(&png, ImageFormat::Png)
            .unwrap()
            .to_rgba8();

        assert!(transformed);
        assert_eq!(decoded.get_pixel(0, 0)[3], 0);
        assert_eq!(decoded.get_pixel(11, 9)[3], 0);
        assert_eq!(decoded.get_pixel(5, 4)[3], 255);
        assert_eq!(decoded.get_pixel(6, 4)[3], 255);
    }

    #[test]
    fn imagegen_png_import_rejects_an_opaque_unremoved_border() {
        let source = RgbaImage::from_pixel(10, 10, parse_color("#102030").unwrap());
        let mut encoded = Cursor::new(Vec::new());
        DynamicImage::ImageRgba8(source)
            .write_to(&mut encoded, ImageFormat::Png)
            .unwrap();
        let args = serde_json::json!({
            "transparent_color": "#ff00ff",
            "transparent_tolerance": 18
        });
        let error = transform_imported_png(args.as_object().unwrap(), encoded.into_inner(), 10, 10)
            .expect_err("unremoved background must fail atomically");

        assert!(error.contains("border opaque"));
    }

    #[test]
    fn obsolete_asset_deletion_updates_manifest_cache_and_rolls_back() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_delete_asset_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let source = root.join("assets/generated/old-unit.png");
        let prepared = root.join(".stasis_cache/play-assets/assets/generated/old-unit.png");
        fs::create_dir_all(source.parent().expect("source parent")).expect("source dir");
        fs::create_dir_all(prepared.parent().expect("prepared parent")).expect("prepared dir");
        let png = render_png(16, 16, parse_color("#102030").unwrap(), &[]).unwrap();
        fs::write(&source, &png).expect("source asset");
        fs::write(&prepared, &png).expect("prepared asset");
        let manifest = AssetManifest {
            schema: "stasis-assets".to_string(),
            version: 2,
            display: None,
            assets: vec![AssetEntry {
                id: "old-unit".to_string(),
                path: "assets/generated/old-unit.png".to_string(),
                content_sha256: sha256(&png),
                prepared_from_sha256: None,
                format: AssetFormat::Sprite {
                    encoding: SpriteEncoding::Png,
                    width: 16,
                    height: 16,
                },
                prepare: None,
                dependencies: Vec::new(),
            }],
        };
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("manifest file");
        let prepared_manifest = root
            .join(".stasis_cache/play-assets")
            .join(DEFAULT_ASSET_MANIFEST_PATH);
        fs::create_dir_all(
            prepared_manifest
                .parent()
                .expect("prepared manifest parent"),
        )
        .expect("prepared manifest dir");
        fs::write(
            &prepared_manifest,
            serde_json::to_vec_pretty(&manifest).expect("prepared manifest"),
        )
        .expect("prepared manifest file");
        let call = ToolCall {
            tool: "delete_asset".to_string(),
            args: serde_json::json!({
                "id": "old-unit",
                "path": "assets/generated/old-unit.png"
            }),
        };

        let transaction = apply_asset_calls(&root, &[&call]).expect("delete transaction");
        assert!(!source.exists());
        assert!(!prepared.exists());
        let updated: AssetManifest = serde_json::from_slice(
            &fs::read(root.join(DEFAULT_ASSET_MANIFEST_PATH)).expect("updated manifest"),
        )
        .expect("updated manifest JSON");
        assert!(updated.assets.is_empty());

        transaction.rollback().expect("rollback");
        assert_eq!(fs::read(&source).expect("restored source"), png);
        assert_eq!(fs::read(&prepared).expect("restored prepared"), png);
        let restored: AssetManifest = serde_json::from_slice(
            &fs::read(root.join(DEFAULT_ASSET_MANIFEST_PATH)).expect("restored manifest"),
        )
        .expect("restored manifest JSON");
        assert_eq!(restored.assets, manifest.assets);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn obsolete_asset_can_be_replaced_under_the_same_id_and_rolled_back() {
        let root = std::env::temp_dir().join(format!(
            "stasis_gauntlet_replace_asset_{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        let old_source = root.join("assets/generated/unit-v1.png");
        let new_source = root.join("assets/generated/unit-v2.png");
        let old_prepared = root.join(".stasis_cache/play-assets/assets/generated/unit-v1.png");
        let new_prepared = root.join(".stasis_cache/play-assets/assets/generated/unit-v2.png");
        fs::create_dir_all(old_source.parent().expect("source parent")).expect("source dir");
        fs::create_dir_all(old_prepared.parent().expect("prepared parent")).expect("prepared dir");
        let old_png = render_png(16, 16, parse_color("#102030").unwrap(), &[]).unwrap();
        fs::write(&old_source, &old_png).expect("old source asset");
        fs::write(&old_prepared, &old_png).expect("old prepared asset");
        let manifest = AssetManifest {
            schema: "stasis-assets".to_string(),
            version: 2,
            display: None,
            assets: vec![AssetEntry {
                id: "unit".to_string(),
                path: "assets/generated/unit-v1.png".to_string(),
                content_sha256: sha256(&old_png),
                prepared_from_sha256: None,
                format: AssetFormat::Sprite {
                    encoding: SpriteEncoding::Png,
                    width: 16,
                    height: 16,
                },
                prepare: None,
                dependencies: Vec::new(),
            }],
        };
        fs::write(
            root.join(DEFAULT_ASSET_MANIFEST_PATH),
            serde_json::to_vec_pretty(&manifest).expect("manifest"),
        )
        .expect("manifest file");
        let prepared_manifest = root
            .join(".stasis_cache/play-assets")
            .join(DEFAULT_ASSET_MANIFEST_PATH);
        fs::create_dir_all(
            prepared_manifest
                .parent()
                .expect("prepared manifest parent"),
        )
        .expect("prepared manifest dir");
        fs::write(
            &prepared_manifest,
            serde_json::to_vec_pretty(&manifest).expect("prepared manifest"),
        )
        .expect("prepared manifest file");
        let delete = ToolCall {
            tool: "delete_asset".to_string(),
            args: serde_json::json!({
                "id": "unit",
                "path": "assets/generated/unit-v1.png"
            }),
        };
        let replace = ToolCall {
            tool: "write_png_asset".to_string(),
            args: serde_json::json!({
                "id": "unit",
                "path": "assets/generated/unit-v2.png",
                "width": 24,
                "height": 24,
                "background": "#405060",
                "shapes": []
            }),
        };

        let transaction =
            apply_asset_calls(&root, &[&delete, &replace]).expect("replacement transaction");
        assert!(!old_source.exists());
        assert!(!old_prepared.exists());
        assert!(new_source.exists());
        assert!(new_prepared.exists());
        let updated: AssetManifest = serde_json::from_slice(
            &fs::read(root.join(DEFAULT_ASSET_MANIFEST_PATH)).expect("updated manifest"),
        )
        .expect("updated manifest JSON");
        assert_eq!(updated.assets.len(), 1);
        assert_eq!(updated.assets[0].id, "unit");
        assert_eq!(updated.assets[0].path, "assets/generated/unit-v2.png");

        transaction.rollback().expect("rollback");
        assert_eq!(fs::read(&old_source).expect("restored old source"), old_png);
        assert_eq!(
            fs::read(&old_prepared).expect("restored old prepared"),
            old_png
        );
        assert!(!new_source.exists());
        assert!(!new_prepared.exists());
        let restored: AssetManifest = serde_json::from_slice(
            &fs::read(root.join(DEFAULT_ASSET_MANIFEST_PATH)).expect("restored manifest"),
        )
        .expect("restored manifest JSON");
        assert_eq!(restored.assets, manifest.assets);
        let _ = fs::remove_dir_all(root);
    }
}
