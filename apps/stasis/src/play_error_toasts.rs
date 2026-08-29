//! Development-only error notifications for the in-process `stasis play` loop.
//!
//! This module intentionally owns no runtime or ABI state. It appends ordinary gfx_cmd v5/v6
//! rectangle and text commands to the buffers that the guest already submitted.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

pub const TOAST_LIFETIME: Duration = Duration::from_secs(10);
pub const MAX_VISIBLE_TOASTS: usize = 5;
pub const MAX_MESSAGE_BYTES: usize = 160;

const GFX_CMD_MAGIC: i32 = 0x4758_4631;
const GFX_CMD_V5_VERSION: i32 = 5;
const GFX_CMD_VERSION: i32 = 6;
const GFX_I_VERSION: usize = 1;
const GFX_I_LINE_COUNT: usize = 3;
const GFX_I_SPRITE_COUNT: usize = 4;
const GFX_I_TEXT_COUNT: usize = 7;
const GFX_I_TEXT_BYTES_USED: usize = 9;
const GFX_I_LOGICAL_W: usize = 10;
const GFX_I_ORDER_COUNT: usize = 22;
const GFX_I_RECT_COUNT: usize = 24;
const GFX_I_ORDER_BASE: usize = 18_464;
const GFX_I_TEXT_BASE: usize = 12_320;
const GFX_F_RECT_REVERSE_BASE: usize = 79_996;
const GFX_F_TEXT_BASE: usize = 112_772;
const GFX_MAX_GEOMETRY: usize = 10_000;
const GFX_MAX_ORDER_V5: usize = 16_144;
const GFX_MAX_ORDER: usize = 16_656;
const GFX_MAX_TEXT: usize = 2_048;
const GFX_TEXT_MAX_BYTES: usize = 65_536;
const GFX_GEOMETRY_STRIDE_F32: usize = 8;
const GFX_TEXT_STRIDE_I32: usize = 3;
const GFX_TEXT_STRIDE_F32: usize = 6;
const GFX_ORDER_KIND_SCALE: i32 = 16_384;
const GFX_ORDER_LINE: i32 = 1;
const GFX_ORDER_SPRITE: i32 = 2;
const GFX_ORDER_RECT: i32 = 4;
const GFX_ORDER_TEXT: i32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
struct Toast {
    message: String,
    shown_at: Instant,
}

#[derive(Debug, Default)]
pub struct PlayErrorToasts {
    items: VecDeque<Toast>,
    font_handle: Option<i32>,
}

impl PlayErrorToasts {
    pub fn new(font_handle: Option<i32>) -> Self {
        Self {
            items: VecDeque::new(),
            font_handle: font_handle.filter(|handle| *handle > 0),
        }
    }

    pub fn enqueue(&mut self, message: impl AsRef<str>, now: Instant) {
        if self.items.len() >= MAX_VISIBLE_TOASTS {
            self.items.pop_front();
        }
        self.items.push_back(Toast {
            message: bound_message(message.as_ref()),
            shown_at: now,
        });
    }

    pub fn enqueue_error(&mut self, category: &str, error: &str, now: Instant) {
        self.enqueue(format!("{category}: {error}"), now);
    }

    #[cfg(test)]
    pub fn len(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.items.len()
    }

    #[cfg(test)]
    pub fn messages(&mut self, now: Instant) -> Vec<String> {
        self.prune(now);
        self.items
            .iter()
            .map(|toast| toast.message.clone())
            .collect()
    }

    /// Append active toast commands after all guest commands. Returns the number of toasts whose
    /// background rectangle was appended. Invalid or exhausted guest buffers are left untouched.
    pub fn append_to_buffers(
        &mut self,
        cmd_i32: &mut [i32],
        cmd_f32: &mut [f32],
        cmd_u8: &mut [u8],
        now: Instant,
    ) -> usize {
        self.prune(now);
        if self.items.is_empty() || self.font_handle.is_none() || !valid_header(cmd_i32) {
            return 0;
        }

        let max_order = if cmd_i32[GFX_I_VERSION] >= GFX_CMD_VERSION {
            GFX_MAX_ORDER
        } else {
            GFX_MAX_ORDER_V5
        };

        let line_count = nonnegative_count(cmd_i32, GFX_I_LINE_COUNT, GFX_MAX_GEOMETRY);
        let mut rect_count = nonnegative_count(cmd_i32, GFX_I_RECT_COUNT, GFX_MAX_GEOMETRY);
        let mut order_count = nonnegative_count(cmd_i32, GFX_I_ORDER_COUNT, max_order);
        let sprite_count = nonnegative_count(cmd_i32, GFX_I_SPRITE_COUNT, 4_096);
        let mut text_count = nonnegative_count(cmd_i32, GFX_I_TEXT_COUNT, GFX_MAX_TEXT);
        let mut text_bytes = nonnegative_count(cmd_i32, GFX_I_TEXT_BYTES_USED, GFX_TEXT_MAX_BYTES);
        let width = cmd_i32
            .get(GFX_I_LOGICAL_W)
            .copied()
            .filter(|width| *width > 0)
            .unwrap_or(800) as f32;
        let toast_width = (width - 32.0).max(200.0);
        let font_handle = self.font_handle;
        let mut appended = 0;

        // An empty v5 order stream means the runtime uses the compatibility category order.
        // Materialize that order before appending so adding a toast never hides guest draws.
        if order_count == 0 {
            let guest_order_count = line_count
                .saturating_add(rect_count)
                .saturating_add(sprite_count)
                .saturating_add(text_count);
            if guest_order_count > max_order
                || GFX_I_ORDER_BASE
                    .checked_add(guest_order_count)
                    .is_none_or(|end| end > cmd_i32.len())
            {
                return 0;
            }
            for index in 0..line_count {
                if !append_order(cmd_i32, &mut order_count, max_order, GFX_ORDER_LINE, index) {
                    return 0;
                }
            }
            for index in 0..rect_count {
                if !append_order(cmd_i32, &mut order_count, max_order, GFX_ORDER_RECT, index) {
                    return 0;
                }
            }
            for index in 0..sprite_count {
                if !append_order(
                    cmd_i32,
                    &mut order_count,
                    max_order,
                    GFX_ORDER_SPRITE,
                    index,
                ) {
                    return 0;
                }
            }
            for index in 0..text_count {
                if !append_order(cmd_i32, &mut order_count, max_order, GFX_ORDER_TEXT, index) {
                    return 0;
                }
            }
        }

        for (index, toast) in self.items.iter().enumerate() {
            if order_count >= max_order || line_count + rect_count >= GFX_MAX_GEOMETRY {
                break;
            }
            let y = 16.0 + index as f32 * 48.0;
            let rect_index = rect_count;
            let rect_base = GFX_F_RECT_REVERSE_BASE - rect_index * GFX_GEOMETRY_STRIDE_F32;
            if rect_base + GFX_GEOMETRY_STRIDE_F32 > cmd_f32.len() {
                break;
            }
            if !append_order(
                cmd_i32,
                &mut order_count,
                max_order,
                GFX_ORDER_RECT,
                rect_index,
            ) {
                break;
            }
            write_rect(cmd_f32, rect_base, 16.0, y, toast_width, 36.0);
            rect_count += 1;
            appended += 1;

            let Some(font_handle) = font_handle else {
                continue;
            };
            if text_count >= GFX_MAX_TEXT {
                continue;
            }
            let bytes = toast.message.as_bytes();
            if text_bytes
                .checked_add(bytes.len() + 1)
                .is_none_or(|end| end > GFX_TEXT_MAX_BYTES)
                || text_bytes + bytes.len() + 1 > cmd_u8.len()
                || order_count >= max_order
            {
                continue;
            }
            let text_index = text_count;
            if !append_order(
                cmd_i32,
                &mut order_count,
                max_order,
                GFX_ORDER_TEXT,
                text_index,
            ) {
                continue;
            }
            let byte_offset = text_bytes;
            cmd_u8[byte_offset..byte_offset + bytes.len()].copy_from_slice(bytes);
            cmd_u8[byte_offset + bytes.len()] = 0;
            let i32_base = GFX_I_TEXT_BASE + text_index * GFX_TEXT_STRIDE_I32;
            let f32_base = GFX_F_TEXT_BASE + text_index * GFX_TEXT_STRIDE_F32;
            if i32_base + GFX_TEXT_STRIDE_I32 > cmd_i32.len()
                || f32_base + GFX_TEXT_STRIDE_F32 > cmd_f32.len()
            {
                // The order was not safely reversible, but these are fixed production buffers;
                // avoid ever writing outside them and report no additional text command.
                order_count -= 1;
                continue;
            }
            cmd_i32[i32_base..i32_base + GFX_TEXT_STRIDE_I32].copy_from_slice(&[
                font_handle,
                byte_offset as i32,
                bytes.len() as i32,
            ]);
            cmd_f32[f32_base..f32_base + GFX_TEXT_STRIDE_F32].copy_from_slice(&[
                26.0,
                y + 10.0,
                1.0,
                1.0,
                1.0,
                1.0,
            ]);
            text_count += 1;
            text_bytes += bytes.len() + 1;
        }

        cmd_i32[GFX_I_ORDER_COUNT] = order_count as i32;
        cmd_i32[GFX_I_RECT_COUNT] = rect_count as i32;
        cmd_i32[GFX_I_TEXT_COUNT] = text_count as i32;
        cmd_i32[GFX_I_TEXT_BYTES_USED] = text_bytes as i32;
        appended
    }

    fn prune(&mut self, now: Instant) {
        self.items.retain(|toast| {
            now.checked_duration_since(toast.shown_at)
                .is_some_and(|age| age < TOAST_LIFETIME)
        });
    }
}

fn valid_header(cmd_i32: &[i32]) -> bool {
    cmd_i32.first().copied() == Some(GFX_CMD_MAGIC)
        && matches!(
            cmd_i32.get(GFX_I_VERSION).copied(),
            Some(GFX_CMD_V5_VERSION | GFX_CMD_VERSION)
        )
}

fn nonnegative_count(buffer: &[i32], index: usize, maximum: usize) -> usize {
    buffer
        .get(index)
        .copied()
        .unwrap_or(0)
        .clamp(0, maximum as i32) as usize
}

fn append_order(
    cmd_i32: &mut [i32],
    count: &mut usize,
    max_order: usize,
    kind: i32,
    index: usize,
) -> bool {
    let Some(slot) = GFX_I_ORDER_BASE.checked_add(*count) else {
        return false;
    };
    if slot >= cmd_i32.len() || *count >= max_order || index > i32::MAX as usize {
        return false;
    }
    cmd_i32[slot] = kind * GFX_ORDER_KIND_SCALE + index as i32;
    *count += 1;
    true
}

fn write_rect(cmd_f32: &mut [f32], base: usize, x: f32, y: f32, w: f32, h: f32) {
    cmd_f32[base..base + GFX_GEOMETRY_STRIDE_F32]
        .copy_from_slice(&[x, y, w, h, 0.08, 0.04, 0.04, 0.94]);
}

fn bound_message(message: &str) -> String {
    let normalized = message.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.len() <= MAX_MESSAGE_BYTES {
        return normalized;
    }
    let mut end = MAX_MESSAGE_BYTES.saturating_sub(3);
    while end > 0 && !normalized.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}...", &normalized[..end])
}

/// Materialize the existing tool font for the native runtime's path-based font API.
///
/// The file is only needed while `stasis play` is running and is removed when this guard drops.
pub struct EmbeddedToastFont {
    path: PathBuf,
    runtime_path: PathBuf,
}

impl EmbeddedToastFont {
    pub fn stage(asset_root: &Path) -> Result<Self, String> {
        let file_name = format!(".stasis-play-toast-font-{}.ttf", std::process::id());
        let path = asset_root.join(&file_name);
        fs::write(
            &path,
            include_bytes!("../assets/gauntlet-font/Basic-Regular.ttf"),
        )
        .map_err(|error| format!("failed to stage embedded play toast font: {error}"))?;
        Ok(Self {
            path,
            runtime_path: PathBuf::from(file_name),
        })
    }

    pub fn runtime_path(&self) -> &Path {
        &self.runtime_path
    }
}

impl Drop for EmbeddedToastFont {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn buffers() -> (Vec<i32>, Vec<f32>, Vec<u8>) {
        let mut i32s = vec![0; 34_608];
        i32s[0] = GFX_CMD_MAGIC;
        i32s[1] = GFX_CMD_VERSION;
        i32s[2] = 2;
        i32s[GFX_I_LINE_COUNT] = 1;
        i32s[GFX_I_ORDER_COUNT] = 1;
        i32s[GFX_I_ORDER_BASE] = 16_384 + 0;
        i32s[GFX_I_LOGICAL_W] = 640;
        (i32s, vec![0.0; 125_060], vec![0; 65_536])
    }

    #[test]
    fn exact_ten_second_boundary_expires() {
        let start = Instant::now();
        let mut queue = PlayErrorToasts::default();
        queue.enqueue("compile failed", start);
        assert_eq!(queue.len(start + Duration::from_millis(9_999)), 1);
        assert_eq!(queue.len(start + TOAST_LIFETIME), 0);
    }

    #[test]
    fn fifo_cap_evicts_oldest() {
        let start = Instant::now();
        let mut queue = PlayErrorToasts::default();
        for index in 0..=MAX_VISIBLE_TOASTS {
            queue.enqueue(
                format!("error {index}"),
                start + Duration::from_millis(index as u64),
            );
        }
        assert_eq!(
            queue.messages(start + Duration::from_secs(1)),
            vec!["error 1", "error 2", "error 3", "error 4", "error 5"]
        );
    }

    #[test]
    fn appends_after_guest_order_and_preserves_present() {
        let start = Instant::now();
        let (mut i32s, mut f32s, mut u8s) = buffers();
        let mut queue = PlayErrorToasts::new(Some(7));
        queue.enqueue("asset failed", start);
        assert_eq!(
            queue.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
            1
        );
        assert_eq!(i32s[2], 2);
        assert_eq!(i32s[GFX_I_ORDER_COUNT], 3);
        assert_eq!(i32s[GFX_I_ORDER_BASE], 16_384);
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 1],
            GFX_ORDER_RECT * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 2],
            GFX_ORDER_TEXT * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(i32s[GFX_I_RECT_COUNT], 1);
        assert_eq!(i32s[GFX_I_TEXT_COUNT], 1);
        assert_eq!(&u8s[..13], b"asset failed\0");
    }

    #[test]
    fn empty_guest_order_is_materialized_before_toast_commands() {
        let start = Instant::now();
        let (mut i32s, mut f32s, mut u8s) = buffers();
        i32s[GFX_I_ORDER_COUNT] = 0;
        i32s[GFX_I_LINE_COUNT] = 1;
        i32s[GFX_I_RECT_COUNT] = 1;
        i32s[GFX_I_SPRITE_COUNT] = 1;
        i32s[GFX_I_TEXT_COUNT] = 1;
        i32s[GFX_I_TEXT_BYTES_USED] = 1;
        let mut queue = PlayErrorToasts::new(Some(7));
        queue.enqueue("error", start);
        assert_eq!(
            queue.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
            1
        );
        assert_eq!(i32s[GFX_I_ORDER_COUNT], 6);
        assert_eq!(
            i32s[GFX_I_ORDER_BASE],
            GFX_ORDER_LINE * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 1],
            GFX_ORDER_RECT * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 2],
            GFX_ORDER_SPRITE * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 3],
            GFX_ORDER_TEXT * GFX_ORDER_KIND_SCALE
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 4],
            GFX_ORDER_RECT * GFX_ORDER_KIND_SCALE + 1
        );
        assert_eq!(
            i32s[GFX_I_ORDER_BASE + 5],
            GFX_ORDER_TEXT * GFX_ORDER_KIND_SCALE + 1
        );
    }

    #[test]
    fn bounded_message_is_single_line_and_utf8_safe() {
        let mut queue = PlayErrorToasts::default();
        let start = Instant::now();
        queue.enqueue("a\n\t".to_string() + &"x".repeat(300), start);
        let message = queue.messages(start).pop().expect("message");
        assert!(message.len() <= MAX_MESSAGE_BYTES);
        assert!(!message.contains('\n'));
        assert!(message.ends_with("..."));
    }

    #[test]
    fn missing_font_is_a_noop_instead_of_unlabeled_background() {
        let start = Instant::now();
        let (mut i32s, mut f32s, mut u8s) = buffers();
        let before_i32 = i32s.clone();
        let before_f32 = f32s.clone();
        let before_u8 = u8s.clone();
        let mut queue = PlayErrorToasts::default();
        queue.enqueue("asset failed", start);
        assert_eq!(
            queue.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
            0
        );
        assert_eq!(i32s, before_i32);
        assert_eq!(f32s, before_f32);
        assert_eq!(u8s, before_u8);
    }

    #[test]
    fn recoverable_error_categories_are_concise_and_distinct() {
        let start = Instant::now();
        let mut queue = PlayErrorToasts::default();
        queue.enqueue_error("Asset refresh failed", "missing assets/unit.svg", start);
        queue.enqueue_error("Live compile failed", "parser error at line 4", start);
        assert_eq!(
            queue.messages(start),
            vec![
                "Asset refresh failed: missing assets/unit.svg",
                "Live compile failed: parser error at line 4"
            ]
        );
    }

    #[test]
    fn invalid_or_exhausted_buffers_degrade_without_writes() {
        let start = Instant::now();
        let mut queue = PlayErrorToasts::new(Some(7));
        queue.enqueue("error", start);
        let mut i32s = vec![0; 4];
        let mut f32s = vec![0.0; 4];
        let mut u8s = vec![0; 4];
        assert_eq!(
            queue.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
            0
        );
        assert_eq!(i32s, vec![0; 4]);
    }

    #[test]
    fn full_order_stream_drops_toasts_without_invalid_data() {
        let start = Instant::now();
        let (mut i32s, mut f32s, mut u8s) = buffers();
        i32s[GFX_I_ORDER_COUNT] = GFX_MAX_ORDER as i32;
        let before = i32s.clone();
        let mut queue = PlayErrorToasts::new(Some(7));
        queue.enqueue("error", start);
        assert_eq!(
            queue.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
            0
        );
        assert_eq!(i32s, before);
    }

    #[test]
    fn staged_font_runtime_path_is_relative_to_prepared_asset_root() {
        let root = std::env::temp_dir().join(format!(
            "stasis-play-toast-font-test-{}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("prepared asset root");
        let font = EmbeddedToastFont::stage(&root).expect("stage embedded font");
        assert!(root.join(font.runtime_path()).is_file());
        drop(font);
        assert!(root
            .read_dir()
            .expect("read prepared root")
            .next()
            .is_none());
        fs::remove_dir(&root).expect("remove prepared asset root");
    }
}
