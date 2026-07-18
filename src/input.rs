//! The plain line prompt — used by the fallback REPL (pipes/CI, or a
//! `--no-default-features` build with no TUI). The interactive fullscreen app lives
//! in [`crate::tui`] and has its own editor with history.

use std::io::Write;

/// What a read returned.
pub enum Line {
    /// A submitted line (may be empty).
    Text(String),
    /// End of input / quit (Ctrl-D, or EOF on a pipe).
    Eof,
}

/// A std-only prompt: prints the prompt and reads a line. No editing, no arrows —
/// that is the TUI's job; this exists so pipes and CI keep working.
pub struct Prompt;

impl Prompt {
    pub fn new() -> Prompt {
        Prompt
    }

    pub fn read(&mut self, prompt: &str) -> Line {
        print!("{prompt}");
        let _ = std::io::stdout().flush();
        let mut s = String::new();
        match std::io::stdin().read_line(&mut s) {
            Ok(0) => Line::Eof,
            Ok(_) => Line::Text(s.trim_end_matches(['\n', '\r']).to_string()),
            Err(_) => Line::Eof,
        }
    }
}

impl Default for Prompt {
    fn default() -> Self {
        Self::new()
    }
}
