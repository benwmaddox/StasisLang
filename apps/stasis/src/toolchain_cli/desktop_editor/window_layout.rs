use super::*;
use serde::{Deserialize, Serialize};

/// SDL desktop coordinates, including window decorations. Negative origins are valid.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
struct Rect([i32; 4]);

impl Rect {
    fn valid(self) -> bool {
        self.0[2] > 0 && self.0[3] > 0
    }

    fn fit(self, bounds: Self) -> Self {
        let [x, y, w, h] = self.0;
        let [bx, by, bw, bh] = bounds.0;
        let w = w.clamp(1, bw);
        let h = h.clamp(1, bh);
        Self([
            (x as i64).clamp(bx as i64, bx as i64 + (bw - w) as i64) as i32,
            (y as i64).clamp(by as i64, by as i64 + (bh - h) as i64) as i32,
            w,
            h,
        ])
    }
}

fn tiled(bounds: Rect) -> (Rect, Rect) {
    let [x, y, w, h] = bounds.0;
    let gap = 8.min(w / 8);
    let left = (w - gap) / 2;
    (
        Rect([x, y, left, h]),
        Rect([x + left + gap, y, w - left - gap, h]),
    )
}

#[derive(Clone, Copy, PartialEq, Serialize, Deserialize)]
struct SavedPlacement {
    editor: Rect,
    game: Rect,
}

fn normal_placement(
    saved: Option<SavedPlacement>,
    editor: Option<Rect>,
    game: Option<Rect>,
) -> Option<SavedPlacement> {
    Some(SavedPlacement {
        editor: editor.or(saved.map(|saved| saved.editor))?,
        game: game.or(saved.map(|saved| saved.game))?,
    })
}

pub(super) struct WindowLayout {
    client: LiveSessionClient,
    path: PathBuf,
    saved: Option<SavedPlacement>,
    startup: bool,
    tile: bool,
    next_request: u64,
    pending: Option<(u64, Instant)>,
    next_poll: Instant,
    editor_target: Option<(Rect, f32, u8)>,
    editor_coordinate_density: f32,
}

impl WindowLayout {
    pub(super) fn new(client: LiveSessionClient, root: &std::path::Path) -> Self {
        let path = root.join(".stasis_cache/editor-windows.json");
        let saved = std::fs::read(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<SavedPlacement>(&bytes).ok())
            .filter(|saved| saved.editor.valid() && saved.game.valid());
        Self {
            client,
            path,
            saved,
            startup: true,
            tile: false,
            next_request: 1,
            pending: None,
            next_poll: Instant::now(),
            editor_target: None,
            editor_coordinate_density: 1.0,
        }
    }

    fn submit(&mut self, command: LiveCommand) -> Result<u64, String> {
        let id = self.next_request;
        self.next_request += 1;
        self.client.submit(LiveRequest::new(id, command))?;
        Ok(id)
    }

    pub(super) fn tile(&mut self) {
        self.tile = true;
        self.next_poll = Instant::now();
    }

    pub(super) fn focus_game(&mut self) -> Result<(), String> {
        self.submit(LiveCommand::FocusGameWindow).map(|_| ())
    }

    pub(super) fn update(&mut self, context: &egui::Context) -> Result<(), String> {
        // Windows and X11 use physical screen coordinates; macOS and Wayland
        // use window coordinates whose native density follows the viewport.
        if cfg!(target_os = "macos")
            || (cfg!(target_os = "linux") && std::env::var_os("WAYLAND_DISPLAY").is_some())
        {
            self.editor_coordinate_density = context
                .input(|input| input.viewport().native_pixels_per_point)
                .unwrap_or(self.editor_coordinate_density);
        }
        if let Some((target, density, frames)) = self.editor_target.take() {
            // Moving between monitors may change native scale and decoration sizes.
            // Size on subsequent frames using the destination viewport's scale.
            let viewport = context.input(|input| input.viewport().clone());
            let scale = context.pixels_per_point() / density;
            let decoration = viewport
                .outer_rect
                .zip(viewport.inner_rect)
                .map(|(outer, inner)| outer.size() - inner.size())
                .unwrap_or(egui::vec2(16.0, 40.0));
            let [x, y, width, height] = target.0;
            context.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                x as f32 / scale,
                y as f32 / scale,
            )));
            let inner = (egui::vec2(width as f32 / scale, height as f32 / scale) - decoration)
                .max(egui::vec2(1.0, 1.0));
            // On small work areas the OS bounds take priority over the preferred minimum.
            context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(
                inner.min(egui::vec2(520.0, 600.0)),
            ));
            context.send_viewport_cmd(egui::ViewportCommand::InnerSize(inner));
            if frames > 1 {
                self.editor_target = Some((target, density, frames - 1));
            }
        }
        while let Some(response) = self.client.try_receive()? {
            if !response.ok {
                self.pending = None;
                self.next_poll = Instant::now() + Duration::from_secs(5);
                return Err(response
                    .error
                    .unwrap_or_else(|| "Window command failed".into()));
            }
            if self.pending.map(|pending| pending.0) != Some(response.request_id) {
                continue;
            }
            self.pending = None;
            let data = response
                .data
                .ok_or("Window placement response has no data")?;
            self.placement(context, &data)?;
        }
        if self
            .pending
            .is_some_and(|(_, sent)| sent.elapsed() > Duration::from_secs(5))
        {
            self.pending = None;
            self.next_poll = Instant::now() + Duration::from_secs(5);
            return Err("Game window did not respond; tiling can be retried when connected".into());
        }
        if self.pending.is_none() && Instant::now() >= self.next_poll {
            let center = |rect: Rect| {
                let [x, y, w, h] = rect.0;
                [x.saturating_add(w / 2), y.saturating_add(h / 2)]
            };
            let saved = self.saved.filter(|_| self.startup && !self.tile);
            let id = self.submit(LiveCommand::WindowPlacement {
                editor_point: saved.map(|saved| center(saved.editor)),
                game_point: saved.map(|saved| center(saved.game)),
            })?;
            self.pending = Some((id, Instant::now()));
            self.next_poll = Instant::now() + Duration::from_secs(2);
        }
        Ok(())
    }

    fn placement(&mut self, context: &egui::Context, data: &Value) -> Result<(), String> {
        let parse = |key: &str| -> Result<Rect, String> {
            let rect: Rect =
                serde_json::from_value(data[key].clone()).map_err(|e| e.to_string())?;
            rect.valid()
                .then_some(rect)
                .ok_or_else(|| format!("Invalid window {key}"))
        };
        let monitor = parse("monitor")?;
        let game = parse("outer")?;
        let density = data["pixel_density"].as_f64().unwrap_or(1.0) as f32;
        if !density.is_finite() || density <= 0.0 {
            return Err("Invalid game window pixel density".into());
        }
        let viewport = context.input(|input| input.viewport().clone());
        let scale = context.pixels_per_point() / self.editor_coordinate_density;
        if self.startup || self.tile {
            let editor_density = if self.saved.is_some() && !self.tile {
                data["editor_monitor_pixel_density"]
                    .as_f64()
                    .unwrap_or(density as f64) as f32
            } else {
                density
            };
            if !editor_density.is_finite() || editor_density <= 0.0 {
                return Err("Invalid editor monitor pixel density".into());
            }
            self.editor_coordinate_density = editor_density;
            let scale = context.pixels_per_point() / editor_density;
            let (editor, game) = if self.tile {
                tiled(monitor)
            } else if let Some(saved) = self.saved {
                (
                    saved.editor.fit(parse("editor_monitor")?),
                    saved.game.fit(parse("game_monitor")?),
                )
            } else {
                tiled(monitor)
            };
            let [x, y, width, height] = game.0;
            self.submit(LiveCommand::PlaceGameWindow {
                x,
                y,
                width,
                height,
            })?;
            let [x, y, _, _] = editor.0;
            context.send_viewport_cmd(egui::ViewportCommand::Maximized(false));
            context.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
            context.send_viewport_cmd(egui::ViewportCommand::OuterPosition(egui::pos2(
                x as f32 / scale,
                y as f32 / scale,
            )));
            self.editor_target = Some((editor, editor_density, 3));
            self.startup = false;
            self.tile = false;
            return Ok(());
        }
        if self.editor_target.is_some() {
            return Ok(());
        }
        // Each native window retains its normal placement independently.
        let editor = viewport
            .outer_rect
            .filter(|_| viewport.minimized != Some(true) && viewport.maximized != Some(true))
            .map(|outer| {
                Rect([
                    (outer.min.x * scale).round() as i32,
                    (outer.min.y * scale).round() as i32,
                    (outer.width() * scale).round() as i32,
                    (outer.height() * scale).round() as i32,
                ])
            })
            .filter(|rect| rect.valid());
        let game = (data["minimized"] != true && data["maximized"] != true).then_some(game);
        if let Some(saved) = normal_placement(self.saved, editor, game) {
            if self.saved == Some(saved) {
                return Ok(());
            }
            std::fs::create_dir_all(self.path.parent().unwrap()).map_err(|e| e.to_string())?;
            let mut file =
                atomic_write_file::AtomicWriteFile::open(&self.path).map_err(|e| e.to_string())?;
            serde_json::to_writer(&mut file, &saved).map_err(|e| e.to_string())?;
            file.commit().map_err(|e| e.to_string())?;
            self.saved = Some(saved);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_negative_monitor_without_overlap_or_offscreen_edges() {
        let (left, right) = tiled(Rect([-1920, -200, 1920, 1040]));
        assert_eq!(left, Rect([-1920, -200, 956, 1040]));
        assert_eq!(right, Rect([-956, -200, 956, 1040]));
        assert_eq!(right.0[0] + right.0[2], 0);
    }

    #[test]
    fn removed_monitor_and_oversized_saved_window_recover_inside_work_area() {
        let bounds = Rect([0, 0, 1280, 720]);
        assert_eq!(Rect([-3000, -1000, 2000, 1500]).fit(bounds), bounds);
        assert_eq!(
            Rect([3000, 2000, 600, 400]).fit(bounds),
            Rect([680, 320, 600, 400])
        );
    }

    #[test]
    fn focus_request_does_not_resize_pause_or_override_game_input() {
        let (client, server) = stasis_runner::live::live_session(4);
        let mut layout = WindowLayout::new(client, std::path::Path::new("nonexistent-layout-test"));
        layout.focus_game().unwrap();
        let requests = server.drain(4);
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].command, LiveCommand::FocusGameWindow);
    }

    #[test]
    fn restore_retains_separate_connected_monitors() {
        let (client, server) = stasis_runner::live::live_session(4);
        let mut layout = WindowLayout::new(client, std::path::Path::new("nonexistent-layout-test"));
        let editor = Rect([-1800, 100, 900, 800]);
        let game = Rect([200, 100, 900, 800]);
        layout.saved = Some(SavedPlacement { editor, game });
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            layout
                .placement(
                    context,
                    &json!({
                            "outer": [0, 0, 800, 600], "monitor": [0, 0, 1920, 1040],
                            "editor_monitor": [-1920, 0, 1920, 1040],
                    "game_monitor": [0, 0, 1920, 1040], "pixel_density": 1.0,
                    "editor_monitor_pixel_density": 2.0
                        }),
                )
                .unwrap();
        });
        assert_eq!(layout.editor_target.unwrap().0, editor);
        assert_eq!(layout.editor_target.unwrap().1, 2.0);
        assert_eq!(layout.editor_coordinate_density, 2.0);
        assert_eq!(
            server.drain(4)[0].command,
            LiveCommand::PlaceGameWindow {
                x: 200,
                y: 100,
                width: 900,
                height: 800
            }
        );
    }

    #[test]
    fn minimized_game_does_not_overwrite_normal_saved_rectangles() {
        let (client, _server) = stasis_runner::live::live_session(4);
        let mut layout = WindowLayout::new(client, std::path::Path::new("nonexistent-layout-test"));
        layout.startup = false;
        let saved = SavedPlacement {
            editor: Rect([0, 0, 800, 900]),
            game: Rect([820, 0, 800, 900]),
        };
        layout.saved = Some(saved);
        let context = egui::Context::default();
        let _ = context.run(egui::RawInput::default(), |context| {
            layout
                .placement(
                    context,
                    &json!({
                        "outer": [-32000, -32000, 160, 28], "monitor": [0, 0, 1920, 1040],
                        "pixel_density": 1.0, "minimized": true
                    }),
                )
                .unwrap();
        });
        assert!(layout.saved == Some(saved));
    }

    #[test]
    fn normal_windows_are_remembered_while_the_other_is_minimized_or_maximized() {
        let old = SavedPlacement {
            editor: Rect([0, 0, 800, 600]),
            game: Rect([820, 0, 800, 600]),
        };
        let moved = Rect([100, 100, 900, 700]);
        let editor_moved = normal_placement(Some(old), Some(moved), None).unwrap();
        assert_eq!(editor_moved.editor, moved);
        assert_eq!(editor_moved.game, old.game);
        let game_moved = normal_placement(Some(old), None, Some(moved)).unwrap();
        assert_eq!(game_moved.editor, old.editor);
        assert_eq!(game_moved.game, moved);
    }

    #[test]
    fn tiling_uses_destination_scale_after_viewport_moves() {
        let (client, _server) = stasis_runner::live::live_session(4);
        let mut layout = WindowLayout::new(client, std::path::Path::new("nonexistent-layout-test"));
        layout.editor_target = Some((Rect([1920, 0, 1280, 1400]), 1.0, 1));
        layout.next_poll = Instant::now() + Duration::from_secs(60);
        let context = egui::Context::default();
        let mut input = egui::RawInput::default();
        let viewport = input.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        viewport.native_pixels_per_point = Some(2.0);
        viewport.outer_rect = Some(egui::Rect::from_min_size(
            egui::pos2(960.0, 0.0),
            egui::vec2(640.0, 700.0),
        ));
        viewport.inner_rect = Some(egui::Rect::from_min_size(
            egui::pos2(964.0, 16.0),
            egui::vec2(632.0, 680.0),
        ));
        let output = context.run(input, |context| layout.update(context).unwrap());
        let commands = &output.viewport_output[&egui::ViewportId::ROOT].commands;
        assert!(
            commands.contains(&egui::ViewportCommand::OuterPosition(egui::pos2(
                960.0, 0.0
            )))
        );
        assert!(commands.contains(&egui::ViewportCommand::InnerSize(egui::vec2(632.0, 680.0))));
    }
}
