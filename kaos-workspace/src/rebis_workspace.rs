//! State and pure editor mechanics for KAOS's Rebis workspace.
//!
//! This module deliberately does not parse Rebis.  Syntax authority belongs to
//! `rebis_lang`; the small scanner here only assigns colours to source cells.
//! Every diagnostic and every graph shown by the workspace comes from
//! [`rebis_lang::parse`].

use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use kaos_core::music::{self, Desk, Timbre};
use kaos_core::retained::RetainedLog;
use rebis_lang::{Expr, ModuleName, ModuleResolver, Record};

/// Vim-compatible editing modes supported by the workspace.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Mode {
    /// Motions and editing verbs.
    Normal,
    /// Direct text insertion.
    Insert,
    /// Character-wise selection.
    Visual,
    /// Line-wise selection.
    VisualLine,
    /// Block-wise (rectangular column) selection.
    VisualBlock,
    /// A `:` command is being entered.
    Command,
    /// A `/` Kaos workspace command is being entered.
    KaosCommand,
}

impl Mode {
    /// Compact mode label used by the status line.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Insert => "INSERT",
            Self::Visual => "VISUAL",
            Self::VisualLine => "V-LINE",
            Self::VisualBlock => "V-BLOCK",
            Self::Command => "COMMAND",
            Self::KaosCommand => "KAOS",
        }
    }
}

/// Lexical colour assigned to one source character.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Highlight {
    Atom,
    /// Characters inside a quoted raw prompt.
    Prompt,
    Forward,
    Mediate,
    /// The `#` module import symbol.
    Import,
    /// The `^` syntax inverter.
    Invert,
    /// A postfix `/provider:model` selector.
    Model,
    Backflow,
    Parenthesis,
    Whitespace,
    /// A `;` line comment (outside a quoted prompt), through end of line.
    Comment,
    Invalid,
}

/// One screen row of a soft-wrapped source view.
///
/// A long line does not run off the right edge — it continues on the next screen
/// row. Which means a screen row and a source line are no longer the same thing,
/// and the gutter is what tells them apart: the row that STARTS a line carries its
/// number, and every row continuing it carries blank space of the same width. So
/// the eye reads "still line 42" from the absence of a number.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct VisualRow {
    /// Zero-based source line this row belongs to.
    pub line: usize,
    /// Char offset in the source where this row begins.
    pub start: usize,
    /// Char offset just past this row's last character.
    pub end: usize,
    /// Whether this row continues the line above rather than starting a new one.
    ///
    /// The gutter shows a number exactly when this is false.
    pub continues: bool,
}

/// Break source into the screen rows a viewport `columns` wide would show.
///
/// Wrapping is by character, not by word: this is code, and a break chosen to
/// keep words whole would move a token away from the column its indentation put
/// it at. An empty line still occupies one row — a blank line is a line.
///
/// Shared by both front ends so the same source wraps at the same places in the
/// terminal and in the visual editor, and a line number means the same thing in
/// either.
#[must_use]
pub fn wrapped_rows(source: &str, columns: usize) -> Vec<VisualRow> {
    let columns = columns.max(1);
    let mut rows = Vec::new();
    let mut start = 0usize;
    for (line, text) in source.split('\n').enumerate() {
        let length = text.chars().count();
        let mut consumed = 0usize;
        loop {
            let taken = (length - consumed).min(columns);
            rows.push(VisualRow {
                line,
                start: start + consumed,
                end: start + consumed + taken,
                continues: consumed > 0,
            });
            consumed += taken;
            if consumed >= length {
                break;
            }
        }
        // Past the newline that ended this line.
        start += length + 1;
    }
    rows
}

/// Width of the line-number gutter for a document of `lines` lines.
///
/// Two digits at least, so a short file's gutter does not visibly widen as it
/// grows past nine lines.
#[must_use]
pub fn gutter_width(lines: usize) -> usize {
    lines.to_string().len().max(2)
}

/// Which screen row holds the char at `offset`, and how far into it.
///
/// The cursor is a char offset in the source; a wrapped view has to place it on a
/// row, and the last row of a line owns the position just past its end so the
/// cursor can sit at end-of-line.
#[must_use]
pub fn row_of_offset(rows: &[VisualRow], offset: usize) -> (usize, usize) {
    for (index, row) in rows.iter().enumerate() {
        let last_of_line = rows.get(index + 1).is_none_or(|next| next.line != row.line);
        if offset < row.end || (last_of_line && offset <= row.end) {
            return (index, offset.saturating_sub(row.start));
        }
    }
    rows.last().map_or((0, 0), |row| {
        (rows.len() - 1, offset.saturating_sub(row.start))
    })
}

/// Complete punctuation token set accepted by Rebis, in reference-manual
/// order. The editor renders this as a compact top-bar language legend.
pub const REBIS_SYMBOLS: &[&str] = &[
    "(", ")", "[", "]", "{", "}", "|", "~", "#", "'", ",", "$", "?", "!", "*", "=", "&", "&:", "+",
    "@", "/", "%", "^", "<>", "><", "->", "<-", ";", "\"",
];

/// The live state of a source buffer, for an editor's status line.
///
/// Every editor in both frontends answers the same question — does this parse,
/// and if not, where — so they answer it with the same value rather than each
/// inventing a phrasing. A reader who learns to read the status in the terminal
/// should not have to learn it again in the visual editor.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SourceState {
    /// Nothing typed yet. An empty buffer is not an error and says nothing.
    Empty,
    /// Parses.
    Valid,
    /// Does not parse, with the reason and — when the parser gives an offset —
    /// the one-based line and column it stopped at.
    Invalid {
        message: String,
        line: usize,
        column: usize,
    },
}

impl SourceState {
    /// Parse `source` and describe the result.
    #[must_use]
    pub fn of(source: &str) -> Self {
        if source.trim().is_empty() {
            return Self::Empty;
        }
        match rebis_lang::parse(source) {
            Ok(_) => Self::Valid,
            Err(error) => {
                let (line, column) = error
                    .offset
                    .map_or((0, 0), |offset| line_column(source, offset));
                Self::Invalid {
                    message: error.message,
                    line,
                    column,
                }
            }
        }
    }

    #[must_use]
    pub const fn is_valid(&self) -> bool {
        matches!(self, Self::Valid)
    }

    /// The short marker: nothing, a tick, or a cross.
    #[must_use]
    pub const fn mark(&self) -> &'static str {
        match self {
            Self::Empty => "",
            Self::Valid => "\u{2713} valid",
            Self::Invalid { .. } => "\u{2717} invalid",
        }
    }

    /// The reason, placed. Empty unless the source is invalid.
    ///
    /// A parser message alone makes the reader hunt for the character it is
    /// talking about; with a line and column the status is actionable.
    #[must_use]
    pub fn detail(&self) -> String {
        match self {
            Self::Empty | Self::Valid => String::new(),
            Self::Invalid {
                message,
                line,
                column,
            } if *line > 0 => format!("{line}:{column} \u{2014} {message}"),
            Self::Invalid { message, .. } => message.clone(),
        }
    }
}

/// One-based line and column of a UTF-8 byte offset.
#[must_use]
pub fn line_column(source: &str, byte: usize) -> (usize, usize) {
    let upto = source.get(..byte.min(source.len())).unwrap_or(source);
    let line = upto.matches('\n').count() + 1;
    let column = upto
        .rsplit('\n')
        .next()
        .map_or(0, |last| last.chars().count())
        + 1;
    (line, column)
}

/// Find the complete structural form whose delimiter is at the character
/// cursor (or immediately before it). Both frontends use this for "run block".
#[must_use]
pub fn matching_form(source: &str, cursor: usize) -> Option<(usize, usize)> {
    let chars: Vec<char> = source.chars().collect();
    let with_model = |start: usize, end: usize| {
        let Some((closing_byte, closing)) = source.char_indices().nth(end) else {
            return (start, end);
        };
        let suffix_start = closing_byte + closing.len_utf8();
        let suffix = rebis_lang::tokens(source).into_iter().find(|token| {
            token.start == suffix_start && token.kind == rebis_lang::TokenKind::Model
        });
        let Some(suffix) = suffix else {
            return (start, end);
        };
        let suffix_end = source[..suffix.end].chars().count().saturating_sub(1);
        (start, suffix_end)
    };
    let at = [Some(cursor), cursor.checked_sub(1)]
        .into_iter()
        .flatten()
        .find(|index| matches!(chars.get(*index), Some('(' | ')' | '[' | ']')))?;
    match chars[at] {
        '(' => {
            let mut depth = 0usize;
            for (index, character) in chars.iter().enumerate().skip(at) {
                match character {
                    '(' => depth += 1,
                    ')' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(with_model(at, index));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        ')' => {
            let mut depth = 0usize;
            for index in (0..=at).rev() {
                match chars[index] {
                    ')' => depth += 1,
                    '(' => {
                        depth -= 1;
                        if depth == 0 {
                            return Some(with_model(index, at));
                        }
                    }
                    _ => {}
                }
            }
            None
        }
        '[' => {
            for (index, character) in chars.iter().enumerate().skip(at + 1) {
                if *character == ']' {
                    return Some((at, index));
                }
            }
            None
        }
        ']' => {
            for index in (0..at).rev() {
                if chars[index] == '[' {
                    return Some((index, at));
                }
            }
            None
        }
        _ => None,
    }
}

/// Parse one selected form and prepend the buffer's top-level definitions and
/// imports, preserving the lexical scope a terminal or visual block run sees.
pub fn scoped_block_source(source: &str, block_source: &str) -> Result<String, String> {
    let block = rebis_lang::parse(block_source).map_err(|error| error.to_string())?;
    let mut items = Vec::new();
    if let Ok(Expr::Compose(top) | Expr::Program(top)) = rebis_lang::parse(source) {
        for item in top {
            let static_item = matches!(item, Expr::Function { .. } | Expr::Import { .. })
                || matches!(
                    &item,
                    Expr::Model { body, .. }
                        if matches!(body.as_ref(), Expr::Function { .. } | Expr::Import { .. })
                );
            if static_item {
                items.push(item);
            }
        }
    }
    if items.is_empty() {
        Ok(rebis_lang::pretty_format(&block))
    } else {
        items.push(block);
        Ok(rebis_lang::pretty_format(&Expr::Compose(items)))
    }
}

/// The `/dream` family: what this machine remembers, and how to forget it.
///
/// A read and a delete, and nothing else. Writing is `(! A)`'s job — a memory
/// you can add to from the command line is a memory that stops being a record
/// of what the programs actually decided.
fn dream_command(cwd: &Path, rest: &str) -> String {
    let dream = kaos_core::dream::Dream::for_project(cwd);
    let (word, argument) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(word, argument)| (word, argument.trim()));
    match word {
        "" | "list" => {
            let kept = dream.all();
            if kept.is_empty() {
                return format!(
                    "dream · nothing kept yet · `(! A)` keeps an answer · {}",
                    dream.path().display()
                );
            }
            let recent = kept
                .iter()
                .enumerate()
                .rev()
                .take(3)
                .map(|(index, entry)| {
                    format!(
                        "{index}: {}",
                        truncate(entry.text.lines().next().unwrap_or(""), 40)
                    )
                })
                .collect::<Vec<_>>()
                .join(" · ");
            format!("dream · {} kept · {recent}", kept.len())
        }
        "forget" if argument == "all" => match dream.forget(None) {
            Ok(count) => format!("dream · forgot all {count}"),
            Err(error) => error,
        },
        "forget" => match argument.parse::<usize>() {
            Ok(index) => match dream.forget(Some(index)) {
                Ok(_) => format!("dream · forgot {index} · {} left", dream.len()),
                Err(error) => error,
            },
            Err(_) => "dream · forget wants a number, or `all`".to_string(),
        },
        other => format!("dream · unknown word `{other}` — list, forget N, forget all"),
    }
}

/// Visual projection shown beside the source editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Visualization {
    Tree,
    Mandala,
    /// Search results from the user's saved Rebis sigil library.
    Sigils,
    /// The program as sound: its wave drawn in braille, and the score beneath.
    /// Derived on the `/music` command rather than on every keystroke — a
    /// piece is rendered in full before it can be drawn, and re-rendering
    /// mid-word would be several megabytes a character.
    Music,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum SigilEntry {
    Temporary {
        id: u64,
        label: String,
    },
    Saved(String),
    /// An embedded standard-library module (`std/...`) — browsable and
    /// openable like a saved sigil, but shipped inside rebis-lang.
    Std(String),
}

impl SigilEntry {
    /// The `/`-separated path this entry lives at (temporaries have none).
    fn path(&self) -> Option<&str> {
        match self {
            SigilEntry::Saved(name) | SigilEntry::Std(name) => Some(name),
            SigilEntry::Temporary { .. } => None,
        }
    }
}

/// One rendered line of the collapsible sigil browser.
#[derive(Clone, Debug, Eq, PartialEq)]
enum VisibleRow {
    /// A folder node: its full path (`std`, `team/reviews`), indent depth,
    /// and whether it is currently expanded.
    Folder {
        path: String,
        depth: usize,
        expanded: bool,
    },
    /// An openable sigil, with its indent depth and the label to show (the
    /// last path segment for entries inside a folder).
    Leaf {
        entry: SigilEntry,
        depth: usize,
        label: String,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct TemporarySigil {
    id: u64,
    label: String,
    source: String,
    path: Option<PathBuf>,
    final_output: Option<String>,
    saved_output: Option<String>,
}

/// Resolves foundational Rebis modules from Kaos's saved hypersigil library.
pub struct HypersigilModules {
    root: PathBuf,
}

impl HypersigilModules {
    /// Use `~/.kaos/sigils`, falling back to `cwd` when no home is available.
    #[must_use]
    pub fn user(cwd: PathBuf) -> Self {
        let root = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or(cwd)
            .join(".kaos")
            .join("sigils");
        Self { root }
    }

    /// Use an explicit root, primarily for embedded hosts and deterministic tests.
    #[must_use]
    pub fn at(root: PathBuf) -> Self {
        Self { root }
    }

    fn root(&self) -> &Path {
        &self.root
    }

    fn resolve_folder(&self, module: &ModuleName) -> Result<Option<String>, String> {
        let directory = self.root.join(module.as_str());
        let metadata = match fs::symlink_metadata(&directory) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(format!("{}: {error}", directory.display())),
        };
        if !metadata.file_type().is_dir() {
            return Ok(None);
        }
        let mut children = Vec::new();
        collect_module_folder(&directory, module.as_str(), 0, &mut children)?;
        children.sort();
        children.dedup();
        if children.is_empty() {
            return Ok(None);
        }
        Ok(Some(format!(
            "({})",
            children
                .iter()
                .map(|child| format!("(# {child})"))
                .collect::<Vec<_>>()
                .join(" ")
        )))
    }
}

impl ModuleResolver for HypersigilModules {
    fn resolve(&self, module: &ModuleName) -> Result<Option<String>, String> {
        let mut path = self.root.join(module.as_str());
        path.set_extension("rebis");
        match fs::read_to_string(&path) {
            Ok(source) => Ok(Some(source)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match self.resolve_folder(module)? {
                    Some(source) => Ok(Some(source)),
                    None => kaos_core::sigils::resolve_collection_module(module.as_str()),
                }
            }
            Err(error) => Err(format!("{}: {error}", path.display())),
        }
    }
}

fn collect_module_folder(
    directory: &Path,
    prefix: &str,
    depth: usize,
    modules: &mut Vec<String>,
) -> Result<(), String> {
    if depth > 32 {
        return Err(format!(
            "module folder nesting exceeds 32 at {}",
            directory.display()
        ));
    }
    let entries =
        fs::read_dir(directory).map_err(|error| format!("{}: {error}", directory.display()))?;
    let mut entries = entries
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("{}: {error}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let file_type = entry
            .file_type()
            .map_err(|error| format!("{}: {error}", entry.path().display()))?;
        let Some(name) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if file_type.is_dir() {
            collect_module_folder(
                &entry.path(),
                &format!("{prefix}/{name}"),
                depth + 1,
                modules,
            )?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|ext| ext.to_str()) == Some("rebis")
        {
            let entry_path = entry.path();
            let Some(stem) = entry_path.file_stem().and_then(|stem| stem.to_str()) else {
                continue;
            };
            let module = format!("{prefix}/{stem}");
            if ModuleName::try_from(module.as_str()).is_ok() {
                modules.push(module);
            }
        }
    }
    Ok(())
}

#[derive(Clone, Debug)]
struct Snapshot {
    source: String,
    cursor: usize,
}

/// Result of feeding one key into the Vim normal-mode command parser.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NormalAction {
    /// The key is not part of an operator or motion; the UI should handle it.
    Unhandled,
    /// A count, operator, `g`, or text-object prefix needs another key.
    Pending,
    /// A motion completed without modifying text.
    Moved,
    /// Text was deleted or otherwise changed while remaining in normal mode.
    Edited,
    /// Text was yanked.
    Yanked,
    /// A change operator completed and Vim should enter insert mode.
    EnterInsert,
}

/// Frontend-neutral key vocabulary for the shared direct/Vim editor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditKey {
    Char(char),
    Paste(String),
    Escape,
    Enter,
    Tab,
    Backspace,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
}

/// Modifier state carried with an [`EditKey`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditModifiers {
    pub ctrl: bool,
    pub shift: bool,
}

/// Observable result of one shared editor key.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditEffect {
    pub handled: bool,
    pub changed: bool,
    pub yanked: bool,
    pub command: bool,
    pub save: bool,
    pub unmatched_parenthesis: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VimOperator {
    Delete,
    Change,
    Yank,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VimMotion {
    CurrentLine,
    Left,
    Right,
    Down,
    Up,
    Word,
    BigWord,
    EndWord,
    EndBigWord,
    BackWord,
    BackBigWord,
    LineStart,
    FirstNonBlank,
    LineEnd,
    DocumentStart,
    DocumentEnd,
    LineNumber(usize),
    InnerWord,
    AWord,
    InnerBigWord,
    ABigWord,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParsedNormal {
    Motion {
        motion: VimMotion,
        count: usize,
    },
    Operator {
        operator: VimOperator,
        motion: VimMotion,
        count: usize,
        linewise: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseNormal {
    Pending,
    Complete(ParsedNormal),
    Invalid,
}

/// A Unicode-safe multiline text buffer with a Vim-like editing core.
#[derive(Clone, Debug)]
pub struct Editor {
    source: String,
    /// Cursor as a character offset, never a byte offset.
    cursor: usize,
    preferred_column: Option<usize>,
    undo: Vec<Snapshot>,
    redo: Vec<Snapshot>,
    dirty: bool,
    /// Incomplete Vim normal command, including counts and operators.
    pending_normal: String,
    visual_anchor: Option<usize>,
    yank: String,
    yank_linewise: bool,
    /// The register holds a rectangular block yank; paste re-lays it column-wise.
    yank_blockwise: bool,
    /// Vim groups all mutations between entering insert mode and Escape into
    /// one undo unit. Direct (non-Vim) editing leaves this disabled.
    insert_session: bool,
    insert_snapshot_recorded: bool,
}

impl Editor {
    /// Build an editor around `source`.
    #[must_use]
    pub fn new(source: impl Into<String>) -> Self {
        Self {
            source: source.into(),
            cursor: 0,
            preferred_column: None,
            undo: Vec::new(),
            redo: Vec::new(),
            dirty: false,
            pending_normal: String::new(),
            visual_anchor: None,
            yank: String::new(),
            yank_linewise: false,
            yank_blockwise: false,
            insert_session: false,
            insert_snapshot_recorded: false,
        }
    }

    /// Current source.
    #[must_use]
    pub fn source(&self) -> &str {
        &self.source
    }

    /// Current character offset.
    #[must_use]
    pub const fn cursor(&self) -> usize {
        self.cursor
    }

    /// Place the cursor at a character boundary. Frontends use this for pointer
    /// hit-testing; all keyboard motions still go through the shared core.
    pub fn set_cursor(&mut self, cursor: usize) {
        self.cursor = cursor.min(self.char_len());
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Current unnamed Vim register.
    #[must_use]
    pub fn yank(&self) -> &str {
        &self.yank
    }

    /// Whether the buffer differs from its last load/save.
    #[must_use]
    pub const fn dirty(&self) -> bool {
        self.dirty
    }

    /// Mark the current buffer as persisted.
    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    /// Put text in the embedded Vim yank register.
    pub fn set_yank(&mut self, text: impl Into<String>) {
        self.yank = text.into();
        self.yank_linewise = false;
        self.yank_blockwise = false;
    }

    /// Replace the entire buffer, recording one undo step.
    pub fn replace(&mut self, source: String) {
        self.remember();
        self.source = source;
        self.cursor = self.cursor.min(self.char_len());
        self.preferred_column = None;
        self.dirty = true;
    }

    fn char_len(&self) -> usize {
        self.source.chars().count()
    }

    fn byte_at(&self, char_index: usize) -> usize {
        self.source
            .char_indices()
            .nth(char_index)
            .map_or(self.source.len(), |(byte, _)| byte)
    }

    fn char_at(&self, char_index: usize) -> Option<char> {
        self.source.chars().nth(char_index)
    }

    fn remember(&mut self) {
        if self.undo.len() == 256 {
            self.undo.remove(0);
        }
        self.undo.push(Snapshot {
            source: self.source.clone(),
            cursor: self.cursor,
        });
        self.redo.clear();
        self.dirty = true;
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    fn remember_insert_edit(&mut self) {
        if self.insert_session && self.insert_snapshot_recorded {
            self.dirty = true;
            self.preferred_column = None;
            self.pending_normal.clear();
            return;
        }
        self.remember();
        if self.insert_session {
            self.insert_snapshot_recorded = true;
        }
    }

    /// Begin one Vim insert/change session. `already_changed` is true for
    /// commands such as `cw`, `s`, `o`, and visual `c`, whose deletion/opening
    /// already recorded the undo snapshot that insertion must join.
    pub fn begin_insert_session(&mut self, already_changed: bool) {
        self.insert_session = true;
        self.insert_snapshot_recorded = already_changed;
        self.pending_normal.clear();
    }

    /// Finish a Vim insert session. Vim places the normal-mode cursor on the
    /// final inserted character rather than the insertion point after it.
    pub fn end_insert_session(&mut self) {
        if self.insert_session && self.insert_snapshot_recorded && self.cursor > 0 {
            let previous = self.char_at(self.cursor - 1);
            if previous != Some('\n') {
                self.cursor -= 1;
            }
        }
        self.insert_session = false;
        self.insert_snapshot_recorded = false;
        self.pending_normal.clear();
    }

    /// Undo one mutation.
    pub fn undo(&mut self) {
        let Some(previous) = self.undo.pop() else {
            return;
        };
        self.redo.push(Snapshot {
            source: std::mem::replace(&mut self.source, previous.source),
            cursor: self.cursor,
        });
        self.cursor = previous.cursor.min(self.char_len());
        self.preferred_column = None;
        self.dirty = true;
    }

    /// Redo one mutation.
    pub fn redo(&mut self) {
        let Some(next) = self.redo.pop() else {
            return;
        };
        self.undo.push(Snapshot {
            source: std::mem::replace(&mut self.source, next.source),
            cursor: self.cursor,
        });
        self.cursor = next.cursor.min(self.char_len());
        self.preferred_column = None;
        self.dirty = true;
    }

    /// Insert one character at the cursor.
    pub fn insert(&mut self, character: char) {
        self.remember_insert_edit();
        let byte = self.byte_at(self.cursor);
        self.source.insert(byte, character);
        self.cursor += 1;
    }

    /// Insert a terminal bracketed paste as one undoable edit.
    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
        self.remember_insert_edit();
        let byte = self.byte_at(self.cursor);
        self.source.insert_str(byte, &normalized);
        self.cursor += normalized.chars().count();
    }

    /// Insert a balanced pair and leave the cursor between it.
    pub fn insert_pair(&mut self, open: char, close: char) {
        self.remember_insert_edit();
        let byte = self.byte_at(self.cursor);
        self.source.insert(byte, open);
        let after_open = self.byte_at(self.cursor + 1);
        self.source.insert(after_open, close);
        self.cursor += 1;
    }

    /// If the next cell is `close`, move over it without inserting a duplicate.
    pub fn skip_close(&mut self, close: char) -> bool {
        if self.char_at(self.cursor) == Some(close) {
            self.cursor += 1;
            true
        } else {
            false
        }
    }

    /// Delete the character before the cursor.  An empty `()` pair is removed
    /// together, matching common Vim editor behaviour.
    pub fn backspace(&mut self) {
        if self.cursor == 0 {
            return;
        }
        let paired =
            self.char_at(self.cursor - 1) == Some('(') && self.char_at(self.cursor) == Some(')');
        self.remember_insert_edit();
        let start = self.byte_at(self.cursor - 1);
        let end = self.byte_at(self.cursor + usize::from(paired));
        self.source.replace_range(start..end, "");
        self.cursor -= 1;
    }

    /// Delete the character under the cursor.
    pub fn delete(&mut self) -> bool {
        if self.cursor >= self.char_len() {
            return false;
        }
        self.remember_insert_edit();
        let start = self.byte_at(self.cursor);
        let end = self.byte_at(self.cursor + 1);
        self.source.replace_range(start..end, "");
        true
    }

    /// Move one cell left.
    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Move one cell right.
    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.char_len());
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Place Insert mode immediately after the current normal-mode character,
    /// without crossing the current line on an empty line.
    pub fn append_after_cursor(&mut self) {
        let row = self.row_col().0;
        let line_end = self.line_start(row) + self.line_len(row);
        if self.cursor < line_end {
            self.cursor += 1;
        }
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Current zero-based row and column.
    #[must_use]
    pub fn row_col(&self) -> (usize, usize) {
        let mut row = 0;
        let mut column = 0;
        for character in self.source.chars().take(self.cursor) {
            if character == '\n' {
                row += 1;
                column = 0;
            } else {
                column += 1;
            }
        }
        (row, column)
    }

    /// Number of display lines, including an empty final line after `\n`.
    #[must_use]
    pub fn line_count(&self) -> usize {
        self.source.bytes().filter(|byte| *byte == b'\n').count() + 1
    }

    fn line_start(&self, wanted: usize) -> usize {
        if wanted == 0 {
            return 0;
        }
        let mut row = 0;
        for (index, character) in self.source.chars().enumerate() {
            if character == '\n' {
                row += 1;
                if row == wanted {
                    return index + 1;
                }
            }
        }
        self.char_len()
    }

    fn line_len(&self, row: usize) -> usize {
        self.source
            .chars()
            .skip(self.line_start(row))
            .take_while(|character| *character != '\n')
            .count()
    }

    fn set_row_col(&mut self, row: usize, column: usize) {
        let row = row.min(self.line_count().saturating_sub(1));
        self.cursor = self.line_start(row) + column.min(self.line_len(row));
    }

    /// Move vertically, preserving the desired column across short lines.
    pub fn vertical(&mut self, delta: isize) {
        let (row, column) = self.row_col();
        let wanted = self.preferred_column.unwrap_or(column);
        let target = row
            .saturating_add_signed(delta)
            .min(self.line_count().saturating_sub(1));
        self.set_row_col(target, wanted);
        self.preferred_column = Some(wanted);
        self.pending_normal.clear();
    }

    /// Move to the first cell of the current line.
    pub fn line_start_motion(&mut self) {
        let (row, _) = self.row_col();
        self.set_row_col(row, 0);
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Move to the end insertion point of the current line.
    pub fn line_end(&mut self) {
        let (row, _) = self.row_col();
        self.set_row_col(row, self.line_len(row));
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Move to the final insertion point in the document.
    pub fn document_end(&mut self) {
        self.cursor = self.char_len();
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Move to the next literal occurrence after the cursor, wrapping once at
    /// the end of the buffer. Both the cursor and returned location remain
    /// character-based even when the source or query contains Unicode.
    fn find_next(&mut self, query: &str) -> Option<bool> {
        if query.is_empty() || self.source.is_empty() {
            return None;
        }
        let start_char = (self.cursor + 1).min(self.char_len());
        let start_byte = self.byte_at(start_char);
        let found = self.source[start_byte..]
            .find(query)
            .map(|offset| (start_byte + offset, false))
            .or_else(|| {
                self.source
                    .find(query)
                    .filter(|byte| *byte < start_byte)
                    .map(|byte| (byte, true))
            });
        let (byte, wrapped) = found?;
        self.cursor = self.source[..byte].chars().count();
        self.preferred_column = None;
        self.pending_normal.clear();
        Some(wrapped)
    }

    /// Move to the start of the next word.
    pub fn next_word(&mut self) {
        let chars: Vec<char> = self.source.chars().collect();
        let mut cursor = self.cursor;
        while cursor < chars.len() && is_word(chars[cursor]) {
            cursor += 1;
        }
        while cursor < chars.len() && !is_word(chars[cursor]) {
            cursor += 1;
        }
        self.cursor = cursor;
        self.preferred_column = None;
        self.pending_normal.clear();
    }

    /// Feed a character into Vim's normal-mode operator/motion grammar.
    ///
    /// Supported combinations share one range engine: counts may precede the
    /// operator or motion (`2dw`, `d2w`), `d`/`c`/`y` compose with
    /// `h l j k w W e E b B 0 ^ $ gg G`, and `iw`/`aw`/`iW`/`aW` text
    /// objects work with every operator.
    pub fn normal_key(&mut self, key: char) -> NormalAction {
        let had_pending = !self.pending_normal.is_empty();
        if !had_pending && !normal_command_starter(key) {
            return NormalAction::Unhandled;
        }
        self.pending_normal.push(key);
        match parse_normal_command(&self.pending_normal) {
            ParseNormal::Pending => NormalAction::Pending,
            ParseNormal::Invalid => {
                self.pending_normal.clear();
                if had_pending {
                    NormalAction::Pending
                } else {
                    NormalAction::Unhandled
                }
            }
            ParseNormal::Complete(command) => {
                self.pending_normal.clear();
                self.apply_normal_command(command)
            }
        }
    }

    fn apply_normal_command(&mut self, command: ParsedNormal) -> NormalAction {
        match command {
            ParsedNormal::Motion { motion, count } => {
                self.cursor = self.motion_target(motion, count);
                self.preferred_column = None;
                NormalAction::Moved
            }
            ParsedNormal::Operator {
                operator,
                motion,
                count,
                linewise,
            } => self.apply_operator(operator, motion, count, linewise),
        }
    }

    fn apply_operator(
        &mut self,
        operator: VimOperator,
        motion: VimMotion,
        count: usize,
        linewise: bool,
    ) -> NormalAction {
        let range = if matches!(
            motion,
            VimMotion::InnerWord | VimMotion::AWord | VimMotion::InnerBigWord | VimMotion::ABigWord
        ) {
            self.word_object_range(motion, count)
                .map(|(from, to)| (from, to, false))
        } else if linewise
            || matches!(
                motion,
                VimMotion::CurrentLine
                    | VimMotion::Down
                    | VimMotion::Up
                    | VimMotion::DocumentStart
                    | VimMotion::DocumentEnd
                    | VimMotion::LineNumber(_)
            )
        {
            Some(self.linewise_range(motion, count))
        } else {
            self.motion_range(operator, motion, count)
                .map(|(from, to)| (from, to, false))
        };
        let Some((from, to, is_linewise)) = range else {
            return if operator == VimOperator::Change {
                self.remember();
                NormalAction::EnterInsert
            } else {
                NormalAction::Edited
            };
        };
        if to <= from || from > self.char_len() {
            return if operator == VimOperator::Change {
                self.remember();
                NormalAction::EnterInsert
            } else {
                NormalAction::Edited
            };
        }
        let to = to.min(self.char_len());
        let from_byte = self.byte_at(from);
        let to_byte = self.byte_at(to);
        self.yank = self.source[from_byte..to_byte].to_string();
        self.yank_linewise = is_linewise;
        self.yank_blockwise = false;
        if is_linewise && !self.yank.ends_with('\n') {
            self.yank.push('\n');
        }
        if operator == VimOperator::Yank {
            // Characterwise backward yanks finish at the start of their
            // range, but `yy`/`yj` leave the cursor where it was.
            if !is_linewise {
                self.cursor = from.min(self.char_len());
            }
            return NormalAction::Yanked;
        }

        self.remember();
        // `cc`/`c2j` change line contents but preserve the final selected line
        // break so inserted text remains a line of its own. Delete operators
        // consume that break, as Vim's linewise register semantics require.
        let edit_to = if operator == VimOperator::Change
            && is_linewise
            && to > from
            && self.char_at(to - 1) == Some('\n')
        {
            self.byte_at(to - 1)
        } else {
            to_byte
        };
        self.source.replace_range(from_byte..edit_to, "");
        self.cursor = from.min(self.char_len());
        if operator == VimOperator::Change {
            NormalAction::EnterInsert
        } else {
            NormalAction::Edited
        }
    }

    fn motion_range(
        &self,
        operator: VimOperator,
        motion: VimMotion,
        count: usize,
    ) -> Option<(usize, usize)> {
        let start = self.cursor.min(self.char_len());
        if operator == VimOperator::Change && motion == VimMotion::Word {
            let chars = self.source.chars().collect::<Vec<_>>();
            if start >= chars.len() {
                return None;
            }
            let target = if chars[start].is_whitespace() {
                let mut target = start;
                for _ in 0..count.max(1) {
                    target = word_forward(&chars, target, false);
                }
                target
            } else {
                let mut target = start;
                for repetition in 0..count.max(1) {
                    target = current_word_end(&chars, target, false);
                    if repetition + 1 < count.max(1) {
                        target = (target + 1).min(chars.len());
                    }
                }
                target.saturating_add(1)
            };
            return Some((start, target.min(chars.len())));
        }
        if operator == VimOperator::Change && motion == VimMotion::BigWord {
            let chars = self.source.chars().collect::<Vec<_>>();
            if start >= chars.len() {
                return None;
            }
            let target = if chars[start].is_whitespace() {
                let mut target = start;
                for _ in 0..count.max(1) {
                    target = word_forward(&chars, target, true);
                }
                target
            } else {
                let mut target = start;
                for repetition in 0..count.max(1) {
                    target = current_word_end(&chars, target, true);
                    if repetition + 1 < count.max(1) {
                        target = (target + 1).min(chars.len());
                    }
                }
                target.saturating_add(1)
            };
            return Some((start, target.min(chars.len())));
        }
        let mut target = match motion {
            // Operator ranges end at an insertion boundary, while normal-mode
            // `l` stops on the last character. This distinction makes `x` and
            // `dl` delete the final character without crossing a line break.
            VimMotion::Right => {
                let row = self.row_col().0;
                (start + count.max(1)).min(self.line_start(row) + self.line_len(row))
            }
            VimMotion::Word | VimMotion::BigWord => {
                let chars = self.source.chars().collect::<Vec<_>>();
                let mut cursor = start;
                for _ in 0..count.max(1) {
                    cursor = word_forward(&chars, cursor, motion == VimMotion::BigWord);
                }
                cursor
            }
            _ => self.motion_target(motion, count),
        };
        // A single `dw`/`yw` at the end of a line stops at that line's end.
        // Vim only lets the `w` motion consume the line break when its count
        // reaches a word on a later line (for example, `2dw`).
        if operator != VimOperator::Change
            && count.max(1) == 1
            && matches!(motion, VimMotion::Word | VimMotion::BigWord)
            && self
                .source
                .chars()
                .skip(start)
                .take(target.saturating_sub(start))
                .any(|character| character == '\n')
        {
            let row = self.row_col().0;
            target = self.line_start(row) + self.line_len(row);
        }
        let inclusive = matches!(
            motion,
            VimMotion::EndWord | VimMotion::EndBigWord | VimMotion::LineEnd
        );
        if target >= start {
            let to = target
                .saturating_add(usize::from(inclusive))
                .min(self.char_len());
            Some((start, to))
        } else {
            Some((target, start))
        }
    }

    fn motion_target(&self, motion: VimMotion, count: usize) -> usize {
        let count = count.max(1);
        let chars = self.source.chars().collect::<Vec<_>>();
        match motion {
            VimMotion::CurrentLine => self.line_start(self.row_col().0),
            VimMotion::Left => self
                .cursor
                .saturating_sub(count)
                .max(self.line_start(self.row_col().0)),
            VimMotion::Right => {
                let row = self.row_col().0;
                (self.cursor + count).min(self.normal_line_end(row))
            }
            VimMotion::Down | VimMotion::Up => {
                let (row, column) = self.row_col();
                let target = if motion == VimMotion::Down {
                    row.saturating_add(count)
                } else {
                    row.saturating_sub(count)
                }
                .min(self.line_count().saturating_sub(1));
                self.line_start(target) + column.min(self.normal_line_column_end(target))
            }
            VimMotion::Word | VimMotion::BigWord => {
                let mut cursor = self.cursor;
                for _ in 0..count {
                    cursor = word_forward(&chars, cursor, motion == VimMotion::BigWord);
                }
                self.normal_cursor(cursor)
            }
            VimMotion::EndWord | VimMotion::EndBigWord => {
                let mut cursor = self.cursor;
                for _ in 0..count {
                    cursor = word_end_forward(&chars, cursor, motion == VimMotion::EndBigWord);
                }
                self.normal_cursor(cursor)
            }
            VimMotion::BackWord | VimMotion::BackBigWord => {
                let mut cursor = self.cursor;
                for _ in 0..count {
                    cursor = word_backward(&chars, cursor, motion == VimMotion::BackBigWord);
                }
                cursor
            }
            VimMotion::LineStart => self.line_start(self.row_col().0),
            VimMotion::FirstNonBlank => {
                let row = self.row_col().0;
                let start = self.line_start(row);
                start
                    + self
                        .source
                        .chars()
                        .skip(start)
                        .take(self.line_len(row))
                        .take_while(|character| character.is_whitespace())
                        .count()
            }
            VimMotion::LineEnd => {
                let row = (self.row_col().0 + count - 1).min(self.line_count().saturating_sub(1));
                self.normal_line_end(row)
            }
            VimMotion::DocumentStart => self.first_non_blank_on_line(0),
            VimMotion::DocumentEnd => {
                self.first_non_blank_on_line(self.line_count().saturating_sub(1))
            }
            VimMotion::LineNumber(line) => self.first_non_blank_on_line(line.saturating_sub(1)),
            VimMotion::InnerWord
            | VimMotion::AWord
            | VimMotion::InnerBigWord
            | VimMotion::ABigWord => self.cursor,
        }
    }

    fn normal_cursor(&self, target: usize) -> usize {
        let target = target.min(self.char_len());
        if target == self.char_len() && !self.source.is_empty() && !self.source.ends_with('\n') {
            target - 1
        } else {
            target
        }
    }

    fn normal_line_column_end(&self, row: usize) -> usize {
        self.line_len(row).saturating_sub(1)
    }

    fn normal_line_end(&self, row: usize) -> usize {
        self.line_start(row) + self.normal_line_column_end(row)
    }

    fn linewise_range(&self, motion: VimMotion, count: usize) -> (usize, usize, bool) {
        let row = self.row_col().0;
        let count = count.max(1);
        let target = match motion {
            VimMotion::CurrentLine => row.saturating_add(count - 1),
            VimMotion::Down => row.saturating_add(count),
            VimMotion::Up => row.saturating_sub(count),
            VimMotion::DocumentStart => 0,
            VimMotion::DocumentEnd => self.line_count().saturating_sub(1),
            VimMotion::LineNumber(line) => line.saturating_sub(1),
            _ => row,
        }
        .min(self.line_count().saturating_sub(1));
        let first = row.min(target);
        let last = row.max(target);
        let from = self.line_start(first);
        let to = if last + 1 < self.line_count() {
            self.line_start(last + 1)
        } else {
            self.char_len()
        };
        (from, to, true)
    }

    fn first_non_blank_on_line(&self, row: usize) -> usize {
        let row = row.min(self.line_count().saturating_sub(1));
        let start = self.line_start(row);
        start
            + self
                .source
                .chars()
                .skip(start)
                .take(self.line_len(row))
                .take_while(|character| character.is_whitespace())
                .count()
    }

    fn word_object_range(&self, motion: VimMotion, count: usize) -> Option<(usize, usize)> {
        let chars = self.source.chars().collect::<Vec<_>>();
        if chars.is_empty() || self.cursor >= chars.len() {
            return None;
        }
        let big = matches!(motion, VimMotion::InnerBigWord | VimMotion::ABigWord);
        let around = matches!(motion, VimMotion::AWord | VimMotion::ABigWord);
        let mut from = word_object_start(&chars, self.cursor, big);
        let mut to = word_object_end(&chars, self.cursor, big);
        for _ in 1..count.max(1) {
            let next = word_forward(&chars, to, big);
            if next >= chars.len() {
                break;
            }
            to = word_object_end(&chars, next, big);
        }
        if around {
            let trailing = to;
            while to < chars.len() && chars[to].is_whitespace() {
                to += 1;
            }
            if to == trailing {
                while from > 0 && chars[from - 1].is_whitespace() {
                    from -= 1;
                }
            }
        }
        Some((from, to))
    }

    /// Open a new line below the current line and place the cursor on it.
    pub fn open_below(&mut self) {
        let (row, _) = self.row_col();
        let insert_at = self.line_start(row) + self.line_len(row);
        self.remember();
        let mut byte = self.byte_at(insert_at);
        if self.char_at(insert_at) == Some('\n') {
            byte += 1;
        }
        self.source.insert(byte, '\n');
        self.cursor = insert_at + 1;
    }

    /// Open a new line above the current line and place the cursor on it.
    pub fn open_above(&mut self) {
        let (row, _) = self.row_col();
        let insert_at = self.line_start(row);
        self.remember();
        let byte = self.byte_at(insert_at);
        self.source.insert(byte, '\n');
        self.cursor = insert_at;
    }

    /// Implement Vim's `dd` operator. Returns true when a line was deleted.
    pub fn normal_d(&mut self) -> bool {
        self.normal_key('d') == NormalAction::Edited
    }

    /// Implement Vim's `yy` line yank.
    pub fn normal_y(&mut self) -> bool {
        self.normal_key('y') == NormalAction::Yanked
    }

    /// Delete from the cursor through the end of the current line.
    pub fn delete_to_line_end(&mut self) -> bool {
        let (row, _) = self.row_col();
        let end = self.line_start(row) + self.line_len(row);
        if end <= self.cursor {
            return false;
        }
        self.remember();
        let from = self.byte_at(self.cursor);
        let to = self.byte_at(end);
        self.yank = self.source[from..to].to_string();
        self.yank_linewise = false;
        self.yank_blockwise = false;
        self.source.replace_range(from..to, "");
        true
    }

    /// Cancel an unfinished normal-mode operator such as the first `d` in `dd`.
    pub fn clear_pending(&mut self) {
        self.pending_normal.clear();
    }

    /// Begin a character-wise or line-wise visual selection.
    pub fn begin_visual(&mut self, linewise: bool) {
        self.visual_anchor = Some(if linewise {
            self.line_start(self.row_col().0)
        } else {
            self.cursor
        });
    }

    /// Leave visual selection mode.
    pub fn end_visual(&mut self) {
        self.visual_anchor = None;
    }

    /// Inclusive character range selected by visual mode.
    #[must_use]
    pub fn visual_range(&self, linewise: bool) -> Option<(usize, usize)> {
        let anchor = self.visual_anchor?;
        if linewise {
            let anchor_row = self.row_col_at(anchor).0;
            let cursor_row = self.row_col().0;
            let first = anchor_row.min(cursor_row);
            let last = anchor_row.max(cursor_row);
            let start = self.line_start(first);
            let end = if last + 1 < self.line_count() {
                self.line_start(last + 1).saturating_sub(1)
            } else {
                self.char_len().saturating_sub(1)
            };
            Some((start, end))
        } else {
            Some((anchor.min(self.cursor), anchor.max(self.cursor)))
        }
    }

    /// Character ranges selected by the current visual mode.
    ///
    /// Character- and line-wise selections return one range. Block mode
    /// returns one range per selected line, which lets every renderer paint and
    /// copy the same rectangle without knowing editor internals.
    #[must_use]
    pub fn selection_ranges(&self, mode: Mode) -> Vec<(usize, usize)> {
        match mode {
            Mode::Visual => self
                .visual_range(false)
                .map(|(from, to)| (from, (to + 1).min(self.char_len())))
                .into_iter()
                .collect(),
            Mode::VisualLine => self
                .visual_range(true)
                .map(|(from, to)| (from, (to + 1).min(self.char_len())))
                .into_iter()
                .collect(),
            Mode::VisualBlock => {
                let Some((top, bottom, left, right)) = self.visual_block_range() else {
                    return Vec::new();
                };
                (top..=bottom)
                    .filter_map(|row| {
                        let start = self.line_start(row);
                        let width = self.line_len(row);
                        let from = start + left.min(width);
                        let to = start + right.saturating_add(1).min(width);
                        (to > from).then_some((from, to))
                    })
                    .collect()
            }
            _ => Vec::new(),
        }
    }

    /// Selected text in reading order. Rectangular selections retain one
    /// fragment per line.
    #[must_use]
    pub fn selected_text(&self, mode: Mode) -> Option<String> {
        let ranges = self.selection_ranges(mode);
        if ranges.is_empty() {
            return None;
        }
        Some(
            ranges
                .into_iter()
                .map(|(from, to)| {
                    self.source
                        .chars()
                        .skip(from)
                        .take(to.saturating_sub(from))
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    fn row_col_at(&self, cursor: usize) -> (usize, usize) {
        let mut row = 0;
        let mut column = 0;
        for character in self.source.chars().take(cursor) {
            if character == '\n' {
                row += 1;
                column = 0;
            } else {
                column += 1;
            }
        }
        (row, column)
    }

    /// Yank the active visual selection.
    pub fn yank_visual(&mut self, linewise: bool) {
        if let Some((start, end)) = self.visual_range(linewise) {
            let from = self.byte_at(start);
            let to = self.byte_at((end + 1).min(self.char_len()));
            self.yank = self.source[from..to].to_string();
            self.yank_linewise = linewise;
            self.yank_blockwise = false;
            if linewise && !self.yank.ends_with('\n') {
                self.yank.push('\n');
            }
        }
        self.end_visual();
    }

    /// Delete and yank the active visual selection.
    pub fn delete_visual(&mut self, linewise: bool) {
        let Some((start, end)) = self.visual_range(linewise) else {
            return;
        };
        let from = self.byte_at(start);
        let to = self.byte_at((end + 1).min(self.char_len()));
        self.yank = self.source[from..to].to_string();
        self.yank_linewise = linewise;
        self.yank_blockwise = false;
        if linewise && !self.yank.ends_with('\n') {
            self.yank.push('\n');
        }
        self.remember();
        self.source.replace_range(from..to, "");
        self.cursor = start.min(self.char_len());
        self.end_visual();
    }

    /// Replace the active visual selection with the unnamed register, as Vim
    /// does for visual `p`/`P`. The replaced text becomes the new unnamed
    /// register and the whole replacement is one undo step.
    pub fn paste_visual(&mut self, linewise: bool) {
        if self.yank.is_empty() {
            return;
        }
        let Some((start, end)) = self.visual_range(linewise) else {
            return;
        };
        let from = self.byte_at(start);
        let to = self.byte_at((end + 1).min(self.char_len()));
        let mut replaced = self.source[from..to].to_string();
        if linewise && !replaced.ends_with('\n') {
            replaced.push('\n');
        }
        let replacement = self.yank.clone();
        let replacement_linewise = self.yank_linewise;
        let replacement_len = replacement.chars().count();
        self.remember();
        self.source.replace_range(from..to, &replacement);
        self.cursor = if replacement_linewise {
            start
        } else {
            start + replacement_len.saturating_sub(1)
        }
        .min(self.char_len());
        self.yank = replaced;
        self.yank_linewise = linewise;
        self.yank_blockwise = false;
        self.end_visual();
    }

    /// Paste the last visual yank after the cursor.
    pub fn paste_after(&mut self) {
        if self.yank.is_empty() {
            return;
        }
        if self.yank_blockwise {
            self.paste_block(true);
            return;
        }
        if self.yank_linewise {
            let row = self.row_col().0;
            let at = if row + 1 < self.line_count() {
                self.line_start(row + 1)
            } else {
                self.char_len()
            };
            let mut text = String::new();
            if at == self.char_len() && !self.source.is_empty() && !self.source.ends_with('\n') {
                text.push('\n');
            }
            text.push_str(&self.yank);
            self.remember();
            let byte = self.byte_at(at);
            self.source.insert_str(byte, &text);
            self.cursor = at + usize::from(text.starts_with('\n'));
            return;
        }
        self.remember();
        let at = (self.cursor + usize::from(self.cursor < self.char_len())).min(self.char_len());
        let byte = self.byte_at(at);
        self.source.insert_str(byte, &self.yank);
        self.cursor = at + self.yank.chars().count().saturating_sub(1);
    }

    /// Paste the last yank before the cursor.
    pub fn paste_before(&mut self) {
        if self.yank.is_empty() {
            return;
        }
        if self.yank_blockwise {
            self.paste_block(false);
            return;
        }
        if self.yank_linewise {
            let at = self.line_start(self.row_col().0);
            self.remember();
            let byte = self.byte_at(at);
            self.source.insert_str(byte, &self.yank);
            self.cursor = at;
            return;
        }
        self.remember();
        let byte = self.byte_at(self.cursor);
        self.source.insert_str(byte, &self.yank);
        self.cursor += self.yank.chars().count().saturating_sub(1);
    }

    /// Begin a block-wise (rectangular column) visual selection at the cursor.
    pub fn begin_visual_block(&mut self) {
        self.visual_anchor = Some(self.cursor);
    }

    /// The rectangle covered by the active block selection, as
    /// `(top_row, bottom_row, left_col, right_col)`, all inclusive. The
    /// columns come from the anchor and cursor columns regardless of which
    /// corner the cursor sits in, so the block stays rectangular as it moves.
    #[must_use]
    pub fn visual_block_range(&self) -> Option<(usize, usize, usize, usize)> {
        let anchor = self.visual_anchor?;
        let (anchor_row, anchor_col) = self.row_col_at(anchor);
        let (cursor_row, cursor_col) = self.row_col();
        Some((
            anchor_row.min(cursor_row),
            anchor_row.max(cursor_row),
            anchor_col.min(cursor_col),
            anchor_col.max(cursor_col),
        ))
    }

    /// The text inside a block rectangle: each row's column slice, clipped to
    /// that row's actual length (short lines contribute fewer characters, as in
    /// Vim), joined by newlines.
    fn block_text(&self, top: usize, bottom: usize, left: usize, right: usize) -> String {
        let chars: Vec<char> = self.source.chars().collect();
        (top..=bottom)
            .map(|row| {
                let start = self.line_start(row);
                let len = self.line_len(row);
                let from = left.min(len);
                let to = (right + 1).min(len);
                chars[start + from..start + to].iter().collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Yank the active block selection into the register, tagged block-wise so
    /// a later paste re-lays it as a column rather than a run of text.
    pub fn yank_visual_block(&mut self) {
        if let Some((top, bottom, left, right)) = self.visual_block_range() {
            self.yank = self.block_text(top, bottom, left, right);
            self.yank_linewise = false;
            self.yank_blockwise = true;
            self.cursor = self.line_start(top) + left.min(self.line_len(top));
        }
        self.end_visual();
    }

    /// Delete the active block selection, cutting the same column range out of
    /// every spanned row and leaving the cursor at the block's top-left cell.
    pub fn delete_visual_block(&mut self) {
        let Some((top, bottom, left, right)) = self.visual_block_range() else {
            return;
        };
        self.yank = self.block_text(top, bottom, left, right);
        self.yank_linewise = false;
        self.yank_blockwise = true;
        self.remember();
        // Cut from the bottom row up so each row's start index stays valid for
        // the rows still to be processed above it.
        for row in (top..=bottom).rev() {
            let len = self.line_len(row);
            if left >= len {
                continue;
            }
            let start = self.line_start(row);
            let from = self.byte_at(start + left);
            let to = self.byte_at(start + (right + 1).min(len));
            self.source.replace_range(from..to, "");
        }
        self.cursor = self.line_start(top) + left.min(self.line_len(top));
        self.end_visual();
    }

    /// Replace a rectangular visual selection with the unnamed register.
    ///
    /// A block register is re-laid row by row; a character/line register is
    /// inserted once at the rectangle's top-left. The removed rectangle
    /// becomes the new block register and the entire put is one undo step.
    pub fn paste_visual_block(&mut self) {
        if self.yank.is_empty() {
            return;
        }
        let Some((top, bottom, left, right)) = self.visual_block_range() else {
            return;
        };
        let replacement = self.yank.clone();
        let replacement_linewise = self.yank_linewise;
        let replacement_blockwise = self.yank_blockwise;
        let replaced = self.block_text(top, bottom, left, right);
        self.remember();
        for row in (top..=bottom).rev() {
            let len = self.line_len(row);
            if left >= len {
                continue;
            }
            let start = self.line_start(row);
            let from = self.byte_at(start + left);
            let to = self.byte_at(start + (right + 1).min(len));
            self.source.replace_range(from..to, "");
        }
        self.cursor = self.line_start(top) + left.min(self.line_len(top));
        self.end_visual();
        self.yank = replacement;
        self.yank_linewise = replacement_linewise;
        self.yank_blockwise = replacement_blockwise;
        if replacement_blockwise {
            self.paste_block_contents(false);
        } else {
            let byte = self.byte_at(self.cursor);
            self.source.insert_str(byte, &self.yank);
            self.cursor += self.yank.chars().count().saturating_sub(1);
        }
        self.yank = replaced;
        self.yank_linewise = false;
        self.yank_blockwise = true;
    }

    /// Paste a block-wise register: insert row `i` of the block at the cursor
    /// column on the `i`-th line at/after the cursor, padding short lines with
    /// spaces so the column stays aligned, and appending new lines when the
    /// block runs past the end of the buffer. `after` shifts the insertion one
    /// column right, matching `p` versus `P`.
    fn paste_block(&mut self, after: bool) {
        self.remember();
        self.paste_block_contents(after);
    }

    fn paste_block_contents(&mut self, after: bool) {
        let fragments: Vec<String> = self.yank.split('\n').map(str::to_string).collect();
        let (row, column) = self.row_col();
        let target = column + usize::from(after && self.line_len(row) > 0);
        for (offset, fragment) in fragments.iter().enumerate() {
            let line = row + offset;
            if line >= self.line_count() {
                self.source.push('\n');
            }
            let len = self.line_len(line);
            if target > len {
                let pad = " ".repeat(target - len);
                let byte = self.byte_at(self.line_start(line) + len);
                self.source.insert_str(byte, &pad);
            }
            let byte = self.byte_at(self.line_start(line) + target);
            self.source.insert_str(byte, fragment);
        }
        self.cursor = self.line_start(row) + target;
    }

    /// Move to the matching structural parenthesis at/adjacent to the cursor.
    pub fn jump_matching_parenthesis(&mut self) -> bool {
        let Some((left, right)) = self.matching_parentheses() else {
            return false;
        };
        self.cursor = if self.cursor == left { right } else { left };
        self.preferred_column = None;
        true
    }

    /// Matching structural parentheses or mediator brackets at the cursor.
    #[must_use]
    pub fn matching_parentheses(&self) -> Option<(usize, usize)> {
        matching_form(&self.source, self.cursor)
    }
}

/// Apply one key to the shared source editor.
///
/// Both terminal and visual frontends call this exact state machine. When Vim
/// is disabled it supplies direct editing with pairs and ordinary navigation;
/// when enabled it owns insert, normal, visual, visual-line, visual-block,
/// registers, operators, counts, motions, and undo/redo.
pub fn handle_edit_key(
    editor: &mut Editor,
    mode: &mut Mode,
    vim_enabled: bool,
    key: EditKey,
    modifiers: EditModifiers,
) -> EditEffect {
    let before = editor.source.clone();
    let mut effect = EditEffect {
        handled: true,
        ..EditEffect::default()
    };

    if modifiers.ctrl && matches!(key, EditKey::Char('s' | 'S')) {
        effect.save = true;
        return effect;
    }

    if !vim_enabled {
        *mode = Mode::Insert;
        match key {
            EditKey::Char(character) if !modifiers.ctrl => insert_character(editor, character),
            EditKey::Paste(text) => editor.insert_text(&text),
            EditKey::Enter => editor.insert('\n'),
            EditKey::Tab => editor.insert_text("  "),
            EditKey::Backspace => editor.backspace(),
            EditKey::Delete => {
                editor.delete();
            }
            EditKey::Left => editor.left(),
            EditKey::Right => editor.right(),
            EditKey::Up => editor.vertical(-1),
            EditKey::Down => editor.vertical(1),
            EditKey::Home => editor.line_start_motion(),
            EditKey::End => editor.line_end(),
            EditKey::Char(_) | EditKey::Escape => effect.handled = false,
        }
        effect.changed = before != editor.source;
        return effect;
    }

    // Ctrl-[ is Vim's portable Escape chord.
    let key = if modifiers.ctrl && matches!(key, EditKey::Char('[')) {
        EditKey::Escape
    } else {
        key
    };

    match *mode {
        Mode::Insert => match key {
            EditKey::Escape => {
                editor.end_insert_session();
                *mode = Mode::Normal;
                editor.clear_pending();
            }
            EditKey::Char('c' | 'C') if modifiers.ctrl => {
                editor.end_insert_session();
                *mode = Mode::Normal;
                editor.clear_pending();
            }
            EditKey::Char(character) if !modifiers.ctrl => insert_character(editor, character),
            EditKey::Paste(text) => editor.insert_text(&text),
            EditKey::Enter => editor.insert('\n'),
            EditKey::Tab => editor.insert_text("  "),
            EditKey::Backspace => editor.backspace(),
            EditKey::Delete => {
                editor.delete();
            }
            EditKey::Left => editor.left(),
            EditKey::Right => editor.right(),
            EditKey::Up => editor.vertical(-1),
            EditKey::Down => editor.vertical(1),
            EditKey::Home => editor.line_start_motion(),
            EditKey::End => editor.line_end(),
            EditKey::Char(_) => effect.handled = false,
        },
        Mode::Normal => {
            let normal = if let EditKey::Char(character) = key {
                editor.normal_key(character)
            } else {
                NormalAction::Unhandled
            };
            match normal {
                NormalAction::Pending | NormalAction::Moved | NormalAction::Edited => {}
                NormalAction::Yanked => effect.yanked = true,
                NormalAction::EnterInsert => {
                    editor.begin_insert_session(true);
                    *mode = Mode::Insert;
                }
                NormalAction::Unhandled => match key {
                    EditKey::Char(':') => {
                        *mode = Mode::Command;
                        editor.clear_pending();
                        effect.command = true;
                    }
                    EditKey::Char('i') => {
                        editor.begin_insert_session(false);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('v') if modifiers.ctrl => {
                        editor.begin_visual_block();
                        *mode = Mode::VisualBlock;
                    }
                    EditKey::Char('V' | 'v') if modifiers.shift => {
                        editor.begin_visual(true);
                        *mode = Mode::VisualLine;
                    }
                    EditKey::Char('v') => {
                        editor.begin_visual(false);
                        *mode = Mode::Visual;
                    }
                    EditKey::Char('a') => {
                        editor.append_after_cursor();
                        editor.begin_insert_session(false);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('I') => {
                        let _ = editor.normal_key('^');
                        editor.begin_insert_session(false);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('A') => {
                        editor.line_end();
                        editor.begin_insert_session(false);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('o') => {
                        editor.open_below();
                        editor.begin_insert_session(true);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('O') => {
                        editor.open_above();
                        editor.begin_insert_session(true);
                        *mode = Mode::Insert;
                    }
                    EditKey::Left => {
                        let _ = editor.normal_key('h');
                    }
                    EditKey::Right => {
                        let _ = editor.normal_key('l');
                    }
                    EditKey::Down => {
                        let _ = editor.normal_key('j');
                    }
                    EditKey::Up => {
                        let _ = editor.normal_key('k');
                    }
                    EditKey::Home => {
                        let _ = editor.normal_key('0');
                    }
                    EditKey::End => {
                        let _ = editor.normal_key('$');
                    }
                    EditKey::Char('x') | EditKey::Delete => {
                        editor.delete();
                    }
                    EditKey::Char('p') => editor.paste_after(),
                    EditKey::Char('P') => editor.paste_before(),
                    EditKey::Char('D') => {
                        editor.delete_to_line_end();
                    }
                    EditKey::Char('C') => {
                        let changed = editor.delete_to_line_end();
                        editor.begin_insert_session(changed);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('s') => {
                        let changed = editor.delete();
                        editor.begin_insert_session(changed);
                        *mode = Mode::Insert;
                    }
                    EditKey::Char('u') if !modifiers.ctrl => editor.undo(),
                    EditKey::Char('r' | 'R') if modifiers.ctrl => editor.redo(),
                    EditKey::Char('%') => {
                        effect.unmatched_parenthesis = !editor.jump_matching_parenthesis();
                    }
                    EditKey::Escape => editor.clear_pending(),
                    EditKey::Paste(_)
                    | EditKey::Enter
                    | EditKey::Tab
                    | EditKey::Backspace
                    | EditKey::Char(_) => {
                        editor.clear_pending();
                        effect.handled = false;
                    }
                },
            }
        }
        Mode::Visual | Mode::VisualLine => {
            let linewise = *mode == Mode::VisualLine;
            match key {
                EditKey::Char('V' | 'v') if modifiers.shift => {
                    editor.begin_visual(true);
                    *mode = Mode::VisualLine;
                }
                EditKey::Escape | EditKey::Char('v') => {
                    editor.end_visual();
                    *mode = Mode::Normal;
                }
                EditKey::Char(character) if visual_motion(character) => {
                    let _ = editor.normal_key(character);
                }
                EditKey::Left => {
                    let _ = editor.normal_key('h');
                }
                EditKey::Right => {
                    let _ = editor.normal_key('l');
                }
                EditKey::Down => {
                    let _ = editor.normal_key('j');
                }
                EditKey::Up => {
                    let _ = editor.normal_key('k');
                }
                EditKey::Home => {
                    let _ = editor.normal_key('0');
                }
                EditKey::End => {
                    let _ = editor.normal_key('$');
                }
                EditKey::Char('y') => {
                    editor.yank_visual(linewise);
                    *mode = Mode::Normal;
                    effect.yanked = true;
                }
                EditKey::Char('d' | 'x') | EditKey::Delete => {
                    editor.delete_visual(linewise);
                    *mode = Mode::Normal;
                }
                EditKey::Char('c') => {
                    editor.delete_visual(linewise);
                    editor.begin_insert_session(true);
                    *mode = Mode::Insert;
                }
                EditKey::Char('p' | 'P') => {
                    editor.paste_visual(linewise);
                    *mode = Mode::Normal;
                }
                EditKey::Char(':') => {
                    *mode = Mode::Command;
                    effect.command = true;
                }
                EditKey::Paste(_)
                | EditKey::Enter
                | EditKey::Tab
                | EditKey::Backspace
                | EditKey::Char(_) => effect.handled = false,
            }
        }
        Mode::VisualBlock => match key {
            EditKey::Escape => {
                editor.end_visual();
                *mode = Mode::Normal;
            }
            EditKey::Char('v') if modifiers.ctrl => {
                editor.end_visual();
                *mode = Mode::Normal;
            }
            EditKey::Char('V' | 'v') if modifiers.shift => {
                editor.begin_visual(true);
                *mode = Mode::VisualLine;
            }
            EditKey::Char('v') => {
                editor.begin_visual(false);
                *mode = Mode::Visual;
            }
            EditKey::Char(character) if visual_motion(character) => {
                let _ = editor.normal_key(character);
            }
            EditKey::Left => {
                let _ = editor.normal_key('h');
            }
            EditKey::Right => {
                let _ = editor.normal_key('l');
            }
            EditKey::Down => {
                let _ = editor.normal_key('j');
            }
            EditKey::Up => {
                let _ = editor.normal_key('k');
            }
            EditKey::Home => {
                let _ = editor.normal_key('0');
            }
            EditKey::End => {
                let _ = editor.normal_key('$');
            }
            EditKey::Char('y') => {
                editor.yank_visual_block();
                *mode = Mode::Normal;
                effect.yanked = true;
            }
            EditKey::Char('d' | 'x') | EditKey::Delete => {
                editor.delete_visual_block();
                *mode = Mode::Normal;
            }
            EditKey::Char('c') => {
                editor.delete_visual_block();
                editor.begin_insert_session(true);
                *mode = Mode::Insert;
            }
            EditKey::Char('p' | 'P') => {
                editor.paste_visual_block();
                *mode = Mode::Normal;
            }
            EditKey::Char(':') => {
                *mode = Mode::Command;
                effect.command = true;
            }
            EditKey::Paste(_)
            | EditKey::Enter
            | EditKey::Tab
            | EditKey::Backspace
            | EditKey::Char(_) => effect.handled = false,
        },
        Mode::Command | Mode::KaosCommand => effect.handled = false,
    }

    effect.changed = before != editor.source;
    effect
}

fn insert_character(editor: &mut Editor, character: char) {
    match character {
        '(' => editor.insert_pair('(', ')'),
        '[' => editor.insert_pair('[', ']'),
        '"' if editor.skip_close('"') => {}
        '"' => editor.insert_pair('"', '"'),
        ')' if editor.skip_close(')') => {}
        ']' if editor.skip_close(']') => {}
        _ => editor.insert(character),
    }
}

fn visual_motion(character: char) -> bool {
    character.is_ascii_digit()
        || matches!(
            character,
            'h' | 'l' | 'j' | 'k' | 'w' | 'W' | 'e' | 'E' | 'b' | 'B' | '^' | '$' | 'g' | 'G'
        )
}

fn is_word(character: char) -> bool {
    character.is_alphanumeric() || matches!(character, '_' | '-' | '/')
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum WordClass {
    Space,
    Keyword,
    Punctuation,
}

fn word_class(character: char, big: bool) -> WordClass {
    if character.is_whitespace() {
        WordClass::Space
    } else if big || is_word(character) {
        WordClass::Keyword
    } else {
        WordClass::Punctuation
    }
}

fn word_forward(chars: &[char], cursor: usize, big: bool) -> usize {
    if cursor >= chars.len() {
        return chars.len();
    }
    let class = word_class(chars[cursor], big);
    let mut at = cursor;
    if class != WordClass::Space {
        while at < chars.len() && word_class(chars[at], big) == class {
            at += 1;
        }
    }
    while at < chars.len() && word_class(chars[at], big) == WordClass::Space {
        at += 1;
    }
    at
}

fn word_backward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() || cursor == 0 {
        return 0;
    }
    let mut at = cursor.min(chars.len());
    while at > 0 && word_class(chars[at - 1], big) == WordClass::Space {
        at -= 1;
    }
    if at == 0 {
        return 0;
    }
    let class = word_class(chars[at - 1], big);
    while at > 0 && word_class(chars[at - 1], big) == class {
        at -= 1;
    }
    at
}

/// End of the word currently under `cursor`, used by Vim's special `cw`
/// semantics (which differ from applying the ordinary `w` motion).
fn current_word_end(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len().saturating_sub(1));
    while at < chars.len() && word_class(chars[at], big) == WordClass::Space {
        at += 1;
    }
    if at >= chars.len() {
        return chars.len().saturating_sub(1);
    }
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}

/// Target of Vim's `e`/`E` motion. When already on a word's final character,
/// the motion advances to the end of the following word.
fn word_end_forward(chars: &[char], cursor: usize, big: bool) -> usize {
    if chars.is_empty() {
        return 0;
    }
    let mut at = cursor.min(chars.len().saturating_sub(1));
    if word_class(chars[at], big) != WordClass::Space
        && (at + 1 == chars.len() || word_class(chars[at + 1], big) != word_class(chars[at], big))
    {
        at = (at + 1).min(chars.len());
    }
    while at < chars.len() && word_class(chars[at], big) == WordClass::Space {
        at += 1;
    }
    if at >= chars.len() {
        return chars.len().saturating_sub(1);
    }
    let class = word_class(chars[at], big);
    while at + 1 < chars.len() && word_class(chars[at + 1], big) == class {
        at += 1;
    }
    at
}

fn word_object_start(chars: &[char], cursor: usize, big: bool) -> usize {
    let class = word_class(chars[cursor], big);
    let mut at = cursor;
    while at > 0 && word_class(chars[at - 1], big) == class {
        at -= 1;
    }
    at
}

fn word_object_end(chars: &[char], cursor: usize, big: bool) -> usize {
    let class = word_class(chars[cursor], big);
    let mut at = cursor;
    while at < chars.len() && word_class(chars[at], big) == class {
        at += 1;
    }
    at
}

fn normal_command_starter(key: char) -> bool {
    key.is_ascii_digit()
        || matches!(
            key,
            'd' | 'c'
                | 'y'
                | 'h'
                | 'l'
                | 'j'
                | 'k'
                | 'w'
                | 'W'
                | 'e'
                | 'E'
                | 'b'
                | 'B'
                | '^'
                | '$'
                | 'g'
                | 'G'
                | 'x'
                | 's'
        )
}

fn parse_count(chars: &[char], at: &mut usize) -> usize {
    if *at >= chars.len() || !matches!(chars[*at], '1'..='9') {
        return 1;
    }
    let mut count = 0usize;
    while *at < chars.len() && chars[*at].is_ascii_digit() {
        count = count
            .saturating_mul(10)
            .saturating_add(chars[*at].to_digit(10).unwrap_or_default() as usize);
        *at += 1;
    }
    count.max(1)
}

fn parse_motion(chars: &[char], operator: bool) -> ParseNormalMotion {
    match chars {
        [] => ParseNormalMotion::Pending,
        ['g'] => ParseNormalMotion::Pending,
        ['g', 'g'] => ParseNormalMotion::Complete(VimMotion::DocumentStart),
        ['i'] | ['a'] if operator => ParseNormalMotion::Pending,
        ['i', 'w'] if operator => ParseNormalMotion::Complete(VimMotion::InnerWord),
        ['a', 'w'] if operator => ParseNormalMotion::Complete(VimMotion::AWord),
        ['i', 'W'] if operator => ParseNormalMotion::Complete(VimMotion::InnerBigWord),
        ['a', 'W'] if operator => ParseNormalMotion::Complete(VimMotion::ABigWord),
        [key] => match key {
            'h' => ParseNormalMotion::Complete(VimMotion::Left),
            'l' => ParseNormalMotion::Complete(VimMotion::Right),
            'j' => ParseNormalMotion::Complete(VimMotion::Down),
            'k' => ParseNormalMotion::Complete(VimMotion::Up),
            'w' => ParseNormalMotion::Complete(VimMotion::Word),
            'W' => ParseNormalMotion::Complete(VimMotion::BigWord),
            'e' => ParseNormalMotion::Complete(VimMotion::EndWord),
            'E' => ParseNormalMotion::Complete(VimMotion::EndBigWord),
            'b' => ParseNormalMotion::Complete(VimMotion::BackWord),
            'B' => ParseNormalMotion::Complete(VimMotion::BackBigWord),
            '0' => ParseNormalMotion::Complete(VimMotion::LineStart),
            '^' => ParseNormalMotion::Complete(VimMotion::FirstNonBlank),
            '$' => ParseNormalMotion::Complete(VimMotion::LineEnd),
            'G' => ParseNormalMotion::Complete(VimMotion::DocumentEnd),
            _ => ParseNormalMotion::Invalid,
        },
        _ => ParseNormalMotion::Invalid,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ParseNormalMotion {
    Pending,
    Complete(VimMotion),
    Invalid,
}

fn parse_normal_command(source: &str) -> ParseNormal {
    let chars = source.chars().collect::<Vec<_>>();
    let mut at = 0usize;
    let outer_count_start = at;
    let outer_count = parse_count(&chars, &mut at);
    let outer_count_explicit = at > outer_count_start;
    if at == chars.len() {
        return ParseNormal::Pending;
    }
    let operator = match chars[at] {
        'd' => Some(VimOperator::Delete),
        'c' => Some(VimOperator::Change),
        'y' => Some(VimOperator::Yank),
        _ => None,
    };
    if let Some(operator) = operator {
        at += 1;
        if at == chars.len() {
            return ParseNormal::Pending;
        }
        let inner_count_start = at;
        let inner_count = parse_count(&chars, &mut at);
        let inner_count_explicit = at > inner_count_start;
        if at == chars.len() {
            return ParseNormal::Pending;
        }
        if chars[at]
            == source
                .chars()
                .find(|character| matches!(character, 'd' | 'c' | 'y'))
                .unwrap()
            && at + 1 == chars.len()
        {
            return ParseNormal::Complete(ParsedNormal::Operator {
                operator,
                motion: VimMotion::CurrentLine,
                count: outer_count.saturating_mul(inner_count),
                linewise: true,
            });
        }
        return match parse_motion(&chars[at..], true) {
            ParseNormalMotion::Pending => ParseNormal::Pending,
            ParseNormalMotion::Invalid => ParseNormal::Invalid,
            ParseNormalMotion::Complete(motion) => {
                let motion = if matches!(motion, VimMotion::DocumentStart | VimMotion::DocumentEnd)
                    && (inner_count_explicit || outer_count_explicit)
                {
                    VimMotion::LineNumber(if inner_count_explicit {
                        inner_count
                    } else {
                        outer_count
                    })
                } else {
                    motion
                };
                ParseNormal::Complete(ParsedNormal::Operator {
                    operator,
                    motion,
                    count: outer_count.saturating_mul(inner_count),
                    linewise: matches!(
                        motion,
                        VimMotion::Down
                            | VimMotion::Up
                            | VimMotion::DocumentStart
                            | VimMotion::DocumentEnd
                            | VimMotion::LineNumber(_)
                    ),
                })
            }
        };
    }
    if at + 1 == chars.len() && matches!(chars[at], 'x' | 's') {
        return ParseNormal::Complete(ParsedNormal::Operator {
            operator: if chars[at] == 'x' {
                VimOperator::Delete
            } else {
                VimOperator::Change
            },
            motion: VimMotion::Right,
            count: outer_count,
            linewise: false,
        });
    }
    match parse_motion(&chars[at..], false) {
        ParseNormalMotion::Pending => ParseNormal::Pending,
        ParseNormalMotion::Invalid => ParseNormal::Invalid,
        ParseNormalMotion::Complete(motion) => {
            let motion = if matches!(motion, VimMotion::DocumentStart | VimMotion::DocumentEnd)
                && outer_count_explicit
            {
                VimMotion::LineNumber(outer_count)
            } else {
                motion
            };
            ParseNormal::Complete(ParsedNormal::Motion {
                motion,
                count: outer_count,
            })
        }
    }
}

/// The inclusive char range `[start, end]` of `source` as a string.
fn slice_chars(source: &str, start: usize, end: usize) -> String {
    source.chars().skip(start).take(end + 1 - start).collect()
}

/// Every folder path that contains `name`. `std/x` → [`std`]; `a/b/c` →
/// [`a`, `a/b`]. A name with no folder yields nothing.
fn folder_ancestors(name: &str) -> Vec<String> {
    let mut ancestors = Vec::new();
    let mut prefix = String::new();
    let segments: Vec<&str> = name.split('/').collect();
    for segment in &segments[..segments.len().saturating_sub(1)] {
        if !prefix.is_empty() {
            prefix.push('/');
        }
        prefix.push_str(segment);
        ancestors.push(prefix.clone());
    }
    ancestors
}

/// The display colour of one lexical token. A prompt is split so its delimiter
/// quotes read as structure while its body reads as prompt text; `character`
/// and `byte` locate the current char within the token to make that call.
fn highlight_for(
    token: rebis_lang::Token,
    source: &str,
    byte: usize,
    character: char,
) -> Highlight {
    use rebis_lang::TokenKind;
    match token.kind {
        TokenKind::Paren => Highlight::Parenthesis,
        // Brackets and the prefix operators are all structural punctuation.
        TokenKind::Bracket
        | TokenKind::Brace
        | TokenKind::Source
        | TokenKind::Meta
        | TokenKind::Star
        | TokenKind::At
        | TokenKind::Bar
        | TokenKind::Tilde
        | TokenKind::Quote
        | TokenKind::Unquote
        | TokenKind::Dollar
        | TokenKind::Question
        | TokenKind::Bang
        | TokenKind::Equals
        | TokenKind::Load
        | TokenKind::Plus
        | TokenKind::Amp
        | TokenKind::Percent => Highlight::Mediate,
        TokenKind::Invert => Highlight::Invert,
        TokenKind::Model => Highlight::Model,
        TokenKind::Forward => Highlight::Forward,
        TokenKind::Backflow => Highlight::Backflow,
        TokenKind::Comment => Highlight::Comment,
        TokenKind::Whitespace => Highlight::Whitespace,
        TokenKind::Invalid => Highlight::Invalid,
        // `#` is a symbol to the lexer; the editor tints the import head.
        TokenKind::Symbol if &source[token.start..token.end] == "#" => Highlight::Import,
        TokenKind::Symbol => Highlight::Atom,
        TokenKind::Prompt => {
            let opening = byte == token.start;
            let closing =
                character == '"' && byte != token.start && byte + character.len_utf8() == token.end;
            if opening || closing {
                Highlight::Atom
            } else {
                Highlight::Prompt
            }
        }
    }
}

/// Assign a display colour to every character of the source.
///
/// The lexical boundaries come from `rebis_lang::tokens` — the language's own
/// tokenizer — so the editor and the parser can never disagree about what is a
/// symbol, an operator, or a prompt. Only the colour policy lives here;
/// validity remains exclusively `rebis_lang::parse`'s job. The returned vector
/// has one entry per source character, in order.
#[must_use]
pub fn highlights(source: &str) -> Vec<Highlight> {
    let mut output = Vec::with_capacity(source.chars().count());
    let mut tokens = rebis_lang::tokens(source).into_iter().peekable();
    for (byte, character) in source.char_indices() {
        // Tokens are contiguous and cover the whole source, so advancing past
        // any whose span ends at or before this byte lands on the one holding it.
        while tokens.peek().is_some_and(|token| byte >= token.end) {
            tokens.next();
        }
        output.push(match tokens.peek() {
            Some(token) => highlight_for(*token, source, byte, character),
            None => Highlight::Invalid,
        });
    }
    output
}

/// A validated request waiting for KAOS's model-host adapter.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RunRequest {
    pub source: String,
    pub input: String,
    pub scope: RunScope,
}

/// What the user asked the host to evaluate. Shared with every frontend so a
/// visual run card and terminal run tree use one typed scope.
pub use kaos_core::run_model::Scope as RunScope;

/// Result of a completed `:` command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceAction {
    None,
    /// Leave the editor while retaining its complete in-memory workspace.
    Suspend,
    /// Leave and intentionally discard the retained workspace.
    Discard,
    Run(RunRequest),
    /// Start immediately alongside existing Rebis or chat subprocesses.
    RunParallel(RunRequest),
    /// Reveal and focus Kaos's retained run browser.
    BrowseRuns,
    /// Open the source-bound supervisory conversation in the right panel.
    OpenSigilChat,
    /// Send one turn to the supervisory agent without leaving the workspace.
    SigilChat(String),
    /// Ask a retained Rebis run about its current or completed execution.
    RunChat {
        id: u64,
        question: String,
    },
    /// Execute a session-level Kaos command without leaving the editor.
    Kaos(String),
}

/// Durable-sigil lifecycle events that require app-owned run/checkpoint state.
/// The editor owns files and identity; the TUI host owns live subprocesses.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceEvent {
    SigilSaved(String),
    SigilOpened(String),
}

/// Complete state for one Rebis editor/visualizer session.
pub struct Workspace {
    pub editor: Editor,
    pub mode: Mode,
    pub vim_enabled: bool,
    pub command: String,
    pub command_choice: usize,
    pub path: Option<PathBuf>,
    pub message: String,
    pub view_top: usize,
    pub view_left: usize,
    /// Keyboard editing follows the cursor; mouse-wheel review deliberately
    /// detaches the source viewport until the next source action.
    source_follow_cursor: bool,
    pub visualization: Visualization,
    /// The program as sound. Shares every rule with the visual editor's Sound
    /// tab — same scale, same synth, same player — because both are this one
    /// desk wearing a different face.
    pub music: Desk,
    pub panel_visible: bool,
    /// The run browser is a panel projection, not ownership of execution.
    /// Switching this off never pauses or clears app-owned runs.
    pub runs_visible: bool,
    pub graph_focus: bool,
    pub graph_top: usize,
    pub graph_left: usize,
    /// Selected row in the sigil browser (index into the visible rows).
    pub sigil_choice: usize,
    /// Folder paths currently expanded in the browser (e.g. `std`,
    /// `team/reviews`). Folders are collapsed by default; Tab toggles them.
    expanded_folders: BTreeSet<String>,
    /// A char range captured when `Ctrl-K` is pressed in visual mode, so the
    /// next `/run` evaluates just the selection. Consumed by the run command.
    pub run_selection: Option<(usize, usize)>,
    /// The visualization panel's inner rectangle from the last draw
    /// (x, y, width, height); None while the panel is hidden. Lets a mouse
    /// click map to the sigil row under the pointer.
    pub panel_inner: Option<(u16, u16, u16, u16)>,
    /// Whether normal mode has received the `Ctrl-W` window prefix.
    pub window_prefix: bool,
    /// Whether the next editor key bypasses command bindings and inserts literally.
    pub literal_next: bool,
    chaos_star_visible: bool,
    cwd: PathBuf,
    sigils_root: PathBuf,
    compiled: Option<Expr>,
    canonical: Option<String>,
    record: Option<Record>,
    record_text: Option<String>,
    /// Complete stream for the current/most recent hosted run. Panel
    /// navigation only changes the projection flags below; it never clears
    /// this retained execution state.
    run_output: RetainedLog,
    run_output_visible: bool,
    result_only_visible: bool,
    final_output: Option<String>,
    /// Last successfully completed output for this sigil. Unlike the visible
    /// output pane, beginning another run does not erase this saved value.
    saved_output: Option<String>,
    /// Last literal source query, retained so `/search` repeats it.
    last_search: Option<String>,
    /// Source-bound supervisory conversation. It is deliberately part of the
    /// workspace so switching views never sacrifices its context.
    sigil_chat_visible: bool,
    sigil_chat_input: String,
    sigil_chat_lines: Vec<String>,
    sigil_chat_expanded: BTreeSet<usize>,
    sigil_chat_busy: bool,
    sigil_chat_run_id: Option<u64>,
    sigil_results: Vec<SigilEntry>,
    /// Where a search matched inside a sigil, when it did: the one-based line
    /// and the line itself. A name-only match has no entry here.
    ///
    /// Kept beside the results rather than inside [`SigilEntry`] because the
    /// entry is an identity — it sorts, groups into folders, and is what
    /// opening acts on — and where a query happened to match is not part of
    /// that identity.
    sigil_hits: BTreeMap<SigilEntry, (usize, String)>,
    temporary_sigils: Vec<TemporarySigil>,
    active_temporary_sigil: Option<u64>,
    current_sigil: Option<String>,
    host_events: VecDeque<WorkspaceEvent>,
    next_temporary_sigil: u64,
    diagnostic: Option<String>,
    error_char: Option<usize>,
}

impl Workspace {
    /// Open an existing source file or create an unnamed empty buffer.
    pub fn open(cwd: PathBuf, path: Option<&str>) -> Result<Self, String> {
        let vim_enabled = load_vim_setting();
        let sigils_root = HypersigilModules::user(cwd.clone()).root().to_path_buf();
        let (source, path, chaos_star_visible) =
            if let Some(path) = path.filter(|value| !value.is_empty()) {
                let path = resolve(&cwd, path);
                if path.exists() {
                    let source = fs::read_to_string(&path)
                        .map_err(|error| format!("could not open {}: {error}", path.display()))?;
                    (source, Some(path), false)
                } else {
                    (String::new(), Some(path), true)
                }
            } else {
                // A fresh session is genuinely empty behind the splash. The
                // first interaction only dismisses the star; it cannot reveal,
                // edit, or execute hidden source.
                (String::new(), None, true)
            };
        let mut workspace = Self {
            editor: Editor::new(source),
            mode: if vim_enabled {
                Mode::Normal
            } else {
                Mode::Insert
            },
            vim_enabled,
            command: String::new(),
            command_choice: 0,
            path,
            message: String::new(),
            view_top: 0,
            view_left: 0,
            source_follow_cursor: true,
            visualization: Visualization::Mandala,
            music: Desk::default(),
            panel_visible: true,
            runs_visible: false,
            graph_focus: false,
            graph_top: 0,
            graph_left: 0,
            sigil_choice: 0,
            expanded_folders: BTreeSet::new(),
            run_selection: None,
            panel_inner: None,
            window_prefix: false,
            literal_next: false,
            chaos_star_visible,
            cwd,
            sigils_root,
            compiled: None,
            canonical: None,
            record: None,
            record_text: None,
            run_output: RetainedLog::default(),
            run_output_visible: false,
            result_only_visible: false,
            final_output: None,
            saved_output: None,
            last_search: None,
            sigil_chat_visible: false,
            sigil_chat_input: String::new(),
            sigil_chat_lines: Vec::new(),
            sigil_chat_expanded: BTreeSet::new(),
            sigil_chat_busy: false,
            sigil_chat_run_id: None,
            sigil_results: Vec::new(),
            sigil_hits: BTreeMap::new(),
            temporary_sigils: Vec::new(),
            active_temporary_sigil: None,
            current_sigil: None,
            host_events: VecDeque::new(),
            next_temporary_sigil: 1,
            diagnostic: None,
            error_char: None,
        };
        workspace.refresh();
        // The sigil explorer is the panel's first view: the library (yours and
        // the embedded std) is browsable the moment the workspace opens. Focus
        // stays in the source editor so typing works immediately.
        workspace.search_sigils("");
        workspace.graph_focus = false;
        workspace.message.clear();
        Ok(workspace)
    }

    /// Recompile after a mutation. This is the sole source of graph data and
    /// diagnostics in the UI.
    pub fn refresh(&mut self) {
        // Source can only become non-empty through an explicit mutation. Lift
        // the splash in that case; non-editing interactions use
        // `dismiss_chaos_star` and are consumed by the TUI.
        if self.chaos_star_visible && !self.editor.source().is_empty() {
            self.chaos_star_visible = false;
        }
        if self.chaos_star_visible {
            self.compiled = None;
            self.canonical = None;
            self.diagnostic = None;
            self.error_char = None;
            return;
        }
        if self.editor.source().is_empty() {
            self.compiled = None;
            self.canonical = None;
            self.diagnostic = None;
            self.error_char = None;
            return;
        }
        match rebis_lang::parse(self.editor.source()) {
            Ok(expr) => {
                self.canonical = Some(rebis_lang::format(&expr));
                self.compiled = Some(expr);
                self.diagnostic = None;
                self.error_char = None;
            }
            Err(error) => {
                self.error_char = error
                    .offset
                    .map(|offset| byte_to_char(self.editor.source(), offset));
                self.diagnostic = Some(error.to_string());
                self.compiled = None;
                self.canonical = None;
            }
        }
    }

    /// Whether the transient chaos-star overlay still covers a new buffer.
    #[must_use]
    pub const fn chaos_star_visible(&self) -> bool {
        self.chaos_star_visible
    }

    /// Remove the transient splash after the first source-editor interaction.
    /// Returns whether this call dismissed it, allowing the TUI to consume the
    /// interaction instead of also applying it to the editor underneath.
    pub fn dismiss_chaos_star(&mut self) -> bool {
        if self.chaos_star_visible {
            self.chaos_star_visible = false;
            self.refresh();
            if self.message.is_empty() {
                self.message =
                    "live compile · /help Kaos commands · :w/:e/:q Vim commands".to_string();
            }
            return true;
        }
        false
    }

    /// Current compiler diagnostic.
    #[must_use]
    pub fn diagnostic(&self) -> Option<&str> {
        self.diagnostic.as_deref()
    }

    /// Character at which the compiler reported an error, when located.
    #[must_use]
    pub const fn error_char(&self) -> Option<usize> {
        self.error_char
    }

    /// Canonical source, available only while the buffer compiles.
    #[must_use]
    pub fn canonical(&self) -> Option<&str> {
        self.canonical.as_deref()
    }

    /// Human path label for the title bar.
    #[must_use]
    pub fn path_label(&self) -> String {
        self.path.as_deref().map_or_else(
            || "[untitled]".to_string(),
            |path| path.display().to_string(),
        )
    }

    #[must_use]
    pub fn current_sigil(&self) -> Option<&str> {
        self.current_sigil.as_deref()
    }

    /// Host-owned resumable state lives beside, but never inside, importable
    /// Rebis source. Both names derive from an already validated sigil name.
    #[must_use]
    pub fn sigil_resume_paths(&self, name: &str) -> (PathBuf, PathBuf) {
        (
            self.sigils_dir().join(format!("{name}.run")),
            self.sigils_dir().join(format!("{name}.checkpoint")),
        )
    }

    /// Point the sigil explorer at a temporary library.
    ///
    /// Exposed rather than `#[cfg(test)]` because the terminal app's own tests
    /// live in another crate now and cannot see this one's test build.
    #[doc(hidden)]
    pub fn set_sigils_root_for_test(&mut self, root: PathBuf) {
        self.sigils_root = root;
    }

    pub fn take_host_event(&mut self) -> Option<WorkspaceEvent> {
        self.host_events.pop_front()
    }

    /// Reveal the source-bound supervisor without leaving or replacing the
    /// editor. The transcript belongs to this workspace and survives view
    /// changes just like the run tree.
    pub fn open_sigil_chat(&mut self) {
        self.panel_visible = true;
        self.runs_visible = false;
        self.run_output_visible = false;
        self.result_only_visible = false;
        self.sigil_chat_visible = true;
        self.graph_focus = true;
        self.graph_left = 0;
        if self.sigil_chat_lines.is_empty() {
            self.sigil_chat_lines.extend([
                "GOD CHANNEL".to_string(),
                "The supervisor sees every live bot's source, input, checkpoint, directive, state, and full trace. One run remains the bound source-edit target; explicit per-run controls can pause, resume, apply guidance, or clear guidance without killing a run.".to_string(),
                String::new(),
            ]);
        }
        self.graph_top = self.sigil_chat_lines.len().saturating_sub(1);
        self.message = "sigil chat · type in the right panel · Enter sends · Esc returns to source"
            .to_string();
    }

    #[must_use]
    pub const fn sigil_chat_visible(&self) -> bool {
        self.sigil_chat_visible
    }

    pub fn hide_sigil_chat(&mut self) {
        self.sigil_chat_visible = false;
    }

    fn reset_sigil_chat(&mut self) {
        self.sigil_chat_visible = false;
        self.sigil_chat_input.clear();
        self.sigil_chat_lines.clear();
        self.sigil_chat_expanded.clear();
        self.sigil_chat_busy = false;
        self.sigil_chat_run_id = None;
    }

    #[must_use]
    pub const fn sigil_chat_busy(&self) -> bool {
        self.sigil_chat_busy
    }

    #[must_use]
    pub const fn sigil_chat_run_id(&self) -> Option<u64> {
        self.sigil_chat_run_id
    }

    pub fn bind_sigil_chat_run(&mut self, run_id: Option<u64>) {
        let binding_already_rendered = self.sigil_chat_lines.iter().any(|line| {
            line.starts_with("system  bound to resumable run #")
                || line.starts_with("system  source-only channel")
        });
        if self.sigil_chat_run_id == run_id && binding_already_rendered {
            return;
        }
        self.sigil_chat_run_id = run_id;
        self.push_sigil_chat_line(match run_id {
            Some(id) => format!("system  bound to resumable run #{id}"),
            None => "system  source-only channel · no unfinished run is bound".to_string(),
        });
    }

    #[must_use]
    pub fn sigil_chat_lines(&self) -> &[String] {
        &self.sigil_chat_lines
    }

    /// Whether one streamed model-work section is expanded in the terminal
    /// projection. The line index is stable for the lifetime of this channel.
    #[must_use]
    pub fn sigil_chat_fold_expanded(&self, line: usize) -> bool {
        self.sigil_chat_expanded.contains(&line)
    }

    /// Toggle the newest fold, or all folds when `all` is requested. Fold
    /// sections are collapsed by default so a long supervisor trace does not
    /// bury the source-bound conversation.
    pub fn toggle_sigil_chat_fold(&mut self, all: bool) {
        let folds = self
            .sigil_chat_lines
            .iter()
            .enumerate()
            .filter_map(|(index, line)| {
                matches!(
                    kaos_core::fold::classify(line),
                    kaos_core::fold::Marker::Open(_)
                )
                .then_some(index)
            })
            .collect::<Vec<_>>();
        if folds.is_empty() {
            return;
        }
        if all {
            let any_collapsed = folds
                .iter()
                .any(|index| !self.sigil_chat_expanded.contains(index));
            if any_collapsed {
                self.sigil_chat_expanded.extend(folds);
            } else {
                self.sigil_chat_expanded.clear();
            }
        } else if let Some(index) = folds.last() {
            if !self.sigil_chat_expanded.remove(index) {
                self.sigil_chat_expanded.insert(*index);
            }
        }
    }

    /// Number of rows the collapsed/expanded channel occupies in the terminal
    /// panel, used to keep scrolling bounded to what is actually visible.
    #[must_use]
    pub fn sigil_chat_visible_rows(&self) -> usize {
        let mut visible = 0usize;
        let mut stack = Vec::new();
        for (index, line) in self.sigil_chat_lines.iter().enumerate() {
            match kaos_core::fold::classify(line) {
                kaos_core::fold::Marker::Open(_) => {
                    let parent_visible = stack.last().copied().unwrap_or(true);
                    let this_visible = parent_visible;
                    if this_visible {
                        visible += 1;
                    }
                    stack.push(this_visible && self.sigil_chat_expanded.contains(&index));
                }
                kaos_core::fold::Marker::Close => {
                    stack.pop();
                }
                kaos_core::fold::Marker::Line(_) => {
                    if stack.last().copied().unwrap_or(true) {
                        visible += 1;
                    }
                }
            }
        }
        visible.max(1)
    }

    #[must_use]
    pub fn sigil_chat_input(&self) -> &str {
        &self.sigil_chat_input
    }

    pub fn insert_sigil_chat_char(&mut self, character: char) {
        self.sigil_chat_input.push(character);
    }

    pub fn backspace_sigil_chat(&mut self) {
        self.sigil_chat_input.pop();
    }

    pub fn clear_sigil_chat_input(&mut self) {
        self.sigil_chat_input.clear();
    }

    /// Consume a non-empty user turn. Keeping this at the workspace boundary
    /// makes Enter atomic: a busy channel cannot duplicate a submission.
    pub fn take_sigil_chat_message(&mut self) -> Option<String> {
        if self.sigil_chat_busy {
            self.message = "god agent is still working".to_string();
            return None;
        }
        let message = self.sigil_chat_input.trim().to_string();
        if message.is_empty() {
            return None;
        }
        self.sigil_chat_input.clear();
        self.push_sigil_chat_line(format!("you     {message}"));
        Some(message)
    }

    pub fn set_sigil_chat_busy(&mut self, busy: bool) {
        self.sigil_chat_busy = busy;
        self.message = if busy {
            "god agent working · bound run paused · peer bot snapshot stays live".to_string()
        } else {
            "sigil chat ready · Enter sends · /runs then p resumes an already-paused run"
                .to_string()
        };
    }

    pub fn push_sigil_chat_line(&mut self, line: impl Into<String>) {
        let line = line.into();
        if line.is_empty() {
            self.sigil_chat_lines.push(String::new());
        } else {
            self.sigil_chat_lines
                .extend(line.lines().map(str::to_string));
        }
        self.graph_top = self.sigil_chat_lines.len().saturating_sub(1);
    }

    /// Keep the cursor in the editor viewport.
    pub fn ensure_visible(&mut self, rows: usize, columns: usize) {
        let rows = rows.max(1);
        let (row, _) = self.editor.row_col();
        if row < self.view_top {
            self.view_top = row;
        } else if row >= self.view_top + rows {
            self.view_top = row + 1 - rows;
        }
        // The view scrolls in SOURCE lines but the pane is filled with SCREEN
        // rows, and a wrapped line occupies several. Counting only lines would let
        // the cursor sit below the pane whenever the lines above it wrap — the
        // longer the file's lines, the further off-screen. So walk the wrapped rows
        // and pull the top down until the cursor's own row fits.
        let wrapped = wrapped_rows(self.editor.source(), columns);
        let (cursor_row, _) = row_of_offset(&wrapped, self.editor.cursor());
        while let Some(top) = wrapped.iter().position(|row| row.line >= self.view_top) {
            if cursor_row < top + rows {
                break;
            }
            // One source line at a time, so the top of the pane is always the
            // start of a line rather than the middle of a wrapped one.
            self.view_top += 1;
        }
        // No horizontal scrolling: overflow goes under, so there is nothing to
        // the right to reach.
        self.view_left = 0;
    }

    /// Whether rendering should keep the source cursor inside the viewport.
    #[must_use]
    pub const fn source_follows_cursor(&self) -> bool {
        self.source_follow_cursor
    }

    /// Reattach the source viewport after a keyboard action or explicit jump.
    pub fn follow_source_cursor(&mut self) {
        self.source_follow_cursor = true;
    }

    /// Scroll source text without moving the editing cursor. The viewport is
    /// clamped to real content so repeated wheel events cannot create a long
    /// blank region that appears stuck while scrolling back.
    pub fn scroll_source_vertical(&mut self, delta: isize, visible_rows: usize) {
        let max_top = self.editor.line_count().saturating_sub(visible_rows.max(1));
        self.view_top = self
            .view_top
            .min(max_top)
            .saturating_add_signed(delta)
            .min(max_top);
        self.source_follow_cursor = false;
    }

    /// Horizontally scroll source text without moving the editing cursor.
    pub fn scroll_source_horizontal(&mut self, delta: isize, visible_columns: usize) {
        let longest = self
            .editor
            .source()
            .lines()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or_default();
        let max_left = longest.saturating_sub(visible_columns.max(1));
        self.view_left = self
            .view_left
            .min(max_left)
            .saturating_add_signed(delta)
            .min(max_left);
        self.source_follow_cursor = false;
    }

    /// Execute the current command-line contents.
    pub fn execute_command(&mut self) -> WorkspaceAction {
        let command = self.command.trim().to_string();
        self.command.clear();
        self.mode = self.editing_mode();
        if command.is_empty() {
            return WorkspaceAction::None;
        }
        if command == "q" || command == "quit" {
            if self.editor.dirty() {
                self.message = "unsaved changes · use :q! to discard or :w".to_string();
                return WorkspaceAction::None;
            }
            return WorkspaceAction::Suspend;
        }
        if command == "q!" {
            return WorkspaceAction::Discard;
        }
        if command == "wq" {
            return if self.save(None).is_ok() {
                WorkspaceAction::Suspend
            } else {
                WorkspaceAction::None
            };
        }
        if command == "w" {
            let _ = self.save(None);
            return WorkspaceAction::None;
        }
        if let Some(path) = command.strip_prefix("w ").map(str::trim) {
            let _ = self.save(Some(path));
            return WorkspaceAction::None;
        }
        if let Some(path) = command.strip_prefix("e ").map(str::trim) {
            self.load(path);
            return WorkspaceAction::None;
        }
        self.message = format!("unknown Vim command :{command}");
        WorkspaceAction::None
    }

    /// Execute a Kaos workspace command entered after `/`.
    pub fn execute_kaos_command(&mut self) -> WorkspaceAction {
        let command = self.command.trim().to_string();
        self.command.clear();
        self.mode = self.editing_mode();
        // A visual selection only survives for the command typed right after
        // it; any other command clears it.
        let run_selection = self.run_selection.take();
        match command.as_str() {
            "chat" => return WorkspaceAction::Suspend,
            _ if command.starts_with("chat run ") => {
                let mut parts = command.splitn(4, char::is_whitespace);
                let _ = parts.next();
                let _ = parts.next();
                let Some(raw_id) = parts.next().filter(|value| !value.is_empty()) else {
                    self.message = "usage: /chat run ID QUESTION".to_string();
                    return WorkspaceAction::None;
                };
                let Ok(id) = raw_id.parse::<u64>() else {
                    self.message = "chat run: ID must be a number".to_string();
                    return WorkspaceAction::None;
                };
                let Some(question) = parts.next().map(str::trim).filter(|value| !value.is_empty())
                else {
                    self.message = "usage: /chat run ID QUESTION".to_string();
                    return WorkspaceAction::None;
                };
                return WorkspaceAction::RunChat {
                    id,
                    question: question.to_string(),
                };
            }
            "runs" => return WorkspaceAction::BrowseRuns,
            "model" | "new" | "clear" | "quit" | "mouse" | "chaos" | "chaos on"
            | "chaos off" | "config" | "config restore" | "theme" | "theme dark"
            | "theme light" => return WorkspaceAction::Kaos(command),
            "format" | "fmt" => {
                if self.compiled.is_none() {
                    self.message = "cannot format until the program compiles".to_string();
                } else if self.source_has_comment() {
                    // Formatting reparses and would drop `;` comments. Require
                    // an explicit `/format!` to confirm the loss.
                    self.message =
                        "format drops ; comments — use /format! to confirm".to_string();
                } else if let Some(expr) = &self.compiled {
                    self.editor.replace(rebis_lang::pretty_format(expr));
                    self.refresh();
                    self.message = "formatted canonical Rebis".to_string();
                }
            }
            "format!" | "fmt!" => {
                if let Some(expr) = &self.compiled {
                    self.editor.replace(rebis_lang::pretty_format(expr));
                    self.refresh();
                    self.message = "formatted canonical Rebis (comments dropped)".to_string();
                } else {
                    self.message = "cannot format until the program compiles".to_string();
                }
            }
            "save" => {
                let _ = self.save(None);
            }
            "vim" | "vim on" => {
                self.set_vim_enabled(true);
                self.message =
                    "Vim mode enabled for this workspace · /vim always persists".to_string();
            }
            "vim off" => {
                self.set_vim_enabled(false);
                self.message = "direct editing enabled · /vim always persists Vim mode".to_string();
            }
            "vim toggle" => {
                self.set_vim_enabled(!self.vim_enabled);
                self.message = if self.vim_enabled {
                    "Vim mode enabled for this workspace · /vim always persists"
                } else {
                    "direct editing enabled · /vim never persists"
                }
                .to_string();
            }
            "vim always" => match save_vim_setting(true) {
                Ok(()) => {
                    self.set_vim_enabled(true);
                    self.message = "Vim mode enabled and saved in ~/.config/kaos/config".to_string();
                }
                Err(error) => self.message = error,
            },
            "vim never" => match save_vim_setting(false) {
                Ok(()) => {
                    self.set_vim_enabled(false);
                    self.message = "direct editing saved in ~/.config/kaos/config".to_string();
                }
                Err(error) => self.message = error,
            },
            _ if command.starts_with("save ") => {
                let path = command.trim_start_matches("save ").trim().to_string();
                let _ = self.save(Some(&path));
            }
            "mandala" => {
                self.sigil_chat_visible = false;
                self.visualization = Visualization::Mandala;
                self.panel_visible = true;
                self.runs_visible = false;
                self.run_output_visible = false;
                self.result_only_visible = false;
                self.message = "mandala view".to_string();
            }
            // `/visual open` draws the saved sigil from disk; plain `/visual`
            // draws the buffer, including unsaved edits.
            "visual open" => match self.path.clone() {
                Some(path) => match std::fs::read_to_string(&path) {
                    Ok(text) => self.open_visual_with(&text),
                    Err(error) => self.message = format!("visual · {error}"),
                },
                None => self.message = "visual · this sigil has no file — /visual".to_string(),
            },
            "visual" => {
                let source = self.editor.source().trim().to_string();
                self.open_visual_with(&source);
            }
            _ if command == "dream" || command.starts_with("dream ") => {
                let cwd = self.cwd.clone();
                self.message = dream_command(&cwd, command.trim_start_matches("dream").trim());
            }
            _ if command == "music" || command.starts_with("music ") => {
                self.music_command(command.trim_start_matches("music").trim());
            }
            "tree" => {
                self.sigil_chat_visible = false;
                self.visualization = Visualization::Tree;
                self.panel_visible = true;
                self.runs_visible = false;
                self.run_output_visible = false;
                self.result_only_visible = false;
                self.message = "expression tree view".to_string();
            }
            "sigils" => self.search_sigils(""),
            _ if command.starts_with("sigils ") => {
                self.search_sigils(command.trim_start_matches("sigils ").trim())
            }
            "sigil chat" => {
                self.open_sigil_chat();
                return WorkspaceAction::OpenSigilChat;
            }
            _ if command.starts_with("sigil save ") => {
                let name = command.trim_start_matches("sigil save ").trim();
                // `std/` is the embedded standard library's reserved namespace:
                // the language never consults the sigil tree for it, so a file
                // there would be dead weight that looks load-bearing.
                if name == "std" || name.starts_with("std/") {
                    self.message =
                        "std/ is the embedded standard library — pick another name".to_string();
                } else {
                    self.save_sigil(name);
                }
            }
            _ if command.starts_with("sigil open ") => {
                let name = command.trim_start_matches("sigil open ").trim();
                self.open_sigil(name);
            }
            "panel" | "panel toggle" => {
                self.panel_visible = !self.panel_visible;
                if !self.panel_visible {
                    self.graph_focus = false;
                }
                self.message = if self.panel_visible {
                    "right panel shown"
                } else {
                    "right panel hidden"
                }
                .to_string();
            }
            "panel hide" => {
                self.panel_visible = false;
                self.graph_focus = false;
                self.message = "right panel hidden".to_string();
            }
            "panel show" => {
                self.panel_visible = true;
                self.message = "right panel shown".to_string();
            }
            "graph" | "focus graph" => {
                self.panel_visible = true;
                self.graph_focus = true;
                self.message = "right panel focus · Esc or /source returns".to_string();
            }
            "source" | "focus source" => {
                self.graph_focus = false;
                self.message = "source focus".to_string();
            }
            "search" => self.search_source(None),
            _ if command.starts_with("search ") => {
                self.search_source(Some(command.trim_start_matches("search ").trim()))
            }
            "run" | "run parallel" => {
                let parallel = command == "run parallel";
                // A selection captured on entering command mode runs as a
                // block; otherwise the whole buffer runs.
                if let Some((start, end)) = run_selection {
                    let slice = slice_chars(self.editor.source(), start, end);
                    return self.run_block(&slice, parallel, None);
                }
                if self.compiled.is_none() {
                    self.message = "run refused: fix the diagnostic".to_string();
                    return WorkspaceAction::None;
                }
                let request = RunRequest {
                    source: self.editor.source().to_string(),
                    input: self.record_text.clone().unwrap_or_default(),
                    scope: RunScope::Program,
                };
                return if parallel {
                    WorkspaceAction::RunParallel(request)
                } else {
                    WorkspaceAction::Run(request)
                };
            }
            "run block" | "run block parallel" => {
                let parallel = command == "run block parallel";
                // The complete form the cursor sits on (or just after) — like
                // Lisp's eval-sexp-at-point. Place the cursor at the block's
                // end and run this.
                let Some((left, right)) = self.editor.matching_parentheses() else {
                    self.message =
                        "run block: put the cursor on the block's ( ) or [ ]".to_string();
                    return WorkspaceAction::None;
                };
                let slice = slice_chars(self.editor.source(), left, right);
                return self.run_block(&slice, parallel, None);
            }
            "output" => {
                self.sigil_chat_visible = false;
                self.panel_visible = true;
                self.runs_visible = false;
                self.graph_focus = true;
                self.graph_top = 0;
                self.graph_left = 0;
                self.run_output_visible = false;
                self.result_only_visible = true;
            }
            "output copy" => {
                if let Some(output) = self.final_output.clone() {
                    self.editor.set_yank(output);
                    self.message = "final Rebis output copied to Vim yank register · use p to paste".to_string();
                } else {
                    self.message = "no final Rebis output yet · use /run".to_string();
                }
            }
            _ if command.starts_with("output write ") => {
                let requested = command.trim_start_matches("output write ").trim();
                if let Some(output) = &self.final_output {
                    let path = resolve(&self.cwd, requested);
                    match fs::write(&path, output) {
                        Ok(()) => self.message = format!("wrote Rebis output to {}", path.display()),
                        Err(error) => self.message = format!("could not write {}: {error}", path.display()),
                    }
                } else {
                    self.message = "no final Rebis output yet · use /run".to_string();
                }
            }
            _ if command.starts_with("record ") => {
                let path = resolve(&self.cwd, command.trim_start_matches("record ").trim());
                match fs::read_to_string(&path) {
                    Ok(text) => {
                        self.record = Some(Record::from_texts(std::slice::from_ref(&text)));
                        self.record_text = Some(text);
                        self.message = format!("loaded record {}", path.display());
                    }
                    Err(error) => {
                        self.message = format!("could not read {}: {error}", path.display())
                    }
                }
            }
            _ if command.starts_with("model ") || command.starts_with("mouse ") => {
                return WorkspaceAction::Kaos(command)
            }
            "help" | "" => {
                self.message =
                    "/chat [run ID QUESTION] /config [restore] /model [MODEL] /chaos [on|off] /new /clear /quit /run [block|parallel] /runs /save [FILE] /vim toggle|on|off|always|never /search [TEXT] /output [copy|write FILE] /theme dark|light /mandala /visual [open] /tree /dream [forget N|all] /music [play|stop|export FILE|root HZ|pace S|partials N|octaves N|timbre NAME] /sigils [QUERY] /sigil save|open NAME /sigil chat /panel toggle|hide|show /graph /source /format[!] /mouse [on|off] /record FILE"
                        .to_string()
            }
            _ => self.message = format!("unknown Kaos command /{command}"),
        }
        WorkspaceAction::None
    }

    fn search_source(&mut self, query: Option<&str>) {
        self.graph_focus = false;
        self.follow_source_cursor();
        let query = query
            .filter(|query| !query.is_empty())
            .map(str::to_string)
            .or_else(|| self.last_search.clone());
        let Some(query) = query else {
            self.message = "search: enter text, or repeat after a previous /search".to_string();
            return;
        };
        self.last_search = Some(query.clone());
        let Some(wrapped) = self.editor.find_next(&query) else {
            self.message = format!("search: {query:?} not found");
            return;
        };
        let (row, column) = self.editor.row_col();
        self.message = format!(
            "{}match {query:?} at {}:{} · /search repeats",
            if wrapped { "wrapped · " } else { "" },
            row + 1,
            column + 1
        );
    }

    const fn editing_mode(&self) -> Mode {
        if self.vim_enabled {
            Mode::Normal
        } else {
            Mode::Insert
        }
    }

    fn set_vim_enabled(&mut self, enabled: bool) {
        if self.vim_enabled && self.mode == Mode::Insert {
            self.editor.end_insert_session();
        }
        self.editor.end_visual();
        self.editor.clear_pending();
        self.vim_enabled = enabled;
        self.mode = self.editing_mode();
    }

    fn save(&mut self, requested: Option<&str>) -> Result<(), ()> {
        let path = requested
            .filter(|path| !path.is_empty())
            .map(|path| resolve(&self.cwd, path))
            .or_else(|| self.path.clone());
        let Some(path) = path else {
            self.message = "no file name · use :w path.rebis".to_string();
            return Err(());
        };
        match fs::write(&path, self.editor.source()) {
            Ok(()) => {
                self.path = Some(path.clone());
                self.editor.mark_clean();
                self.remove_active_temporary_sigil();
                self.current_sigil = None;
                let message = format!(
                    "wrote {} bytes to {}",
                    self.editor.source().len(),
                    path.display()
                );
                self.refresh_sigils_if_visible(message);
                Ok(())
            }
            Err(error) => {
                self.message = format!("could not write {}: {error}", path.display());
                Err(())
            }
        }
    }

    fn sigils_dir(&self) -> PathBuf {
        self.sigils_root.clone()
    }

    fn temporary_label(&self) -> String {
        self.current_sigil
            .clone()
            .or_else(|| {
                self.path
                    .as_ref()
                    .and_then(|path| path.file_name())
                    .and_then(|name| name.to_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "untitled".to_string())
    }

    fn park_current_as_temporary_sigil(&mut self) -> Option<u64> {
        if !self.editor.dirty() {
            return None;
        }
        let source = self.editor.source().to_string();
        let label = self.temporary_label();
        if let Some(id) = self.active_temporary_sigil {
            if let Some(temporary) = self
                .temporary_sigils
                .iter_mut()
                .find(|temporary| temporary.id == id)
            {
                temporary.source = source;
                temporary.path.clone_from(&self.path);
                temporary.label = label;
                temporary.final_output.clone_from(&self.final_output);
                temporary.saved_output.clone_from(&self.saved_output);
                return Some(id);
            }
        }
        let id = self.next_temporary_sigil;
        self.next_temporary_sigil += 1;
        self.temporary_sigils.push(TemporarySigil {
            id,
            label,
            source,
            path: self.path.clone(),
            final_output: self.final_output.clone(),
            saved_output: self.saved_output.clone(),
        });
        Some(id)
    }

    fn remove_active_temporary_sigil(&mut self) {
        if let Some(id) = self.active_temporary_sigil.take() {
            self.temporary_sigils.retain(|temporary| temporary.id != id);
        }
    }

    /// Move the sigil-browser selection by `delta` rows and keep it inside
    /// the visible window (the list starts one row below the SIGILS header).
    /// The browser rendered as a collapsible tree: folders (collapsed by
    /// default) and the leaves inside expanded ones, in source order.
    fn visible_sigil_rows(&self) -> Vec<VisibleRow> {
        // Temporaries have no folder — they are always-visible top-level
        // leaves, kept first, as `sigil_results` already orders them.
        let mut rows = Vec::new();
        let mut path_bearing: Vec<&SigilEntry> = Vec::new();
        for entry in &self.sigil_results {
            match entry {
                SigilEntry::Temporary { id, label } => rows.push(VisibleRow::Leaf {
                    entry: entry.clone(),
                    depth: 0,
                    label: format!("temp:{id}  * {label} (unsaved)"),
                }),
                _ => path_bearing.push(entry),
            }
        }
        self.build_folder_rows(&path_bearing, "", 0, &mut rows);
        rows
    }

    /// Recursively lay out the path-bearing entries as folders and leaves.
    /// `prefix` is the folder path consumed so far (empty at the root).
    fn build_folder_rows(
        &self,
        entries: &[&SigilEntry],
        prefix: &str,
        depth: usize,
        rows: &mut Vec<VisibleRow>,
    ) {
        // The part of an entry's path below the consumed prefix.
        fn rest<'a>(entry: &'a SigilEntry, prefix: &str) -> &'a str {
            entry.path().unwrap_or("").get(prefix.len()..).unwrap_or("")
        }
        let mut index = 0;
        while index < entries.len() {
            let head = rest(entries[index], prefix).split('/').next().unwrap_or("");
            // Gather the run of entries sharing this head segment.
            let mut end = index;
            while end < entries.len() && rest(entries[end], prefix).split('/').next() == Some(head)
            {
                end += 1;
            }
            let group = &entries[index..end];
            index = end;

            // A group is a folder when any member has a deeper segment.
            let folder_path = format!("{prefix}{head}");
            let has_children = group.iter().any(|entry| rest(entry, prefix).contains('/'));
            if has_children {
                let expanded = self.expanded_folders.contains(&folder_path);
                rows.push(VisibleRow::Folder {
                    path: folder_path.clone(),
                    depth,
                    expanded,
                });
                if expanded {
                    self.build_folder_rows(group, &format!("{folder_path}/"), depth + 1, rows);
                }
            }
            // Any member that ends exactly here is a leaf at this depth.
            for entry in group {
                if !rest(entry, prefix).contains('/') {
                    let embedded = matches!(entry, SigilEntry::Std(_));
                    let mut label = if embedded {
                        format!("{head}  (embedded)")
                    } else {
                        head.to_string()
                    };
                    // A content match says where and what, so the browser
                    // answers the question without being opened first.
                    if let Some((number, line)) = self.sigil_hits.get(entry) {
                        label.push_str(&format!("  :{number}  {}", truncate(line, 52)));
                    }
                    rows.push(VisibleRow::Leaf {
                        entry: (*entry).clone(),
                        depth,
                        label,
                    });
                }
            }
        }
    }

    pub fn move_sigil_choice(&mut self, delta: isize) {
        let count = self.visible_sigil_rows().len();
        if count == 0 {
            return;
        }
        self.sigil_choice = self
            .sigil_choice
            .saturating_add_signed(delta)
            .min(count - 1);
        let row = self.sigil_choice + 1; // +1: the SIGILS header line
        if row < self.graph_top {
            self.graph_top = row;
        }
        if let Some((_, _, _, height)) = self.panel_inner {
            let height = height.max(1) as usize;
            if row >= self.graph_top + height {
                self.graph_top = row + 1 - height;
            }
        }
    }

    /// Toggle the folder under the selection, if the selected row is one.
    /// Returns true when a folder was toggled.
    pub fn toggle_selected_folder(&mut self) -> bool {
        let rows = self.visible_sigil_rows();
        if let Some(VisibleRow::Folder { path, expanded, .. }) = rows.get(self.sigil_choice) {
            if *expanded {
                self.expanded_folders.remove(path);
            } else {
                self.expanded_folders.insert(path.clone());
            }
            return true;
        }
        false
    }

    /// Put the cursor on `line` (one-based) — where a content search matched.
    ///
    /// Clamped rather than refused: a sigil can change between the search and
    /// the opening, and landing at the end of a file that shrank is better than
    /// declining to open it.
    fn go_to_line(&mut self, line: usize) {
        let source = self.editor.source().to_string();
        let offset = source
            .split_inclusive('\n')
            .take(line.saturating_sub(1))
            .map(str::len)
            .sum::<usize>()
            .min(source.len());
        self.editor.set_cursor(offset);
        self.follow_source_cursor();
        self.graph_focus = false;
    }

    /// Open the leaf under the selection, or toggle it if it is a folder.
    pub fn open_selected_sigil(&mut self) {
        let rows = self.visible_sigil_rows();
        // Where the search matched inside this sigil, if it did — read before
        // opening, because opening replaces the results.
        let hit = rows.get(self.sigil_choice).and_then(|row| match row {
            VisibleRow::Leaf { entry, .. } => self.sigil_hits.get(entry).map(|(line, _)| *line),
            VisibleRow::Folder { .. } => None,
        });
        match rows.get(self.sigil_choice) {
            Some(VisibleRow::Folder { .. }) => {
                self.toggle_selected_folder();
            }
            Some(VisibleRow::Leaf { entry, .. }) => {
                match entry.clone() {
                    SigilEntry::Temporary { id, .. } => self.open_temporary_sigil(id),
                    SigilEntry::Saved(name) | SigilEntry::Std(name) => self.open_sigil(&name),
                }
                // A search that found the line is a search that should land on
                // it. Opening at the top and leaving the reader to find it
                // again is the browser knowing something and not saying it.
                if let Some(line) = hit {
                    self.go_to_line(line);
                    self.message = format!("{} · line {line}", self.message);
                }
            }
            None => self.message = "no sigil selected".to_string(),
        }
    }

    /// Map a mouse click to a browser row: open a leaf, toggle a folder.
    pub fn click_sigil(&mut self, column: u16, row: u16) -> bool {
        if self.visualization != Visualization::Sigils {
            return false;
        }
        let Some((x, y, width, height)) = self.panel_inner else {
            return false;
        };
        if column < x || column >= x + width || row < y || row >= y + height {
            return false;
        }
        let line = self.graph_top + (row - y) as usize;
        let Some(index) = line.checked_sub(1) else {
            return false; // the SIGILS header row
        };
        if index >= self.visible_sigil_rows().len() {
            return false;
        }
        self.sigil_choice = index;
        self.open_selected_sigil();
        true
    }

    /// Run one block: parse it, prepend the buffer's top-level definitions and
    /// imports so it resolves as it would in place (Lisp eval-region), then
    /// hand it to the run path. A malformed slice reports a diagnostic.
    fn run_block(
        &mut self,
        block_src: &str,
        parallel: bool,
        input: Option<String>,
    ) -> WorkspaceAction {
        let program = match scoped_block_source(self.editor.source(), block_src) {
            Ok(program) => program,
            Err(error) => {
                self.message = format!("run block: {error}");
                return WorkspaceAction::None;
            }
        };
        let request = RunRequest {
            source: program,
            input: input.unwrap_or_else(|| self.record_text.clone().unwrap_or_default()),
            scope: RunScope::Block,
        };
        if parallel {
            WorkspaceAction::RunParallel(request)
        } else {
            WorkspaceAction::Run(request)
        }
    }

    /// Whether the buffer holds a `;` line comment — a `;` outside a quoted
    /// prompt. Reuses the highlighter, which already tracks prompt state.
    fn source_has_comment(&self) -> bool {
        highlights(self.editor.source()).contains(&Highlight::Comment)
    }

    fn refresh_sigils_if_visible(&mut self, message: String) {
        if self.visualization == Visualization::Sigils {
            self.search_sigils("");
        }
        self.message = message;
    }

    fn open_temporary_sigil(&mut self, id: u64) {
        if self.sigil_chat_busy {
            self.message = "wait for the god-agent turn before changing sigils".to_string();
            return;
        }
        if self.active_temporary_sigil == Some(id) {
            self.message = format!("temporary sigil temp:{id} is already open");
            return;
        }
        let Some(temporary) = self
            .temporary_sigils
            .iter()
            .find(|temporary| temporary.id == id)
            .cloned()
        else {
            self.message = format!("temporary sigil temp:{id} no longer exists");
            return;
        };
        self.park_current_as_temporary_sigil();
        self.reset_sigil_chat();
        self.editor = Editor::new(temporary.source);
        self.editor.dirty = true;
        self.chaos_star_visible = false;
        self.path = temporary.path;
        self.final_output = temporary.final_output;
        self.saved_output = temporary.saved_output;
        self.run_output.clear();
        self.run_output_visible = false;
        self.result_only_visible = false;
        self.current_sigil = None;
        self.active_temporary_sigil = Some(id);
        self.view_top = 0;
        self.view_left = 0;
        self.refresh();
        let message = format!(
            "restored temp:{id} ({}) · save it to make it permanent",
            temporary.label
        );
        if self.visualization == Visualization::Sigils {
            self.search_sigils("");
        }
        self.message = message;
    }

    /// A sigil name: one or more `/`-separated segments of letters, numbers,
    /// `-` and `_` — the same shape as a Rebis module path, so a sigil saved
    /// as `repair/loop` is importable as `(# repair/loop)`.
    fn sigil_name(name: &str) -> Option<String> {
        let name = name.trim().trim_end_matches(".rebis");
        let valid = !name.is_empty()
            && name.split('/').all(|segment| {
                !segment.is_empty()
                    && segment.chars().all(|character| {
                        character.is_ascii_alphanumeric() || "-_".contains(character)
                    })
            });
        valid.then(|| name.to_string())
    }

    fn sigil_output_path(&self, name: &str) -> PathBuf {
        self.sigils_dir().join(format!("{name}.output"))
    }

    /// Persist the last successful value separately from executable Rebis
    /// source, so importing the sigil remains definition-only and parseable.
    fn save_sigil_output(&self, name: &str) -> Result<bool, String> {
        let path = self.sigil_output_path(name);
        match &self.saved_output {
            Some(output) => fs::write(&path, output)
                .map(|()| true)
                .map_err(|error| format!("could not save {}: {error}", path.display())),
            None => match fs::remove_file(&path) {
                Ok(()) => Ok(false),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
                Err(error) => Err(format!("could not clear {}: {error}", path.display())),
            },
        }
    }

    fn restore_sigil_output(&mut self, name: &str) -> Result<bool, String> {
        self.run_output.clear();
        self.run_output_visible = false;
        self.result_only_visible = false;
        let path = self.sigil_output_path(name);
        match fs::read_to_string(&path) {
            Ok(output) => {
                self.final_output = Some(output.clone());
                self.saved_output = Some(output);
                Ok(true)
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                self.final_output = None;
                self.saved_output = None;
                Ok(false)
            }
            Err(error) => {
                self.final_output = None;
                self.saved_output = None;
                Err(format!("could not restore {}: {error}", path.display()))
            }
        }
    }

    fn clear_sigil_output(&mut self) {
        self.run_output.clear();
        self.run_output_visible = false;
        self.result_only_visible = false;
        self.final_output = None;
        self.saved_output = None;
    }

    fn save_sigil(&mut self, requested: &str) {
        if self.compiled.is_none() {
            self.message = "sigil save refused: fix the diagnostic".to_string();
            return;
        }
        let Some(name) = Self::sigil_name(requested) else {
            self.message = "sigil name: letters, numbers, - and _, with / for folders".to_string();
            return;
        };
        let path = self.sigils_dir().join(format!("{name}.rebis"));
        // A folder name like repair/loop creates its directories on save.
        if let Some(parent) = path.parent() {
            if let Err(error) = fs::create_dir_all(parent) {
                self.message = format!("could not create {}: {error}", parent.display());
                return;
            }
        }
        match fs::write(&path, self.editor.source()) {
            Ok(()) => match self.save_sigil_output(&name) {
                Ok(output_saved) => {
                    self.editor.mark_clean();
                    self.remove_active_temporary_sigil();
                    self.current_sigil = Some(name.clone());
                    self.host_events
                        .push_back(WorkspaceEvent::SigilSaved(name.clone()));
                    self.refresh_sigils_if_visible(if output_saved {
                        format!("saved sigil {name} · output sidecar saved")
                    } else {
                        format!("saved sigil {name} · no output yet")
                    });
                }
                Err(error) => self.message = error,
            },
            Err(error) => self.message = format!("could not save {}: {error}", path.display()),
        }
    }

    fn open_sigil(&mut self, requested: &str) {
        if self.sigil_chat_busy {
            self.message = "wait for the god-agent turn before changing sigils".to_string();
            return;
        }
        if let Some(id) = requested
            .trim()
            .strip_prefix("temp:")
            .and_then(|id| id.parse().ok())
        {
            self.open_temporary_sigil(id);
            return;
        }
        // Read-only modules open as buffer copies; saving back into their
        // namespace is refused, so edits go to a new name.
        let requested_name = requested.trim().trim_end_matches(".rebis");
        let standard_source = rebis_lang::std_modules()
            .iter()
            .find(|(name, _)| *name == requested_name)
            .map(|(_, source)| (*source).to_string());
        let embedded_label = if standard_source.is_some() {
            "embedded std"
        } else {
            "rebis collection"
        };
        let embedded_source = standard_source.or_else(|| {
            kaos_core::sigils::resolve_collection_module(requested_name)
                .ok()
                .flatten()
        });
        if let Some(source) = embedded_source {
            let parked = self.park_current_as_temporary_sigil();
            self.reset_sigil_chat();
            self.editor = Editor::new(source);
            self.chaos_star_visible = false;
            self.path = None;
            self.current_sigil = None;
            self.active_temporary_sigil = None;
            self.clear_sigil_output();
            self.view_top = 0;
            self.view_left = 0;
            self.refresh();
            let message = parked.map_or_else(
                || format!("opened {requested_name} ({embedded_label}) · /sigil save NAME copies it"),
                |id| {
                    format!(
                    "opened {requested_name} ({embedded_label}) · previous edits parked as temp:{id}"
                )
                },
            );
            if self.visualization == Visualization::Sigils {
                self.search_sigils("");
            }
            self.message = message;
            return;
        }
        let Some(name) = Self::sigil_name(requested) else {
            self.message = "usage: /sigil open NAME|temp:N".to_string();
            return;
        };
        let path = self.sigils_dir().join(format!("{name}.rebis"));
        match fs::read_to_string(&path) {
            Ok(source) => {
                let parked = self.park_current_as_temporary_sigil();
                self.reset_sigil_chat();
                self.editor = Editor::new(source);
                self.chaos_star_visible = false;
                self.path = None;
                self.current_sigil = Some(name.clone());
                self.active_temporary_sigil = None;
                let restored_output = self.restore_sigil_output(&name);
                self.view_top = 0;
                self.view_left = 0;
                self.refresh();
                let mut message = parked.map_or_else(
                    || format!("opened sigil {name} · :w PATH saves a working copy"),
                    |id| format!("opened sigil {name} · previous edits parked as temp:{id}"),
                );
                match restored_output {
                    Ok(true) => message.push_str(" · output sidecar restored"),
                    Ok(false) => {}
                    Err(error) => message.push_str(&format!(" · {error}")),
                }
                if self.visualization == Visualization::Sigils {
                    self.search_sigils("");
                }
                self.host_events
                    .push_back(WorkspaceEvent::SigilOpened(name.clone()));
                self.message = message;
            }
            Err(error) => self.message = format!("could not open sigil {name}: {error}"),
        }
    }

    /// Collect saved sigil names under `dir`, recursing into folders so a
    /// file at `repair/loop.rebis` lists as `repair/loop`. Depth is bounded
    /// to keep a wild symlink from walking the disk.
    fn collect_sigils(dir: &std::path::Path, prefix: &str, depth: usize, out: &mut Vec<String>) {
        if depth > 8 {
            return;
        }
        for entry in fs::read_dir(dir)
            .into_iter()
            .flatten()
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let Some(stem) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if path.is_dir() {
                Self::collect_sigils(&path, &format!("{prefix}{stem}/"), depth + 1, out);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rebis") {
                out.push(format!("{prefix}{stem}"));
            }
        }
    }

    /// Open `source` in the mandala editor.
    ///
    /// The editor is a windowed application and this workspace owns the
    /// terminal, so it runs as a detached child rather than inline. A program
    /// that does not parse opens too — as source, in the editor that can repair
    /// it — and the status line carries the diagnostic so the missing drawing is
    /// explained rather than mysterious.
    fn open_visual_with(&mut self, source: &str) {
        let source = source.trim();
        if source.is_empty() {
            self.message = "visual · nothing to draw".to_string();
            return;
        }
        // A program that does not parse is NOT refused here: the editor opens it
        // as source, which is where it gets repaired. Only say what is wrong, so
        // the reader knows why no drawing appeared.
        let diagnostic = kaos_core::visual::Mandala::from_rebis(source)
            .err()
            .map(|error| error.to_string());
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kaos"));
        match std::process::Command::new(exe)
            .arg("visual")
            .arg(source)
            // The editor inherits this workspace's working context, so a
            // program drawn there resolves paths and imports the same way.
            .current_dir(&self.cwd)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
        {
            Ok(_) => {
                self.message = match diagnostic {
                    Some(error) => format!("visual · opened as source · {error}"),
                    None => "visual · editor opened".to_string(),
                }
            }
            Err(error) => self.message = format!("visual · could not open: {error}"),
        }
    }

    /// Note where `query` first appears in `source`, and say whether it did.
    ///
    /// Case-insensitive, like the name match beside it, and comments count:
    /// this library documents its macros above them, so the sentence that
    /// explains what something is for is exactly what a person half-remembers.
    fn record_hit(&mut self, entry: &SigilEntry, source: &str, query: &str) -> bool {
        let Some((number, line)) = source
            .lines()
            .enumerate()
            .find(|(_, line)| line.to_ascii_lowercase().contains(query))
        else {
            return false;
        };
        self.sigil_hits
            .insert(entry.clone(), (number + 1, line.trim().to_string()));
        true
    }

    /// Find sigils by name **or** by what is written in them.
    ///
    /// A library is a body of source, so searching it should be a grep: a query
    /// that names no sigil but appears in one still finds it, and the result
    /// says where. That is how you find the macro you half-remember without
    /// already knowing which module someone filed it under — which is the only
    /// time you actually need to search.
    ///
    /// A path is still a path. `std/map` matches the module by name, and
    /// searching for `std/` lists the whole embedded folder, because a name
    /// match and a content match are the same list.
    ///
    /// The first hit is what is recorded, and opening the result goes to it.
    /// One line per sigil rather than one per occurrence: the browser is a
    /// tree of what exists, and a file that matched forty times is still one
    /// file — the editor's own `/search` is what walks the rest.
    fn search_sigils(&mut self, query: &str) {
        let query = query.to_ascii_lowercase();
        let dir = self.sigils_dir();
        let mut saved = Vec::new();
        Self::collect_sigils(&dir, "", 0, &mut saved);
        self.sigil_hits.clear();

        let embedded: Vec<String> = rebis_lang::std_modules()
            .iter()
            .map(|(name, _)| (*name).to_string())
            .collect();
        let collected: Vec<String> = kaos_core::sigils::collection_entries()
            .into_iter()
            .map(|entry| entry.name)
            .collect();

        let matches_name = |name: &String| name.to_ascii_lowercase().contains(&query);
        let named = saved.iter().any(matches_name)
            || embedded.iter().any(matches_name)
            || collected.iter().any(matches_name)
            || self
                .temporary_sigils
                .iter()
                .any(|sigil| sigil.label.to_ascii_lowercase().contains(&query));

        // Contents are searched only when NOTHING is named that. Not a
        // ranking — a fallback, and the difference is the whole design.
        // Searching `std/` is browsing: it should list that folder, not every
        // module that imports from it, which is what a plain grep returns and
        // is most of the library. Searching `std-memo` is looking for
        // something, nothing is called that, and the contents are where the
        // answer is. Which search you meant is legible from whether anything
        // bears the name, so it does not have to be asked for.
        let grep = !named && !query.is_empty();

        let mut sigils: Vec<SigilEntry> = self
            .temporary_sigils
            .iter()
            .filter(|sigil| sigil.label.to_ascii_lowercase().contains(&query))
            .map(|sigil| SigilEntry::Temporary {
                id: sigil.id,
                label: sigil.label.clone(),
            })
            .collect();
        sigils.sort_by_key(|entry| match entry {
            SigilEntry::Temporary { id, .. } => *id,
            SigilEntry::Saved(_) | SigilEntry::Std(_) => unreachable!(),
        });

        let mut personal: Vec<SigilEntry> = saved
            .into_iter()
            .filter(|name| {
                if !grep {
                    return matches_name(name);
                }
                let source = fs::read_to_string(dir.join(format!("{name}.rebis")))
                    .ok()
                    .unwrap_or_default();
                self.record_hit(&SigilEntry::Saved(name.clone()), &source, &query)
            })
            .map(SigilEntry::Saved)
            .collect();
        personal.sort();
        sigils.extend(personal);

        // The embedded standard library lists last, as a std/ folder, and the
        // collection after it.
        sigils.extend(
            rebis_lang::std_modules()
                .iter()
                .filter(|(name, source)| {
                    if !grep {
                        return name.to_ascii_lowercase().contains(&query);
                    }
                    self.record_hit(&SigilEntry::Std((*name).to_string()), source, &query)
                })
                .map(|(name, _)| SigilEntry::Std((*name).to_string()))
                .collect::<Vec<_>>(),
        );
        sigils.extend(
            collected
                .into_iter()
                .filter(|name| {
                    if !grep {
                        return matches_name(name);
                    }
                    // Through the module resolver rather than a guessed path:
                    // the collection is a submodule the core locates.
                    let source = kaos_core::sigils::resolve_collection_module(name)
                        .ok()
                        .flatten()
                        .unwrap_or_default();
                    self.record_hit(&SigilEntry::Std(name.clone()), &source, &query)
                })
                .map(SigilEntry::Std)
                .collect::<Vec<_>>(),
        );
        self.sigil_results = sigils;

        // Folders collapse by default. A non-empty query auto-expands the
        // folders that contain matches, so results are never hidden.
        if query.is_empty() {
            self.expanded_folders.clear();
        } else {
            let ancestors: Vec<String> = self
                .sigil_results
                .iter()
                .filter_map(SigilEntry::path)
                .flat_map(folder_ancestors)
                .collect();
            self.expanded_folders.extend(ancestors);
        }
        self.sigil_chat_visible = false;
        self.visualization = Visualization::Sigils;
        self.panel_visible = true;
        self.runs_visible = false;
        self.graph_focus = true;
        self.graph_top = 0;
        self.graph_left = 0;
        self.run_output_visible = false;
        self.result_only_visible = false;
        self.message = if grep {
            format!(
                "nothing is named that · {} sigil(s) contain it · opening goes to the line",
                self.sigil_results.len()
            )
        } else {
            format!(
                "{} sigil(s) match · /sigil open NAME|temp:N",
                self.sigil_results.len()
            )
        };
    }

    fn load(&mut self, requested: &str) {
        if self.sigil_chat_busy {
            self.message = "wait for the god-agent turn before opening another file".to_string();
            return;
        }
        if self.editor.dirty() {
            self.message = "unsaved changes · :w first or leave with :q!".to_string();
            return;
        }
        let path = resolve(&self.cwd, requested);
        match fs::read_to_string(&path) {
            Ok(source) => {
                self.reset_sigil_chat();
                self.editor = Editor::new(source);
                self.chaos_star_visible = false;
                self.path = Some(path.clone());
                self.view_top = 0;
                self.view_left = 0;
                self.refresh();
                self.message = format!("opened {}", path.display());
            }
            Err(error) => {
                self.message = format!("could not open {}: {error}", path.display());
            }
        }
    }

    /// The `/music` family: open the sound panel, play it, tune it, keep it.
    ///
    /// One command with words after it rather than six commands, because they
    /// are one thing — the section, and what to do with what is in it. The
    /// program is taken from the buffer at the moment it is asked for, so what
    /// sounds is what is written now, unsaved edits included.
    fn music_command(&mut self, rest: &str) {
        let (word, argument) = rest
            .split_once(char::is_whitespace)
            .map_or((rest, ""), |(word, argument)| (word, argument.trim()));
        match word {
            // Bare `/music` opens the section on the buffer as it stands.
            "" | "read" => {
                self.sigil_chat_visible = false;
                self.visualization = Visualization::Music;
                self.panel_visible = true;
                self.runs_visible = false;
                self.run_output_visible = false;
                self.result_only_visible = false;
                self.graph_top = 0;
                self.read_music();
            }
            "play" => {
                if !self.music.loaded() {
                    self.read_music();
                }
                self.music.play();
            }
            "stop" => self.music.stop(),
            "export" => {
                let name = if argument.is_empty() {
                    self.path
                        .as_ref()
                        .and_then(|path| path.file_stem())
                        .map_or_else(|| "sigil".to_string(), |stem| stem.to_string_lossy().into())
                        + ".wav"
                } else {
                    argument.to_string()
                };
                let path = resolve(&self.cwd, &name);
                if !self.music.loaded() {
                    self.read_music();
                }
                if let Err(error) = self.music.export(&path) {
                    self.music.message = error;
                }
            }
            "root" | "pace" | "partials" | "octaves" | "timbre" => {
                self.tune_music(word, argument);
            }
            other => {
                self.music.message = format!(
                    "music · unknown word `{other}` — play, stop, export [FILE], root, pace, \
                     partials, octaves, timbre"
                );
            }
        }
        self.message = self.music.message.clone();
    }

    /// Take the buffer as it stands and derive its music.
    fn read_music(&mut self) {
        let source = self.editor.source().to_string();
        if let Err(error) = self.music.read(&source) {
            self.music.message = format!("music · {error}");
        }
    }

    /// Change one knob and re-render, so the change is heard on the next play
    /// and seen on this frame.
    fn tune_music(&mut self, knob: &str, value: &str) {
        let number = value.parse::<f64>();
        match knob {
            "timbre" => {
                let Some(timbre) = Timbre::ALL
                    .iter()
                    .copied()
                    .find(|timbre| timbre.name() == value)
                else {
                    self.music.message = format!(
                        "music · timbre is one of: {}",
                        Timbre::ALL
                            .iter()
                            .map(|timbre| timbre.name())
                            .collect::<Vec<_>>()
                            .join(", ")
                    );
                    return;
                };
                self.music.tuning.timbre = timbre;
            }
            "root" => match number {
                Ok(hz) if (20.0..=4000.0).contains(&hz) => self.music.tuning.root_hz = hz,
                _ => {
                    self.music.message = "music · root wants a frequency in 20–4000 Hz".to_string();
                    return;
                }
            },
            "pace" => match number {
                Ok(seconds) if (0.02..=2.0).contains(&seconds) => self.music.tuning.pace = seconds,
                _ => {
                    self.music.message = "music · pace wants seconds per atom, 0.02–2".to_string();
                    return;
                }
            },
            "partials" => match value.parse::<usize>() {
                Ok(count) if (1..=6).contains(&count) => self.music.tuning.partials = count,
                _ => {
                    self.music.message = "music · partials wants 1–6".to_string();
                    return;
                }
            },
            "octaves" => match value.parse::<usize>() {
                Ok(count) if (1..=6).contains(&count) => self.music.tuning.octaves = count,
                _ => {
                    self.music.message = "music · octaves wants 1–6".to_string();
                    return;
                }
            },
            _ => return,
        }
        self.music.retune();
        self.music.message = format!(
            "{} Hz · {:.2} s/atom · {} partials · {} octaves · {}",
            self.music.tuning.root_hz,
            self.music.tuning.pace,
            self.music.tuning.partials,
            self.music.tuning.octaves,
            self.music.tuning.timbre.name(),
        );
    }

    /// The sound panel: the wave, then the score.
    ///
    /// Takes `&mut self` where the other projections do not, because the
    /// playhead is read from the player and reading it reaps a player that has
    /// finished. A projection that could not do that would leave the playhead
    /// frozen at the end of a piece that stopped.
    pub fn music_lines(&mut self, width: usize, max_rows: usize) -> Vec<String> {
        let width = width.max(8);
        if !self.music.loaded() {
            return vec![
                "no sound yet".to_string(),
                String::new(),
                "/music        read the buffer and draw its wave".to_string(),
                "/music play   hear it".to_string(),
            ];
        }
        let mut lines = Vec::new();
        // A quarter of the panel to the wave, the rest to the score, with the
        // wave never so short that it stops being a shape.
        let wave_rows = (max_rows / 4).clamp(3, 8);
        lines.extend(music::braille(&self.music.bands(width * 2), wave_rows));
        // The ruler carries the playhead: the piece's own clock, drawn under
        // the shape it is moving through.
        let seconds = self.music.score.seconds.max(f64::EPSILON);
        let head = self
            .music
            .playhead()
            .map(|at| ((at / seconds).clamp(0.0, 1.0) * (width - 1) as f64).round() as usize);
        lines.push(
            (0..width)
                .map(|column| match head {
                    Some(at) if at == column => '▲',
                    _ if column % 8 == 0 => '┬',
                    _ => '─',
                })
                .collect(),
        );
        lines.push(format!(
            "0s{:>width$.1}s",
            seconds,
            width = width.saturating_sub(4)
        ));
        lines.push(String::new());
        lines.push(format!(
            "{} Hz · {:.2} s/atom · {} partials · {} · {}",
            self.music.tuning.root_hz,
            self.music.tuning.pace,
            self.music.tuning.partials,
            self.music.tuning.timbre.name(),
            self.music.message,
        ));
        lines.push(String::new());
        let listed = max_rows.saturating_sub(lines.len()).max(1);
        lines.extend(self.score_lines(width, listed));
        lines
    }

    /// The score as text: every note, and the Fibonacci sum behind it.
    ///
    /// Written out rather than summarised, because the claim the section makes
    /// — that the tone comes from the word by arithmetic — is only worth
    /// anything if the arithmetic is on screen to be checked.
    #[must_use]
    pub fn score_lines(&self, width: usize, max_rows: usize) -> Vec<String> {
        if !self.music.loaded() {
            return vec!["no sound yet · /music reads the buffer".to_string()];
        }
        let mut lines = vec!["ONSET   LENGTH  D   PITCH     WORD = FIBONACCI".to_string()];
        for note in self
            .music
            .score
            .notes
            .iter()
            .skip(self.graph_top)
            .take(max_rows.saturating_sub(1).max(1))
        {
            let terms = self
                .music
                .reading(&note.token)
                .terms
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join("+");
            lines.push(truncate(
                &format!(
                    "{:>6.3}  {:>6.3}  {}  {:>7.1}  {} = {}",
                    note.start, note.duration, note.depth, note.hz, note.token, terms
                ),
                width,
            ));
        }
        lines
    }

    /// Produce the language's native expression tree.
    /// `max_rows` bounds rendering cost and prevents a graph from taking over the
    /// entire terminal.
    #[must_use]
    pub fn graph_lines(&self, width: usize, max_rows: usize) -> Vec<String> {
        if self.result_only_visible {
            return self
                .output_lines()
                .into_iter()
                .skip(self.graph_top)
                .take(max_rows.max(1))
                .map(|line| {
                    truncate(
                        &line.chars().skip(self.graph_left).collect::<String>(),
                        width.max(1),
                    )
                })
                .collect();
        }
        if self.run_output_visible && !self.run_output.is_empty() {
            return self
                .run_output
                .iter()
                .skip(self.graph_top)
                .take(max_rows.max(1))
                .map(|line| {
                    truncate(
                        &line.chars().skip(self.graph_left).collect::<String>(),
                        width.max(1),
                    )
                })
                .collect();
        }
        if self.visualization == Visualization::Sigils {
            let mut output = vec!["SIGILS".to_string()];
            let rows = self.visible_sigil_rows();
            if rows.is_empty() {
                output.push("(no matches)".to_string());
            } else {
                output.extend(rows.iter().enumerate().map(|(index, row)| {
                    let cursor = if index == self.sigil_choice {
                        "❯"
                    } else {
                        " "
                    };
                    match row {
                        VisibleRow::Folder {
                            path,
                            depth,
                            expanded,
                        } => {
                            let indent = "  ".repeat(*depth);
                            let mark = if *expanded { '▾' } else { '▸' };
                            let name = path.rsplit('/').next().unwrap_or(path);
                            format!("{cursor} {indent}{mark} {name}/")
                        }
                        VisibleRow::Leaf { depth, label, .. } => {
                            let indent = "  ".repeat(*depth);
                            format!("{cursor} {indent}  {label}")
                        }
                    }
                }));
            }
            output.push(String::new());
            output.push("Enter opens · Tab expands folders".to_string());
            return output
                .into_iter()
                .skip(self.graph_top)
                .take(max_rows.max(1))
                .map(|line| {
                    truncate(
                        &line.chars().skip(self.graph_left).collect::<String>(),
                        width.max(1),
                    )
                })
                .collect();
        }
        // Sound is drawn by [`Self::music_lines`], which needs `&mut` for the
        // playhead. What is left of it here is the part that is pure: the
        // score. A caller that only has `&self` still gets the truth, minus the
        // moving parts.
        if self.visualization == Visualization::Music {
            return self.score_lines(width, max_rows);
        }
        if self.chaos_star_visible {
            return Vec::new();
        }
        let Some(expr) = &self.compiled else {
            return vec![
                "o-[]-o  graph unavailable".to_string(),
                "fix the compiler diagnostic".to_string(),
            ];
        };
        let rendered = match self.visualization {
            Visualization::Mandala => rebis_lang::mandala(expr),
            Visualization::Tree => self.record.as_ref().map_or_else(
                || rebis_lang::tree(expr),
                |record| rebis_lang::tree_scored(expr, record),
            ),
            Visualization::Sigils | Visualization::Music => unreachable!(),
        };
        let mut output = Vec::new();
        if self.visualization == Visualization::Tree {
            output.push(rebis_lang::REBIS_SIGIL.to_string());
        }
        output.extend(rendered.lines().map(str::to_string));
        output
            .into_iter()
            .skip(self.graph_top)
            .take(max_rows.max(1))
            .map(|line| {
                truncate(
                    &line.chars().skip(self.graph_left).collect::<String>(),
                    width.max(1),
                )
            })
            .collect()
    }

    /// Scroll the right-hand projection without permitting blank overscroll.
    pub fn scroll_graph_vertical(&mut self, delta: isize, visible_rows: usize) {
        let row_count = if self.sigil_chat_visible {
            self.sigil_chat_visible_rows()
        } else if self.result_only_visible {
            self.output_lines().len()
        } else if self.run_output_visible && !self.run_output.is_empty() {
            self.run_output.len()
        } else if self.visualization == Visualization::Sigils {
            self.visible_sigil_rows().len() + 3
        } else if self.visualization == Visualization::Music {
            // The wave and its ruler stay put; the score under them scrolls.
            self.music.score.notes.len() + 1
        } else if self.chaos_star_visible {
            0
        } else if let Some(expr) = &self.compiled {
            let rendered = match self.visualization {
                Visualization::Mandala => rebis_lang::mandala(expr),
                Visualization::Tree => self.record.as_ref().map_or_else(
                    || rebis_lang::tree(expr),
                    |record| rebis_lang::tree_scored(expr, record),
                ),
                Visualization::Sigils | Visualization::Music => unreachable!(),
            };
            rendered.lines().count() + usize::from(self.visualization == Visualization::Tree)
        } else {
            2
        };
        let max_top = row_count.saturating_sub(visible_rows.max(1));
        self.graph_top = self
            .graph_top
            .min(max_top)
            .saturating_add_signed(delta)
            .min(max_top);
    }

    /// Reset the output pane when a captured request actually reaches the head
    /// of KAOS's work queue. Merely enqueueing a request must leave the active
    /// run's trace and returned value intact.
    pub fn begin_run(&mut self, scope: RunScope) {
        self.run_output.clear();
        self.sigil_chat_visible = false;
        self.runs_visible = true;
        self.graph_focus = true;
        self.run_output_visible = true;
        self.result_only_visible = false;
        self.final_output = None;
        self.graph_top = 0;
        self.graph_left = 0;
        self.message = format!(
            "running Rebis {}… · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · Pg scroll · ^C exits Kaos",
            scope.label()
        );
    }

    /// Append one streamed line from a running hosted Rebis program.
    pub fn push_run_output(&mut self, line: &str) {
        if let Some(value) = line.strip_prefix("result   ") {
            let output = self.final_output.get_or_insert_with(String::new);
            if !output.is_empty() {
                output.push('\n');
            }
            if value != "nothing" {
                output.push_str(value);
            }
        }
        self.run_output.push(line.to_string());
        self.message = "running Rebis… · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · Pg scroll · ^C exits Kaos"
            .to_string();
    }

    /// Mark the hosted run complete while retaining its trace in the right pane.
    pub fn finish_run(&mut self, code: i32) {
        if code == 0 {
            self.saved_output.clone_from(&self.final_output);
        }
        self.message = match code {
            0 => "Rebis run complete · output retained with this sigil".to_string(),
            kaos_core::outcome::ABSTAINED_EXIT => {
                "Rebis run abstained · work retained, gate refused it".to_string()
            }
            _ => format!("Rebis run exited ({code})"),
        };
    }

    /// A hosted child disappeared without completing the Rebis program. The
    /// app retains the run request and prompt journal; `p` owns continuation.
    pub fn pause_run(&mut self, reason: &str) {
        self.message = format!("Rebis run paused · {reason}");
    }

    fn output_lines(&self) -> Vec<String> {
        let mut lines = vec!["RESULT".to_string(), String::new()];
        match self
            .final_output
            .as_deref()
            .filter(|value| !value.is_empty())
        {
            Some(output) => lines.extend(output.lines().map(str::to_string)),
            None => lines.push("(no returned value)".to_string()),
        }
        lines
    }
}

fn load_vim_setting() -> bool {
    kaos_core::config::enabled("vim_mode") || kaos_core::config::enabled("vim")
}

fn save_vim_setting(enabled: bool) -> Result<(), String> {
    kaos_core::config::set_value("vim_mode", &enabled.to_string()).map(|_| ())
}

fn resolve(cwd: &Path, requested: &str) -> PathBuf {
    let path = Path::new(requested);
    if path.is_absolute() {
        path.to_path_buf()
    } else if let Some(rest) = requested.strip_prefix("~/") {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| cwd.to_path_buf())
            .join(rest)
    } else {
        cwd.join(path)
    }
}

fn byte_to_char(source: &str, byte: usize) -> usize {
    source
        .get(..byte.min(source.len()))
        .map_or_else(|| source.chars().count(), |prefix| prefix.chars().count())
}

fn truncate(source: &str, width: usize) -> String {
    if source.chars().count() <= width {
        source.to_string()
    } else if width <= 1 {
        "…".to_string()
    } else {
        source.chars().take(width - 1).chain(['…']).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{gutter_width, row_of_offset, wrapped_rows};

    #[test]
    fn a_long_line_continues_on_the_next_row_without_a_new_number() {
        // The whole point: overflow goes UNDER, and the gutter says it is still
        // the same line.
        let rows = wrapped_rows("(a b c d e)\nshort", 5);
        let numbers: Vec<Option<usize>> = rows
            .iter()
            .map(|row| (!row.continues).then_some(row.line + 1))
            .collect();
        assert_eq!(
            numbers,
            vec![Some(1), None, None, Some(2)],
            "only the row that starts a line is numbered"
        );
        let text = |row: &super::VisualRow| {
            "(a b c d e)\nshort"
                .chars()
                .skip(row.start)
                .take(row.end - row.start)
                .collect::<String>()
        };
        assert_eq!(text(&rows[0]), "(a b ");
        assert_eq!(text(&rows[1]), "c d e");
        assert_eq!(text(&rows[2]), ")");
        assert_eq!(text(&rows[3]), "short");
        // Every row belongs to the line it came from.
        assert_eq!(
            rows.iter().map(|row| row.line).collect::<Vec<_>>(),
            vec![0, 0, 0, 1]
        );
    }

    #[test]
    fn a_blank_line_still_occupies_one_row() {
        let rows = wrapped_rows("a\n\nb", 8);
        assert_eq!(rows.len(), 3, "a blank line is a line: {rows:?}");
        assert!(rows.iter().all(|row| !row.continues));
        assert_eq!(rows[1].start, rows[1].end, "the blank row is empty");
    }

    #[test]
    fn the_cursor_lands_on_the_row_that_holds_it_including_end_of_line() {
        let source = "(a b c d e)\nshort";
        let rows = wrapped_rows(source, 5);
        // First char of each row.
        assert_eq!(row_of_offset(&rows, 0), (0, 0));
        assert_eq!(row_of_offset(&rows, 5), (1, 0));
        assert_eq!(row_of_offset(&rows, 10), (2, 0));
        // Mid-row.
        assert_eq!(row_of_offset(&rows, 7), (1, 2));
        // Just past the last character of a wrapped line: end of line 1, which is
        // where the cursor sits after pressing `$`.
        assert_eq!(row_of_offset(&rows, 11), (2, 1));
        // And on the last line.
        assert_eq!(row_of_offset(&rows, 17), (3, 5));
    }

    #[test]
    fn the_gutter_never_narrows_below_two_digits() {
        assert_eq!(gutter_width(1), 2);
        assert_eq!(gutter_width(9), 2);
        assert_eq!(gutter_width(10), 2);
        assert_eq!(gutter_width(100), 3);
        assert_eq!(gutter_width(1234), 4);
    }

    #[test]
    fn a_zero_width_viewport_still_produces_rows() {
        // A pane can be laid out with no room at all for a frame; wrapping must
        // not spin or return nothing.
        let rows = wrapped_rows("abc", 0);
        assert_eq!(rows.len(), 3, "one char per row at the floor: {rows:?}");
    }

    use super::*;

    #[test]
    fn new_workspace_uses_the_chat_star_over_an_empty_buffer() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        assert!(workspace.chaos_star_visible());
        assert_eq!(workspace.editor.source(), "");
        assert!(workspace.diagnostic().is_none());
        assert_eq!(workspace.visualization, Visualization::Sigils);
        assert_eq!(workspace.graph_lines(80, 24)[0], "SIGILS");
        assert_eq!(kaos_core::theme::chaos_star_lines().len(), 11);
        assert!(kaos_core::theme::chaos_star_lines()[5].contains('◯'));
        // The first interaction lifts the star but adds no source.
        workspace.dismiss_chaos_star();
        workspace.refresh();
        assert!(!workspace.chaos_star_visible());
        assert_eq!(workspace.editor.source(), "");
        assert!(workspace.diagnostic().is_none());
        assert!(workspace.compiled.is_none());
    }

    #[test]
    fn workspace_accepts_lisp_style_top_level_definitions_and_forms() {
        let source = "(~ investigate (topic)\n\
                        (-> topic \"Investigate this topic in depth\"))\n\n\
                      ([\"Build an app from both reports\"]\n\
                        (investigate \"fibonacci\")\n\
                        (investigate \"chaos magic\"))";
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new(source);
        workspace.refresh();

        assert!(!workspace.chaos_star_visible());
        assert!(workspace.diagnostic().is_none());
        assert!(matches!(
            workspace.compiled,
            Some(rebis_lang::Expr::Program(_))
        ));
        let canonical = workspace.canonical().unwrap();
        assert!(canonical.starts_with("(~ investigate"));
        assert!(canonical.contains("\n([\"Build an app"));
    }

    #[test]
    fn chaos_star_is_never_saved_as_source() {
        let root = std::env::temp_dir().join(format!(
            "kaos-star-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut workspace = Workspace::open(root.clone(), Some("empty.rebis")).unwrap();
        assert!(workspace.chaos_star_visible());

        workspace.save(None).unwrap();

        assert_eq!(fs::read_to_string(root.join("empty.rebis")).unwrap(), "");
        assert!(workspace.chaos_star_visible());
        workspace.dismiss_chaos_star();
        assert!(!workspace.chaos_star_visible());
        assert_eq!(workspace.editor.source(), "");
        assert!(workspace.diagnostic().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn existing_file_bypasses_the_transient_star() {
        let root = std::env::temp_dir().join(format!(
            "kaos-existing-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("program.rebis"), "\"loaded\"").unwrap();

        let workspace = Workspace::open(root.clone(), Some("program.rebis")).unwrap();

        assert!(!workspace.chaos_star_visible());
        assert_eq!(workspace.editor.source(), "\"loaded\"");
        assert_eq!(workspace.canonical(), Some("\"loaded\""));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn mediator_delimiters_are_structural_and_highlighted() {
        let source = "([\"synthesize\"] \"a\")";
        let colours = highlights(source);
        assert_eq!(colours[1], Highlight::Mediate); // <
        assert_eq!(colours[14], Highlight::Mediate); // >
        assert_eq!(colours[0], Highlight::Parenthesis);
        assert_eq!(colours[19], Highlight::Parenthesis);
        assert_eq!(highlights("[a]")[0], Highlight::Mediate);
        assert_eq!(highlights("'(,work)")[0], Highlight::Mediate);
        assert_eq!(highlights("'(,work)")[2], Highlight::Mediate);
        // `$` composition heads read as operators, not as invalid characters.
        assert_eq!(highlights("($ a)")[1], Highlight::Mediate);
        assert_eq!(highlights("(^ (-> a b))")[1], Highlight::Invert);
    }

    #[test]
    fn quoted_prompt_contents_have_their_own_highlight() {
        let colours = highlights("\"hello\"");
        assert_eq!(colours[0], Highlight::Atom);
        assert_eq!(colours[1], Highlight::Prompt);
        assert_eq!(colours[5], Highlight::Prompt);
        assert_eq!(colours[6], Highlight::Atom);
        assert_eq!(highlights("(# std/loops)")[1], Highlight::Import);
    }

    #[test]
    fn a_routing_slash_is_tinted_without_stealing_module_slashes() {
        // Routing heads a form now, so the `/` is one token and the selector
        // after it is an ordinary word.
        let source = "(/ ollama:qwen4:4b \"hello\")";
        let colours = highlights(source);
        let slash = source
            .chars()
            .position(|character| character == '/')
            .unwrap();
        assert_eq!(colours[slash], Highlight::Model);

        let import = "(# std/loops)";
        let import_colours = highlights(import);
        let slash = import
            .chars()
            .position(|character| character == '/')
            .unwrap();
        assert_eq!(import_colours[slash], Highlight::Atom);
    }

    #[test]
    fn format_guards_comments_and_bang_confirms_the_drop() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("; keep me\n(-> \"a\" \"b\")".to_string());
        workspace.refresh();
        // Plain /format refuses when the buffer has a comment.
        workspace.command = "format".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.message.contains("drops ; comments"));
        assert!(workspace.editor.source().starts_with("; keep me"));
        // /format! confirms and reformats, dropping the comment.
        workspace.command = "format!".to_string();
        workspace.execute_kaos_command();
        assert!(!workspace.editor.source().contains("keep me"));
        assert!(workspace.editor.source().contains("->"));
    }

    #[test]
    fn line_comments_highlight_to_end_of_line_but_not_inside_prompts() {
        // `; note` comments the rest of the line; the newline and the next
        // line's code are highlighted normally.
        let colours = highlights("a ; note\n(b)");
        assert_eq!(colours[0], Highlight::Atom); // a
        assert_eq!(colours[2], Highlight::Comment); // ;
        assert_eq!(colours[7], Highlight::Comment); // last char of "note"
        assert_eq!(colours[8], Highlight::Whitespace); // newline ends it
        assert_eq!(colours[9], Highlight::Parenthesis); // ( on the next line
                                                        // Every `;` inside a quoted prompt is prompt text, including after an
                                                        // escaped quote and on later physical lines.
        let prompt = "\"first; line\nsecond \\\"quoted; text\\\"; end\"";
        let inside = highlights(prompt);
        for (index, character) in prompt.chars().enumerate() {
            if character == ';' {
                assert_eq!(inside[index], Highlight::Prompt, "semicolon at {index}");
            }
        }
    }

    #[test]
    fn highlighting_uses_the_language_tokenizer_for_boundaries() {
        // The editor no longer has its own lexer: every character's colour comes
        // from the token `rebis_lang::tokens` places it in, so highlighting can
        // never disagree with the parser about a boundary. One colour per char.
        let source = "(let (a \"x\") ($ a \" ; not a comment\"))";
        let colours = highlights(source);
        assert_eq!(colours.len(), source.chars().count());
        for token in rebis_lang::tokens(source) {
            let mut chars = source[token.start..token.end].char_indices();
            if let Some((_, first)) = chars.next() {
                let char_index = source[..token.start].chars().count();
                let expected = highlight_for(token, source, token.start, first);
                assert_eq!(
                    colours[char_index], expected,
                    "token {token:?} head mis-coloured"
                );
            }
        }
        // `$` and `let`'s bound symbols land as operator / atom, never invalid.
        assert!(!colours.contains(&Highlight::Invalid));
    }

    #[test]
    fn semicolon_prompt_text_compiles_and_formats_without_comment_confirmation() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("\"first; line\nsecond; line\"".to_string());
        workspace.refresh();

        assert!(workspace.diagnostic().is_none());
        assert!(!workspace.source_has_comment());
        workspace.command = "format".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(workspace.message.contains("formatted canonical Rebis"));
        assert_eq!(workspace.editor.source(), "\"first; line\\nsecond; line\"");
    }

    #[test]
    fn the_sound_panel_draws_the_wave_and_shows_its_arithmetic() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("(\"joy and expansion\" x)".to_string());
        workspace.refresh();

        workspace.command = "music".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(workspace.visualization, Visualization::Music);
        assert!(workspace.panel_visible, "the panel stayed shut");
        assert!(workspace.music.loaded(), "no piece was derived");

        let lines = workspace.music_lines(60, 24);
        assert!(
            lines.iter().any(
                |line| line.chars().all(|c| ('\u{2800}'..='\u{28ff}').contains(&c))
                    && !line.is_empty()
            ),
            "no wave was drawn: {lines:#?}"
        );
        // Every word of the prompt is in the score, with the sum it came from.
        let score = lines.join("\n");
        for word in ["joy", "and", "expansion"] {
            assert!(score.contains(word), "{word} is not in the score: {score}");
        }
        assert!(
            score.contains(" = ") && score.contains('+'),
            "the Fibonacci decomposition is not shown: {score}"
        );

        // Tuning is a command, and it re-renders rather than waiting for a play.
        let before = workspace.music.samples.len();
        workspace.command = "music pace 0.5".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(
            workspace.music.samples.len() > before,
            "a slower pace did not make a longer piece"
        );
        workspace.command = "music timbre saw".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(workspace.music.tuning.timbre.name(), "saw");
        // A knob that will not take the value says so and changes nothing.
        workspace.command = "music root nonsense".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(
            workspace.message.contains("20–4000"),
            "{}",
            workspace.message
        );
        assert!((workspace.music.tuning.root_hz - 220.0).abs() < f64::EPSILON);
    }

    #[test]
    fn sigil_names_are_safe_file_stems() {
        assert_eq!(
            Workspace::sigil_name("repair-loop"),
            Some("repair-loop".into())
        );
        assert_eq!(
            Workspace::sigil_name("repair_loop.rebis"),
            Some("repair_loop".into())
        );
        assert_eq!(Workspace::sigil_name("../escape"), None);
        assert_eq!(Workspace::sigil_name("two words"), None);
        // Folder names: /-separated segments, same shape as module paths.
        assert_eq!(
            Workspace::sigil_name("repair/loop"),
            Some("repair/loop".into())
        );
        assert_eq!(Workspace::sigil_name("a/../b"), None);
        assert_eq!(Workspace::sigil_name("/leading"), None);
        assert_eq!(Workspace::sigil_name("trailing/"), None);
        assert_eq!(Workspace::sigil_name("a//b"), None);
    }

    #[test]
    fn sigil_browser_keeps_results_in_the_scrollable_panel() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.visualization = Visualization::Sigils;
        workspace.sigil_results = vec![
            SigilEntry::Temporary {
                id: 1,
                label: "draft".into(),
            },
            SigilEntry::Saved("audit".into()),
            SigilEntry::Saved("team/repair-loop".into()),
        ];
        let lines = workspace.graph_lines(80, 20).join("\n");
        assert!(lines.contains("temp:1"));
        assert!(lines.contains("audit"));
        // A folder shows collapsed by default, hiding its contents.
        assert!(lines.contains("▸ team/"));
        assert!(!lines.contains("repair-loop"));
        assert!(lines.contains("Tab expands"));
    }

    #[test]
    fn edited_buffer_can_be_parked_and_restored_as_a_temporary_sigil() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor.insert_text("draft ");
        let draft = workspace.park_current_as_temporary_sigil().unwrap();
        let source = workspace.editor.source().to_string();
        assert_eq!(draft, 1);
        assert_eq!(workspace.temporary_sigils.len(), 1);

        workspace.editor = Editor::new("\"another sigil\"");
        workspace.open_temporary_sigil(draft);

        assert_eq!(workspace.editor.source(), source);
        assert!(workspace.editor.dirty());
        assert_eq!(workspace.active_temporary_sigil, Some(draft));
        workspace.remove_active_temporary_sigil();
        assert!(workspace.temporary_sigils.is_empty());
    }

    #[test]
    fn opening_saved_sigil_parks_dirty_source_in_the_visible_panel() {
        let root = std::env::temp_dir().join(format!(
            "kaos-sigils-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("saved.rebis"), "\"saved source\"").unwrap();
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = root.clone();
        workspace.visualization = Visualization::Sigils;
        workspace.editor.insert_text("draft ");
        let draft = workspace.editor.source().to_string();

        workspace.open_sigil("saved");

        assert_eq!(workspace.editor.source(), "\"saved source\"");
        assert_eq!(workspace.temporary_sigils.len(), 1);
        assert!(workspace
            .graph_lines(100, 20)
            .iter()
            .any(|line| line.contains("temp:1") && line.contains("unsaved")));

        workspace.open_sigil("temp:1");
        assert_eq!(workspace.editor.source(), draft);
        let saved_draft = root.join("draft.rebis");
        workspace.save(Some(saved_draft.to_str().unwrap())).unwrap();
        assert!(workspace.temporary_sigils.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hypersigil_resolver_supports_qualified_foundational_modules() {
        let root = std::env::temp_dir().join(format!(
            "kaos-modules-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let std_dir = root.join("std");
        fs::create_dir_all(&std_dir).unwrap();
        fs::write(std_dir.join("loops.rebis"), "(~ loop (x) ',x)").unwrap();
        let modules = HypersigilModules::at(root.clone());
        let name = ModuleName::try_from("std/loops").unwrap();

        assert_eq!(
            modules.resolve(&name).unwrap().as_deref(),
            Some("(~ loop (x) ',x)")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn hypersigil_resolver_imports_a_whole_folder_in_stable_order() {
        let root = std::env::temp_dir().join(format!(
            "kaos-module-folder-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(root.join("team/nested")).unwrap();
        fs::write(root.join("team/zeta.rebis"), "(~ zeta (x) ',x)").unwrap();
        fs::write(root.join("team/alpha.rebis"), "(~ alpha (x) ',x)").unwrap();
        fs::write(root.join("team/nested/beta.rebis"), "(~ beta (x) ',x)").unwrap();
        fs::write(root.join("team/ignored.txt"), "not a module").unwrap();
        let modules = HypersigilModules::at(root.clone());
        let name = ModuleName::try_from("team").unwrap();

        assert_eq!(
            modules.resolve(&name).unwrap().as_deref(),
            Some("((# team/alpha) (# team/nested/beta) (# team/zeta))")
        );

        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_source_state_places_the_error_it_reports() {
        assert_eq!(SourceState::of("   \n  "), SourceState::Empty);
        assert!(SourceState::of("(-> \"a\" \"b\")").is_valid());

        // The user's real case: a prompt that is never closed.
        let broken = "(\n  (-> \"a\"\n     \"never closed)\n";
        let state = SourceState::of(broken);
        assert!(!state.is_valid());
        let SourceState::Invalid { line, column, .. } = &state else {
            panic!("expected an invalid state, got {state:?}");
        };
        assert_eq!(*line, 3, "the unterminated prompt starts on line 3");
        assert!(*column > 1);
        // The detail is actionable rather than just a sentence.
        assert!(state.detail().starts_with("3:"), "{}", state.detail());
        assert_eq!(state.mark(), "\u{2717} invalid");
    }

    #[test]
    fn line_and_column_are_one_based_and_count_characters() {
        assert_eq!(line_column("abc", 0), (1, 1));
        assert_eq!(line_column("abc", 2), (1, 3));
        assert_eq!(line_column("ab\ncd", 3), (2, 1));
        // Columns count characters, not bytes.
        assert_eq!(line_column("é\nx", "é".len() + 1), (2, 1));
    }

    #[test]
    fn matching_supports_parentheses_and_mediator_brackets() {
        let mut editor = Editor::new("([\"synthesize\"] (\"a\") (\"b\"))");
        assert_eq!(editor.matching_parentheses(), Some((0, 27)));
        editor.cursor = 1;
        assert_eq!(editor.matching_parentheses(), Some((1, 14)));
        editor.cursor = 14;
        assert_eq!(editor.matching_parentheses(), Some((1, 14)));

        let editor = Editor::new("(/ claude:opus5 (-> \"a\" \"b\"))");
        assert_eq!(
            editor
                .matching_parentheses()
                .map(|(start, end)| slice_chars(editor.source(), start, end)),
            Some("(/ claude:opus5 (-> \"a\" \"b\"))".to_string())
        );
    }

    #[test]
    fn unicode_motions_and_undo_are_character_safe() {
        let mut editor = Editor::new("α β\nγ");
        editor.next_word();
        assert_eq!(editor.row_col(), (0, 2));
        editor.line_end();
        editor.insert('δ');
        assert_eq!(editor.source(), "α βδ\nγ");
        editor.undo();
        assert_eq!(editor.source(), "α β\nγ");
        editor.redo();
        assert_eq!(editor.source(), "α βδ\nγ");
    }

    #[test]
    fn dd_deletes_exactly_one_line() {
        let mut editor = Editor::new("a\nb\nc");
        editor.vertical(1);
        assert!(!editor.normal_d());
        assert!(editor.normal_d());
        assert_eq!(editor.source(), "a\nc");
    }

    #[test]
    fn vim_change_word_uses_end_of_word_and_enters_insert() {
        let mut editor = Editor::new("alpha beta");
        assert_eq!(editor.normal_key('c'), NormalAction::Pending);
        assert_eq!(editor.normal_key('w'), NormalAction::EnterInsert);
        assert_eq!(editor.source(), " beta");
        assert_eq!(editor.cursor(), 0);
        editor.insert_text("omega");
        assert_eq!(editor.source(), "omega beta");
    }

    #[test]
    fn vim_change_and_insert_are_one_undo_unit() {
        let mut editor = Editor::new("alpha beta");
        assert_eq!(editor.normal_key('c'), NormalAction::Pending);
        assert_eq!(editor.normal_key('w'), NormalAction::EnterInsert);
        editor.begin_insert_session(true);
        for character in "omega".chars() {
            editor.insert(character);
        }
        editor.end_insert_session();
        assert_eq!(editor.source(), "omega beta");
        assert_eq!(editor.cursor(), 4);

        editor.undo();
        assert_eq!(editor.source(), "alpha beta");
        editor.redo();
        assert_eq!(editor.source(), "omega beta");
    }

    #[test]
    fn vim_operators_compose_with_counts_and_motions() {
        let mut editor = Editor::new("one two three four");
        assert_eq!(editor.normal_key('2'), NormalAction::Pending);
        assert_eq!(editor.normal_key('d'), NormalAction::Pending);
        assert_eq!(editor.normal_key('w'), NormalAction::Edited);
        assert_eq!(editor.source(), "three four");

        assert_eq!(editor.normal_key('d'), NormalAction::Pending);
        assert_eq!(editor.normal_key('$'), NormalAction::Edited);
        assert_eq!(editor.source(), "");
        editor.undo();
        assert_eq!(editor.source(), "three four");
    }

    #[test]
    fn vim_counted_change_word_stops_at_the_last_words_end() {
        let mut editor = Editor::new("one two three");
        assert_eq!(editor.normal_key('2'), NormalAction::Pending);
        assert_eq!(editor.normal_key('c'), NormalAction::Pending);
        assert_eq!(editor.normal_key('w'), NormalAction::EnterInsert);
        assert_eq!(editor.source(), " three");
    }

    #[test]
    fn vim_single_word_operator_preserves_a_line_break_but_count_can_cross_it() {
        let mut deleted = Editor::new("one\ntwo");
        assert_eq!(deleted.normal_key('d'), NormalAction::Pending);
        assert_eq!(deleted.normal_key('w'), NormalAction::Edited);
        assert_eq!(deleted.source(), "\ntwo");

        let mut yanked = Editor::new("one\ntwo");
        assert_eq!(yanked.normal_key('y'), NormalAction::Pending);
        assert_eq!(yanked.normal_key('w'), NormalAction::Yanked);
        assert_eq!(yanked.yank, "one");

        let mut counted = Editor::new("one\ntwo three");
        assert_eq!(counted.normal_key('2'), NormalAction::Pending);
        assert_eq!(counted.normal_key('d'), NormalAction::Pending);
        assert_eq!(counted.normal_key('w'), NormalAction::Edited);
        assert_eq!(counted.source(), "three");
    }

    #[test]
    fn vim_normal_cursor_stays_on_characters_and_within_its_line() {
        let mut editor = Editor::new("abc\nde");
        assert_eq!(editor.normal_key('$'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (0, 2));
        assert_eq!(editor.normal_key('l'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (0, 2));
        assert_eq!(editor.normal_key('h'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (0, 1));

        assert_eq!(editor.normal_key('j'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (1, 1));
        assert_eq!(editor.normal_key('l'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (1, 1));
    }

    #[test]
    fn vim_line_end_operators_are_inclusive_and_x_never_eats_a_newline() {
        let mut to_end = Editor::new("abc\ndef");
        to_end.cursor = 1;
        assert_eq!(to_end.normal_key('d'), NormalAction::Pending);
        assert_eq!(to_end.normal_key('$'), NormalAction::Edited);
        assert_eq!(to_end.source(), "a\ndef");

        let mut last = Editor::new("abc\ndef");
        assert_eq!(last.normal_key('$'), NormalAction::Moved);
        assert_eq!(last.normal_key('x'), NormalAction::Edited);
        assert_eq!(last.source(), "ab\ndef");
        assert_eq!(last.row_col(), (0, 2));
    }

    #[test]
    fn vim_end_word_advances_when_already_at_a_words_end() {
        let mut editor = Editor::new("one two three");
        editor.cursor = 2;
        assert_eq!(editor.normal_key('e'), NormalAction::Moved);
        assert_eq!(editor.cursor(), 6);
        assert_eq!(editor.normal_key('2'), NormalAction::Pending);
        assert_eq!(editor.normal_key('e'), NormalAction::Moved);
        assert_eq!(editor.cursor(), 12);
    }

    #[test]
    fn vim_counts_apply_to_x_and_s() {
        let mut deleted = Editor::new("abcdef");
        assert_eq!(deleted.normal_key('3'), NormalAction::Pending);
        assert_eq!(deleted.normal_key('x'), NormalAction::Edited);
        assert_eq!(deleted.source(), "def");

        let mut changed = Editor::new("abcdef");
        assert_eq!(changed.normal_key('2'), NormalAction::Pending);
        assert_eq!(changed.normal_key('s'), NormalAction::EnterInsert);
        assert_eq!(changed.source(), "cdef");
    }

    #[test]
    fn vim_counted_g_and_gg_go_to_the_requested_line() {
        let mut editor = Editor::new("zero\n  one\n    two\nthree");
        assert_eq!(editor.normal_key('3'), NormalAction::Pending);
        assert_eq!(editor.normal_key('G'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (2, 4));

        assert_eq!(editor.normal_key('2'), NormalAction::Pending);
        assert_eq!(editor.normal_key('g'), NormalAction::Pending);
        assert_eq!(editor.normal_key('g'), NormalAction::Moved);
        assert_eq!(editor.row_col(), (1, 2));
    }

    #[test]
    fn vim_cc_preserves_the_line_break_for_inserted_replacement() {
        let mut editor = Editor::new("one\ntwo\nthree");
        editor.vertical(1);
        assert_eq!(editor.normal_key('c'), NormalAction::Pending);
        assert_eq!(editor.normal_key('c'), NormalAction::EnterInsert);
        assert_eq!(editor.source(), "one\n\nthree");
        editor.begin_insert_session(true);
        editor.insert_text("replacement");
        editor.end_insert_session();
        assert_eq!(editor.source(), "one\nreplacement\nthree");
    }

    #[test]
    fn vim_word_text_objects_distinguish_inner_and_around() {
        let mut inner = Editor::new("one alpha beta");
        inner.cursor = 5;
        assert_eq!(inner.normal_key('c'), NormalAction::Pending);
        assert_eq!(inner.normal_key('i'), NormalAction::Pending);
        assert_eq!(inner.normal_key('w'), NormalAction::EnterInsert);
        assert_eq!(inner.source(), "one  beta");

        let mut around = Editor::new("one alpha beta");
        around.cursor = 5;
        assert_eq!(around.normal_key('d'), NormalAction::Pending);
        assert_eq!(around.normal_key('a'), NormalAction::Pending);
        assert_eq!(around.normal_key('w'), NormalAction::Edited);
        assert_eq!(around.source(), "one beta");
    }

    #[test]
    fn vim_line_operators_and_linewise_put_share_register_semantics() {
        let mut editor = Editor::new("one\ntwo\nthree");
        editor.cursor = 2;
        assert_eq!(editor.normal_key('y'), NormalAction::Pending);
        assert_eq!(editor.normal_key('y'), NormalAction::Yanked);
        assert_eq!(editor.cursor(), 2);
        editor.vertical(1);
        editor.paste_after();
        assert_eq!(editor.source(), "one\ntwo\none\nthree");

        assert_eq!(editor.normal_key('2'), NormalAction::Pending);
        assert_eq!(editor.normal_key('d'), NormalAction::Pending);
        assert_eq!(editor.normal_key('d'), NormalAction::Edited);
        assert_eq!(editor.source(), "one\ntwo\n");
    }

    #[test]
    fn graph_view_is_derived_from_rebis_tree() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("([\"synthesize\"] (-> \"a\" \"b\") (<- \"c\" \"d\"))");
        workspace.refresh();
        workspace.visualization = Visualization::Tree;
        assert!(workspace.diagnostic().is_none());
        let graph = workspace.graph_lines(120, 100).join("\n");
        assert!(graph.contains("□ mediator square"));
        assert!(graph.contains("→ forward"));
        assert!(graph.contains("a"));
        assert!(graph.contains("d"));
    }

    #[test]
    fn terminal_views_parse_format_and_render_percent_control() {
        let source = "(% (-> \"question\" ($ \"Return exactly one token: 0 or 1. \" \"No explanation.\")) one zero)";
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new(source);
        workspace.refresh();

        assert!(workspace.diagnostic().is_none());
        assert!(workspace
            .canonical()
            .is_some_and(|text| text.contains("(%")));

        workspace.visualization = Visualization::Tree;
        let tree = workspace.graph_lines(240, 100).join("\n");
        assert!(tree.contains("% binary gate"));
        assert!(tree.contains("Return exactly one token: 0 or 1."));

        workspace.command = "format".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(workspace.editor.source().contains("(%"));

        workspace.command = "mandala".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(workspace.graph_lines(240, 100).join("\n").contains('%'));

        let percent = source.find('%').unwrap();
        let colours = highlights(source);
        assert_eq!(
            colours[source[..percent].chars().count()],
            Highlight::Mediate
        );
    }

    #[test]
    fn mandala_renders_function_templates_and_call_boxes() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new(
            "((~ inspect (target) (-> target \"Write report\")) \
              (inspect \"Inspect parser\"))",
        );
        workspace.refresh();
        workspace.visualization = Visualization::Mandala;
        let graph = workspace.graph_lines(240, 100).join("\n");
        assert!(graph.contains("~[inspect(target)]"));
        assert!(graph.contains("─[inspect]─o"));
    }

    #[test]
    fn visual_mode_yanks_deletes_and_pastes_unicode_safely() {
        let mut editor = Editor::new("αβγ delta");
        editor.begin_visual(false);
        editor.right();
        editor.delete_visual(false);
        assert_eq!(editor.source(), "γ delta");
        editor.document_end();
        editor.paste_after();
        assert_eq!(editor.source(), "γ deltaαβ");
    }

    #[test]
    fn visual_block_yanks_and_deletes_a_column_rectangle() {
        let mut editor = Editor::new("abcd\nefgh\nijkl");
        editor.cursor = 1; // row 0, column 1
        editor.begin_visual_block();
        editor.cursor = 12; // row 2, column 2
        assert_eq!(editor.visual_block_range(), Some((0, 2, 1, 2)));

        editor.yank_visual_block();
        assert_eq!(editor.yank, "bc\nfg\njk");
        assert!(editor.yank_blockwise);

        // Re-select and delete the same rectangle: the column is cut from every
        // spanned row, not the whole lines.
        editor.cursor = 1;
        editor.begin_visual_block();
        editor.cursor = 12;
        editor.delete_visual_block();
        assert_eq!(editor.source(), "ad\neh\nil");
        assert_eq!(editor.cursor(), 1);
    }

    #[test]
    fn visual_block_paste_relays_the_register_as_a_column() {
        let mut editor = Editor::new("abc\ndef\nghi");
        editor.cursor = 1; // row 0, column 1
        editor.begin_visual_block();
        editor.cursor = 9; // row 2, column 1
        editor.yank_visual_block();
        assert_eq!(editor.yank, "b\ne\nh");

        // Pasting the block register lays each fragment down the same column
        // rather than dumping the three lines at one spot.
        editor.cursor = 0;
        editor.paste_before();
        assert_eq!(editor.source(), "babc\nedef\nhghi");
    }

    #[test]
    fn visual_block_paste_pads_short_lines_to_keep_the_column_aligned() {
        let mut editor = Editor::new("abcd\nx");
        editor.yank = "P\nQ".to_string();
        editor.yank_linewise = false;
        editor.yank_blockwise = true;
        editor.cursor = 2; // row 0, on 'c' (column 2); paste lands at column 3
        editor.paste_after();
        // The second row is shorter than the target column, so it is padded
        // with spaces before "Q" lands, keeping the pasted block rectangular.
        assert_eq!(editor.source(), "abcPd\nx  Q");
    }

    #[test]
    fn visual_block_put_replaces_the_rectangle_in_one_undo_step() {
        let mut editor = Editor::new("abcd\nefgh\nijkl");
        editor.set_yank("X\nY\nZ");
        editor.yank_blockwise = true;
        editor.set_cursor(1);
        editor.begin_visual_block();
        editor.set_cursor(12);
        editor.paste_visual_block();
        assert_eq!(editor.source(), "aXd\neYh\niZl");
        assert_eq!(editor.yank(), "bc\nfg\njk");

        editor.undo();
        assert_eq!(editor.source(), "abcd\nefgh\nijkl");
    }

    #[test]
    fn selected_text_includes_the_visual_cursor_cell() {
        let mut editor = Editor::new("alpha beta");
        editor.set_cursor(0);
        editor.begin_visual(false);
        editor.set_cursor(4);
        assert_eq!(editor.selected_text(Mode::Visual).as_deref(), Some("alpha"));
        assert_eq!(editor.selection_ranges(Mode::Visual), vec![(0, 5)]);
    }

    #[test]
    fn shared_key_state_machine_keeps_frontends_identical() {
        let sequence = [
            (EditKey::Char('i'), EditModifiers::default()),
            (
                EditKey::Paste("alpha beta".to_string()),
                EditModifiers::default(),
            ),
            (EditKey::Escape, EditModifiers::default()),
            (EditKey::Char('0'), EditModifiers::default()),
            (EditKey::Char('d'), EditModifiers::default()),
            (EditKey::Char('w'), EditModifiers::default()),
            (EditKey::Char('u'), EditModifiers::default()),
            (
                EditKey::Char('r'),
                EditModifiers {
                    ctrl: true,
                    shift: false,
                },
            ),
        ];
        let mut terminal = Editor::new("");
        let mut visual = Editor::new("");
        let mut terminal_mode = Mode::Normal;
        let mut visual_mode = Mode::Normal;
        for (key, modifiers) in sequence {
            handle_edit_key(
                &mut terminal,
                &mut terminal_mode,
                true,
                key.clone(),
                modifiers,
            );
            handle_edit_key(&mut visual, &mut visual_mode, true, key, modifiers);
        }
        assert_eq!(terminal.source(), visual.source());
        assert_eq!(terminal.cursor(), visual.cursor());
        assert_eq!(terminal_mode, visual_mode);
        assert_eq!(terminal.yank(), visual.yank());
    }

    #[test]
    fn shared_direct_mode_pairs_and_ctrl_bracket_escape_work() {
        let mut direct = Editor::new("");
        let mut direct_mode = Mode::Insert;
        handle_edit_key(
            &mut direct,
            &mut direct_mode,
            false,
            EditKey::Char('('),
            EditModifiers::default(),
        );
        assert_eq!(direct.source(), "()");
        assert_eq!(direct.cursor(), 1);

        let mut vim = Editor::new("");
        let mut vim_mode = Mode::Insert;
        vim.begin_insert_session(false);
        handle_edit_key(
            &mut vim,
            &mut vim_mode,
            true,
            EditKey::Char('['),
            EditModifiers {
                ctrl: true,
                shift: false,
            },
        );
        assert_eq!(vim_mode, Mode::Normal);
    }

    #[test]
    fn vim_visual_put_replaces_the_selection_in_one_undo_step() {
        let mut editor = Editor::new("one TWO three");
        editor.set_yank("replacement");
        editor.cursor = 4;
        editor.begin_visual(false);
        editor.cursor = 6;
        editor.paste_visual(false);
        assert_eq!(editor.source(), "one replacement three");
        assert_eq!(editor.yank, "TWO");
        assert_eq!(editor.cursor(), 14);

        editor.undo();
        assert_eq!(editor.source(), "one TWO three");
    }

    #[test]
    fn bracketed_paste_is_atomic_and_preserves_every_line() {
        let mut editor = Editor::new("");
        editor.insert_text("first\r\nsecond\nthird\r");
        assert_eq!(editor.source(), "first\nsecond\nthird\n");
        assert_eq!(editor.row_col(), (3, 0));
        editor.undo();
        assert_eq!(editor.source(), "");
    }

    #[test]
    fn normal_line_yank_and_put_work() {
        let mut editor = Editor::new("one\ntwo\n");
        assert!(!editor.normal_y());
        assert!(editor.normal_y());
        editor.document_end();
        editor.paste_after();
        assert_eq!(editor.source(), "one\ntwo\none\n");
    }

    #[test]
    fn slash_commands_control_the_scrollable_panel() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("([\"merge\"] \"left\" \"right\")");
        workspace.refresh();
        workspace.command = "panel hide".to_string();
        workspace.execute_kaos_command();
        assert!(!workspace.panel_visible);
        workspace.command = "panel toggle".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.panel_visible);
        workspace.command = "panel toggle".to_string();
        workspace.execute_kaos_command();
        assert!(!workspace.panel_visible);
        workspace.command = "graph".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.panel_visible);
        assert!(workspace.graph_focus);
        workspace.graph_left = 8;
        let shifted = workspace.graph_lines(20, 2);
        workspace.graph_left = 0;
        let origin = workspace.graph_lines(20, 2);
        assert_ne!(shifted, origin);
    }

    #[test]
    fn sigil_chat_opens_a_source_bound_right_panel_without_losing_source() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.dismiss_chaos_star();
        workspace.editor = Editor::new("(-> \"inspect\" \"report\")");
        workspace.refresh();
        let original = workspace.editor.source().to_string();

        workspace.command = "sigil chat".to_string();
        assert_eq!(
            workspace.execute_kaos_command(),
            WorkspaceAction::OpenSigilChat
        );
        assert!(workspace.sigil_chat_visible());
        assert!(workspace.panel_visible);
        assert!(workspace.graph_focus);
        assert_eq!(workspace.editor.source(), original);

        for character in "change the final mediator".chars() {
            workspace.insert_sigil_chat_char(character);
        }
        assert_eq!(
            workspace.take_sigil_chat_message().as_deref(),
            Some("change the final mediator")
        );
        assert!(workspace.sigil_chat_input().is_empty());
        assert!(workspace
            .sigil_chat_lines()
            .iter()
            .any(|line| line.contains("change the final mediator")));
        assert_eq!(workspace.editor.source(), original);
    }

    #[test]
    fn sigil_chat_work_sections_collapse_and_expand_without_losing_rows() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.dismiss_chaos_star();
        workspace.open_sigil_chat();
        workspace.push_sigil_chat_line("\u{1e}FOLD_OPEN\u{1f}model work");
        workspace.push_sigil_chat_line("god     reading source");
        workspace.push_sigil_chat_line("\u{1e}FOLD_CLOSE");

        let collapsed = workspace.sigil_chat_visible_rows();
        workspace.toggle_sigil_chat_fold(false);
        let expanded = workspace.sigil_chat_visible_rows();

        assert_eq!(collapsed + 1, expanded);
        assert!(workspace.sigil_chat_fold_expanded(3));
        workspace.toggle_sigil_chat_fold(false);
        assert_eq!(workspace.sigil_chat_visible_rows(), collapsed);
    }

    #[test]
    fn source_search_repeats_wraps_and_keeps_unicode_cursor_offsets() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.dismiss_chaos_star();
        workspace.editor = Editor::new("α first\nβ middle\nβ last");
        workspace.graph_focus = true;

        workspace.command = "search β".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(workspace.editor.row_col(), (1, 0));
        assert!(
            !workspace.graph_focus,
            "a source match must focus the editor"
        );
        assert!(workspace.message.contains("2:1"));

        workspace.command = "search".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.editor.row_col(), (2, 0));
        workspace.command = "search".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.editor.row_col(), (1, 0));
        assert!(workspace.message.starts_with("wrapped"));

        let cursor = workspace.editor.cursor();
        workspace.command = "search absent".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.editor.cursor(), cursor);
        assert!(workspace.message.contains("not found"));
        assert!(!workspace.editor.dirty());
    }

    #[test]
    fn source_search_without_a_previous_query_explains_what_is_missing() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.dismiss_chaos_star();
        workspace.command = "search".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.message.contains("enter text"));
    }

    #[test]
    fn runs_command_requests_the_host_run_browser() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.command = "runs".to_string();
        assert_eq!(
            workspace.execute_kaos_command(),
            WorkspaceAction::BrowseRuns
        );
    }

    #[test]
    fn config_commands_are_delegated_to_the_host_without_leaving_the_editor() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        for command in ["config", "config restore"] {
            workspace.command = command.to_string();
            assert_eq!(
                workspace.execute_kaos_command(),
                WorkspaceAction::Kaos(command.to_string())
            );
        }
    }

    #[test]
    fn theme_commands_pass_through_to_the_host_dispatcher() {
        // `/theme dark|light` (and bare `/theme`) must reach the outer Kaos
        // dispatcher that persists the mode, not fall through to the
        // unknown-command branch — otherwise the workspace ignores them.
        for command in ["theme", "theme dark", "theme light"] {
            let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
            workspace.command = command.to_string();
            assert_eq!(
                workspace.execute_kaos_command(),
                WorkspaceAction::Kaos(command.to_string()),
                "/{command} should be handled by the host",
            );
        }
    }

    #[test]
    fn chat_command_suspends_unsaved_rebis_source_in_memory() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor.insert('x');
        workspace.command = "chat".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::Suspend);
        assert!(workspace.editor.dirty());
        assert!(workspace.editor.source().starts_with('x'));
    }

    #[test]
    fn run_command_executes_without_a_record() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("\"run this\"");
        workspace.refresh();
        workspace.command = "run".to_string();
        assert!(matches!(
            workspace.execute_kaos_command(),
            WorkspaceAction::Run(RunRequest {
                input,
                scope: RunScope::Program,
                ..
            }) if input.is_empty()
        ));
    }

    #[test]
    fn successful_output_is_retained_but_plain_runs_keep_the_declared_record() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("\"run this\"");
        workspace.refresh();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("result   first checkpoint");
        workspace.finish_run(0);

        // A later failed attempt may show partial output, but it must not replace
        // the saved successful output.
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("result   incomplete attempt");
        workspace.finish_run(3);
        workspace.command = "run".to_string();

        assert!(matches!(
            workspace.execute_kaos_command(),
            WorkspaceAction::Run(RunRequest {
                input,
                scope: RunScope::Program,
                ..
            }) if input.is_empty()
        ));
        assert_eq!(workspace.saved_output.as_deref(), Some("first checkpoint"));
    }

    #[test]
    fn run_parallel_requests_an_immediate_program_lane() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("\"run beside active work\"");
        workspace.refresh();
        workspace.command = "run parallel".to_string();

        assert!(matches!(
            workspace.execute_kaos_command(),
            WorkspaceAction::RunParallel(RunRequest {
                scope: RunScope::Program,
                ..
            })
        ));
    }

    #[test]
    fn run_selection_evaluates_only_the_block_with_buffer_definitions() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new(
            "(\n  (~ improve (v) (-> v \"sharpen\"))\n  (improve \"draft\"))".to_string(),
        );
        workspace.refresh();
        // Select the `(improve "draft")` call — its char range in the buffer.
        let source = workspace.editor.source();
        let start = source.find("(improve").unwrap();
        let end = source.rfind(')').unwrap() - 1; // the call's own close paren
                                                  // Char indices (the source is ASCII here).
        workspace.run_selection = Some((start, end));
        workspace.command = "run".to_string();
        let action = workspace.execute_kaos_command();
        let WorkspaceAction::Run(RunRequest { source, scope, .. }) = action else {
            panic!("expected a run action, got {action:?}");
        };
        assert_eq!(scope, RunScope::Block);
        // The definition is prepended so `improve` resolves, and the block is
        // present — but the sibling call is not run twice.
        assert!(source.contains("improve"));
        assert!(source.contains("sharpen"));
        // The selection is consumed: a following plain run uses the buffer.
        assert!(workspace.run_selection.is_none());
    }

    #[test]
    fn run_block_evaluates_the_form_at_the_cursor() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("(-> \"a\" \"b\")\n(-> \"c\" \"d\")".to_string());
        workspace.refresh();
        // The cursor starts on the first form's opening `(` — the block the
        // matcher resolves. (Interactively, placing it after the closing `)`
        // resolves the same form.)
        workspace.command = "run block".to_string();
        let action = workspace.execute_kaos_command();
        let WorkspaceAction::Run(RunRequest { source, .. }) = action else {
            panic!("expected a run action, got {action:?}");
        };
        // Only the first form runs; the second is not in the block.
        assert!(source.contains("\"a\""));
        assert!(!source.contains("\"c\""));
    }

    #[test]
    fn run_block_parallel_keeps_definitions_and_requests_a_parallel_lane() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new(
            "(~ inspect (topic) (-> topic \"report\"))\n(inspect \"parser\")".to_string(),
        );
        workspace.refresh();
        let source = workspace.editor.source();
        let block_start = source.find("(inspect \"").unwrap();
        for _ in 0..block_start {
            workspace.editor.right();
        }
        workspace.command = "run block parallel".to_string();

        let action = workspace.execute_kaos_command();
        let WorkspaceAction::RunParallel(RunRequest { source, scope, .. }) = action else {
            panic!("expected a parallel run action, got {action:?}");
        };
        assert_eq!(scope, RunScope::Block);
        assert!(
            source.matches("inspect").count() >= 2,
            "parallel block source: {source}"
        );
        assert!(
            source.contains("\"report\""),
            "parallel block source: {source}"
        );
        assert!(source.contains("\"parser\""));
    }

    #[test]
    fn block_runs_keep_model_bound_top_level_definitions() {
        let source =
            "(/ claude:opus5 (~ inspect (topic) (-> topic \"report\")))\n(inspect \"parser\")";
        let scoped = scoped_block_source(source, "(inspect \"parser\")").unwrap();
        let parsed = rebis_lang::parse(&scoped).unwrap();

        assert!(
            scoped.contains("claude:opus5"),
            "scoped block source: {scoped}"
        );
        assert!(matches!(parsed, Expr::Compose(_)));
    }

    #[test]
    fn run_block_reports_a_diagnostic_for_an_incomplete_slice() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("(-> \"a\" \"b\")".to_string());
        workspace.refresh();
        // Move the cursor off any bracket, into the middle of the form.
        for _ in 0..4 {
            workspace.editor.right();
        }
        workspace.command = "run block".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert!(workspace.message.contains("cursor on the block"));
    }

    #[test]
    fn slash_save_writes_and_assigns_a_path() {
        let dir = std::env::temp_dir();
        let name = format!("kaos-rebis-save-{}.rebis", std::process::id());
        let path = dir.join(&name);
        let mut workspace = Workspace::open(dir, None).unwrap();
        workspace.command = format!("save {name}");
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(workspace.path.as_deref(), Some(path.as_path()));
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            workspace.editor.source()
        );
        fs::remove_file(path).unwrap();
    }

    #[test]
    fn hosted_run_output_replaces_tree_until_the_next_run() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("firing   1 · ○ scout release");
        workspace.push_run_output("score    0.500");
        assert_eq!(
            workspace.graph_lines(120, 10),
            vec!["firing   1 · ○ scout release", "score    0.500"]
        );
        workspace.graph_top = 1;
        workspace.graph_left = 5;
        assert_eq!(workspace.graph_lines(120, 10), vec!["    0.500"]);
        workspace.finish_run(0);
        assert!(workspace.message.contains("complete"));
    }

    #[test]
    fn panel_navigation_never_discards_or_reclaims_a_background_run_stream() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("([\"merge\"] \"left\" \"right\")");
        workspace.refresh();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("agent   first retained line");

        workspace.command = "mandala".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.run_output, vec!["agent   first retained line"]);
        assert!(!workspace
            .graph_lines(200, 20)
            .join("\n")
            .contains("first retained"));

        // Streaming continues without stealing the panel back from the user.
        workspace.push_run_output("agent   second retained line");
        assert_eq!(
            workspace.run_output,
            vec![
                "agent   first retained line",
                "agent   second retained line"
            ]
        );
        assert!(!workspace
            .graph_lines(200, 20)
            .join("\n")
            .contains("second retained"));

        workspace.command = "tree".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.run_output.len(), 2);
    }

    #[test]
    fn output_command_shows_and_copies_the_returned_value() {
        let mut workspace = Workspace::open(std::env::temp_dir(), None).unwrap();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("result   first line");
        workspace.push_run_output("result   second line");
        workspace.command = "output".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(
            workspace.graph_lines(80, 10),
            vec!["RESULT", "", "first line", "second line"]
        );
        workspace.command = "output copy".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.message.contains("Vim yank register"));
    }

    #[test]
    fn visualization_commands_switch_between_tree_and_mandala() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.editor = Editor::new("([\"merge\"] \"left\" \"right\")");
        workspace.refresh();
        // Switch off the default sigil view into the mandala.
        workspace.command = "mandala".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.visualization, Visualization::Mandala);
        assert!(workspace.graph_lines(200, 20)[0].contains("o-[]-o-[]-o"));
        workspace.command = "tree".to_string();
        assert_eq!(workspace.execute_kaos_command(), WorkspaceAction::None);
        assert_eq!(workspace.visualization, Visualization::Tree);
        assert!(workspace
            .graph_lines(200, 20)
            .join("\n")
            .contains("□ mediator square"));
        workspace.command = "mandala".to_string();
        workspace.execute_kaos_command();
        assert_eq!(workspace.visualization, Visualization::Mandala);
    }

    #[test]
    fn sigils_save_search_and_open_in_folders() {
        let root = std::env::temp_dir().join(format!(
            "kaos-sigil-folders-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = root.clone();
        workspace.editor = Editor::new("(~ fix (x) '(-> ,x \"repair\"))".to_string());
        workspace.refresh();

        // Save into a folder: directories are created on demand.
        workspace.save_sigil("repair/loop");
        assert!(root.join("repair/loop.rebis").is_file());

        // Search walks folders and lists the qualified name.
        workspace.search_sigils("repair/");
        assert!(workspace
            .sigil_results
            .contains(&SigilEntry::Saved("repair/loop".into())));

        // Open by qualified name.
        workspace.editor = Editor::new(String::new());
        workspace.open_sigil("repair/loop");
        assert!(workspace.editor.source().contains("(~ fix (x)"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn saved_sigils_restore_their_own_output_sidecars() {
        let root = std::env::temp_dir().join(format!(
            "kaos-sigil-output-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        fs::create_dir_all(&root).unwrap();
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = root.clone();

        workspace.editor = Editor::new("\"first sigil\"");
        workspace.refresh();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("result   first state");
        workspace.finish_run(0);
        workspace.save_sigil("first");
        assert_eq!(
            fs::read_to_string(root.join("first.output")).unwrap(),
            "first state"
        );

        workspace.editor = Editor::new("\"second sigil\"");
        workspace.refresh();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("result   second state");
        workspace.finish_run(0);
        workspace.save_sigil("second");

        workspace.open_sigil("first");
        assert_eq!(workspace.final_output.as_deref(), Some("first state"));
        workspace.command = "run".to_string();
        assert!(matches!(
            workspace.execute_kaos_command(),
            WorkspaceAction::Run(RunRequest { input, .. }) if input.is_empty()
        ));

        workspace.open_sigil("second");
        assert_eq!(workspace.final_output.as_deref(), Some("second state"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn browser_shows_std_collapsed_and_tab_expands_it() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = std::env::temp_dir().join("kaos-sigils-none");
        // Empty query: the std folder is one collapsed row.
        workspace.search_sigils("");
        let rows = workspace.visible_sigil_rows();
        assert!(rows.iter().any(|row| matches!(
            row,
            VisibleRow::Folder { path, expanded, .. } if path == "std" && !*expanded
        )));
        assert!(!rows.iter().any(|row| matches!(
            row,
            VisibleRow::Leaf { entry, .. } if matches!(entry, SigilEntry::Std(_))
        )));
        // The std folder is the only path-bearing row, so it is the selection.
        workspace.sigil_choice = rows
            .iter()
            .position(|row| matches!(row, VisibleRow::Folder { path, .. } if path == "std"))
            .unwrap();
        assert!(workspace.toggle_selected_folder());
        // Now every std module is a visible leaf under the folder.
        let expanded = workspace.visible_sigil_rows();
        let leaves = expanded
            .iter()
            .filter(|row| matches!(row, VisibleRow::Leaf { entry, .. } if matches!(entry, SigilEntry::Std(_))))
            .count();
        assert_eq!(leaves, rebis_lang::std_modules().len());
        // The collapsed marker flipped to expanded in the render.
        assert!(workspace.graph_lines(60, 60).join("\n").contains("▾ std/"));
    }

    #[test]
    fn a_query_auto_expands_matching_folders() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = std::env::temp_dir().join("kaos-sigils-none");
        workspace.search_sigils("std/spr");
        // The one match (std/spread) is visible without a manual expand.
        let rows = workspace.visible_sigil_rows();
        assert!(rows.iter().any(|row| matches!(
            row,
            VisibleRow::Leaf { entry, .. } if matches!(entry, SigilEntry::Std(n) if n == "std/spread")
        )));
    }

    #[test]
    fn clicking_a_folder_toggles_and_clicking_a_leaf_opens() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.sigils_root = std::env::temp_dir().join("kaos-sigils-none");
        workspace.search_sigils("");
        workspace.panel_inner = Some((50, 1, 40, 30));
        // Row y=1 is the SIGILS header.
        assert!(!workspace.click_sigil(55, 1));
        // The std folder row: clicking it expands (opens no editor buffer).
        let folder_line = workspace
            .visible_sigil_rows()
            .iter()
            .position(|row| matches!(row, VisibleRow::Folder { path, .. } if path == "std"))
            .unwrap();
        let source_before = workspace.editor.source().to_string();
        assert!(workspace.click_sigil(55, 2 + folder_line as u16));
        assert_eq!(workspace.editor.source(), source_before);
        assert!(workspace.expanded_folders.contains("std"));
        // Now a child leaf is clickable and opens into the editor.
        let leaf_line = workspace
            .visible_sigil_rows()
            .iter()
            .position(|row| matches!(row, VisibleRow::Leaf { entry, .. } if matches!(entry, SigilEntry::Std(n) if n == "std/canon")))
            .unwrap();
        assert!(workspace.click_sigil(55, 2 + leaf_line as u16));
        assert!(workspace.editor.source().contains("std/canon"));
    }

    #[test]
    fn searching_falls_through_to_contents_and_lands_on_the_line() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();

        // A name search is browsing: `std/` lists that folder and nothing else,
        // even though nearly every module in the library imports from it. A
        // plain grep would return most of the library and be useless for this.
        workspace.search_sigils("std/");
        assert_eq!(
            workspace
                .sigil_results
                .iter()
                .filter(|entry| matches!(entry, SigilEntry::Std(name) if name.starts_with("std/")))
                .count(),
            rebis_lang::std_modules().len()
        );
        assert!(
            workspace.sigil_hits.is_empty(),
            "a name search read contents"
        );

        // A macro nothing is named after falls through to the contents, and the
        // result records where.
        workspace.search_sigils("std-memo");
        let found = workspace
            .sigil_results
            .iter()
            .find(|entry| matches!(entry, SigilEntry::Std(name) if name == "std/correspondence"))
            .cloned()
            .expect("the macro is defined in std/correspondence");
        let (line, text) = workspace
            .sigil_hits
            .get(&found)
            .cloned()
            .expect("a content match records its line");
        assert!(line > 0);
        assert!(
            text.to_ascii_lowercase().contains("std-memo"),
            "the recorded line is not the matching one: {text}"
        );
        assert!(
            workspace.message.contains("contain it"),
            "{}",
            workspace.message
        );

        // The browser says where, so the question is answered before anything
        // is opened.
        let row = workspace
            .visible_sigil_rows()
            .into_iter()
            .find_map(|row| match row {
                VisibleRow::Leaf { entry, label, .. } if entry == found => Some(label),
                _ => None,
            })
            .expect("the hit is on a row");
        assert!(
            row.contains(&format!(":{line}")),
            "the row hides the line: {row}"
        );

        // And opening it lands there rather than at the top.
        workspace.sigil_choice = workspace
            .visible_sigil_rows()
            .iter()
            .position(|row| matches!(row, VisibleRow::Leaf { entry, .. } if *entry == found))
            .expect("the hit is selectable");
        workspace.open_selected_sigil();
        let (row, _) = workspace.editor.row_col();
        assert_eq!(row + 1, line, "opening did not go to the matched line");
        assert!(
            workspace
                .editor
                .source()
                .lines()
                .nth(row)
                .is_some_and(|at| at.contains("std-memo")),
            "the cursor landed away from the match"
        );
    }

    #[test]
    fn sigil_search_lists_the_embedded_std_folder() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.search_sigils("std/");
        let std_entries = workspace
            .sigil_results
            .iter()
            .filter(|entry| matches!(entry, SigilEntry::Std(_)))
            .count();
        assert_eq!(std_entries, rebis_lang::std_modules().len());
        // A narrower query filters within the folder.
        workspace.search_sigils("std/spr");
        assert_eq!(
            workspace.sigil_results,
            vec![SigilEntry::Std("std/spread".into())]
        );
    }

    #[test]
    fn opening_an_embedded_std_sigil_loads_its_source() {
        let mut workspace = Workspace::open(PathBuf::from("."), None).unwrap();
        workspace.open_sigil("std/flow");
        assert!(workspace
            .editor
            .source()
            .contains("(~ std-apply (worker value)"));
        assert!(workspace.message.contains("embedded std"));
        // The buffer is a copy: no sigil identity, and saving back into
        // std/ stays refused at the command layer.
        assert_eq!(workspace.current_sigil, None);
        workspace.command = "sigil save std/mine".to_string();
        workspace.execute_kaos_command();
        assert!(workspace.message.contains("standard library"));
    }
}
