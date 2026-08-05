//! The palette, shared by the terminal app and `kaos visual`.
//!
//! Black, white, grey, and blue. The page is black and the text is white, or
//! the page is turned over and it is the other way round; everything between
//! them — panels, shape interiors, the two quieter weights of text — is a
//! neutral cool grey, and they separate from one another by brightness alone.
//! Nothing structural carries a hue, which is the whole reason the coloured
//! voices can stay as quiet as they are and still be seen: on a grey field, a
//! little blue is a lot of signal.
//!
//! Blue marks identity, focus, success, recursion and active work. Teal — blue
//! turned toward green, the same family seen from the other side — carries
//! executable flow, navigation, live data and ranges. Red is the exception to
//! the scheme and is kept for one job: invalid source, failed work, and
//! destructive controls. It is the one hue that is not white, grey, or blue,
//! and it is here because a failure that reads as a success is the single
//! mistake an interface must not make. A warning drawn in the palette's own
//! colours is a warning the eye skims.
//!
//! Blue and teal are told apart by which of green and blue leads, not by
//! brightness, so they survive being drawn thin: a one-pixel rule, a braille
//! wave, an underline.
//!
//! `/theme dark` and `/theme light` persist the choice in the Kaos config, and
//! both interfaces read it back through [`mode`] — one setting dresses the
//! window, the terminal, and one-shot CLI output alike. Pure std: these are
//! just escape codes.

// The four roles the one-shot CLI output uses. They resolve from the current
// mode rather than being fixed, so `kaos scry`, `kaos auth` and the rest follow
// `/theme` like everything else. Kept as functions, not constants, because the
// mode is read from the config at run time.

// These six are named for the hues an older palette used. They are kept as
// they are because five hundred call sites read better than a rename would buy,
// and because every one of them already resolves through the palette by ROLE —
// so a mode change or a repaint reaches all of them at once. What each *is*
// today is written on it.

/// Success, focus, recursion, active work, and the chaos star. Blue.
#[allow(non_snake_case)]
pub fn GREEN() -> (u8, u8, u8) {
    current().accent
}
/// The flow/information role. Teal.
#[allow(non_snake_case)]
pub fn PURPLE() -> (u8, u8, u8) {
    current().secondary
}
/// Failure, invalid state, warnings, and destructive actions.
#[allow(non_snake_case)]
pub fn RED() -> (u8, u8, u8) {
    current().danger
}
/// Rules and frames. The dim grey, quieter than any text.
#[allow(non_snake_case)]
pub fn OXBLOOD() -> (u8, u8, u8) {
    current().rule
}
/// Secondary text. Grey.
#[allow(non_snake_case)]
pub fn ASH() -> (u8, u8, u8) {
    current().faint
}
/// A heading or a label naming what stands beside it. Silver.
#[allow(non_snake_case)]
pub fn SILVER() -> (u8, u8, u8) {
    current().mid
}
/// Emphasis against the ground. White, or black.
#[allow(non_snake_case)]
pub fn BONE() -> (u8, u8, u8) {
    current().ink
}

// ── cool structure with semantic RGB roles ─────────────────────────────────

/// Which way round the interface runs.
///
/// Shape, glyph, and rule roles run from the ground to the ink and separate
/// through brightness alone, every one of them a neutral cool grey. Blue, teal,
/// and red are reserved for semantic state. One mode reverses figure and
/// ground.
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
    /// Silver: one step back from the ink, for a second class of emphasis —
    /// a heading, a pane title, a label that names what is beside it.
    ///
    /// A TEXT tone. These three carry most of what anyone actually reads — a
    /// config listing, a panel label, a diagnostic — and text has one job, so
    /// they are the PLAINEST things on screen: grey, and never as coloured as
    /// any of the three state roles. A caption tinted far enough to be pretty
    /// is a caption that is harder to read, which is a bad trade every time.
    pub mid: (u8, u8, u8),
    /// Grey: secondary text. A text tone: see [`Palette::mid`].
    pub faint: (u8, u8, u8),
    /// The dim grey of frames, dividers, and hairlines.
    ///
    /// Not a text tone, and that is the point of its existing. A border drawn
    /// in the colour of secondary text reads as loudly as the text it encloses,
    /// so a screen of framed panes becomes a grid with some words in it. This
    /// one sits between the panel it draws and the text inside, which is what
    /// lets the structure be seen without being read.
    pub rule: (u8, u8, u8),
    /// Blue, for success, focus, recursion, active work, and the chaos star.
    pub accent: (u8, u8, u8),
    /// The second chromatic voice: executable flow, navigation, live data,
    /// and ranges.
    ///
    /// Named for its ROLE, not its hue, which is what let it become a second
    /// blue when the interface was cut down to blue, red and the neutrals. It
    /// is separated from [`Palette::accent`] by LIGHTNESS rather than hue now,
    /// so the two still read apart at a glance while the screen keeps one
    /// chromatic family and one alarm.
    pub secondary: (u8, u8, u8),
    /// Red, for invalid source, failed work, warnings, destructive actions —
    /// and quantity, which is the one thing here that is not a state.
    ///
    /// The only hue in the interface that is not blue or a neutral, which is
    /// precisely why the numeric plane borrows it: the other pole of the
    /// language should be the other colour on the screen, and there is exactly
    /// one left. It means "look here" in both cases; whether that is alarm or
    /// arithmetic is told by what is drawn, not by a third hue.
    pub danger: (u8, u8, u8),
}

/// The palette for a mode.
///
/// **Blue, red, and the neutrals — nothing else.** One grey family from ground
/// to ink, two blues separated by lightness, and one red. A screen with three
/// hues on it makes a reader learn which of them matters; a screen with one
/// chromatic family and one alarm does not.
///
/// Light reverses the luminance hierarchy and nothing else.
pub const fn palette(mode: Mode) -> Palette {
    match mode {
        Mode::Dark => Palette {
            // White on black, and five greys climbing between them: panel,
            // interior, rule, grey text, silver.
            ground: (0, 0, 0),
            chrome: (20, 24, 30),
            fill: (33, 39, 49),
            rule: (62, 70, 84),
            faint: (126, 136, 152),
            mid: (198, 205, 216),
            ink: (255, 255, 255),
            accent: (96, 154, 255),
            // The second blue: deeper and more saturated than the accent, not
            // paler. Flow must read apart from focus WITHOUT outshouting it —
            // a pale secondary separates from the page fine and then puts
            // navigation louder than selection, which is the hierarchy
            // backwards. The band is narrow: darker than this stops separating
            // from a black ground at all.
            secondary: (52, 110, 235),
            danger: (255, 96, 96),
        },
        Mode::Light => Palette {
            // The same page turned over: black on white, and the same five
            // greys counted from the other end.
            ground: (255, 255, 255),
            chrome: (238, 241, 246),
            fill: (247, 249, 252),
            rule: (196, 203, 214),
            faint: (100, 110, 126),
            mid: (66, 76, 90),
            ink: (0, 0, 0),
            accent: (32, 86, 232),
            // Deeper again, counted from the other end of the page: on white,
            // separating from the ground means going darker, not lighter.
            secondary: (18, 46, 132),
            danger: (188, 32, 44),
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
    fn every_surface_is_a_neutral_grey() {
        // The whole surface is grey separating by brightness, so nothing
        // structural competes with the three state colours — which is what
        // lets those three be seen without being loud. Grey here means cool:
        // blue at least as high as green, green at least as high as red, and
        // the spread between the extremes small enough that no surface reads
        // as a colour of its own.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            for (name, (r, g, b)) in [
                ("ground", p.ground),
                ("chrome", p.chrome),
                ("fill", p.fill),
                ("rule", p.rule),
                ("ink", p.ink),
                ("mid", p.mid),
                ("faint", p.faint),
            ] {
                assert!(b >= g && g >= r, "{name} of {m:?} is not cool: {r},{g},{b}");
                assert!(
                    i32::from(b) - i32::from(r) <= 26,
                    "{name} of {m:?} is a colour, not a grey: {r},{g},{b}"
                );
            }
            // The two ends go all the way: black and white, not a tinted page
            // and a soft ink. Everything with a colour in it is state.
            let (dark_end, light_end) = if m == Mode::Dark {
                (p.ground, p.ink)
            } else {
                (p.ink, p.ground)
            };
            assert!(
                dark_end.2 <= 32,
                "{m:?}: the dark end should be near-black: {dark_end:?}"
            );
            assert!(
                light_end.0 >= 236,
                "{m:?}: the light end should be white: {light_end:?}"
            );
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
    fn the_three_voices_stay_apart_even_though_they_are_muted() {
        // A palette whose warning and whose highlight are neighbours is worse
        // than a dull one: a failure that reads as a success is the one mistake
        // an interface must not make.
        //
        // Two of the three voices are now the SAME hue — the interface is blue,
        // red and the neutrals, and nothing else — so the rule that keeps them
        // apart cannot be which channel wins. Identity and flow both lead with
        // blue and are separated by lightness; failure leads with red, and is
        // the only thing on screen that does.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            let (r, g, b) = p.accent;
            assert!(b > g && g > r, "{m:?} accent {r},{g},{b} is not blue");
            let (r, g, b) = p.secondary;
            assert!(b > g && g > r, "{m:?} secondary {r},{g},{b} is not blue");
            let (r, g, b) = p.danger;
            assert!(r > g && r > b, "{m:?} danger {r},{g},{b} is not red");

            let apart = |a: (u8, u8, u8), b: (u8, u8, u8)| {
                let d = |x: u8, y: u8| f64::from(i32::from(x) - i32::from(y)).abs();
                d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
            };
            // Channel distance separates voices of DIFFERENT hues, which is
            // what these were when the rule was written. The two blues cannot
            // meet it and no pair of them could: on a black ground a tone must
            // clear 4.5 contrast to be read at all, and the readable band above
            // that is narrower than 120 channels wide. What actually tells them
            // apart now is lightness, checked below.
            for (left, right, pair) in [
                (p.accent, p.danger, "accent/danger"),
                (p.secondary, p.danger, "secondary/danger"),
            ] {
                assert!(
                    apart(left, right) >= 120.0,
                    "{m:?} {pair} are too close to distinguish: {:.0}",
                    apart(left, right)
                );
            }
            // The two blues, by the measure that applies to them. 1.5:1 is what
            // the band allows once both ends must stay readable — tighter than
            // a two-hue palette bought, and the price of one chromatic family.
            let separation = contrast_ratio(p.accent, p.secondary);
            assert!(
                separation >= 1.5,
                "{m:?} the two blues are too close to read apart: {separation:.2}"
            );
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
    fn every_text_tone_is_readable_against_the_page() {
        // The invariant behind "readable text takes the theme's ink": all three text
        // tones clear the ordinary body-text contrast bar against the ground, so a
        // config listing or a panel caption is legible whatever the palette does for
        // decoration. Tinting a caption far enough to be pretty makes it harder to
        // read, which is a bad trade every time.
        for m in [Mode::Dark, Mode::Light] {
            let p = palette(m);
            for (name, tone) in [("ink", p.ink), ("mid", p.mid), ("faint", p.faint)] {
                let ratio = contrast_ratio(tone, p.ground);
                assert!(
                    ratio >= 4.5,
                    "{m:?} {name} reads at only {ratio:.1}:1 against the page"
                );
            }
            // And they are text, not decoration. In a warm palette that cannot mean
            // "grey" — beige is tinted by definition — so it means PLAINER than
            // anything carrying state: no text tone may be as coloured as the least
            // coloured of the three voices.
            let spread =
                |(r, g, b): (u8, u8, u8)| i32::from(r.max(g).max(b)) - i32::from(r.min(g).min(b));
            let loudest_text = [p.ink, p.mid, p.faint]
                .into_iter()
                .map(spread)
                .max()
                .expect("three tones");
            let plainest_voice = [p.accent, p.secondary, p.danger]
                .into_iter()
                .map(spread)
                .min()
                .expect("three voices");
            assert!(
                loudest_text < plainest_voice,
                "{m:?}: text is tinted up to {loudest_text} where the plainest state \
                 colour is only {plainest_voice} — text should be the quietest thing \
                 on screen"
            );
        }
    }

    #[test]
    fn the_three_text_tones_are_distinguishable() {
        // Warm text still separates its roles by brightness, so the steps between
        // its tones have to be real.
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

    /// Blue, red, and the neutrals — enforced rather than intended.
    ///
    /// A palette is a promise about what a reader has to learn, and promises
    /// kept only by care get broken by the next person adding a colour for a
    /// good local reason. Every tone here must be a neutral, a blue, or the one
    /// red; a teal or a green fails this and says so.
    #[test]
    fn every_tone_is_a_neutral_a_blue_or_the_one_red() {
        /// How far apart channels may be and still count as grey.
        ///
        /// Wide enough for the cool greys this palette actually uses — the
        /// widest is 26 — and nowhere near wide enough for a chromatic tone:
        /// the teal this replaced spread 146.
        const NEUTRAL: i32 = 30;

        for mode in [Mode::Dark, Mode::Light] {
            let p = palette(mode);
            let tones: [(&str, (u8, u8, u8)); 9] = [
                ("ground", p.ground),
                ("chrome", p.chrome),
                ("fill", p.fill),
                ("rule", p.rule),
                ("faint", p.faint),
                ("mid", p.mid),
                ("ink", p.ink),
                ("accent", p.accent),
                ("secondary", p.secondary),
            ];
            for (name, (r, g, b)) in tones {
                let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
                let spread = r.max(g).max(b) - r.min(g).min(b);
                let neutral = spread <= NEUTRAL;
                // A blue leads with blue and does not let green stand level
                // with it — which is what a teal does, and why teal is gone.
                let blue = b > r && b > g && b - g > NEUTRAL;
                assert!(
                    neutral || blue,
                    "{mode:?} {name} ({r},{g},{b}) is neither a neutral nor a blue"
                );
            }
            let (r, g, b) = p.danger;
            let (r, g, b) = (i32::from(r), i32::from(g), i32::from(b));
            assert!(
                r > g && r > b,
                "{mode:?} danger ({r},{g},{b}) is the one red and must lead with red"
            );
        }
    }

    #[test]
    fn the_two_blues_are_told_apart_by_lightness() {
        // They share a hue on purpose, so nothing else can tell them apart.
        for mode in [Mode::Dark, Mode::Light] {
            let p = palette(mode);
            let lightness = |(r, g, b): (u8, u8, u8)| i32::from(r) + i32::from(g) + i32::from(b);
            assert!(
                (lightness(p.accent) - lightness(p.secondary)).abs() > 90,
                "{mode:?}: accent and secondary are too close to read apart"
            );
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
