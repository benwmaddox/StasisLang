//! Render the real failed-Apply/repair snapshots produced by
//! `cargo test -p stasis --bin stasis failed_apply_and_repair_keep_one_card_per_proposal`.
//! Run with `cargo run -p stasis --example semantic_revision_evidence`.
#[path = "../src/toolchain_cli/desktop_editor/semantic_diff.rs"]
mod semantic_diff;
#[path = "../src/toolchain_cli/desktop_editor/semantic_revisions.rs"]
mod semantic_revisions;

use eframe::egui;
use serde::Deserialize;
use stasis_ai::task_session::Task;
use stasis_compiler::frontend::workshop::WorkshopSemanticEditPlan;
use std::{fs, path::PathBuf, time::Duration};

#[derive(Deserialize)]
struct Plan {
    revision: usize,
    plan: WorkshopSemanticEditPlan,
}

#[derive(Deserialize)]
struct Snapshot {
    label: String,
    task: Task,
    plans: Vec<Plan>,
}

struct EvidenceApp {
    snapshots: Vec<Snapshot>,
    directory: PathBuf,
    frame: usize,
    settle_frames: u8,
    requested: bool,
}

impl eframe::App for EvidenceApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.frame == self.snapshots.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.settle_frames += 1;
        ctx.set_visuals(egui::Visuals::dark());
        let snapshot = &self.snapshots[self.frame];
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Stasis AI editor - failed Apply and repair");
            ui.label(&snapshot.label);
            ui.label(format!("Task: {}", snapshot.task.objective));
            egui::ScrollArea::vertical().show(ui, |ui| {
                for action in snapshot.task.actions.values() {
                    for proposal in semantic_revisions::proposal_revisions(action) {
                        ui.push_id((action.id.as_str(), proposal.revision), |ui| {
                            ui.group(|ui| {
                                semantic_revisions::render_heading(
                                    ui,
                                    action.id.as_str(),
                                    &proposal,
                                );
                                let plan = snapshot
                                    .plans
                                    .iter()
                                    .find(|plan| plan.revision == proposal.revision)
                                    .expect("every proposal retains its real compiler preview");
                                semantic_diff::render(ui, &plan.plan, "semantic-files");
                            });
                        });
                    }
                }
            });
        });

        if !self.requested && self.settle_frames >= 4 {
            self.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        for event in ctx.input(|input| input.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event {
                let rgba = image
                    .pixels
                    .iter()
                    .flat_map(|pixel| pixel.to_srgba_unmultiplied())
                    .collect::<Vec<_>>();
                let path = self
                    .directory
                    .join(format!("repair-{}.png", self.frame + 1));
                image::save_buffer(
                    &path,
                    &rgba,
                    image.size[0] as u32,
                    image.size[1] as u32,
                    image::ColorType::Rgba8,
                )
                .expect("save native repair evidence");
                self.frame += 1;
                if self.frame == self.snapshots.len() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                } else {
                    self.requested = false;
                    self.settle_frames = 0;
                }
            }
        }
        ctx.request_repaint_after(Duration::from_millis(75));
    }
}

fn main() -> eframe::Result<()> {
    let directory = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/task-520-review");
    let source = fs::read(directory.join("failed-apply-repair.json"))
        .expect("run failed_apply_and_repair_keep_one_card_per_proposal first");
    let snapshots: Vec<Snapshot> = serde_json::from_slice(&source).expect("valid host evidence");
    assert_eq!(snapshots.len(), 4);
    eframe::run_native(
        "Stasis repair revision evidence",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default().with_inner_size([1000.0, 680.0]),
            ..Default::default()
        },
        Box::new(move |_| {
            Box::new(EvidenceApp {
                snapshots,
                directory,
                frame: 0,
                settle_frames: 0,
                requested: false,
            })
        }),
    )
}
