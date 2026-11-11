//! Notes tab: a Markdown-first notepad with a file list per game workspace.
//!
//! The files are ordinary `.md` files on disk. The editor is intentionally
//! simple: a toolbar, a plain text editing surface and a rendered preview.
use bevy_egui::egui;

use std::path::Path;
use std::time::{Duration, Instant};

use egui::{RichText, ScrollArea};

use crate::store::{NoteEntry, Store};

use super::editor_md::MarkdownEditor;
use super::{UiAction, theme};

/// Notes-tab local state.
pub struct NotesTab {
    notes: Vec<NoteEntry>,
    selected: Option<NoteEntry>,
    editor: MarkdownEditor,
    dirty_since: Option<Instant>,
    error: Option<String>,
}

impl Default for NotesTab {
    fn default() -> Self {
        Self::new()
    }
}

impl NotesTab {
    pub fn new() -> Self {
        Self {
            notes: Vec::new(),
            selected: None,
            editor: MarkdownEditor::new(),
            dirty_since: None,
            error: None,
        }
    }

    pub fn reload(&mut self, store: &Store) {
        self.notes = store.list_notes().unwrap_or_default();
        self.selected = None;
        self.editor = MarkdownEditor::new();
        self.dirty_since = None;
        self.error = None;
    }

    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        store: &mut Store,
        _actions: &mut Vec<UiAction>,
    ) -> Option<(String, theme::ToastKind)> {
        let mut notice = None;
        if store.workspace().is_none() {
            theme::empty_state(
                ui,
                "🗋",
                "No workspace yet",
                "Stickie creates a private note space per game. Open the overlay while a game is \
                 in front to get started.",
                None,
            );
            return None;
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Notes")
                        .font(theme::font_title())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(format!(
                        "{} markdown file{} · plain .md on disk",
                        self.notes.len(),
                        if self.notes.len() == 1 { "" } else { "s" }
                    ))
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "+ New note").clicked() {
                    self.flush(store);
                    match store.create_note() {
                        Ok(note) => {
                            if let Err(error) = self.open_note(store, &note) {
                                self.error = Some(format!("Could not open note: {error}"));
                            } else {
                                self.refresh(store);
                                notice = Some((
                                    "New Markdown note created.".to_string(),
                                    theme::ToastKind::Success,
                                ));
                            }
                        }
                        Err(error) => {
                            self.error = Some(format!("Could not create note: {error}"));
                        }
                    }
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

        let body_rect = egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(
                ui.available_width(),
                (ui.available_height() - 2.0).max(160.0),
            ),
        );
        let list_width = 226.0;
        let list_rect =
            egui::Rect::from_min_size(body_rect.min, egui::vec2(list_width, body_rect.height()));
        let editor_rect = egui::Rect::from_min_max(
            egui::pos2(list_rect.right() + 12.0, body_rect.top()),
            body_rect.max,
        );

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(list_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| self.show_note_list(ui, store),
        );

        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(editor_rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                if !self.editor.is_open() {
                    if theme::empty_state(
                        ui,
                        "🗋",
                        "Pick a note, or start a new one",
                        "Notes are Markdown, with a live preview and no lock-in. Everything stays \
                         in this game's workspace folder.",
                        Some("New note"),
                    )
                    .is_some_and(|response| response.clicked())
                    {
                        self.flush(store);
                        if let Ok(note) = store.create_note() {
                            if let Err(error) = self.open_note(store, &note) {
                                self.error = Some(format!("Could not open note: {error}"));
                            } else {
                                self.refresh(store);
                            }
                        }
                    }
                } else {
                    let height = (ui.available_height() - 38.0).max(180.0);
                    ui.scope_builder(
                        egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            ui.cursor().min,
                            egui::vec2(ui.available_width(), height),
                        )),
                        |ui| self.editor.show(ui),
                    );
                }
            },
        );

        notice
    }

    fn show_note_list(&mut self, ui: &mut egui::Ui, store: &mut Store) {
        egui::Frame::NONE
            .fill(theme::PANEL_BG_RAISED)
            .stroke(egui::Stroke::new(1.0, theme::BORDER))
            .corner_radius(egui::CornerRadius::same(theme::RADIUS_MD as u8))
            .inner_margin(egui::Margin::same(6))
            .show(ui, |ui| {
                ui.set_min_height(ui.available_height());
                ui.set_width(ui.available_width());
                if self.notes.is_empty() {
                    ui.add_space(24.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new("No notes yet")
                                .font(theme::font_body())
                                .color(theme::TEXT_MUTED),
                        );
                        ui.label(
                            RichText::new("Your first note is one click away.")
                                .font(theme::font_small())
                                .color(theme::TEXT_FAINT),
                        );
                    });
                    return;
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        for note in self.notes.clone() {
                            let selected = self
                                .selected
                                .as_ref()
                                .is_some_and(|current| current.path == note.path);
                            let response = self.note_row(ui, &note, selected);
                            if response.clicked() && !selected {
                                self.flush(store);
                                if let Err(error) = self.open_note(store, &note) {
                                    self.error = Some(format!("Could not open note: {error}"));
                                }
                            }
                        }
                    });
            });
    }

    fn note_row(&self, ui: &mut egui::Ui, note: &NoteEntry, selected: bool) -> egui::Response {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 48.0), egui::Sense::click());
        if selected {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(theme::RADIUS_SM as u8),
                theme::tint(theme::ACCENT, 26),
            );
        } else if response.hovered() {
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius::same(theme::RADIUS_SM as u8),
                theme::hover_fill(),
            );
        }
        ui.painter().text(
            rect.left_top() + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            truncate_chars(&note.title, 30),
            theme::font_body(),
            if selected {
                theme::ACCENT
            } else {
                theme::TEXT_PRIMARY
            },
        );
        ui.painter().text(
            rect.left_bottom() + egui::vec2(10.0, -9.0),
            egui::Align2::LEFT_BOTTOM,
            format!("{} · Markdown", relative_time(note.modified_secs)),
            theme::font_small(),
            theme::TEXT_FAINT,
        );
        response
    }

    fn open_note(&mut self, store: &Store, note: &NoteEntry) -> anyhow::Result<()> {
        let contents = store.read_note(note)?;
        self.editor
            .open(note.path.clone(), note.title.clone(), contents);
        self.selected = Some(note.clone());
        self.dirty_since = None;
        self.error = None;
        Ok(())
    }

    fn refresh(&mut self, store: &Store) {
        self.notes = store.list_notes().unwrap_or_default();
        self.dirty_since = None;
    }

    /// Saves the open note immediately if it has unsaved edits.
    pub fn flush(&mut self, store: &mut Store) {
        self.save_if_needed(store);
    }

    /// Autosaves after a short idle window and honours Ctrl/Cmd+S.
    pub fn autosave(&mut self, store: &mut Store) {
        if !self.editor.is_open() {
            self.dirty_since = None;
            return;
        }
        if self.editor.save_requested() {
            self.save_if_needed(store);
            return;
        }
        if !self.editor.is_dirty() {
            self.dirty_since = None;
            return;
        }
        match self.dirty_since {
            Some(at) if at.elapsed() >= Duration::from_millis(900) => self.save_if_needed(store),
            Some(_) => {}
            None => self.dirty_since = Some(Instant::now()),
        }
    }

    fn save_if_needed(&mut self, store: &mut Store) {
        if !self.editor.is_open() || !self.editor.is_dirty() {
            return;
        }
        let Some(note) = self.selected.clone() else {
            return;
        };
        match store.write_note(&note, self.editor.text()) {
            Ok(()) => {
                self.editor.mark_saved();
                self.dirty_since = None;
                self.notes = store.list_notes().unwrap_or_default();
                self.error = None;
            }
            Err(error) => {
                self.error = Some(format!("Could not save note: {error}"));
                // Retry on the next autosave tick rather than losing the text.
                self.dirty_since = Some(Instant::now());
            }
        }
    }

    /// Inserts a screenshot reference into the open note (used by the Shots tab).
    /// Returns `false` when no note is open, so the shell can explain why.
    pub fn attach_screenshot(&mut self, path: &Path, store: &Store) -> bool {
        if !self.editor.is_open() {
            return false;
        }
        let relative = store
            .workspace()
            .and_then(|workspace| path.strip_prefix(workspace.root()).ok())
            .map(|relative| relative.to_string_lossy().replace('\\', "/"));
        let link = relative
            .map(|relative| format!("../{relative}"))
            .unwrap_or_else(|| path.display().to_string().replace('\\', "/"));
        self.editor
            .text_mut()
            .push_str(&format!("\n\n![Screenshot]({link})\n"));
        self.dirty_since = Some(Instant::now());
        true
    }
}

fn relative_time(unix_secs: u64) -> String {
    if unix_secs == 0 {
        return "unknown".into();
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
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

fn truncate_chars(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(max_chars).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}
