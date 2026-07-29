//! The palette, shared by the terminal app and `kaos visual`.
//!
//! Two modes with quiet frost-blue structure and three semantic colours. Green
//! marks success, focus, recursion, and active work; blue carries flow,
//! navigation, and information; red marks invalid, failed, or destructive
//! states. Both the terminal app and `kaos visual` read this one palette.
//! `/theme dark` and `/theme light` persist the choice in the Kaos config, and
//! both interfaces read it back through [`mode`]. Pure std — these are just
//! escape codes.

// The four roles the one-shot CLI output uses. They resolve from the current
// mode rather than being fixed, so `kaos scry`, `kaos auth` and the rest follow
// `/theme` like everything else. Kept as functions, not constants, because the
// mode is read from the config at run time.

/// Success, focus, recursion, active work, and the chaos star.
#[allow(non_snake_case)]
pub fn GREEN() -> (u8, u8, u8) {
    current().accent
}
/// Legacy colour name for the blue flow/information role.
#[allow(non_snake_case)]
pub fn PURPLE() -> (u8, u8, u8) {
    current().secondary
}
/// Failure, invalid state, warnings, and destructive actions.
#[allow(non_snake_case)]
pub fn RED() -> (u8, u8, u8) {
    current().danger
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

// ── cool structure with semantic RGB roles ─────────────────────────────────

/// Which way round the interface runs.
///
/// Shape, glyph, and rule roles share one cold hue family and separate through
/// brightness. Green, blue, and red are reserved for semantic state. One mode
/// reverses the lightness of the figure and ground.
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

/// The whole interface in nine tones, so a mode is one value rather than a
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
    /// One step back from the ink, for a second class of emphasis.
    pub mid: (u8, u8, u8),
    /// Secondary text and rules.
    pub faint: (u8, u8, u8),
    /// Green, for success, focus, recursion, active work, and the chaos star.
    pub accent: (u8, u8, u8),
    /// Blue, for executable flow, navigation, live data, and range selections.
    ///
    /// Named for its ROLE, not its hue: the palette's second chromatic voice.
    /// Keeping the role name lets future palettes change without lying to
    /// callers about what the colour means.
    pub secondary: (u8, u8, u8),
    /// Red, for invalid source, failed work, warnings, and destructive actions.
    pub danger: (u8, u8, u8),
}

/// The palette for a mode. Surfaces and text remain cool and restrained while
/// the RGB semantic roles carry state. Light reverses the luminance hierarchy.
pub const fn palette(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            ground: (7, 21, 33),
            chrome: (13, 33, 50),
            fill: (20, 44, 64),
            ink: (232, 247, 255),
            mid: (167, 207, 227),
            faint: (108, 154, 181),
            accent: (72, 214, 126),
            secondary: (114, 183, 255),
            danger: (255, 107, 122),
        },
        Mode::Light => Palette {
            ground: (242, 249, 253),
            chrome: (228, 241, 248),
            fill: (250, 253, 255),
            ink: (11, 42, 61),
            mid: (42, 89, 116),
            faint: (75, 116, 139),
            // Deep enough to remain readable against the icy page.
            accent: (18, 116, 63),
            secondary: (40, 108, 173),
            danger: (180, 35, 53),
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

/// The palette's own ground, as an SGR background parameter.
///
/// One-shot output cannot repaint the terminal the way the fullscreen app
/// paints its ground, so every styled span carries the ground with it. Without
/// this a light theme puts near-black ink straight onto a dark terminal — the
/// one thing a light theme must not do — and a dark theme disappears on a light
/// terminal. With it, the configured mode decides how Kaos looks in both
/// frontends instead of the surrounding terminal deciding for it.
fn ground_sgr(ground: (u8, u8, u8)) -> String {
    format!("48;2;{};{};{}", ground.0, ground.1, ground.2)
}

/// Wrap `s` in a 24-bit foreground colour on the palette's ground.
pub fn fg(rgb: (u8, u8, u8), s: &str) -> String {
    let bg = ground_sgr(current().ground);
    format!("\x1b[{bg};38;2;{};{};{}m{}\x1b[0m", rgb.0, rgb.1, rgb.2, s)
}

/// Bold + coloured.
pub fn bold(rgb: (u8, u8, u8), s: &str) -> String {
    let bg = ground_sgr(current().ground);
    format!(
        "\x1b[1;{bg};38;2;{};{};{}m{}\x1b[0m",
        rgb.0, rgb.1, rgb.2, s
    )
}

/// Dim coloured.
pub fn dim(rgb: (u8, u8, u8), s: &str) -> String {
    let bg = ground_sgr(current().ground);
    format!(
        "\x1b[2;{bg};38;2;{};{};{}m{}\x1b[0m",
        rgb.0, rgb.1, rgb.2, s
    )
}

pub fn green(s: &str) -> String {
    bold(current().accent, s)
}
/// Bold danger text.
pub fn red(s: &str) -> String {
    bold(current().danger, s)
}
pub fn ash(s: &str) -> String {
    fg(current().faint, s)
}
pub fn bone(s: &str) -> String {
    fg(current().ink, s)
}

/// The Sigil of Chaos — Carroll's eight-rayed star, the sole symbol of the Pact,
/// rendered small in the primary green for prompts and banners.
pub fn chaosphere() -> String {
    green("\u{2734}") // an eight-pointed star ✴
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

/// The same eight-arrowed star in compact terminal-watermark form.
///
/// Five cells square is large enough to preserve all eight arrowheads but
/// small enough to sit quietly in a pane corner without becoming content.
pub fn compact_chaos_star_lines() -> [&'static str; 5] {
    ["↖ ↑ ↗", " ╲│╱ ", "←─•─→", " ╱│╲ ", "↙ ↓ ↘"]
}

/// The Chaos Star rendered in bold green, ready for a banner.
pub fn chaos_star_green() -> String {
    chaos_star_lines()
        .iter()
        .map(|l| bold(current().accent, l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Compatibility name for [`chaos_star_green`].
pub fn chaos_star_red() -> String {
    chaos_star_green()
}

/// A horizontal rule in the neutral faint tone, `n` wide.
pub fn rule(n: usize) -> String {
    dim(current().faint, &"\u{2500}".repeat(n))
}

/// The prompt: a green sigil and chevron.
pub fn prompt() -> String {
    format!("{} {} ", chaosphere(), bold(current().accent, "\u{276f}")) // ✴ ❯
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relative_luminance((r, g, b): (u8, u8, u8)) -> f32 {
        let linear = |channel: u8| {
            let channel = f32::from(channel) / 255.0;
            if channel <= 0.04045 {
                channel / 12.92
            } else {
                ((channel + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(r) + 0.7152 * linear(g) + 0.0722 * linear(b)
    }

    fn contrast_ratio(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
        let (bright, dark) = {
            let (a, b) = (relative_luminance(a), relative_luminance(b));
            if a > b {
                (a, b)
            } else {
                (b, a)
            }
        };
        (bright + 0.05) / (dark + 0.05)
    }

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
    fn every_structural_role_belongs_to_the_frost_blue_family() {
        // Structure reads through brightness and retains a restrained blue
        // cast; semantic state is allowed to use the RGB roles below.
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
                    b > r && b >= g && g >= r,
                    "{name} of {m:?} is outside the frost-blue family: {r},{g},{b}"
                );
            }
        }
    }

    #[test]
    fn light_and_dark_keep_opposite_value_structures() {
        let (d, l) = (palette(Mode::Dark), palette(Mode::Light));
        // Ink and ground swap ends of the scale.
        assert!(d.ink.0 > d.ground.0, "dark should be light-on-dark");
        assert!(l.ink.0 < l.ground.0, "light should be dark-on-light");
    }

    #[test]
    fn the_semantic_roles_are_green_blue_and_red() {
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let (r, g, b) = p.accent;
            assert!(g > r && g > b, "{m:?} accent {r},{g},{b} is not green");
            let (r, g, b) = p.secondary;
            assert!(b > g && g > r, "{m:?} secondary {r},{g},{b} is not blue");
            let (r, g, b) = p.danger;
            assert!(r > g && r > b, "{m:?} danger {r},{g},{b} is not red");
            assert_ne!(p.accent, p.secondary);
            assert_ne!(p.accent, p.danger);
            assert_ne!(p.secondary, p.danger);
        }
    }

    #[test]
    fn the_semantic_colours_read_against_their_ground() {
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            for (name, colour) in [
                ("accent", p.accent),
                ("secondary", p.secondary),
                ("danger", p.danger),
            ] {
                assert!(
                    contrast_ratio(colour, p.ground) >= 4.5,
                    "{m:?} {name} does not separate from the ground"
                );
            }
        }
    }

    #[test]
    fn the_three_text_tones_are_distinguishable() {
        // Blue-tinted text still separates roles by brightness, so the steps
        // between its tones have to be real.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            assert!(
                contrast_ratio(p.ink, p.mid) >= 1.45,
                "{m:?} ink/mid too close"
            );
            assert!(
                contrast_ratio(p.mid, p.faint) >= 1.45,
                "{m:?} mid/faint too close"
            );
        }
    }

    #[test]
    fn ink_and_ground_stay_far_enough_apart_to_read() {
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let gap = contrast_ratio(p.ink, p.ground);
            assert!(gap >= 12.0, "{m:?} contrast is only {gap}");
            // Secondary text must still separate from the ground.
            let faint_gap = contrast_ratio(p.faint, p.ground);
            assert!(faint_gap >= 4.5, "{m:?} faint contrast is only {faint_gap}");
        }
    }

    #[test]
    fn styled_output_carries_the_palette_ground() {
        // One-shot output cannot repaint the terminal, so each styled span
        // brings its own ground. Without it the configured mode would only
        // decide the ink and the surrounding terminal would decide the rest —
        // which is exactly how a light theme ends up unreadable on a dark
        // terminal.
        let ground = current().ground;
        let expected = format!("48;2;{};{};{}", ground.0, ground.1, ground.2);
        for painted in [
            fg((1, 2, 3), "x"),
            bold((1, 2, 3), "x"),
            dim((1, 2, 3), "x"),
        ] {
            assert!(painted.contains(&expected), "no ground in {painted:?}");
            assert!(painted.contains("38;2;1;2;3"), "no ink in {painted:?}");
            assert!(painted.ends_with("\u{1b}[0m"), "unreset {painted:?}");
        }
    }

    #[test]
    fn compact_terminal_star_preserves_all_eight_directions() {
        let star = compact_chaos_star_lines().join("\n");
        for arrow in ['↑', '↗', '→', '↘', '↓', '↙', '←', '↖'] {
            assert!(star.contains(arrow), "compact star is missing {arrow}");
        }
        assert!(star.contains('•'));
    }
}
