#[path = "../src/toolchain_cli/desktop_editor/semantic_diff.rs"]
mod semantic_diff;

use eframe::egui;
use stasis_compiler::frontend::workshop::{
    plan_workshop_semantic_edits, workshop_source_items, WorkshopSemanticEdit,
    WorkshopSemanticEditBatch, WorkshopSemanticEditOperation, WorkshopSemanticEditPlan,
    WorkshopSourceFile, WorkshopSourceItemKind, WorkshopSymbolSelector,
};
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

const MAIN_SOURCE: &str = "function main(): i32 { return 1; }\n\
function north(): i32 { return 10; }\n\
function east(): i32 { return 20; }\n\
function south(): i32 { return 30; }\n\
function west(): i32 { return 40; }\n\
function center(): i32 { return 50; }\n\
function upper(): i32 { return 60; }\n\
function lower(): i32 { return 70; }\n\
function entry(): i32 { return 80; }\n\
function exit(): i32 { return 90; }\n\
function left(): i32 { return 100; }\n\
function right(): i32 { return 110; }\n\
function near(): i32 { return 120; }\n\
function wide(): i32 { return 1 + 2 + 3 + 4 + 5 + 6 + 7 + 8 + 9 + 10 + 11 + 12 + 13 + 14 + 15 + 16 + 17 + 18 + 19 + 20 + 21 + 22 + 23 + 24 + 25; }\n\
function middle(): i32 { return 130; }\n\
function far(): i32 { return 140; }\n";
const PLAYER_SOURCE: &str = "function player(): i32 { return 7; }\n\
function obsolete(): i32 { return 0; }\n\
function score(): i32 { return 3; }\n";

struct EvidenceApp {
    plan: WorkshopSemanticEditPlan,
    expanded: bool,
    output: PathBuf,
    screenshot_requested: bool,
    settle_frames: u8,
}

impl eframe::App for EvidenceApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.settle_frames = self.settle_frames.saturating_add(1);
        ctx.set_visuals(egui::Visuals::dark());
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.heading("Stasis AI editor - semantic diff evidence");
            ui.label("Compiler-owned plan preview: update, add, delete across two files");
            egui::ScrollArea::vertical().show(ui, |ui| {
                if self.expanded {
                    force_expanded(ui, &self.plan);
                }
                semantic_diff::render(ui, &self.plan, "evidence-diff");
            });
        });

        if !self.screenshot_requested && self.settle_frames >= 4 {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        } else if !self.screenshot_requested {
            ctx.request_repaint_after(Duration::from_millis(50));
        }
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Screenshot { image, .. } = event {
                save_screenshot(&self.output, &image).unwrap_or_else(|error| {
                    panic!(
                        "failed to save semantic diff evidence {}: {error}",
                        self.output.display()
                    )
                });
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }
    }
}

fn force_expanded(ui: &egui::Ui, plan: &WorkshopSemanticEditPlan) {
    let base_id = ui.make_persistent_id("evidence-diff");
    for (index, change) in plan.changed_files.iter().enumerate() {
        let id = base_id.with(("semantic-file", index, change.file.as_str()));
        let mut state =
            egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), id, true);
        state.set_open(true);
        state.store(ui.ctx());
    }
}

fn save_screenshot(path: &Path, image: &egui::ColorImage) -> Result<(), String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }
    let rgba = image
        .pixels
        .iter()
        .flat_map(|pixel| pixel.to_srgba_unmultiplied())
        .collect::<Vec<_>>();
    image::save_buffer(
        path,
        &rgba,
        image.size[0] as u32,
        image.size[1] as u32,
        image::ColorType::Rgba8,
    )
    .map_err(|error| format!("failed to write PNG: {error}"))
}

fn evidence_plan() -> WorkshopSemanticEditPlan {
    let files = vec![
        WorkshopSourceFile {
            path: "src/main.stasis".to_string(),
            source: MAIN_SOURCE.to_string(),
        },
        WorkshopSourceFile {
            path: "src/player.stasis".to_string(),
            source: PLAYER_SOURCE.to_string(),
        },
    ];
    let items = workshop_source_items(&files).expect("evidence source parses");
    let hash_for = |file: &str, name: &str| {
        items
            .iter()
            .find(|item| item.file == file && item.name == name)
            .map(|item| item.source_hash.clone())
            .expect("evidence symbol exists")
    };
    let batch = WorkshopSemanticEditBatch {
        schema_version: 1,
        edits: vec![
            WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: selector("main", "src/main.stasis"),
                new_source: Some("function main(): i32 { return 42; }".to_string()),
                expected_source_hash: Some(hash_for("src/main.stasis", "main")),
            },
            WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Update,
                target: selector("far", "src/main.stasis"),
                new_source: Some("function far(): i32 { return 777; }".to_string()),
                expected_source_hash: Some(hash_for("src/main.stasis", "far")),
            },
            WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Add,
                target: selector("bonus", "src/player.stasis"),
                new_source: Some("function bonus(): i32 { return 99; }".to_string()),
                expected_source_hash: None,
            },
            WorkshopSemanticEdit {
                operation: WorkshopSemanticEditOperation::Delete,
                target: selector("obsolete", "src/player.stasis"),
                new_source: None,
                expected_source_hash: Some(hash_for("src/player.stasis", "obsolete")),
            },
        ],
    };
    plan_workshop_semantic_edits(&files, &batch)
        .map(|(_, plan)| plan)
        .expect("evidence semantic plan")
}

fn selector(name: &str, file: &str) -> WorkshopSymbolSelector {
    WorkshopSymbolSelector {
        symbol_id: None,
        name: name.to_string(),
        kind: Some(WorkshopSourceItemKind::Function),
        file: Some(file.to_string()),
        owner: None,
        signature: None,
    }
}

fn parse_args() -> Result<(bool, PathBuf), String> {
    let mut expanded = false;
    let mut output = PathBuf::from("target/semantic-diff-compact.png");
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--expanded" => expanded = true,
            "--output" => {
                output = args
                    .next()
                    .map(PathBuf::from)
                    .ok_or_else(|| "--output requires a path".to_string())?;
            }
            "--help" | "-h" => {
                println!("semantic diff evidence");
                println!("  --expanded           render all file bodies expanded at 1100x760");
                println!("  --output <path>      PNG destination");
                std::process::exit(0);
            }
            unknown => return Err(format!("unknown argument '{unknown}'")),
        }
    }
    Ok((expanded, output))
}

fn main() -> eframe::Result<()> {
    let (expanded, output) = parse_args().unwrap_or_else(|error| {
        eprintln!("{error}");
        std::process::exit(2);
    });
    let (width, height) = if expanded {
        (1100.0, 700.0)
    } else {
        (640.0, 480.0)
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([width, height])
            .with_min_inner_size([width, height]),
        ..Default::default()
    };
    eframe::run_native(
        "Stasis semantic diff evidence",
        options,
        Box::new(move |_creation_context| {
            Box::new(EvidenceApp {
                plan: evidence_plan(),
                expanded,
                output,
                screenshot_requested: false,
                settle_frames: 0,
            })
        }),
    )
}
