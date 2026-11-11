//! Stickies tab: fast, colorful notes that persist with the game workspace.
use bevy_egui::egui;

use std::collections::BTreeSet;
use std::time::{Duration, Instant};

use egui::{RichText, ScrollArea};

use crate::store::{StickyColor as StoreColor, StickyNote, Store};

use super::{UiAction, theme};

/// Stickies-tab local state.
pub struct StickiesTab {
    stickies: Vec<StickyNote>,
    dirty: BTreeSet<String>,
    dirty_since: Option<Instant>,
    pending_delete: Option<String>,
    error: Option<String>,
}

impl Default for StickiesTab {
    fn default() -> Self {
        Self::new()
    }
}

impl StickiesTab {
    pub fn new() -> Self {
        Self {
            stickies: Vec::new(),
            dirty: BTreeSet::new(),
            dirty_since: None,
            pending_delete: None,
            error: None,
        }
    }

    pub fn reload(&mut self, store: &Store) {
        self.stickies = store.stickies().to_vec();
        self.dirty.clear();
        self.dirty_since = None;
        self.pending_delete = None;
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
                "▣",
                "No stickies yet",
                "Focus a game, open the overlay and Stickie will create a private sticky board for it.",
                None,
            );
            return None;
        }

        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.label(
                    RichText::new("Stickies")
                        .font(theme::font_title())
                        .color(theme::TEXT_PRIMARY),
                );
                ui.label(
                    RichText::new(format!(
                        "{} note{} · saved with this game",
                        self.stickies.len(),
                        if self.stickies.len() == 1 { "" } else { "s" }
                    ))
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if theme::primary_button(ui, "+ New sticky").clicked() {
                    match store.create_sticky() {
                        Ok(sticky) => {
                            self.stickies.push(sticky);
                            notice =
                                Some(("New sticky added.".to_string(), theme::ToastKind::Success));
                        }
                        Err(error) => {
                            self.error = Some(format!("Could not create sticky: {error}"));
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

        if self.stickies.is_empty() {
            let action = theme::empty_state(
                ui,
                "▣",
                "A clean board",
                "Quick thoughts, combos, codes, reminder for later. Stickies are for the things \
                 that do not deserve a full note yet.",
                Some("Create the first sticky"),
            );
            if action.is_some_and(|response| response.clicked()) {
                match store.create_sticky() {
                    Ok(sticky) => self.stickies.push(sticky),
                    Err(error) => {
                        self.error = Some(format!("Could not create sticky: {error}"));
                    }
                }
            }
            return notice;
        }

        let cards_area = (ui.available_height() - 4.0).max(180.0);
        ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(cards_area)
            .show(ui, |ui| {
                let gap = 10.0;
                let available = ui.available_width();
                let columns = ((available + gap) / (260.0 + gap)).floor().clamp(1.0, 4.0) as usize;
                let card_width =
                    ((available - gap * (columns - 1) as f32) / columns as f32).clamp(190.0, 340.0);

                let mut index = 0;
                while index < self.stickies.len() {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        for offset in 0..columns {
                            let card_index = index + offset;
                            if card_index >= self.stickies.len() {
                                break;
                            }
                            let mut changed = false;
                            let mut palette_change: Option<StoreColor> = None;
                            let mut delete = false;
                            let sticky = &mut self.stickies[card_index];
                            ui.push_id(&sticky.id, |ui| {
                                let color = theme_color(sticky.color);
                                ui.allocate_ui_with_layout(
                                    egui::vec2(card_width, 194.0),
                                    egui::Layout::top_down(egui::Align::Min),
                                    |ui| {
                                        egui::Frame::NONE
                                            .fill(color.fill())
                                            .stroke(egui::Stroke::new(
                                                1.0,
                                                theme::tint(color.accent(), 90),
                                            ))
                                            .corner_radius(egui::CornerRadius::same(
                                                theme::RADIUS_MD as u8,
                                            ))
                                            .inner_margin(egui::Margin::same(10))
                                            .show(ui, |ui| {
                                                ui.set_width(card_width - 20.0);
                                                ui.set_min_height(174.0);
                                                ui.vertical(|ui| {
                                                    ui.horizontal(|ui| {
                                                        ui.label(
                                                            RichText::new(color.label())
                                                                .font(theme::font_small())
                                                                .color(theme::tint(
                                                                    color.accent(),
                                                                    210,
                                                                )),
                                                        );
                                                        ui.with_layout(
                                                            egui::Layout::right_to_left(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                let remove = ui.add(
                                                                    egui::Button::new(
                                                                        RichText::new("×").color(
                                                                            theme::tint(
                                                                                color.text(),
                                                                                170,
                                                                            ),
                                                                        ),
                                                                    )
                                                                    .frame(false),
                                                                );
                                                                if remove
                                                                    .on_hover_text("Delete sticky")
                                                                    .clicked()
                                                                {
                                                                    delete = true;
                                                                }
                                                            },
                                                        );
                                                    });

                                                    let edit = ui.add(
                                                        egui::TextEdit::multiline(&mut sticky.text)
                                                            .hint_text(
                                                                RichText::new("Write it down…")
                                                                    .color(theme::tint(
                                                                        color.text(),
                                                                        100,
                                                                    )),
                                                            )
                                                            .text_color(color.text())
                                                            .font(theme::font_body())
                                                            .desired_rows(5)
                                                            .desired_width(ui.available_width())
                                                            .frame(egui::Frame::NONE),
                                                    );
                                                    if edit.changed() {
                                                        changed = true;
                                                    }

                                                    ui.add_space(4.0);
                                                    ui.horizontal(|ui| {
                                                        for candidate in StoreColor::ALL {
                                                            let candidate_theme =
                                                                theme_color(candidate);
                                                            let selected =
                                                                sticky.color == candidate;
                                                            let (rect, swatch) = ui
                                                                .allocate_exact_size(
                                                                    egui::vec2(16.0, 16.0),
                                                                    egui::Sense::click(),
                                                                );
                                                            ui.painter().circle_filled(
                                                                rect.center(),
                                                                6.0,
                                                                candidate_theme.accent(),
                                                            );
                                                            if selected {
                                                                ui.painter().circle_stroke(
                                                                    rect.center(),
                                                                    7.5,
                                                                    egui::Stroke::new(
                                                                        1.0,
                                                                        theme::TEXT_PRIMARY,
                                                                    ),
                                                                );
                                                            }
                                                            if swatch
                                                                .on_hover_text(
                                                                    candidate_theme.label(),
                                                                )
                                                                .clicked()
                                                            {
                                                                palette_change = Some(candidate);
                                                            }
                                                        }
                                                    });
                                                });
                                            });
                                    },
                                );
                            });
                            if changed {
                                self.dirty.insert(sticky.id.clone());
                                self.dirty_since = Some(Instant::now());
                            }
                            if let Some(new_color) = palette_change {
                                sticky.color = new_color;
                                self.dirty.insert(sticky.id.clone());
                                self.dirty_since = Some(Instant::now());
                            }
                            if delete {
                                self.pending_delete = Some(sticky.id.clone());
                            }
                        }
                    });
                    index += columns;
                    ui.add_space(gap);
                }
            });

        if let Some(id) = self.pending_delete.take() {
            match store.delete_sticky(&id) {
                Ok(()) => {
                    self.stickies.retain(|sticky| sticky.id != id);
                    self.dirty.remove(&id);
                    notice = Some(("Sticky deleted.".to_string(), theme::ToastKind::Info));
                }
                Err(error) => {
                    self.error = Some(format!("Could not delete sticky: {error}"));
                }
            }
        }

        notice
    }

    pub fn autosave(&mut self, store: &mut Store) {
        if !self.dirty.is_empty()
            && self
                .dirty_since
                .is_some_and(|at| at.elapsed() >= Duration::from_millis(700))
        {
            self.save_dirty(store);
        }
    }

    pub fn flush(&mut self, store: &mut Store) {
        if !self.dirty.is_empty() {
            self.save_dirty(store);
        }
    }

    fn save_dirty(&mut self, store: &mut Store) {
        for id in std::mem::take(&mut self.dirty) {
            let Some(sticky) = self.stickies.iter().find(|sticky| sticky.id == id) else {
                continue;
            };
            if let Err(error) = store.save_sticky(sticky.clone()) {
                self.error = Some(format!("Could not save sticky: {error}"));
                self.dirty.insert(id);
            }
        }
        if self.dirty.is_empty() {
            self.dirty_since = None;
        } else {
            self.dirty_since = Some(Instant::now());
        }
    }
}

fn theme_color(color: StoreColor) -> theme::StickyColor {
    match color {
        StoreColor::Amber => theme::StickyColor::Amber,
        StoreColor::Mint => theme::StickyColor::Mint,
        StoreColor::Sky => theme::StickyColor::Sky,
        StoreColor::Rose => theme::StickyColor::Rose,
        StoreColor::Lilac => theme::StickyColor::Lilac,
        StoreColor::Paper => theme::StickyColor::Paper,
    }
}
