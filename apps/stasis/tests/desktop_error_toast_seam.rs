use std::time::Instant;

#[path = "../src/play_error_toasts.rs"]
mod play_error_toasts;

use play_error_toasts::{PlayErrorToasts, MAX_VISIBLE_TOASTS, TOAST_LIFETIME};

const MAGIC: i32 = 0x4758_4631;
const VERSION: i32 = 5;
const I_LINE_COUNT: usize = 3;
const I_SPRITE_COUNT: usize = 4;
const I_TEXT_COUNT: usize = 7;
const I_TEXT_BYTES_USED: usize = 9;
const I_LOGICAL_W: usize = 10;
const I_ORDER_COUNT: usize = 22;
const I_ORDER_BASE: usize = 18_464;

fn valid_buffers() -> (Vec<i32>, Vec<f32>, Vec<u8>) {
    let mut i32s = vec![0; 34_608];
    i32s[0] = MAGIC;
    i32s[1] = VERSION;
    i32s[I_LOGICAL_W] = 800;
    (i32s, vec![0.0; 125_060], vec![0; 65_536])
}

#[test]
fn representative_recoverable_errors_have_lifetime_cap_and_guest_order() {
    let start = Instant::now();
    let mut queue = PlayErrorToasts::new(Some(7));
    queue.enqueue_error("Asset refresh failed", "assets/unit.svg", start);
    queue.enqueue_error("Live compile failed", "line 4", start);
    for index in 0..MAX_VISIBLE_TOASTS {
        queue.enqueue(format!("error {index}"), start);
    }
    assert_eq!(queue.len(start), MAX_VISIBLE_TOASTS);
    assert_eq!(queue.messages(start)[0], "error 0");
    assert_eq!(queue.len(start + TOAST_LIFETIME), 0);

    let (mut i32s, mut f32s, mut u8s) = valid_buffers();
    i32s[2] = 2; // Preserve the guest's present flag.
    i32s[I_LINE_COUNT] = 1;
    i32s[I_SPRITE_COUNT] = 1;
    i32s[I_TEXT_COUNT] = 1;
    i32s[I_TEXT_BYTES_USED] = 1;
    i32s[I_ORDER_COUNT] = 1;
    i32s[I_ORDER_BASE] = 16_384;
    let mut one = PlayErrorToasts::new(Some(7));
    one.enqueue_error("Asset refresh failed", "assets/unit.svg", start);
    assert_eq!(
        one.append_to_buffers(&mut i32s, &mut f32s, &mut u8s, start),
        1
    );
    assert_eq!(i32s[2], 2);
    assert_eq!(i32s[I_ORDER_BASE], 16_384);
    assert_eq!(i32s[I_ORDER_COUNT], 3);
    assert!(String::from_utf8_lossy(&u8s).contains("Asset refresh failed"));
}

#[test]
fn development_toast_component_is_not_in_packaged_runtime_sources() {
    let desktop_aot = include_str!("../src/compiler_backend.rs");
    let mobile_runtime = include_str!("../../../runtime/stasis_mobile_aot_runtime.c");
    let web_runtime = include_str!("../../../runtime/web/game.js");
    assert!(!desktop_aot.contains("play_error_toasts"));
    assert!(!mobile_runtime.contains("play_error_toasts"));
    assert!(!web_runtime.contains("play_error_toasts"));
}
