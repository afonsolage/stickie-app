//! Files tab: browse the workspace `Files/` tree, create folders and reveal
//! files in the OS file manager. The last open folder is persisted per game.
use bevy_egui::egui;

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use egui::{RichText, ScrollArea};

use crate::store::{FileEntry, Store};

use super::{UiAction, theme};

/// Files-tab local state.
pub struct FilesTab {
    cwd: String,
    entries: Vec<FileEntry>,
    new_folder_open: bool,
    new_folder_name: String,
    error: Option<String>,
}

impl Default for FilesTab {
    fn default() -> Self {
        Self::new()
    }
}

impl FilesTab {
    pub fn new() -> Self {
        Self {
            cwd: String::new(),
            entries: Vec::new(),
            new_folder_open: false,
            new_folder_name: String::new(),
            error: None,
        }
    }

    pub fn reload(&mut self, store: &Store) {
        self.cwd = store.last_folder().unwrap_or_default().to_string();
        self.refresh(store);
    }

    fn refresh(&mut self, store: &Store) {
        match store.list_folder(&self.cwd) {
            Ok(entries) => {
                self.entries = entries;
                self.error = None;
            }
            Err(error) => {
                // A stale last-folder (deleted/replaced) should not dead-end the
                // user; fall back to the Files root once.
                if !self.cwd.is_empty() {
                    self.cwd.clear();
                    if let Ok(entries) = store.list_folder("") {
                        self.entries = entries;
                        self.error = Some(format!(
                            "The last folder is unavailable ({error}). Opened Files instead."
                        ));
                        return;
                    }
                }
                self.entries.clear();
                self.error = Some(format!("Could not read folder: {error}"));
            }
        }
    }

    fn navigate(&mut self, store: &mut Store, relative: String) {
        self.cwd = relative;
        if let Err(error) = store.set_last_folder(&self.cwd) {
            log::warn!("could not persist last folder: {error:#}");
        }
        self.refresh(store);
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        store: &mut Store,
        actions: &mut Vec<UiAction>,
    ) -> Option<(String, theme::ToastKind)> {
        if store.workspace().is_none() {
            theme::empty_state(
                ui,
                "F",
                "No workspace yet",
                "Open the overlay while a game is focused and Stickie will create a workspace for it.",
                None,
            );
            return None;
        }

        let mut notice = None;
        super::tab_header(
            ui,
            "Files",
            Some("Game saves, screenshots, whatever you need to find later."),
        );
        ui.add_space(4.0);

        let mut breadcrumb_target: Option<String> = None;
        ui.horizontal(|ui| {
            let can_go_up = !self.cwd.is_empty();
            if ui.add_enabled(can_go_up, egui::Button::new("Up")).clicked() {
                let parent = PathBuf::from(&self.cwd)
                    .parent()
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                self.navigate(store, parent.replace('\\', "/"));
            }

            breadcrumb_target = Self::breadcrumb(ui, &self.cwd);

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Open in Explorer").clicked()
                    && let Some(root) = store.files_dir()
                {
                    let path = if self.cwd.is_empty() {
                        root.to_path_buf()
                    } else {
                        root.join(&self.cwd)
                    };
                    actions.push(UiAction::OpenPath(path));
                }
                if ui.button("New folder").clicked() {
                    self.new_folder_open = !self.new_folder_open;
                    self.new_folder_name.clear();
                }
                if ui.button("Refresh").clicked() {
                    self.refresh(store);
                }
            });
        });

        if let Some(target) = breadcrumb_target {
            self.navigate(store, target);
        }

        if self.new_folder_open {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.new_folder_name)
                        .hint_text("Folder name")
                        .desired_width(240.0),
                );
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let create = theme::primary_button(ui, "Create").clicked() || enter;
                if create {
                    let name = self.new_folder_name.trim().to_string();
                    if name.is_empty() {
                        notice = Some((
                            "Folder name cannot be empty.".to_string(),
                            theme::ToastKind::Error,
                        ));
                    } else {
                        match store.create_folder(&self.cwd, &name) {
                            Ok(entry) => {
                                self.new_folder_open = false;
                                self.new_folder_name.clear();
                                self.refresh(store);
                                notice = Some((
                                    format!("Created folder “{}”.", entry.name),
                                    theme::ToastKind::Success,
                                ));
                            }
                            Err(error) => {
                                notice = Some((
                                    format!("Could not create folder: {error}"),
                                    theme::ToastKind::Error,
                                ));
                            }
                        }
                    }
                }
                if ui.button("Cancel").clicked() {
                    self.new_folder_open = false;
                    self.new_folder_name.clear();
                }
                response.request_focus();
            });
        }

        if let Some(error) = &self.error {
            ui.add_space(6.0);
            ui.label(
                RichText::new(error)
                    .font(theme::font_small())
                    .color(theme::DANGER),
            );
        }

        ui.add_space(8.0);
        let panel_height = (ui.available_height() - 4.0).max(120.0);
        egui::Frame::NONE
            .fill(theme::INPUT_BG)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS_MD as u8))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_min_height(panel_height);
                if self.entries.is_empty() {
                    if theme::empty_state(
                        ui,
                        "F",
                        "This folder is empty",
                        "Create a folder to start sorting saves, builds or screenshots.",
                        Some("New folder"),
                    )
                    .is_some_and(|response| response.clicked())
                    {
                        self.new_folder_open = true;
                        self.new_folder_name.clear();
                    }
                } else {
                    ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            let entries = self.entries.clone();
                            for entry in entries {
                                let response = self.file_row(ui, &entry);
                                if response.clicked() {
                                    if entry.is_dir {
                                        let relative =
                                            entry.relative.to_string_lossy().replace('\\', "/");
                                        self.navigate(store, relative);
                                    } else {
                                        actions.push(UiAction::OpenPath(entry.path.clone()));
                                    }
                                }
                            }
                        });
                }
            });

        notice
    }

    fn breadcrumb(ui: &mut egui::Ui, cwd: &str) -> Option<String> {
        let components = cwd
            .split('/')
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let mut target = None;
        if ui.link("Files").clicked() && !components.is_empty() {
            target = Some(String::new());
        }
        for (index, component) in components.iter().enumerate() {
            ui.label(RichText::new("/").color(theme::TEXT_FAINT));
            if index + 1 == components.len() {
                ui.label(
                    RichText::new(*component)
                        .font(theme::font_body())
                        .color(theme::TEXT_PRIMARY),
                );
            } else if ui.link(*component).clicked() {
                target = Some(components[..=index].join("/"));
            }
        }
        target
    }

    fn file_row(&self, ui: &mut egui::Ui, entry: &FileEntry) -> egui::Response {
        let row_height = 30.0;
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), row_height),
            egui::Sense::click(),
        );
        if response.hovered() {
            ui.painter()
                .rect_filled(rect, egui::CornerRadius::same(4), theme::hover_fill());
        }
        let icon = if entry.is_dir { "▸" } else { "·" };
        let icon_color = if entry.is_dir {
            theme::ACCENT
        } else {
            theme::TEXT_FAINT
        };
        ui.painter().text(
            rect.left_center() + egui::vec2(10.0, 0.0),
            egui::Align2::LEFT_CENTER,
            icon,
            theme::font_body(),
            icon_color,
        );
        ui.painter().text(
            rect.left_center() + egui::vec2(30.0, 0.0),
            egui::Align2::LEFT_CENTER,
            &entry.name,
            theme::font_body(),
            if entry.is_dir {
                theme::TEXT_PRIMARY
            } else {
                theme::TEXT_MUTED
            },
        );
        let meta = if entry.is_dir {
            "folder".to_string()
        } else {
            format!(
                "{} · {}",
                human_size(entry.path.metadata().map(|meta| meta.len()).unwrap_or(0)),
                relative_time(entry.modified_secs)
            )
        };
        ui.painter().text(
            rect.right_center() - egui::vec2(10.0, 0.0),
            egui::Align2::RIGHT_CENTER,
            meta,
            theme::font_small(),
            theme::TEXT_FAINT,
        );
        response
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn relative_time(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return "unknown".to_string();
    }
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let age = now.saturating_sub(unix_secs);
    match age {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", age / 60),
        3600..=86399 => format!("{}h ago", age / 3600),
        86400..=2_592_000 => format!("{}d ago", age / 86400),
        _ => "a while ago".to_string(),
    }
}
