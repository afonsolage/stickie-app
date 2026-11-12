//! The egui overlay: shell layout, navigation, tabs and transient notices.
//!
//! This module is deliberately thin. Tabs own their own widgets/state and talk
//! to the [`Store`] directly; anything that needs the Bevy window, platform or
//! event loop is returned as a [`UiAction`] for `app.rs` to execute.

mod capture_overlay;
mod editor_md;
mod files;
mod notepad;
pub(crate) mod screen_sticky;
mod screenshot_editor;
mod screenshots;
mod settings;
mod stickies;
mod theme;

use std::path::PathBuf;
use std::time::{Duration, Instant};

use bevy::prelude::Resource;
use bevy_egui::egui::{self, Align, Layout, RichText, UiBuilder};
use egui::{Color32, CornerRadius, Margin, Rect, Stroke, Vec2, pos2, vec2};

use crate::store::Store;
use capture_overlay::{CaptureOutcome, CaptureOverlay};
use theme::ToastKind;

pub use screenshot_editor::{ScreenshotEditor, ScreenshotEditorEvent};

/// A user-visible action requested by the overlay. Executed by the app layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiAction {
    Hide,
    Quit,
    CaptureScreenshot,
    RebindHotkey(String),
    RevealDataDir,
    OpenPath(PathBuf),
    /// Pin the sticky with this id to a small always-on-top window.
    PinSticky(String),
    /// Stop showing the sticky with this id in its small window.
    UnpinSticky(String),
}

/// Read-only data the UI needs from the app/platform layer.
#[derive(Debug, Clone)]
pub struct UiView {
    pub workspace_name: String,
    pub game_title: Option<String>,
    pub executable: Option<String>,
    pub hotkey_active: String,
    pub hotkey_warning: Option<String>,
    pub overlay_supported: bool,
    pub capture_supported: bool,
    pub scrim_opacity: f32,
    pub platform: String,
    pub data_root: PathBuf,
}

/// Tab destinations, in rail order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Notes,
    Stickies,
    Files,
    Screenshots,
    Settings,
}

impl Tab {
    const ALL: [Self; 5] = [
        Self::Notes,
        Self::Stickies,
        Self::Files,
        Self::Screenshots,
        Self::Settings,
    ];

    const fn label(self) -> &'static str {
        match self {
            Self::Notes => "Notes",
            Self::Stickies => "Stickies",
            Self::Files => "Files",
            Self::Screenshots => "Shots",
            Self::Settings => "Settings",
        }
    }

    const fn shortcut(self) -> &'static str {
        match self {
            Self::Notes => "Ctrl+1",
            Self::Stickies => "Ctrl+2",
            Self::Files => "Ctrl+3",
            Self::Screenshots => "Ctrl+4",
            Self::Settings => "Ctrl+5",
        }
    }
}

/// A transient message shown at the bottom center of the overlay.
struct Notice {
    text: String,
    kind: ToastKind,
    expires_at: Instant,
}

/// State for the whole overlay UI.
#[derive(Resource)]
pub struct OverlayUi {
    active_tab: Tab,
    notes: notepad::NotesTab,
    stickies: stickies::StickiesTab,
    files: files::FilesTab,
    screenshots: screenshots::ScreenshotsTab,
    settings: settings::SettingsTab,
    capture: CaptureOverlay,
    actions: Vec<UiAction>,
    notice: Option<Notice>,
    theme_applied: bool,
    opened_once: bool,
}

impl Default for OverlayUi {
    fn default() -> Self {
        Self::new()
    }
}

impl OverlayUi {
    /// Creates an empty overlay UI. A workspace must be loaded before drawing.
    pub fn new() -> Self {
        Self {
            active_tab: Tab::default(),
            notes: notepad::NotesTab::new(),
            stickies: stickies::StickiesTab::new(),
            files: files::FilesTab::new(),
            screenshots: screenshots::ScreenshotsTab::new(),
            settings: settings::SettingsTab::new(),
            capture: CaptureOverlay::new(),
            actions: Vec::new(),
            notice: None,
            theme_applied: false,
            opened_once: false,
        }
    }

    /// Reloads per-workspace state after the app opens a different game.
    pub fn on_workspace_changed(&mut self, store: &Store) {
        self.notes.reload(store);
        self.stickies.reload(store);
        self.files.reload(store);
        self.screenshots.reload(store);
        self.settings.on_workspace_changed(store);
        if !self.opened_once {
            self.opened_once = true;
            self.active_tab = Tab::Notes;
        }
    }

    /// Shows an error as a transient toast.
    pub fn push_error(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: ToastKind::Error,
            expires_at: Instant::now() + Duration::from_secs(7),
        });
    }

    /// Shows an informational toast.
    pub fn push_info(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: ToastKind::Info,
            expires_at: Instant::now() + Duration::from_secs(4),
        });
    }

    /// Shows a success toast.
    pub fn push_success(&mut self, text: impl Into<String>) {
        self.notice = Some(Notice {
            text: text.into(),
            kind: ToastKind::Success,
            expires_at: Instant::now() + Duration::from_secs(3),
        });
    }

    /// Starts the full-screen crop/annotate capture mode with a fresh screen
    /// capture. The image is kept in memory until the user saves it.
    pub fn begin_capture(&mut self, image: image::RgbaImage) {
        self.capture.begin(image);
    }

    /// Whether the crop/annotate capture mode currently owns the overlay.
    pub fn is_capturing(&self) -> bool {
        self.capture.is_active()
    }

    /// Opens the Stickies tab (used by the screen-sticky Edit action).
    pub fn open_stickies(&mut self) {
        self.active_tab = Tab::Stickies;
    }

    /// Requests that the screenshot editor open this capture on the next frame.
    pub fn open_screenshot(&mut self, path: PathBuf) {
        self.active_tab = Tab::Screenshots;
        self.screenshots.queue_open(path);
    }

    /// Returns and clears all actions queued since the previous frame.
    pub fn take_actions(&mut self) -> Vec<UiAction> {
        std::mem::take(&mut self.actions)
    }

    /// Debounced autosave, called after the UI has been drawn each frame.
    pub fn autosave(&mut self, store: &mut Store) {
        self.notes.autosave(store);
        self.stickies.autosave(store);
        self.settings.autosave(store);
    }

    /// Flushes all pending edits, used before hiding/quitting the overlay.
    pub fn flush(&mut self, store: &mut Store) {
        self.notes.flush(store);
        self.stickies.flush(store);
        self.settings.flush(store);
    }

    /// Draws the overlay for one egui pass.
    pub fn show(&mut self, viewport: &mut egui::Ui, store: &mut Store, view: &UiView) {
        let ctx = viewport.ctx().clone();
        if !self.theme_applied {
            theme::apply_theme(&ctx);
            self.theme_applied = true;
        }

        if self.capture.is_active() {
            if let Some(outcome) = self.capture.show(viewport) {
                self.handle_capture_outcome(outcome, store);
            }
            self.draw_notice(&ctx);
            return;
        }

        self.handle_global_shortcuts(&ctx, view);

        let screen = ctx.content_rect();
        viewport.painter().rect_filled(
            screen,
            CornerRadius::ZERO,
            Color32::from_black_alpha(scrim_alpha(view.scrim_opacity)),
        );

        let surface = surface_rect(screen);
        egui::Area::new(egui::Id::new("stickie-surface"))
            .order(egui::Order::Foreground)
            .fixed_pos(surface.min)
            .show(&ctx, |ui| {
                draw_surface(ui, surface.size(), self, store, view);
            });

        self.draw_notice(&ctx);
    }

    fn handle_capture_outcome(&mut self, outcome: CaptureOutcome, store: &mut Store) {
        let image = match outcome {
            CaptureOutcome::Cancel => {
                self.capture.finish();
                return;
            }
            CaptureOutcome::Save(image) => {
                match store.save_screenshot(&image) {
                    Ok(path) => {
                        self.screenshots.reload(store);
                        self.active_tab = Tab::Screenshots;
                        self.capture.finish();
                        let name = path
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                            .unwrap_or_else(|| path.display().to_string());
                        self.push_success(format!("Screenshot saved as {name}"));
                    }
                    Err(error) => self
                        .capture
                        .set_error(format!("Could not save screenshot: {error:#}")),
                }
                return;
            }
            CaptureOutcome::Insert(image) => image,
        };

        match store.save_screenshot(&image) {
            Ok(path) => {
                let inserted = self.notes.attach_screenshot(&path, store);
                self.screenshots.reload(store);
                self.capture.finish();
                if inserted {
                    self.active_tab = Tab::Notes;
                    self.push_success("Screenshot inserted into the note.");
                } else {
                    self.active_tab = Tab::Screenshots;
                    self.push_error("Screenshot saved, but no note was open to insert it into.");
                }
            }
            Err(error) => self
                .capture
                .set_error(format!("Could not save screenshot: {error:#}")),
        }
    }

    fn handle_global_shortcuts(&mut self, ctx: &egui::Context, view: &UiView) {
        let hide = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        if hide {
            // Autosave happens before the app handles the action, and a
            // half-word is much better than trapping the user in the overlay.
            self.actions.push(UiAction::Hide);
        }

        if ctx.input(|input| input.modifiers.command || input.modifiers.ctrl) {
            let tab = ctx.input(|input| {
                if input.key_pressed(egui::Key::Num1) {
                    Some(Tab::Notes)
                } else if input.key_pressed(egui::Key::Num2) {
                    Some(Tab::Stickies)
                } else if input.key_pressed(egui::Key::Num3) {
                    Some(Tab::Files)
                } else if input.key_pressed(egui::Key::Num4) {
                    Some(Tab::Screenshots)
                } else if input.key_pressed(egui::Key::Num5) {
                    Some(Tab::Settings)
                } else {
                    None
                }
            });
            if let Some(tab) = tab {
                self.active_tab = tab;
            }
        }

        if !view.capture_supported && self.active_tab == Tab::Screenshots {
            // Screenshots remain a Windows-only feature for now; the tab still
            // shows why and lets the user manage existing captures.
        }
    }

    fn draw_notice(&mut self, ctx: &egui::Context) {
        let Some(notice) = &self.notice else {
            return;
        };
        if Instant::now() >= notice.expires_at {
            self.notice = None;
            return;
        }
        let mut dummy = egui::Ui::new(
            ctx.clone(),
            "notice-anchor".into(),
            UiBuilder::new()
                .layer_id(egui::LayerId::background())
                .max_rect(ctx.content_rect()),
        );
        let _ = &mut dummy;
        theme::toast(&dummy, &notice.text, notice.kind);
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

fn scrim_alpha(opacity: f32) -> u8 {
    (opacity.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn surface_rect(screen: Rect) -> Rect {
    let margin_x = (screen.width() * 0.055).clamp(40.0, 96.0);
    let margin_y = (screen.height() * 0.06).clamp(32.0, 72.0);
    let max_size = vec2(1180.0, 780.0);
    let size = vec2(
        (screen.width() - margin_x * 2.0)
            .min(max_size.x)
            .max(720.0_f32.min(screen.width() - 24.0)),
        (screen.height() - margin_y * 2.0)
            .min(max_size.y)
            .max(480.0_f32.min(screen.height() - 24.0)),
    );
    Rect::from_center_size(screen.center(), size)
}

fn draw_surface(
    ui: &mut egui::Ui,
    size: Vec2,
    overlay: &mut OverlayUi,
    store: &mut Store,
    view: &UiView,
) {
    ui.set_width(size.x);
    ui.set_height(size.y);

    let frame = egui::Frame::NONE
        .fill(theme::tint(theme::PANEL_BG, 246))
        .stroke(Stroke::new(1.0, theme::BORDER_STRONG))
        .corner_radius(CornerRadius::same(16))
        .inner_margin(Margin::ZERO)
        .shadow(egui::epaint::Shadow {
            offset: [0, 10],
            blur: 36,
            spread: 0,
            color: Color32::from_black_alpha(120),
        });
    frame.show(ui, |ui| {
        let rect = ui.max_rect();
        let pad = vec2(18.0, 14.0);
        let inner = rect.shrink2(pad);
        let top_h = 38.0;
        let status_h = 20.0;
        let gap = 10.0;
        let top_rect = Rect::from_min_size(inner.min, vec2(inner.width(), top_h));
        let status_rect =
            Rect::from_min_max(pos2(inner.left(), inner.bottom() - status_h), inner.max);
        let body_rect = Rect::from_min_max(
            pos2(inner.left(), top_rect.bottom() + gap),
            pos2(inner.right(), status_rect.top() - gap),
        );

        overlay.top_bar(ui, top_rect, store, view);
        body(ui, body_rect, overlay, store, view);
        overlay.status_bar(ui, status_rect, view);
    });
}

impl OverlayUi {
    fn top_bar(&mut self, ui: &mut egui::Ui, rect: Rect, store: &Store, view: &UiView) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;

                // Wordmark: an amber square + tracked uppercase text.
                let (mark_rect, _) = ui.allocate_exact_size(vec2(10.0, 10.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(mark_rect, CornerRadius::same(2), theme::ACCENT);
                ui.label(
                    RichText::new("STICKIE")
                        .font(theme::font_at(12.0))
                        .color(theme::TEXT_PRIMARY)
                        .extra_letter_spacing(1.5),
                );
                ui.add_space(4.0);
                ui.add(egui::Separator::default().vertical().spacing(10.0));

                theme::pill(ui, &view.workspace_name, theme::ACCENT);
                if let Some(title) = compact_title(view) {
                    ui.label(
                        RichText::new(title)
                            .font(theme::font_small())
                            .color(theme::TEXT_FAINT),
                    )
                    .on_hover_text(view.game_title.as_deref().unwrap_or_default());
                }
                if view.hotkey_warning.is_some() {
                    let response = theme::pill(ui, "hotkey issue", theme::DANGER);
                    if let Some(warning) = &view.hotkey_warning {
                        response.on_hover_text(warning);
                    }
                }

                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let close = theme::icon_button(ui, "×", "Close overlay (Esc)");
                    if close.clicked() {
                        self.actions.push(UiAction::Hide);
                    }
                    if view.capture_supported {
                        ui.add_space(2.0);
                        if ui
                            .add(
                                egui::Button::new(
                                    RichText::new("Capture")
                                        .font(theme::font_button())
                                        .color(theme::ON_ACCENT),
                                )
                                .fill(theme::ACCENT)
                                .corner_radius(theme::radius(theme::RADIUS_SM))
                                .min_size(vec2(0.0, theme::CONTROL_HEIGHT)),
                            )
                            .on_hover_text("Capture a screen region")
                            .clicked()
                        {
                            self.actions.push(UiAction::CaptureScreenshot);
                        }
                    }
                });

                // Keep the store parameter touched for future status use.
                let _ = store;
            },
        );
    }

    fn status_bar(&mut self, ui: &mut egui::Ui, rect: Rect, view: &UiView) {
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(rect)
                .layout(Layout::left_to_right(Align::Center)),
            |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} · local workspace · no cloud",
                        view.workspace_name
                    ))
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(
                        RichText::new("Esc close")
                            .font(theme::font_small())
                            .color(theme::TEXT_FAINT),
                    );
                    ui.add_space(8.0);
                    theme::kbd(ui, &view.hotkey_active);
                    ui.add_space(8.0);
                    ui.label(
                        RichText::new(format!("{} · {}", view.platform, env!("CARGO_PKG_VERSION")))
                            .font(theme::font_small())
                            .color(theme::TEXT_FAINT),
                    );
                });
            },
        );
    }
}

fn compact_title(view: &UiView) -> Option<String> {
    let source = view
        .game_title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .or(view.executable.as_deref())?;
    let trimmed = source.trim();
    if trimmed.len() <= 54 {
        Some(trimmed.to_string())
    } else {
        let mut out = trimmed.chars().take(52).collect::<String>();
        out.push('…');
        Some(out)
    }
}

fn body(ui: &mut egui::Ui, rect: Rect, overlay: &mut OverlayUi, store: &mut Store, view: &UiView) {
    let nav_w = 128.0;
    let nav_rect = Rect::from_min_size(rect.min, vec2(nav_w, rect.height()));
    let content_rect = Rect::from_min_max(pos2(nav_rect.right() + 18.0, rect.top()), rect.max);

    ui.scope_builder(
        UiBuilder::new()
            .max_rect(nav_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            theme::section_label(ui, "Workspace");
            ui.add_space(4.0);
            for tab in Tab::ALL {
                let selected = overlay.active_tab == tab;
                let icon = match tab {
                    Tab::Notes => NavIcon::Notes,
                    Tab::Stickies => NavIcon::Stickies,
                    Tab::Files => NavIcon::Files,
                    Tab::Screenshots => NavIcon::Screenshots,
                    Tab::Settings => NavIcon::Settings,
                };
                let response = nav_item(ui, icon, tab.label(), selected).on_hover_text(format!(
                    "{} · {}",
                    tab.label(),
                    tab.shortcut()
                ));
                if response.clicked() {
                    overlay.active_tab = tab;
                }
            }
            ui.add_space(12.0);
            theme::section_label(ui, "Quick");
            if view.capture_supported
                && nav_item(ui, NavIcon::Capture, "Capture", false)
                    .on_hover_text("Capture a screen region for notes")
                    .clicked()
            {
                overlay.actions.push(UiAction::CaptureScreenshot);
            }
            let data = nav_item(ui, NavIcon::Data, "Open data", false)
                .on_hover_text("Open the Stickie data folder in Explorer");
            if data.clicked() {
                overlay.actions.push(UiAction::RevealDataDir);
            }
        },
    );

    ui.scope_builder(
        UiBuilder::new()
            .max_rect(content_rect)
            .layout(Layout::top_down(Align::Min)),
        |ui| {
            let notice = match overlay.active_tab {
                Tab::Notes => overlay.notes.show(ui, store, &mut overlay.actions),
                Tab::Stickies => overlay.stickies.show(ui, store, &mut overlay.actions),
                Tab::Files => overlay.files.show(ui, store, &mut overlay.actions),
                Tab::Screenshots => overlay.screenshots.show(
                    ui,
                    store,
                    view.capture_supported,
                    &mut overlay.actions,
                ),
                Tab::Settings => overlay.settings.show(ui, store, view, &mut overlay.actions),
            };
            if let Some((text, kind)) = notice {
                match kind {
                    ToastKind::Info => overlay.push_info(text),
                    ToastKind::Success => overlay.push_success(text),
                    ToastKind::Error => overlay.push_error(text),
                }
            }

            if let Some(path) = overlay.screenshots.take_insert_request() {
                let attached = overlay.notes.attach_screenshot(&path, store);
                overlay.active_tab = Tab::Notes;
                if attached {
                    overlay.push_success("Screenshot reference added to the open note.");
                } else {
                    overlay.push_error(
                        "Open or create a note first, then insert the screenshot again.",
                    );
                }
            }
        },
    );
}

/// Icons drawn with egui primitives so the overlay does not depend on emoji
/// font coverage (the default font can otherwise render boxes for symbols).
#[derive(Clone, Copy)]
enum NavIcon {
    Notes,
    Stickies,
    Files,
    Screenshots,
    Settings,
    Capture,
    Data,
}

fn nav_item(ui: &mut egui::Ui, icon: NavIcon, label: &str, selected: bool) -> egui::Response {
    let width = ui.available_width().max(118.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, 34.0), egui::Sense::click());
    let painter = ui.painter().clone();
    if selected {
        painter.rect_filled(
            rect,
            theme::radius(theme::RADIUS_MD),
            theme::tint(theme::ACCENT, 24),
        );
        let bar = Rect::from_min_size(rect.left_top() + vec2(0.0, 7.0), vec2(3.0, 20.0));
        painter.rect_filled(bar, theme::radius(2.0), theme::ACCENT);
    } else if response.hovered() {
        painter.rect_filled(rect, theme::radius(theme::RADIUS_MD), theme::hover_fill());
    }
    let color = if selected {
        theme::ACCENT
    } else if response.hovered() {
        theme::TEXT_PRIMARY
    } else {
        theme::TEXT_MUTED
    };
    let icon_rect = Rect::from_center_size(rect.left_center() + vec2(18.0, 0.0), Vec2::splat(16.0));
    paint_nav_icon(&painter, icon, icon_rect, color);
    painter.text(
        rect.left_center() + vec2(38.0, 0.0),
        egui::Align2::LEFT_CENTER,
        label,
        theme::font_body(),
        if selected { theme::TEXT_PRIMARY } else { color },
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    response
}

fn paint_nav_icon(painter: &egui::Painter, icon: NavIcon, rect: Rect, color: Color32) {
    let stroke = Stroke::new(1.4, color);
    let r = Rect::from_center_size(rect.center(), vec2(14.0, 14.0));
    match icon {
        NavIcon::Notes => {
            painter.rect_stroke(r, theme::radius(2.0), stroke, egui::StrokeKind::Inside);
            for offset in [4.0, 7.0, 10.0] {
                painter.line_segment(
                    [
                        pos2(r.left() + 3.0, r.top() + offset),
                        pos2(r.right() - 3.0, r.top() + offset),
                    ],
                    Stroke::new(1.0, color),
                );
            }
        }
        NavIcon::Stickies => {
            painter.rect_filled(r, theme::radius(2.5), theme::tint(color, 80));
            painter.rect_stroke(r, theme::radius(2.5), stroke, egui::StrokeKind::Inside);
            painter.line_segment(
                [
                    pos2(r.left() + 3.0, r.bottom() - 3.0),
                    pos2(r.right() - 3.0, r.bottom() - 3.0),
                ],
                Stroke::new(1.0, color),
            );
        }
        NavIcon::Files => {
            let tab = Rect::from_min_size(r.left_top(), vec2(6.0, 3.0));
            painter.rect_stroke(tab, theme::radius(1.0), stroke, egui::StrokeKind::Inside);
            painter.rect_stroke(
                Rect::from_min_max(pos2(r.left(), r.top() + 3.0), r.right_bottom()),
                theme::radius(2.0),
                stroke,
                egui::StrokeKind::Inside,
            );
        }
        NavIcon::Screenshots => {
            let body = Rect::from_center_size(r.center() + vec2(0.0, 1.0), vec2(14.0, 10.0));
            painter.rect_stroke(body, theme::radius(2.0), stroke, egui::StrokeKind::Inside);
            painter.circle_stroke(r.center(), 3.0, Stroke::new(1.2, color));
            painter.rect_filled(
                Rect::from_min_size(pos2(r.center().x - 3.0, r.top() - 0.5), vec2(6.0, 2.0)),
                theme::radius(1.0),
                color,
            );
        }
        NavIcon::Settings => {
            for (offset, knob) in [(4.0, 6.0), (8.0, 3.0), (12.0, 9.0)] {
                let y = r.top() + offset;
                painter.line_segment(
                    [pos2(r.left(), y), pos2(r.right(), y)],
                    Stroke::new(1.0, color),
                );
                painter.circle_filled(pos2(r.left() + knob, y), 1.7, color);
            }
        }
        NavIcon::Capture => {
            painter.circle_stroke(r.center(), 6.0, stroke);
            painter.circle_filled(r.center(), 2.0, color);
        }
        NavIcon::Data => {
            painter.line_segment([r.left_bottom(), r.right_top()], stroke);
            painter.line_segment([pos2(r.center().x + 1.0, r.top()), r.right_top()], stroke);
            painter.line_segment([r.right_top(), pos2(r.right(), r.center().y - 1.0)], stroke);
        }
    }
}

/// Helper used by tabs to lay out a header row with a title and right-aligned
/// action buttons.
fn tab_header(ui: &mut egui::Ui, title: &str, subtitle: Option<&str>) {
    ui.horizontal(|ui| {
        ui.label(
            RichText::new(title)
                .font(theme::font_title())
                .color(theme::TEXT_PRIMARY),
        );
        if let Some(subtitle) = subtitle {
            ui.label(
                RichText::new(subtitle)
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
            );
        }
    });
    ui.add_space(2.0);
}
