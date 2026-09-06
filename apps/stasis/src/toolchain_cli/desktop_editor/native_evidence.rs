//! Opt-in native renderer evidence; never contacts a provider or live runtime.
#[cfg(target_os = "windows")]
use super::*;

#[cfg(target_os = "windows")]
#[test]
fn capture_native_task_timeline() {
    use stasis_ai::task_session::FocusedTestResult;
    use winit::platform::windows::EventLoopBuilderExtWindows;

    let Ok(output) = std::env::var("STASIS_EDITOR_EVIDENCE_PNG") else {
        return;
    };
    let width: f32 = std::env::var("STASIS_EDITOR_EVIDENCE_WIDTH")
        .unwrap_or_else(|_| "1100".into())
        .parse()
        .unwrap();
    let scale: f32 = std::env::var("STASIS_EDITOR_EVIDENCE_SCALE")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .unwrap();
    let repair = std::env::var_os("STASIS_EDITOR_EVIDENCE_REPAIR").is_some();
    let (client, _server) = stasis_runner::live::live_session(16);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap();
    let mut editor = DesktopEditor::new(client, root.clone(), Arc::new(AtomicBool::new(false)));
    for objective in [
        "Improve enemy movement",
        "Add an arena tileset",
        "Polish the pause menu",
        "Add dash ability",
    ] {
        editor.state.objective = objective.into();
        editor.state.create_task().unwrap();
    }
    let task = editor.state.session.active_task_mut().unwrap();
    task.set_vision_capability(true).unwrap();
    task.set_provider_state(configured_provider_state(&ProviderConfig::Codex))
        .unwrap();
    task.append_reply("Add a short dash with a cooldown and a brief invulnerability window. Keep movement deterministic between ticks.").unwrap();
    let asset = root.join("samples/asset_breakout/assets/arena_background.png");
    let bytes = std::fs::read(&asset).unwrap();
    let sha256 = format!("{:x}", Sha256::digest(&bytes));
    let pixels = image::load_from_memory(&bytes).unwrap().to_rgba8();
    task.attach_screenshot_with_sha256("arena-reference", asset.display().to_string(), &sha256)
        .unwrap();
    let task_id = task.id.clone();
    task.append_result("I will update player movement and add focused coverage for the cooldown and collision behavior.").unwrap();
    let attachment_mode = std::env::var("STASIS_EDITOR_EVIDENCE_ATTACHMENTS").ok();
    if attachment_mode.is_none() {
        task.propose_action(
            "dash",
            "Add tick-based dash movement and cooldown to the player.",
        )
        .unwrap();
        task.accept_action("dash").unwrap();
        task.apply_action("dash").unwrap();
        task.begin_focused_tests().unwrap();
        if repair {
            task.finish_focused_tests(FocusedTestResult::failed(
                "Cooldown boundary: expected 18 ticks, observed 17.",
            ))
            .unwrap();
            task.mark_action_for_repair(
                "dash",
                "Keep invulnerability active through the final dash tick.",
            )
            .unwrap();
        } else {
            task.finish_focused_tests(FocusedTestResult::passed(
                "Dash distance and cooldown boundaries passed.",
            ))
            .unwrap();
        }
    } else if attachment_mode.as_deref() != Some("reference") {
        task.add_generated_image(
            "arena-asset",
            asset.display().to_string(),
            stasis_ai::task_session::ImageAttribution::new(
                "Local UI fixture",
                None,
                Some("Repository arena artwork".into()),
            )
            .unwrap(),
        )
        .unwrap();
    }
    task.record_turn(1840, 2410, 386, 1200).unwrap();
    editor.state.preview = Some(ScreenshotPreview {
        task_id,
        screenshot_id: "arena-reference".into(),
        path: asset,
        width: pixels.width() as usize,
        height: pixels.height() as usize,
        rgba: pixels.into_raw(),
        scheduled_tick: 120,
        captured_tick: 120,
        sha256,
        runtime_identity: LiveRuntimeIdentity {
            session_id: "native-ui-fixture".into(),
            generation: 1,
            source_hashes: BTreeMap::new(),
            indexed_collections: Vec::new(),
            complete: true,
        },
    });
    let semantic_root = if std::env::var_os("STASIS_EDITOR_EVIDENCE_SEMANTIC").is_some() {
        let (mut preview_editor, root, _) = super::tests::review_fixture("merged_timeline_native");
        super::tests::finish_preview(&mut preview_editor);
        editor = preview_editor;
        Some(root)
    } else {
        None
    };
    editor.state.focus = FocusArea::Game;
    if std::env::var_os("STASIS_EDITOR_EVIDENCE_CANCEL").is_some() {
        editor.state.handle(TaskSessionCommand::Cancel).unwrap();
    }

    struct CaptureApp {
        editor: DesktopEditor,
        output: PathBuf,
        frames: usize,
        started: Instant,
        captured: Arc<AtomicBool>,
        width: f32,
        scale: f32,
    }
    impl eframe::App for CaptureApp {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
            if self.frames < 2 {
                context.set_pixels_per_point(self.scale);
                context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    self.width, 900.0,
                )));
            }
            assert!(
                self.started.elapsed() < Duration::from_secs(30),
                "native screenshot timed out"
            );
            let screenshot = context.input(|input| {
                input.events.iter().find_map(|event| {
                    if let egui::Event::Screenshot { image, .. } = event {
                        Some(image.clone())
                    } else {
                        None
                    }
                })
            });
            if let Some(screenshot) = screenshot {
                let bytes: Vec<u8> = screenshot
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array())
                    .collect();
                image::save_buffer(
                    &self.output,
                    &bytes,
                    screenshot.width() as u32,
                    screenshot.height() as u32,
                    image::ColorType::Rgba8,
                )
                .unwrap();
                self.captured.store(true, Ordering::Release);
                context.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            self.editor.ui(context);
            self.frames += 1;
            if self.frames == 8 {
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            context.request_repaint();
        }
    }
    let captured = Arc::new(AtomicBool::new(false));
    let receipt = captured.clone();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stasis editor - deterministic review fixture")
            .with_inner_size([width, 900.0]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Stasis evidence",
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_pixels_per_point(scale);
            Box::new(CaptureApp {
                editor,
                output: output.into(),
                frames: 0,
                started: Instant::now(),
                captured,
                width,
                scale,
            })
        }),
    )
    .unwrap();
    assert!(receipt.load(Ordering::Acquire));
    if let Some(root) = semantic_root {
        std::fs::remove_dir_all(root).unwrap();
    }
}
