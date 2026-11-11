//! Settings tab: hotkey, overlay opacity, storage location and app info.
use bevy_egui::egui;

use std::time::{Duration, Instant};

use egui::{RichText, ScrollArea};

use crate::store::{MAX_SCRIM_OPACITY, Store};

use super::{UiAction, UiView, theme};

/// Settings-tab local state.
pub struct SettingsTab {
    hotkey_input: String,
    hotkey_initialized: bool,
    scrim_draft: f32,
    start_hidden_draft: bool,
    config_initialized: bool,
    config_dirty_since: Option<Instant>,
}

impl Default for SettingsTab {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsTab {
    pub fn new() -> Self {
        Self {
            hotkey_input: String::new(),
            hotkey_initialized: false,
            scrim_draft: crate::store::DEFAULT_SCRIM_OPACITY,
            start_hidden_draft: true,
            config_initialized: false,
            config_dirty_since: None,
        }
    }

    pub fn on_workspace_changed(&mut self, store: &Store) {
        let config = store.config();
        self.scrim_draft = config.scrim_opacity;
        self.start_hidden_draft = config.start_hidden;
        self.hotkey_input.clone_from(&config.toggle_hotkey);
        self.hotkey_initialized = false;
        self.config_initialized = false;
        self.config_dirty_since = None;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        store: &mut Store,
        view: &UiView,
        actions: &mut Vec<UiAction>,
    ) -> Option<(String, theme::ToastKind)> {
        if !self.config_initialized {
            self.config_initialized = true;
            if !self.hotkey_initialized {
                self.hotkey_input.clone_from(&store.config().toggle_hotkey);
                self.hotkey_initialized = true;
            }
        }

        super::tab_header(ui, "Settings", Some("Tiny, local, and out of your way."));
        ui.add_space(6.0);

        let mut notice = None;
        let content_width = ui.available_width();
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_min_width(content_width);
                ui.set_max_width(content_width);

                theme::section_label(ui, "Overlay");
                ui.add_space(6.0);
                egui::Grid::new("settings-overlay-grid")
                    .num_columns(2)
                    .spacing([18.0, 12.0])
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new("Background dim")
                                .font(theme::font_body())
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.vertical(|ui| {
                            let slider = ui.add(
                                egui::Slider::new(
                                    &mut self.scrim_draft,
                                    0.0..=MAX_SCRIM_OPACITY,
                                )
                                .custom_formatter(|value, _| format!("{:.0}%", value * 100.0)),
                            );
                            if slider.changed() {
                                self.config_dirty_since = Some(Instant::now());
                            }
                            ui.label(
                                RichText::new(
                                    "How much the game is dimmed while Stickie is open.",
                                )
                                .font(theme::font_small())
                                .color(theme::TEXT_FAINT),
                            );
                        });
                        ui.end_row();

                        ui.label(
                            RichText::new("Start hidden")
                                .font(theme::font_body())
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.vertical(|ui| {
                            if ui
                                .checkbox(&mut self.start_hidden_draft, "Wait for the hotkey")
                                .changed()
                            {
                                self.config_dirty_since = Some(Instant::now());
                            }
                            ui.label(
                                RichText::new(
                                    "When disabled, the overlay opens on launch. Useful while setting up.",
                                )
                                .font(theme::font_small())
                                .color(theme::TEXT_FAINT),
                            );
                        });
                        ui.end_row();
                    });

                ui.add_space(18.0);
                theme::section_label(ui, "Hotkey");
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Active binding: {}",
                        view.hotkey_active
                    ))
                    .font(theme::font_small())
                    .color(theme::TEXT_MUTED),
                );
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    let field = ui.add_sized(
                        [220.0, 30.0],
                        egui::TextEdit::singleline(&mut self.hotkey_input)
                            .hint_text("Win+Shift+Tab")
                            .font(theme::font_mono(13.0)),
                    );
                    if field.changed() {
                        self.hotkey_initialized = true;
                    }
                    if theme::primary_button(ui, "Apply").clicked() {
                        let spec = self.hotkey_input.trim().to_string();
                        if spec.is_empty() {
                            notice = Some((
                                "Hotkey cannot be empty.".to_string(),
                                theme::ToastKind::Error,
                            ));
                        } else {
                            actions.push(UiAction::RebindHotkey(spec.clone()));
                            self.hotkey_initialized = false;
                            notice = Some((
                                format!("Registering {spec}…"),
                                theme::ToastKind::Info,
                            ));
                        }
                    }
                    if ui.button("Reset").clicked() {
                        self.hotkey_input = crate::store::DEFAULT_TOGGLE_HOTKEY.to_string();
                        self.hotkey_initialized = true;
                        actions.push(UiAction::RebindHotkey(self.hotkey_input.clone()));
                    }
                });
                ui.add_space(4.0);
                ui.label(
                    RichText::new(
                        "Use modifier+key syntax, e.g. Win+Shift+Tab. The new binding is \
                         registered immediately; the previous one is kept if registration fails.",
                    )
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
                if let Some(warning) = &view.hotkey_warning {
                    ui.add_space(6.0);
                    ui.label(
                        RichText::new(warning)
                            .font(theme::font_small())
                            .color(theme::DANGER),
                    );
                }

                ui.add_space(18.0);
                theme::section_label(ui, "Storage");
                ui.add_space(6.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(view.data_root.display().to_string())
                            .font(theme::font_mono(12.0))
                            .color(theme::TEXT_MUTED),
                    );
                    if ui.button("Open folder").clicked() {
                        actions.push(UiAction::RevealDataDir);
                    }
                });
                ui.label(
                    RichText::new(
                        "One workspace per game under workspaces/. Notes are plain Markdown files; \
                         nothing leaves this folder.",
                    )
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );

                ui.add_space(18.0);
                theme::section_label(ui, "About");
                ui.add_space(6.0);
                ui.label(
                    RichText::new(format!(
                        "Stickie {} · {} · Bevy + egui",
                        env!("CARGO_PKG_VERSION"),
                        view.platform
                    ))
                    .font(theme::font_body())
                    .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(
                        "A lightweight, notes-first replacement for the Steam overlay. \
                         No telemetry, no network, no injection.",
                    )
                    .font(theme::font_small())
                    .color(theme::TEXT_MUTED),
                );
                ui.hyperlink_to(
                    "github.com/afonsolage/stickie-app",
                    "https://github.com/afonsolage/stickie-app",
                );

                ui.add_space(20.0);
                if ui
                    .add(egui::Button::new(
                        RichText::new("Quit Stickie").color(theme::DANGER),
                    ))
                    .clicked()
                {
                    actions.push(UiAction::Quit);
                }
                ui.add_space(12.0);
            });

        notice
    }

    pub fn autosave(&mut self, store: &mut Store) {
        if self
            .config_dirty_since
            .is_some_and(|at| at.elapsed() >= Duration::from_millis(450))
        {
            self.save_config(store);
        }
    }

    pub fn flush(&mut self, store: &mut Store) {
        if self.config_dirty_since.is_some() {
            self.save_config(store);
        }
    }

    fn save_config(&mut self, store: &mut Store) {
        let mut config = store.config().clone();
        config.scrim_opacity = self.scrim_draft.clamp(0.0, MAX_SCRIM_OPACITY);
        config.start_hidden = self.start_hidden_draft;
        if let Err(error) = store.save_config(config) {
            log::error!("failed to save config: {error:#}");
        }
        self.config_dirty_since = None;
    }
}
