use image::{DynamicImage, ImageFormat, ImageReader, Limits};
use sha2::{Digest, Sha256};
use stasis_ai::task_session::TaskId;
use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

pub(super) const MAX_IMAGE_BYTES: usize = 16 * 1024 * 1024;
pub(super) const MAX_IMAGE_DIMENSION: u32 = 4096;
const MAX_IMAGE_PIXELS: u64 = 16_777_216;
const MAX_THUMBNAIL_EDGE: u32 = 512;

static SESSION_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AttachmentOrigin {
    FilePicker,
    FileDrop,
    Clipboard,
    GameCapture,
}

impl AttachmentOrigin {
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::FilePicker => "file selection",
            Self::FileDrop => "drag and drop",
            Self::Clipboard => "clipboard paste",
            Self::GameCapture => "live game capture",
        }
    }
}

#[derive(Debug, Clone)]
pub(super) struct OwnedImageAttachment {
    pub(super) path: PathBuf,
    pub(super) name: String,
    pub(super) mime_type: &'static str,
    pub(super) byte_len: usize,
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) sha256: String,
    pub(super) origin: AttachmentOrigin,
    pub(super) thumbnail_rgba: Vec<u8>,
    pub(super) thumbnail_width: u32,
    pub(super) thumbnail_height: u32,
}

pub(super) struct SessionAttachmentStore {
    root: PathBuf,
    entries: BTreeMap<(TaskId, String), OwnedImageAttachment>,
}

impl SessionAttachmentStore {
    pub(super) fn new() -> Self {
        let process = std::process::id();
        let mut sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let root = loop {
            let candidate =
                std::env::temp_dir().join(format!("stasis-editor-{process}-{sequence}"));
            match std::fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    sequence = SESSION_SEQUENCE.fetch_add(1, Ordering::Relaxed);
                }
                Err(_) => break candidate,
            }
        };
        Self {
            root,
            entries: BTreeMap::new(),
        }
    }

    pub(super) fn get(&self, task_id: &TaskId, id: &str) -> Option<&OwnedImageAttachment> {
        self.entries.get(&(task_id.clone(), id.to_string()))
    }

    pub(super) fn remove(&mut self, task_id: &TaskId, id: &str) -> bool {
        let removed = self.entries.remove(&(task_id.clone(), id.to_string()));
        if let Some(attachment) = removed {
            let _ = std::fs::remove_file(attachment.path);
            true
        } else {
            false
        }
    }

    pub(super) fn insert_encoded(
        &mut self,
        task_id: &TaskId,
        id: String,
        name: String,
        origin: AttachmentOrigin,
        bytes: &[u8],
    ) -> Result<OwnedImageAttachment, String> {
        let (format, mime_type, extension) = encoded_format(bytes)?;
        let decoded = decode_limited(bytes, format)?;
        self.insert_decoded(
            task_id, id, name, origin, mime_type, extension, bytes, decoded,
        )
    }

    pub(super) fn insert_rgba(
        &mut self,
        task_id: &TaskId,
        id: String,
        name: String,
        origin: AttachmentOrigin,
        width: usize,
        height: usize,
        rgba: &[u8],
    ) -> Result<OwnedImageAttachment, String> {
        validate_dimensions(width as u64, height as u64)?;
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4))
            .ok_or_else(|| "clipboard image dimensions overflow".to_string())?;
        if rgba.len() != expected {
            return Err("clipboard image pixel length does not match its dimensions".into());
        }
        let image = image::RgbaImage::from_raw(width as u32, height as u32, rgba.to_vec())
            .ok_or_else(|| "clipboard image pixels are invalid".to_string())?;
        let decoded = DynamicImage::ImageRgba8(image);
        let mut bytes = Vec::new();
        decoded
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .map_err(|error| format!("could not encode clipboard image: {error}"))?;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(format!(
                "clipboard image exceeds the {} MiB limit",
                MAX_IMAGE_BYTES / 1024 / 1024
            ));
        }
        self.insert_decoded(
            task_id,
            id,
            name,
            origin,
            "image/png",
            "png",
            &bytes,
            decoded,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_decoded(
        &mut self,
        task_id: &TaskId,
        id: String,
        name: String,
        origin: AttachmentOrigin,
        mime_type: &'static str,
        extension: &'static str,
        bytes: &[u8],
        decoded: DynamicImage,
    ) -> Result<OwnedImageAttachment, String> {
        if self.entries.contains_key(&(task_id.clone(), id.clone())) {
            return Err("attachment ID is already in use for this task".into());
        }
        std::fs::create_dir_all(&self.root)
            .map_err(|error| format!("could not create attachment storage: {error}"))?;
        let sha256 = format!("{:x}", Sha256::digest(bytes));
        let task_hash = format!("{:x}", Sha256::digest(task_id.as_str().as_bytes()));
        let path = self
            .root
            .join(format!("{}-{id}-{sha256}.{extension}", &task_hash[..12]));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("could not create owned attachment: {error}"))?;
        if let Err(error) = file.write_all(bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = std::fs::remove_file(&path);
            return Err(format!("could not save owned attachment: {error}"));
        }

        let width = decoded.width();
        let height = decoded.height();
        let thumbnail = decoded
            .thumbnail(MAX_THUMBNAIL_EDGE, MAX_THUMBNAIL_EDGE)
            .to_rgba8();
        let attachment = OwnedImageAttachment {
            path,
            name: bounded_name(&name),
            mime_type,
            byte_len: bytes.len(),
            width,
            height,
            sha256,
            origin,
            thumbnail_width: thumbnail.width(),
            thumbnail_height: thumbnail.height(),
            thumbnail_rgba: thumbnail.into_raw(),
        };
        self.entries
            .insert((task_id.clone(), id), attachment.clone());
        Ok(attachment)
    }
}

impl Drop for SessionAttachmentStore {
    fn drop(&mut self) {
        if self.root.parent() == Some(std::env::temp_dir().as_path())
            && self
                .root
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("stasis-editor-"))
        {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
}

pub(super) fn read_bounded(path: &Path) -> Result<Vec<u8>, String> {
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|error| format!("could not inspect {}: {error}", path.display()))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err("attachment must be a regular file".into());
    }
    if metadata.len() > MAX_IMAGE_BYTES as u64 {
        return Err(format!(
            "image exceeds the {} MiB limit",
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let file = std::fs::File::open(path)
        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_IMAGE_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("could not read {}: {error}", path.display()))?;
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "image exceeds the {} MiB limit",
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    if bytes.is_empty() {
        return Err("image is empty".into());
    }
    Ok(bytes)
}

fn encoded_format(bytes: &[u8]) -> Result<(ImageFormat, &'static str, &'static str), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]) {
        Ok((ImageFormat::Png, "image/png", "png"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Ok((ImageFormat::Jpeg, "image/jpeg", "jpg"))
    } else {
        Err("only PNG and JPEG images can be attached".into())
    }
}

fn decode_limited(bytes: &[u8], format: ImageFormat) -> Result<DynamicImage, String> {
    if bytes.len() > MAX_IMAGE_BYTES {
        return Err(format!(
            "image exceeds the {} MiB limit",
            MAX_IMAGE_BYTES / 1024 / 1024
        ));
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_IMAGE_DIMENSION);
    limits.max_image_height = Some(MAX_IMAGE_DIMENSION);
    limits.max_alloc = Some(MAX_IMAGE_PIXELS.saturating_mul(4));
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(limits);
    let image = reader
        .decode()
        .map_err(|error| format!("image could not be decoded safely: {error}"))?;
    validate_dimensions(image.width() as u64, image.height() as u64)?;
    Ok(image)
}

pub(super) fn decode_png_rgba_limited(bytes: &[u8]) -> Result<image::RgbaImage, String> {
    decode_limited(bytes, ImageFormat::Png).map(DynamicImage::into_rgba8)
}

fn validate_dimensions(width: u64, height: u64) -> Result<(), String> {
    if width == 0 || height == 0 {
        return Err("image dimensions must be non-zero".into());
    }
    if width > MAX_IMAGE_DIMENSION as u64 || height > MAX_IMAGE_DIMENSION as u64 {
        return Err(format!(
            "image dimensions exceed {MAX_IMAGE_DIMENSION}x{MAX_IMAGE_DIMENSION}"
        ));
    }
    if width.saturating_mul(height) > MAX_IMAGE_PIXELS {
        return Err(format!("image exceeds the {MAX_IMAGE_PIXELS} pixel limit"));
    }
    Ok(())
}

fn bounded_name(name: &str) -> String {
    let name = Path::new(name)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("image");
    let bounded = name.chars().take(128).collect::<String>();
    if bounded.is_empty() {
        "image".into()
    } else {
        bounded
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
        let image = image::RgbaImage::from_pixel(width, height, image::Rgba(color));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgba8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Png)
            .unwrap();
        bytes
    }

    fn jpeg(width: u32, height: u32) -> Vec<u8> {
        let image = image::RgbImage::from_pixel(width, height, image::Rgb([12, 34, 56]));
        let mut bytes = Vec::new();
        DynamicImage::ImageRgb8(image)
            .write_to(&mut Cursor::new(&mut bytes), ImageFormat::Jpeg)
            .unwrap();
        bytes
    }

    #[test]
    fn intake_owns_hashes_and_bounds_a_png_thumbnail() {
        let mut store = SessionAttachmentStore::new();
        let task = TaskId::new("one");
        let bytes = png(900, 450, [1, 2, 3, 255]);
        let attachment = store
            .insert_encoded(
                &task,
                "file-1".into(),
                "sample.png".into(),
                AttachmentOrigin::FileDrop,
                &bytes,
            )
            .unwrap();
        assert_eq!(attachment.width, 900);
        assert_eq!(attachment.height, 450);
        assert_eq!(attachment.thumbnail_width, 512);
        assert_eq!(attachment.thumbnail_height, 256);
        assert_eq!(attachment.sha256, format!("{:x}", Sha256::digest(&bytes)));
        assert_eq!(std::fs::read(&attachment.path).unwrap(), bytes);
    }

    #[test]
    fn intake_rejects_unapproved_mime_and_oversized_dimensions() {
        let mut store = SessionAttachmentStore::new();
        let task = TaskId::new("one");
        assert!(store
            .insert_encoded(
                &task,
                "bad".into(),
                "bad.gif".into(),
                AttachmentOrigin::FilePicker,
                b"GIF89a"
            )
            .unwrap_err()
            .contains("PNG and JPEG"));
        let oversized = png(MAX_IMAGE_DIMENSION + 1, 1, [0, 0, 0, 255]);
        assert!(store
            .insert_encoded(
                &task,
                "huge".into(),
                "huge.png".into(),
                AttachmentOrigin::FilePicker,
                &oversized
            )
            .unwrap_err()
            .contains("decoded safely"));
    }

    #[test]
    fn removal_deletes_only_the_owned_copy() {
        let mut store = SessionAttachmentStore::new();
        let task = TaskId::new("one");
        let attachment = store
            .insert_encoded(
                &task,
                "file-1".into(),
                "sample.png".into(),
                AttachmentOrigin::FilePicker,
                &png(2, 2, [0, 0, 0, 255]),
            )
            .unwrap();
        assert!(attachment.path.exists());
        assert!(store.remove(&task, "file-1"));
        assert!(!attachment.path.exists());
    }

    #[test]
    fn jpeg_and_clipboard_pixels_use_the_same_bounded_owned_contract() {
        let mut store = SessionAttachmentStore::new();
        let task = TaskId::new("one");
        let jpeg = store
            .insert_encoded(
                &task,
                "jpeg".into(),
                "photo.jpg".into(),
                AttachmentOrigin::FilePicker,
                &jpeg(3, 2),
            )
            .unwrap();
        assert_eq!(
            (jpeg.mime_type, jpeg.width, jpeg.height),
            ("image/jpeg", 3, 2)
        );
        let rgba = [255_u8, 0, 0, 255].repeat(4);
        let pasted = store
            .insert_rgba(
                &task,
                "paste".into(),
                "clipboard.png".into(),
                AttachmentOrigin::Clipboard,
                2,
                2,
                &rgba,
            )
            .unwrap();
        assert_eq!(
            (pasted.mime_type, pasted.width, pasted.height),
            ("image/png", 2, 2)
        );
        assert!(store
            .insert_rgba(
                &task,
                "short".into(),
                "bad.png".into(),
                AttachmentOrigin::Clipboard,
                2,
                2,
                &rgba[..8]
            )
            .unwrap_err()
            .contains("pixel length"));
    }

    #[test]
    fn malformed_data_and_duplicate_ids_fail_without_replacing_owned_bytes() {
        let mut store = SessionAttachmentStore::new();
        let task = TaskId::new("one");
        let bytes = png(2, 2, [9, 8, 7, 255]);
        let first = store
            .insert_encoded(
                &task,
                "same".into(),
                "first.png".into(),
                AttachmentOrigin::FilePicker,
                &bytes,
            )
            .unwrap();
        assert!(store
            .insert_encoded(
                &task,
                "same".into(),
                "second.png".into(),
                AttachmentOrigin::FileDrop,
                &png(2, 2, [1, 1, 1, 255])
            )
            .unwrap_err()
            .contains("already in use"));
        assert_eq!(std::fs::read(first.path).unwrap(), bytes);
        let mut truncated = bytes[..12].to_vec();
        truncated.extend_from_slice(b"broken");
        assert!(store
            .insert_encoded(
                &task,
                "bad".into(),
                "bad.png".into(),
                AttachmentOrigin::FileDrop,
                &truncated
            )
            .is_err());
    }

    #[test]
    fn identical_ids_are_isolated_by_task_and_store_drop_cleans_session_files() {
        let (root, first, second) = {
            let mut store = SessionAttachmentStore::new();
            let one = TaskId::new("one");
            let two = TaskId::new("two");
            let first = store
                .insert_encoded(
                    &one,
                    "same".into(),
                    "one.png".into(),
                    AttachmentOrigin::FilePicker,
                    &png(1, 1, [1, 2, 3, 255]),
                )
                .unwrap()
                .path;
            let second = store
                .insert_encoded(
                    &two,
                    "same".into(),
                    "two.png".into(),
                    AttachmentOrigin::FilePicker,
                    &png(1, 1, [4, 5, 6, 255]),
                )
                .unwrap()
                .path;
            assert_ne!(first, second);
            (store.root.clone(), first, second)
        };
        assert!(!first.exists());
        assert!(!second.exists());
        assert!(!root.exists());
    }
}
