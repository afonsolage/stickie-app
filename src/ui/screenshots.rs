//! Screenshots tab: capture the game, crop and annotate the result, then keep
//! it in the game workspace (or drop it into the open note).
use bevy_egui::egui;

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use egui::{RichText, ScrollArea, TextureHandle, TextureOptions};

use crate::store::Store;

use super::{ScreenshotEditor, ScreenshotEditorEvent, UiAction, theme};

/// Screenshots-tab local state.
#[derive(Default)]
pub struct ScreenshotsTab {
    captures: Vec<PathBuf>,
    editor: Option<ScreenshotEditor>,
    pending_open: Option<PathBuf>,
    thumbnails: HashMap<PathBuf, TextureHandle>,
    error: Option<String>,
    insert_request: Option<PathBuf>,
}

impl ScreenshotsTab {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn reload(&mut self, store: &Store) {
        self.captures = store.list_screenshots().unwrap_or_default();
        self.editor = None;
        self.pending_open = None;
        self.thumbnails.clear();
        self.error = None;
        self.insert_request = None;
    }

    /// Queues a freshly captured screenshot to be opened in the editor.
    pub fn queue_open(&mut self, path: PathBuf) {
        self.pending_open = Some(path);
    }

    /// Consumes a request to insert the last saved screenshot into the note.
    pub fn take_insert_request(&mut self) -> Option<PathBuf> {
        self.insert_request.take()
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        store: &mut Store,
        capture_supported: bool,
        actions: &mut Vec<UiAction>,
    ) -> Option<(String, theme::ToastKind)> {
        let mut notice = None;
        if let Some(path) = self.pending_open.take() {
            match ScreenshotEditor::open(ui.ctx(), path.clone()) {
                Ok(editor) => {
                    self.editor = Some(editor);
                    self.error = None;
                }
                Err(error) => {
                    self.error = Some(format!("Could not open screenshot: {error}"));
                }
            }
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Screenshots")
                        .font(theme::font_title())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new("Crop, draw, save. Game captures stay inside this workspace.")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if !capture_supported {
                    theme::pill(ui, "Windows only", theme::TEXT_FAINT);
                } else if theme::primary_button(ui, "Capture").clicked() {
                    actions.push(UiAction::CaptureScreenshot);
                }
                if self.editor.is_some() {
                    if ui.button("Back to library").clicked() {
                        self.editor = None;
                        self.captures = store.list_screenshots().unwrap_or_default();
                    }
                } else if ui.button("Refresh").clicked() {
                    self.captures = store.list_screenshots().unwrap_or_default();
                }
            });
        });
        ui.add_space(8.0);

        if let Some(error) = &self.error {
            ui.label(
                RichText::new(error)
                    .font(theme::font_small())
                    .color(theme::DANGER),
            );
            ui.add_space(4.0);
        }

        if let Some(editor) = &mut self.editor {
            let event = editor.show(ui);
            match event {
                Some(ScreenshotEditorEvent::Close) => {
                    self.editor = None;
                    self.captures = store.list_screenshots().unwrap_or_default();
                }
                Some(ScreenshotEditorEvent::Saved(path)) => {
                    notice = Some((
                        format!("Saved {}.", file_label(&path)),
                        theme::ToastKind::Success,
                    ));
                    self.captures = store.list_screenshots().unwrap_or_default();
                    self.thumbnails.clear();
                }
                Some(ScreenshotEditorEvent::InsertIntoNote(path)) => {
                    self.insert_request = Some(path);
                    self.editor = None;
                }
                None => {}
            }
            return notice;
        }

        if self.captures.is_empty() {
            let action = theme::empty_state(
                ui,
                "C",
                "No screenshots yet",
                if capture_supported {
                    "Capture the game window, then crop and draw on the result before saving it \
                     to this workspace."
                } else {
                    "Screenshot capture is available on Windows. The gallery will still show \
                     captures already present in this workspace."
                },
                capture_supported.then_some("Capture now"),
            );
            if action.is_some_and(|response| response.clicked()) {
                actions.push(UiAction::CaptureScreenshot);
            }
            return notice;
        }

        ui.label(
            RichText::new(format!(
                "{} capture{}",
                self.captures.len(),
                if self.captures.len() == 1 { "" } else { "s" }
            ))
            .font(theme::font_small())
            .color(theme::TEXT_MUTED),
        );
        ui.add_space(4.0);

        // Preload thumbnails before borrowing the capture list inside the
        // scroll closure, so the closure only needs immutable state.
        let captures = self.captures.clone();
        for path in &captures {
            let _ = self.thumbnail(ui.ctx(), path);
        }
        let mut pending_editor: Option<PathBuf> = None;
        let area_height = (ui.available_height() - 4.0).max(160.0);
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(area_height)
            .show(ui, |ui| {
                let gap = 12.0;
                let available = ui.available_width();
                let columns = ((available + gap) / (260.0 + gap)).floor().clamp(1.0, 3.0) as usize;
                let card_width =
                    ((available - gap * (columns - 1) as f32) / columns as f32).clamp(220.0, 360.0);
                let mut index = 0;
                while index < captures.len() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for offset in 0..columns {
                            let capture_index = index + offset;
                            if capture_index >= captures.len() {
                                break;
                            }
                            let path = captures[capture_index].clone();
                            ui.push_id(&path, |ui| {
                                ui.allocate_ui_with_layout(
                                    egui::vec2(card_width, 166.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        let frame_response = egui::Frame::NONE
                                            .fill(theme::PANEL_BG_RAISED)
                                            .stroke(egui::Stroke::new(1.0, theme::BORDER))
                                            .corner_radius(egui::CornerRadius::same(
                                                theme::RADIUS_MD as u8,
                                            ))
                                            .inner_margin(egui::Margin::same(8))
                                            .show(ui, |ui| {
                                                ui.set_width(card_width - 16.0);
                                                ui.set_min_height(150.0);
                                                ui.vertical(|ui| {
                                                    if let Some(texture) =
                                                        self.thumbnails.get(&path)
                                                    {
                                                        let size = texture.size_vec2();
                                                        let max =
                                                            egui::vec2(card_width - 16.0, 112.0);
                                                        let scale = (max.x / size.x)
                                                            .min(max.y / size.y)
                                                            .min(1.0);
                                                        ui.add(
                                                            egui::Image::new(texture)
                                                                .fit_to_exact_size(size * scale),
                                                        );
                                                    } else {
                                                        ui.allocate_space(egui::vec2(
                                                            card_width - 16.0,
                                                            112.0,
                                                        ));
                                                    }
                                                    ui.add_space(4.0);
                                                    ui.label(
                                                        RichText::new(file_label(&path))
                                                            .font(theme::font_small())
                                                            .color(theme::TEXT_MUTED),
                                                    );
                                                });
                                            });
                                        let response =
                                            frame_response.response.interact(egui::Sense::click());
                                        if response.on_hover_text("Edit screenshot").clicked() {
                                            pending_editor = Some(path.clone());
                                        }
                                    },
                                );
                            });
                        }
                    });
                    index += columns;
                    ui.add_space(gap);
                }
            });

        if let Some(path) = pending_editor {
            match ScreenshotEditor::open(ui.ctx(), path) {
                Ok(editor) => self.editor = Some(editor),
                Err(error) => {
                    self.error = Some(format!("Could not open screenshot: {error}"));
                }
            }
        }
        notice
    }

    fn thumbnail(&mut self, ctx: &egui::Context, path: &Path) -> Option<&TextureHandle> {
        if !self.thumbnails.contains_key(path) {
            let image = image::open(path).ok()?.to_rgba8();
            let size = [image.width() as usize, image.height() as usize];
            let color_image = egui::ColorImage::from_rgba_unmultiplied(size, image.as_raw());
            let texture = ctx.load_texture(
                format!("stickie-thumb-{}", path.display()),
                color_image,
                TextureOptions::LINEAR,
            );
            self.thumbnails.insert(path.to_path_buf(), texture);
        }
        self.thumbnails.get(path)
    }
}

fn file_label(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}
