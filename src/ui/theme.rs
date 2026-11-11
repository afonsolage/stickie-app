//! Stickie design system.
//!
//! A deliberately small, opinionated dark theme for a transparent in-game
//! overlay. The goals, in order:
//!
//! 1. **Legibility over a busy game scene.** Text/background pairs are checked
//!    against WCAG contrast ratios (see the tests at the bottom of this file).
//! 2. **Quiet styling.** Low-saturation surfaces, hairline borders, a single
//!    warm accent (amber) instead of neon chrome. The accent is bright enough
//!    to read over gameplay without the eye strain of saturated lime/cyan.
//! 3. **Compact widgets.** The overlay should feel like a tool, not a website:
//!    tight spacing, 28-32 px interactive rows, no heavy shadows or gradients.
//!
//! Everything here is painted with egui primitives or plain unicode glyphs; no
//! font assets or image assets are required. The widgets accept `&mut Ui` so
//! they can be embedded in any root viewport `Ui` (egui 0.36 removed the
//! `CentralPanel::show(ctx)` entry point).

// The design system intentionally exposes a complete widget kit; individual
// widgets are adopted by screens over time, so unused exports are expected.
#![allow(dead_code)]

// `egui` is not a direct dependency of the crate; it is re-exported by
// `bevy_egui`. Importing it this way keeps this file usable from the library
// and from the `#[path]`-included component gallery example.
use bevy_egui::egui::{
    self, Align2, Color32, CornerRadius, CursorIcon, FontId, Frame, Margin, Response, RichText,
    Sense, Shadow, Stroke, StrokeKind, TextStyle, Ui, Vec2, WidgetInfo, WidgetType, vec2,
};

// ---------------------------------------------------------------------------
// Core palette
// ---------------------------------------------------------------------------

/// Full-screen dimming layer painted behind the overlay.
pub const SCRIM: Color32 = Color32::from_black_alpha(110);

/// Main overlay surface. Opaque on purpose: panels must stay readable on top of
/// explosion-heavy frames.
pub const PANEL_BG: Color32 = Color32::from_rgb(0x12, 0x16, 0x1D);

/// Raised surface for cards, the active note, toolbar groups.
pub const PANEL_BG_RAISED: Color32 = Color32::from_rgb(0x1A, 0x20, 0x2B);

/// Recessed surface for text inputs and the editor canvas.
pub const INPUT_BG: Color32 = Color32::from_rgb(0x0D, 0x10, 0x16);

/// Hairline border around surfaces.
pub const BORDER: Color32 = Color32::from_rgb(0x2C, 0x35, 0x42);

/// Stronger border for hovered/focused controls and keycaps.
pub const BORDER_STRONG: Color32 = Color32::from_rgb(0x3B, 0x46, 0x58);

/// Primary text.
pub const TEXT_PRIMARY: Color32 = Color32::from_rgb(0xE9, 0xED, 0xF4);

/// Secondary text: captions, metadata, inactive navigation.
pub const TEXT_MUTED: Color32 = Color32::from_rgb(0x9A, 0xA5, 0xB8);

/// Tertiary text: placeholders, separators, hints.
pub const TEXT_FAINT: Color32 = Color32::from_rgb(0x6C, 0x76, 0x89);

/// The one accent color: warm amber. Distinct from typical red/green gameplay
/// HUD colors while remaining readable on dark and mid-tone backgrounds.
pub const ACCENT: Color32 = Color32::from_rgb(0xF2, 0xB4, 0x41);

/// Hover / pressed variant of [`ACCENT`].
pub const ACCENT_STRONG: Color32 = Color32::from_rgb(0xFF, 0xC9, 0x60);

/// Dark text painted on top of [`ACCENT`] fills.
pub const ON_ACCENT: Color32 = Color32::from_rgb(0x1A, 0x14, 0x09);

/// Destructive / error color.
pub const DANGER: Color32 = Color32::from_rgb(0xE5, 0x48, 0x4D);

/// Success / confirmation color.
pub const SUCCESS: Color32 = Color32::from_rgb(0x4C, 0xC3, 0x8A);

/// Translucent variant of an opaque color, used for fills and glows.
///
/// `color` is expected to be opaque (all palette constants are).
pub fn tint(color: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha)
}

/// Subtle hover wash for custom-painted click targets.
pub fn hover_fill() -> Color32 {
    Color32::from_rgba_unmultiplied(0xE9, 0xED, 0xF4, 14)
}

// ---------------------------------------------------------------------------
// Sticky note palette
// ---------------------------------------------------------------------------

/// The six sticky-note identities. Each variant keeps a matching
/// `(accent, fill, text)` triple so boards, pills and note headers agree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum StickyColor {
    #[default]
    Amber,
    Mint,
    Sky,
    Rose,
    Lilac,
    Paper,
}

impl StickyColor {
    /// All palette variants in display order.
    pub const ALL: [Self; 6] = [
        Self::Amber,
        Self::Mint,
        Self::Sky,
        Self::Rose,
        Self::Lilac,
        Self::Paper,
    ];

    /// Stable palette slot, useful for persistence.
    pub const fn index(self) -> usize {
        match self {
            Self::Amber => 0,
            Self::Mint => 1,
            Self::Sky => 2,
            Self::Rose => 3,
            Self::Lilac => 4,
            Self::Paper => 5,
        }
    }

    /// Palette variant for a persisted index (wraps, never panics).
    pub const fn from_index(index: usize) -> Self {
        match index % Self::ALL.len() {
            0 => Self::Amber,
            1 => Self::Mint,
            2 => Self::Sky,
            3 => Self::Rose,
            4 => Self::Lilac,
            _ => Self::Paper,
        }
    }

    /// Human readable name.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Amber => "Amber",
            Self::Mint => "Mint",
            Self::Sky => "Sky",
            Self::Rose => "Rose",
            Self::Lilac => "Lilac",
            Self::Paper => "Paper",
        }
    }

    /// Accent color: borders, tabs, active icons.
    pub const fn accent(self) -> Color32 {
        match self {
            Self::Amber => Color32::from_rgb(0xF2, 0xB4, 0x41),
            Self::Mint => Color32::from_rgb(0x7B, 0xD8, 0xA8),
            Self::Sky => Color32::from_rgb(0x6F, 0xC3, 0xF2),
            Self::Rose => Color32::from_rgb(0xF2, 0x88, 0x9B),
            Self::Lilac => Color32::from_rgb(0xC3, 0x9B, 0xF2),
            Self::Paper => Color32::from_rgb(0xE6, 0xE1, 0xD6),
        }
    }

    /// Card fill color on the dark overlay.
    pub const fn fill(self) -> Color32 {
        match self {
            Self::Amber => Color32::from_rgb(0x3A, 0x2E, 0x14),
            Self::Mint => Color32::from_rgb(0x1C, 0x35, 0x29),
            Self::Sky => Color32::from_rgb(0x16, 0x30, 0x3F),
            Self::Rose => Color32::from_rgb(0x3B, 0x1F, 0x26),
            Self::Lilac => Color32::from_rgb(0x2C, 0x21, 0x40),
            Self::Paper => Color32::from_rgb(0x2B, 0x2A, 0x26),
        }
    }

    /// Text color for text printed on [`Self::fill`].
    pub const fn text(self) -> Color32 {
        match self {
            Self::Amber => Color32::from_rgb(0xFF, 0xE6, 0xB8),
            Self::Mint => Color32::from_rgb(0xD3, 0xF5, 0xE3),
            Self::Sky => Color32::from_rgb(0xD6, 0xF0, 0xFF),
            Self::Rose => Color32::from_rgb(0xFF, 0xD9, 0xDF),
            Self::Lilac => Color32::from_rgb(0xEA, 0xDB, 0xFF),
            Self::Paper => Color32::from_rgb(0xF5, 0xF2, 0xEA),
        }
    }
}

// ---------------------------------------------------------------------------
// Spacing, radii and typography
// ---------------------------------------------------------------------------

/// Extra-small gap: icon-to-label, pill padding.
pub const SPACE_XS: f32 = 4.0;
/// Small gap: inside toolbars, between tightly related controls.
pub const SPACE_SM: f32 = 8.0;
/// Medium gap: list rows, form fields.
pub const SPACE_MD: f32 = 12.0;
/// Large gap: panel padding, card padding.
pub const SPACE_LG: f32 = 16.0;
/// Extra-large gap: section separation, floating margins.
pub const SPACE_XL: f32 = 24.0;
/// Double extra-large gap: page gutters.
pub const SPACE_XXL: f32 = 32.0;

/// Small corner radius: tool buttons, keycaps.
pub const RADIUS_SM: f32 = 5.0;
/// Medium corner radius: cards, buttons, inputs.
pub const RADIUS_MD: f32 = 8.0;
/// Large corner radius: panels.
pub const RADIUS_LG: f32 = 12.0;
/// Pill radius (clamped by [`CornerRadius`] internally).
pub const RADIUS_PILL: f32 = 255.0;

/// Height of compact tool buttons.
pub const TOOL_BUTTON_SIZE: f32 = 28.0;
/// Height of list/navigation rows.
pub const NAV_ROW_HEIGHT: f32 = 34.0;
/// Height of text inputs and standard buttons.
pub const CONTROL_HEIGHT: f32 = 30.0;

/// Extra-small type: keycaps, metadata.
pub const FONT_XS: f32 = 10.5;
/// Small type: captions, section labels.
pub const FONT_SM: f32 = 11.5;
/// Body type: labels and prose.
pub const FONT_BODY: f32 = 13.5;
/// Button type.
pub const FONT_BUTTON: f32 = 13.0;
/// Editor code type.
pub const FONT_CODE: f32 = 13.5;
/// Sub-heading type.
pub const FONT_LG: f32 = 18.0;
/// Page title type.
pub const FONT_XL: f32 = 22.0;
/// Hero/display type.
pub const FONT_DISPLAY: f32 = 30.0;

/// Convert a float radius into an egui [`CornerRadius`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn radius(r: f32) -> CornerRadius {
    CornerRadius::same(r.round().clamp(0.0, 255.0) as u8)
}

/// Convert a float padding into an egui [`Margin`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn margin(p: f32) -> Margin {
    Margin::same(p.round().clamp(0.0, 127.0) as i8)
}

/// Convert float horizontal/vertical padding into an egui [`Margin`].
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
pub fn margin_xy(x: f32, y: f32) -> Margin {
    Margin::symmetric(
        x.round().clamp(0.0, 127.0) as i8,
        y.round().clamp(0.0, 127.0) as i8,
    )
}

/// Small text font.
pub fn font_small() -> FontId {
    FontId::proportional(FONT_SM)
}

/// Body text font.
pub fn font_body() -> FontId {
    FontId::proportional(FONT_BODY)
}

/// Button text font.
pub fn font_button() -> FontId {
    FontId::proportional(FONT_BUTTON)
}

/// Section heading font.
pub fn font_heading() -> FontId {
    FontId::proportional(FONT_LG)
}

/// Page title font.
pub fn font_title() -> FontId {
    FontId::proportional(FONT_XL)
}

/// Hero display font.
pub fn font_display() -> FontId {
    FontId::proportional(FONT_DISPLAY)
}

/// Proportional font at an arbitrary size, for large glyphs and painted text.
pub fn font_at(size: f32) -> FontId {
    FontId::proportional(size)
}

/// Monospace font at an arbitrary size.
pub fn font_mono(size: f32) -> FontId {
    FontId::monospace(size)
}

// ---------------------------------------------------------------------------
// Global theme
// ---------------------------------------------------------------------------

/// Install the Stickie look on an egui context.
///
/// Call once after the egui context exists (idempotent and cheap enough to call
/// again, e.g. after an egui theme change).
pub fn apply_theme(ctx: &egui::Context) {
    // Stickie is always dark; do not follow the OS theme preference.
    ctx.set_theme(egui::Theme::Dark);
    ctx.set_style_of(egui::Theme::Dark, build_style());
}

/// Build a full egui [`egui::Style`] from the Stickie tokens.
fn build_style() -> egui::Style {
    let mut style = egui::Style::default();
    let mut visuals = egui::Visuals::dark();

    visuals.dark_mode = true;
    visuals.override_text_color = Some(TEXT_PRIMARY);
    visuals.panel_fill = PANEL_BG;
    visuals.window_fill = PANEL_BG_RAISED;
    visuals.window_stroke = Stroke::new(1.0, BORDER);
    visuals.window_corner_radius = radius(RADIUS_LG);
    visuals.window_shadow = Shadow::NONE;
    visuals.popup_shadow = Shadow {
        offset: [0, 6],
        blur: 18,
        spread: 0,
        color: Color32::from_black_alpha(120),
    };
    visuals.menu_corner_radius = radius(RADIUS_MD);
    visuals.hyperlink_color = ACCENT;
    visuals.faint_bg_color = PANEL_BG_RAISED;
    visuals.extreme_bg_color = INPUT_BG;
    visuals.text_edit_bg_color = Some(INPUT_BG);
    visuals.code_bg_color = INPUT_BG;
    visuals.warn_fg_color = ACCENT;
    visuals.error_fg_color = DANGER;
    visuals.button_frame = true;
    visuals.striped = false;
    visuals.interact_cursor = Some(CursorIcon::PointingHand);
    visuals.selection = egui::style::Selection {
        bg_fill: tint(ACCENT, 64),
        stroke: Stroke::new(1.0, ACCENT),
    };
    visuals.text_cursor = egui::style::TextCursorStyle {
        stroke: Stroke::new(2.0, ACCENT),
        ..Default::default()
    };

    let widgets = &mut visuals.widgets;
    widgets.noninteractive = egui::style::WidgetVisuals {
        bg_fill: PANEL_BG,
        weak_bg_fill: PANEL_BG,
        bg_stroke: Stroke::new(1.0, BORDER),
        corner_radius: radius(RADIUS_MD),
        fg_stroke: Stroke::new(1.0, TEXT_MUTED),
        expansion: 0.0,
    };
    widgets.inactive = egui::style::WidgetVisuals {
        bg_fill: PANEL_BG_RAISED,
        weak_bg_fill: PANEL_BG_RAISED,
        bg_stroke: Stroke::new(1.0, BORDER),
        corner_radius: radius(RADIUS_MD),
        fg_stroke: Stroke::new(1.0, TEXT_PRIMARY),
        expansion: 0.0,
    };
    widgets.hovered = egui::style::WidgetVisuals {
        bg_fill: tint(ACCENT, 28),
        weak_bg_fill: tint(ACCENT, 22),
        bg_stroke: Stroke::new(1.0, tint(ACCENT, 150)),
        corner_radius: radius(RADIUS_MD),
        fg_stroke: Stroke::new(1.0, ACCENT_STRONG),
        expansion: 0.0,
    };
    widgets.active = egui::style::WidgetVisuals {
        bg_fill: tint(ACCENT, 48),
        weak_bg_fill: tint(ACCENT, 40),
        bg_stroke: Stroke::new(1.0, ACCENT),
        corner_radius: radius(RADIUS_MD),
        fg_stroke: Stroke::new(1.0, ACCENT_STRONG),
        expansion: 0.0,
    };
    widgets.open = widgets.hovered;

    style.visuals = visuals;
    style.spacing.item_spacing = vec2(SPACE_SM, SPACE_SM);
    style.spacing.button_padding = vec2(10.0, 5.0);
    style.spacing.interact_size = vec2(32.0, CONTROL_HEIGHT);
    style.spacing.menu_margin = margin(SPACE_MD);
    style.spacing.window_margin = margin(SPACE_MD);
    style.spacing.indent = 16.0;
    style.spacing.scroll = egui::style::ScrollStyle::thin();
    style.text_styles = [
        (TextStyle::Small, font_small()),
        (TextStyle::Body, font_body()),
        (TextStyle::Button, font_button()),
        (TextStyle::Heading, font_heading()),
        (TextStyle::Monospace, font_mono(FONT_CODE)),
    ]
    .into();

    style
}

// ---------------------------------------------------------------------------
// Frames
// ---------------------------------------------------------------------------

/// Frame for a full-height content panel.
pub fn panel_frame() -> Frame {
    Frame::NONE
        .fill(PANEL_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(radius(RADIUS_LG))
        .inner_margin(margin(SPACE_LG))
}

/// Frame for a raised card inside a panel.
pub fn card_frame() -> Frame {
    Frame::NONE
        .fill(PANEL_BG_RAISED)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(radius(RADIUS_MD))
        .inner_margin(margin(SPACE_MD))
        .shadow(Shadow {
            offset: [0, 2],
            blur: 8,
            spread: 0,
            color: Color32::from_black_alpha(48),
        })
}

/// Frame for a recessed control cluster (editor canvas, toolbars).
pub fn sunken_frame() -> Frame {
    Frame::NONE
        .fill(INPUT_BG)
        .stroke(Stroke::new(1.0, BORDER))
        .corner_radius(radius(RADIUS_MD))
        .inner_margin(margin(SPACE_MD))
}

// ---------------------------------------------------------------------------
// Painted widgets
// ---------------------------------------------------------------------------

/// Paint the full-screen overlay dim behind the rest of the UI.
///
/// Intended to be called on the root viewport [`Ui`] before any content.
pub fn scrim(ui: &Ui) {
    let rect = ui.ctx().content_rect();
    ui.ctx()
        .layer_painter(ui.layer_id())
        .rect_filled(rect, CornerRadius::ZERO, SCRIM);
}

/// Vertical navigation / list row with an icon, a label and a selected state.
///
/// The row spans the available width so it can live in a fixed-width rail.
pub fn nav_button(ui: &mut Ui, glyph: &str, label: &str, selected: bool) -> Response {
    let width = ui.available_width().max(120.0);
    let (rect, response) = ui.allocate_exact_size(vec2(width, NAV_ROW_HEIGHT), Sense::click());
    let painter = ui.painter().clone();

    if selected {
        painter.rect_filled(rect, radius(RADIUS_MD), tint(ACCENT, 24));
        let bar = egui::Rect::from_min_size(
            rect.left_top() + vec2(0.0, 7.0),
            vec2(3.0, NAV_ROW_HEIGHT - 14.0),
        );
        painter.rect_filled(bar, radius(2.0), ACCENT);
    } else if response.hovered() {
        painter.rect_filled(rect, radius(RADIUS_MD), hover_fill());
    }

    let (fg, weight_color) = if selected {
        (ACCENT, TEXT_PRIMARY)
    } else if response.hovered() {
        (TEXT_PRIMARY, TEXT_PRIMARY)
    } else {
        (TEXT_MUTED, TEXT_MUTED)
    };
    painter.text(
        rect.left_center() + vec2(14.0, 0.0),
        Align2::LEFT_CENTER,
        glyph,
        // Monospace has the widest fallback chain (Hack + emoji fonts), so
        // geometric and emoji glyphs both render without missing-glyph boxes.
        font_mono(14.0),
        fg,
    );
    painter.text(
        rect.left_center() + vec2(40.0, 0.0),
        Align2::LEFT_CENTER,
        label,
        font_body(),
        weight_color,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, label));
    response
}

/// Square, label-less toolbar button with a tooltip.
pub fn icon_button(ui: &mut Ui, glyph: &str, tooltip: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOOL_BUTTON_SIZE), Sense::click());
    let painter = ui.painter().clone();

    if response.is_pointer_button_down_on() {
        painter.rect_filled(rect, radius(RADIUS_SM), tint(ACCENT, 40));
    } else if response.hovered() {
        painter.rect_filled(rect, radius(RADIUS_SM), hover_fill());
    }
    let color = if response.hovered() {
        TEXT_PRIMARY
    } else {
        TEXT_MUTED
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        font_mono(13.0),
        color,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.widget_info(|| {
        let mut info = WidgetInfo::new(WidgetType::Button);
        info.label = Some(tooltip.to_owned());
        info
    });
    response.on_hover_text(tooltip)
}

/// Toolbar button that can be toggled on, e.g. formatting or drawing tools.
pub fn tool_button(ui: &mut Ui, glyph: &str, tooltip: &str, selected: bool) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(TOOL_BUTTON_SIZE), Sense::click());
    let painter = ui.painter().clone();

    if selected {
        painter.rect_filled(rect, radius(RADIUS_SM), tint(ACCENT, 34));
        painter.rect_stroke(
            rect,
            radius(RADIUS_SM),
            Stroke::new(1.0, tint(ACCENT, 170)),
            StrokeKind::Inside,
        );
    } else if response.is_pointer_button_down_on() {
        painter.rect_filled(rect, radius(RADIUS_SM), tint(ACCENT, 40));
    } else if response.hovered() {
        painter.rect_filled(rect, radius(RADIUS_SM), hover_fill());
    }

    let color = if selected {
        ACCENT_STRONG
    } else if response.hovered() {
        TEXT_PRIMARY
    } else {
        TEXT_MUTED
    };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        glyph,
        font_mono(12.0),
        color,
    );
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Button, true, selected, tooltip));
    response.on_hover_text(tooltip)
}

/// Solid accent call-to-action button.
pub fn primary_button(ui: &mut Ui, label: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font_button(), ON_ACCENT);
    let size = vec2(
        galley.size().x + 2.0 * SPACE_MD,
        CONTROL_HEIGHT.max(galley.size().y + SPACE_SM),
    );
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let fill = if response.hovered() || response.is_pointer_button_down_on() {
        ACCENT_STRONG
    } else {
        ACCENT
    };
    ui.painter().rect_filled(rect, radius(RADIUS_MD), fill);
    ui.painter()
        .galley(rect.center() - galley.size() * 0.5, galley, ON_ACCENT);
    if response.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, true, label));
    response
}

/// Small rounded status tag with a tinted background.
pub fn pill(ui: &mut Ui, text: &str, color: Color32) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_small(), color);
    let padding = vec2(SPACE_SM, 2.0);
    let size = galley.size() + padding * 2.0;
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius(RADIUS_PILL), tint(color, 32));
    painter.rect_stroke(
        rect,
        radius(RADIUS_PILL),
        Stroke::new(1.0, tint(color, 96)),
        StrokeKind::Inside,
    );
    painter.galley(rect.min + padding, galley, color);
    response
}

/// Keycap-style keyboard shortcut hint, e.g. `Ctrl` + `S`.
pub fn kbd(ui: &mut Ui, text: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_mono(FONT_XS), TEXT_MUTED);
    let padding = vec2(6.0, 2.0);
    let size = vec2(galley.size().x + padding.x * 2.0, 18.0);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter().clone();
    painter.rect_filled(rect, radius(4.0), PANEL_BG_RAISED);
    painter.rect_stroke(
        rect,
        radius(4.0),
        Stroke::new(1.0, BORDER_STRONG),
        StrokeKind::Inside,
    );
    // Fake the 3D keycap with one darker pixel row at the bottom.
    painter.rect_filled(
        egui::Rect::from_min_max(
            egui::pos2(rect.left() + 2.0, rect.bottom() - 3.0),
            egui::pos2(rect.right() - 2.0, rect.bottom() - 1.0),
        ),
        radius(1.0),
        BORDER,
    );
    painter.galley(rect.min + padding, galley, TEXT_MUTED);
    response
}

/// Tracked, uppercase section label followed by a hairline rule.
pub fn section_label(ui: &mut Ui, text: &str) -> Response {
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_uppercase(), font_small(), TEXT_FAINT);
    let height = 18.0;
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), height), Sense::hover());
    let painter = ui.painter().clone();
    painter.galley(
        egui::pos2(rect.left(), rect.center().y - galley.size().y * 0.5),
        galley.clone(),
        TEXT_FAINT,
    );
    let line_start = rect.left() + galley.size().x + SPACE_MD;
    if line_start < rect.right() {
        painter.hline(
            line_start..=rect.right(),
            rect.center().y,
            Stroke::new(1.0, BORDER),
        );
    }
    response
}

/// Centered empty state for an empty note, empty library or error view.
///
/// Returns the action button response when `action_label` is provided.
pub fn empty_state(
    ui: &mut Ui,
    icon: &str,
    title: &str,
    body: &str,
    action_label: Option<&str>,
) -> Option<Response> {
    let mut action_response = None;
    ui.vertical_centered(|ui| {
        ui.add_space(SPACE_XL);
        let (icon_rect, _) = ui.allocate_exact_size(Vec2::splat(56.0), Sense::hover());
        ui.painter()
            .circle_filled(icon_rect.center(), 28.0, tint(ACCENT, 20));
        // Monospace has the widest fallback chain (Hack + emoji fonts), so both
        // geometric glyphs and emoji icons render instead of missing-glyph boxes.
        ui.painter().text(
            icon_rect.center(),
            Align2::CENTER_CENTER,
            icon,
            font_mono(21.0),
            TEXT_MUTED,
        );
        ui.add_space(SPACE_MD);
        ui.label(
            RichText::new(title)
                .font(font_heading())
                .color(TEXT_PRIMARY),
        );
        ui.add_space(SPACE_XS);
        ui.add(egui::Label::new(RichText::new(body).font(font_body()).color(TEXT_MUTED)).wrap());
        if let Some(label) = action_label {
            ui.add_space(SPACE_LG);
            action_response = Some(primary_button(ui, label));
        }
        ui.add_space(SPACE_XL);
    });
    action_response
}

/// Kinds of transient status messages.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToastKind {
    /// Neutral information.
    Info,
    /// Something succeeded.
    Success,
    /// Something failed.
    Error,
}

impl ToastKind {
    /// Accent color for the toast icon.
    pub const fn color(self) -> Color32 {
        match self {
            Self::Info => ACCENT,
            Self::Success => SUCCESS,
            Self::Error => DANGER,
        }
    }

    /// Leading glyph for the toast.
    pub const fn icon(self) -> &'static str {
        match self {
            Self::Info => "i",
            Self::Success => "✓",
            Self::Error => "!",
        }
    }
}

/// Paint a floating toast at the bottom center of the viewport.
///
/// The parent decides how long to keep calling this; no timers are owned by the
/// theme. Uses an [`egui::Area`] so it does not disturb the layout.
pub fn toast(ui: &Ui, text: &str, kind: ToastKind) {
    let ctx = ui.ctx();
    egui::Area::new(egui::Id::new("stickie-toast"))
        .order(egui::Order::Foreground)
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -SPACE_XL))
        .interactable(false)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(PANEL_BG_RAISED)
                .stroke(Stroke::new(1.0, BORDER))
                .corner_radius(radius(RADIUS_MD))
                .inner_margin(margin_xy(SPACE_MD, SPACE_SM))
                .shadow(Shadow {
                    offset: [0, 4],
                    blur: 16,
                    spread: 0,
                    color: Color32::from_black_alpha(110),
                })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(kind.icon())
                                .font(font_at(12.0))
                                .color(kind.color()),
                        );
                        ui.label(RichText::new(text).font(font_body()).color(TEXT_PRIMARY));
                    });
                });
        });
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn channel_luminance(value: u8) -> f32 {
        let c = f32::from(value) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(color: Color32) -> f32 {
        0.2126 * channel_luminance(color.r())
            + 0.7152 * channel_luminance(color.g())
            + 0.0722 * channel_luminance(color.b())
    }

    fn contrast(a: Color32, b: Color32) -> f32 {
        let (la, lb) = (luminance(a), luminance(b));
        let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
        (hi + 0.05) / (lo + 0.05)
    }

    #[test]
    fn core_text_meets_wcag_aa_on_panels() {
        assert!(contrast(TEXT_PRIMARY, PANEL_BG) >= 7.0);
        assert!(contrast(TEXT_PRIMARY, PANEL_BG_RAISED) >= 7.0);
        assert!(contrast(TEXT_MUTED, PANEL_BG) >= 4.5);
        assert!(contrast(TEXT_MUTED, PANEL_BG_RAISED) >= 4.5);
        assert!(contrast(ACCENT, PANEL_BG) >= 4.5);
        assert!(contrast(ON_ACCENT, ACCENT) >= 4.5);
        assert!(contrast(DANGER, PANEL_BG) >= 4.0);
        assert!(contrast(SUCCESS, PANEL_BG_RAISED) >= 4.5);
    }

    #[test]
    fn sticky_palettes_are_readable() {
        for color in StickyColor::ALL {
            assert!(
                contrast(color.text(), color.fill()) >= 7.0,
                "{color:?} body text"
            );
            assert!(
                contrast(color.accent(), color.fill()) >= 4.5,
                "{color:?} accent"
            );
        }
    }

    #[test]
    fn palette_indices_roundtrip() {
        for (expected, color) in StickyColor::ALL.into_iter().enumerate() {
            assert_eq!(color.index(), expected);
            assert_eq!(StickyColor::from_index(expected), color);
        }
        assert_eq!(StickyColor::from_index(6), StickyColor::Amber);
        assert_eq!(
            StickyColor::from_index(usize::MAX),
            StickyColor::from_index(usize::MAX % 6)
        );
    }

    #[test]
    fn radii_and_margins_are_clamped() {
        assert_eq!(radius(-5.0), CornerRadius::ZERO);
        assert_eq!(radius(10_000.0), radius(RADIUS_PILL));
        assert_eq!(margin(-5.0), Margin::ZERO);
        assert_eq!(margin(1_000.0), Margin::same(127));
    }

    #[test]
    fn tint_preserves_rgb() {
        let dim = tint(ACCENT, 64);
        assert_eq!(dim.a(), 64);
        assert_eq!((dim.r(), dim.g(), dim.b()), (0x3D, 0x2D, 0x10));
    }
}
