//! The palette, shared by the terminal app and `kaos visual`.
//!
//! Two modes, each a neutral grey scale plus a single purple accent. The
//! shapes, sigils and rules carry the meaning through brightness; the accent is
//! spent only on what the eye should find first, so it never competes with
//! itself. Both the terminal app and `kaos visual` read this one palette. `/theme dark`
//! and `/theme light` persist the choice in the Kaos config, and both
//! interfaces read it back through [`mode`]. Pure std — these are just escape
//! codes.

// The four roles the one-shot CLI output uses. They resolve from the current
// mode rather than being fixed, so `kaos scry`, `kaos auth` and the rest follow
// `/theme` like everything else. Kept as functions, not constants, because the
// mode is read from the config at run time.

/// Headings, prompts, the sigil of chaos — the accent.
#[allow(non_snake_case)]
pub fn RED() -> (u8, u8, u8) {
    current().accent
}
/// Rules and frames.
#[allow(non_snake_case)]
pub fn OXBLOOD() -> (u8, u8, u8) {
    current().faint
}
/// Secondary text.
#[allow(non_snake_case)]
pub fn ASH() -> (u8, u8, u8) {
    current().faint
}
/// Emphasis against the ground.
#[allow(non_snake_case)]
pub fn BONE() -> (u8, u8, u8) {
    current().ink
}

// ── monochrome modes ────────────────────────────────────────────────────────

/// Which way round the interface runs.
///
/// The palette is deliberately black and white: the shapes, glyphs and rules
/// carry the meaning, so colour is left to do nothing but separate figure from
/// ground. One mode is the other inverted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Mode {
    #[default]
    Dark,
    Light,
}

impl Mode {
    /// Parse `dark` / `light`, however it was typed.
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "dark" => Some(Mode::Dark),
            "light" => Some(Mode::Light),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Mode::Dark => "dark",
            Mode::Light => "light",
        }
    }

    pub fn flipped(self) -> Self {
        match self {
            Mode::Dark => Mode::Light,
            Mode::Light => Mode::Dark,
        }
    }
}

/// The whole interface in five tones, so a mode is one value rather than a
/// scattering of constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Palette {
    /// The page.
    pub ground: (u8, u8, u8),
    /// Panels and chrome, a step from the ground.
    pub chrome: (u8, u8, u8),
    /// Shape interiors.
    pub fill: (u8, u8, u8),
    /// Text, strokes, and every drawn symbol.
    pub ink: (u8, u8, u8),
    /// One step back from the ink, for a second class of emphasis. With no
    /// colour to distinguish roles, brightness has to do that work.
    pub mid: (u8, u8, u8),
    /// Secondary text and rules.
    pub faint: (u8, u8, u8),
    /// The one colour: purple, for what the eye should go to first —
    /// headings, the selection, the active tool, a live arrow. Everything
    /// else stays neutral so the accent keeps its force.
    pub accent: (u8, u8, u8),
}

/// The palette for a mode. Light is not a tint of dark — it is the inverse, so
/// ink and ground swap ends.
pub const fn palette(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            ground: (12, 12, 12),
            chrome: (22, 22, 22),
            fill: (30, 30, 30),
            ink: (238, 238, 238),
            mid: (190, 190, 190),
            faint: (140, 140, 140),
            // Bright enough to carry on a near-black ground.
            accent: (176, 132, 232),
        },
        Mode::Light => Palette {
            ground: (250, 250, 250),
            chrome: (240, 240, 240),
            fill: (255, 255, 255),
            ink: (16, 16, 16),
            mid: (70, 70, 70),
            faint: (120, 120, 120),
            // Deepened so it still reads against white.
            accent: (104, 58, 168),
        },
    }
}

/// The configured mode, defaulting to dark. Read fresh so a `/theme` change
/// applies to anything started afterwards without a restart dance.
pub fn mode() -> Mode {
    crate::config::value("theme")
        .as_deref()
        .and_then(Mode::parse)
        .unwrap_or_default()
}

/// Persist the mode. Both the terminal app and `kaos visual` read it back
/// through [`mode`], so one setting dresses both.
pub fn set_mode(mode: Mode) -> Result<(), String> {
    crate::config::set_value("theme", mode.name()).map(|_| ())
}

/// The current palette.
pub fn current() -> Palette {
    palette(mode())
}

/// Wrap `s` in a 24-bit foreground colour.
pub fn fg(rgb: (u8, u8, u8), s: &str) -> String {
    format!("\x1b[38;2;{};{};{}m{}\x1b[0m", rgb.0, rgb.1, rgb.2, s)
}

/// Bold + coloured.
pub fn bold(rgb: (u8, u8, u8), s: &str) -> String {
    format!("\x1b[1;38;2;{};{};{}m{}\x1b[0m", rgb.0, rgb.1, rgb.2, s)
}

/// Dim coloured.
pub fn dim(rgb: (u8, u8, u8), s: &str) -> String {
    format!("\x1b[2;38;2;{};{};{}m{}\x1b[0m", rgb.0, rgb.1, rgb.2, s)
}

pub fn red(s: &str) -> String {
    bold(current().accent, s)
}
pub fn ash(s: &str) -> String {
    fg(current().faint, s)
}
pub fn bone(s: &str) -> String {
    fg(current().ink, s)
}

/// The Sigil of Chaos — Carroll's eight-rayed star, the sole symbol of the Pact,
/// rendered small in red for the prompt and banners.
pub fn chaosphere() -> String {
    red("\u{2734}") // an eight-pointed star ✴
}

/// The Chaos Star — the eight-arrowed Sigil of Chaos, as ASCII art. Eight arrows
/// radiate symmetrically from a central point (N, NE, E, SE, S, SW, W, NW), the
/// diagonal rays sweeping outward at a true 45° so the whole reads as a round
/// starburst rather than a boxy cross.
pub fn chaos_star_lines() -> [&'static str; 11] {
    [
        "              \u{2191}",                                   //               ↑
        "              \u{2502}",                                   //               │
        "        \u{2196}     \u{2502}     \u{2197}",               //         ↖     │     ↗
        "          \u{2572}   \u{2502}   \u{2571}",                 //           ╲   │   ╱
        "            \u{2572} \u{2502} \u{2571}",                   //             ╲ │ ╱
        "    \u{2190}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{25ef}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2500}\u{2192}", // ←─────────◯─────────→
        "            \u{2571} \u{2502} \u{2572}",                   //             ╱ │ ╲
        "          \u{2571}   \u{2502}   \u{2572}",                 //           ╱   │   ╲
        "        \u{2199}     \u{2502}     \u{2198}",               //         ↙     │     ↘
        "              \u{2502}",                                   //               │
        "              \u{2193}",                                   //               ↓
    ]
}

/// The Chaos Star rendered in bold red, ready to print in a banner.
pub fn chaos_star_red() -> String {
    chaos_star_lines()
        .iter()
        .map(|l| bold(current().accent, l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A horizontal rule in oxblood, `n` wide.
pub fn rule(n: usize) -> String {
    dim(current().faint, &"\u{2500}".repeat(n))
}

/// The prompt: a red sigil and chevron.
pub fn prompt() -> String {
    format!("{} {} ", chaosphere(), bold(current().accent, "\u{276f}")) // ✴ ❯
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_parse_however_they_are_typed() {
        assert_eq!(Mode::parse("dark"), Some(Mode::Dark));
        assert_eq!(Mode::parse("  LIGHT "), Some(Mode::Light));
        assert_eq!(Mode::parse("Dark"), Some(Mode::Dark));
        assert_eq!(Mode::parse("sepia"), None);
        assert_eq!(Mode::parse(""), None);
    }

    #[test]
    fn a_mode_round_trips_through_its_name() {
        for m in [Mode::Dark, Mode::Light] {
            assert_eq!(Mode::parse(m.name()), Some(m));
            assert_eq!(m.flipped().flipped(), m);
            assert_ne!(m.flipped(), m);
        }
    }

    #[test]
    fn the_neutral_tones_are_true_greys() {
        // The scale carries meaning through brightness alone; only `accent`
        // is allowed to have a hue.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            for (name, (r, g, b)) in [
                ("ground", p.ground),
                ("chrome", p.chrome),
                ("fill", p.fill),
                ("ink", p.ink),
                ("mid", p.mid),
                ("faint", p.faint),
            ] {
                assert!(
                    r == g && g == b,
                    "{} of {m:?} is not grey: {r},{g},{b}",
                    name
                );
            }
        }
    }

    #[test]
    fn light_inverts_dark_rather_than_tinting_it() {
        let (d, l) = (palette(Mode::Dark), palette(Mode::Light));
        // Ink and ground swap ends of the scale.
        assert!(d.ink.0 > d.ground.0, "dark should be light-on-dark");
        assert!(l.ink.0 < l.ground.0, "light should be dark-on-light");
    }

    #[test]
    fn the_accent_is_the_only_colour_and_it_is_purple() {
        for m in [Mode::Dark, Mode::Light] {
            let (r, g, b) = palette(m).accent;
            assert!(!(r == g && g == b), "{m:?} accent is grey, not an accent");
            // Purple: blue strongest, red above green, green lowest.
            assert!(b > r && r > g, "{m:?} accent {r},{g},{b} is not purple");
        }
    }

    #[test]
    fn the_accent_reads_against_its_ground() {
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let lum = |(r, g, b): (u8, u8, u8)| {
                0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b)
            };
            assert!(
                (lum(p.accent) - lum(p.ground)).abs() > 40.0,
                "{m:?} accent does not separate from the ground"
            );
        }
    }

    #[test]
    fn the_three_text_tones_are_distinguishable() {
        // With colour gone, brightness is the only thing separating roles, so
        // the steps between them have to be real.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let step = |a: (u8, u8, u8), b: (u8, u8, u8)| (i16::from(a.0) - i16::from(b.0)).abs();
            assert!(step(p.ink, p.mid) >= 40, "{m:?} ink/mid too close");
            assert!(step(p.mid, p.faint) >= 40, "{m:?} mid/faint too close");
        }
    }

    #[test]
    fn ink_and_ground_stay_far_enough_apart_to_read() {
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let gap = (i16::from(p.ink.0) - i16::from(p.ground.0)).abs();
            assert!(gap > 180, "{m:?} contrast is only {gap}");
            // Secondary text must still separate from the ground.
            let faint_gap = (i16::from(p.faint.0) - i16::from(p.ground.0)).abs();
            assert!(faint_gap > 60, "{m:?} faint contrast is only {faint_gap}");
        }
    }
}
