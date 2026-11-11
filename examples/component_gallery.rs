//! Standalone component gallery for the Stickie egui design system.
//!
//! The gallery is intentionally decoupled from the app: it includes the UI
//! modules with `#[path]` so it can be compiled and inspected independently of
//! how `src/ui/mod.rs` is wired up.
//!
//! ```bash
//! CARGO_HOME="$PWD/.cargo-home" cargo run --example component_gallery
//! CARGO_HOME="$PWD/.cargo-home" cargo run --example component_gallery -- /path/to/screenshot.png
//! ```
//!
//! If a PNG path is passed as the first argument, the screenshot editor page
//! opens it. Without an argument a synthetic test image is generated in the
//! system temp directory so the editor is always exercisable.

#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "The gallery generates a synthetic demo screenshot with pixel-space math."
)]
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use bevy::window::WindowPlugin;
use bevy_egui::{EguiContexts, EguiPlugin, EguiPrimaryContextPass, egui};

use editor_md::MarkdownEditor;
use screenshot_editor::{ScreenshotEditor, ScreenshotEditorEvent};

#[path = "../src/ui/theme.rs"]
mod theme;

#[path = "../src/ui/editor_md.rs"]
mod editor_md;

#[path = "../src/ui/screenshot_editor.rs"]
mod screenshot_editor;

/// Width of the gallery navigation rail, including the frame padding.
const RAIL_WIDTH: f32 = 240.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Page {
    Appearance,
    Markdown,
    Screenshot,
}

struct ToastState {
    text: String,
    kind: theme::ToastKind,
    until: f64,
}

#[derive(Resource)]
struct Gallery {
    page: Page,
    theme_applied: bool,
    md: MarkdownEditor,
    md_loaded: bool,
    shot: ScreenshotEditor,
    shot_attempted: bool,
    toast: Option<ToastState>,
}

impl Gallery {
    fn new() -> Self {
        // Deterministic page selection keeps automated screenshots simple:
        // STICKIE_GALLERY_PAGE=markdown|screenshot.
        let page = match std::env::var("STICKIE_GALLERY_PAGE").as_deref() {
            Ok("markdown") => Page::Markdown,
            Ok("screenshot") => Page::Screenshot,
            _ => Page::Appearance,
        };
        Self {
            page,
            theme_applied: false,
            md: MarkdownEditor::new(),
            md_loaded: false,
            shot: ScreenshotEditor::new(),
            shot_attempted: false,
            toast: None,
        }
    }

    fn show_toast(&mut self, text: impl Into<String>, kind: theme::ToastKind, now: f64) {
        self.toast = Some(ToastState {
            text: text.into(),
            kind,
            until: now + 2.5,
        });
    }
}

fn main() {
    let screenshot_path = std::env::args().nth(1).map(PathBuf::from);

    App::new()
        .insert_resource(ClearColor(Color::srgb(0.035, 0.045, 0.065)))
        .insert_resource(Gallery::new())
        .insert_resource(ScreenshotSource(screenshot_path))
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Stickie — component gallery".into(),
                resolution: (1320, 860).into(),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(EguiPlugin::default())
        .add_systems(Startup, setup_camera)
        .add_systems(EguiPrimaryContextPass, gallery_ui)
        .run();
}

/// Path passed on the command line for the screenshot editor page.
#[derive(Resource)]
struct ScreenshotSource(Option<PathBuf>);

fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Bevy system parameters are passed by value by design."
)]
fn gallery_ui(
    mut contexts: EguiContexts,
    mut gallery: ResMut<Gallery>,
    source: Res<ScreenshotSource>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    if !gallery.theme_applied {
        theme::apply_theme(ctx);
        gallery.theme_applied = true;
    }
    let now = ctx.input(|i| i.time);

    let mut viewport = egui::Ui::new(
        ctx.clone(),
        "viewport".into(),
        egui::UiBuilder::new()
            .layer_id(egui::LayerId::background())
            .max_rect(ctx.viewport_rect()),
    );
    theme::scrim(&viewport);

    egui::Frame::NONE
        .inner_margin(theme::margin(theme::SPACE_XL))
        .show(&mut viewport, |ui| {
            let body_height = ui.available_height();
            ui.horizontal_top(|ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(RAIL_WIDTH, body_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| rail(ui, &mut gallery),
                );
                ui.add_space(theme::SPACE_LG);
                let content_width = ui.available_width();
                ui.allocate_ui_with_layout(
                    egui::vec2(content_width, body_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| content(ui, &mut gallery, &source, now),
                );
            });
        });

    if gallery.page == Page::Markdown {
        handle_editor_autosave(&mut gallery, ctx, now);
    }

    // Transient toast: one-shot status feedback, painted on top.
    if let Some(toast) = &gallery.toast
        && now < toast.until
    {
        theme::toast(&viewport, &toast.text, toast.kind);
        ctx.request_repaint_after(Duration::from_millis(100));
    }
    if gallery
        .toast
        .as_ref()
        .is_some_and(|toast| now >= toast.until)
    {
        gallery.toast = None;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Shell: navigation rail
// ---------------------------------------------------------------------------

fn rail(ui: &mut egui::Ui, gallery: &mut Gallery) {
    theme::panel_frame()
        .inner_margin(theme::margin(theme::SPACE_MD))
        .show(ui, |ui| {
            ui.set_width(RAIL_WIDTH - 2.0 * theme::SPACE_MD - 2.0);
            ui.set_min_height(ui.available_height());

            ui.horizontal(|ui| {
                let (mark, _) =
                    ui.allocate_exact_size(egui::vec2(28.0, 28.0), egui::Sense::hover());
                ui.painter()
                    .rect_filled(mark, theme::radius(theme::RADIUS_SM), theme::ACCENT);
                ui.painter().text(
                    mark.center(),
                    egui::Align2::CENTER_CENTER,
                    "S",
                    theme::font_heading(),
                    theme::ON_ACCENT,
                );
                ui.add_space(theme::SPACE_XS);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new("Stickie")
                            .font(theme::font_heading())
                            .color(theme::TEXT_PRIMARY),
                    );
                    ui.label(
                        egui::RichText::new("component gallery")
                            .font(theme::font_small())
                            .color(theme::TEXT_FAINT),
                    );
                });
            });

            ui.add_space(theme::SPACE_LG);
            theme::section_label(ui, "Gallery");
            ui.add_space(theme::SPACE_XS);

            if theme::nav_button(ui, "◐", "Appearance", gallery.page == Page::Appearance).clicked()
            {
                gallery.page = Page::Appearance;
            }
            if theme::nav_button(ui, "🖹", "Markdown", gallery.page == Page::Markdown).clicked() {
                gallery.page = Page::Markdown;
            }
            if theme::nav_button(ui, "🖼", "Screenshot", gallery.page == Page::Screenshot).clicked()
            {
                gallery.page = Page::Screenshot;
            }

            // Push the footer to the bottom of the rail.
            let remaining = ui.available_height() - 56.0;
            if remaining > 0.0 {
                ui.add_space(remaining);
            }
            theme::section_label(ui, "About");
            ui.label(
                egui::RichText::new("A minimal overlay design system.")
                    .font(theme::font_small())
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(theme::SPACE_XS);
            ui.horizontal(|ui| {
                theme::pill(ui, "egui 0.36", theme::TEXT_MUTED);
                theme::pill(ui, env!("CARGO_PKG_VERSION"), theme::TEXT_FAINT);
            });
        });
}

// ---------------------------------------------------------------------------
// Shell: page content
// ---------------------------------------------------------------------------

fn content(ui: &mut egui::Ui, gallery: &mut Gallery, source: &ScreenshotSource, now: f64) {
    {
        let (title, subtitle) = match gallery.page {
            Page::Appearance => (
                "Appearance",
                "Design tokens, painted widgets and empty states for the Stickie overlay.",
            ),
            Page::Markdown => (
                "Markdown editor",
                "A distraction-free markdown notepad with live preview and autosave hooks.",
            ),
            Page::Screenshot => (
                "Screenshot editor",
                "Crop and annotate a captured frame, then drop it into a note.",
            ),
        };

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    egui::RichText::new(title)
                        .font(theme::font_title())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    egui::RichText::new(subtitle)
                        .font(theme::font_body())
                        .color(theme::TEXT_MUTED),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                theme::pill(ui, "theme.rs", theme::ACCENT);
                theme::pill(ui, "dark overlay", theme::TEXT_MUTED);
            });
        });

        ui.add_space(theme::SPACE_MD);
        match gallery.page {
            Page::Appearance => {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| page_appearance(ui, gallery, now));
            }
            Page::Markdown => page_markdown(ui, gallery),
            Page::Screenshot => page_screenshot(ui, gallery, source, now),
        }
    }
}

// ---------------------------------------------------------------------------
// Page: appearance
// ---------------------------------------------------------------------------

fn page_appearance(ui: &mut egui::Ui, gallery: &mut Gallery, now: f64) {
    appearance_palette(ui);
    ui.add_space(theme::SPACE_MD);
    appearance_sticky_palette(ui);
    ui.add_space(theme::SPACE_MD);
    appearance_widgets(ui);
    ui.add_space(theme::SPACE_MD);
    appearance_feedback(ui, gallery, now);
    ui.add_space(theme::SPACE_MD);
    appearance_empty_state(ui, gallery, now);
}

fn appearance_palette(ui: &mut egui::Ui) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_label(ui, "Core palette");
        ui.add_space(theme::SPACE_SM);
        ui.columns(2, |columns| {
            let tokens = [
                ("Scrim", theme::SCRIM, "dim behind the overlay"),
                ("Panel", theme::PANEL_BG, "main surface"),
                ("Raised", theme::PANEL_BG_RAISED, "cards and toolbars"),
                ("Input", theme::INPUT_BG, "editor canvas"),
                ("Border", theme::BORDER, "hairline"),
                ("Text", theme::TEXT_PRIMARY, "primary text"),
                ("Muted", theme::TEXT_MUTED, "secondary text"),
                ("Faint", theme::TEXT_FAINT, "hints"),
                ("Accent", theme::ACCENT, "warm amber, one accent only"),
                ("On accent", theme::ON_ACCENT, "text on accent fills"),
                ("Danger", theme::DANGER, "destructive actions"),
                ("Success", theme::SUCCESS, "confirmations"),
            ];
            for (index, (name, color, note)) in tokens.into_iter().enumerate() {
                let column = index % 2;
                token_row(&mut columns[column], name, color, note);
            }
        });
    });
}

fn appearance_sticky_palette(ui: &mut egui::Ui) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_label(ui, "Sticky palette");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            for color in theme::StickyColor::ALL {
                sticky_swatch(ui, color);
            }
        });
    });
}

fn appearance_widgets(ui: &mut egui::Ui) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_label(ui, "Navigation");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            ui.allocate_ui_with_layout(
                egui::vec2(188.0, theme::NAV_ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    theme::nav_button(ui, "🖹", "Notes", false);
                },
            );
            ui.allocate_ui_with_layout(
                egui::vec2(188.0, theme::NAV_ROW_HEIGHT),
                egui::Layout::top_down(egui::Align::LEFT),
                |ui| {
                    theme::nav_button(ui, "🖼", "Screenshots", true);
                },
            );
        });

        ui.add_space(theme::SPACE_MD);
        theme::section_label(ui, "Toolbar and actions");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            theme::icon_button(ui, "↶", "Undo");
            theme::icon_button(ui, "↷", "Redo");
            theme::icon_button(ui, "💾", "Save");
            ui.separator();
            theme::tool_button(ui, "B", "Bold (Ctrl+B)", false);
            theme::tool_button(ui, "I", "Italic (Ctrl+I)", true);
            theme::tool_button(ui, "•", "Bullet list", false);
            theme::tool_button(ui, "☑", "Checklist", false);
            ui.separator();
            theme::primary_button(ui, "Primary action");
        });

        ui.add_space(theme::SPACE_MD);
        theme::section_label(ui, "Status tags");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal_wrapped(|ui| {
            for color in theme::StickyColor::ALL {
                theme::pill(ui, color.label(), color.accent());
            }
            theme::pill(ui, "Danger", theme::DANGER);
        });

        ui.add_space(theme::SPACE_MD);
        theme::section_label(ui, "Keyboard hints");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            theme::kbd(ui, "Ctrl");
            ui.label(egui::RichText::new("+").color(theme::TEXT_FAINT));
            theme::kbd(ui, "S");
            ui.add_space(theme::SPACE_XS);
            ui.label(
                egui::RichText::new("save note")
                    .font(theme::font_small())
                    .color(theme::TEXT_MUTED),
            );
            ui.add_space(theme::SPACE_MD);
            theme::kbd(ui, "Ctrl");
            ui.label(egui::RichText::new("+").color(theme::TEXT_FAINT));
            theme::kbd(ui, "Shift");
            ui.label(egui::RichText::new("+").color(theme::TEXT_FAINT));
            theme::kbd(ui, "P");
            ui.add_space(theme::SPACE_XS);
            ui.label(
                egui::RichText::new("toggle preview")
                    .font(theme::font_small())
                    .color(theme::TEXT_MUTED),
            );
        });
    });
}

fn appearance_feedback(ui: &mut egui::Ui, gallery: &mut Gallery, now: f64) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_label(ui, "Feedback");
        ui.add_space(theme::SPACE_SM);
        ui.horizontal(|ui| {
            if theme::primary_button(ui, "Show toast").clicked() {
                gallery.show_toast("Note saved to workspace", theme::ToastKind::Success, now);
            }
            if theme::icon_button(ui, "⚠", "Show error toast").clicked() {
                gallery.show_toast(
                    "Could not write Notes/todo.md",
                    theme::ToastKind::Error,
                    now,
                );
            }
            if theme::icon_button(ui, "🐾", "Show info toast").clicked() {
                gallery.show_toast(
                    "Switched to Elden Ring workspace",
                    theme::ToastKind::Info,
                    now,
                );
            }
            ui.label(
                egui::RichText::new("toasts anchor bottom-center, above the game")
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
            );
        });
    });
}

fn appearance_empty_state(ui: &mut egui::Ui, gallery: &mut Gallery, now: f64) {
    theme::card_frame().show(ui, |ui| {
        ui.set_width(ui.available_width());
        theme::section_label(ui, "Empty state");
        ui.add_space(theme::SPACE_SM);
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), 260.0),
            egui::Layout::top_down(egui::Align::Center),
            |ui| {
                if theme::empty_state(
                    ui,
                    "🗋",
                    "No notes yet",
                    "Notes live next to your game saves. Create one to capture a build, a boss pattern or a route.",
                    Some("Create a note"),
                )
                .is_some_and(|response| response.clicked())
                {
                    gallery.show_toast("Creating note…", theme::ToastKind::Info, now);
                }
            },
        );
    });
}

fn token_row(ui: &mut egui::Ui, name: &str, color: egui::Color32, note: &str) {
    ui.horizontal(|ui| {
        let (swatch, _) = ui.allocate_exact_size(egui::vec2(18.0, 18.0), egui::Sense::hover());
        ui.painter().rect_filled(swatch, theme::radius(4.0), color);
        ui.painter().rect_stroke(
            swatch,
            theme::radius(4.0),
            egui::Stroke::new(1.0, theme::BORDER_STRONG),
            egui::StrokeKind::Inside,
        );
        ui.add_space(theme::SPACE_XS);
        ui.label(
            egui::RichText::new(name)
                .font(theme::font_body())
                .color(theme::TEXT_PRIMARY),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.label(
                egui::RichText::new(note)
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
            );
            ui.add_space(theme::SPACE_XS);
            ui.label(
                egui::RichText::new(format!(
                    "#{:02X}{:02X}{:02X}",
                    color.r(),
                    color.g(),
                    color.b()
                ))
                .font(theme::font_mono(theme::FONT_XS))
                .color(theme::TEXT_MUTED),
            );
        });
    });
}

fn sticky_swatch(ui: &mut egui::Ui, color: theme::StickyColor) {
    egui::Frame::NONE
        .fill(color.fill())
        .stroke(egui::Stroke::new(1.0, theme::tint(color.accent(), 120)))
        .corner_radius(theme::radius(theme::RADIUS_MD))
        .inner_margin(theme::margin(theme::SPACE_MD))
        .show(ui, |ui| {
            // Frame content inherits the parent's horizontal layout; force a
            // vertical stack so the card reads like a sticky note.
            ui.with_layout(egui::Layout::top_down(egui::Align::LEFT), |ui| {
                ui.set_width(120.0);
                ui.horizontal(|ui| {
                    let (dot, _) =
                        ui.allocate_exact_size(egui::vec2(8.0, 8.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot.center(), 4.0, color.accent());
                    ui.label(
                        egui::RichText::new(color.label())
                            .font(theme::font_small())
                            .color(theme::tint(color.text(), 200)),
                    );
                });
                ui.add_space(theme::SPACE_XS);
                ui.label(
                    egui::RichText::new("Sticky note")
                        .font(theme::font_heading())
                        .color(color.text()),
                );
                ui.label(
                    egui::RichText::new("drag to pin")
                        .font(theme::font_small())
                        .color(theme::tint(color.text(), 180)),
                );
            });
        });
}

// ---------------------------------------------------------------------------
// Page: markdown
// ---------------------------------------------------------------------------

/// Demo note shown in the gallery's Markdown editor.
const SAMPLE_NOTE: &str = r#"# Malenia — attempt log

> "I am Malenia, Blade of Miquella." Keep your distance in phase two.

## Openers

- **Waterfowl Dance**: run *away*, then dodge into the second flurry.
- [ ] Practice the third flurry timing
- [x] Upgrade Mimic Tear to +10

## Build notes

```text
Vigor 40 · Dex 55 · Bleed: Rivers of Blood
```

1. Respec to 60 dex after Radahn
2. Stack bleed resistance before the rot bloom

[Full fight notes](https://example.com/malenia) — replace with your own route.
"#;

fn page_markdown(ui: &mut egui::Ui, gallery: &mut Gallery) {
    if !gallery.md_loaded {
        gallery.md_loaded = true;
        gallery.md.open(
            PathBuf::from("Notes/boss-patterns.md"),
            "Boss patterns — Malenia".to_owned(),
            SAMPLE_NOTE.to_owned(),
        );
        if std::env::var("STICKIE_GALLERY_PREVIEW").is_ok_and(|value| value == "1") {
            gallery.md.set_preview(true);
        }
    }
    let height = ui.available_height().max(420.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            gallery.md.show(ui);
        },
    );
}

/// Demo of the parent-owned persistence contract: the widget never touches the
/// filesystem, so the gallery just flips the saved snapshot when a save is
/// requested (or when the debounce window elapses).
fn handle_editor_autosave(gallery: &mut Gallery, ctx: &egui::Context, now: f64) {
    if !gallery.md.is_open() {
        return;
    }
    if gallery.md.save_requested() {
        let path = gallery.md.path().display().to_string();
        gallery.md.mark_saved();
        gallery.show_toast(format!("Saved {path}"), theme::ToastKind::Success, now);
    } else if gallery.md.is_dirty()
        && gallery
            .md
            .last_edit()
            .is_some_and(|instant| instant.elapsed() >= Duration::from_millis(1200))
    {
        gallery.md.mark_saved();
        gallery.show_toast("Autosaved", theme::ToastKind::Info, now);
    }
    if gallery.md.is_dirty() {
        ctx.request_repaint_after(Duration::from_millis(250));
    }
}

// ---------------------------------------------------------------------------
// Page: screenshot
// ---------------------------------------------------------------------------

fn page_screenshot(ui: &mut egui::Ui, gallery: &mut Gallery, source: &ScreenshotSource, now: f64) {
    if !gallery.shot_attempted {
        gallery.shot_attempted = true;
        let path = source.0.clone().unwrap_or_else(demo_screenshot_path);
        match ScreenshotEditor::open(ui.ctx(), path) {
            Ok(editor) => gallery.shot = editor,
            Err(error) => {
                log::warn!("screenshot editor demo could not open image: {error:#}");
                gallery.show_toast(
                    format!("Could not open screenshot: {error}"),
                    theme::ToastKind::Error,
                    now,
                );
            }
        }
    }

    let height = ui.available_height().max(420.0);
    ui.allocate_ui_with_layout(
        egui::vec2(ui.available_width(), height),
        egui::Layout::top_down(egui::Align::LEFT),
        |ui| {
            if let Some(event) = gallery.shot.show(ui) {
                match event {
                    ScreenshotEditorEvent::Close => gallery.shot.close(),
                    ScreenshotEditorEvent::Saved(path) => gallery.show_toast(
                        format!(
                            "Saved {}",
                            path.file_name().map_or_else(String::new, |name| name
                                .to_string_lossy()
                                .into_owned())
                        ),
                        theme::ToastKind::Success,
                        now,
                    ),
                    ScreenshotEditorEvent::InsertIntoNote(path) => {
                        if gallery.md.is_open() {
                            let mut text = gallery.md.text().to_owned();
                            if !text.ends_with('\n') {
                                text.push('\n');
                            }
                            let _ = write!(text, "\n![screenshot]({})\n", path.display());
                            gallery.md.set_text(text);
                            gallery.md.set_preview(true);
                        }
                        gallery.show_toast(
                            format!(
                                "Inserted {} into the note",
                                path.file_name().map_or_else(String::new, |name| name
                                    .to_string_lossy()
                                    .into_owned())
                            ),
                            theme::ToastKind::Success,
                            now,
                        );
                    }
                }
            }
        },
    );
}

/// Generate a synthetic "game screenshot" in the temp directory so the gallery
/// can demonstrate cropping and annotation without a real capture pipeline.
fn demo_screenshot_path() -> PathBuf {
    let path = std::env::temp_dir().join("stickie-gallery-sample.png");
    if path.exists() {
        return path;
    }

    let width = 1280_u32;
    let height = 720_u32;
    let mut image = image::RgbaImage::new(width, height);
    let fw = width as f32;
    let fh = height as f32;

    // Sky gradient + distant mountains + a sun.
    for (x, y, pixel) in image.enumerate_pixels_mut() {
        let fx = x as f32 / fw;
        let fy = y as f32 / fh;
        let horizon = 0.62;
        let (r, g, b) = if fy < horizon {
            let t = fy / horizon;
            (
                lerp(0.05, 0.32, t),
                lerp(0.07, 0.16, t),
                lerp(0.15, 0.30, t),
            )
        } else {
            let t = (fy - horizon) / (1.0 - horizon);
            (
                lerp(0.09, 0.03, t),
                lerp(0.06, 0.03, t),
                lerp(0.10, 0.05, t),
            )
        };
        let mut color = [to_u8(r), to_u8(g), to_u8(b), 255];

        // Sun glow.
        let sun = (fx - 0.72) * fw / fh;
        let sun_distance = (sun * sun + (fy - 0.30) * (fy - 0.30)).sqrt();
        if sun_distance < 0.075 {
            color = [255, 186, 92, 255];
        } else if sun_distance < 0.16 {
            let glow = 1.0 - (sun_distance - 0.075) / 0.085;
            color = [
                mix(color[0], 255, glow * 0.45),
                mix(color[1], 170, glow * 0.35),
                mix(color[2], 90, glow * 0.25),
                255,
            ];
        }

        // Mountain ridge silhouettes.
        let ridge = horizon
            + 0.015 * (fx * 9.0).sin()
            + 0.02 * (fx * 21.0 + 1.3).sin()
            + 0.008 * (fx * 47.0).sin();
        if fy > ridge && fy < horizon + 0.12 {
            color = [12, 16, 28, 255];
        }
        *pixel = image::Rgba(color);
    }

    // HUD: health bar, stamina bar, compass dot, crosshair.
    fill_rect(&mut image, 48, 620, 320, 16, [18, 20, 26, 255]);
    fill_rect(&mut image, 50, 622, 200, 12, [206, 74, 74, 255]);
    fill_rect(&mut image, 48, 646, 260, 10, [18, 20, 26, 255]);
    fill_rect(&mut image, 50, 648, 180, 6, [210, 176, 74, 255]);
    fill_rect(&mut image, 1090, 52, 140, 12, [18, 20, 26, 255]);
    fill_rect(&mut image, 1092, 54, 96, 8, [90, 190, 255, 255]);
    fill_circle(&mut image, 640, 360, 26, [235, 238, 245, 200]);
    fill_circle(&mut image, 640, 360, 22, [10, 12, 18, 255]);
    fill_rect(&mut image, 636, 344, 4, 14, [235, 238, 245, 255]);
    fill_rect(&mut image, 636, 382, 4, 14, [235, 238, 245, 255]);
    fill_rect(&mut image, 624, 356, 14, 4, [235, 238, 245, 255]);
    fill_rect(&mut image, 658, 356, 14, 4, [235, 238, 245, 255]);

    let _ = image.save(&path);
    path
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

fn to_u8(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn mix(a: u8, b: u8, t: f32) -> u8 {
    (f32::from(a) + (f32::from(b) - f32::from(a)) * t.clamp(0.0, 1.0)).round() as u8
}

fn fill_rect(
    image: &mut image::RgbaImage,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: [u8; 4],
) {
    for py in y..(y + height).min(image.height()) {
        for px in x..(x + width).min(image.width()) {
            image.put_pixel(px, py, image::Rgba(color));
        }
    }
}

fn fill_circle(image: &mut image::RgbaImage, cx: u32, cy: u32, radius: u32, color: [u8; 4]) {
    let radius_squared = i64::from(radius) * i64::from(radius);
    for py in cy.saturating_sub(radius)..(cy + radius).min(image.height()) {
        for px in cx.saturating_sub(radius)..(cx + radius).min(image.width()) {
            let dx = i64::from(px) - i64::from(cx);
            let dy = i64::from(py) - i64::from(cy);
            if dx * dx + dy * dy <= radius_squared {
                image.put_pixel(px, py, image::Rgba(color));
            }
        }
    }
}
