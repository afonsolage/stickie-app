//! Self-contained Markdown notepad state and widget.
//!
//! This module owns no file IO on purpose: the app controller decides when to
//! read a note, when to write it back (debounced autosave), and marks the editor
//! saved with [`MarkdownEditor::mark_saved`]. The widget itself only renders and
//! reports intent through [`MarkdownEditor::save_requested`].
//!
//! ```ignore
//! # use std::path::PathBuf;
//! # use bevy_egui::egui::Ui;
//! # use stickie::ui::editor_md::MarkdownEditor;
//! fn ui(editor: &mut MarkdownEditor, ui: &mut Ui) {
//!     editor.show(ui);
//!     if editor.save_requested() {
//!         // parent writes editor.text() to editor.path(), then:
//!         editor.mark_saved();
//!     }
//! }
//! ```

// Component module: the app may not wire every helper (yet); unused exports are
// intentional public surface for the shell and future command palette.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::time::Instant;

use bevy_egui::egui::text::{CCursor, CCursorRange};
use bevy_egui::egui::{
    self, Align, Id, Key, KeyboardShortcut, Label, Layout, Modifiers, RichText, ScrollArea, Sense,
    Stroke, TextEdit, Ui, Vec2,
};
use egui_commonmark::{CommonMarkCache, CommonMarkViewer};

use super::theme;

/// Editor id salt, combined with the note path so multiple tabs keep separate
/// cursors, scroll state and undo stacks.
fn editor_id(path: &Path) -> Id {
    Id::new(("stickie-markdown-editor", path))
}

// ---------------------------------------------------------------------------
// Pure formatting model
// ---------------------------------------------------------------------------

/// A selection in *character* units (not byte offsets).
///
/// Markdown formatting must not split UTF-8 code points, so all public helpers
/// operate on characters. Converted to bytes only when splicing strings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CharSelection {
    /// Inclusive start character index.
    pub start: usize,
    /// Exclusive end character index.
    pub end: usize,
}

impl CharSelection {
    /// Create a normalized (sorted) selection.
    pub const fn new(start: usize, end: usize) -> Self {
        if start <= end {
            Self { start, end }
        } else {
            Self {
                start: end,
                end: start,
            }
        }
    }

    /// A collapsed cursor.
    pub const fn caret(index: usize) -> Self {
        Self {
            start: index,
            end: index,
        }
    }

    /// Number of selected characters.
    pub const fn len(self) -> usize {
        self.end - self.start
    }

    /// True when nothing is selected.
    pub const fn is_empty(self) -> bool {
        self.start == self.end
    }

    /// Clamp to a buffer containing `char_count` characters.
    pub fn clamped(self, char_count: usize) -> Self {
        Self::new(self.start.min(char_count), self.end.min(char_count))
    }
}

/// Inline and block Markdown commands exposed by the toolbar and shortcuts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FormatAction {
    /// `**bold**`
    Bold,
    /// `*italic*`
    Italic,
    /// `` `inline code` ``
    InlineCode,
    /// `# Heading`
    Heading1,
    /// `## Heading`
    Heading2,
    /// `- bullet`
    BulletList,
    /// `- [ ] task`
    Checklist,
    /// `> quote`
    Quote,
    /// `[label](url)`
    Link,
}

/// Result of applying a [`FormatAction`] to a buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FormatOutcome {
    /// The new buffer text.
    pub text: String,
    /// Where the selection should land after the edit.
    pub selection: CharSelection,
}

/// Apply a formatting command to `text`, editing in character space.
///
/// Toggle behavior: selected text already wrapped in the marker is unwrapped,
/// otherwise it is wrapped. A collapsed selection inserts the markers and puts
/// the caret between them. Block actions add/remove the prefix on every line
/// touched by the selection.
pub fn apply_format(text: &str, selection: CharSelection, action: FormatAction) -> FormatOutcome {
    match action {
        FormatAction::Bold => wrap_selection(text, selection, "**", "**"),
        FormatAction::Italic => wrap_selection(text, selection, "*", "*"),
        FormatAction::InlineCode => wrap_selection(text, selection, "`", "`"),
        FormatAction::Link => link_selection(text, selection),
        FormatAction::Heading1 => prefix_lines(
            text,
            selection,
            LineFormat {
                prefix: "# ",
                strip: &["# "],
                strip_before_add: &["###### ", "##### ", "#### ", "### ", "## ", "# "],
            },
        ),
        FormatAction::Heading2 => prefix_lines(
            text,
            selection,
            LineFormat {
                prefix: "## ",
                strip: &["## "],
                strip_before_add: &["###### ", "##### ", "#### ", "### ", "## "],
            },
        ),
        FormatAction::BulletList => prefix_lines(
            text,
            selection,
            LineFormat {
                prefix: "- ",
                strip: &["- ", "* ", "+ "],
                strip_before_add: &["- ", "* ", "+ "],
            },
        ),
        FormatAction::Checklist => prefix_lines(
            text,
            selection,
            LineFormat {
                prefix: "- [ ] ",
                strip: &["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] "],
                strip_before_add: &["- [ ] ", "- [x] ", "- [X] ", "* [ ] ", "* [x] "],
            },
        ),
        FormatAction::Quote => prefix_lines(
            text,
            selection,
            LineFormat {
                prefix: "> ",
                strip: &["> "],
                strip_before_add: &["> "],
            },
        ),
    }
}

/// Wrap (or unwrap, when already wrapped) the selection with `left`/`right`.
fn wrap_selection(text: &str, selection: CharSelection, left: &str, right: &str) -> FormatOutcome {
    let chars: Vec<char> = text.chars().collect();
    let selection = selection.clamped(chars.len());
    let left_chars: Vec<char> = left.chars().collect();
    let right_chars: Vec<char> = right.chars().collect();

    if !selection.is_empty() {
        let selected = &chars[selection.start..selection.end];

        // Case 1: the selection itself includes the markers.
        let minimum = left_chars.len() + right_chars.len();
        if selected.len() > minimum
            && selected.starts_with(&left_chars)
            && selected.ends_with(&right_chars)
        {
            let inner = &selected[left_chars.len()..selected.len() - right_chars.len()];
            let mut out = chars.clone();
            out.splice(selection.start..selection.end, inner.iter().copied());
            return FormatOutcome {
                text: out.into_iter().collect(),
                selection: CharSelection::new(selection.start, selection.start + inner.len()),
            };
        }

        // Case 2: the markers sit immediately before/after the selection.
        if selection.start >= left_chars.len()
            && selection.end + right_chars.len() <= chars.len()
            && chars[selection.start - left_chars.len()..selection.start] == left_chars[..]
            && chars[selection.end..selection.end + right_chars.len()] == right_chars[..]
        {
            let mut out = chars.clone();
            out.drain(selection.end..selection.end + right_chars.len());
            out.drain(selection.start - left_chars.len()..selection.start);
            let start = selection.start - left_chars.len();
            return FormatOutcome {
                text: out.into_iter().collect(),
                selection: CharSelection::new(start, start + selection.len()),
            };
        }
    }

    // Wrap: insert opening marker before the selection, closing marker after.
    let mut out = chars;
    out.splice(selection.start..selection.start, left_chars.iter().copied());
    let closing_at = selection.end + left_chars.len();
    out.splice(closing_at..closing_at, right_chars.iter().copied());
    FormatOutcome {
        text: out.into_iter().collect(),
        selection: CharSelection::new(
            selection.start + left_chars.len(),
            selection.end + left_chars.len(),
        ),
    }
}

/// Turn the selection into `[label](url)`.
///
/// With a selection the label is kept and the URL placeholder is selected so
/// the user can paste an address immediately. With a caret, an empty link is
/// inserted and the caret lands between the brackets.
fn link_selection(text: &str, selection: CharSelection) -> FormatOutcome {
    let chars: Vec<char> = text.chars().collect();
    let selection = selection.clamped(chars.len());
    let mut out = chars;

    // Toggle off when the selection is exactly `[label](url)`.
    if !selection.is_empty() {
        let selected: String = out[selection.start..selection.end].iter().collect();
        let label = selected
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(')'))
            .and_then(|rest| rest.split_once("]("))
            .map(|(label, _url)| label)
            .filter(|label| !label.is_empty());
        if let Some(label) = label {
            let label: Vec<char> = label.chars().collect();
            let label_len = label.len();
            out.splice(selection.start..selection.end, label);
            return FormatOutcome {
                text: out.into_iter().collect(),
                selection: CharSelection::new(selection.start, selection.start + label_len),
            };
        }
    }

    // Build `[label](url)` with a placeholder URL.
    let label: Vec<char> = out[selection.start..selection.end].to_vec();
    let mut inserted: Vec<char> = vec!['['];
    inserted.extend_from_slice(&label);
    inserted.push(']');
    inserted.push('(');
    let url_start = selection.start + inserted.len();
    inserted.extend("https://".chars());
    inserted.push(')');

    out.splice(selection.start..selection.end, inserted);
    let selection_start = if label.is_empty() {
        // Caret between the brackets for an empty link.
        selection.start + 1
    } else {
        url_start
    };
    let selection_end = if label.is_empty() {
        selection_start
    } else {
        selection_start + "https://".len()
    };
    FormatOutcome {
        text: out.into_iter().collect(),
        selection: CharSelection::new(selection_start, selection_end),
    }
}

/// Parameters for block-level (line prefix) formatting.
#[derive(Clone, Copy)]
struct LineFormat<'a> {
    /// Prefix added when toggling a block on.
    prefix: &'a str,
    /// Prefixes that count as "already formatted" for toggle-off.
    strip: &'a [&'a str],
    /// Prefixes removed before adding a new prefix (e.g. replace `## ` with `# `).
    strip_before_add: &'a [&'a str],
}

/// Add or remove a line prefix across all lines touched by the selection.
fn prefix_lines(text: &str, selection: CharSelection, format: LineFormat<'_>) -> FormatOutcome {
    let chars: Vec<char> = text.chars().collect();
    let selection = selection.clamped(chars.len());
    let spans = line_spans(&chars);
    let first_line = line_index_containing(&spans, selection.start);
    let last_char = if selection.is_empty() {
        selection.start
    } else {
        selection.end - 1
    };
    let last_line = line_index_containing(&spans, last_char);

    let lines: Vec<String> = spans[first_line..=last_line]
        .iter()
        .map(|(start, end)| chars[*start..*end].iter().collect())
        .collect();

    let marked = lines
        .iter()
        .filter(|line| format.strip.iter().any(|prefix| line.starts_with(prefix)))
        .count();
    let non_blank = lines.iter().filter(|line| !line.trim().is_empty()).count();
    let remove = non_blank > 0 && marked == non_blank;

    let mut edits: Vec<(usize, usize, Vec<char>)> = Vec::new();
    for ((start, end), line) in spans[first_line..=last_line].iter().zip(&lines) {
        let replacement = if remove {
            match format
                .strip
                .iter()
                .find(|prefix| line.starts_with(**prefix))
            {
                Some(prefix) => line[prefix.len()..].to_owned(),
                None => line.clone(),
            }
        } else {
            let mut base = line.as_str();
            for prefix in format.strip_before_add {
                if let Some(rest) = base.strip_prefix(prefix) {
                    base = rest;
                    break;
                }
            }
            let mut replacement = String::with_capacity(format.prefix.len() + base.len());
            replacement.push_str(format.prefix);
            replacement.push_str(base);
            replacement
        };
        if replacement != *line {
            edits.push((*start, *end, replacement.chars().collect()));
        }
    }

    let mapped = apply_line_edits(&chars, &edits, selection.start, selection.end);
    let mut outcome = FormatOutcome {
        text: mapped.text,
        selection: mapped.selection,
    };

    // A caret sitting at the start of a line should move after the new prefix so
    // the user can keep typing inside the heading/list item.
    if selection.is_empty() && !remove && selection.start == spans[first_line].0 {
        let prefix_chars = format.prefix.chars().count();
        outcome.selection = CharSelection::caret(mapped.selection.start + prefix_chars);
    }
    outcome
}

/// Apply line edits and translate the selection through them.
///
/// Edits are sorted by buffer position and non-overlapping. Any position at the
/// start of an edited line stays at that line start; positions after it shift by
/// the line's length delta.
fn apply_line_edits(
    chars: &[char],
    edits: &[(usize, usize, Vec<char>)],
    selection_start: usize,
    selection_end: usize,
) -> FormatOutcome {
    let mut out: Vec<char> = chars.to_vec();
    for (start, end, replacement) in edits.iter().rev() {
        out.splice(*start..*end, replacement.iter().copied());
    }

    let map = |position: usize| -> usize {
        let mut delta: isize = 0;
        for (start, end, replacement) in edits {
            let shift = replacement.len().cast_signed() - (*end - *start).cast_signed();
            if position <= *start {
                break;
            }
            if position <= *end {
                // Inside (or at the end of) a rewritten line.
                return (position.cast_signed() + delta + shift).cast_unsigned();
            }
            delta += shift;
        }
        (position.cast_signed() + delta).cast_unsigned()
    };

    let char_count = out.len();
    let start = map(selection_start).min(char_count);
    let end = map(selection_end).min(char_count);
    FormatOutcome {
        text: out.into_iter().collect(),
        selection: CharSelection::new(start, end),
    }
}

/// Character spans (start, end-exclusive) of each line, excluding the newline.
fn line_spans(chars: &[char]) -> Vec<(usize, usize)> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, character) in chars.iter().enumerate() {
        if *character == '\n' {
            spans.push((start, index));
            start = index + 1;
        }
    }
    spans.push((start, chars.len()));
    spans
}

/// Index of the line containing a character position.
fn line_index_containing(spans: &[(usize, usize)], position: usize) -> usize {
    spans
        .iter()
        .rposition(|(start, _)| *start <= position)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// Editor state
// ---------------------------------------------------------------------------

/// Keyboard shortcuts handled by [`MarkdownEditor::show`].
const SHORTCUT_SAVE: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::S);
const SHORTCUT_PREVIEW: KeyboardShortcut =
    KeyboardShortcut::new(Modifiers::COMMAND.plus(Modifiers::SHIFT), Key::P);
const SHORTCUT_BOLD: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::B);
const SHORTCUT_ITALIC: KeyboardShortcut = KeyboardShortcut::new(Modifiers::COMMAND, Key::I);

/// Open Markdown note state plus its egui widget.
///
/// The widget is embeddable: [`MarkdownEditor::show`] takes a `&mut Ui` and
/// renders a complete panel (header, toolbar, editor or preview, status bar).
#[allow(
    clippy::struct_excessive_bools,
    reason = "Editor mode flags are independent UI state, not one enum state machine."
)]
pub struct MarkdownEditor {
    open: bool,
    path: PathBuf,
    title: String,
    buffer: String,
    saved_buffer: String,
    preview: bool,
    focus_requested: bool,
    save_requested: bool,
    last_edit: Option<Instant>,
    cache: CommonMarkCache,
}

impl Default for MarkdownEditor {
    fn default() -> Self {
        Self::new()
    }
}

impl MarkdownEditor {
    /// Create a closed, empty editor.
    pub fn new() -> Self {
        Self {
            open: false,
            path: PathBuf::new(),
            title: String::new(),
            buffer: String::new(),
            saved_buffer: String::new(),
            preview: false,
            focus_requested: false,
            save_requested: false,
            last_edit: None,
            cache: CommonMarkCache::default(),
        }
    }

    /// Open a note with already-loaded contents.
    pub fn open(&mut self, path: PathBuf, title: String, contents: String) {
        self.open = true;
        self.path = path;
        self.title = title;
        self.buffer = contents;
        self.saved_buffer = self.buffer.clone();
        self.preview = false;
        self.focus_requested = true;
        self.save_requested = false;
        self.last_edit = None;
        self.cache = CommonMarkCache::default();
    }

    /// Close the note and release the buffer.
    pub fn close(&mut self) {
        self.open = false;
        self.path.clear();
        self.title.clear();
        self.buffer.clear();
        self.saved_buffer.clear();
        self.preview = false;
        self.focus_requested = false;
        self.save_requested = false;
        self.last_edit = None;
        self.cache = CommonMarkCache::default();
    }

    /// Whether a note is currently open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Path of the open note (empty when closed).
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Human readable note title.
    pub fn title(&self) -> &str {
        &self.title
    }

    /// Current buffer contents.
    pub fn text(&self) -> &str {
        &self.buffer
    }

    /// Mutable buffer, e.g. for appending an inserted screenshot link.
    pub fn text_mut(&mut self) -> &mut String {
        self.last_edit = Some(Instant::now());
        &mut self.buffer
    }

    /// Replace the whole buffer (marks the editor dirty).
    pub fn set_text(&mut self, text: impl Into<String>) {
        self.buffer = text.into();
        self.last_edit = Some(Instant::now());
    }

    /// Whether the buffer differs from the last saved snapshot.
    pub fn is_dirty(&self) -> bool {
        self.buffer != self.saved_buffer
    }

    /// Record that the buffer was written to disk.
    pub fn mark_saved(&mut self) {
        self.saved_buffer.clone_from(&self.buffer);
    }

    /// Consume the pending save request (Ctrl+S or the Save button).
    pub fn save_requested(&mut self) -> bool {
        let requested = self.save_requested;
        self.save_requested = false;
        requested
    }

    /// Request editor focus on the next frame.
    pub fn request_focus(&mut self) {
        self.focus_requested = true;
    }

    /// Ask for a save on the next frame (used by external command palettes).
    pub fn request_save(&mut self) {
        self.save_requested = true;
    }

    /// True when the preview tab is active.
    pub fn is_preview(&self) -> bool {
        self.preview
    }

    /// Switch between edit and preview mode.
    pub fn set_preview(&mut self, preview: bool) {
        self.preview = preview;
    }

    /// Toggle edit/preview mode.
    pub fn toggle_preview(&mut self) {
        self.preview = !self.preview;
    }

    /// Timestamp of the last content edit, for parent-owned debounced autosave.
    pub fn last_edit(&self) -> Option<Instant> {
        self.last_edit
    }

    /// Apply a [`FormatAction`] to the current selection.
    pub fn format(&mut self, ctx: &egui::Context, action: FormatAction) {
        let selection = self.selection(ctx);
        let outcome = apply_format(&self.buffer, selection, action);
        if outcome.text != self.buffer {
            self.buffer = outcome.text;
            self.last_edit = Some(Instant::now());
            self.store_selection(ctx, outcome.selection);
        }
    }

    /// Render the editor panel.
    pub fn show(&mut self, ui: &mut Ui) {
        theme::panel_frame().show(ui, |ui| {
            ui.set_width(ui.available_width());
            if !self.open {
                let _ = theme::empty_state(
                    ui,
                    "🖹",
                    "No note open",
                    "Pick a note from the Files tab, or create one to start writing.",
                    None,
                );
                return;
            }

            self.handle_shortcuts(ui);
            self.header(ui);
            ui.add_space(theme::SPACE_SM);
            self.formatting_toolbar(ui);
            ui.add_space(theme::SPACE_SM);

            let footer_height = 24.0;
            let body_height = (ui.available_height() - footer_height - theme::SPACE_SM).max(140.0);
            if self.preview {
                self.preview_panel(ui, body_height);
            } else {
                self.editor_panel(ui, body_height);
            }
            ui.add_space(theme::SPACE_XS);
            self.status_bar(ui);
        });
    }

    // -- internals ----------------------------------------------------------

    fn handle_shortcuts(&mut self, ui: &mut Ui) {
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_SAVE)) {
            self.save_requested = true;
        }
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_PREVIEW)) {
            self.preview = !self.preview;
        }

        let editing = !self.preview && ui.memory(|memory| memory.has_focus(editor_id(&self.path)));
        if !editing {
            return;
        }
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_BOLD)) {
            let ctx = ui.ctx().clone();
            self.format(&ctx, FormatAction::Bold);
        }
        if ui.input_mut(|input| input.consume_shortcut(&SHORTCUT_ITALIC)) {
            let ctx = ui.ctx().clone();
            self.format(&ctx, FormatAction::Italic);
        }
    }

    fn header(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(
                RichText::new("🖹")
                    .font(theme::font_at(16.0))
                    .color(theme::TEXT_MUTED),
            );
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.horizontal(|ui| {
                    let name = self
                        .path
                        .file_name()
                        .and_then(std::ffi::OsStr::to_str)
                        .filter(|name| !name.is_empty())
                        .unwrap_or(self.title.as_str());
                    ui.add(
                        Label::new(
                            RichText::new(name)
                                .font(theme::font_button())
                                .color(theme::TEXT_PRIMARY),
                        )
                        .truncate(),
                    )
                    .on_hover_text(self.path.display().to_string());
                    if self.is_dirty() {
                        let (dot, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
                        ui.painter().circle_filled(dot.center(), 3.5, theme::ACCENT);
                        ui.label(
                            RichText::new("unsaved")
                                .font(theme::font_small())
                                .color(theme::TEXT_FAINT),
                        );
                    }
                });
                ui.add(
                    Label::new(
                        RichText::new(format!("{} · Markdown", self.title))
                            .font(theme::font_small())
                            .color(theme::TEXT_FAINT),
                    )
                    .truncate(),
                );
            });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if theme::primary_button(ui, "Save").clicked() {
                    self.save_requested = true;
                }
                ui.selectable_value(
                    &mut self.preview,
                    true,
                    RichText::new("Preview").font(theme::font_button()),
                );
                ui.selectable_value(
                    &mut self.preview,
                    false,
                    RichText::new("Edit").font(theme::font_button()),
                );
            });
        });
    }

    fn formatting_toolbar(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.preview {
                ui.label(
                    RichText::new(
                        "Preview mode — checkboxes are interactive, text is rendered read-only.",
                    )
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
                );
                return;
            }

            let buttons: [(FormatAction, &str, &str); 9] = [
                (FormatAction::Bold, "B", "Bold — Ctrl+B"),
                (FormatAction::Italic, "I", "Italic — Ctrl+I"),
                (FormatAction::Heading1, "H1", "Heading 1"),
                (FormatAction::Heading2, "H2", "Heading 2"),
                (FormatAction::BulletList, "•", "Bullet list"),
                (FormatAction::Checklist, "☑", "Checklist"),
                (FormatAction::InlineCode, "`", "Inline code"),
                (FormatAction::Quote, "❯", "Quote"),
                (FormatAction::Link, "🔗", "Link"),
            ];
            for (action, glyph, tooltip) in buttons {
                if theme::tool_button(ui, glyph, tooltip, false).clicked() {
                    let ctx = ui.ctx().clone();
                    self.format(&ctx, action);
                    ui.memory_mut(|memory| memory.request_focus(editor_id(&self.path)));
                }
            }
            ui.separator();
            ui.label(
                RichText::new("Select text, then format. Actions toggle.")
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
            );
        });
    }

    fn editor_panel(&mut self, ui: &mut Ui, height: f32) {
        if self.focus_requested {
            ui.memory_mut(|memory| memory.request_focus(editor_id(&self.path)));
            self.focus_requested = false;
        }
        let frame = egui::Frame::NONE
            .fill(theme::INPUT_BG)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(theme::radius(theme::RADIUS_MD))
            .inner_margin(theme::margin(theme::SPACE_SM));
        frame.show(ui, |ui| {
            ScrollArea::vertical()
                .auto_shrink([false, false])
                .max_height(height)
                .show(ui, |ui| {
                    let output = TextEdit::multiline(&mut self.buffer)
                        .id(editor_id(&self.path))
                        .code_editor()
                        .desired_width(f32::INFINITY)
                        .desired_rows(22)
                        .frame(egui::Frame::NONE)
                        .margin(theme::margin(theme::SPACE_XS))
                        .hint_text(
                            "Start writing.\n\nMarkdown works: # headings, **bold**, - lists, \
                             - [ ] checklists, `code`, > quotes and [links](https://example.com).",
                        )
                        .show(ui);
                    if output.response.response.changed() {
                        self.last_edit = Some(Instant::now());
                    }
                });
        });
    }

    fn preview_panel(&mut self, ui: &mut Ui, height: f32) {
        let frame = egui::Frame::NONE
            .fill(theme::INPUT_BG)
            .stroke(Stroke::new(1.0, theme::BORDER))
            .corner_radius(theme::radius(theme::RADIUS_MD))
            .inner_margin(theme::margin(theme::SPACE_LG));
        frame.show(ui, |ui| {
            ui.style_mut().url_in_tooltip = true;
            ui.style_mut().visuals.code_bg_color = theme::PANEL_BG;
            ui.style_mut().visuals.extreme_bg_color = theme::PANEL_BG;
            // Center a comfortable reading measure on wide overlay windows.
            let width = ui.available_width();
            let column_width = width.min(760.0);
            let offset = (width - column_width) * 0.5;
            ui.horizontal_top(|ui| {
                ui.add_space(offset);
                ui.allocate_ui_with_layout(
                    egui::vec2(column_width, height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ScrollArea::vertical()
                            .auto_shrink([false, false])
                            .max_height(height)
                            .show(ui, |ui| {
                                if self.buffer.trim().is_empty() {
                                    ui.label(
                                        RichText::new("Nothing to preview yet.")
                                            .font(theme::font_body())
                                            .color(theme::TEXT_FAINT),
                                    );
                                } else {
                                    CommonMarkViewer::new().show(ui, &mut self.cache, &self.buffer);
                                }
                            });
                    },
                );
            });
        });
    }

    fn status_bar(&self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            let words = self.buffer.split_whitespace().count();
            let lines = self.buffer.lines().count().max(1);
            let chars = self.buffer.chars().count();
            ui.label(
                RichText::new(format!("{words} words · {lines} lines · {chars} chars"))
                    .font(theme::font_small())
                    .color(theme::TEXT_FAINT),
            );
            if !self.preview {
                let (line, column) = self.cursor_line_column(ui.ctx());
                ui.label(
                    RichText::new(format!("Ln {line}, Col {column}"))
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                // With right-to-left layout the first widget sits at the
                // right edge, so insert in visual reverse order.
                ui.label(
                    RichText::new("preview")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
                theme::kbd(ui, "P");
                ui.label(
                    RichText::new("+")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
                theme::kbd(ui, "Shift");
                ui.label(
                    RichText::new("+")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
                theme::kbd(ui, "Ctrl");
                ui.add_space(theme::SPACE_SM);
                ui.label(
                    RichText::new("save")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
                theme::kbd(ui, "S");
                ui.label(
                    RichText::new("+")
                        .font(theme::font_small())
                        .color(theme::TEXT_FAINT),
                );
                theme::kbd(ui, "Ctrl");
            });
        });
    }

    fn cursor_line_column(&self, ctx: &egui::Context) -> (usize, usize) {
        let selection = self.selection(ctx);
        let mut line = 1;
        let mut column = 1;
        for (index, character) in self.buffer.chars().enumerate() {
            if index >= selection.start {
                break;
            }
            if character == '\n' {
                line += 1;
                column = 1;
            } else {
                column += 1;
            }
        }
        (line, column)
    }

    fn selection(&self, ctx: &egui::Context) -> CharSelection {
        let state = TextEdit::load_state(ctx, editor_id(&self.path));
        state
            .and_then(|state| state.cursor.char_range())
            .map_or_else(
                || CharSelection::caret(self.buffer.chars().count()),
                |range| {
                    let range = range.as_sorted_char_range();
                    CharSelection::new(usize::from(range.start), usize::from(range.end))
                },
            )
    }

    fn store_selection(&self, ctx: &egui::Context, selection: CharSelection) {
        let mut state = TextEdit::load_state(ctx, editor_id(&self.path)).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(
            CCursor::new(selection.start),
            CCursor::new(selection.end),
        )));
        TextEdit::store_state(ctx, editor_id(&self.path), state);
    }
}

/// Convenience: formatted response text for a toolbar label (used by tests and
/// any future command palette).
#[must_use]
pub fn action_label(action: FormatAction) -> &'static str {
    match action {
        FormatAction::Bold => "Bold",
        FormatAction::Italic => "Italic",
        FormatAction::InlineCode => "Inline code",
        FormatAction::Heading1 => "Heading 1",
        FormatAction::Heading2 => "Heading 2",
        FormatAction::BulletList => "Bullet list",
        FormatAction::Checklist => "Checklist",
        FormatAction::Quote => "Quote",
        FormatAction::Link => "Link",
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn outcome(text: &str, start: usize, end: usize, action: FormatAction) -> FormatOutcome {
        apply_format(text, CharSelection::new(start, end), action)
    }

    #[test]
    fn bold_wraps_selection_and_keeps_it() {
        let result = outcome("hello world", 6, 11, FormatAction::Bold);
        assert_eq!(result.text, "hello **world**");
        assert_eq!(result.selection, CharSelection::new(8, 13));
    }

    #[test]
    fn bold_toggles_off_enclosing_markers() {
        let result = outcome("hello **world**", 8, 13, FormatAction::Bold);
        assert_eq!(result.text, "hello world");
        assert_eq!(result.selection, CharSelection::new(6, 11));
    }

    #[test]
    fn bold_toggles_off_when_markers_are_selected() {
        let result = outcome("a **bold** b", 2, 10, FormatAction::Bold);
        assert_eq!(result.text, "a bold b");
        assert_eq!(result.selection, CharSelection::new(2, 6));
    }

    #[test]
    fn bold_with_caret_inserts_markers_and_centers_caret() {
        let result = outcome("hello", 5, 5, FormatAction::Bold);
        assert_eq!(result.text, "hello****");
        assert_eq!(result.selection, CharSelection::caret(7));
    }

    #[test]
    fn italic_uses_single_asterisks() {
        let result = outcome("hi there", 3, 8, FormatAction::Italic);
        assert_eq!(result.text, "hi *there*");
        assert_eq!(result.selection, CharSelection::new(4, 9));
    }

    #[test]
    fn inline_code_wraps_with_backticks() {
        let result = outcome("run cargo test", 4, 14, FormatAction::InlineCode);
        assert_eq!(result.text, "run `cargo test`");
        assert_eq!(result.selection, CharSelection::new(5, 15));
    }

    #[test]
    fn heading_prefixes_each_selected_line() {
        let result = outcome("title\nbody", 0, 10, FormatAction::Heading1);
        assert_eq!(result.text, "# title\n# body");
        assert_eq!(result.selection, CharSelection::new(0, 14));
    }

    #[test]
    fn heading_toggles_off_and_replaces_levels() {
        let off = outcome("# title", 2, 7, FormatAction::Heading1);
        assert_eq!(off.text, "title");
        assert_eq!(off.selection, CharSelection::new(0, 5));

        let replace = outcome("## sub", 0, 6, FormatAction::Heading1);
        assert_eq!(replace.text, "# sub");
    }

    #[test]
    fn caret_at_line_start_moves_after_heading_prefix() {
        let result = outcome("", 0, 0, FormatAction::Heading1);
        assert_eq!(result.text, "# ");
        assert_eq!(result.selection, CharSelection::caret(2));
    }

    #[test]
    fn bullet_list_adds_and_removes_per_line() {
        let added = outcome("a\nb", 0, 3, FormatAction::BulletList);
        assert_eq!(added.text, "- a\n- b");
        assert_eq!(added.selection, CharSelection::new(0, 7));

        let removed = outcome("- a\n- b", 0, 7, FormatAction::BulletList);
        assert_eq!(removed.text, "a\nb");
        assert_eq!(removed.selection, CharSelection::new(0, 3));
    }

    #[test]
    fn checklist_adds_and_removes() {
        let added = outcome("task", 0, 4, FormatAction::Checklist);
        assert_eq!(added.text, "- [ ] task");
        let removed = outcome(
            &added.text,
            0,
            added.text.chars().count(),
            FormatAction::Checklist,
        );
        assert_eq!(removed.text, "task");
    }

    #[test]
    fn quote_prefixes_lines() {
        let result = outcome("stay chill\nkeep going", 0, 21, FormatAction::Quote);
        assert_eq!(result.text, "> stay chill\n> keep going");
        let undone = apply_format(
            &result.text,
            CharSelection::new(0, result.text.chars().count()),
            FormatAction::Quote,
        );
        assert_eq!(undone.text, "stay chill\nkeep going");
    }

    #[test]
    fn link_wraps_selection_and_selects_url() {
        let result = outcome("read the docs", 9, 13, FormatAction::Link);
        assert_eq!(result.text, "read the [docs](https://)");
        assert_eq!(result.selection, CharSelection::new(16, 24));
    }

    #[test]
    fn link_with_caret_puts_caret_between_brackets() {
        let result = outcome("", 0, 0, FormatAction::Link);
        assert_eq!(result.text, "[](https://)");
        assert_eq!(result.selection, CharSelection::caret(1));
    }

    #[test]
    fn link_toggles_off() {
        let result = outcome("[docs](https://example.com)", 0, 27, FormatAction::Link);
        assert_eq!(result.text, "docs");
        assert_eq!(result.selection, CharSelection::new(0, 4));
    }

    #[test]
    fn selection_is_clamped_to_buffer() {
        let result = outcome("ab", 10, 20, FormatAction::Bold);
        assert_eq!(result.text, "ab****");
        assert_eq!(result.selection, CharSelection::caret(4));
    }

    #[test]
    fn unicode_selection_uses_characters_not_bytes() {
        // "héllo wörld" — the accented characters are multi-byte in UTF-8.
        let result = outcome("héllo wörld", 6, 11, FormatAction::Bold);
        assert_eq!(result.text, "héllo **wörld**");
        assert_eq!(result.selection, CharSelection::new(8, 13));
    }

    #[test]
    fn line_spans_handle_trailing_newline() {
        let chars: Vec<char> = "a\nb\n".chars().collect();
        assert_eq!(line_spans(&chars), vec![(0, 1), (2, 3), (4, 4)]);
    }

    #[test]
    fn action_labels_are_stable() {
        assert_eq!(action_label(FormatAction::Bold), "Bold");
        assert_eq!(action_label(FormatAction::Link), "Link");
    }
}
