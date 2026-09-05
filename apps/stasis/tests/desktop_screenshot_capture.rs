#![cfg(windows)]

use serde_json::json;
use sha2::{Digest, Sha256};
use stasis::{run_live_in_process, LiveRunConfig};
use stasis_runner::live::{live_session, LiveCommand, LiveRequest, LiveSessionClient};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn copy_tree(source: &Path, destination: &Path) {
    fs::create_dir_all(destination).unwrap();
    for entry in fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.path().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn request(client: &LiveSessionClient, id: u64, command: LiveCommand) -> serde_json::Value {
    client.submit(LiveRequest::new(id, command)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let response = client
            .receive_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("bounded live response");
        if response.request_id == id {
            let value = serde_json::to_value(response).unwrap();
            assert_eq!(value["ok"], true, "{value}");
            return value;
        }
    }
}

struct QuitOnDrop(LiveSessionClient);
impl Drop for QuitOnDrop {
    fn drop(&mut self) {
        let _ = self.0.submit(LiveRequest::new(999, LiveCommand::Quit));
    }
}

#[test]
fn live_game_capture_completes_with_decodable_pixels_while_paused() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let evidence = repository.join("target/task515-evidence");
    let project = evidence.join(format!("game-{}", std::process::id()));
    fs::create_dir_all(&project).unwrap();
    copy_tree(
        &repository.join("src/stdlib"),
        &project.join(".stasis_cache/toolchain/src/stdlib"),
    );
    fs::write(
        project.join("main.stasis"),
        concat!(
        "import \"/.stasis_cache/toolchain/src/stdlib/graphics.stasis\";\n",
        "function main(): i32 { init_window(320, 180, \"Screenshot completion\"); return 0; }\n",
        "function tick(): i32 { return 0; }\n",
        "function on_code_swap(): void { return; }\n",
        "function render(): i32 { begin_frame(); clear(0.03, 0.06, 0.12, 1.0); ",
        "fill_rect(80.0, 45.0, 160.0, 90.0, 0.9, 0.15, 0.08, 1.0); end_frame(); return 0; }\n",
    ),
    )
    .unwrap();
    let (client, server) = live_session(16);
    let evidence_for_thread = evidence.clone();
    let consumer = std::thread::spawn(move || {
        let guard = QuitOnDrop(client);
        request(&guard.0, 1, LiveCommand::Pause);
        let response = request(
            &guard.0,
            2,
            LiveCommand::CaptureFrame {
                artifact: "active-task".into(),
            },
        );
        assert_eq!(response["kind"], "capture_completed");
        assert!(response["runtime_identity"].is_object(), "{response}");
        let data = &response["data"];
        assert_eq!(
            data["scheduled_tick"], data["captured_tick"],
            "capture must not advance a paused game"
        );
        let path = PathBuf::from(data["path"].as_str().unwrap());
        let bytes = fs::read(&path).unwrap();
        assert_eq!(data["byte_length"].as_u64(), Some(bytes.len() as u64));
        assert_eq!(data["sha256"], format!("{:x}", Sha256::digest(&bytes)));
        let pixels = image::open(&path)
            .expect("completion must imply a valid PNG")
            .to_rgba8();
        assert_eq!(u64::from(pixels.width()), data["width"].as_u64().unwrap());
        assert_eq!(u64::from(pixels.height()), data["height"].as_u64().unwrap());
        assert!(
            pixels
                .pixels()
                .filter(|p| p[0] > 170 && p[1] < 90 && p[2] < 80)
                .count()
                > 500
        );
        let retained = evidence_for_thread.join("live-game.png");
        fs::copy(path, &retained).unwrap();
        fs::write(
            evidence_for_thread.join("capture.json"),
            serde_json::to_vec_pretty(&json!({
                "response": response, "inspected_png": retained,
                "oracle": "paused live game produces red rectangle on dark background"
            }))
            .unwrap(),
        )
        .unwrap();
    });
    let result = run_live_in_process(
        &project.join("main.stasis"),
        Some(&project),
        16_000,
        Some(4000),
        server,
        LiveRunConfig::new(project.clone(), "main.stasis".into(), "build".into()),
    );
    let captured = consumer.join();
    result.expect("fresh live runtime");
    captured.expect("capture consumer");
}
