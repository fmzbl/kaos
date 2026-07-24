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
    pub(crate) blue: Color32,
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
            blue: rgb(p.blue),
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
    paths
        .iter()
        .find_map(|path| std::fs::read(path).ok().map(|bytes| ((*path).to_string(), bytes)))
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

/// Dress egui in the kaos palette so the editor matches the terminal app.
pub(crate) fn install_theme(ctx: &egui::Context, k: Ink) {
    let mut visuals = if kaos_core::theme::mode() == kaos_core::theme::Mode::Light {
        egui::Visuals::light()
    } else {
        egui::Visuals::dark()
    };
    visuals.panel_fill = k.chrome;
    visuals.window_fill = k.chrome;
    visuals.extreme_bg_color = k.ground;
    visuals.override_text_color = Some(k.ink);
    visuals.widgets.noninteractive.bg_stroke = UiStroke::new(1.0, k.faint);
    visuals.widgets.inactive.bg_fill = k.fill;
    visuals.widgets.inactive.bg_stroke = UiStroke::new(1.0, k.faint);
    visuals.widgets.hovered.bg_stroke = UiStroke::new(1.0, k.accent);
    visuals.widgets.active.bg_fill = k.accent;
    visuals.selection.bg_fill = k.accent.gamma_multiply(0.35);
    visuals.selection.stroke = UiStroke::new(1.0, k.accent);
    // egui's defaults still carry unrelated colours in a few corners. Remove
    // those so the shared purple accent remains the only chromatic role.
    visuals.hyperlink_color = k.ink;
    visuals.warn_fg_color = k.ink;
    visuals.error_fg_color = k.ink;
    for w in [
        &mut visuals.widgets.noninteractive,
        &mut visuals.widgets.inactive,
        &mut visuals.widgets.hovered,
        &mut visuals.widgets.active,
        &mut visuals.widgets.open,
    ] {
        w.fg_stroke = UiStroke::new(w.fg_stroke.width, k.ink);
    }
    visuals.widgets.hovered.weak_bg_fill = k.fill;
    visuals.widgets.active.weak_bg_fill = k.faint;
    ctx.set_visuals(visuals);
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
