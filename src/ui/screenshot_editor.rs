//! Self-contained screenshot crop + annotation editor.
//!
//! The editor never captures anything: the parent application captures a frame,
//! writes a PNG and hands the path to [`ScreenshotEditor::open`]. The editor
//! keeps the original file untouched and writes `<stem>-annotated.png` next to
//! it, applying the crop and every annotation in normalized image space so the
//! result is resolution independent.
//!
//! ```ignore
//! # use std::path::PathBuf;
//! # use bevy_egui::egui::Ui;
//! # use stickie::ui::screenshot_editor::{ScreenshotEditor, ScreenshotEditorEvent};
//! fn ui(editor: &mut ScreenshotEditor, ui: &mut Ui) {
//!     match editor.show(ui) {
//!         Some(ScreenshotEditorEvent::Close) => editor.close(),
//!         Some(ScreenshotEditorEvent::Saved(path)) => log::info!("saved {path:?}"),
//!         Some(ScreenshotEditorEvent::InsertIntoNote(path)) => log::info!("insert {path:?}"),
//!         None => {}
//!     }
//! }
//! ```

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Pixel-space math is bounded by image dimensions and 0..=1 normalized coordinates."
)]
// Component module: some helpers are used by the app layer and tests only; the
// full public surface is kept stable for integration.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

use anyhow::{Context, bail};
use bevy_egui::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Key, KeyboardShortcut, Layout, Modifiers, Rect,
    Response, RichText, ScrollArea, Sense, Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};
use image::{Rgba, RgbaImage};

use super::theme;

/// Drawing / selection tools.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    /// Drag to set the crop rectangle.
    #[default]
    Crop,
    /// Freehand pen stroke.
    Pen,
    /// Straight rectangle outline.
    Rectangle,
    /// Arrow from press point to release point.
    Arrow,
}

impl Tool {
    /// Short button label.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Crop => "Crop",
            Self::Pen => "Pen",
            Self::Rectangle => "Rect",
            Self::Arrow => "Arrow",
        }
    }

    /// Tooltip text.
    pub const fn tooltip(self) -> &'static str {
        match self {
            Self::Crop => "Crop — drag a region, then save or insert",
            Self::Pen => "Pen — freehand annotate",
            Self::Rectangle => "Rectangle — drag a box",
            Self::Arrow => "Arrow — drag to point at something",
        }
    }
}

/// Actions the parent app should react to after a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenshotEditorEvent {
    /// User asked to close the editor.
    Close,
    /// An annotated PNG was written to this path.
    Saved(PathBuf),
    /// User wants the image linked into the open note.
    InsertIntoNote(PathBuf),
}

/// A crop rectangle in normalized (0..=1) image coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CropRect {
    /// Top-left corner, normalized.
    pub min: [f32; 2],
    /// Bottom-right corner, normalized.
    pub max: [f32; 2],
}

impl CropRect {
    /// Normalized width.
    pub fn width(self) -> f32 {
        (self.max[0] - self.min[0]).max(0.0)
    }

    /// Normalized height.
    pub fn height(self) -> f32 {
        (self.max[1] - self.min[1]).max(0.0)
    }

    /// Crop size in source-image pixels.
    pub fn pixel_size(self, image_size: [usize; 2]) -> (u32, u32) {
        let width = (self.width() * image_size[0] as f32).round().max(1.0) as u32;
        let height = (self.height() * image_size[1] as f32).round().max(1.0) as u32;
        (width, height)
    }
}

/// One annotation, stored in normalized image coordinates so it scales with the
/// displayed image and can be rasterized at full source resolution on save.
#[derive(Clone, Debug, PartialEq)]
struct Annotation {
    kind: Tool,
    color: Color32,
    /// Stroke width normalized against the displayed image width.
    width: f32,
    /// Points in normalized image space.
    points: Vec<[f32; 2]>,
}

#[derive(Clone, Debug, Default)]
struct Snapshot {
    annotations: Vec<Annotation>,
    crop: Option<CropRect>,
}

/// Colors offered in the screenshot editor toolbar.
const SWATCHES: [Color32; 6] = [
    Color32::from_rgb(0xF5, 0xF7, 0xFA), // white
    Color32::from_rgb(0xF2, 0xB4, 0x41), // amber
    Color32::from_rgb(0xE5, 0x48, 0x4D), // red
    Color32::from_rgb(0x6F, 0xC3, 0xF2), // sky
    Color32::from_rgb(0x4C, 0xC3, 0x8A), // mint
    Color32::from_rgb(0x10, 0x12, 0x16), // near black
];

const SHORTCUT_UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);

const MAX_ZOOM: f32 = 8.0;
const MIN_ZOOM: f32 = 0.25;

/// Screenshot crop / annotate editor state plus widget.
pub struct ScreenshotEditor {
    open: bool,
    path: PathBuf,
    image_size: [usize; 2],
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    tool: Tool,
    color: Color32,
    /// Stroke width in *display* pixels (converted to normalized on use).
    stroke_width: f32,
    annotations: Vec<Annotation>,
    crop: Option<CropRect>,
    draft: Option<Annotation>,
    crop_draft: Option<CropRect>,
    drag_start: Option<[f32; 2]>,
    history: Vec<Snapshot>,
    zoom: f32,
    status: Option<String>,
    save_error: Option<String>,
}

impl Default for ScreenshotEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenshotEditor {
    /// Create a closed editor.
    pub fn new() -> Self {
        Self {
            open: false,
            path: PathBuf::new(),
            image_size: [1, 1],
            texture: None,
            error: None,
            tool: Tool::Crop,
            color: SWATCHES[0],
            stroke_width: 3.0,
            annotations: Vec::new(),
            crop: None,
            draft: None,
            crop_draft: None,
            drag_start: None,
            history: Vec::new(),
            zoom: 1.0,
            status: None,
            save_error: None,
        }
    }

    /// Load a PNG from disk and return a ready-to-show editor.
    ///
    /// This is the usual entry point: the parent captures a frame to a PNG and
    /// hands over the path. Use [`ScreenshotEditor::load`] to replace the image
    /// of an editor that already exists.
    pub fn open(ctx: &egui::Context, path: PathBuf) -> anyhow::Result<Self> {
        let mut editor = Self::new();
        editor.load(ctx, path)?;
        Ok(editor)
    }

    /// Load (or replace) the image shown by an existing editor.
    ///
    /// On failure the error is also stored so [`ScreenshotEditor::show`] can
    /// render it with a close action; the returned error lets the parent show a
    /// toast instead. A bad capture never panics the app.
    pub fn load(&mut self, ctx: &egui::Context, path: PathBuf) -> anyhow::Result<()> {
        self.reset();
        self.open = true;
        self.path = path;
        if let Err(error) = self.load_texture(ctx) {
            self.texture = None;
            self.error = Some(format!("{error:#}"));
            return Err(error);
        }
        Ok(())
    }

    /// Close the editor and release the texture and undo history.
    pub fn close(&mut self) {
        let mut reset = Self::new();
        std::mem::swap(self, &mut reset);
        self.open = false;
    }

    /// Whether an image is loaded (or an error is being displayed).
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Path of the screenshot being edited.
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Source image dimensions in pixels.
    pub fn image_size(&self) -> [usize; 2] {
        self.image_size
    }

    /// Whether the user changed the crop or added annotations.
    pub fn is_modified(&self) -> bool {
        !self.annotations.is_empty() || self.crop.is_some()
    }

    /// Number of committed annotations.
    pub fn annotation_count(&self) -> usize {
        self.annotations.len()
    }

    /// Undo the last crop/annotation change.
    pub fn undo(&mut self) {
        if let Some(snapshot) = self.history.pop() {
            self.annotations = snapshot.annotations;
            self.crop = snapshot.crop;
            self.save_error = None;
            self.status = Some("Undid last edit".to_owned());
        }
    }

    /// Remove all annotations and the crop.
    pub fn reset(&mut self) {
        self.annotations.clear();
        self.crop = None;
        self.draft = None;
        self.crop_draft = None;
        self.drag_start = None;
        self.history.clear();
        self.zoom = 1.0;
        self.status = None;
        self.save_error = None;
    }

    /// Rasterize the crop and annotations at source resolution and write
    /// `<stem>-annotated.png` next to the original.
    ///
    /// The original file is never modified.
    pub fn save_annotated(&mut self) -> anyhow::Result<PathBuf> {
        if !self.open || self.error.is_some() {
            bail!("no screenshot is loaded");
        }
        let source = image::open(&self.path)
            .with_context(|| format!("could not re-open {}", self.path.display()))?
            .to_rgba8();
        let mut canvas = source;
        for annotation in &self.annotations {
            draw_annotation(&mut canvas, annotation);
        }
        let output = match self.crop {
            Some(crop) => {
                let (x, y, width, height) = crop_pixels(crop, canvas.width(), canvas.height());
                image::imageops::crop_imm(&canvas, x, y, width, height).to_image()
            }
            None => canvas,
        };
        let output_path = annotated_path(&self.path);
        output
            .save(&output_path)
            .with_context(|| format!("could not write {}", output_path.display()))?;
        Ok(output_path)
    }

    /// Render the editor panel. Returns an event when the user acts.
    pub fn show(&mut self, ui: &mut Ui) -> Option<ScreenshotEditorEvent> {
        let mut event = None;
        theme::panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !self.open {
                let _ = theme::empty_state(
                    ui,
                    "🖼",
                    "No screenshot open",
                    "Capture a frame and it will open here for cropping and annotation.",
                    None,
                );
                return;
            }

            self.header(ui, &mut event);
            ui.add_space(theme::SPACE_SM);

            if self.error.is_some() {
                let body = self.error.clone().unwrap_or_default();
                let action =
                    theme::empty_state(ui, "⚠", "Could not open screenshot", &body, Some("Close"));
                if action.is_some_and(|response| response.clicked()) {
                    event = Some(ScreenshotEditorEvent::Close);
                }
                return;
            }

            self.handle_shortcuts(ui, &mut event);
            self.toolbar(ui);
            ui.add_space(theme::SPACE_SM);

            let footer_height = 36.0;
            let canvas_height =
                (ui.available_height() - footer_height - theme::SPACE_SM).max(160.0);
            self.canvas(ui, canvas_height);
            ui.add_space(theme::SPACE_SM);
            self.footer(ui, &mut event);
        });
        event
    }

    // -- loading ------------------------------------------------------------

    fn load_texture(&mut self, ctx: &egui::Context) -> anyhow::Result<()> {
        // A missing file is reported as an error instead of a panic.
        let dynamic = image::open(&self.path)
            .with_context(|| format!("could not open {}", self.path.display()))?;
        let width = dynamic.width() as usize;
        let height = dynamic.height() as usize;
        if width == 0 || height == 0 {
            bail!("image has zero size");
        }
        let max_side = ctx.input(|input| input.max_texture_side);
        if width > max_side || height > max_side {
            bail!("image is {width}x{height}, larger than the GPU texture limit of {max_side}");
        }
        let rgba = dynamic.to_rgba8();
        let color_image = egui::ColorImage::from_rgba_unmultiplied([width, height], rgba.as_raw());
        let name = self
            .path
            .file_name()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or("screenshot")
            .to_owned();
        self.texture = Some(ctx.load_texture(name, color_image, egui::TextureOptions::LINEAR));
        self.image_size = [width, height];
        self.error = None;
        Ok(())
    }

    // -- header / toolbar / footer -----------------------------------------

    fn header(&mut self, ui: &mut Ui, event: &mut Option<ScreenshotEditorEvent>) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("🖼")
                    .font(theme::font_at(16.0))
                    .color(theme::TEXT_MUTED),
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let name = self
                    .path
                    .file_name()
                    .and_then(std::ffi::OsStr::to_str)
                    .unwrap_or("screenshot");
                ui.add(
                    egui::Label::new(
                        RichText::new(name)
                            .font(theme::font_button())
                            .color(theme::TEXT_PRIMARY),
                    )
                    .truncate(),
                )
                .on_hover_text(self.path.display().to_string());
                ui.label(
                    RichText::new(format!(
                        "{} × {} px · original untouched",
                        self.image_size[0], self.image_size[1]
                    ))
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::icon_button(ui, "×", "Close editor").clicked() {
                    *event = Some(ScreenshotEditorEvent::Close);
                }
                if self.is_modified() {
                    theme::pill(ui, "edited", theme::ACCENT);
                }
            });
        });
    }

    fn toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            for tool in [Tool::Crop, Tool::Pen, Tool::Rectangle, Tool::Arrow] {
                if tool_icon_button(ui, tool, self.tool == tool).clicked() {
                    self.tool = tool;
                }
            }
            ui.separator();

            for (index, swatch) in SWATCHES.into_iter().enumerate() {
                if color_swatch(ui, swatch, self.color == swatch).clicked() {
                    self.color = SWATCHES[index];
                }
            }
            ui.separator();

            ui.spacing_mut().slider_width = 84.0;
            ui.add(
                egui::Slider::new(&mut self.stroke_width, 1.0..=16.0)
                    .show_value(false)
                    .text("width"),
            );

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::tool_button(ui, "+", "Zoom in", false).clicked() {
                    self.zoom = (self.zoom * 1.25).min(MAX_ZOOM);
                }
                ui.label(
                    RichText::new(format!("{:.0}%", self.zoom * 100.0))
                        .font(theme::font_small())
                        .color(theme::TEXT_MUTED),
                );
                if theme::tool_button(ui, "-", "Zoom out", false).clicked() {
                    self.zoom = (self.zoom / 1.25).max(MIN_ZOOM);
                }
                let at_fit = (self.zoom - 1.0).abs() < 0.001;
                if theme::tool_button(ui, "1:1", "Zoom to fit", at_fit).clicked() {
                    self.zoom = 1.0;
                }
                ui.separator();
                if theme::tool_button(ui, "⟲", "Reset all edits", false).clicked() {
                    self.reset();
                }
                let undo_enabled = !self.history.is_empty();
                if theme::tool_button(ui, "↶", "Undo (Ctrl+Z)", undo_enabled).clicked() {
                    self.undo();
                }
            });
        });
    }

    fn footer(&mut self, ui: &mut Ui, event: &mut Option<ScreenshotEditorEvent>) {
        ui.horizontal(|ui| {
            if let Some(crop) = self.crop {
                let (width, height) = crop.pixel_size(self.image_size);
                theme::pill(ui, &format!("crop {width}×{height}"), theme::ACCENT);
                if ui
                    .add(
                        egui::Button::new(
                            RichText::new("Clear crop")
                                .font(theme::font_small())
                                .color(theme::TEXT_MUTED),
                        )
                        .frame(false),
                    )
                    .clicked()
                {
                    self.push_history();
                    self.crop = None;
                }
            } else {
                ui.label(
                    RichText::new("Pick Crop, drag a region, then Save annotated.")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
            }
            if self.annotation_count() > 0 {
                theme::pill(
                    ui,
                    &format!("{} annotations", self.annotation_count()),
                    theme::TEXT_MUTED,
                );
            }

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary_button(ui, "Save annotated").clicked()
                    && let Some(path) = self.commit_save()
                {
                    *event = Some(ScreenshotEditorEvent::Saved(path));
                }
                let insert = ui.add(
                    egui::Button::new(RichText::new("Insert in note").font(theme::font_button()))
                        .min_size(vec2(0.0, theme::CONTROL_HEIGHT)),
                );
                if insert.clicked() {
                    let path = if self.is_modified() {
                        self.commit_save()
                    } else {
                        Some(self.path.clone())
                    };
                    if let Some(path) = path {
                        *event = Some(ScreenshotEditorEvent::InsertIntoNote(path));
                    }
                }
            });
        });
        if let Some(error) = &self.save_error {
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(error)
                    .font(theme::font_small())
                    .color(theme::DANGER),
            );
        } else if let Some(status) = &self.status {
            ui.add_space(theme::SPACE_XS);
            ui.label(
                RichText::new(status)
                    .font(theme::font_small())
                    .color(theme::SUCCESS),
            );
        }
    }

    fn commit_save(&mut self) -> Option<PathBuf> {
        match self.save_annotated() {
            Ok(path) => {
                self.save_error = None;
                self.status = Some(format!(
                    "Saved {} · original untouched",
                    path.file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .unwrap_or("annotated.png")
                ));
                Some(path)
            }
            Err(error) => {
                self.save_error = Some(format!("{error:#}"));
                self.status = None;
                None
            }
        }
    }

    fn handle_shortcuts(&mut self, ui: &mut Ui, event: &mut Option<ScreenshotEditorEvent>) {
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_UNDO)) {
            self.undo();
        }
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_SAVE))
            && let Some(path) = self.commit_save()
        {
            *event = Some(ScreenshotEditorEvent::Saved(path));
        }
    }

    // -- canvas -------------------------------------------------------------

    fn canvas(&mut self, ui: &mut Ui, height: f32) {
        let frame = egui::Frame::NONE
            .fill(theme::PANEL_BG)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(theme::radius(theme::RADIUS_MD))
            .inner_margin(theme::margin(theme::SPACE_SM));
        frame.show(ui, |ui| {
            let size = vec2(
                ui.available_width(),
                (height - 2.0 * theme::SPACE_SM).max(120.0),
            );
            let (canvas_rect, _) = ui.allocate_exact_size(size, Sense::hover());
            let image_size = self.image_size;
            let fit_scale = (canvas_rect.width() / image_size[0] as f32)
                .min(canvas_rect.height() / image_size[1] as f32)
                .max(0.0001);
            let scale = (fit_scale * self.zoom).max(0.0001);
            let shown = vec2(image_size[0] as f32 * scale, image_size[1] as f32 * scale);

            if self.zoom <= 1.0 + f32::EPSILON {
                // Fit (or zoomed out): center in the canvas and paint directly.
                let image_rect = Rect::from_center_size(canvas_rect.center(), shown);
                self.paint_canvas(ui, image_rect);
            } else {
                // Zoomed in: allow scrolling the oversized image.
                ui.scope_builder(UiBuilder::new().max_rect(canvas_rect), |ui| {
                    ScrollArea::both()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let padding = vec2(theme::SPACE_MD, theme::SPACE_MD);
                            let (content, _) =
                                ui.allocate_exact_size(shown + padding * 2.0, Sense::hover());
                            let image_rect = Rect::from_center_size(content.center(), shown);
                            self.paint_canvas(ui, image_rect);
                        });
                });
            }
        });
    }

    fn paint_canvas(&mut self, ui: &mut Ui, image_rect: Rect) {
        let painter = ui.painter().clone();
        painter.rect_filled(
            image_rect.expand(1.0),
            CornerRadius::ZERO,
            Color32::from_black_alpha(90),
        );
        if let Some(texture) = &self.texture {
            painter.image(
                texture.id(),
                image_rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        // Committed annotations.
        for annotation in &self.annotations {
            paint_annotation(&painter, annotation, image_rect);
        }
        // In-progress annotation.
        if let Some(draft) = &self.draft {
            paint_annotation(&painter, draft, image_rect);
        }
        // Crop preview / committed crop overlay.
        if let Some(draft) = self.crop_draft {
            paint_crop_overlay(&painter, draft, image_rect, false);
        } else if let Some(crop) = self.crop {
            paint_crop_overlay(&painter, crop, image_rect, true);
        }

        let response = ui.interact(
            image_rect,
            ui.id().with("screenshot-canvas"),
            Sense::click_and_drag(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }

        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.begin_drag(normalize(pointer, image_rect), image_rect);
            }
        } else if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.update_drag(normalize(pointer, image_rect));
            }
        } else if response.drag_stopped() {
            self.finish_drag();
        }
    }

    // -- drag handling ------------------------------------------------------

    fn begin_drag(&mut self, point: [f32; 2], image_rect: Rect) {
        self.drag_start = Some(point);
        match self.tool {
            Tool::Crop => {
                self.crop_draft = None;
                self.draft = None;
            }
            Tool::Pen => {
                self.draft = Some(Annotation {
                    kind: Tool::Pen,
                    color: self.color,
                    width: self.normalized_width(image_rect),
                    points: vec![point],
                });
            }
            Tool::Rectangle | Tool::Arrow => {
                self.draft = Some(Annotation {
                    kind: self.tool,
                    color: self.color,
                    width: self.normalized_width(image_rect),
                    points: vec![point, point],
                });
            }
        }
    }

    /// Stroke width normalized against the displayed image width, so what the
    /// user sees on screen is what gets rasterized at save time.
    fn normalized_width(&self, image_rect: Rect) -> f32 {
        if image_rect.width() <= 0.0 {
            return 0.0;
        }
        (self.stroke_width / image_rect.width()).clamp(0.0, 0.25)
    }

    fn update_drag(&mut self, point: [f32; 2]) {
        match self.tool {
            Tool::Crop => {
                if let Some(start) = self.drag_start {
                    self.crop_draft = Some(CropRect {
                        min: [start[0].min(point[0]), start[1].min(point[1])],
                        max: [start[0].max(point[0]), start[1].max(point[1])],
                    });
                }
            }
            Tool::Pen => {
                if let Some(draft) = &mut self.draft {
                    let should_push = draft.points.last().is_none_or(|last| {
                        let dx = point[0] - last[0];
                        let dy = point[1] - last[1];
                        dx * dx + dy * dy > 0.000_004
                    });
                    if should_push {
                        draft.points.push(point);
                    }
                }
            }
            Tool::Rectangle | Tool::Arrow => {
                if let Some(draft) = &mut self.draft
                    && let Some(end) = draft.points.get_mut(1)
                {
                    *end = point;
                }
            }
        }
    }

    fn finish_drag(&mut self) {
        match self.tool {
            Tool::Crop => {
                if let Some(crop) = self.crop_draft.take()
                    && crop.width() > 0.01
                    && crop.height() > 0.01
                {
                    self.push_history();
                    self.crop = Some(crop);
                    self.status = Some(format!(
                        "Crop {}×{} px",
                        crop.pixel_size(self.image_size).0,
                        crop.pixel_size(self.image_size).1
                    ));
                }
            }
            Tool::Pen => {
                if let Some(annotation) = self.draft.take() {
                    self.push_history();
                    self.annotations.push(annotation);
                }
            }
            Tool::Rectangle | Tool::Arrow => {
                if let Some(annotation) = self.draft.take() {
                    let long_enough = annotation.points.len() == 2
                        && distance(annotation.points[0], annotation.points[1]) > 0.005;
                    if long_enough {
                        self.push_history();
                        self.annotations.push(annotation);
                    }
                }
            }
        }
        self.drag_start = None;
    }

    fn push_history(&mut self) {
        self.history.push(Snapshot {
            annotations: self.annotations.clone(),
            crop: self.crop,
        });
        if self.history.len() > 64 {
            self.history.remove(0);
        }
    }
}

// ---------------------------------------------------------------------------
// Painting helpers
// ---------------------------------------------------------------------------

fn paint_tool_icon(painter: &egui::Painter, rect: Rect, tool: Tool, color: Color32) {
    let stroke = Stroke::new(1.6, color);
    let center = rect.center();
    match tool {
        Tool::Crop => {
            let half = 7.0;
            for (sx, sy) in [(-1.0_f32, -1.0_f32), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
                let corner = center + vec2(sx * half, sy * half);
                painter.line_segment([corner, corner + vec2(-sx * 4.0, 0.0)], stroke);
                painter.line_segment([corner, corner + vec2(0.0, -sy * 4.0)], stroke);
            }
        }
        Tool::Pen => {
            painter.line_segment([center + vec2(-5.0, 6.0), center + vec2(4.0, -5.0)], stroke);
            painter.line_segment([center + vec2(-6.0, 7.0), center + vec2(-3.5, 4.5)], stroke);
            painter.circle_filled(center + vec2(5.0, -6.0), 1.4, color);
        }
        Tool::Rectangle => {
            painter.rect_stroke(
                Rect::from_center_size(center, vec2(14.0, 10.0)),
                theme::radius(2.0),
                stroke,
                StrokeKind::Middle,
            );
        }
        Tool::Arrow => {
            painter.line_segment([center + vec2(-6.0, 6.0), center + vec2(5.0, -5.0)], stroke);
            painter.line_segment([center + vec2(5.0, -5.0), center + vec2(1.5, -5.0)], stroke);
            painter.line_segment([center + vec2(5.0, -5.0), center + vec2(5.0, -1.5)], stroke);
        }
    }
}

fn tool_icon_button(ui: &mut Ui, tool: Tool, selected: bool) -> Response {
    let (rect, response) =
        ui.allocate_exact_size(Vec2::splat(theme::TOOL_BUTTON_SIZE), Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(
            rect,
            theme::radius(theme::RADIUS_SM),
            theme::tint(theme::ACCENT, 34),
        );
        painter.rect_stroke(
            rect,
            theme::radius(theme::RADIUS_SM),
            Stroke::new(1.0, theme::tint(theme::ACCENT, 170)),
            StrokeKind::Inside,
        );
    } else if response.hovered() {
        painter.rect_filled(rect, theme::radius(theme::RADIUS_SM), theme::hover_fill());
    }
    let color = if selected {
        theme::ACCENT_STRONG
    } else if response.hovered() {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_MUTED
    };
    paint_tool_icon(&painter, rect, tool, color);
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::WidgetType::Button, true, selected, tool.label())
    });
    response.on_hover_text(tool.tooltip())
}

fn color_swatch(ui: &mut Ui, color: Color32, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::click());
    let painter = ui.painter().clone();
    let center = rect.center();
    painter.circle_filled(center, 8.0, color);
    if selected {
        painter.circle_stroke(center, 10.0, Stroke::new(2.0, theme::ACCENT));
    } else if response.hovered() {
        painter.circle_stroke(center, 10.0, Stroke::new(1.0, theme::TEXT_MUTED));
    }
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response
}

fn paint_crop_overlay(painter: &egui::Painter, crop: CropRect, image_rect: Rect, committed: bool) {
    let rect = crop_to_screen(crop, image_rect);
    let dim = Color32::from_black_alpha(if committed { 150 } else { 120 });
    painter.rect_filled(
        Rect::from_min_max(image_rect.min, pos2(image_rect.max.x, rect.min.y)),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(pos2(image_rect.min.x, rect.max.y), image_rect.max),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(
            pos2(image_rect.min.x, rect.min.y),
            pos2(rect.min.x, rect.max.y),
        ),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_filled(
        Rect::from_min_max(
            pos2(rect.max.x, rect.min.y),
            pos2(image_rect.max.x, rect.max.y),
        ),
        CornerRadius::ZERO,
        dim,
    );
    painter.rect_stroke(
        rect,
        CornerRadius::ZERO,
        Stroke::new(1.5, theme::ACCENT),
        StrokeKind::Inside,
    );
    if committed {
        // Corner + edge midpoint handles, like a crop selection in a design tool.
        let handle_points = [
            rect.left_top(),
            rect.center_top(),
            rect.right_top(),
            rect.right_center(),
            rect.right_bottom(),
            rect.center_bottom(),
            rect.left_bottom(),
            rect.left_center(),
        ];
        for point in handle_points {
            let handle = Rect::from_center_size(point, Vec2::splat(7.0));
            painter.rect_filled(handle, theme::radius(1.5), Color32::WHITE);
            painter.rect_stroke(
                handle,
                theme::radius(1.5),
                Stroke::new(1.0, theme::ACCENT),
                StrokeKind::Inside,
            );
        }
    }
}

fn paint_annotation(painter: &egui::Painter, annotation: &Annotation, image_rect: Rect) {
    let points: Vec<egui::Pos2> = annotation
        .points
        .iter()
        .map(|point| denormalize(*point, image_rect))
        .collect();
    let width = (annotation.width * image_rect.width()).max(0.75);
    let stroke = Stroke::new(width, annotation.color);
    match annotation.kind {
        Tool::Pen => {
            if points.len() == 1 {
                painter.circle_filled(points[0], width * 0.5, annotation.color);
            } else {
                painter.line(points, stroke);
            }
        }
        Tool::Rectangle => {
            if points.len() >= 2 {
                let rect = Rect::from_two_pos(points[0], points[1]);
                painter.rect_stroke(rect, CornerRadius::ZERO, stroke, StrokeKind::Middle);
            }
        }
        Tool::Arrow => {
            if points.len() >= 2 {
                let vector = points[1] - points[0];
                if vector.length() > 0.5 {
                    painter.arrow(points[0], vector, stroke);
                }
            }
        }
        Tool::Crop => {}
    }
}

fn crop_to_screen(crop: CropRect, image_rect: Rect) -> Rect {
    Rect::from_min_max(
        denormalize(crop.min, image_rect),
        denormalize(crop.max, image_rect),
    )
}

fn normalize(position: egui::Pos2, image_rect: Rect) -> [f32; 2] {
    let width = image_rect.width().max(0.0001);
    let height = image_rect.height().max(0.0001);
    [
        ((position.x - image_rect.min.x) / width).clamp(0.0, 1.0),
        ((position.y - image_rect.min.y) / height).clamp(0.0, 1.0),
    ]
}

fn denormalize(point: [f32; 2], image_rect: Rect) -> egui::Pos2 {
    pos2(
        image_rect.min.x + point[0] * image_rect.width(),
        image_rect.min.y + point[1] * image_rect.height(),
    )
}

fn distance(a: [f32; 2], b: [f32; 2]) -> f32 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    (dx * dx + dy * dy).sqrt()
}

// ---------------------------------------------------------------------------
// Rasterization (save path)
// ---------------------------------------------------------------------------

/// Output path for an annotated screenshot: `<stem>-annotated.png` next to the
/// original. The original file is never touched.
pub fn annotated_path(path: &Path) -> PathBuf {
    let stem = path
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .unwrap_or("screenshot");
    path.with_file_name(format!("{stem}-annotated.png"))
}

/// Crop rectangle to integer source-image pixel bounds, clamped to the image.
fn crop_pixels(crop: CropRect, width: u32, height: u32) -> (u32, u32, u32, u32) {
    if width == 0 || height == 0 {
        return (0, 0, 1, 1);
    }
    let image_width = width as f32;
    let image_height = height as f32;
    let x0 = (crop.min[0] * image_width).round().clamp(0.0, image_width) as u32;
    let y0 = (crop.min[1] * image_height)
        .round()
        .clamp(0.0, image_height) as u32;
    let x1 = (crop.max[0] * image_width).round().clamp(0.0, image_width) as u32;
    let y1 = (crop.max[1] * image_height)
        .round()
        .clamp(0.0, image_height) as u32;
    let x0 = x0.min(width - 1);
    let y0 = y0.min(height - 1);
    let x1 = x1.max(x0 + 1).min(width);
    let y1 = y1.max(y0 + 1).min(height);
    (x0, y0, x1 - x0, y1 - y0)
}

fn draw_annotation(image: &mut RgbaImage, annotation: &Annotation) {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return;
    }
    let color = annotation.color.to_srgba_unmultiplied();
    let thickness = (annotation.width * width as f32).max(1.0);
    let points: Vec<(f32, f32)> = annotation
        .points
        .iter()
        .map(|point| (point[0] * width as f32, point[1] * height as f32))
        .collect();
    match annotation.kind {
        Tool::Pen => {
            if points.len() == 1 {
                draw_segment(image, points[0], points[0], thickness, color);
            }
            for pair in points.windows(2) {
                draw_segment(image, pair[0], pair[1], thickness, color);
            }
        }
        Tool::Rectangle => {
            if points.len() >= 2 {
                let (first, second) = (points[0], points[1]);
                let corners = [
                    (first.0, first.1),
                    (second.0, first.1),
                    (second.0, second.1),
                    (first.0, second.1),
                ];
                let mut previous = corners[corners.len() - 1];
                for corner in corners {
                    draw_segment(image, previous, corner, thickness, color);
                    previous = corner;
                }
            }
        }
        Tool::Arrow => {
            if points.len() >= 2 {
                draw_segment(image, points[0], points[1], thickness, color);
                draw_arrow_head(image, points[0], points[1], thickness, color);
            }
        }
        Tool::Crop => {}
    }
}

fn draw_arrow_head(
    image: &mut RgbaImage,
    start: (f32, f32),
    tip: (f32, f32),
    thickness: f32,
    color: [u8; 4],
) {
    let dx = tip.0 - start.0;
    let dy = tip.1 - start.1;
    let length = (dx * dx + dy * dy).sqrt();
    if length < 0.001 {
        return;
    }
    let direction = (dx / length, dy / length);
    let head = (thickness * 3.0).max(length * 0.18).min(length * 0.6);
    for sign in [1.0_f32, -1.0] {
        let (sin, cos) = (0.5 * sign).sin_cos();
        let wing = (
            tip.0 - (direction.0 * cos - direction.1 * sin) * head,
            tip.1 - (direction.0 * sin + direction.1 * cos) * head,
        );
        draw_segment(image, tip, wing, thickness, color);
    }
}

fn draw_segment(
    image: &mut RgbaImage,
    start: (f32, f32),
    end: (f32, f32),
    thickness: f32,
    color: [u8; 4],
) {
    let (width, height) = image.dimensions();
    let radius = (thickness * 0.5).max(0.5);
    let min_x = (start.0.min(end.0) - radius - 1.0).floor().max(0.0) as u32;
    let min_y = (start.1.min(end.1) - radius - 1.0).floor().max(0.0) as u32;
    let max_x = ((start.0.max(end.0) + radius + 1.0).ceil() as u32).min(width - 1);
    let max_y = ((start.1.max(end.1) + radius + 1.0).ceil() as u32).min(height - 1);
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let dist = distance_to_segment(x as f32, y as f32, start, end);
            let coverage = (radius + 0.5 - dist).clamp(0.0, 1.0);
            if coverage > 0.0 {
                blend_pixel(image, x, y, color, coverage);
            }
        }
    }
}

fn distance_to_segment(px: f32, py: f32, start: (f32, f32), end: (f32, f32)) -> f32 {
    let abx = end.0 - start.0;
    let aby = end.1 - start.1;
    let apx = px - start.0;
    let apy = py - start.1;
    let denominator = abx * abx + aby * aby;
    let t = if denominator > 0.0 {
        ((apx * abx + apy * aby) / denominator).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let dx = px - (start.0 + abx * t);
    let dy = py - (start.1 + aby * t);
    (dx * dx + dy * dy).sqrt()
}

fn blend_pixel(image: &mut RgbaImage, x: u32, y: u32, color: [u8; 4], coverage: f32) {
    let alpha = (f32::from(color[3]) / 255.0) * coverage;
    if alpha <= 0.0 {
        return;
    }
    let pixel: &mut Rgba<u8> = image.get_pixel_mut(x, y);
    for channel in 0..3 {
        pixel[channel] = (f32::from(color[channel]) * alpha
            + f32::from(pixel[channel]) * (1.0 - alpha))
            .round() as u8;
    }
    pixel[3] = 255;
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotated_path_keeps_original_name() {
        assert_eq!(
            annotated_path(Path::new("/tmp/capture.png")),
            PathBuf::from("/tmp/capture-annotated.png")
        );
        assert_eq!(
            annotated_path(Path::new("boss fight.jpg")),
            PathBuf::from("boss fight-annotated.png")
        );
    }

    #[test]
    fn crop_pixels_clamps_to_bounds() {
        let crop = CropRect {
            min: [-0.5, 0.25],
            max: [1.5, 0.75],
        };
        assert_eq!(crop_pixels(crop, 100, 200), (0, 50, 100, 100));

        let tiny = CropRect {
            min: [0.5, 0.5],
            max: [0.5, 0.5],
        };
        assert_eq!(crop_pixels(tiny, 100, 100), (50, 50, 1, 1));
    }

    #[test]
    fn pixel_size_reports_crop_dimensions() {
        let crop = CropRect {
            min: [0.0, 0.0],
            max: [0.5, 0.25],
        };
        assert_eq!(crop.pixel_size([1920, 1080]), (960, 270));
    }

    #[test]
    fn distance_to_segment_is_zero_on_line() {
        assert!(distance_to_segment(5.0, 0.0, (0.0, 0.0), (10.0, 0.0)) < 0.001);
        assert!((distance_to_segment(5.0, 2.0, (0.0, 0.0), (10.0, 0.0)) - 2.0).abs() < 0.001);
    }

    #[test]
    fn drawing_a_stroke_changes_only_nearby_pixels() {
        let mut image = RgbaImage::from_pixel(32, 32, Rgba([0, 0, 0, 255]));
        let annotation = Annotation {
            kind: Tool::Pen,
            color: Color32::from_rgb(255, 0, 0),
            width: 4.0 / 32.0,
            points: vec![[0.15, 0.5], [0.85, 0.5]],
        };
        draw_annotation(&mut image, &annotation);
        let center = image.get_pixel(16, 16);
        assert!(center[0] > 200, "center pixel should be painted red");
        assert_eq!(center[1], 0);
        assert_eq!(image.get_pixel(0, 0)[0], 0);
    }

    #[test]
    fn rectangle_rasterizes_an_outline() {
        let mut image = RgbaImage::from_pixel(40, 40, Rgba([0, 0, 0, 255]));
        let annotation = Annotation {
            kind: Tool::Rectangle,
            color: Color32::from_rgb(0, 255, 0),
            width: 3.0 / 40.0,
            points: vec![[0.25, 0.25], [0.75, 0.75]],
        };
        draw_annotation(&mut image, &annotation);
        assert!(image.get_pixel(10, 20)[1] > 150, "left edge painted");
        assert!(image.get_pixel(20, 10)[1] > 150, "top edge painted");
        assert_eq!(image.get_pixel(20, 20)[1], 0, "inside is untouched");
    }

    #[test]
    fn tool_labels_are_stable() {
        assert_eq!(Tool::Crop.label(), "Crop");
        assert_eq!(Tool::Pen.label(), "Pen");
        assert_eq!(Tool::Rectangle.label(), "Rect");
        assert_eq!(Tool::Arrow.label(), "Arrow");
    }

    #[test]
    fn closed_editor_reports_no_state() {
        let mut editor = ScreenshotEditor::new();
        assert!(!editor.is_open());
        assert!(!editor.is_modified());
        assert!(editor.save_annotated().is_err());
        editor.close();
        assert!(!editor.is_open());
    }

    #[test]
    fn opening_a_missing_file_returns_an_error() {
        let ctx = egui::Context::default();
        let result = ScreenshotEditor::open(&ctx, PathBuf::from("definitely-not-a-real.png"));
        assert!(result.is_err());
    }
}
