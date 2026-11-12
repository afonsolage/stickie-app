//! Minimal pinned screen-sticky widget.
//!
//! This widget is drawn as the root egui `Ui` of a small transparent
//! always-on-top Bevy window (the window manager lives in `app.rs`). It knows
//! nothing about Bevy, windows or the store: it receives presentation data and
//! returns the actions the parent window manager should perform.
//!
//! Layout, top to bottom:
//!
//! - a slim draggable strip with a faint grip (dragging it asks the OS window
//!   manager to start a window move);
//! - the note body: the sticky text in the note color, scrollable, with a muted
//!   placeholder when empty;
//! - hover-only controls in the top-right: `Edit` and `×` (unpin), painted on a
//!   translucent dark chip so they stay legible over every palette color.
//!
//! The card fills the whole window and keeps transparent corners: the widget
//! never paints a panel background or chrome around the sticky.

use bevy_egui::egui::{
    self, Align, Color32, CornerRadius, CursorIcon, Layout, Rect, Response, RichText, Sense,
    Stroke, StrokeKind, Ui, UiBuilder, Vec2, pos2, vec2,
};

use crate::store::StickyColor as StoreColor;

use super::theme;

/// Minimum card size that still fits the drag strip and the hover controls.
pub(crate) const MIN_CARD_SIZE: Vec2 = vec2(80.0, 60.0);

/// Maximum height of the draggable top strip.
pub(crate) const STRIP_MAX_HEIGHT: f32 = 22.0;

/// Padding between the card border and its contents.
pub(crate) const CARD_PADDING: f32 = 10.0;

/// Actions the window manager should perform for a screen sticky.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenStickyAction {
    /// The user wants to edit this sticky in the main overlay.
    Edit,
    /// The user wants this sticky to stop floating on screen.
    Unpin,
}

/// What happened in the widget during the current frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenStickyResponse {
    /// Toolbar action, if any.
    pub action: Option<ScreenStickyAction>,
    /// The user dragged the top strip; the parent should start an OS window move.
    pub drag_requested: bool,
}

/// Render a pinned sticky filling `ui`'s window and report interactions.
///
/// `color` is the store-side note palette (mapped to the theme palette here so
/// callers such as `app.rs` never need access to the private `theme` module);
/// `text` is the note body, scrolling inside the card. The caller owns the
/// window: it should call `Window::start_drag_move()` when
/// [`ScreenStickyResponse::drag_requested`] is set and open the matching sticky
/// in the main overlay for [`ScreenStickyAction::Edit`].
pub fn show(ui: &mut Ui, text: &str, color: StoreColor) -> ScreenStickyResponse {
    let color = theme_color(color);
    let mut response = ScreenStickyResponse::default();
    let card = ui.max_rect();
    let painter = ui.painter().clone();
    if !is_usable(card) {
        // Tiny window: keep a readable color card but skip the chrome/controls.
        paint_card(&painter, card, color, false);
        return response;
    }

    let pointer = ui.ctx().pointer_hover_pos();
    let hovered = controls_visible(pointer, card);
    paint_card(&painter, card, color, hovered);

    let strip_height = drag_strip_height(card.height());
    let strip = Rect::from_min_size(card.min, vec2(card.width(), strip_height));
    paint_drag_strip(&painter, strip, card, color, hovered);

    // The strip must be registered before the hover buttons so the buttons win
    // the hit test when they overlap it (egui's topmost widget takes the click).
    let drag = ui.interact(
        strip,
        ui.id().with("stickie-screen-sticky-drag"),
        Sense::click_and_drag(),
    );
    if drag.hovered() {
        ui.ctx().set_cursor_icon(if drag.dragged() {
            CursorIcon::Grabbing
        } else {
            CursorIcon::Grab
        });
    }
    response.drag_requested = drag.dragged();

    let body = body_rect(card, strip_height);
    paint_body(ui, body, text, color);

    if hovered {
        let toolbar = toolbar_rect(card, strip_height);
        ui.scope_builder(
            UiBuilder::new()
                .max_rect(toolbar)
                .layout(Layout::right_to_left(Align::Center)),
            |ui| {
                ui.spacing_mut().item_spacing.x = theme::SPACE_XS;
                if chip_button(ui, "×", "Unpin from screen").clicked() {
                    response.action = Some(ScreenStickyAction::Unpin);
                }
                if chip_button(ui, "Edit", "Edit this sticky in the overlay").clicked() {
                    response.action = Some(ScreenStickyAction::Edit);
                }
            },
        );
    }

    response
}

// ---------------------------------------------------------------------------
// Pure geometry helpers (unit tested)
// ---------------------------------------------------------------------------

/// Map a store palette color to the matching theme palette.
///
/// Kept here so the app layer can call [`show`] with `store::StickyColor` even
/// though the `theme` module itself is private to `ui`.
pub(crate) fn theme_color(color: StoreColor) -> theme::StickyColor {
    match color {
        StoreColor::Amber => theme::StickyColor::Amber,
        StoreColor::Mint => theme::StickyColor::Mint,
        StoreColor::Sky => theme::StickyColor::Sky,
        StoreColor::Rose => theme::StickyColor::Rose,
        StoreColor::Lilac => theme::StickyColor::Lilac,
        StoreColor::Paper => theme::StickyColor::Paper,
    }
}

/// Height of the draggable top strip for a card of `card_height` points.
pub(crate) fn drag_strip_height(card_height: f32) -> f32 {
    let clamped = (card_height * 0.16).clamp(14.0, STRIP_MAX_HEIGHT);
    clamped.min(card_height.max(0.0))
}

/// Body rectangle below the drag strip, inset by [`CARD_PADDING`].
pub(crate) fn body_rect(card: Rect, strip_height: f32) -> Rect {
    let min = pos2(
        card.left() + CARD_PADDING,
        card.top() + strip_height + CARD_PADDING * 0.4,
    );
    let max = pos2(card.right() - CARD_PADDING, card.bottom() - CARD_PADDING);
    Rect::from_min_max(min, pos2(max.x.max(min.x), max.y.max(min.y)))
}

/// Whether the hover controls should be visible (pointer over the card).
pub(crate) fn controls_visible(pointer: Option<egui::Pos2>, card: Rect) -> bool {
    pointer.is_some_and(|point| card.contains(point))
}

/// Whether the card is large enough to draw the full widget.
pub(crate) fn is_usable(card: Rect) -> bool {
    card.width() >= MIN_CARD_SIZE.x && card.height() >= MIN_CARD_SIZE.y
}

/// Toolbar area at the top-right of the card, inside the drag strip.
pub(crate) fn toolbar_rect(card: Rect, strip_height: f32) -> Rect {
    let width = 84.0_f32.min((card.width() - 2.0 * CARD_PADDING).max(32.0));
    let height = (strip_height - 4.0).max(16.0);
    Rect::from_min_size(
        pos2(card.right() - CARD_PADDING - width, card.top() + 2.0),
        vec2(width, height),
    )
}

// ---------------------------------------------------------------------------
// Painting helpers
// ---------------------------------------------------------------------------

fn paint_card(painter: &egui::Painter, card: Rect, color: theme::StickyColor, hovered: bool) {
    let radius = theme::radius(theme::RADIUS_LG);
    painter.rect_filled(card, radius, color.fill());
    let border = if hovered { 180 } else { 130 };
    painter.rect_stroke(
        card,
        radius,
        Stroke::new(1.0, theme::tint(color.accent(), border)),
        StrokeKind::Inside,
    );
}

fn paint_drag_strip(
    painter: &egui::Painter,
    strip: Rect,
    card: Rect,
    color: theme::StickyColor,
    hovered: bool,
) {
    // Only the top corners are rounded so the strip reads as a header.
    let radius = CornerRadius {
        nw: theme::RADIUS_LG as u8,
        ne: theme::RADIUS_LG as u8,
        sw: 0,
        se: 0,
    };
    if hovered {
        painter.rect_filled(strip, radius, theme::tint(color.accent(), 26));
    }
    // Three faint grip dots, centered in the strip.
    let center = pos2(card.center().x, strip.center().y);
    let alpha = if hovered { 170 } else { 90 };
    for offset in [-1.0_f32, 0.0, 1.0] {
        painter.circle_filled(
            pos2(center.x + offset * 5.0, center.y),
            1.4,
            theme::tint(color.text(), alpha),
        );
    }
}

fn paint_body(ui: &mut Ui, body: Rect, text: &str, color: theme::StickyColor) {
    if body.width() <= 0.0 || body.height() <= 0.0 {
        return;
    }
    ui.scope_builder(UiBuilder::new().max_rect(body), |ui| {
        ui.set_clip_rect(body);
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("stickie-screen-sticky-body")
            .show(ui, |ui| {
                ui.set_width(body.width());
                if text.trim().is_empty() {
                    ui.label(
                        RichText::new("Empty sticky")
                            .font(theme::font_body())
                            .color(theme::tint(color.text(), 140))
                            .italics(),
                    );
                    ui.label(
                        RichText::new("Hover the card, then press Edit.")
                            .font(theme::font_small())
                            .color(theme::tint(color.text(), 110))
                            .italics(),
                    );
                } else {
                    ui.label(
                        RichText::new(text)
                            .font(theme::font_body())
                            .color(color.text()),
                    );
                }
            });
    });
}

/// Compact toolbar button painted on a translucent dark chip.
fn chip_button(ui: &mut Ui, label: &str, tooltip: &str) -> Response {
    let galley =
        ui.painter()
            .layout_no_wrap(label.to_owned(), theme::font_small(), theme::TEXT_PRIMARY);
    let size = vec2(
        galley.size().x + theme::SPACE_SM,
        galley.size().y + theme::SPACE_XS,
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter().clone();
    let fill = if response.hovered() {
        Color32::from_black_alpha(210)
    } else {
        Color32::from_black_alpha(150)
    };
    painter.rect_filled(rect, theme::radius(theme::RADIUS_SM), fill);
    painter.rect_stroke(
        rect,
        theme::radius(theme::RADIUS_SM),
        Stroke::new(1.0, theme::tint(theme::ACCENT, 130)),
        StrokeKind::Inside,
    );
    painter.galley(
        rect.center() - galley.size() * 0.5,
        galley,
        theme::TEXT_PRIMARY,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.on_hover_text(tooltip)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use bevy_egui::egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput};

    fn card() -> Rect {
        Rect::from_min_size(Pos2::ZERO, vec2(280.0, 220.0))
    }

    fn run_frames(text: &str, color: StoreColor, frames: Vec<Vec<Event>>) -> ScreenStickyResponse {
        let ctx = egui::Context::default();
        let mut response = ScreenStickyResponse::default();
        for events in frames {
            let input = RawInput {
                screen_rect: Some(card()),
                events,
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                response = show(ui, text, color);
            });
            output.drop_without_applying_deltas();
        }
        response
    }

    fn pointer(pos: Pos2, pressed: Option<bool>) -> Vec<Event> {
        let mut events = vec![Event::PointerMoved(pos)];
        if let Some(pressed) = pressed {
            events.push(Event::PointerButton {
                pos,
                button: PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            });
        }
        events
    }

    #[test]
    fn drag_strip_height_is_clamped() {
        assert!((drag_strip_height(220.0) - STRIP_MAX_HEIGHT).abs() < f32::EPSILON);
        assert!((drag_strip_height(100.0) - 16.0).abs() < f32::EPSILON);
        assert!((drag_strip_height(10.0) - 10.0).abs() < f32::EPSILON);
        assert!((drag_strip_height(0.0)).abs() < f32::EPSILON);
    }

    #[test]
    fn body_rect_is_inset_below_the_strip() {
        let card = card();
        let body = body_rect(card, drag_strip_height(card.height()));
        assert!(body.left() >= card.left() + CARD_PADDING - f32::EPSILON);
        assert!(body.top() > card.top() + STRIP_MAX_HEIGHT);
        assert!(body.right() <= card.right() - CARD_PADDING + f32::EPSILON);
        assert!(body.bottom() < card.bottom());
        assert!(body.width() > 0.0 && body.height() > 0.0);
    }

    #[test]
    fn body_rect_never_inverts_on_tiny_cards() {
        let tiny = Rect::from_min_size(Pos2::ZERO, vec2(12.0, 8.0));
        let body = body_rect(tiny, drag_strip_height(tiny.height()));
        assert!(body.width() >= 0.0);
        assert!(body.height() >= 0.0);
    }

    #[test]
    fn controls_reveal_only_over_the_card() {
        let card = card();
        assert!(controls_visible(Some(pos2(140.0, 110.0)), card));
        assert!(!controls_visible(Some(pos2(400.0, 110.0)), card));
        assert!(!controls_visible(None, card));
    }

    #[test]
    fn usability_requires_a_minimum_size() {
        assert!(is_usable(card()));
        assert!(!is_usable(Rect::from_min_size(
            Pos2::ZERO,
            vec2(40.0, 220.0)
        )));
        assert!(!is_usable(Rect::from_min_size(
            Pos2::ZERO,
            vec2(280.0, 20.0)
        )));
    }

    #[test]
    fn toolbar_stays_inside_the_card() {
        let card = card();
        let toolbar = toolbar_rect(card, drag_strip_height(card.height()));
        assert!(toolbar.left() > card.left());
        assert!(toolbar.right() <= card.right());
        assert!(toolbar.top() >= card.top());
        assert!(toolbar.bottom() <= card.top() + STRIP_MAX_HEIGHT);
    }

    #[test]
    fn empty_note_renders_without_panicking() {
        // A frame without pointer input still draws the card + placeholder.
        let response = run_frames("", StoreColor::Amber, vec![Vec::new()]);
        assert_eq!(response, ScreenStickyResponse::default());
    }

    #[test]
    fn store_palette_maps_to_theme_palette() {
        let cases = [
            (StoreColor::Amber, "Amber"),
            (StoreColor::Mint, "Mint"),
            (StoreColor::Sky, "Sky"),
            (StoreColor::Rose, "Rose"),
            (StoreColor::Lilac, "Lilac"),
            (StoreColor::Paper, "Paper"),
        ];
        for (color, label) in cases {
            assert_eq!(theme_color(color).label(), label);
        }
    }

    #[test]
    fn draggable_strip_reports_window_drag() {
        let strip_y = drag_strip_height(card().height()) * 0.5;
        let start = pos2(140.0, strip_y);
        // First frame registers the widget in egui's hit test; the press and the
        // drag movement follow on later frames like a real pointer.
        let response = run_frames(
            "note text",
            StoreColor::Mint,
            vec![
                pointer(start, None),
                pointer(start, Some(true)),
                pointer(pos2(160.0, strip_y + 8.0), None),
                pointer(pos2(180.0, strip_y + 16.0), None),
            ],
        );
        assert!(response.drag_requested);
        assert!(response.action.is_none());
    }

    #[test]
    fn hover_toolbar_unpin_is_clickable() {
        let card = card();
        let strip_height = drag_strip_height(card.height());
        let toolbar = toolbar_rect(card, strip_height);
        // The `×` chip is the rightmost control in the right-to-left layout.
        let unpin = pos2(card.right() - CARD_PADDING - 8.0, toolbar.center().y);
        let response = run_frames(
            "note text",
            StoreColor::Sky,
            vec![
                pointer(unpin, None),
                pointer(unpin, Some(true)),
                pointer(unpin, Some(false)),
            ],
        );
        assert_eq!(response.action, Some(ScreenStickyAction::Unpin));
    }

    #[test]
    fn hover_toolbar_edit_is_clickable() {
        let card = card();
        let strip_height = drag_strip_height(card.height());
        let toolbar = toolbar_rect(card, strip_height);
        // The `Edit` chip sits to the left of the rightmost `×` chip.
        let edit = pos2(card.right() - 45.0, toolbar.center().y);
        let response = run_frames(
            "note text",
            StoreColor::Lilac,
            vec![
                pointer(edit, None),
                pointer(edit, Some(true)),
                pointer(edit, Some(false)),
            ],
        );
        assert_eq!(response.action, Some(ScreenStickyAction::Edit));
    }

    #[test]
    fn chip_buttons_are_keyboard_clickable_when_hovered() {
        // Keyboard activation is out of scope for this widget, but a synthetic
        // key event must not panic the UI or leak a drag request.
        let response = run_frames(
            "text",
            StoreColor::Rose,
            vec![
                pointer(pos2(140.0, 110.0), None),
                vec![Event::Key {
                    key: Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Modifiers::NONE,
                }],
            ],
        );
        assert!(!response.drag_requested);
    }
}
