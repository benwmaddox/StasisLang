use sha2::{Digest, Sha256};
use stasis_ai::image_generation::GeneratedImage;
use stasis_ai::task_session::TaskId;
use std::fs::{self, OpenOptions};
use std::io::Cursor;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const MAX_PNG_BYTES: usize = 8 * 1024 * 1024;
const MAX_EDGE: u32 = 4096;
const MAX_PIXELS: u64 = 16 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(super) struct ImageArtifact {
    pub task_id: TaskId,
    pub id: String,
    pub png: Vec<u8>,
    pub rgba: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub sha256: String,
    pub provider: String,
    pub model: String,
    pub route: String,
    pub fallback: String,
    pub cost_micros: Option<u64>,
}

pub(super) fn validate_generated(
    task_id: TaskId,
    id: String,
    generated: GeneratedImage,
) -> Result<ImageArtifact, String> {
    if generated.png.is_empty() || generated.png.len() > MAX_PNG_BYTES {
        return Err(format!("generated PNG must be 1 to {MAX_PNG_BYTES} bytes"));
    }
    let mut reader =
        image::ImageReader::with_format(Cursor::new(&generated.png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_PIXELS * 4);
    reader.limits(limits);
    let decoded = reader
        .decode()
        .map_err(|error| format!("image provider returned invalid PNG: {error}"))?
        .to_rgba8();
    let (width, height) = decoded.dimensions();
    if width == 0
        || height == 0
        || width > MAX_EDGE
        || height > MAX_EDGE
        || u64::from(width) * u64::from(height) > MAX_PIXELS
    {
        return Err("generated PNG exceeds the 4096 px / 16 megapixel preview limit".into());
    }
    let sha256 = format!("{:x}", Sha256::digest(&generated.png));
    Ok(ImageArtifact {
        task_id,
        id,
        png: generated.png,
        rgba: decoded.into_raw(),
        width: width as usize,
        height: height as usize,
        sha256,
        provider: generated.provider,
        model: generated.model,
        route: generated.route,
        fallback: generated.fallback,
        cost_micros: generated.cost_micros,
    })
}

pub(super) fn import_png(
    project_root: &Path,
    task_id: &TaskId,
    artifact: &ImageArtifact,
    relative: &str,
) -> Result<PathBuf, String> {
    if &artifact.task_id != task_id {
        return Err("generated image belongs to a different task".into());
    }
    if format!("{:x}", Sha256::digest(&artifact.png)) != artifact.sha256 {
        return Err("generated image changed after preview".into());
    }
    let relative = Path::new(relative.trim());
    let mut components = relative.components();
    if components.next() != Some(Component::Normal("assets".as_ref()))
        || !relative
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || relative.extension().and_then(|value| value.to_str()) != Some("png")
    {
        return Err("import path must be a normal project-relative assets/*.png path".into());
    }
    for part in relative.components() {
        let Component::Normal(part) = part else {
            unreachable!()
        };
        validate_portable_component(part)?;
    }
    let root = project_root
        .canonicalize()
        .map_err(|error| format!("project root could not be resolved: {error}"))?;
    let destination = root.join(relative);
    let parent = destination
        .parent()
        .ok_or_else(|| "import path has no parent".to_string())?;
    let mut cursor = root.clone();
    for part in relative
        .parent()
        .expect("relative asset parent")
        .components()
    {
        let Component::Normal(part) = part else {
            unreachable!()
        };
        cursor.push(part);
        match fs::symlink_metadata(&cursor) {
            Ok(metadata) if is_link_or_reparse_point(&metadata) => {
                return Err("import path contains a symlink".into())
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err("import path parent is not a directory".into())
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&cursor)
                    .map_err(|error| format!("failed creating import directory: {error}"))?
            }
            Err(error) => return Err(format!("failed inspecting import directory: {error}")),
        }
    }
    if fs::symlink_metadata(&destination).is_ok() {
        return Err("import destination already exists; choose a new path".into());
    }
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = parent.join(format!(".stasis-image-{}-{nonce}.tmp", std::process::id()));
    let write: Result<(), String> = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("failed creating temporary import: {error}"))?;
        file.write_all(&artifact.png)
            .map_err(|error| format!("failed writing import: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("failed syncing import: {error}"))?;
        fs::hard_link(&temporary, &destination)
            .map_err(|error| format!("failed creating collision-safe import: {error}"))?;
        Ok(())
    })();
    let _ = fs::remove_file(&temporary);
    write?;
    Ok(destination)
}

fn validate_portable_component(part: &std::ffi::OsStr) -> Result<(), String> {
    let value = part
        .to_str()
        .ok_or_else(|| "import path must be valid UTF-8".to_string())?;
    if value.is_empty()
        || value.ends_with(['.', ' '])
        || value.contains(':')
        || value.chars().any(|c| c < ' ')
    {
        return Err("import path contains a non-portable component".into());
    }
    let stem = value
        .split('.')
        .next()
        .unwrap_or(value)
        .to_ascii_uppercase();
    if matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit()
            && stem.as_bytes()[3] != b'0')
    {
        return Err("import path contains a reserved device name".into());
    }
    Ok(())
}

#[cfg(windows)]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_type().is_symlink() || metadata.file_attributes() & 0x400 != 0
}

#[cfg(not(windows))]
fn is_link_or_reparse_point(metadata: &fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(test)]
mod tests {
    use super::*;
    use stasis_ai::image_generation::GeneratedImage;

    fn png() -> Vec<u8> {
        use image::ImageEncoder;
        let mut bytes = Vec::new();
        image::codecs::png::PngEncoder::new(&mut bytes)
            .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        bytes
    }
    fn artifact(task: &str) -> ImageArtifact {
        validate_generated(
            TaskId::new(task),
            "image-1".into(),
            GeneratedImage {
                png: png(),
                provider: "test".into(),
                model: "fixture".into(),
                route: "direct".into(),
                fallback: "disabled".into(),
                cost_micros: None,
            },
        )
        .unwrap()
    }

    fn generated(png: Vec<u8>) -> GeneratedImage {
        GeneratedImage {
            png,
            provider: "test".into(),
            model: "fixture".into(),
            route: "direct".into(),
            fallback: "disabled".into(),
            cost_micros: None,
        }
    }
    fn root() -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "stasis-desktop-image-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        fs::create_dir(&path).unwrap();
        path
    }

    #[test]
    fn import_is_task_scoped_create_only_and_rejects_traversal() {
        let root = root();
        let image = artifact("task-1");
        assert!(import_png(&root, &TaskId::new("task-2"), &image, "assets/unit.png").is_err());
        assert!(import_png(&root, &TaskId::new("task-1"), &image, "../unit.png").is_err());
        let path = import_png(
            &root,
            &TaskId::new("task-1"),
            &image,
            "assets/generated/unit.png",
        )
        .unwrap();
        assert_eq!(fs::read(&path).unwrap(), image.png);
        assert!(import_png(
            &root,
            &TaskId::new("task-1"),
            &image,
            "assets/generated/unit.png"
        )
        .is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn preview_rejects_invalid_and_oversized_png_before_allocation() {
        assert!(validate_generated(
            TaskId::new("task-1"),
            "bad".into(),
            generated(b"not png".to_vec())
        )
        .is_err());
        use image::ImageEncoder;
        let pixels = vec![0_u8; (MAX_EDGE as usize + 1) * 4];
        let mut encoded = Vec::new();
        image::codecs::png::PngEncoder::new(&mut encoded)
            .write_image(&pixels, MAX_EDGE + 1, 1, image::ExtendedColorType::Rgba8)
            .unwrap();
        assert!(
            validate_generated(TaskId::new("task-1"), "wide".into(), generated(encoded)).is_err()
        );
    }

    #[test]
    fn import_rechecks_hash_and_portable_components_without_writing() {
        let root = root();
        let mut image = artifact("task-1");
        image.png.push(0);
        assert!(import_png(
            &root,
            &TaskId::new("task-1"),
            &image,
            "assets/generated/unit.png"
        )
        .is_err());
        assert!(!root.join("assets/generated/unit.png").exists());
        let image = artifact("task-1");
        for path in [
            "assets/CON.png",
            "assets/bad:name.png",
            "assets/bad./unit.png",
            "assets/trailing /unit.png",
            "assets/unit.PNG",
        ] {
            assert!(
                import_png(&root, &TaskId::new("task-1"), &image, path).is_err(),
                "accepted {path}"
            );
        }
        fs::remove_dir_all(root).unwrap();
    }
}
