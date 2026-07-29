//! The visual editor's appearance: the resolved palette (`Ink`), egui theme
//! dressing, and the symbol-capable font fallback.
//!
//! This is the visual crate's *presentation* seam. The palette itself lives in
//! [`kaos_core::theme`] and is shared with the terminal app; this module only
//! turns those tones into egui `Color32`s and installs them (plus fonts) into
//! an egui context, so the frontend's look is one small module rather than
//! scattered through the canvas code.

use eframe::egui;
use egui::{Color32, Stroke as UiStroke};

pub(crate) fn rgb((r, g, b): (u8, u8, u8)) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// The shared tones of the current mode, resolved once per window.
#[derive(Clone, Copy)]
pub(crate) struct Ink {
    pub(crate) accent: Color32,
    pub(crate) secondary: Color32,
    pub(crate) danger: Color32,
    pub(crate) ground: Color32,
    pub(crate) chrome: Color32,
    pub(crate) fill: Color32,
    pub(crate) ink: Color32,
    pub(crate) faint: Color32,
}

impl Ink {
    pub(crate) fn load() -> Self {
        let p = kaos_core::theme::current();
        Self {
            accent: rgb(p.accent),
            secondary: rgb(p.secondary),
            danger: rgb(p.danger),
            ground: rgb(p.ground),
            chrome: rgb(p.chrome),
            fill: rgb(p.fill),
            ink: rgb(p.ink),
            faint: rgb(p.faint),
        }
    }
}

/// System faces with wide symbol coverage, most preferred first. Only
/// single-face `.ttf`/`.otf` files: a `.ttc` collection would fail to parse and
/// panic inside the font stack.
const SYMBOL_FALLBACK_PROPORTIONAL: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
    "/System/Library/Fonts/Supplemental/Arial Unicode.ttf",
    "/Library/Fonts/Arial Unicode.ttf",
    "C:\\Windows\\Fonts\\seguisym.ttf",
];

/// The same, for the monospace family.
const SYMBOL_FALLBACK_MONOSPACE: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
    "/usr/share/fonts/TTF/DejaVuSansMono.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
    "C:\\Windows\\Fonts\\consola.ttf",
];

/// The first candidate that exists and can be read, with its bytes.
fn first_readable_font(paths: &[&str]) -> Option<(String, Vec<u8>)> {
    paths.iter().find_map(|path| {
        std::fs::read(path)
            .ok()
            .map(|bytes| ((*path).to_string(), bytes))
    })
}

/// Give egui a font that can actually draw this editor's symbols.
///
/// egui's bundled faces cover Latin text but not the box-drawing, geometric and
/// technical characters the UI uses for tools and state (`┈`, `▹`, `●`, `⏸`,
/// `─`). A glyph the font lacks renders as tofu — the black square. Append the
/// first system face we find with broad symbol coverage as a *fallback* for
/// both families, so ordinary text still uses egui's own fonts and only the
/// missing glyphs come from here. Each family prefers the face with matching
/// metrics but can borrow from the other, so a symbol renders either way. When
/// no candidate is present the fonts are left untouched — never a crash.
pub(crate) fn install_symbol_fallback(ctx: &egui::Context) {
    let proportional = first_readable_font(SYMBOL_FALLBACK_PROPORTIONAL);
    let monospace = first_readable_font(SYMBOL_FALLBACK_MONOSPACE);
    if proportional.is_none() && monospace.is_none() {
        return;
    }
    let mut fonts = egui::FontDefinitions::default();
    for (name, bytes) in [proportional.clone(), monospace.clone()]
        .into_iter()
        .flatten()
    {
        fonts
            .font_data
            .insert(name, egui::FontData::from_owned(bytes));
    }
    // Fall back in metric-matching order, then to the other face.
    for (family, order) in [
        (
            egui::FontFamily::Proportional,
            [proportional.as_ref(), monospace.as_ref()],
        ),
        (
            egui::FontFamily::Monospace,
            [monospace.as_ref(), proportional.as_ref()],
        ),
    ] {
        let entry = fonts.families.entry(family).or_default();
        for (name, _) in order.into_iter().flatten() {
            if !entry.contains(name) {
                entry.push(name.clone());
            }
        }
    }
    ctx.set_fonts(fonts);
}

/// The chrome's geometry, in one place.
///
/// The canvas draws a technical plate — squares, hairline arrows, glyphs — and
/// the chrome around it should read as the instrument's own labelling rather
/// than as an application wrapped around a drawing. That means near-square
/// corners, hairline rules, and controls that are quiet until you are on them.
mod metric {
    /// Widget corners. Not zero — that reads as brutalist and fights the
    /// canvas's curves — but far below egui's default 6, which reads as a
    /// consumer app.
    pub(super) const ROUNDING: f32 = 2.0;
    /// Floating surfaces sit one step softer than inline widgets.
    pub(super) const ROUNDING_FLOATING: f32 = 3.0;
    /// Horizontal and vertical rhythm between items. egui's default vertical
    /// gap of 3 packs rows tight enough that a toolbar reads as one mass.
    pub(super) const GAP: (f32, f32) = (9.0, 7.0);
    /// Inside a button, around its label.
    pub(super) const BUTTON_PADDING: (f32, f32) = (9.0, 5.0);
    /// The minimum a click target may shrink to.
    pub(super) const HIT_HEIGHT: f32 = 24.0;
}

/// The type scale. Four steps and one monospace, chosen so a glance can rank
/// what it is looking at without any colour being spent on hierarchy.
///
/// Buttons are MONOSPACE on purpose, and it is the one deliberate risk in this
/// chrome. Every control in this editor is a lowercase verb — `run`, `clear`,
/// `reset view` — sitting beside a canvas whose whole vocabulary is monospaced
/// glyphs. Setting them in the proportional UI face made them read as a web
/// toolbar that happened to be bolted to a diagram; in mono they read as
/// instrument controls, and the chrome and the canvas finally look like one
/// object.
fn install_type_scale(style: &mut egui::Style) {
    use egui::{FontFamily::Monospace, FontFamily::Proportional, FontId, TextStyle};
    style.text_styles = [
        (TextStyle::Heading, FontId::new(15.0, Proportional)),
        (TextStyle::Body, FontId::new(13.0, Proportional)),
        (TextStyle::Button, FontId::new(12.5, Monospace)),
        (TextStyle::Small, FontId::new(11.0, Proportional)),
        (TextStyle::Monospace, FontId::new(12.5, Monospace)),
    ]
    .into();
}

/// Dress egui in the kaos palette so the editor matches the terminal app.
///
/// The palette itself is shared with the TUI and is not re-decided here — what
/// this sets is how the accent is SPENT. egui's defaults, and this editor's
/// first pass over them, flooded a pressed control with the saturated accent,
/// which
/// made the loudest thing on screen the fact that you had just clicked
/// something. Accent now lives in strokes and text: it marks where you are, not
/// what you did. Fills stay quiet frost blue in every state.
pub(crate) fn install_theme(ctx: &egui::Context, k: Ink) {
    let light = kaos_core::theme::mode() == kaos_core::theme::Mode::Light;
    let mut visuals = if light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };

    visuals.panel_fill = k.chrome;
    visuals.window_fill = k.chrome;
    visuals.extreme_bg_color = k.ground;
    visuals.faint_bg_color = k.chrome;
    visuals.code_bg_color = k.fill;
    // Left unset so a widget's own foreground stroke decides its text colour —
    // an override here would win over every state below, including the accent
    // that marks the pressed and open ones.
    visuals.override_text_color = None;

    let hairline = |width: f32, color: Color32| UiStroke::new(width, color);
    let wash = |color: Color32, opacity: f32| {
        Color32::from_rgba_unmultiplied(
            color.r(),
            color.g(),
            color.b(),
            (opacity.clamp(0.0, 1.0) * 255.0).round() as u8,
        )
    };
    // Rules and edges are hairlines, not lines: full-strength `faint` on every
    // frame boundary turns a panel into a wireframe.
    let rule = wash(k.faint, if light { 0.45 } else { 0.35 });

    // Resting: a surface a step forward from the panel, edged rather than
    // outlined, with no accent anywhere.
    visuals.widgets.noninteractive.bg_fill = k.chrome;
    visuals.widgets.noninteractive.weak_bg_fill = k.chrome;
    visuals.widgets.noninteractive.bg_stroke = hairline(1.0, rule);
    visuals.widgets.inactive.bg_fill = k.fill;
    visuals.widgets.inactive.weak_bg_fill = k.fill;
    visuals.widgets.inactive.bg_stroke = hairline(1.0, rule);

    // Hovered: the edge picks up the accent and nothing else moves. The fill is
    // deliberately unchanged so a pointer crossing a toolbar does not make the
    // whole row flicker.
    visuals.widgets.hovered.bg_fill = k.fill;
    visuals.widgets.hovered.weak_bg_fill = k.fill;
    visuals.widgets.hovered.bg_stroke = hairline(1.0, k.accent);

    // Pressed and open: the edge thickens and the LABEL takes the accent. This
    // is the whole accent budget for a control, and it is why a press no longer
    // repaints the button.
    visuals.widgets.active.bg_fill = k.fill;
    visuals.widgets.active.weak_bg_fill = k.fill;
    visuals.widgets.active.bg_stroke = hairline(1.4, k.accent);
    visuals.widgets.open.bg_fill = k.fill;
    visuals.widgets.open.weak_bg_fill = k.fill;
    visuals.widgets.open.bg_stroke = hairline(1.0, k.accent);

    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.fg_stroke = hairline(w.fg_stroke.width, k.ink);
        w.rounding = egui::Rounding::same(metric::ROUNDING);
        w.expansion = 0.0;
    }
    // The two states that carry the accent, applied after the loop above has
    // levelled every foreground to ink.
    visuals.widgets.active.fg_stroke = hairline(1.0, k.accent);
    visuals.widgets.open.fg_stroke = hairline(1.0, k.accent);

    // Selection is a wash, not a highlighter: it has to sit under text and
    // leave it readable.
    visuals.selection.bg_fill = k.accent.gamma_multiply(if light { 0.20 } else { 0.26 });
    visuals.selection.stroke = hairline(1.0, k.accent);

    // Floating surfaces read as lifted by their edge rather than by a shadow —
    // a drop shadow under a menu is the one thing that would make this chrome
    // look like a consumer app again.
    visuals.window_rounding = egui::Rounding::same(metric::ROUNDING_FLOATING);
    visuals.menu_rounding = egui::Rounding::same(metric::ROUNDING_FLOATING);
    visuals.window_stroke = hairline(1.0, rule);
    visuals.window_shadow = egui::epaint::Shadow::NONE;
    visuals.popup_shadow = egui::epaint::Shadow::NONE;

    // egui's defaults still carry unrelated colours in a few corners. Route
    // them through Kaos's chosen semantic roles instead.
    visuals.hyperlink_color = k.secondary;
    visuals.warn_fg_color = k.danger;
    visuals.error_fg_color = k.danger;
    visuals.text_cursor.stroke = hairline(1.5, k.accent);
    // Text and range selection: egui ships its own blue here, which is a third
    // hue nobody chose. The accent at egui's own default weight keeps the
    // palette the only source of colour in the editor.
    visuals.selection.bg_fill =
        Color32::from_rgba_unmultiplied(k.accent.r(), k.accent.g(), k.accent.b(), 66);

    let mut style = (*ctx.style()).clone();
    style.visuals = visuals;
    install_type_scale(&mut style);
    style.spacing.item_spacing = egui::vec2(metric::GAP.0, metric::GAP.1);
    style.spacing.button_padding = egui::vec2(metric::BUTTON_PADDING.0, metric::BUTTON_PADDING.1);
    style.spacing.interact_size.y = metric::HIT_HEIGHT;
    style.spacing.window_margin = egui::Margin::same(10.0);
    style.spacing.menu_margin = egui::Margin::same(8.0);
    style.spacing.indent = 18.0;
    ctx.set_style(style);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every colour in the editor comes from the shared semantic palette.
    ///
    /// egui ships defaults with hues of their own — a blue text selection, a
    /// blue hyperlink, a yellow warning, a red error. Each one that survives is
    /// a colour nobody chose, and it makes the interface look like the toolkit
    /// rather than like Kaos. Kaos deliberately chooses its own green, blue,
    /// and red roles.
    #[test]
    fn the_editor_uses_only_the_shared_semantic_palette() {
        let k = Ink::load();
        let ctx = egui::Context::default();
        install_theme(&ctx, k);
        let v = ctx.style().visuals.clone();
        // Color32 stores premultiplied channels, so a translucent role is not
        // literally the role's bytes. Reconstruct at the same alpha instead.
        let is_role = |c: Color32, role: Color32| {
            c == Color32::from_rgba_unmultiplied(role.r(), role.g(), role.b(), c.a())
        };
        let roles = [
            k.ground,
            k.chrome,
            k.fill,
            k.ink,
            k.faint,
            k.accent,
            k.secondary,
            k.danger,
        ];
        let from_palette = |c: Color32| c.a() == 0 || roles.iter().any(|role| is_role(c, *role));
        let mut strays: Vec<String> = Vec::new();
        let mut check = |name: &str, c: Color32| {
            if !from_palette(c) {
                strays.push(format!("{name} = {c:?}"));
            }
        };
        for (name, w) in [
            ("noninteractive", &v.widgets.noninteractive),
            ("inactive", &v.widgets.inactive),
            ("hovered", &v.widgets.hovered),
            ("active", &v.widgets.active),
            ("open", &v.widgets.open),
        ] {
            check(&format!("{name}.bg_fill"), w.bg_fill);
            check(&format!("{name}.weak_bg_fill"), w.weak_bg_fill);
            check(&format!("{name}.bg_stroke"), w.bg_stroke.color);
            check(&format!("{name}.fg_stroke"), w.fg_stroke.color);
        }
        check("selection.bg_fill", v.selection.bg_fill);
        check("selection.stroke", v.selection.stroke.color);
        check("hyperlink_color", v.hyperlink_color);
        check("faint_bg_color", v.faint_bg_color);
        check("extreme_bg_color", v.extreme_bg_color);
        check("code_bg_color", v.code_bg_color);
        check("warn_fg_color", v.warn_fg_color);
        check("error_fg_color", v.error_fg_color);
        check("window_fill", v.window_fill);
        check("window_stroke", v.window_stroke.color);
        check("panel_fill", v.panel_fill);
        check("text_cursor", v.text_cursor.stroke.color);
        assert!(
            strays.is_empty(),
            "colours outside the palette: {}",
            strays.join(", ")
        );
    }

    #[test]
    fn symbol_fallback_candidates_are_single_face_fonts() {
        // A `.ttc` collection cannot be parsed by the font stack and would
        // panic at first use, so the candidate lists must never contain one.
        for path in SYMBOL_FALLBACK_PROPORTIONAL
            .iter()
            .chain(SYMBOL_FALLBACK_MONOSPACE)
        {
            let lower = path.to_ascii_lowercase();
            assert!(
                lower.ends_with(".ttf") || lower.ends_with(".otf"),
                "{path} is not a single-face font file"
            );
        }
        // Missing candidates are simply skipped, never an error.
        assert!(first_readable_font(&["/nonexistent/font.ttf"]).is_none());
    }

    /// The accent is spent on strokes and text, never on a fill.
    ///
    /// A saturated fill on the pressed state makes the loudest thing on screen
    /// the fact that a button was clicked, which is the least interesting event
    /// in the editor. Pinned here because it is a one-line change to undo by
    /// accident and nothing else would notice.
    #[test]
    fn no_widget_state_fills_with_the_accent() {
        let ctx = egui::Context::default();
        let k = Ink::load();
        install_theme(&ctx, k);
        let v = &ctx.style().visuals;
        for (name, w) in [
            ("noninteractive", &v.widgets.noninteractive),
            ("inactive", &v.widgets.inactive),
            ("hovered", &v.widgets.hovered),
            ("active", &v.widgets.active),
            ("open", &v.widgets.open),
        ] {
            assert_ne!(w.bg_fill, k.accent, "{name}.bg_fill floods with the accent");
            assert_ne!(
                w.weak_bg_fill, k.accent,
                "{name}.weak_bg_fill floods with the accent"
            );
        }
        // …and the two states that DO carry it carry it in the edge and label.
        assert_eq!(v.widgets.active.bg_stroke.color, k.accent);
        assert_eq!(v.widgets.active.fg_stroke.color, k.accent);
        assert_eq!(v.widgets.hovered.bg_stroke.color, k.accent);
    }

    /// Controls are monospace; running text is not. The chrome and the canvas
    /// have to look like one instrument.
    #[test]
    fn buttons_are_monospace_and_body_text_is_not() {
        let ctx = egui::Context::default();
        install_theme(&ctx, Ink::load());
        let style = ctx.style();
        assert_eq!(
            style.text_styles[&egui::TextStyle::Button].family,
            egui::FontFamily::Monospace
        );
        assert_eq!(
            style.text_styles[&egui::TextStyle::Body].family,
            egui::FontFamily::Proportional
        );
        // A real scale, not five copies of one size.
        assert!(
            style.text_styles[&egui::TextStyle::Heading].size
                > style.text_styles[&egui::TextStyle::Body].size
                && style.text_styles[&egui::TextStyle::Body].size
                    > style.text_styles[&egui::TextStyle::Small].size
        );
    }

    /// Corners stay near-square and no floating surface casts a shadow.
    #[test]
    fn geometry_stays_flat_and_near_square() {
        let ctx = egui::Context::default();
        install_theme(&ctx, Ink::load());
        let v = &ctx.style().visuals;
        assert_eq!(v.widgets.inactive.rounding.nw, metric::ROUNDING);
        assert_eq!(v.window_shadow, egui::epaint::Shadow::NONE);
        assert_eq!(v.popup_shadow, egui::epaint::Shadow::NONE);
    }
}
