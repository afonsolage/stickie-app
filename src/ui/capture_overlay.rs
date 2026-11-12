//! Lightshot-style full-screen capture overlay.
//!
//! The overlay owns no capture logic and no file IO. The app captures the frozen
//! frame into an [`image::RgbaImage`], hands it to [`CaptureOverlay::begin`], and
//! then draws [`CaptureOverlay::show`] in the root viewport `Ui` every frame
//! while the overlay is active:
//!
//! 1. **Select**: the frozen frame is letterboxed full-window under a dark
//!    scrim. The user drags a region (Esc cancels).
//! 2. **Annotate**: a compact floating toolbar offers pen/rectangle/arrow,
//!    colors, stroke width, undo, reselect, Save, Insert in note and Cancel.
//!    Drags are clamped to the selected region.
//!
//! Save/Insert return the final **cropped + annotated full-resolution** image;
//! the parent stores it or links it into the current note. The preview texture
//! is downscaled when the source exceeds egui's `max_texture_side`; the full
//! resolution source is always kept for rendering the output.
//!
//! ```ignore
//! # use bevy_egui::egui::Ui;
//! # use image::RgbaImage;
//! # use stickie::ui::capture_overlay::{CaptureOutcome, CaptureOverlay};
//! fn frame(overlay: &mut CaptureOverlay, ui: &mut Ui) {
//!     if let Some(outcome) = overlay.show(ui) {
//!         match outcome {
//!             CaptureOutcome::Save(image) => { /* store image */ }
//!             CaptureOutcome::Insert(image) => { /* attach image to note */ }
//!             CaptureOutcome::Cancel => {}
//!         }
//!         overlay.finish();
//!     }
//! }
//! ```

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "Pixel-space math is bounded by image dimensions and 0..=1 normalized coordinates."
)]

use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, Id, Key, KeyboardShortcut, Modifiers, Order,
    Rect, RichText, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2,
};
use image::RgbaImage;

use super::screenshot_editor::{
    Annotation, CropRect, SWATCHES, Tool, color_swatch, crop_pixels, crop_to_screen, distance,
    draw_annotation, fitted_texture_size, normalize, paint_annotation, paint_crop_overlay,
    tool_icon_button,
};
use super::theme;

/// Minimum committed selection size, in screen pixels.
pub(crate) const MIN_SELECTION_SCREEN_PX: f32 = 6.0;

/// Pen/rectangle/arrow minimum drag length, in normalized image units.
const MIN_ANNOTATION_DRAG: f32 = 0.004;

const SHORTCUT_UNDO: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::Z);
const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);

const TOOLBAR_ID: &str = "stickie-capture-toolbar";
const INSTRUCTION_ID: &str = "stickie-capture-instruction";
const ERROR_ID: &str = "stickie-capture-error";
const TEXTURE_NAME: &str = "stickie-capture-overlay";

/// What the parent app should do with the finished capture.
#[derive(Debug)]
pub enum CaptureOutcome {
    /// Persist the image (cropped + annotated) as a screenshot.
    Save(RgbaImage),
    /// Insert the image (cropped + annotated) into the open note.
    Insert(RgbaImage),
    /// Discard the capture.
    Cancel,
}

/// Which interaction stage the overlay is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Waiting for the user to drag a region.
    Selecting,
    /// Region selected: annotate / save / insert.
    Annotating,
}

/// What a toolbar click or shortcut asked the overlay to render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CaptureCommand {
    Save,
    Insert,
}

/// Full-screen frozen-frame capture overlay state and widget.
pub struct CaptureOverlay {
    active: bool,
    image: Option<RgbaImage>,
    image_size: [usize; 2],
    texture: Option<egui::TextureHandle>,
    error: Option<String>,
    stage: Stage,
    selection: Option<CropRect>,
    selection_draft: Option<CropRect>,
    selection_start: Option<[f32; 2]>,
    tool: Tool,
    color: Color32,
    /// Stroke width in *screen* pixels; converted to normalized on drag start.
    stroke_width: f32,
    annotations: Vec<Annotation>,
    draft: Option<Annotation>,
    history: Vec<Vec<Annotation>>,
}

impl Default for CaptureOverlay {
    fn default() -> Self {
        Self::new()
    }
}

impl CaptureOverlay {
    /// Create an inactive overlay.
    pub fn new() -> Self {
        Self {
            active: false,
            image: None,
            image_size: [0, 0],
            texture: None,
            error: None,
            stage: Stage::Selecting,
            selection: None,
            selection_draft: None,
            selection_start: None,
            tool: Tool::Pen,
            color: SWATCHES[0],
            stroke_width: 3.0,
            annotations: Vec::new(),
            draft: None,
            history: Vec::new(),
        }
    }

    /// Start capture mode for a frozen frame.
    ///
    /// The image is kept at full resolution; only a downscaled preview is
    /// uploaded to egui on the next [`CaptureOverlay::show`].
    pub fn begin(&mut self, image: RgbaImage) {
        *self = Self::new();
        self.active = true;
        self.image_size = [image.width() as usize, image.height() as usize];
        if image.width() == 0 || image.height() == 0 {
            self.error = Some("The captured image is empty. Nothing to select.".to_owned());
        }
        self.image = Some(image);
    }

    /// Whether capture mode is showing (or showing an error to dismiss).
    pub fn is_active(&self) -> bool {
        self.active
    }

    /// Draw the capture overlay and return the user's action, if any.
    ///
    /// The parent should call [`CaptureOverlay::finish`] after handling the
    /// returned outcome or showing an error.
    pub fn show(&mut self, ui: &mut Ui) -> Option<CaptureOutcome> {
        if !self.active {
            return None;
        }
        let ctx = ui.ctx().clone();
        let screen = ctx.viewport_rect();
        ui.painter()
            .rect_filled(screen, CornerRadius::ZERO, Color32::BLACK);

        if self.texture.is_none()
            && self.image.is_some()
            && self.error.is_none()
            && let Err(error) = self.ensure_texture(&ctx)
        {
            self.error = Some(error);
        }

        let image_rect = fitted_image_rect(self.image_size, screen);
        if let (Some(texture), Some(rect)) = (&self.texture, image_rect) {
            ui.painter().image(
                texture.id(),
                rect,
                Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
                Color32::WHITE,
            );
        }

        let mut outcome = self.handle_shortcuts(ui);
        if self.error.is_some() {
            if let Some(cancel) = self.error_card(ui) {
                outcome = Some(cancel);
            }
            return outcome;
        }

        if let Some(image_rect) = image_rect {
            match self.stage {
                Stage::Selecting => self.show_selecting(ui, image_rect),
                Stage::Annotating => self.show_annotating(ui, image_rect, &mut outcome),
            }
        }
        outcome
    }

    /// Reset to the inactive state. Safe to call repeatedly.
    pub fn finish(&mut self) {
        *self = Self::new();
    }

    /// Show an inline error (capture failure, unusable image, ...).
    ///
    /// Also activates the overlay so the parent can display the error frame.
    pub fn set_error(&mut self, message: impl Into<String>) {
        self.error = Some(message.into());
        self.active = true;
    }

    // -- texture -----------------------------------------------------------

    fn ensure_texture(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let Some(image) = self.image.as_ref() else {
            return Err("No captured frame is available.".to_owned());
        };
        let [width, height] = self.image_size;
        if width == 0 || height == 0 {
            return Err("The captured image is empty. Nothing to select.".to_owned());
        }
        // 2560-wide and 4K frames would exceed egui's texture budget; the
        // preview is downscaled, the full-resolution source is kept for output.
        let max_side = ctx.input(|input| input.max_texture_side).max(1);
        let [texture_width, texture_height] = fitted_texture_size([width, height], max_side);
        let preview = if texture_width == width && texture_height == height {
            image.clone()
        } else {
            image::imageops::resize(
                image,
                texture_width as u32,
                texture_height as u32,
                image::imageops::FilterType::Triangle,
            )
        };
        let color_image = egui::ColorImage::from_rgba_unmultiplied(
            [texture_width, texture_height],
            preview.as_raw(),
        );
        self.texture =
            Some(ctx.load_texture(TEXTURE_NAME, color_image, egui::TextureOptions::LINEAR));
        Ok(())
    }

    // -- keyboard ----------------------------------------------------------

    fn handle_shortcuts(&mut self, ui: &mut Ui) -> Option<CaptureOutcome> {
        if ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape)) {
            return Some(CaptureOutcome::Cancel);
        }
        if self.error.is_some() {
            return None;
        }
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_UNDO)) {
            self.undo();
        }
        self.selection?;
        let save = ui.input_mut(|input| {
            input.consume_key(Modifiers::NONE, Key::Enter) || input.consume_shortcut(&SHORTCUT_SAVE)
        });
        if save {
            return self.render_outcome(CaptureCommand::Save);
        }
        None
    }

    // -- select stage ------------------------------------------------------

    fn show_selecting(&mut self, ui: &mut Ui, image_rect: Rect) {
        let painter = ui.painter().clone();
        painter.rect_filled(image_rect, CornerRadius::ZERO, theme::SCRIM);

        let response = ui.interact(
            image_rect,
            ui.id().with("stickie-capture-select"),
            Sense::click_and_drag(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
            if let Some(pointer) = response.hover_pos() {
                let guide = Stroke::new(1.0, Color32::from_white_alpha(45));
                painter.hline(image_rect.x_range(), pointer.y, guide);
                painter.vline(pointer.x, image_rect.y_range(), guide);
            }
        }
        if let Some(draft) = self.selection_draft {
            paint_crop_overlay(&painter, draft, image_rect, false);
        }

        self.instruction_bar(ui.ctx(), "Drag to select an area", "Esc to cancel");

        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.selection_start = Some(normalize(pointer, image_rect));
                self.selection_draft = None;
            }
        } else if response.dragged() {
            if let (Some(pointer), Some(start)) =
                (response.interact_pointer_pos(), self.selection_start)
            {
                self.selection_draft =
                    Some(rect_from_points(start, normalize(pointer, image_rect)));
            }
        } else if response.drag_stopped() {
            let committed = self.selection_draft.take();
            if let Some(selection) = committed
                && screen_selection_valid(selection, image_rect, MIN_SELECTION_SCREEN_PX)
            {
                self.selection = Some(selection);
                self.stage = Stage::Annotating;
                self.tool = Tool::Pen;
                self.annotations.clear();
                self.draft = None;
                self.history.clear();
            }
            self.selection_start = None;
        }
    }

    // -- annotate stage ----------------------------------------------------

    fn show_annotating(
        &mut self,
        ui: &mut Ui,
        image_rect: Rect,
        outcome: &mut Option<CaptureOutcome>,
    ) {
        let Some(selection) = self.selection else {
            self.stage = Stage::Selecting;
            return;
        };
        let painter = ui.painter().clone();
        paint_crop_overlay(&painter, selection, image_rect, false);
        for annotation in &self.annotations {
            paint_annotation(&painter, annotation, image_rect);
        }
        if let Some(draft) = &self.draft {
            paint_annotation(&painter, draft, image_rect);
        }
        paint_selection_size(
            &painter,
            selection,
            image_rect,
            self.image_size,
            ui.ctx().viewport_rect(),
        );

        let selection_rect = crop_to_screen(selection, image_rect);
        let response = ui.interact(
            selection_rect,
            ui.id().with("stickie-capture-annotate"),
            Sense::click_and_drag(),
        );
        if response.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Crosshair);
        }
        if response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.begin_annotation(normalize(pointer, image_rect), selection, image_rect);
            }
        } else if response.dragged() {
            if let Some(pointer) = response.interact_pointer_pos() {
                self.update_annotation(normalize(pointer, image_rect), selection);
            }
        } else if response.drag_stopped() {
            self.finish_annotation();
        }

        self.show_toolbar(ui, selection, outcome);
    }

    fn begin_annotation(&mut self, point: [f32; 2], selection: CropRect, image_rect: Rect) {
        let point = clamp_to_crop(point, selection);
        let width = if image_rect.width() > 0.0 {
            (self.stroke_width / image_rect.width()).clamp(0.0, 0.25)
        } else {
            0.0
        };
        let points = if self.tool == Tool::Pen {
            vec![point]
        } else {
            vec![point, point]
        };
        self.draft = Some(Annotation {
            kind: self.tool,
            color: self.color,
            width,
            points,
        });
    }

    fn update_annotation(&mut self, point: [f32; 2], selection: CropRect) {
        let point = clamp_to_crop(point, selection);
        match self.tool {
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
            Tool::Crop => {}
        }
    }

    fn finish_annotation(&mut self) {
        if let Some(annotation) = self.draft.take() {
            let keep = match annotation.kind {
                Tool::Pen => true,
                _ => {
                    annotation.points.len() == 2
                        && distance(annotation.points[0], annotation.points[1])
                            >= MIN_ANNOTATION_DRAG
                }
            };
            if keep {
                self.push_history();
                self.annotations.push(annotation);
            }
        }
    }

    fn undo(&mut self) {
        if let Some(snapshot) = self.history.pop() {
            self.annotations = snapshot;
        }
    }

    fn reselect(&mut self) {
        self.stage = Stage::Selecting;
        self.selection = None;
        self.selection_draft = None;
        self.selection_start = None;
        self.annotations.clear();
        self.draft = None;
        self.history.clear();
    }

    fn push_history(&mut self) {
        self.history.push(self.annotations.clone());
        if self.history.len() > 64 {
            self.history.remove(0);
        }
    }

    fn render_outcome(&mut self, command: CaptureCommand) -> Option<CaptureOutcome> {
        let Some(image) = self.image.as_ref() else {
            self.error = Some("No captured frame is available to export.".to_owned());
            return None;
        };
        let Some(selection) = self.selection else {
            self.error = Some("Select an area before saving.".to_owned());
            return None;
        };
        match render_capture(image, selection, &self.annotations) {
            Ok(output) => Some(match command {
                CaptureCommand::Save => CaptureOutcome::Save(output),
                CaptureCommand::Insert => CaptureOutcome::Insert(output),
            }),
            Err(error) => {
                self.error = Some(format!("Could not build the capture: {error}"));
                None
            }
        }
    }

    // -- floating UI -------------------------------------------------------

    fn instruction_bar(&self, ctx: &egui::Context, instruction: &str, shortcut: &str) {
        egui::Area::new(Id::new(INSTRUCTION_ID))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, theme::SPACE_XL))
            .interactable(false)
            .show(ctx, |ui| {
                capture_frame().show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new("⌖")
                                .font(theme::font_mono(13.0))
                                .color(theme::ACCENT),
                        );
                        ui.label(
                            RichText::new(instruction)
                                .font(theme::font_body())
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(theme::SPACE_XS);
                        theme::kbd(ui, shortcut);
                    });
                });
            });
    }

    fn show_toolbar(&mut self, ui: &Ui, selection: CropRect, outcome: &mut Option<CaptureOutcome>) {
        let ctx = ui.ctx().clone();
        egui::Area::new(Id::new(TOOLBAR_ID))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_TOP, vec2(0.0, theme::SPACE_LG))
            .show(&ctx, |ui| {
                capture_frame().show(ui, |ui| {
                    self.toolbar_controls(ui, selection, outcome);
                });
            });
    }

    fn toolbar_controls(
        &mut self,
        ui: &mut Ui,
        selection: CropRect,
        outcome: &mut Option<CaptureOutcome>,
    ) {
        ui.horizontal(|ui| {
            for tool in [Tool::Pen, Tool::Rectangle, Tool::Arrow] {
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
            ui.spacing_mut().slider_width = 78.0;
            ui.add(
                egui::Slider::new(&mut self.stroke_width, 1.0..=16.0)
                    .show_value(false)
                    .text("width"),
            );
            ui.separator();
            if theme::tool_button(ui, "↶", "Undo (Ctrl+Z)", !self.history.is_empty()).clicked() {
                self.undo();
            }
            if theme::tool_button(ui, "⛶", "Reselect area", false).clicked() {
                self.reselect();
            }
            ui.separator();
            if theme::tool_button(ui, "×", "Cancel (Esc)", false).clicked() {
                *outcome = Some(CaptureOutcome::Cancel);
            }
            if ui
                .add(
                    egui::Button::new(RichText::new("Insert in note").font(theme::font_button()))
                        .min_size(vec2(0.0, theme::CONTROL_HEIGHT)),
                )
                .clicked()
            {
                *outcome = self.render_outcome(CaptureCommand::Insert);
            }
            if theme::primary_button(ui, "Save").clicked() {
                *outcome = self.render_outcome(CaptureCommand::Save);
            }
        });

        ui.add_space(theme::SPACE_XS);
        ui.horizontal(|ui| {
            let (width, height) = selection.pixel_size(self.image_size);
            theme::pill(ui, &format!("{width} × {height} px"), theme::ACCENT);
            ui.label(
                RichText::new(
                    "Drag inside the selection to annotate · Enter or Ctrl+S saves · Esc cancels",
                )
                .font(theme::font_small())
                .color(theme::TEXT_FAINT),
            );
        });
    }

    fn error_card(&self, ui: &mut Ui) -> Option<CaptureOutcome> {
        let message = self.error.clone().unwrap_or_default();
        let mut outcome = None;
        let ctx = ui.ctx().clone();
        egui::Area::new(Id::new(ERROR_ID))
            .order(Order::Foreground)
            .anchor(Align2::CENTER_CENTER, Vec2::ZERO)
            .show(&ctx, |ui| {
                theme::card_frame().show(ui, |ui| {
                    ui.set_max_width(420.0);
                    ui.vertical_centered(|ui| {
                        ui.add_space(theme::SPACE_SM);
                        let (icon_rect, _) =
                            ui.allocate_exact_size(Vec2::splat(44.0), Sense::hover());
                        ui.painter().circle_filled(
                            icon_rect.center(),
                            22.0,
                            theme::tint(theme::DANGER, 26),
                        );
                        ui.painter().text(
                            icon_rect.center(),
                            Align2::CENTER_CENTER,
                            "⚠",
                            theme::font_mono(20.0),
                            theme::DANGER,
                        );
                        ui.add_space(theme::SPACE_SM);
                        ui.label(
                            RichText::new("Capture failed")
                                .font(theme::font_heading())
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(theme::SPACE_XS);
                        ui.add(
                            egui::Label::new(
                                RichText::new(&message)
                                    .font(theme::font_body())
                                    .color(theme::TEXT_MUTED),
                            )
                            .wrap(),
                        );
                        ui.add_space(theme::SPACE_MD);
                        if theme::primary_button(ui, "Dismiss").clicked() {
                            outcome = Some(CaptureOutcome::Cancel);
                        }
                        ui.add_space(theme::SPACE_XS);
                        ui.label(
                            RichText::new("Esc also leaves capture mode")
                                .font(theme::font_small())
                                .color(theme::TEXT_FAINT),
                        );
                        ui.add_space(theme::SPACE_SM);
                    });
                });
            });
        outcome
    }
}

/// Compact frame used by the floating capture toolbar / instructions / errors.
fn capture_frame() -> egui::Frame {
    egui::Frame::NONE
        .fill(theme::tint(theme::PANEL_BG_RAISED, 250))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .corner_radius(theme::radius(theme::RADIUS_LG))
        .inner_margin(theme::margin_xy(theme::SPACE_MD, theme::SPACE_SM))
        .shadow(egui::epaint::Shadow {
            offset: [0, 6],
            blur: 20,
            spread: 0,
            color: Color32::from_black_alpha(130),
        })
}

// ---------------------------------------------------------------------------
// Pure geometry / rendering helpers
// ---------------------------------------------------------------------------

/// Letterboxed rectangle for the frozen image inside the viewport.
///
/// Returns `None` for an empty image or an empty viewport.
pub(crate) fn fitted_image_rect(image_size: [usize; 2], screen: Rect) -> Option<Rect> {
    let [image_width, image_height] = image_size;
    if image_width == 0 || image_height == 0 {
        return None;
    }
    if !screen.is_positive() {
        return None;
    }
    let scale = (screen.width() / image_width as f32).min(screen.height() / image_height as f32);
    if !scale.is_finite() || scale <= 0.0 {
        return None;
    }
    Some(Rect::from_center_size(
        screen.center(),
        vec2(image_width as f32 * scale, image_height as f32 * scale),
    ))
}

/// Normalized rectangle between two arbitrary drag points.
pub(crate) fn rect_from_points(a: [f32; 2], b: [f32; 2]) -> CropRect {
    CropRect {
        min: [a[0].min(b[0]), a[1].min(b[1])],
        max: [a[0].max(b[0]), a[1].max(b[1])],
    }
}

/// Clamp a normalized point into a selection rectangle.
pub(crate) fn clamp_to_crop(point: [f32; 2], crop: CropRect) -> [f32; 2] {
    [
        point[0].clamp(crop.min[0], crop.max[0]),
        point[1].clamp(crop.min[1], crop.max[1]),
    ]
}

/// A selection is only committed when it is at least `min_screen_px` wide/tall.
pub(crate) fn screen_selection_valid(
    selection: CropRect,
    image_rect: Rect,
    min_screen_px: f32,
) -> bool {
    let rect = crop_to_screen(selection, image_rect);
    rect.width() >= min_screen_px && rect.height() >= min_screen_px
}

/// Rasterize annotations into a copy of the full-resolution frame and crop it.
///
/// The source image is never modified; the returned image is the final capture
/// the parent should store or insert.
pub(crate) fn render_capture(
    image: &RgbaImage,
    selection: CropRect,
    annotations: &[Annotation],
) -> anyhow::Result<RgbaImage> {
    if image.width() == 0 || image.height() == 0 {
        anyhow::bail!("image is empty");
    }
    let mut canvas = image.clone();
    for annotation in annotations {
        draw_annotation(&mut canvas, annotation);
    }
    let (x, y, width, height) = crop_pixels(selection, canvas.width(), canvas.height());
    Ok(image::imageops::crop_imm(&canvas, x, y, width, height).to_image())
}

/// Size chip anchored to the selection corners.
fn paint_selection_size(
    painter: &egui::Painter,
    selection: CropRect,
    image_rect: Rect,
    image_size: [usize; 2],
    screen: Rect,
) {
    let (width, height) = selection.pixel_size(image_size);
    let text = format!("{width} × {height}");
    let galley = painter.layout_no_wrap(text, theme::font_small(), theme::TEXT_PRIMARY);
    let padding = vec2(theme::SPACE_SM, 3.0);
    let size = galley.size() + padding * 2.0;
    let selection_rect = crop_to_screen(selection, image_rect);
    let mut position = pos2(
        selection_rect.left(),
        selection_rect.top() - size.y - theme::SPACE_XS,
    );
    if position.y < screen.top() + theme::SPACE_XS {
        position.y = selection_rect.top() + theme::SPACE_XS;
    }
    if position.x + size.x > screen.right() - theme::SPACE_XS {
        position.x = screen.right() - size.x - theme::SPACE_XS;
    }
    let chip = Rect::from_min_size(position, size);
    painter.rect_filled(
        chip,
        theme::radius(theme::RADIUS_SM),
        Color32::from_black_alpha(200),
    );
    painter.rect_stroke(
        chip,
        theme::radius(theme::RADIUS_SM),
        Stroke::new(1.0, theme::tint(theme::ACCENT, 140)),
        StrokeKind::Inside,
    );
    painter.galley(chip.min + padding, galley, theme::TEXT_PRIMARY);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::Pos2;
    use image::Rgba;

    fn test_screen() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))
    }

    fn run_show(overlay: &mut CaptureOverlay) -> Option<CaptureOutcome> {
        run_show_with_input(overlay, egui::RawInput::default())
    }

    fn run_show_with_input(
        overlay: &mut CaptureOverlay,
        mut input: egui::RawInput,
    ) -> Option<CaptureOutcome> {
        input.screen_rect = Some(test_screen());
        let ctx = egui::Context::default();
        let mut outcome = None;
        let output = ctx.run_ui(input, |ui| {
            outcome = overlay.show(ui);
        });
        output.drop_without_applying_deltas();
        outcome
    }

    fn key_event(key: Key, modifiers: Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    fn overlay_in_annotate_stage() -> CaptureOverlay {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(320, 180, [30, 40, 60, 255]));
        overlay.selection = Some(CropRect {
            min: [0.2, 0.2],
            max: [0.8, 0.8],
        });
        overlay.stage = Stage::Annotating;
        overlay.annotations.push(Annotation {
            kind: Tool::Rectangle,
            color: Color32::from_rgb(255, 0, 0),
            width: 3.0 / 320.0,
            points: vec![[0.3, 0.3], [0.7, 0.7]],
        });
        overlay
    }

    fn expect_save(outcome: Option<CaptureOutcome>, selection: CropRect) -> RgbaImage {
        match outcome {
            Some(CaptureOutcome::Save(image)) => {
                let (width, height) = selection.pixel_size([320, 180]);
                assert_eq!(image.dimensions(), (width, height));
                image
            }
            other => panic!("expected a Save outcome, got {other:?}"),
        }
    }

    fn solid(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
        RgbaImage::from_pixel(width, height, Rgba(color))
    }

    #[test]
    fn fitted_rect_letterboxes_and_centers() {
        let screen = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 500.0));

        let square = fitted_image_rect([1000, 1000], screen).expect("valid image");
        assert!((square.width() - 500.0).abs() < 0.01);
        assert!((square.height() - 500.0).abs() < 0.01);
        assert!((square.center().x - 500.0).abs() < 0.01);
        assert!((square.center().y - 250.0).abs() < 0.01);

        let wide = fitted_image_rect([2000, 1000], screen).expect("valid image");
        assert!((wide.width() - 1000.0).abs() < 0.01);
        assert!((wide.height() - 500.0).abs() < 0.01);

        let tall = fitted_image_rect([500, 1000], screen).expect("valid image");
        assert!((tall.width() - 250.0).abs() < 0.01);
        assert!((tall.height() - 500.0).abs() < 0.01);

        assert!(fitted_image_rect([0, 0], screen).is_none());
        assert!(
            fitted_image_rect([100, 100], Rect::from_min_size(Pos2::ZERO, Vec2::ZERO)).is_none()
        );
    }

    #[test]
    fn rect_from_points_orders_the_corners() {
        let rect = rect_from_points([0.8, 0.9], [0.2, 0.1]);
        assert!((rect.min[0] - 0.2).abs() < f32::EPSILON);
        assert!((rect.min[1] - 0.1).abs() < f32::EPSILON);
        assert!((rect.max[0] - 0.8).abs() < f32::EPSILON);
        assert!((rect.max[1] - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn clamp_to_crop_keeps_points_inside_selection() {
        let crop = CropRect {
            min: [0.2, 0.3],
            max: [0.7, 0.8],
        };
        assert_eq!(clamp_to_crop([0.5, 0.5], crop), [0.5, 0.5]);
        assert_eq!(clamp_to_crop([-1.0, 2.0], crop), [0.2, 0.8]);
        assert_eq!(clamp_to_crop([1.0, 0.0], crop), [0.7, 0.3]);
    }

    #[test]
    fn tiny_selections_are_rejected() {
        let image_rect = Rect::from_min_size(Pos2::ZERO, vec2(1000.0, 1000.0));
        let tiny = CropRect {
            min: [0.5, 0.5],
            max: [0.504, 0.504],
        };
        assert!(!screen_selection_valid(tiny, image_rect, 6.0));

        let big = CropRect {
            min: [0.1, 0.1],
            max: [0.5, 0.5],
        };
        assert!(screen_selection_valid(big, image_rect, 6.0));
    }

    #[test]
    fn render_capture_crops_to_selection() {
        let image = solid(40, 40, [0, 0, 0, 255]);
        let selection = CropRect {
            min: [0.25, 0.25],
            max: [0.75, 0.75],
        };
        let output = render_capture(&image, selection, &[]).expect("render");
        assert_eq!(output.dimensions(), (20, 20));
        assert_eq!(output.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn render_capture_rasterizes_annotations_before_cropping() {
        let image = solid(40, 40, [0, 0, 0, 255]);
        let selection = CropRect {
            min: [0.25, 0.25],
            max: [0.75, 0.75],
        };
        let annotation = Annotation {
            kind: Tool::Pen,
            color: Color32::from_rgb(255, 255, 255),
            width: 4.0 / 40.0,
            points: vec![[0.3, 0.5], [0.7, 0.5]],
        };
        let output = render_capture(&image, selection, &[annotation]).expect("render");
        assert_eq!(output.dimensions(), (20, 20));
        // The stroke crosses the middle row of the crop; the top-left corner is
        // untouched black.
        assert!(output.get_pixel(10, 10)[0] > 150);
        assert_eq!(output.get_pixel(0, 0), &Rgba([0, 0, 0, 255]));
    }

    #[test]
    fn render_capture_rejects_an_empty_image() {
        let image = solid(0, 0, [0, 0, 0, 255]);
        let selection = CropRect {
            min: [0.0, 0.0],
            max: [1.0, 1.0],
        };
        assert!(render_capture(&image, selection, &[]).is_err());
    }

    #[test]
    fn begin_and_finish_toggle_activity() {
        let mut overlay = CaptureOverlay::new();
        assert!(!overlay.is_active());
        overlay.begin(solid(64, 48, [10, 20, 30, 255]));
        assert!(overlay.is_active());
        overlay.finish();
        assert!(!overlay.is_active());
    }

    #[test]
    fn empty_images_produce_an_error_state() {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(0, 0, [0, 0, 0, 255]));
        assert!(overlay.is_active());
        assert!(overlay.error.is_some());
        assert!(run_show(&mut overlay).is_none());
        overlay.finish();
    }

    #[test]
    fn show_selecting_stage_does_not_panic_or_commit() {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(320, 180, [30, 40, 60, 255]));
        assert!(run_show(&mut overlay).is_none());
        assert_eq!(overlay.stage, Stage::Selecting);
        overlay.finish();
    }

    #[test]
    fn annotating_stage_renders_and_saves() {
        let mut overlay = overlay_in_annotate_stage();
        // Renders the toolbar and annotations without panicking.
        assert!(run_show(&mut overlay).is_none());

        let selection = overlay.selection.expect("selection");
        let outcome = overlay.render_outcome(CaptureCommand::Save).expect("save");
        expect_save(Some(outcome), selection);
        overlay.finish();
    }

    #[test]
    fn escape_cancels_capture_mode() {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(64, 36, [10, 20, 30, 255]));
        let outcome = run_show_with_input(
            &mut overlay,
            egui::RawInput {
                events: vec![key_event(Key::Escape, Modifiers::NONE)],
                ..Default::default()
            },
        );
        assert!(matches!(outcome, Some(CaptureOutcome::Cancel)));
        overlay.finish();
    }

    #[test]
    fn enter_saves_the_selected_region() {
        let mut overlay = overlay_in_annotate_stage();
        let selection = overlay.selection.expect("selection");
        let outcome = run_show_with_input(
            &mut overlay,
            egui::RawInput {
                events: vec![key_event(Key::Enter, Modifiers::NONE)],
                ..Default::default()
            },
        );
        expect_save(outcome, selection);
        overlay.finish();
    }

    #[test]
    fn ctrl_s_saves_the_selected_region() {
        let mut overlay = overlay_in_annotate_stage();
        let selection = overlay.selection.expect("selection");
        let outcome = run_show_with_input(
            &mut overlay,
            egui::RawInput {
                events: vec![key_event(
                    Key::S,
                    Modifiers {
                        ctrl: true,
                        command: true,
                        ..Default::default()
                    },
                )],
                ..Default::default()
            },
        );
        expect_save(outcome, selection);
        overlay.finish();
    }

    #[test]
    fn large_sources_downscale_the_preview_texture_only() {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(2560, 1440, [5, 10, 15, 255]));
        let outcome = run_show_with_input(
            &mut overlay,
            egui::RawInput {
                max_texture_side: Some(2048),
                ..Default::default()
            },
        );
        assert!(outcome.is_none());
        let texture = overlay.texture.as_ref().expect("preview texture");
        assert_eq!(texture.size(), [2048, 1152]);
        // The full-resolution source is kept for crop/export.
        assert_eq!(overlay.image_size, [2560, 1440]);
        overlay.finish();
    }

    #[test]
    fn inactive_overlay_draws_nothing() {
        let mut overlay = CaptureOverlay::new();
        assert!(run_show(&mut overlay).is_none());
    }

    #[test]
    fn undo_restores_the_previous_annotation_list() {
        let mut overlay = overlay_in_annotate_stage();
        assert_eq!(overlay.annotations.len(), 1);
        overlay.undo();
        // The helper staged the annotation without history; nothing to undo.
        assert_eq!(overlay.annotations.len(), 1);

        overlay.push_history();
        overlay.annotations.clear();
        overlay.undo();
        assert_eq!(overlay.annotations.len(), 1);
        overlay.finish();
    }

    #[test]
    fn reselect_clears_selection_and_annotations() {
        let mut overlay = overlay_in_annotate_stage();
        overlay.reselect();
        assert_eq!(overlay.stage, Stage::Selecting);
        assert!(overlay.selection.is_none());
        assert!(overlay.annotations.is_empty());
        assert!(overlay.history.is_empty());
        overlay.finish();
    }

    #[test]
    fn annotations_are_clamped_inside_the_selection() {
        let mut overlay = overlay_in_annotate_stage();
        let selection = overlay.selection.expect("selection");
        let image_rect = Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0));
        overlay.begin_annotation([-1.0, -1.0], selection, image_rect);
        let draft = overlay.draft.as_ref().expect("draft");
        assert!((draft.points[0][0] - selection.min[0]).abs() < f32::EPSILON);
        assert!((draft.points[0][1] - selection.min[1]).abs() < f32::EPSILON);

        overlay.update_annotation([5.0, 5.0], selection);
        let draft = overlay.draft.as_ref().expect("draft");
        let last = draft.points.last().expect("point");
        assert!((last[0] - selection.max[0]).abs() < f32::EPSILON);
        assert!((last[1] - selection.max[1]).abs() < f32::EPSILON);
        overlay.finish();
    }

    #[test]
    fn insert_outcome_returns_the_rendered_image() {
        let mut overlay = overlay_in_annotate_stage();
        let selection = overlay.selection.expect("selection");
        let outcome = overlay
            .render_outcome(CaptureCommand::Insert)
            .expect("insert outcome");
        match outcome {
            CaptureOutcome::Insert(image) => {
                let (width, height) = selection.pixel_size([320, 180]);
                assert_eq!(image.dimensions(), (width, height));
            }
            other => panic!("expected an Insert outcome, got {other:?}"),
        }
        overlay.finish();
    }

    #[test]
    fn toolbar_cancel_returns_cancel_without_state_loss() {
        let mut overlay = CaptureOverlay::new();
        overlay.begin(solid(64, 64, [1, 2, 3, 255]));
        overlay.set_error("capture backend unavailable");
        assert!(overlay.is_active());
        assert!(run_show(&mut overlay).is_none());
        // There is no synthetic pointer/keyboard input in this harness; the
        // error card is still rendered and remains dismissable via Esc.
        overlay.finish();
        assert!(!overlay.is_active());
    }
}
