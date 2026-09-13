//! Opt-in native evidence for semantic plans recorded by live acceptance.
#[cfg(target_os = "windows")]
use super::*;

#[cfg(target_os = "windows")]
#[derive(serde::Deserialize)]
struct RecordedReview {
    task_id: String,
    objective: String,
    provider: String,
    model: String,
    action_id: String,
    description: String,
    plan: stasis_compiler::frontend::workshop::WorkshopSemanticEditPlan,
}

#[cfg(target_os = "windows")]
fn evidence_file_name(index: usize, task_id: &str, action_id: &str) -> String {
    let safe = |value: &str| {
        value
            .chars()
            .map(|character| {
                if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                    character
                } else {
                    '_'
                }
            })
            .collect::<String>()
    };
    format!(
        "recorded-review-{:02}-{}-{}.png",
        index + 1,
        safe(task_id),
        safe(action_id)
    )
}

#[cfg(target_os = "windows")]
#[test]
fn capture_recorded_live_proposal_reviews() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use winit::platform::windows::EventLoopBuilderExtWindows;

    let Ok(report_path) = std::env::var("STASIS_EDITOR_REVIEW_REPORT") else {
        return;
    };
    let output_dir = std::env::var("STASIS_EDITOR_REVIEW_OUTPUT_DIR")
        .expect("STASIS_EDITOR_REVIEW_OUTPUT_DIR is required with STASIS_EDITOR_REVIEW_REPORT");
    let width: f32 = std::env::var("STASIS_EDITOR_REVIEW_WIDTH")
        .unwrap_or_else(|_| "1200".into())
        .parse()
        .expect("STASIS_EDITOR_REVIEW_WIDTH must be a number");
    let height: f32 = std::env::var("STASIS_EDITOR_REVIEW_HEIGHT")
        .unwrap_or_else(|_| "900".into())
        .parse()
        .expect("STASIS_EDITOR_REVIEW_HEIGHT must be a number");
    let scale: f32 = std::env::var("STASIS_EDITOR_REVIEW_SCALE")
        .unwrap_or_else(|_| "1".into())
        .parse()
        .expect("STASIS_EDITOR_REVIEW_SCALE must be a number");

    let report_bytes = std::fs::read(&report_path).expect("read configured live report.json");
    let report: serde_json::Value =
        serde_json::from_slice(&report_bytes).expect("parse configured live report.json");
    assert_eq!(
        report.get("result").and_then(serde_json::Value::as_str),
        Some("passed"),
        "review evidence requires a passed final live report"
    );
    let reviews = report
        .get("tasks")
        .and_then(serde_json::Value::as_array)
        .expect("live report tasks must be an array")
        .iter()
        .cloned()
        .map(|task| {
            serde_json::from_value::<RecordedReview>(task).expect(
                "each live report task must contain its recorded reviewed plan and provenance",
            )
        })
        .collect::<Vec<_>>();
    assert!(
        !reviews.is_empty(),
        "configured live report contains no recorded proposal reviews"
    );
    std::fs::create_dir_all(&output_dir).expect("create review evidence output directory");

    struct CaptureApp {
        reviews: Vec<RecordedReview>,
        output_dir: PathBuf,
        index: usize,
        frames: usize,
        started: Instant,
        captured: Arc<AtomicUsize>,
        width: f32,
        height: f32,
        scale: f32,
        expanded: BTreeSet<String>,
    }

    impl eframe::App for CaptureApp {
        fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
            if self.index == self.reviews.len() {
                context.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            if self.frames < 2 {
                context.set_pixels_per_point(self.scale);
                context.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                    self.width,
                    self.height,
                )));
            }
            assert!(
                self.started.elapsed() < Duration::from_secs(45),
                "recorded review screenshots timed out"
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
                let review = &self.reviews[self.index];
                let output = self.output_dir.join(evidence_file_name(
                    self.index,
                    &review.task_id,
                    &review.action_id,
                ));
                let bytes = screenshot
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_array().into_iter().take(3))
                    .collect::<Vec<_>>();
                image::save_buffer(
                    output,
                    &bytes,
                    screenshot.width() as u32,
                    screenshot.height() as u32,
                    image::ColorType::Rgb8,
                )
                .expect("save recorded review screenshot");
                self.captured.fetch_add(1, Ordering::AcqRel);
                self.index += 1;
                if self.index == self.reviews.len() {
                    context.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
                self.frames = 0;
                clear_evidence(context);
            }

            configure_visuals(context);
            expand_for_evidence(context);
            let review = &self.reviews[self.index];
            egui::CentralPanel::default().show(context, |ui| {
                ui.heading("Recorded live proposal review");
                ui.label(
                    egui::RichText::new(format!(
                        "Task {} | Provider {} | Model {} | Action {}",
                        review.task_id, review.provider, review.model, review.action_id
                    ))
                    .strong(),
                );
                ui.label(&review.objective);
                ui.label(egui::RichText::new(&review.description).italics());
                ui.label(
                    "Retrospective evidence from report.json; this renderer does not execute the action.",
                );
                ui.separator();
                semantic_diff::render(
                    ui,
                    &review.plan,
                    ("recorded-live-proposal-review", self.index),
                    &format!("{}/{}", review.task_id, review.action_id),
                    &mut self.expanded,
                );
            });

            self.frames += 1;
            if self.frames == 8 {
                context.send_viewport_cmd(egui::ViewportCommand::Screenshot);
            }
            context.request_repaint();
        }
    }

    let captured = Arc::new(AtomicUsize::new(0));
    let receipt = captured.clone();
    let expected = reviews.len();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Stasis editor - recorded live proposal reviews")
            .with_inner_size([width, height]),
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    eframe::run_native(
        "Stasis recorded review evidence",
        options,
        Box::new(move |creation| {
            creation.egui_ctx.set_pixels_per_point(scale);
            Box::new(CaptureApp {
                reviews,
                output_dir: output_dir.into(),
                index: 0,
                frames: 0,
                started: Instant::now(),
                captured,
                width,
                height,
                scale,
                expanded: BTreeSet::new(),
            })
        }),
    )
    .expect("run recorded review evidence renderer");
    assert_eq!(receipt.load(Ordering::Acquire), expected);
}
