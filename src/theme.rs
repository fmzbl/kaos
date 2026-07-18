//! The red current — the TUI palette.
//!
//! kaos's principal ray is **Red** (Mars, war, vitality — *Liber Kaos*), and
//! the interface wears it. All chrome is rendered in 24-bit ANSI red on a dark
//! ground, with the other rays muted so the red dominates. Pure std — these are
//! just escape codes.

/// The blood-red of the principal ray — headings, prompts, the sigil of chaos.
pub const RED: (u8, u8, u8) = (220, 40, 48);
/// A deeper oxblood for rules and frames.
pub const OXBLOOD: (u8, u8, u8) = (120, 24, 28);
/// A dim ash for secondary text.
pub const ASH: (u8, u8, u8) = (150, 140, 140);
/// Near-white for emphasis on the red ground.
pub const BONE: (u8, u8, u8) = (235, 225, 222);

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
    bold(RED, s)
}
pub fn ash(s: &str) -> String {
    fg(ASH, s)
}
pub fn bone(s: &str) -> String {
    fg(BONE, s)
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
        .map(|l| bold(RED, l))
        .collect::<Vec<_>>()
        .join("\n")
}

/// A horizontal rule in oxblood, `n` wide.
pub fn rule(n: usize) -> String {
    dim(OXBLOOD, &"\u{2500}".repeat(n))
}

/// The prompt: a red sigil and chevron.
pub fn prompt() -> String {
    format!("{} {} ", chaosphere(), bold(RED, "\u{276f}")) // ✴ ❯
}
