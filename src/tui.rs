//! The fullscreen app — a ratatui terminal UI for the Pact.
//!
//! A scrollable transcript, an input line with arrow-key history, a status bar, and
//! the red Chaos Star on the splash. Commands are executed by re-invoking the `kaos`
//! binary in one-shot mode as a **subprocess**, whose themed ANSI output is streamed
//! back through a pipe and rendered into the transcript with `ansi-to-tui`. That way
//! ratatui owns the screen (the subprocess writes to a pipe, never the terminal), and
//! long/agent commands stream live. The model is passed down via `KAOS_MODEL`.

use std::io::{self, BufRead, BufReader, Read, Write};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use ansi_to_tui::IntoText;
#[cfg(not(test))]
use base64::Engine;
use crossterm::event::EnableMouseCapture;
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, Event, KeyCode,
    KeyEventKind, KeyModifiers, KeyboardEnhancementFlags, MouseButton, MouseEventKind,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use ratatui::buffer::Buffer;
use ratatui::layout::{Alignment, Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{Block, Borders, Clear, Paragraph};
use ratatui::{DefaultTerminal, Frame};

use crate::rebis_workspace::{
    self, Highlight, Mode as RebisMode, NormalAction, RunRequest, RunScope, Visualization,
    Workspace as RebisWorkspace, WorkspaceAction, WorkspaceEvent,
};
use crate::theme;

// The terminal palette, resolved once at startup from the configured mode
// (`/theme dark|light`). Monochrome by design: the shapes and rules carry the
// meaning, so colour only separates figure from ground.
fn tone(rgb: (u8, u8, u8)) -> Color {
    Color::Rgb(rgb.0, rgb.1, rgb.2)
}
fn c_ink() -> Color {
    tone(crate::theme::current().ink)
}
fn c_faint() -> Color {
    tone(crate::theme::current().faint)
}
static PALETTE: std::sync::LazyLock<crate::theme::Palette> =
    std::sync::LazyLock::new(crate::theme::current);
#[allow(non_snake_case)]
fn C_RED() -> Color {
    tone(PALETTE.accent)
}
#[allow(non_snake_case)]
fn C_OX() -> Color {
    tone(PALETTE.faint)
}
#[allow(non_snake_case)]
fn C_ASH() -> Color {
    tone(PALETTE.faint)
}
#[allow(non_snake_case)]
fn C_BONE() -> Color {
    tone(PALETTE.ink)
}
// The accents. With colour gone these separate by brightness instead of hue,
// which is why the palette carries a `mid` tone between ink and faint.
/// The page the whole app is drawn on.
fn c_ground() -> Color {
    tone(PALETTE.ground)
}
#[allow(non_snake_case)]
fn C_GOLD() -> Color {
    tone(PALETTE.accent)
}
#[allow(non_snake_case)]
fn C_TEAL() -> Color {
    tone(PALETTE.mid)
}
#[allow(non_snake_case)]
fn C_BLUE() -> Color {
    tone(PALETTE.faint)
}
/// A finished run. Formerly green; now simply the brightest tone.
#[allow(non_snake_case)]
fn C_DONE() -> Color {
    tone(PALETTE.ink)
}
/// Internal marker distinguishing a literal chat intent from `/code`'s CLI
/// grammar (`[dir] [xK] task -- gate`). It is consumed before spawning.
const RAW_CHAT_TASK_ARG: &str = "__kaos_raw_chat_task__";
const RAW_CHAT_TASK_ENV: &str = "KAOS_RAW_CHAT_TASK_STDIN";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CommandSpec {
    display: &'static str,
    insert: &'static str,
}

const fn command(display: &'static str, insert: &'static str) -> CommandSpec {
    CommandSpec { display, insert }
}

const MAIN_SLASH_COMMANDS: &[CommandSpec] = &[
    command("rebis [FILE]", "rebis "),
    command("runs", "runs"),
    command("config", "config"),
    command("config restore", "config restore"),
    command("sigils [QUERY]", "sigils "),
    command("code [PATH] [INTENT]", "code "),
    command("cast [INTENT]", "cast "),
    command("attach [FILE]", "attach "),
    command("cd [DIR]", "cd "),
    command("model [MODEL]", "model "),
    command("chaos [on|off]", "chaos"),
    command("mouse [on|off]", "mouse"),
    command("new", "new"),
    command("clear", "clear"),
    command("quit", "quit"),
];
const MODEL_SELECTIONS: &[CommandSpec] = &[
    command("model claude", "model claude"),
    command("model claude:sonnet", "model claude:sonnet"),
    command("model claude:opus", "model claude:opus"),
    command("model claude:haiku", "model claude:haiku"),
    command("model claude:fable", "model claude:fable"),
    command("model anthropic", "model anthropic"),
    command("model openai", "model openai"),
    command("model openrouter", "model openrouter"),
    command("model ollama", "model ollama"),
    command("model sim", "model sim"),
];
const REBIS_SLASH_COMMANDS: &[CommandSpec] = &[
    command("chat", "chat"),
    command("config", "config"),
    command("config restore", "config restore"),
    command("model [MODEL]", "model "),
    command("chaos", "chaos"),
    command("chaos on", "chaos on"),
    command("chaos off", "chaos off"),
    command("mouse [on|off]", "mouse"),
    command("new", "new"),
    command("clear", "clear"),
    command("quit", "quit"),
    command("run", "run"),
    command("run parallel", "run parallel"),
    command("run block", "run block"),
    command("run block parallel", "run block parallel"),
    command("runs", "runs"),
    command("save [FILE]", "save "),
    command("vim on", "vim on"),
    command("vim off", "vim off"),
    command("vim always", "vim always"),
    command("vim never", "vim never"),
    command("output", "output"),
    command("output copy", "output copy"),
    command("output write [FILE]", "output write "),
    command("mandala", "mandala"),
    command("visual", "visual"),
    command("visual open", "visual open"),
    command("theme dark", "theme dark"),
    command("theme light", "theme light"),
    command("sessions", "sessions"),
    command("resume [N|id]", "resume "),
    command("forget-session [N|id]", "forget-session "),
    command("tree", "tree"),
    command("graph", "graph"),
    command("source", "source"),
    command("panel", "panel"),
    command("panel hide", "panel hide"),
    command("panel show", "panel show"),
    command("format", "format"),
    command("format!", "format!"),
    command("search [TEXT]", "search "),
    command("sigils [QUERY]", "sigils "),
    command("sigil chat", "sigil chat"),
    command("sigil save [NAME]", "sigil save "),
    command("sigil open [NAME|temp:N]", "sigil open "),
    command("record [FILE]", "record "),
    command("help", "help"),
];

fn completions(query: &str, catalog: &'static [CommandSpec]) -> Vec<CommandSpec> {
    let query = query.trim_start_matches('/').to_ascii_lowercase();
    catalog
        .iter()
        .copied()
        .filter(|command| {
            command.insert.starts_with(&query)
                || (command.insert.ends_with(' ') && query.starts_with(command.insert))
        })
        .collect()
}

fn main_completions(query: &str) -> Vec<CommandSpec> {
    let normalized = query.trim_start_matches('/').to_ascii_lowercase();
    if normalized.starts_with("model ") {
        MODEL_SELECTIONS
            .iter()
            .copied()
            .filter(|model| model.insert.starts_with(&normalized))
            .collect()
    } else {
        completions(query, MAIN_SLASH_COMMANDS)
    }
}

fn rebis_completions(query: &str) -> Vec<CommandSpec> {
    let normalized = query.trim_start_matches('/').to_ascii_lowercase();
    if normalized.starts_with("model ") {
        MODEL_SELECTIONS
            .iter()
            .copied()
            .filter(|model| model.insert.starts_with(&normalized))
            .collect()
    } else {
        completions(query, REBIS_SLASH_COMMANDS)
    }
}

fn missing_command_argument(query: &str, command: CommandSpec) -> bool {
    let query = query.trim_start_matches('/');
    command.display.contains('<')
        && command.insert.ends_with(' ')
        && query.trim_end() == command.insert.trim_end()
}

fn red_bold() -> Style {
    Style::new().fg(C_RED()).add_modifier(Modifier::BOLD)
}

/// Render a status line while reserving the right edge for the selected model.
/// Returning the status rectangle lets command-mode cursor placement stay out
/// of the model badge.
fn render_footer_with_model(f: &mut Frame, area: Rect, status: Line<'_>, model: &str) -> Rect {
    let badge = format!(" MODEL {model} ");
    let badge_width = (badge.chars().count() as u16).min(area.width);
    let areas =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(badge_width)]).split(area);
    f.render_widget(Paragraph::new(status), areas[0]);
    f.render_widget(
        Paragraph::new(Span::styled(
            badge,
            Style::new()
                .fg(Color::Black)
                .bg(C_GOLD())
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Right),
        areas[1],
    );
    areas[0]
}

/// Braille spinner frames (advanced by wall-clock, so it spins smoothly regardless
/// of how often the UI redraws).
const SPIN: [&str; 10] = [
    "\u{280b}", "\u{2819}", "\u{2839}", "\u{2838}", "\u{283c}", "\u{2834}", "\u{2826}", "\u{2827}",
    "\u{2807}", "\u{280f}",
];

/// Strip ANSI escape sequences so streamed, coloured output can be shown as plain
/// text in the one-line status bar.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for n in chars.by_ref() {
                    if n.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Parse an ANSI-bearing string into a single styled `Line` (its first line). Used
/// to turn a fold's summary marker payload into a renderable header.
fn ansi_first_line(s: &str) -> Line<'static> {
    match s.into_text() {
        Ok(text) => text.lines.into_iter().next().unwrap_or_default(),
        Err(_) => Line::raw(s.to_string()),
    }
}

/// A message from a running command subprocess.
enum Msg {
    Line(String),
    Done(i32),
}

/// One item in the transcript: either a plain rendered line, or a collapsible fold
/// (a summary the reader always sees + a body they can expand). Folds are how the
/// app shows "what the system is doing" without drowning the screen — a coding
/// conclave streams each adept's steps into a fold that opens on demand.
enum Entry {
    Line(Line<'static>),
    Fold(Fold),
}

struct Fold {
    /// The always-visible summary (parsed from the child's FOLD_OPEN marker).
    summary: Line<'static>,
    body: Vec<Line<'static>>,
    collapsed: bool,
}

/// A running command.
struct Job {
    child: Arc<Mutex<Child>>,
    rx: Receiver<Msg>,
    label: String,
    /// True only when this job actually creates/resumes a claude conversation (the
    /// single-adept `code` path on a claude-cli mind) — on success it marks the
    /// session resumable. A conclave or a non-claude mind never touches the session,
    /// so marking those would make the next turn `--resume` a UUID that was never
    /// created.
    claude_session: bool,
    /// Identity of a hosted Rebis run. Its stream is retained in the run tree
    /// as well as the workspace output pane and chat transcript.
    rebis_run_id: Option<u64>,
    /// Hosted children lead their own process group, so pause/cancel includes
    /// model and command descendants instead of orphaning them.
    owns_process_group: bool,
}

struct ChildTransport {
    args: Vec<String>,
    stdin: Option<String>,
    raw_chat_task: bool,
    label: String,
}

/// Move literal chat text off argv and onto stdin. Besides avoiding the OS's
/// per-argument limit, this prevents pasted code containing ` -- `, `x3`, or a
/// directory-looking first token from being reinterpreted as `/code` syntax.
fn prepare_child_transport(args: Vec<String>, input: Option<String>) -> ChildTransport {
    let raw_chat_task = input.is_none()
        && args.first().is_some_and(|arg| arg == "code")
        && args.get(1).is_some_and(|arg| arg == RAW_CHAT_TASK_ARG);
    if raw_chat_task {
        let task = args.get(2).cloned().unwrap_or_default();
        return ChildTransport {
            args: vec!["code".to_string()],
            stdin: Some(task.clone()),
            raw_chat_task: true,
            label: format!(
                "code . {}",
                task.lines().next().unwrap_or("(empty chat task)")
            ),
        };
    }
    ChildTransport {
        label: args.join(" "),
        args,
        stdin: input,
        raw_chat_task: false,
    }
}

struct JobPoll {
    lines: Vec<String>,
    done: Option<i32>,
}

/// One isolated supervisory turn. The model edits a private bridge copy, never
/// the live editor buffer directly; completion validates and merges that copy
/// against the exact source revision on which the turn began.
struct SigilChatJob {
    job: Job,
    base_source: String,
    bridge_dir: PathBuf,
    run_id: Option<u64>,
    /// The channel paused this run, so a completed turn should continue it.
    /// A run that was already paused remains paused for the user to inspect.
    resume_after: bool,
}

const SAVED_REBIS_RUN_HEADER: &str = "KAOS_REBIS_SAVED_RUN_V1\n";

#[derive(Clone, Debug, Eq, PartialEq)]
struct SavedRebisRun {
    source: String,
    input: String,
    scope: RunScope,
    parallel: bool,
    chaos: bool,
    output: Vec<String>,
    elapsed: Duration,
    pause_reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SigilRunControl {
    Pause(u64),
    Resume(u64),
    ApplyDirective(u64),
    ClearDirective(u64),
}

fn parse_sigil_run_controls(source: &str) -> Result<Vec<SigilRunControl>, String> {
    let mut controls = Vec::new();
    for (line_index, raw) in source.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let command = words.next().unwrap_or_default();
        let id = words
            .next()
            .ok_or_else(|| format!("control line {} has no run id", line_index + 1))?
            .parse::<u64>()
            .map_err(|_| format!("control line {} has an invalid run id", line_index + 1))?;
        if words.next().is_some() {
            return Err(format!(
                "control line {} has unexpected trailing text",
                line_index + 1
            ));
        }
        controls.push(match command {
            "PAUSE" => SigilRunControl::Pause(id),
            "RESUME" => SigilRunControl::Resume(id),
            "APPLY_DIRECTIVE" => SigilRunControl::ApplyDirective(id),
            "CLEAR_DIRECTIVE" => SigilRunControl::ClearDirective(id),
            _ => {
                return Err(format!(
                    "control line {} uses unknown action {command:?}",
                    line_index + 1
                ))
            }
        });
    }
    Ok(controls)
}

fn push_saved_run_field(encoded: &mut Vec<u8>, value: &str) {
    encoded.extend_from_slice(value.len().to_string().as_bytes());
    encoded.push(b'\n');
    encoded.extend_from_slice(value.as_bytes());
    encoded.push(b'\n');
}

fn take_saved_run_field(encoded: &mut &[u8]) -> Result<String, String> {
    let Some(newline) = encoded.iter().position(|byte| *byte == b'\n') else {
        return Err("saved run field has no length terminator".to_string());
    };
    let length = std::str::from_utf8(&encoded[..newline])
        .map_err(|_| "saved run field length is not UTF-8".to_string())?
        .parse::<usize>()
        .map_err(|_| "saved run field length is not a number".to_string())?;
    *encoded = &encoded[newline + 1..];
    if encoded.len() < length + 1 || encoded[length] != b'\n' {
        return Err("saved run field is truncated".to_string());
    }
    let value = String::from_utf8(encoded[..length].to_vec())
        .map_err(|_| "saved run field is not UTF-8".to_string())?;
    *encoded = &encoded[length + 1..];
    Ok(value)
}

fn encode_saved_rebis_run(saved: &SavedRebisRun) -> Vec<u8> {
    let mut encoded = SAVED_REBIS_RUN_HEADER.as_bytes().to_vec();
    push_saved_run_field(&mut encoded, &saved.source);
    push_saved_run_field(&mut encoded, &saved.input);
    push_saved_run_field(
        &mut encoded,
        match saved.scope {
            RunScope::Program => "program",
            RunScope::Block => "block",
        },
    );
    push_saved_run_field(&mut encoded, if saved.parallel { "1" } else { "0" });
    push_saved_run_field(&mut encoded, if saved.chaos { "1" } else { "0" });
    push_saved_run_field(&mut encoded, &saved.elapsed.as_millis().to_string());
    push_saved_run_field(&mut encoded, &saved.pause_reason);
    push_saved_run_field(&mut encoded, &saved.output.len().to_string());
    for line in &saved.output {
        push_saved_run_field(&mut encoded, line);
    }
    encoded
}

fn decode_saved_rebis_run(encoded: &[u8]) -> Result<SavedRebisRun, String> {
    let Some(mut fields) = encoded.strip_prefix(SAVED_REBIS_RUN_HEADER.as_bytes()) else {
        return Err("unrecognized saved Rebis run format".to_string());
    };
    let source = take_saved_run_field(&mut fields)?;
    let input = take_saved_run_field(&mut fields)?;
    let scope = match take_saved_run_field(&mut fields)?.as_str() {
        "program" => RunScope::Program,
        "block" => RunScope::Block,
        _ => return Err("saved Rebis run has an invalid scope".to_string()),
    };
    let parallel = match take_saved_run_field(&mut fields)?.as_str() {
        "0" => false,
        "1" => true,
        _ => return Err("saved Rebis run has an invalid parallel flag".to_string()),
    };
    let chaos = match take_saved_run_field(&mut fields)?.as_str() {
        "0" => false,
        "1" => true,
        _ => return Err("saved Rebis run has an invalid chaos flag".to_string()),
    };
    let elapsed_ms = take_saved_run_field(&mut fields)?
        .parse::<u64>()
        .map_err(|_| "saved Rebis run has an invalid elapsed time".to_string())?;
    let pause_reason = take_saved_run_field(&mut fields)?;
    let output_count = take_saved_run_field(&mut fields)?
        .parse::<usize>()
        .map_err(|_| "saved Rebis run has an invalid output count".to_string())?;
    let mut output = Vec::with_capacity(output_count);
    for _ in 0..output_count {
        output.push(take_saved_run_field(&mut fields)?);
    }
    if !fields.is_empty() {
        return Err("saved Rebis run has trailing data".to_string());
    }
    Ok(SavedRebisRun {
        source,
        input,
        scope,
        parallel,
        chaos,
        output,
        elapsed: Duration::from_millis(elapsed_ms),
        pause_reason,
    })
}

fn atomic_write(path: &std::path::Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("tmp.{}", std::process::id()));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, path)
}

fn poll_job(job: &Job) -> JobPoll {
    let mut lines = Vec::new();
    let mut done = None;
    loop {
        match job.rx.try_recv() {
            Ok(Msg::Line(line)) => lines.push(line),
            Ok(Msg::Done(code)) => done = Some(code),
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                done.get_or_insert(-1);
                break;
            }
        }
    }
    JobPoll { lines, done }
}

/// One captured unit waiting behind the active subprocess. Rebis requests
/// carry their source and record input by value, so later editor changes cannot
/// mutate what will eventually run.
enum QueuedWork {
    Line(String),
    Rebis { id: u64, request: RunRequest },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RebisRunState {
    AwaitingPermission,
    Queued,
    Running,
    /// The program reached its successful result. Non-success exits remain
    /// `Running` + paused and therefore cannot enter a terminal failure state.
    Complete,
    Cancelled,
}

/// Durable UI history for one submitted run. Queued requests begin empty,
/// receive their text stream once active, and remain explorable after exit.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RebisRunEntry {
    id: u64,
    /// Saved-sigil identity and durable sidecars, when this unfinished run has
    /// been attached by `/sigil save` or restored by `/sigil open`.
    sigil: Option<String>,
    saved_run_path: Option<PathBuf>,
    saved_checkpoint_path: Option<PathBuf>,
    /// Captured source/input used both for the first child and for a replacement
    /// child. Ordinary editor changes cannot touch it; an explicit guarded
    /// `/sigil chat` revision may replace only `source` while preserving input,
    /// identity, trace, timers, and the prompt journal.
    request: RunRequest,
    /// Completed prompt answers, committed before the interpreter advances.
    checkpoint_path: PathBuf,
    /// Persistent guidance read by the child before every unfinished prompt.
    /// `/sigil chat` is the only writer; checkpoint replays ignore it.
    directive_path: PathBuf,
    scope: RunScope,
    preview: String,
    state: RebisRunState,
    output: Vec<String>,
    expanded: bool,
    /// Submission time lets queued and permission-gated runs expose how long
    /// they have waited before the model process actually starts.
    queued_at: Instant,
    started_at: Option<Instant>,
    /// Frozen execution duration once a run exits or is cancelled.
    elapsed: Option<Duration>,
    /// Explicit opt-in: this run owns a subprocess outside the shared FIFO.
    parallel: bool,
    /// Captured when submitted so toggling `/chaos` cannot change queued work.
    chaos: bool,
    /// A running run whose subprocess has been suspended (SIGSTOP) with `p`.
    /// It stays `Running` but makes no progress until resumed (SIGCONT).
    paused: bool,
    pause_reason: Option<String>,
    paused_at: Option<Instant>,
    paused_total: Duration,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct RebisRunView {
    entry: RebisRunEntry,
    /// One-based position in the shared FIFO, including queued chat work.
    queue_position: Option<usize>,
}

#[derive(Clone, Copy, Debug)]
struct RebisWorkspaceChrome<'a> {
    selected_model: &'a str,
    config_document: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum RebisRunTreeRow {
    Header(usize),
    Section {
        run: usize,
        depth: usize,
        kind: RebisRunSectionKind,
        title: String,
    },
    Output {
        run: usize,
        depth: usize,
        text: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RebisRunSectionKind {
    Agent,
    Model,
    Step,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum TextPaneKind {
    Chat,
    RebisSource,
    RebisPanel,
}

impl TextPaneKind {
    const fn label(self) -> &'static str {
        match self {
            Self::Chat => "chat",
            Self::RebisSource => "source",
            Self::RebisPanel => "right panel",
        }
    }
}

/// A plain-cell snapshot of one rendered text pane. Keeping panes separate is
/// what prevents a drag in the source from swallowing the right-hand panel.
#[derive(Clone, Debug, Eq, PartialEq)]
struct TextPane {
    kind: TextPaneKind,
    /// Full rendered pane, including non-document chrome such as a gutter.
    area: Rect,
    /// First selectable column. For Rebis/config source this begins after the
    /// line-number separator; other panes use `area.x`.
    content_left: u16,
    rows: Vec<Vec<String>>,
}

impl TextPane {
    fn content_area(&self) -> Rect {
        let left = self.content_left.clamp(self.area.x, self.area.right());
        Rect::new(
            left,
            self.area.y,
            self.area.right().saturating_sub(left),
            self.area.height,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct TextPaneRegion {
    kind: TextPaneKind,
    area: Rect,
    content_left: u16,
}

impl TextPaneRegion {
    const fn full(kind: TextPaneKind, area: Rect) -> Self {
        Self {
            kind,
            area,
            content_left: area.x,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PaneSelection {
    pane: TextPaneKind,
    anchor: Position,
    head: Position,
    dragged: bool,
    copied_text: String,
}

struct PendingRebisRun {
    id: u64,
    request: RunRequest,
    parallel: bool,
}

/// The application state.
pub struct App {
    /// Dedicated Rebis source/graph workspace. While present it owns the screen;
    /// background model jobs continue to stream into the retained transcript.
    rebis: Option<RebisWorkspace>,
    /// Workspace suspended while chat owns the screen.
    suspended_rebis: Option<RebisWorkspace>,
    /// `/config` reuses the editor while remembering where `:q` should return.
    config_editor: bool,
    config_return_rebis: Option<RebisWorkspace>,
    transcript: Vec<Entry>,
    /// The fold currently receiving streamed body lines, and how deeply nested we are
    /// (the child may open a fold inside a fold; we render one level and keep the rest
    /// as body, but must balance the CLOSE count).
    open_fold: Option<usize>,
    fold_depth: usize,
    /// The fold the reader has selected to toggle (ordinal among all folds).
    sel_fold: Option<usize>,
    scroll: u16,
    follow: bool,
    view_h: u16,
    /// Total wrapped display rows (set each draw) — the basis for scroll clamping.
    content_rows: u16,
    input: String,
    cursor: usize,
    command_choice: usize,
    history: Vec<String>,
    hist_nav: Option<usize>,
    stash: String,
    model: String, // "sim" | "claude" | "ollama:<model>"
    /// The conversation as durable data, saved on exit and reloadable with
    /// `/chat resume`. Separate from `transcript`, which is the rendered screen.
    session: crate::sessions::Session,
    /// Plain text streamed by the running job, flushed into `session` as one
    /// model turn when the job finishes.
    session_reply: String,
    cwd: PathBuf,
    job: Option<Job>,
    /// Opt-in Rebis evaluations running beside the shared serial job.
    parallel_jobs: Vec<Job>,
    /// Source-bound god-agent turn running beside Rebis without occupying its
    /// serial FIFO or ordinary chat conversation.
    sigil_chat_job: Option<SigilChatJob>,
    /// When the current job started (for the spinner + elapsed clock).
    job_start: Option<Instant>,
    /// The agent's latest activity — the most recent non-empty output line — shown
    /// next to the spinner so the status bar says *what* it is doing.
    activity: String,
    /// Whether the adept may run shell (bash), not only edit files. `None` until the
    /// first coding task forces the choice; then remembered for the session.
    yolo: Option<bool>,
    /// A coding job held back until the yolo question is answered.
    pending: Option<Vec<String>>,
    /// A chaos-mode Rebis run held until its tool-using agents receive authority.
    pending_rebis: Option<PendingRebisRun>,
    /// Further parallel runs waiting for the one-at-a-time authority prompt.
    parallel_gate_queue: Vec<PendingRebisRun>,
    /// Remembered authority for every later Rebis agent in this app session.
    rebis_authority: bool,
    /// Direct mode runs one tool agent per Rebis prompt on any backend — a
    /// native Claude agent on the Claude CLI, a node-scoped Conductor run
    /// elsewhere. Chaos mode explicitly opts into the full Kaos pipeline.
    rebis_chaos_mode: bool,
    /// Messages, commands, and Rebis evaluations submitted while a working was
    /// already underway. They run in one shared FIFO, one per finished job.
    queue: Vec<QueuedWork>,
    /// Submitted Rebis runs remain browsable through queued, running, and
    /// completed states; Tab expands each run's captured text stream.
    rebis_runs: Vec<RebisRunEntry>,
    rebis_run_choice: usize,
    /// Scroll offset in the flattened run/output tree. This is deliberately
    /// separate from the mandala's scroll so browsing runs never moves it.
    rebis_run_top: usize,
    next_rebis_run_id: u64,
    /// A stable UUID pinning ONE claude conversation to this session, so the agent
    /// keeps memory across turns. The first coding turn creates it; later turns
    /// resume it (`resumed`). `/new` mints a fresh one to forget.
    session_id: String,
    resumed: bool,
    quit: bool,
    /// Armed by an idle ^C: the exit is deferred until a confirming second ^C,
    /// so a single stray chord never drops the app. Any other key disarms it.
    confirm_quit: bool,
    /// Whether the terminal's mouse events are captured by the app (wheel scroll,
    /// click focus, and pane-local selection). `/mouse off` releases it for raw
    /// terminal selection instead.
    mouse_captured: bool,
    /// Last rendered text for each pane plus the current clipped mouse drag.
    text_panes: Vec<TextPane>,
    text_selection: Option<PaneSelection>,
    /// Kept alive because X11/Wayland clipboard contents are owned by the
    /// process that set them. This also supplies a reliable local fallback when
    /// a terminal consumes Ctrl-Shift-C or ignores OSC-52.
    clipboard: Option<arboard::Clipboard>,
}

/// Enter the alternate screen and run the app to completion, then restore.
pub fn run() -> io::Result<()> {
    run_with_rebis(Some(None))
}

/// Enter the application directly in the Rebis editor/graph workspace.
pub fn run_rebis(path: Option<&str>) -> io::Result<()> {
    run_with_rebis(Some(path))
}

fn run_with_rebis(rebis_path: Option<Option<&str>>) -> io::Result<()> {
    let mut terminal = ratatui::init();
    // Mouse capture lets Kaos clip a drag to the pane where it began. Raw
    // terminal selection remains available through `/mouse off`.
    let _ = execute!(io::stdout(), EnableMouseCapture);
    let _ = execute!(io::stdout(), EnableBracketedPaste);
    let _ = execute!(
        io::stdout(),
        PushKeyboardEnhancementFlags(
            KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
                | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS
        )
    );
    let mut app = App::new();
    if let Some(path) = rebis_path {
        app.open_rebis(path);
    }
    let res = app.run_loop(&mut terminal);
    // Persist before tearing the terminal down, so a session survives any exit
    // path — /quit, Ctrl-C, or an error out of the loop.
    app.save_session();
    let _ = execute!(io::stdout(), PopKeyboardEnhancementFlags);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    let _ = execute!(io::stdout(), DisableBracketedPaste);
    ratatui::restore();
    res
}

impl App {
    fn new() -> App {
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        let mut app = App {
            rebis: None,
            suspended_rebis: None,
            config_editor: false,
            config_return_rebis: None,
            transcript: Vec::new(),
            open_fold: None,
            fold_depth: 0,
            sel_fold: None,
            scroll: 0,
            follow: true,
            view_h: 1,
            content_rows: 1,
            input: String::new(),
            cursor: 0,
            command_choice: 0,
            history: load_history(),
            hist_nav: None,
            stash: String::new(),
            model: crate::provider::Spec::parse(
                &std::env::var("KAOS_MODEL").unwrap_or_else(|_| "sim".to_string()),
            )
            .canonical(),
            session: crate::sessions::Session::new(
                std::env::var("KAOS_MODEL").unwrap_or_else(|_| "sim".to_string()),
                cwd.display().to_string(),
            ),
            session_reply: String::new(),
            cwd,
            job: None,
            parallel_jobs: Vec::new(),
            sigil_chat_job: None,
            job_start: None,
            activity: String::new(),
            // Honour a pre-set env var; otherwise leave undecided so the first
            // coding task raises the question.
            yolo: std::env::var("KAOS_CLAUDE_YOLO")
                .ok()
                .map(|v| !matches!(v.as_str(), "0" | "false" | "no" | "")),
            pending: None,
            pending_rebis: None,
            parallel_gate_queue: Vec::new(),
            rebis_authority: false,
            // One direct provider agent per prompt is the default. File/command
            // authority is gated independently; `/chaos on` is a larger,
            // explicit orchestration choice, not the way tools are enabled.
            rebis_chaos_mode: false,
            queue: Vec::new(),
            rebis_runs: Vec::new(),
            rebis_run_choice: 0,
            rebis_run_top: 0,
            next_rebis_run_id: 1,
            session_id: gen_uuid(),
            resumed: false,
            quit: false,
            confirm_quit: false,
            // Captured by default so selection understands pane boundaries.
            mouse_captured: true,
            text_panes: Vec::new(),
            text_selection: None,
            clipboard: None,
        };
        app.splash();
        app
    }

    fn splash(&mut self) {
        self.push_line(Line::raw(""));
        for l in theme::chaos_star_lines() {
            self.push_line(Line::from(Span::styled(l.to_string(), red_bold())));
        }
        self.push_line(Line::raw(""));
        self.push_line(Line::from(vec![
            Span::styled("✴ kaos", red_bold()),
            Span::styled("  — the Pact convenes.", Style::new().fg(C_ASH())),
        ]));
        self.push_line(Line::from(Span::styled(
            "speak an intent — the adept works these files · /cast for a one-shot · /help",
            Style::new().fg(C_ASH()),
        )));
        self.push_line(Line::raw(""));
    }

    /// Snapshot the already-rendered pane cells, then paint the active clipped
    /// selection over them. Text extraction therefore sees the real display but
    /// never crosses from one pane into another.
    fn capture_text_panes(&mut self, f: &mut Frame, regions: &[TextPaneRegion]) {
        let buffer = f.buffer_mut();
        self.text_panes = regions
            .iter()
            .copied()
            .filter(|region| {
                region.area.width > 0
                    && region.area.height > 0
                    && region.content_left < region.area.right()
            })
            .map(|region| snapshot_text_pane(buffer, region))
            .collect();

        let Some(selection) = &self.text_selection else {
            return;
        };
        let Some(pane) = self
            .text_panes
            .iter()
            .find(|pane| pane.kind == selection.pane)
        else {
            self.text_selection = None;
            return;
        };
        highlight_pane_selection(buffer, pane.content_area(), selection);
    }

    fn begin_pane_selection(&mut self, column: u16, row: u16) -> bool {
        let position = Position { x: column, y: row };
        let Some(pane) = self
            .text_panes
            .iter()
            .find(|pane| point_in_rect(position, pane.area))
        else {
            self.text_selection = None;
            return false;
        };
        let position = clamp_to_rect(position, pane.content_area());
        self.text_selection = Some(PaneSelection {
            pane: pane.kind,
            anchor: position,
            head: position,
            dragged: false,
            copied_text: String::new(),
        });
        true
    }

    fn drag_pane_selection(&mut self, column: u16, row: u16) -> bool {
        let dragging_chat = {
            let Some(selection) = &mut self.text_selection else {
                return false;
            };
            let Some(pane) = self
                .text_panes
                .iter()
                .find(|pane| pane.kind == selection.pane)
            else {
                return false;
            };
            selection.head = clamp_to_rect(Position { x: column, y: row }, pane.content_area());
            selection.dragged |= selection.head != selection.anchor;
            selection.dragged && selection.pane == TextPaneKind::Chat
        };
        // A real drag over the chat transcript means the reader has left the
        // live tail to copy something. Stop auto-following so new streamed
        // output does not scroll the transcript — and thus the highlighted
        // range — out from under the cursor mid-selection. A bare click never
        // reaches here (it stays `dragged == false`), so it leaves follow
        // alone; End or scrolling back to the bottom re-follows as usual.
        if dragging_chat {
            self.follow = false;
        }
        true
    }

    /// Finish a drag and return whether it was a selection (rather than a
    /// click), which pane owned it, and the pane-local text copied.
    fn finish_pane_selection(
        &mut self,
        column: u16,
        row: u16,
    ) -> Option<(bool, TextPaneKind, String)> {
        self.drag_pane_selection(column, row);
        let selection = self.text_selection.as_mut()?;
        if !selection.dragged {
            let pane = selection.pane;
            self.text_selection = None;
            return Some((false, pane, String::new()));
        }
        let pane = self
            .text_panes
            .iter()
            .find(|pane| pane.kind == selection.pane)?;
        let text = selected_pane_text(pane, selection);
        selection.copied_text.clone_from(&text);
        Some((true, selection.pane, text))
    }

    fn announce_pane_copy(&mut self, pane: TextPaneKind, text: &str, copied: bool) {
        let message = if text.is_empty() {
            format!("empty {} selection", pane.label())
        } else if copied {
            format!(
                "copied {} character(s) from {} · /mouse off uses raw terminal selection",
                text.chars().count(),
                pane.label()
            )
        } else {
            format!(
                "selected {} text, but the terminal clipboard refused it",
                pane.label()
            )
        };
        if let Some(workspace) = &mut self.rebis {
            workspace.message = message;
        } else {
            self.activity = message;
        }
    }

    /// Copy the active pane-local drag selection without changing focus,
    /// cancelling a job, or clearing the highlighted range.
    fn copy_active_pane_selection(&mut self) {
        let selected = self.text_selection.as_ref().and_then(|selection| {
            self.text_panes
                .iter()
                .find(|pane| pane.kind == selection.pane)
                .map(|pane| {
                    let text = if selection.copied_text.is_empty() {
                        selected_pane_text(pane, selection)
                    } else {
                        selection.copied_text.clone()
                    };
                    (selection.pane, text)
                })
        });
        let Some((pane, text)) = selected else {
            let message = "no active pane selection · drag over text first".to_string();
            if let Some(workspace) = &mut self.rebis {
                workspace.message = message;
            } else {
                self.activity = message;
            }
            return;
        };
        let copied = text.is_empty() || self.copy_to_clipboard(&text).is_ok();
        self.announce_pane_copy(pane, &text, copied);
    }

    /// Put pane text on the native desktop clipboard when available. Retaining
    /// the `Clipboard` in `App` keeps Linux clipboard ownership alive; OSC-52
    /// remains useful over SSH and in terminals without a desktop connection.
    fn copy_to_clipboard(&mut self, text: &str) -> io::Result<()> {
        #[cfg(not(test))]
        {
            if self.clipboard.is_none() {
                self.clipboard = arboard::Clipboard::new().ok();
            }
            if let Some(clipboard) = &mut self.clipboard {
                if clipboard.set_text(text.to_string()).is_ok() {
                    return Ok(());
                }
                // Reconnect on the next copy if the compositor/X server went
                // away while Kaos was running.
                self.clipboard = None;
            }
            copy_to_terminal_clipboard(text)
        }
        #[cfg(test)]
        {
            // Unit tests exercise routing without touching the developer's
            // real desktop clipboard.
            let _ = &mut self.clipboard;
            copy_to_terminal_clipboard(text)
        }
    }

    fn register_rebis_run(&mut self, request: &RunRequest, state: RebisRunState) -> u64 {
        self.register_rebis_run_with_mode(request, state, false)
    }

    fn register_rebis_run_with_mode(
        &mut self,
        request: &RunRequest,
        state: RebisRunState,
        parallel: bool,
    ) -> u64 {
        let id = self.next_rebis_run_id;
        self.next_rebis_run_id += 1;
        let now = Instant::now();
        let already_started = matches!(
            state,
            RebisRunState::Running | RebisRunState::Complete | RebisRunState::Cancelled
        );
        let already_finished = matches!(state, RebisRunState::Complete | RebisRunState::Cancelled);
        let workspace = if self.config_editor {
            self.config_return_rebis
                .as_ref()
                .or(self.suspended_rebis.as_ref())
        } else {
            self.rebis.as_ref().or(self.suspended_rebis.as_ref())
        };
        let saved_identity = workspace.and_then(|workspace| {
            workspace.current_sigil().map(|name| {
                let (run_path, checkpoint_path) = workspace.sigil_resume_paths(name);
                (name.to_string(), run_path, checkpoint_path)
            })
        });
        self.rebis_runs.push(RebisRunEntry {
            id,
            sigil: saved_identity.as_ref().map(|saved| saved.0.clone()),
            saved_run_path: saved_identity.as_ref().map(|saved| saved.1.clone()),
            saved_checkpoint_path: saved_identity.as_ref().map(|saved| saved.2.clone()),
            request: request.clone(),
            checkpoint_path: std::env::temp_dir()
                .join(format!("kaos-rebis-{}-{id}.checkpoint", self.session_id)),
            directive_path: std::env::temp_dir()
                .join(format!("kaos-rebis-{}-{id}.directive", self.session_id)),
            scope: request.scope,
            preview: rebis_source_preview(&request.source),
            state,
            output: Vec::new(),
            expanded: state == RebisRunState::Running,
            queued_at: now,
            started_at: already_started.then_some(now),
            elapsed: already_finished.then_some(Duration::ZERO),
            parallel,
            chaos: self.rebis_chaos_mode,
            paused: false,
            pause_reason: None,
            paused_at: None,
            paused_total: Duration::ZERO,
        });
        self.rebis_run_choice = self.rebis_runs.len() - 1;
        id
    }

    fn has_active_jobs(&self) -> bool {
        self.job.is_some()
            || !self.parallel_jobs.is_empty()
            || self.rebis_runs.iter().any(|run| {
                !run.parallel
                    && run.state == RebisRunState::Running
                    && run.paused
                    && self.job_for_run(run.id).is_none()
            })
    }

    fn rebis_run_views(&self) -> Vec<RebisRunView> {
        self.rebis_runs
            .iter()
            .cloned()
            .map(|entry| {
                let queue_position = self
                    .queue
                    .iter()
                    .enumerate()
                    .find_map(|(index, work)| {
                        matches!(work, QueuedWork::Rebis { id, .. } if *id == entry.id)
                            .then_some(index + 1)
                    })
                    .or_else(|| {
                        self.parallel_gate_queue
                            .iter()
                            .position(|pending| pending.id == entry.id)
                            .map(|index| index + 1)
                    });
                RebisRunView {
                    entry,
                    queue_position,
                }
            })
            .collect()
    }

    fn rebis_run_max_top(&self) -> usize {
        let (panel_width, panel_height) = self
            .rebis
            .as_ref()
            .or(self.suspended_rebis.as_ref())
            .and_then(|workspace| workspace.panel_inner)
            .map_or((80, 3), |(_, _, width, height)| (width, height));
        let row_count = rebis_run_display_rows(&self.rebis_run_views(), panel_width as usize).len();
        if row_count == 0 {
            return 0;
        }
        let visible = row_count
            .min(panel_height.saturating_sub(2) as usize)
            .max(1);
        row_count.saturating_sub(visible)
    }

    fn scroll_rebis_runs(&mut self, delta: isize) {
        let max_top = self.rebis_run_max_top();
        self.rebis_run_top = self
            .rebis_run_top
            .min(max_top)
            .saturating_add_signed(delta)
            .min(max_top);
    }

    fn clamp_rebis_run_choice(&mut self) {
        self.rebis_run_choice = if self.rebis_runs.is_empty() {
            0
        } else {
            self.rebis_run_choice.min(self.rebis_runs.len() - 1)
        };
    }

    fn focus_selected_rebis_run(&mut self) {
        self.clamp_rebis_run_choice();
        let views = self.rebis_run_views();
        let panel_width = self
            .rebis
            .as_ref()
            .or(self.suspended_rebis.as_ref())
            .and_then(|workspace| workspace.panel_inner)
            .map_or(80, |(_, _, width, _)| width as usize);
        let rows = rebis_run_display_rows(&views, panel_width);
        let header = rows
            .iter()
            .position(
                |row| matches!(row, RebisRunTreeRow::Header(run) if *run == self.rebis_run_choice),
            )
            .unwrap_or(0);
        self.rebis_run_top = header;
    }

    fn describe_rebis_run_choice(&mut self) {
        self.clamp_rebis_run_choice();
        self.focus_selected_rebis_run();
        let message = self.rebis_runs.get(self.rebis_run_choice).map(|run| {
            let state = match run.state {
                RebisRunState::AwaitingPermission => {
                    "awaiting authority · y once · a sigil · n deny"
                }
                RebisRunState::Queued => "queued · u/Delete removes · Tab expands",
                RebisRunState::Running if run.paused => {
                    "paused · p resumes · Tab stream · ↑/↓ scroll · ^C cancels the run"
                }
                RebisRunState::Running => {
                    "running · p pauses · Tab stream · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · ^C stops the run"
                }
                RebisRunState::Complete => {
                    "complete · Tab stream · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · Pg scroll · u/Delete removes"
                }
                RebisRunState::Cancelled => {
                    "cancelled · Tab stream · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · Pg scroll · u/Delete removes"
                }
            };
            format!(
                "Rebis {} {}/{}{} · {} · {state}",
                run.scope.label(),
                self.rebis_run_choice + 1,
                self.rebis_runs.len(),
                if run.parallel { " · parallel" } else { "" },
                rebis_run_timer(run)
            )
        });
        if let (Some(workspace), Some(message)) = (&mut self.rebis, message) {
            workspace.message = message;
        }
    }

    fn toggle_selected_rebis_run(&mut self) -> bool {
        self.clamp_rebis_run_choice();
        let Some(run) = self.rebis_runs.get_mut(self.rebis_run_choice) else {
            return false;
        };
        run.expanded = !run.expanded;
        self.describe_rebis_run_choice();
        true
    }

    /// The subprocess of the run with `id`, whether it is the active serial job
    /// or one of the independent parallel jobs.
    fn job_for_run(&self, id: u64) -> Option<&Job> {
        if let Some(job) = &self.job {
            if job.rebis_run_id == Some(id) {
                return Some(job);
            }
        }
        self.parallel_jobs
            .iter()
            .find(|job| job.rebis_run_id == Some(id))
    }

    /// Suspend or resume the selected run with `p`. A live child receives
    /// SIGSTOP/SIGCONT. If an interrupted child has already exited, `p` launches
    /// a replacement from the captured request; its prompt journal replays every
    /// completed answer and retries the first unfinished prompt.
    fn toggle_pause_selected_rebis_run(&mut self) -> bool {
        self.clamp_rebis_run_choice();
        let Some(selected) = self.rebis_runs.get(self.rebis_run_choice) else {
            return false;
        };
        if selected.state != RebisRunState::Running {
            if let Some(workspace) = &mut self.rebis {
                workspace.message = "only a running run can be paused".to_string();
            }
            return false;
        }
        let id = selected.id;
        let resume = selected.paused;
        let request = selected.request.clone();
        let parallel = selected.parallel;
        if resume && self.job_for_run(id).is_none() {
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                resume_rebis_run_clock(run);
                run.output.push(
                    "resumed     ▶ rebuilding completed prompts · retrying first unfinished prompt"
                        .to_string(),
                );
            }
            self.start_rebis_run(id, request, parallel);
            if let Some(workspace) = &mut self.rebis {
                workspace.message =
                    "run resumed from its last completed prompt · failed prompt will retry"
                        .to_string();
            }
            return true;
        }
        let Some(job) = self.job_for_run(id) else {
            if let Some(workspace) = &mut self.rebis {
                workspace.message = "this run has no live subprocess to pause".to_string();
            }
            return false;
        };
        let pid = job
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .id();
        let owns_process_group = job.owns_process_group;
        // No signal crate: `kill` sends SIGSTOP/SIGCONT to the child or its
        // hosted process group.
        let signal = if resume { "-CONT" } else { "-STOP" };
        let sent = signal_process(pid, owns_process_group, signal);
        if !sent {
            if let Some(workspace) = &mut self.rebis {
                workspace.message = "could not signal the run's subprocess".to_string();
            }
            return false;
        }
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            if resume {
                resume_rebis_run_clock(run);
            } else {
                pause_rebis_run_clock(run, "paused manually");
            }
            run.output.push(if resume {
                "resumed     ▶ run continues at the current prompt".to_string()
            } else {
                "paused      ⏸ run suspended · p resumes".to_string()
            });
        }
        if let Some(workspace) = &mut self.rebis {
            workspace.message = if resume {
                "run resumed".to_string()
            } else {
                "run paused · p resumes".to_string()
            };
        }
        if !resume {
            let _ = self.persist_rebis_run(id);
        }
        true
    }

    /// Remove the selected waiting or finished Rebis run without disturbing
    /// chat work, the active subprocess, or any other queued request.
    fn remove_selected_rebis_run(&mut self) -> bool {
        self.clamp_rebis_run_choice();
        let Some(selected) = self.rebis_runs.get(self.rebis_run_choice) else {
            return false;
        };
        if selected.state == RebisRunState::Running {
            if let Some(workspace) = &mut self.rebis {
                workspace.message =
                    "the active run cannot be removed · cancel it first".to_string();
            }
            return false;
        }
        let id = selected.id;
        let scope = selected.scope;
        let checkpoint_path = selected.checkpoint_path.clone();
        let directive_path = selected.directive_path.clone();
        let durable_checkpoint = selected.saved_checkpoint_path.as_ref() == Some(&checkpoint_path);
        let was_queued = selected.state == RebisRunState::Queued;
        let was_awaiting_permission = selected.state == RebisRunState::AwaitingPermission;
        if was_awaiting_permission {
            if self.pending_rebis.as_ref().map(|pending| pending.id) != Some(id) {
                return false;
            }
            self.pending_rebis = None;
        } else if was_queued {
            if let Some(queue_index) = self.queue.iter().position(
                |work| matches!(work, QueuedWork::Rebis { id: queued, .. } if *queued == id),
            ) {
                let _ = self.queue.remove(queue_index);
            } else if let Some(queue_index) = self
                .parallel_gate_queue
                .iter()
                .position(|pending| pending.id == id)
            {
                self.parallel_gate_queue.remove(queue_index);
            } else {
                return false;
            }
        }
        self.rebis_runs.remove(self.rebis_run_choice);
        if !durable_checkpoint {
            let _ = std::fs::remove_file(checkpoint_path);
        }
        let _ = std::fs::remove_file(directive_path);
        self.clamp_rebis_run_choice();
        let label = format!("Rebis {}", scope.label());
        if was_awaiting_permission || was_queued {
            let remaining = self
                .rebis_runs
                .iter()
                .filter(|run| {
                    matches!(
                        run.state,
                        RebisRunState::AwaitingPermission | RebisRunState::Queued
                    )
                })
                .count();
            self.push_line(Line::from(vec![
                Span::styled("↶ unqueued ", Style::new().fg(C_GOLD())),
                Span::styled(label.clone(), Style::new().fg(C_BONE())),
            ]));
            if let Some(workspace) = &mut self.rebis {
                workspace.message = if remaining == 0 {
                    format!("{label} removed from the queue")
                } else {
                    format!("{label} removed · {remaining} queued Rebis run(s) remain")
                };
            }
        } else if let Some(workspace) = &mut self.rebis {
            workspace.message = format!("{label} removed from run history");
        }
        if was_awaiting_permission {
            self.advance_rebis_gate_queue();
            self.drain_queue();
        }
        true
    }

    /// Append a plain line to the transcript — into the open fold's body if one is
    /// receiving stream output, else at top level.
    fn push_line(&mut self, line: Line<'static>) {
        match self.open_fold {
            Some(i) => {
                if let Some(Entry::Fold(f)) = self.transcript.get_mut(i) {
                    f.body.push(line);
                    return;
                }
                // The open fold vanished (drained) — fall through to top level.
                self.open_fold = None;
                self.transcript.push(Entry::Line(line));
            }
            None => self.transcript.push(Entry::Line(line)),
        }
    }

    /// Route a wheel step through the pane geometry captured by the last draw.
    /// Source scrolling detaches from the stationary edit cursor; keyboard
    /// input reattaches it. Every vertical target clamps at its real end.
    fn on_mouse_scroll(
        &mut self,
        delta: isize,
        column: u16,
        row: u16,
        modifiers: KeyModifiers,
        size: (u16, u16),
    ) {
        self.text_selection = None;
        if self.rebis.is_none() {
            if delta < 0 {
                self.scroll_up(3);
            } else {
                self.scroll_down(3);
            }
            return;
        }
        if self
            .rebis
            .as_mut()
            .is_some_and(RebisWorkspace::dismiss_chaos_star)
        {
            return;
        }
        let position = Position { x: column, y: row };
        let pane = self
            .text_panes
            .iter()
            .find(|pane| point_in_rect(position, pane.area))
            .map(|pane| (pane.kind, pane.content_area()))
            .unwrap_or_else(|| {
                let workspace = self.rebis.as_ref().expect("checked above");
                let kind = if mouse_over_rebis_graph(workspace, column, row, size) {
                    TextPaneKind::RebisPanel
                } else {
                    TextPaneKind::RebisSource
                };
                (kind, Rect::new(0, 0, size.0.max(1), size.1.max(1)))
            });
        let vertical_amount = delta.saturating_mul(3);
        let horizontal_amount = delta.saturating_mul(4);
        match pane.0 {
            TextPaneKind::Chat => {
                if delta < 0 {
                    self.scroll_up(3);
                } else {
                    self.scroll_down(3);
                }
            }
            TextPaneKind::RebisPanel => {
                let runs_visible = self
                    .rebis
                    .as_ref()
                    .is_some_and(|workspace| workspace.runs_visible);
                if modifiers.contains(KeyModifiers::SHIFT) {
                    if let Some(workspace) = &mut self.rebis {
                        workspace.graph_left = workspace
                            .graph_left
                            .saturating_add_signed(horizontal_amount);
                    }
                } else if runs_visible {
                    if let Some(workspace) = &mut self.rebis {
                        workspace.graph_focus = true;
                    }
                    self.scroll_rebis_runs(vertical_amount);
                } else if let Some(workspace) = &mut self.rebis {
                    workspace.scroll_graph_vertical(vertical_amount, pane.1.height as usize);
                }
            }
            TextPaneKind::RebisSource => {
                if let Some(workspace) = &mut self.rebis {
                    if modifiers.contains(KeyModifiers::SHIFT) {
                        workspace
                            .scroll_source_horizontal(horizontal_amount, pane.1.width as usize);
                    } else {
                        workspace.scroll_source_vertical(vertical_amount, pane.1.height as usize);
                    }
                }
            }
        }
    }

    fn run_loop(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.quit {
            terminal.draw(|f| self.draw(f))?;
            self.pump();
            if event::poll(Duration::from_millis(50))? {
                match event::read()? {
                    Event::Key(k) if k.kind == KeyEventKind::Press => {
                        self.on_key(k.code, k.modifiers);
                    }
                    Event::Paste(text) => self.on_paste(&text),
                    Event::Mouse(m) => match m.kind {
                        MouseEventKind::ScrollUp => self.on_mouse_scroll(
                            -1,
                            m.column,
                            m.row,
                            m.modifiers,
                            crossterm::terminal::size().unwrap_or((80, 24)),
                        ),
                        MouseEventKind::ScrollDown => self.on_mouse_scroll(
                            1,
                            m.column,
                            m.row,
                            m.modifiers,
                            crossterm::terminal::size().unwrap_or((80, 24)),
                        ),
                        MouseEventKind::Down(MouseButton::Left) => {
                            // The entry star remains a one-event veil. Once it
                            // is gone, defer pane clicks until mouse-up so a drag
                            // can become a clipped text selection instead.
                            let star_visible = self
                                .rebis
                                .as_ref()
                                .is_some_and(RebisWorkspace::chaos_star_visible);
                            if star_visible
                                || (!self.begin_pane_selection(m.column, m.row)
                                    && self.rebis.is_some())
                            {
                                self.on_rebis_click(
                                    m.column,
                                    m.row,
                                    crossterm::terminal::size().unwrap_or((80, 24)),
                                );
                            }
                        }
                        MouseEventKind::Drag(MouseButton::Left) => {
                            self.drag_pane_selection(m.column, m.row);
                        }
                        MouseEventKind::Up(MouseButton::Left) => {
                            if let Some((dragged, pane, text)) =
                                self.finish_pane_selection(m.column, m.row)
                            {
                                if dragged {
                                    let copied =
                                        text.is_empty() || self.copy_to_clipboard(&text).is_ok();
                                    self.announce_pane_copy(pane, &text, copied);
                                } else if self.rebis.is_some() {
                                    self.on_rebis_click(
                                        m.column,
                                        m.row,
                                        crossterm::terminal::size().unwrap_or((80, 24)),
                                    );
                                }
                            }
                        }
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
        Ok(())
    }

    // ── rendering ──────────────────────────────────────────────────

    fn draw(&mut self, f: &mut Frame) {
        // Paint the ground first. Without this the app inherits whatever the
        // terminal is set to, so light mode would put dark ink on a dark
        // background — the one thing a light theme must not do.
        f.render_widget(
            Block::default().style(Style::new().bg(c_ground()).fg(c_ink())),
            f.area(),
        );
        let queue_len = self.queue.len();
        self.clamp_rebis_run_choice();
        let rebis_runs = self.rebis_run_views();
        let selected_model = self.model.clone();
        let config_editor = self.config_editor;
        if let Some(workspace) = &mut self.rebis {
            let pane_areas = draw_rebis_workspace(
                workspace,
                queue_len,
                &rebis_runs,
                self.rebis_run_choice,
                self.rebis_run_top,
                RebisWorkspaceChrome {
                    selected_model: &selected_model,
                    config_document: config_editor,
                },
                f,
            );
            self.capture_text_panes(f, &pane_areas);
            return;
        }
        // The input field grows as you type or paste. Embedded newlines become
        // real rows and long lines hard-wrap; when the prompt is taller than the
        // screen, a viewport follows the cursor instead of clipping its tail.
        const PROMPT: &str = "✴ ❯ ";
        let full_w = f.area().width.max(1) as usize;
        let mut input_cells: Vec<(char, Style)> = PROMPT.chars().map(|c| (c, red_bold())).collect();
        let bone = Style::new().fg(C_BONE());
        input_cells.extend(self.input.chars().map(|c| (c, bone)));
        let input_rows = hard_wrap(&input_cells, full_w);
        let cursor_cell = PROMPT.chars().count() + self.cursor;
        let (cursor_row, cursor_column) = wrapped_cursor(&input_cells, cursor_cell, full_w);
        // Keep at least one transcript row plus the title and footer. Very long
        // prompts use the remaining surface and scroll within it.
        let max_input_h = f.area().height.saturating_sub(3).max(1) as usize;
        let input_h = input_rows.len().min(max_input_h).max(1) as u16;
        let input_top = cursor_row
            .saturating_add(1)
            .saturating_sub(input_h as usize)
            .min(input_rows.len().saturating_sub(input_h as usize));

        let rows = Layout::vertical([
            Constraint::Length(1),       // title
            Constraint::Min(1),          // transcript
            Constraint::Length(input_h), // input (grows with the text)
            Constraint::Length(1),       // status
        ])
        .split(f.area());

        // Title bar.
        let mind = crate::provider::Spec::parse(&self.model).label();
        let title = Line::from(vec![
            Span::styled("✴ kaos", red_bold()),
            Span::raw("   "),
            Span::styled(format!("mind:{mind}"), Style::new().fg(C_ASH())),
        ]);
        f.render_widget(Paragraph::new(title), rows[0]);

        // Transcript. Pre-wrap each line to the body width into display rows (styles
        // preserved), so long output flows onto new lines AND the scroll offset stays
        // exact (1 row per rendered line).
        let body_h = rows[1].height;
        self.view_h = body_h;
        let width = rows[1].width as usize;
        let rendered = self.rendered_lines();
        let wrapped: Vec<Line> = rendered.iter().flat_map(|l| wrap_line(l, width)).collect();
        self.content_rows = wrapped.len() as u16;
        let max_scroll = self.content_rows.saturating_sub(body_h);
        if self.follow {
            self.scroll = max_scroll;
        } else {
            self.scroll = self.scroll.min(max_scroll);
        }
        let body = Paragraph::new(Text::from(wrapped)).scroll((self.scroll, 0));
        f.render_widget(body, rows[1]);

        f.render_widget(
            Paragraph::new(Text::from(input_rows)).scroll((input_top as u16, 0)),
            rows[2],
        );
        let suggestions = if self.input.starts_with('/') {
            main_completions(&self.input)
        } else {
            Vec::new()
        };
        if !suggestions.is_empty() {
            let height = (suggestions.len().min(7) + 2) as u16;
            let width = rows[1].width.min(34);
            let area = Rect::new(
                rows[1].x,
                rows[1].y + rows[1].height.saturating_sub(height),
                width,
                height.min(rows[1].height),
            );
            f.render_widget(Clear, area);
            let selected = self.command_choice.min(suggestions.len() - 1);
            let visible = suggestions.len().min(7);
            let start = selected.saturating_sub(visible.saturating_sub(1));
            let lines = suggestions
                .iter()
                .skip(start)
                .take(visible)
                .enumerate()
                .map(|(index, command)| {
                    let index = start + index;
                    Line::from(Span::styled(
                        format!("/{}", command.display),
                        if index == selected {
                            Style::new()
                                .fg(Color::Black)
                                .bg(C_TEAL())
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::new().fg(C_BONE())
                        },
                    ))
                })
                .collect::<Vec<_>>();
            f.render_widget(
                Paragraph::new(lines)
                    .block(Block::default().borders(Borders::ALL).title(" COMMANDS ")),
                area,
            );
        }
        f.set_cursor_position(Position {
            x: rows[2].x + cursor_column as u16,
            y: rows[2].y + cursor_row.saturating_sub(input_top) as u16,
        });

        // Status bar: the yolo question takes precedence, then the live spinner.
        let status = if self.pending.is_some() {
            Line::from(vec![
                Span::styled("grant full authority?  ", red_bold()),
                Span::styled(
                    "[y] unbound   [n] edits only   [Esc] cancel",
                    Style::new().fg(C_ASH()),
                ),
            ])
        } else {
            self.status_line(&rows, max_scroll)
        };
        render_footer_with_model(f, rows[3], status, &selected_model);
        self.capture_text_panes(f, &[TextPaneRegion::full(TextPaneKind::Chat, rows[1])]);
    }

    /// The normal (non-pending) status line: spinner while working, else hints.
    fn status_line(&self, rows: &[ratatui::layout::Rect], max_scroll: u16) -> Line<'static> {
        match (&self.job, self.job_start) {
            (Some(job), Some(start)) => {
                if let Some(run) = job.rebis_run_id.and_then(|id| {
                    self.rebis_runs
                        .iter()
                        .find(|run| run.id == id && run.paused)
                }) {
                    return Line::from(vec![
                        Span::styled(
                            "Ⅱ paused  ",
                            Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD),
                        ),
                        Span::styled(job.label.clone(), Style::new().fg(C_BONE())),
                        Span::styled(
                            format!(
                                "  {} · p resumes",
                                run.pause_reason.as_deref().unwrap_or("waiting")
                            ),
                            Style::new().fg(C_ASH()),
                        ),
                    ]);
                }
                let elapsed = job
                    .rebis_run_id
                    .and_then(|id| self.rebis_runs.iter().find(|run| run.id == id))
                    .map_or_else(|| start.elapsed(), active_rebis_run_elapsed);
                let ms = elapsed.as_millis();
                let frame = SPIN[(ms / 90) as usize % SPIN.len()];
                let secs = elapsed.as_secs();
                // Budget the activity text to the remaining width so it never wraps.
                let queued = if self.queue.is_empty() {
                    String::new()
                } else {
                    format!("  ⧗{}", self.queue.len())
                };
                let parallel = if self.parallel_jobs.is_empty() {
                    String::new()
                } else {
                    format!("  ∥{}", self.parallel_jobs.len())
                };
                let head = format!("{frame} {}  {secs}s{parallel}{queued}  ", job.label);
                let room = (rows[3].width as usize).saturating_sub(head.chars().count() + 3);
                let act = truncate(&self.activity, room.max(4));
                Line::from(vec![
                    Span::styled(format!("{frame} "), red_bold()),
                    Span::styled(job.label.clone(), Style::new().fg(C_BONE())),
                    Span::styled(format!("  {secs}s"), Style::new().fg(C_OX())),
                    Span::styled(parallel, Style::new().fg(C_TEAL())),
                    Span::styled(queued, Style::new().fg(C_ASH())),
                    Span::styled(
                        "  ".to_string() + &act,
                        Style::new().fg(C_ASH()).add_modifier(Modifier::ITALIC),
                    ),
                ])
            }
            _ if !self.parallel_jobs.is_empty() => Line::from(vec![
                Span::styled("∥ ", Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD)),
                Span::styled(
                    format!(
                        "{} parallel Rebis run{} active",
                        self.parallel_jobs.len(),
                        if self.parallel_jobs.len() == 1 {
                            ""
                        } else {
                            "s"
                        }
                    ),
                    Style::new().fg(C_BONE()),
                ),
                Span::styled(
                    format!("  ⧗{} serial", self.queue.len()),
                    Style::new().fg(C_ASH()),
                ),
            ]),
            _ if !self.follow => {
                // Scrolled up into the backlog — make it obvious and how to return.
                let above = self.scroll;
                let below = max_scroll.saturating_sub(self.scroll);
                Line::from(vec![
                    Span::styled("\u{25b2} scrollback ", red_bold()),
                    Span::styled(
                        format!("{above} above · {below} below — PgDn / End to return to live"),
                        Style::new().fg(C_ASH()),
                    ),
                ])
            }
            _ => {
                let folds = self.fold_indices().len();
                let hint = if folds > 0 {
                    format!(
                        "{}   Tab select · Enter fold · ^E all ({folds}) · ↑↓ history · ^L clear · ^C cancel/quit",
                        short_path(&self.cwd)
                    )
                } else {
                    format!(
                        "{}   ↑↓ history · wheel/PgUp scroll · ^L clear · ^C cancel · Esc quit",
                        short_path(&self.cwd)
                    )
                };
                Line::from(Span::styled(hint, Style::new().fg(C_ASH())))
            }
        }
    }

    // ── streaming ──────────────────────────────────────────────────

    fn pump(&mut self) {
        self.handle_workspace_events();
        let mut live_run_changed = false;
        let sigil_chat_poll = self.sigil_chat_job.as_ref().map(|turn| poll_job(&turn.job));
        if let Some(poll) = sigil_chat_poll {
            for line in poll.lines {
                let rendered = match crate::fold::classify(&line) {
                    crate::fold::Marker::Open(summary) => {
                        Some(format!("god     ▶ {}", strip_ansi(summary)))
                    }
                    crate::fold::Marker::Close => None,
                    crate::fold::Marker::Line(content) => {
                        let content = strip_ansi(content);
                        (!content.trim().is_empty()).then(|| format!("god     {content}"))
                    }
                };
                if let (Some(workspace), Some(rendered)) =
                    (self.background_rebis_workspace(), rendered)
                {
                    workspace.push_sigil_chat_line(rendered);
                }
            }
            if let Some(code) = poll.done {
                let turn = self
                    .sigil_chat_job
                    .take()
                    .expect("polled sigil-chat job must exist");
                self.finish_sigil_chat_turn(turn, code);
            }
        }

        let primary_poll = self.job.as_ref().map(poll_job);
        if let Some(poll) = primary_poll {
            let run_id = self.job.as_ref().and_then(|job| job.rebis_run_id);
            live_run_changed |= run_id.is_some() && (!poll.lines.is_empty() || poll.done.is_some());
            for line in poll.lines {
                if self.handle_rebis_pause_marker(run_id, &line) {
                    continue;
                }
                self.retain_rebis_stream(run_id, &line, false);
                self.record_reply_line(&line);
                self.push_stream_line(&line);
            }
            if let Some(code) = poll.done {
                let job = self.job.take().expect("polled primary job must exist");
                // The turn is over: fold the streamed reply into the session
                // and persist, so a crash loses nothing already answered.
                self.save_session();
                self.finish_rebis_subprocess(job.rebis_run_id, code, false);
                if code == 0 && job.claude_session {
                    self.resumed = true;
                }
                self.job_start = None;
                self.activity.clear();
                self.open_fold = None;
                self.fold_depth = 0;
                let resumable = code != 0 && job.rebis_run_id.is_some();
                let note = if code == 0 {
                    Span::styled("  ✴ done", Style::new().fg(C_DONE()))
                } else if resumable {
                    Span::styled(
                        "  Ⅱ paused · p retries the unfinished prompt",
                        Style::new().fg(C_GOLD()),
                    )
                } else {
                    Span::styled(format!("  ✴ exited ({code})"), Style::new().fg(C_RED()))
                };
                self.push_line(Line::from(note));
                self.push_line(Line::raw(""));
            }
        }

        let parallel_polls = self
            .parallel_jobs
            .iter()
            .enumerate()
            .map(|(index, job)| (index, job.rebis_run_id, poll_job(job)))
            .collect::<Vec<_>>();
        let mut completed = Vec::new();
        for (index, run_id, poll) in parallel_polls {
            live_run_changed |= run_id.is_some() && (!poll.lines.is_empty() || poll.done.is_some());
            for line in poll.lines {
                if self.handle_rebis_pause_marker(run_id, &line) {
                    continue;
                }
                self.retain_rebis_stream(run_id, &line, true);
                if let Some(run_id) = run_id {
                    self.push_parallel_stream_line(run_id, &line);
                }
            }
            if let Some(code) = poll.done {
                completed.push((index, code));
            }
        }
        for (index, code) in completed.into_iter().rev() {
            let job = self.parallel_jobs.remove(index);
            self.finish_rebis_subprocess(job.rebis_run_id, code, true);
            let id = job.rebis_run_id.unwrap_or_default();
            let note = if code == 0 {
                Span::styled(format!("  ∥ run #{id} done"), Style::new().fg(C_DONE()))
            } else {
                Span::styled(
                    format!("  ∥ run #{id} paused ({code}) · p retries"),
                    Style::new().fg(C_GOLD()),
                )
            };
            self.push_line(Line::from(note));
        }

        self.drain_queue();
        // Peer bots keep running while the bound bot is inspected. Refresh the
        // shared snapshot atomically so the God Agent can reread current traces.
        if live_run_changed {
            self.refresh_sigil_chat_run_context();
        }
    }

    fn finish_sigil_chat_turn(&mut self, turn: SigilChatJob, code: i32) {
        let source_path = turn.bridge_dir.join("sigil.rebis");
        let proposed = std::fs::read_to_string(&source_path);
        let mut applied_source = None;
        if code != 0 {
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.push_sigil_chat_line(format!(
                    "system  god agent stopped ({code}); live source was not changed"
                ));
            }
        } else {
            match proposed {
                Err(error) => {
                    if let Some(workspace) = self.background_rebis_workspace() {
                        workspace.push_sigil_chat_line(format!(
                            "system  could not read the proposed source: {error}"
                        ));
                    }
                }
                Ok(proposed) if proposed == turn.base_source => {
                    if let Some(workspace) = self.background_rebis_workspace() {
                        workspace.push_sigil_chat_line("system  source unchanged");
                    }
                }
                Ok(proposed) => {
                    let current_matches = self
                        .background_rebis_workspace()
                        .is_some_and(|workspace| workspace.editor.source() == turn.base_source);
                    if !current_matches {
                        if let Some(workspace) = self.background_rebis_workspace() {
                            workspace.push_sigil_chat_line(format!(
                                "system  source conflict: the editor changed during this turn; proposal retained at {}",
                                source_path.display()
                            ));
                        }
                    } else if let Err(error) = rebis_lang::parse(&proposed) {
                        if let Some(workspace) = self.background_rebis_workspace() {
                            workspace.push_sigil_chat_line(format!(
                                "system  rejected invalid Rebis ({error}); proposal retained at {}",
                                source_path.display()
                            ));
                        }
                    } else {
                        if let Some(workspace) = self.background_rebis_workspace() {
                            workspace.editor.replace(proposed.clone());
                            workspace.refresh();
                            workspace.push_sigil_chat_line(
                                "system  ✓ valid source revision applied to the live editor",
                            );
                        }
                        applied_source = Some(proposed);
                    }
                }
            }
        }

        if let (Some(id), Some(source)) = (turn.run_id, applied_source.as_deref()) {
            self.rewrite_rebis_run_source(id, source);
            self.retire_rebis_child_for_rewrite(id);
            let _ = self.persist_rebis_run(id);
        }

        if turn.resume_after {
            if let Some(id) = turn.run_id {
                if applied_source.is_some() {
                    if let Some(run) = self.rebis_runs.iter().find(|run| run.id == id) {
                        let request = run.request.clone();
                        let parallel = run.parallel;
                        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                            run.output.push(
                                "resumed     ▶ source revised · rebuilding from retained prompts"
                                    .to_string(),
                            );
                        }
                        self.start_rebis_run(id, request, parallel);
                    }
                } else {
                    self.resume_stopped_run(id);
                }
            }
        }
        if code == 0 {
            self.apply_sigil_chat_run_controls(&turn.bridge_dir);
        }
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.set_sigil_chat_busy(false);
        }
    }

    /// Replace every not-yet-launched copy of a run request as well as the
    /// durable browser entry. Completed prompt answers remain in the journal;
    /// exact prompts replay, and the first changed prompt truncates only its
    /// divergent tail.
    fn rewrite_rebis_run_source(&mut self, id: u64, source: &str) {
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            run.request.source = source.to_string();
            run.preview = rebis_source_preview(source);
            pause_rebis_run_clock(run, "source revised by sigil chat");
            run.output.push(
                "source      god-agent revision installed · prompt journal retained".to_string(),
            );
        }
        for work in &mut self.queue {
            if let QueuedWork::Rebis {
                id: queued,
                request,
            } = work
            {
                if *queued == id {
                    request.source = source.to_string();
                }
            }
        }
        if let Some(pending) = &mut self.pending_rebis {
            if pending.id == id {
                pending.request.source = source.to_string();
            }
        }
        for pending in &mut self.parallel_gate_queue {
            if pending.id == id {
                pending.request.source = source.to_string();
            }
        }
    }

    /// A stopped interpreter cannot adopt new syntax in-place. Retire only its
    /// process, deliberately leaving the run state and atomic prompt journal
    /// alive for immediate reconstruction.
    fn retire_rebis_child_for_rewrite(&mut self, id: u64) {
        if self
            .job
            .as_ref()
            .is_some_and(|job| job.rebis_run_id == Some(id))
        {
            let job = self.job.take().expect("checked primary run child");
            let mut child = job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.owns_process_group {
                let _ = signal_process(child.id(), true, "-KILL");
            } else {
                let _ = child.kill();
            }
            self.job_start = None;
        }
        if let Some(index) = self
            .parallel_jobs
            .iter()
            .position(|job| job.rebis_run_id == Some(id))
        {
            let job = self.parallel_jobs.remove(index);
            let mut child = job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.owns_process_group {
                let _ = signal_process(child.id(), true, "-KILL");
            } else {
                let _ = child.kill();
            }
        }
    }

    /// Consume the private child-to-parent pause protocol. The control marker
    /// never leaks into user output; the run tree receives one readable event.
    fn handle_rebis_pause_marker(&mut self, run_id: Option<u64>, line: &str) -> bool {
        let Some(reason) = crate::pause::marker_reason(line) else {
            return false;
        };
        let Some(run_id) = run_id else {
            return true;
        };
        let reason = reason.to_string();
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == run_id) {
            pause_rebis_run_clock(run, &reason);
            run.output
                .push(format!("paused      ⏸ {reason} · p resumes"));
        }
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.message = format!("Rebis run paused · {reason} · p resumes");
        }
        let _ = self.persist_rebis_run(run_id);
        true
    }

    fn retain_rebis_stream(&mut self, run_id: Option<u64>, line: &str, parallel: bool) {
        let Some(run_id) = run_id else {
            return;
        };
        let plain = strip_ansi(line);
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == run_id) {
            run.output.push(plain.clone());
        }
        if !parallel {
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.push_run_output(&plain);
            }
        }
    }

    fn finish_rebis_subprocess(&mut self, run_id: Option<u64>, code: i32, parallel: bool) {
        let Some(run_id) = run_id else {
            return;
        };
        let saved_paths = self
            .rebis_runs
            .iter()
            .find(|run| run.id == run_id)
            .and_then(|run| {
                Some((
                    run.saved_run_path.clone()?,
                    run.saved_checkpoint_path.clone()?,
                ))
            });
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == run_id) {
            if code == 0 {
                finish_rebis_run_clock(run);
                run.state = RebisRunState::Complete;
                let _ = std::fs::remove_file(&run.checkpoint_path);
                let _ = std::fs::remove_file(&run.directive_path);
            } else {
                let reason = format!("child stopped unexpectedly ({code})");
                pause_rebis_run_clock(run, &reason);
                run.output.push(format!(
                    "paused      ⏸ {reason} · p retries the first unfinished prompt"
                ));
            }
        }
        if code == 0 {
            if let Some((run_path, checkpoint_path)) = saved_paths {
                let _ = std::fs::remove_file(run_path);
                let _ = std::fs::remove_file(checkpoint_path);
            }
        } else {
            let _ = self.persist_rebis_run(run_id);
        }
        if !parallel {
            if let Some(workspace) = self.background_rebis_workspace() {
                if code == 0 {
                    workspace.finish_run(0);
                } else {
                    workspace.pause_run(&format!(
                        "child stopped unexpectedly ({code}) · p retries the unfinished prompt"
                    ));
                }
            }
        }
    }

    /// Parallel subprocesses keep fold markers isolated in their own retained
    /// run trees. The shared chat transcript receives tagged plain activity so
    /// one run can never close or capture another run's fold.
    fn push_parallel_stream_line(&mut self, run_id: u64, line: &str) {
        let text = match crate::fold::classify(line) {
            crate::fold::Marker::Open(summary) => format!("▶ {}", strip_ansi(summary)),
            crate::fold::Marker::Close => return,
            crate::fold::Marker::Line(content) => strip_ansi(content),
        };
        if text.trim().is_empty() {
            return;
        }
        self.push_line(Line::from(vec![
            Span::styled(
                format!("∥ #{run_id}  "),
                Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(text, Style::new().fg(C_ASH())),
        ]));
    }

    /// Handle one streamed line from a running command, interpreting the fold
    /// protocol ([`crate::fold`]): FOLD_OPEN starts a collapsible group, FOLD_CLOSE
    /// ends it, everything else is content routed into the open fold (if any).
    fn push_stream_line(&mut self, s: &str) {
        match crate::fold::classify(s) {
            crate::fold::Marker::Open(summary) => {
                self.fold_depth += 1;
                if self.fold_depth > 1 {
                    // A nested fold: render its summary as a body header rather than a
                    // second collapsible level (we render one level deep).
                    self.push_ansi_as_lines(summary);
                } else {
                    let sum_line = ansi_first_line(summary);
                    self.transcript.push(Entry::Fold(Fold {
                        summary: sum_line,
                        body: Vec::new(),
                        collapsed: true,
                    }));
                    self.open_fold = Some(self.transcript.len() - 1);
                }
            }
            crate::fold::Marker::Close => {
                if self.fold_depth > 0 {
                    self.fold_depth -= 1;
                }
                if self.fold_depth == 0 {
                    self.open_fold = None;
                }
            }
            crate::fold::Marker::Line(content) => {
                // The latest non-empty content line is "what the agent is doing".
                let plain = strip_ansi(content).trim().to_string();
                if !plain.is_empty() {
                    self.activity = plain;
                }
                self.push_ansi_as_lines(content);
            }
        }
        // Keep the transcript bounded, but generously — trimming deletes old
        // commands, so only do it far beyond a normal session, and only when NOT
        // scrolled up (so what you're reading never shifts under you).
        if self.follow && self.transcript.len() > 50_000 {
            self.transcript.drain(0..10_000);
            // Indices shifted by 10_000 — rebase or drop the open-fold pointer.
            self.open_fold = self.open_fold.and_then(|i| i.checked_sub(10_000));
            self.sel_fold = None;
        }
        // NB: do NOT force follow here. If the user has scrolled up to read old
        // commands, streaming output must not yank them back to the bottom.
    }

    /// Parse ANSI content into styled lines and append them (via [`Self::push_line`],
    /// so they land in the open fold's body when one is active).
    fn push_ansi_as_lines(&mut self, s: &str) {
        match s.into_text() {
            Ok(text) => {
                for line in text.lines {
                    self.push_line(line);
                }
            }
            Err(_) => self.push_line(Line::raw(s.to_string())),
        }
    }

    // ── folds ──────────────────────────────────────────────────────

    /// Transcript indices of every fold, in order (their ordinal is the selection id).
    fn fold_indices(&self) -> Vec<usize> {
        self.transcript
            .iter()
            .enumerate()
            .filter(|(_, e)| matches!(e, Entry::Fold(_)))
            .map(|(i, _)| i)
            .collect()
    }

    /// Move the fold selection by `delta` (+1 next, −1 previous), wrapping. With no
    /// selection yet, the newest fold is chosen — the one a reader most likely wants.
    fn move_fold_selection(&mut self, delta: isize) {
        let n = self.fold_indices().len();
        if n == 0 {
            return;
        }
        let next = match self.sel_fold {
            None => n - 1,
            Some(cur) => {
                let m = n as isize;
                (((cur as isize + delta) % m + m) % m) as usize
            }
        };
        self.sel_fold = Some(next);
        self.follow = false; // reviewing folds means the reader has left the live tail
    }

    /// Toggle the selected fold (collapsed ⇄ expanded). If nothing is selected, act on
    /// the newest fold.
    fn toggle_selected_fold(&mut self) {
        let folds = self.fold_indices();
        if folds.is_empty() {
            return;
        }
        let ord = self
            .sel_fold
            .unwrap_or(folds.len() - 1)
            .min(folds.len() - 1);
        self.sel_fold = Some(ord);
        if let Some(Entry::Fold(f)) = self.transcript.get_mut(folds[ord]) {
            f.collapsed = !f.collapsed;
        }
    }

    /// Expand every fold if any is collapsed, else collapse them all — an outline
    /// toggle for the whole transcript.
    fn toggle_all_folds(&mut self) {
        let any_collapsed = self
            .transcript
            .iter()
            .any(|e| matches!(e, Entry::Fold(f) if f.collapsed));
        for e in &mut self.transcript {
            if let Entry::Fold(f) = e {
                f.collapsed = !any_collapsed;
            }
        }
    }

    /// Flatten the entry list into the styled lines to render, expanding folds that
    /// are open and drawing a caret + line-count header for every fold. The selected
    /// fold's header is marked so the reader can see what a toggle will act on.
    fn rendered_lines(&self) -> Vec<Line<'static>> {
        let sel_idx = self
            .sel_fold
            .and_then(|ord| self.fold_indices().get(ord).copied());
        let mut out = Vec::new();
        for (i, entry) in self.transcript.iter().enumerate() {
            match entry {
                Entry::Line(l) => out.push(l.clone()),
                Entry::Fold(f) => {
                    let selected = Some(i) == sel_idx;
                    let caret = if f.collapsed { "▸" } else { "▾" };
                    let head_style = if selected {
                        red_bold()
                    } else {
                        Style::new().fg(C_OX()).add_modifier(Modifier::BOLD)
                    };
                    let mut spans = vec![Span::styled(format!("{caret} "), head_style)];
                    spans.extend(f.summary.spans.iter().cloned());
                    if f.collapsed {
                        spans.push(Span::styled(
                            format!("   ({} lines — Enter to expand)", f.body.len()),
                            Style::new().fg(C_ASH()),
                        ));
                    }
                    out.push(Line::from(spans));
                    if !f.collapsed {
                        for b in &f.body {
                            // Indent body lines so the group reads as one unit.
                            let mut s = vec![Span::raw("  ")];
                            s.extend(b.spans.iter().cloned());
                            out.push(Line::from(s));
                        }
                    }
                }
            }
        }
        out
    }

    // ── input ──────────────────────────────────────────────────────

    fn on_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if selection_copy_shortcut(code, mods) {
            self.copy_active_pane_selection();
            return;
        }
        if ctrl_c_shortcut(code, mods) {
            // ^C is STOP-first on both screens: it cancels whatever is in flight
            // and, only when idle, asks before quitting (a second ^C confirms).
            if self.rebis.is_some() {
                self.workspace_ctrl_c();
            } else {
                self.chat_ctrl_c();
            }
            return;
        }
        // Any key other than ^C answers "stay" to a pending quit confirmation.
        // The key still performs its normal action below.
        self.disarm_quit();
        if self.pending_rebis.is_some() {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.approve_rebis_authority(false),
                KeyCode::Char('a') | KeyCode::Char('A') => self.approve_rebis_authority(true),
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.deny_rebis_authority()
                }
                _ => {}
            }
            return;
        }
        if self.rebis.is_some() {
            self.on_rebis_key(code, mods);
            return;
        }
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        // A pending yolo question captures the keyboard until answered.
        if self.pending.is_some() {
            match code {
                KeyCode::Char('y') | KeyCode::Char('Y') => self.decide_yolo(true),
                KeyCode::Char('n') | KeyCode::Char('N') => self.decide_yolo(false),
                KeyCode::Esc => {
                    self.pending = None;
                    self.note("cancelled.");
                }
                _ => {}
            }
            return;
        }
        // Chat keeps `/` as its command prefix. Ctrl-K (or the legacy Ctrl-/)
        // is a convenient way to open that same palette without first clearing
        // the input manually.
        if command_palette_shortcut(code, mods) {
            self.input = "/".to_string();
            self.cursor = 1;
            self.command_choice = 0;
            return;
        }
        if self.input.starts_with('/') {
            let suggestions = main_completions(&self.input);
            if !suggestions.is_empty() {
                match code {
                    KeyCode::Up => {
                        self.command_choice = self.command_choice.saturating_sub(1);
                        return;
                    }
                    KeyCode::Down => {
                        self.command_choice = (self.command_choice + 1).min(suggestions.len() - 1);
                        return;
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        let command = suggestions[self.command_choice.min(suggestions.len() - 1)];
                        self.input = format!("/{}", command.insert);
                        self.cursor = self.input.chars().count();
                        return;
                    }
                    KeyCode::Enter => {
                        let command = suggestions[self.command_choice.min(suggestions.len() - 1)];
                        if missing_command_argument(&self.input, command) {
                            self.input = format!("/{}", command.insert);
                            self.cursor = self.input.chars().count();
                            return;
                        }
                        if !self
                            .input
                            .trim_start_matches('/')
                            .starts_with(command.insert)
                        {
                            self.input = format!("/{}", command.insert);
                            self.cursor = self.input.chars().count();
                        }
                        self.submit();
                        return;
                    }
                    _ => {}
                }
            }
        }
        match code {
            KeyCode::Char('d') if ctrl => self.quit = true,
            KeyCode::Char('l') if ctrl => self.clear_transcript(),
            KeyCode::Char('e') if ctrl => self.toggle_all_folds(),
            KeyCode::Char('u') if ctrl => {
                self.input.clear();
                self.cursor = 0;
            }
            // Fold navigation lives on Tab / Shift-Tab; toggling on Enter when the
            // input line is empty (otherwise Enter submits the command).
            KeyCode::Tab => self.move_fold_selection(1),
            KeyCode::BackTab => self.move_fold_selection(-1),
            KeyCode::Esc => self.quit = true,
            KeyCode::Char(c) if !ctrl => {
                let byte_idx = self.byte_at(self.cursor);
                self.input.insert(byte_idx, c);
                self.cursor += 1;
                self.command_choice = 0;
            }
            KeyCode::Backspace => {
                if self.cursor > 0 {
                    let start = self.byte_at(self.cursor - 1);
                    let end = self.byte_at(self.cursor);
                    self.input.replace_range(start..end, "");
                    self.cursor -= 1;
                    self.command_choice = 0;
                }
            }
            KeyCode::Delete => {
                if self.cursor < self.char_len() {
                    let start = self.byte_at(self.cursor);
                    let end = self.byte_at(self.cursor + 1);
                    self.input.replace_range(start..end, "");
                }
            }
            KeyCode::Left => self.cursor = self.cursor.saturating_sub(1),
            KeyCode::Right => self.cursor = (self.cursor + 1).min(self.char_len()),
            KeyCode::Home => self.cursor = 0,
            KeyCode::End => {
                // When scrolled up into the backlog, End snaps back to live output;
                // otherwise it moves the input cursor to the end of the line.
                if !self.follow {
                    self.follow = true;
                } else {
                    self.cursor = self.char_len();
                }
            }
            KeyCode::Up => self.history_prev(),
            KeyCode::Down => self.history_next(),
            KeyCode::PageUp => self.scroll_up(self.view_h.max(1)),
            KeyCode::PageDown => self.scroll_down(self.view_h.max(1)),
            KeyCode::Enter => {
                // Empty input + a fold in view → Enter toggles the selected fold;
                // otherwise it submits the command as usual.
                if self.input.is_empty() && !self.fold_indices().is_empty() {
                    self.toggle_selected_fold();
                } else {
                    self.submit();
                }
            }
            _ => {}
        }
    }

    fn on_paste(&mut self, text: &str) {
        if let Some(workspace) = &mut self.rebis {
            // Like a key press, the first paste only dismisses the entry veil.
            if !text.is_empty() && workspace.dismiss_chaos_star() {
                return;
            }
            workspace.follow_source_cursor();
            match workspace.mode {
                RebisMode::Insert => {
                    if !text.is_empty() {
                        workspace.editor.insert_text(text);
                    }
                }
                RebisMode::Command | RebisMode::KaosCommand => workspace.command.push_str(text),
                _ => workspace.message = "enter insert mode with i before pasting".to_string(),
            }
            workspace.refresh();
            return;
        }
        let byte = self.byte_at(self.cursor);
        self.input.insert_str(byte, text);
        self.cursor += text.chars().count();
    }

    /// Route keys to the full-screen Rebis editor. Parsing and graph lowering
    /// happen in `RebisWorkspace::refresh`, which calls `rebis_lang::parse`.
    /// A left click focuses the Rebis pane under the pointer — the mouse
    /// counterpart of Ctrl-W h/l, and the only focus path without vim.
    fn on_rebis_click(&mut self, column: u16, row: u16, size: (u16, u16)) {
        let rebis_runs = self.rebis_run_views();
        let Some(workspace) = &mut self.rebis else {
            return;
        };
        if workspace.dismiss_chaos_star() {
            return;
        }
        let over_graph = mouse_over_rebis_graph(workspace, column, row, size);
        if over_graph && workspace.runs_visible && !rebis_runs.is_empty() {
            if let Some(panel) = workspace
                .panel_inner
                .map(|(x, y, width, height)| Rect::new(x, y, width, height))
            {
                let run_rows = rebis_run_display_rows(&rebis_runs, panel.width as usize);
                if let Some((area, start, visible)) =
                    rebis_run_browser_layout(panel, run_rows.len(), self.rebis_run_top)
                {
                    let inside = column >= area.x
                        && column < area.right()
                        && row >= area.y
                        && row < area.bottom();
                    if inside {
                        let first_row = area.y + 1;
                        if row >= first_row && (row - first_row) < visible as u16 {
                            let tree_index = start + (row - first_row) as usize;
                            self.rebis_run_choice = match run_rows.get(tree_index) {
                                Some(RebisRunTreeRow::Header(run))
                                | Some(RebisRunTreeRow::Section { run, .. })
                                | Some(RebisRunTreeRow::Output { run, .. }) => *run,
                                None => self.rebis_run_choice,
                            };
                        }
                        workspace.graph_focus = true;
                        let selected = self.rebis_run_choice.min(rebis_runs.len() - 1);
                        workspace.message = format!(
                            "Rebis {} {}/{} · Tab stream · ↑/↓ scroll · ⇧↑ top · ⇧↓ tail · Pg scroll · u/Delete remove",
                            rebis_runs[selected].entry.scope.label(),
                            selected + 1,
                            rebis_runs.len()
                        );
                        return;
                    }
                }
            }
        }
        // A click on a sigil row opens that sigil directly.
        if workspace.click_sigil(column, row) {
            return;
        }
        if over_graph == workspace.graph_focus {
            if !over_graph {
                workspace.follow_source_cursor();
            }
            return;
        }
        workspace.graph_focus = over_graph;
        if !over_graph {
            workspace.follow_source_cursor();
        }
        workspace.message = if over_graph {
            "mandala focus · click the source pane to return".to_string()
        } else {
            "source focus".to_string()
        };
    }

    fn on_rebis_key(&mut self, code: KeyCode, mods: KeyModifiers) {
        if selection_copy_shortcut(code, mods) {
            self.copy_active_pane_selection();
            return;
        }
        if ctrl_c_shortcut(code, mods) {
            self.workspace_ctrl_c();
            return;
        }
        // Any key other than ^C answers "stay" to a pending quit confirmation.
        self.disarm_quit();
        // On first entry the star is a modal, one-interaction veil over the
        // empty editor. Ordinary keys and commands are consumed when it lifts;
        // the global Ctrl-C exit remains immediate.
        if self
            .rebis
            .as_mut()
            .is_some_and(RebisWorkspace::dismiss_chaos_star)
        {
            return;
        }
        let ctrl = mods.contains(KeyModifiers::CONTROL);
        let shift = mods.contains(KeyModifiers::SHIFT);
        if command_palette_shortcut(code, mods) {
            let Some(workspace) = self.rebis.as_mut() else {
                return;
            };
            if workspace.vim_enabled && workspace.mode == RebisMode::Insert {
                workspace.editor.end_insert_session();
            }
            if matches!(workspace.mode, RebisMode::Visual | RebisMode::VisualLine) {
                let linewise = workspace.mode == RebisMode::VisualLine;
                workspace.run_selection = workspace.editor.visual_range(linewise);
                workspace.editor.end_visual();
            }
            workspace.mode = RebisMode::KaosCommand;
            workspace.command.clear();
            workspace.command_choice = 0;
            workspace.editor.clear_pending();
            return;
        }
        let rebis_run_count = self.rebis_runs.len();
        let browsing_runs = rebis_run_count > 0
            && self.rebis.as_ref().is_some_and(|workspace| {
                workspace.runs_visible
                    && workspace.graph_focus
                    && !matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand)
            });
        if browsing_runs {
            match code {
                KeyCode::Char('j') => {
                    self.rebis_run_choice = (self.rebis_run_choice + 1).min(rebis_run_count - 1);
                    self.describe_rebis_run_choice();
                    return;
                }
                KeyCode::Char('k') => {
                    self.rebis_run_choice = self.rebis_run_choice.saturating_sub(1);
                    self.describe_rebis_run_choice();
                    return;
                }
                KeyCode::Down if shift => {
                    self.rebis_run_top = self.rebis_run_max_top();
                    return;
                }
                KeyCode::Down => {
                    self.scroll_rebis_runs(1);
                    return;
                }
                KeyCode::Up if shift => {
                    self.rebis_run_top = 0;
                    return;
                }
                KeyCode::Up => {
                    self.scroll_rebis_runs(-1);
                    return;
                }
                KeyCode::Tab | KeyCode::BackTab => {
                    self.toggle_selected_rebis_run();
                    return;
                }
                KeyCode::PageDown => {
                    self.scroll_rebis_runs(10);
                    return;
                }
                KeyCode::PageUp => {
                    self.scroll_rebis_runs(-10);
                    return;
                }
                KeyCode::Char('g') | KeyCode::Home => {
                    self.rebis_run_top = 0;
                    return;
                }
                KeyCode::End => {
                    self.rebis_run_top = self.rebis_run_max_top();
                    return;
                }
                KeyCode::Char('p') if !ctrl => {
                    self.toggle_pause_selected_rebis_run();
                    return;
                }
                KeyCode::Char('u') if !ctrl => {
                    self.remove_selected_rebis_run();
                    return;
                }
                KeyCode::Delete | KeyCode::Backspace => {
                    self.remove_selected_rebis_run();
                    return;
                }
                _ => {}
            }
        }
        let mut action = WorkspaceAction::None;
        let Some(workspace) = self.rebis.as_mut() else {
            return;
        };

        if workspace.sigil_chat_visible()
            && workspace.graph_focus
            && !matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand)
        {
            let visible_rows = workspace
                .panel_inner
                .map_or(1, |(_, _, _, height)| height.saturating_sub(3) as usize);
            match code {
                KeyCode::Esc => {
                    workspace.graph_focus = false;
                    workspace.message =
                        "source focus · /sigil chat returns to the channel".to_string();
                }
                KeyCode::Enter => {
                    if let Some(message) = workspace.take_sigil_chat_message() {
                        action = WorkspaceAction::SigilChat(message);
                    }
                }
                KeyCode::Char('u') if ctrl => workspace.clear_sigil_chat_input(),
                KeyCode::Char(character) if !ctrl => workspace.insert_sigil_chat_char(character),
                KeyCode::Backspace | KeyCode::Delete => workspace.backspace_sigil_chat(),
                KeyCode::Up => workspace.scroll_graph_vertical(-1, visible_rows),
                KeyCode::Down => workspace.scroll_graph_vertical(1, visible_rows),
                KeyCode::PageUp => workspace.scroll_graph_vertical(-10, visible_rows),
                KeyCode::PageDown => workspace.scroll_graph_vertical(10, visible_rows),
                KeyCode::Home if ctrl || shift => workspace.graph_top = 0,
                KeyCode::End if ctrl || shift => workspace.graph_top = usize::MAX,
                _ => {}
            }
            self.handle_rebis_action(action);
            return;
        }

        if !workspace.graph_focus
            && !matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand)
        {
            workspace.follow_source_cursor();
        }

        if workspace.literal_next {
            workspace.literal_next = false;
            if let KeyCode::Char(character) = code {
                workspace.editor.insert(character);
                workspace.refresh();
                workspace.message = format!("inserted literal {character:?}");
            }
            return;
        }
        if ctrl
            && code == KeyCode::Char('v')
            && workspace.mode == RebisMode::Insert
            && !workspace.graph_focus
        {
            workspace.literal_next = true;
            workspace.message = "literal insert · press the next character".to_string();
            return;
        }

        if !workspace.vim_enabled && workspace.mode == RebisMode::Insert && !workspace.graph_focus {
            match code {
                KeyCode::Char('s') if ctrl => {
                    workspace.command = "w".to_string();
                    action = workspace.execute_command();
                }
                KeyCode::Char(character) if !ctrl => match character {
                    '(' => workspace.editor.insert_pair('(', ')'),
                    '[' => workspace.editor.insert_pair('[', ']'),
                    '"' if workspace.editor.skip_close('"') => {}
                    '"' => workspace.editor.insert_pair('"', '"'),
                    ')' if workspace.editor.skip_close(')') => {}
                    ']' if workspace.editor.skip_close(']') => {}
                    _ => workspace.editor.insert(character),
                },
                KeyCode::Enter => workspace.editor.insert('\n'),
                KeyCode::Tab => workspace.editor.insert_text("  "),
                KeyCode::Backspace => workspace.editor.backspace(),
                KeyCode::Delete => {
                    workspace.editor.delete();
                }
                KeyCode::Left => workspace.editor.left(),
                KeyCode::Right => workspace.editor.right(),
                KeyCode::Up => workspace.editor.vertical(-1),
                KeyCode::Down => workspace.editor.vertical(1),
                KeyCode::Home => workspace.editor.line_start_motion(),
                KeyCode::End => workspace.editor.line_end(),
                _ => {}
            }
            workspace.refresh();
            self.handle_rebis_action(action);
            return;
        }

        if workspace.window_prefix {
            workspace.window_prefix = false;
            match code {
                KeyCode::Char('l') | KeyCode::Right => {
                    workspace.panel_visible = true;
                    workspace.graph_focus = true;
                    workspace.message = "mandala focus · Ctrl-W h returns to source".to_string();
                }
                KeyCode::Char('h') | KeyCode::Left => {
                    workspace.graph_focus = false;
                    workspace.message = "source focus · Ctrl-W l opens mandala".to_string();
                }
                _ => workspace.message = "unknown Ctrl-W window motion".to_string(),
            }
            return;
        }
        if ctrl && code == KeyCode::Char('w') && !matches!(workspace.mode, RebisMode::Insert) {
            workspace.window_prefix = true;
            workspace.message = "Ctrl-W: h source · l mandala".to_string();
            return;
        }

        if workspace.graph_focus
            && !matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand)
        {
            match code {
                KeyCode::Esc => {
                    workspace.graph_focus = false;
                    workspace.message = "source focus".to_string();
                }
                KeyCode::Char(':') => {
                    workspace.mode = RebisMode::Command;
                    workspace.command.clear();
                }
                // In the sigil browser, vertical motion moves the selection
                // and Enter opens it; other views scroll as before.
                KeyCode::Char('j') | KeyCode::Down
                    if workspace.visualization == Visualization::Sigils =>
                {
                    workspace.move_sigil_choice(1);
                }
                KeyCode::Char('k') | KeyCode::Up
                    if workspace.visualization == Visualization::Sigils =>
                {
                    workspace.move_sigil_choice(-1);
                }
                KeyCode::Enter if workspace.visualization == Visualization::Sigils => {
                    workspace.open_selected_sigil();
                }
                KeyCode::Tab | KeyCode::BackTab
                    if workspace.visualization == Visualization::Sigils =>
                {
                    workspace.toggle_selected_folder();
                }
                KeyCode::Char('j') | KeyCode::Down => workspace.graph_top += 1,
                KeyCode::Char('k') | KeyCode::Up => {
                    workspace.graph_top = workspace.graph_top.saturating_sub(1)
                }
                KeyCode::Char('l') | KeyCode::Right => workspace.graph_left += 4,
                KeyCode::Char('h') | KeyCode::Left => {
                    workspace.graph_left = workspace.graph_left.saturating_sub(4)
                }
                KeyCode::PageDown => workspace.graph_top += 10,
                KeyCode::PageUp => workspace.graph_top = workspace.graph_top.saturating_sub(10),
                KeyCode::Home => workspace.graph_left = 0,
                KeyCode::Char('g') => workspace.graph_top = 0,
                _ => {}
            }
            return;
        }

        // Saving is available from every editor mode.
        if ctrl && matches!(code, KeyCode::Char('s')) {
            workspace.command = "w".to_string();
            action = workspace.execute_command();
        } else {
            match workspace.mode {
                RebisMode::Insert => match code {
                    KeyCode::Esc => {
                        workspace.editor.end_insert_session();
                        workspace.mode = RebisMode::Normal;
                        workspace.editor.clear_pending();
                    }
                    KeyCode::Char('c') if ctrl => {
                        workspace.editor.end_insert_session();
                        workspace.mode = RebisMode::Normal;
                    }
                    KeyCode::Char(character) if !ctrl => match character {
                        '(' => workspace.editor.insert_pair('(', ')'),
                        '[' => workspace.editor.insert_pair('[', ']'),
                        '"' if workspace.editor.skip_close('"') => {}
                        '"' => workspace.editor.insert_pair('"', '"'),
                        ')' if workspace.editor.skip_close(')') => {}
                        ']' if workspace.editor.skip_close(']') => {}
                        _ => workspace.editor.insert(character),
                    },
                    KeyCode::Enter => workspace.editor.insert('\n'),
                    KeyCode::Tab => {
                        workspace.editor.insert(' ');
                        workspace.editor.insert(' ');
                    }
                    KeyCode::Backspace => workspace.editor.backspace(),
                    KeyCode::Delete => {
                        workspace.editor.delete();
                    }
                    KeyCode::Left => workspace.editor.left(),
                    KeyCode::Right => workspace.editor.right(),
                    KeyCode::Up => workspace.editor.vertical(-1),
                    KeyCode::Down => workspace.editor.vertical(1),
                    KeyCode::Home => workspace.editor.line_start_motion(),
                    KeyCode::End => workspace.editor.line_end(),
                    _ => {}
                },
                RebisMode::Normal => {
                    let normal = if let KeyCode::Char(character) = code {
                        workspace.editor.normal_key(character)
                    } else {
                        NormalAction::Unhandled
                    };
                    match normal {
                        NormalAction::Pending | NormalAction::Moved | NormalAction::Edited => {}
                        NormalAction::Yanked => {
                            workspace.message = "selection yanked".to_string();
                        }
                        NormalAction::EnterInsert => {
                            workspace.editor.begin_insert_session(true);
                            workspace.mode = RebisMode::Insert;
                        }
                        NormalAction::Unhandled => match code {
                            KeyCode::Char(':') => {
                                workspace.mode = RebisMode::Command;
                                workspace.command.clear();
                                workspace.editor.clear_pending();
                            }
                            KeyCode::Char('i') => {
                                workspace.editor.begin_insert_session(false);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('v') if ctrl => {
                                workspace.editor.begin_visual_block();
                                workspace.mode = RebisMode::VisualBlock;
                            }
                            KeyCode::Char('V') | KeyCode::Char('v') if shift => {
                                workspace.editor.begin_visual(true);
                                workspace.mode = RebisMode::VisualLine;
                            }
                            KeyCode::Char('v') => {
                                workspace.editor.begin_visual(false);
                                workspace.mode = RebisMode::Visual;
                            }
                            KeyCode::Char('a') => {
                                workspace.editor.append_after_cursor();
                                workspace.editor.begin_insert_session(false);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('I') => {
                                let _ = workspace.editor.normal_key('^');
                                workspace.editor.begin_insert_session(false);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('A') => {
                                workspace.editor.line_end();
                                workspace.editor.begin_insert_session(false);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('o') => {
                                workspace.editor.open_below();
                                workspace.editor.begin_insert_session(true);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('O') => {
                                workspace.editor.open_above();
                                workspace.editor.begin_insert_session(true);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Left => {
                                let _ = workspace.editor.normal_key('h');
                            }
                            KeyCode::Right => {
                                let _ = workspace.editor.normal_key('l');
                            }
                            KeyCode::Down => {
                                let _ = workspace.editor.normal_key('j');
                            }
                            KeyCode::Up => {
                                let _ = workspace.editor.normal_key('k');
                            }
                            KeyCode::Home => {
                                let _ = workspace.editor.normal_key('0');
                            }
                            KeyCode::End => {
                                let _ = workspace.editor.normal_key('$');
                            }
                            KeyCode::Char('x') | KeyCode::Delete => {
                                workspace.editor.delete();
                            }
                            KeyCode::Char('p') => workspace.editor.paste_after(),
                            KeyCode::Char('P') => workspace.editor.paste_before(),
                            KeyCode::Char('D') => {
                                workspace.editor.delete_to_line_end();
                            }
                            KeyCode::Char('C') => {
                                let changed = workspace.editor.delete_to_line_end();
                                workspace.editor.begin_insert_session(changed);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('s') => {
                                let changed = workspace.editor.delete();
                                workspace.editor.begin_insert_session(changed);
                                workspace.mode = RebisMode::Insert;
                            }
                            KeyCode::Char('u') if !ctrl => workspace.editor.undo(),
                            KeyCode::Char('r') if ctrl => workspace.editor.redo(),
                            KeyCode::Char('%') => {
                                if !workspace.editor.jump_matching_parenthesis() {
                                    workspace.message =
                                        "no matching structural parenthesis".to_string();
                                }
                            }
                            KeyCode::Esc => workspace.editor.clear_pending(),
                            _ => workspace.editor.clear_pending(),
                        },
                    }
                }
                RebisMode::Visual | RebisMode::VisualLine => {
                    let linewise = workspace.mode == RebisMode::VisualLine;
                    match code {
                        KeyCode::Char('V') | KeyCode::Char('v') if shift => {
                            workspace.editor.begin_visual(true);
                            workspace.mode = RebisMode::VisualLine;
                        }
                        KeyCode::Esc | KeyCode::Char('v') => {
                            workspace.editor.end_visual();
                            workspace.mode = RebisMode::Normal;
                        }
                        KeyCode::Char(character)
                            if character.is_ascii_digit()
                                || matches!(
                                    character,
                                    'h' | 'l'
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
                                ) =>
                        {
                            let _ = workspace.editor.normal_key(character);
                        }
                        KeyCode::Left => {
                            let _ = workspace.editor.normal_key('h');
                        }
                        KeyCode::Right => {
                            let _ = workspace.editor.normal_key('l');
                        }
                        KeyCode::Down => {
                            let _ = workspace.editor.normal_key('j');
                        }
                        KeyCode::Up => {
                            let _ = workspace.editor.normal_key('k');
                        }
                        KeyCode::Home => {
                            let _ = workspace.editor.normal_key('0');
                        }
                        KeyCode::End => {
                            let _ = workspace.editor.normal_key('$');
                        }
                        KeyCode::Char('y') => {
                            workspace.editor.yank_visual(linewise);
                            workspace.mode = RebisMode::Normal;
                            workspace.message = "selection yanked".to_string();
                        }
                        KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => {
                            workspace.editor.delete_visual(linewise);
                            workspace.mode = RebisMode::Normal;
                        }
                        KeyCode::Char('c') => {
                            workspace.editor.delete_visual(linewise);
                            workspace.editor.begin_insert_session(true);
                            workspace.mode = RebisMode::Insert;
                        }
                        KeyCode::Char('p') | KeyCode::Char('P') => {
                            workspace.editor.paste_visual(linewise);
                            workspace.mode = RebisMode::Normal;
                        }
                        KeyCode::Char(':') => {
                            workspace.mode = RebisMode::Command;
                            workspace.command.clear();
                        }
                        _ => {}
                    }
                }
                RebisMode::VisualBlock => match code {
                    // Ctrl-V toggles the block selection off; a bare `v`/`V`
                    // switches to character- or line-wise selection in place.
                    KeyCode::Esc => {
                        workspace.editor.end_visual();
                        workspace.mode = RebisMode::Normal;
                    }
                    KeyCode::Char('v') if ctrl => {
                        workspace.editor.end_visual();
                        workspace.mode = RebisMode::Normal;
                    }
                    KeyCode::Char('V') | KeyCode::Char('v') if shift => {
                        workspace.editor.begin_visual(true);
                        workspace.mode = RebisMode::VisualLine;
                    }
                    KeyCode::Char('v') => {
                        workspace.editor.begin_visual(false);
                        workspace.mode = RebisMode::Visual;
                    }
                    KeyCode::Char(character)
                        if character.is_ascii_digit()
                            || matches!(
                                character,
                                'h' | 'l'
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
                            ) =>
                    {
                        let _ = workspace.editor.normal_key(character);
                    }
                    KeyCode::Left => {
                        let _ = workspace.editor.normal_key('h');
                    }
                    KeyCode::Right => {
                        let _ = workspace.editor.normal_key('l');
                    }
                    KeyCode::Down => {
                        let _ = workspace.editor.normal_key('j');
                    }
                    KeyCode::Up => {
                        let _ = workspace.editor.normal_key('k');
                    }
                    KeyCode::Home => {
                        let _ = workspace.editor.normal_key('0');
                    }
                    KeyCode::End => {
                        let _ = workspace.editor.normal_key('$');
                    }
                    KeyCode::Char('y') => {
                        workspace.editor.yank_visual_block();
                        workspace.mode = RebisMode::Normal;
                        workspace.message = "block yanked".to_string();
                    }
                    KeyCode::Char('d') | KeyCode::Char('x') | KeyCode::Delete => {
                        workspace.editor.delete_visual_block();
                        workspace.mode = RebisMode::Normal;
                    }
                    KeyCode::Char('c') => {
                        workspace.editor.delete_visual_block();
                        workspace.editor.begin_insert_session(true);
                        workspace.mode = RebisMode::Insert;
                    }
                    KeyCode::Char(':') => {
                        workspace.mode = RebisMode::Command;
                        workspace.command.clear();
                    }
                    _ => {}
                },
                RebisMode::Command => match code {
                    KeyCode::Esc => {
                        workspace.command.clear();
                        workspace.mode = if workspace.vim_enabled {
                            RebisMode::Normal
                        } else {
                            RebisMode::Insert
                        };
                    }
                    KeyCode::Enter => action = workspace.execute_command(),
                    KeyCode::Backspace => {
                        workspace.command.pop();
                    }
                    KeyCode::Char(character) if !ctrl => workspace.command.push(character),
                    _ => {}
                },
                RebisMode::KaosCommand => match code {
                    KeyCode::Esc => {
                        workspace.command.clear();
                        workspace.mode = if workspace.vim_enabled {
                            RebisMode::Normal
                        } else {
                            RebisMode::Insert
                        };
                    }
                    KeyCode::Up => {
                        let count = rebis_completions(&workspace.command).len();
                        if count > 0 {
                            workspace.command_choice = workspace.command_choice.saturating_sub(1);
                        }
                    }
                    KeyCode::Down => {
                        let count = rebis_completions(&workspace.command).len();
                        if count > 0 {
                            workspace.command_choice =
                                (workspace.command_choice + 1).min(count - 1);
                        }
                    }
                    KeyCode::Tab | KeyCode::BackTab => {
                        let choices = rebis_completions(&workspace.command);
                        if !choices.is_empty() {
                            workspace.command = choices
                                [workspace.command_choice.min(choices.len() - 1)]
                            .insert
                            .to_string();
                        }
                    }
                    KeyCode::Enter => {
                        let choices = rebis_completions(&workspace.command);
                        if !choices.is_empty() {
                            let command = choices[workspace.command_choice.min(choices.len() - 1)];
                            if missing_command_argument(&workspace.command, command) {
                                workspace.command = command.insert.to_string();
                                return;
                            }
                            if !workspace.command.starts_with(command.insert) {
                                workspace.command = command.insert.to_string();
                            }
                        }
                        action = workspace.execute_kaos_command();
                    }
                    KeyCode::Backspace => {
                        workspace.command.pop();
                        workspace.command_choice = 0;
                    }
                    KeyCode::Char(character) if !ctrl => {
                        workspace.command.push(character);
                        workspace.command_choice = 0;
                    }
                    _ => {}
                },
            }
        }

        // Motions do not change the AST, but compiling them is cheap and keeping a
        // single refresh point prevents a future edit verb from forgetting it.
        workspace.refresh();
        self.handle_rebis_action(action);
    }

    fn open_rebis(&mut self, path: Option<&str>) {
        if path.is_none() {
            if let Some(workspace) = self.suspended_rebis.take() {
                self.rebis = Some(workspace);
                return;
            }
        }
        match RebisWorkspace::open(self.cwd.clone(), path) {
            Ok(workspace) => self.rebis = Some(workspace),
            Err(error) => self.note(&error),
        }
    }

    fn open_config(&mut self, restore: bool) {
        let path = if restore {
            crate::config::restore_defaults()
        } else {
            crate::config::load().map_err(|error| {
                format!(
                    "could not open {}: {error}",
                    crate::config::path().display()
                )
            })
        };
        match path {
            Ok(path) => self.open_config_path(path, restore),
            Err(error) => self.note(&error),
        }
    }

    /// Open the real Kaos config in the existing editor without destroying an
    /// in-progress Rebis document. `:q` returns to exactly the previous surface.
    fn open_config_path(&mut self, path: PathBuf, restored: bool) {
        let previous = self.rebis.take();
        let was_config_editor = self.config_editor;
        match RebisWorkspace::open(self.cwd.clone(), path.to_str()) {
            Ok(mut workspace) => {
                if !was_config_editor {
                    self.config_return_rebis = previous;
                }
                workspace.panel_visible = false;
                workspace.graph_focus = false;
                workspace.message = if restored {
                    "all defaults restored · edit if needed · :w saves · :q returns · restart Kaos to apply"
                        .to_string()
                } else {
                    "Kaos config · :w saves · :q returns · restart Kaos to apply".to_string()
                };
                self.rebis = Some(workspace);
                self.config_editor = true;
            }
            Err(error) => {
                self.rebis = previous;
                self.note(&error);
            }
        }
    }

    fn leave_config_editor(&mut self) {
        self.rebis = self.config_return_rebis.take();
        self.config_editor = false;
    }

    /// The config document may temporarily occupy `self.rebis`, but run state
    /// belongs to the actual Rebis workspace parked underneath it.
    fn background_rebis_workspace(&mut self) -> Option<&mut RebisWorkspace> {
        if self.config_editor {
            self.config_return_rebis
                .as_mut()
                .or(self.suspended_rebis.as_mut())
        } else {
            self.rebis.as_mut().or(self.suspended_rebis.as_mut())
        }
    }

    fn handle_workspace_events(&mut self) {
        loop {
            let event = self
                .background_rebis_workspace()
                .and_then(RebisWorkspace::take_host_event);
            match event {
                Some(WorkspaceEvent::SigilSaved(name)) => self.save_sigil_run_state(&name),
                Some(WorkspaceEvent::SigilOpened(name)) => self.restore_sigil_run_state(&name),
                None => break,
            }
        }
    }

    fn save_sigil_run_state(&mut self, name: &str) {
        let paths = self
            .background_rebis_workspace()
            .map(|workspace| workspace.sigil_resume_paths(name));
        let Some((run_path, checkpoint_path)) = paths else {
            return;
        };
        let matching = self.rebis_runs.iter().rposition(|run| {
            run.state == RebisRunState::Running && run.sigil.as_deref() == Some(name)
        });
        let selected = self
            .rebis_runs
            .get(self.rebis_run_choice)
            .filter(|run| run.state == RebisRunState::Running)
            .map(|_| self.rebis_run_choice);
        let fallback = self
            .rebis_runs
            .iter()
            .rposition(|run| run.state == RebisRunState::Running);
        let Some(index) = matching.or(selected).or(fallback) else {
            let _ = std::fs::remove_file(&run_path);
            let _ = std::fs::remove_file(&checkpoint_path);
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace
                    .message
                    .push_str(" · no unfinished run to checkpoint");
            }
            return;
        };
        let id = self.rebis_runs[index].id;
        self.rebis_runs[index].sigil = Some(name.to_string());
        self.rebis_runs[index].saved_run_path = Some(run_path);
        self.rebis_runs[index].saved_checkpoint_path = Some(checkpoint_path);
        match self.persist_rebis_run(id) {
            Ok(()) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace
                        .message
                        .push_str(" · resumable step, record, trace, and prompt journal saved");
                }
            }
            Err(error) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace
                        .message
                        .push_str(&format!(" · could not save resumable step: {error}"));
                }
            }
        }
    }

    fn persist_rebis_run(&self, id: u64) -> Result<(), String> {
        let run = self
            .rebis_runs
            .iter()
            .find(|run| run.id == id)
            .ok_or_else(|| format!("run #{id} no longer exists"))?;
        let run_path = run
            .saved_run_path
            .as_ref()
            .ok_or_else(|| "run has no saved sigil metadata path".to_string())?;
        let saved_checkpoint = run
            .saved_checkpoint_path
            .as_ref()
            .ok_or_else(|| "run has no saved sigil checkpoint path".to_string())?;
        let saved = SavedRebisRun {
            source: run.request.source.clone(),
            input: run.request.input.clone(),
            scope: run.scope,
            parallel: run.parallel,
            chaos: run.chaos,
            output: run.output.clone(),
            elapsed: active_rebis_run_elapsed(run),
            pause_reason: run
                .pause_reason
                .clone()
                .unwrap_or_else(|| "saved at the last completed prompt".to_string()),
        };
        atomic_write(run_path, &encode_saved_rebis_run(&saved))
            .map_err(|error| format!("could not write {}: {error}", run_path.display()))?;
        if run.checkpoint_path != *saved_checkpoint {
            match std::fs::read(&run.checkpoint_path) {
                Ok(checkpoint) => atomic_write(saved_checkpoint, &checkpoint).map_err(|error| {
                    format!("could not write {}: {error}", saved_checkpoint.display())
                })?,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    let _ = std::fs::remove_file(saved_checkpoint);
                }
                Err(error) => {
                    return Err(format!(
                        "could not read {}: {error}",
                        run.checkpoint_path.display()
                    ));
                }
            }
        }
        Ok(())
    }

    fn restore_sigil_run_state(&mut self, name: &str) {
        if let Some(index) = self.rebis_runs.iter().position(|run| {
            run.state == RebisRunState::Running && run.sigil.as_deref() == Some(name)
        }) {
            self.rebis_run_choice = index;
            let resident_id = self.rebis_runs[index].id;
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.message.push_str(&format!(
                    " · unfinished run #{} is still resident · /runs then p resumes",
                    resident_id
                ));
            }
            return;
        }
        let Some((run_path, checkpoint_path)) = self
            .background_rebis_workspace()
            .map(|workspace| workspace.sigil_resume_paths(name))
        else {
            return;
        };
        let encoded = match std::fs::read(&run_path) {
            Ok(encoded) => encoded,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return,
            Err(error) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace.message.push_str(&format!(
                        " · could not read saved run {}: {error}",
                        run_path.display()
                    ));
                }
                return;
            }
        };
        let saved = match decode_saved_rebis_run(&encoded) {
            Ok(saved) => saved,
            Err(error) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace
                        .message
                        .push_str(&format!(" · saved run ignored: {error}"));
                }
                return;
            }
        };
        let program_source = self
            .background_rebis_workspace()
            .map(|workspace| workspace.editor.source().to_string())
            .unwrap_or_else(|| saved.source.clone());
        let source = if saved.scope == RunScope::Program {
            program_source
        } else {
            saved.source.clone()
        };
        if let Err(error) = rebis_lang::parse(&source) {
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace
                    .message
                    .push_str(&format!(" · saved run source is invalid: {error}"));
            }
            return;
        }
        let request = RunRequest {
            source,
            input: saved.input,
            scope: saved.scope,
        };
        let id =
            self.register_rebis_run_with_mode(&request, RebisRunState::Running, saved.parallel);
        let now = Instant::now();
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            run.sigil = Some(name.to_string());
            run.saved_run_path = Some(run_path);
            run.saved_checkpoint_path = Some(checkpoint_path.clone());
            run.checkpoint_path = checkpoint_path;
            run.chaos = saved.chaos;
            run.output = saved.output;
            run.output.push(
                "restored    Ⅱ saved sigil checkpoint · p reconstructs the unfinished prompt"
                    .to_string(),
            );
            run.expanded = true;
            run.started_at = now.checked_sub(saved.elapsed).or(Some(now));
            run.elapsed = None;
            run.paused = true;
            run.paused_at = Some(now);
            run.paused_total = Duration::ZERO;
            run.pause_reason = Some(saved.pause_reason);
        }
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.message.push_str(&format!(
                " · run #{id} restored at its last completed prompt · /runs then p resumes"
            ));
        }
    }

    fn handle_rebis_action(&mut self, action: WorkspaceAction) {
        match action {
            WorkspaceAction::None => {}
            WorkspaceAction::Suspend => {
                if self.config_editor {
                    self.leave_config_editor();
                } else {
                    self.suspended_rebis = self.rebis.take();
                }
            }
            WorkspaceAction::Discard => {
                if self.sigil_chat_job.is_some() {
                    if let Some(workspace) = &mut self.rebis {
                        workspace.message =
                            "god agent is still working · ^C cancels before :q!".to_string();
                    }
                    return;
                }
                if self.config_editor {
                    self.leave_config_editor();
                } else {
                    self.rebis = None;
                    self.suspended_rebis = None;
                }
            }
            WorkspaceAction::Run(request) => self.submit_rebis_run(request, false),
            WorkspaceAction::RunParallel(request) => self.submit_rebis_run(request, true),
            WorkspaceAction::BrowseRuns => {
                if let Some(workspace) = &mut self.rebis {
                    workspace.hide_sigil_chat();
                    workspace.panel_visible = true;
                    workspace.runs_visible = true;
                    workspace.graph_focus = true;
                }
                if self.rebis_runs.is_empty() {
                    if let Some(workspace) = &mut self.rebis {
                        workspace.message = "no Rebis runs yet".to_string();
                    }
                } else {
                    self.rebis_run_choice = self
                        .rebis_runs
                        .iter()
                        .position(|run| run.state == RebisRunState::Running)
                        .unwrap_or(self.rebis_runs.len() - 1);
                    self.focus_selected_rebis_run();
                    self.describe_rebis_run_choice();
                }
            }
            WorkspaceAction::OpenSigilChat => self.open_sigil_chat_channel(),
            WorkspaceAction::SigilChat(message) => self.submit_sigil_chat_message(message),
            WorkspaceAction::Kaos(command) => {
                self.dispatch(&format!("/{command}"));
                let mouse_captured = self.mouse_captured;
                if let Some(workspace) = &mut self.rebis {
                    // `set_model` already reports whether the selection persisted.
                    if command.starts_with("chaos") {
                        workspace.message = if self.rebis_chaos_mode {
                            "CHAOS mode · each Rebis prompt uses the Kaos Conductor pipeline"
                                .to_string()
                        } else {
                            "DIRECT mode · one tool agent per Rebis prompt".to_string()
                        };
                    } else if command.starts_with("config") {
                        // Opening the config sets a more useful editor-specific
                        // message than the generic `/<command> executed` notice.
                    } else if !command.starts_with("model") {
                        workspace.message = if command.starts_with("mouse") {
                            if mouse_captured {
                                "mouse captured — drag copies within one pane · /mouse off uses raw terminal selection".to_string()
                            } else {
                                "mouse released — raw terminal selection may cross panes · /mouse on clips drags".to_string()
                            }
                        } else {
                            format!("/{command} executed")
                        };
                    }
                }
            }
        }
    }

    fn sigil_chat_run_context(&self, bound_id: Option<u64>) -> String {
        let live = self
            .rebis_runs
            .iter()
            .filter(|run| {
                matches!(
                    run.state,
                    RebisRunState::AwaitingPermission
                        | RebisRunState::Queued
                        | RebisRunState::Running
                )
            })
            .collect::<Vec<_>>();
        let mut context = format!(
            "GLOBAL LIVE BOT SNAPSHOT\nlive bots: {}\nbound mutation target: {}\npeer sources and state are read-only unless run-control.txt contains an explicit validated action\n",
            live.len(),
            bound_id.map_or_else(|| "none".to_string(), |id| format!("run #{id}"))
        );
        if live.is_empty() {
            context.push_str("\nNO LIVE, PAUSED, QUEUED, OR PERMISSION-GATED REBIS BOTS\n");
            return context;
        }
        for run in live {
            let role = if Some(run.id) == bound_id {
                "BOUND SOURCE/MUTATION TARGET"
            } else {
                "READ-ONLY PEER BOT"
            };
            let child = if self.job_for_run(run.id).is_some() {
                "resident child"
            } else {
                "no resident child"
            };
            let checkpoint = std::fs::read(&run.checkpoint_path).map_or_else(
                |_| "(no completed prompt checkpoint yet)".to_string(),
                |bytes| String::from_utf8_lossy(&bytes).into_owned(),
            );
            let directive = crate::rebis_supervisor::read_directive(&run.directive_path)
                .unwrap_or_else(|| "(none)".to_string());
            let trace = if run.output.is_empty() {
                "(no retained trace yet)".to_string()
            } else {
                run.output.join("\n")
            };
            context.push_str(&format!(
                "\n===== RUN #{} · {role} =====\nstate: {:?}\npaused: {}\npause reason: {}\nchild: {child}\nsigil: {}\nscope: {}\nmode: {}\nparallel: {}\ntimer: {}\ncheckpoint path: {}\ndirective path: {}\n\nCURRENT SUPERVISOR DIRECTIVE\n{}\n\nCAPTURED SOURCE\n{}\n\nCAPTURED RECORD / INPUT\n{}\n\nPROMPT CHECKPOINT JOURNAL\n{}\n\nFULL RETAINED TRACE\n{}\n===== END RUN #{} =====\n",
                run.id,
                run.state,
                run.paused,
                run.pause_reason.as_deref().unwrap_or("none"),
                run.sigil.as_deref().unwrap_or("unsaved"),
                run.scope.label(),
                if run.chaos { "CHAOS" } else { "DIRECT" },
                run.parallel,
                rebis_run_timer(run),
                run.checkpoint_path.display(),
                run.directive_path.display(),
                directive,
                run.request.source,
                run.request.input,
                checkpoint,
                trace,
                run.id
            ));
        }
        context
    }

    fn write_sigil_chat_control_bridge(&self, bridge_dir: &std::path::Path) -> Result<(), String> {
        let control = b"# GOD AGENT RUN CONTROL\n# Write only actions explicitly requested in the current USER TURN.\n# Valid actions: PAUSE ID, RESUME ID, APPLY_DIRECTIVE ID, CLEAR_DIRECTIVE ID\n# For APPLY_DIRECTIVE, first edit runs/ID/directive.txt.\n# Cancellation and deletion are intentionally unavailable.\n";
        atomic_write(&bridge_dir.join("run-control.txt"), control)
            .map_err(|error| format!("could not create run control manifest: {error}"))?;
        for run in self.rebis_runs.iter().filter(|run| {
            matches!(
                run.state,
                RebisRunState::AwaitingPermission | RebisRunState::Queued | RebisRunState::Running
            )
        }) {
            let directory = bridge_dir.join("runs").join(run.id.to_string());
            std::fs::create_dir_all(&directory)
                .map_err(|error| format!("could not create {}: {error}", directory.display()))?;
            atomic_write(
                &directory.join("source.rebis"),
                run.request.source.as_bytes(),
            )
            .map_err(|error| format!("could not snapshot run #{} source: {error}", run.id))?;
            atomic_write(&directory.join("input.txt"), run.request.input.as_bytes())
                .map_err(|error| format!("could not snapshot run #{} input: {error}", run.id))?;
            atomic_write(
                &directory.join("trace.txt"),
                run.output.join("\n").as_bytes(),
            )
            .map_err(|error| format!("could not snapshot run #{} trace: {error}", run.id))?;
            let directive =
                crate::rebis_supervisor::read_directive(&run.directive_path).unwrap_or_default();
            atomic_write(&directory.join("directive.txt"), directive.as_bytes()).map_err(
                |error| format!("could not snapshot run #{} directive: {error}", run.id),
            )?;
        }
        Ok(())
    }

    fn refresh_sigil_chat_run_context(&self) {
        let Some(turn) = &self.sigil_chat_job else {
            return;
        };
        let workspace = self.rebis.as_ref().or(self.suspended_rebis.as_ref());
        let source_label =
            workspace.map_or_else(|| "[unavailable]".to_string(), RebisWorkspace::path_label);
        let channel_history = workspace.map_or_else(String::new, |workspace| {
            workspace.sigil_chat_lines().join("\n")
        });
        let run_context = self.sigil_chat_run_context(turn.run_id);
        let context = format!(
            "SIGIL: {source_label}\nMODEL: {}\n\n{run_context}\nCHANNEL HISTORY\n{channel_history}\n",
            self.model
        );
        let _ = atomic_write(&turn.bridge_dir.join("run-context.txt"), context.as_bytes());
    }

    /// Bind the panel channel to the selected unfinished run when possible.
    /// Completed runs remain immutable evidence; with none unfinished, the god
    /// agent can still revise the editor source for the next run.
    fn open_sigil_chat_channel(&mut self) {
        if let Some(run_id) = self.sigil_chat_job.as_ref().map(|turn| turn.run_id) {
            if let Some(workspace) = &mut self.rebis {
                workspace.bind_sigil_chat_run(run_id);
                workspace.open_sigil_chat();
            }
            return;
        }
        let selected = self
            .rebis_runs
            .get(self.rebis_run_choice)
            .filter(|run| {
                matches!(
                    run.state,
                    RebisRunState::AwaitingPermission
                        | RebisRunState::Queued
                        | RebisRunState::Running
                )
            })
            .map(|run| run.id);
        let run_id = selected.or_else(|| {
            self.rebis_runs
                .iter()
                .rev()
                .find(|run| {
                    matches!(
                        run.state,
                        RebisRunState::AwaitingPermission
                            | RebisRunState::Queued
                            | RebisRunState::Running
                    )
                })
                .map(|run| run.id)
        });
        if let Some(workspace) = &mut self.rebis {
            workspace.bind_sigil_chat_run(run_id);
            workspace.open_sigil_chat();
        }
    }

    /// Stop a live interpreter while its supervisor reasons over an exact
    /// source/trace snapshot. This is SIGSTOP, not cancellation: the prompt
    /// journal and process remain intact unless a valid source edit requires a
    /// checkpoint reconstruction.
    fn pause_rebis_run_by_id(&mut self, id: u64, reason: &str) -> Result<(), String> {
        let Some(run) = self.rebis_runs.iter().find(|run| run.id == id) else {
            return Err(format!("run #{id} does not exist"));
        };
        if run.state != RebisRunState::Running || run.paused {
            return Err(format!("run #{id} is not an unpaused running bot"));
        }
        let Some(job) = self.job_for_run(id) else {
            return Err(format!("run #{id} has no resident child to pause"));
        };
        let pid = job
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .id();
        let owns_process_group = job.owns_process_group;
        if !signal_process(pid, owns_process_group, "-STOP") {
            return Err(format!("could not pause run #{id}"));
        }
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            pause_rebis_run_clock(run, reason);
            run.output.push(format!("paused      ⏸ {reason}"));
        }
        Ok(())
    }

    fn resume_rebis_run_by_id(&mut self, id: u64) -> Result<(), String> {
        let Some(run) = self.rebis_runs.iter().find(|run| run.id == id) else {
            return Err(format!("run #{id} does not exist"));
        };
        if run.state != RebisRunState::Running || !run.paused {
            return Err(format!("run #{id} is not a paused running bot"));
        }
        let request = run.request.clone();
        let parallel = run.parallel;
        if let Some(job) = self.job_for_run(id) {
            let pid = job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .id();
            if !signal_process(pid, job.owns_process_group, "-CONT") {
                return Err(format!("could not resume run #{id}"));
            }
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                resume_rebis_run_clock(run);
                run.output
                    .push("resumed     ▶ god-agent run control".to_string());
            }
        } else {
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                resume_rebis_run_clock(run);
                run.output
                    .push("resumed     ▶ god-agent control rebuilding from checkpoint".to_string());
            }
            self.start_rebis_run(id, request, parallel);
        }
        Ok(())
    }

    fn pause_run_for_sigil_chat(&mut self, id: u64) -> bool {
        self.pause_rebis_run_by_id(id, "god agent inspecting source and global run context")
            .is_ok()
    }

    fn apply_sigil_chat_run_controls(&mut self, bridge_dir: &std::path::Path) {
        let control_path = bridge_dir.join("run-control.txt");
        let source = match std::fs::read_to_string(&control_path) {
            Ok(source) => source,
            Err(error) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace.push_sigil_chat_line(format!(
                        "system  run control ignored: could not read {}: {error}",
                        control_path.display()
                    ));
                }
                return;
            }
        };
        let controls = match parse_sigil_run_controls(&source) {
            Ok(controls) => controls,
            Err(error) => {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace.push_sigil_chat_line(format!(
                        "system  all run controls rejected: {error}"
                    ));
                }
                return;
            }
        };
        for control in controls {
            let id = match control {
                SigilRunControl::Pause(id)
                | SigilRunControl::Resume(id)
                | SigilRunControl::ApplyDirective(id)
                | SigilRunControl::ClearDirective(id) => id,
            };
            let live = self.rebis_runs.iter().any(|run| {
                run.id == id
                    && matches!(
                        run.state,
                        RebisRunState::AwaitingPermission
                            | RebisRunState::Queued
                            | RebisRunState::Running
                    )
            });
            if !live {
                if let Some(workspace) = self.background_rebis_workspace() {
                    workspace.push_sigil_chat_line(format!(
                        "system  control rejected: run #{id} is not live"
                    ));
                }
                continue;
            }
            let result = match control {
                SigilRunControl::Pause(_) => self
                    .pause_rebis_run_by_id(id, "paused by explicit god-agent run control")
                    .map(|()| format!("run #{id} paused")),
                SigilRunControl::Resume(_) => self
                    .resume_rebis_run_by_id(id)
                    .map(|()| format!("run #{id} resumed")),
                SigilRunControl::ApplyDirective(_) => {
                    let draft = bridge_dir
                        .join("runs")
                        .join(id.to_string())
                        .join("directive.txt");
                    match std::fs::read_to_string(&draft) {
                        Ok(directive) if !directive.trim().is_empty() => {
                            let directive = directive.trim().to_string();
                            let runtime_path = self
                                .rebis_runs
                                .iter()
                                .find(|run| run.id == id)
                                .map(|run| run.directive_path.clone())
                                .expect("live run checked");
                            atomic_write(&runtime_path, directive.as_bytes())
                                .map_err(|error| {
                                    format!("could not set directive for run #{id}: {error}")
                                })
                                .map(|()| {
                                    if let Some(run) =
                                        self.rebis_runs.iter_mut().find(|run| run.id == id)
                                    {
                                        run.output.push(format!(
                                            "directive   god agent set {} byte(s) of guidance",
                                            directive.len()
                                        ));
                                    }
                                    format!(
                                        "run #{id} directive applied · affects its next unfinished prompt"
                                    )
                                })
                        }
                        Ok(_) => Err(format!(
                            "run #{id} directive is empty; use CLEAR_DIRECTIVE to remove guidance"
                        )),
                        Err(error) => Err(format!(
                            "could not read directive draft for run #{id}: {error}"
                        )),
                    }
                }
                SigilRunControl::ClearDirective(_) => {
                    let runtime_path = self
                        .rebis_runs
                        .iter()
                        .find(|run| run.id == id)
                        .map(|run| run.directive_path.clone())
                        .expect("live run checked");
                    match std::fs::remove_file(&runtime_path) {
                        Ok(()) => Ok(()),
                        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                        Err(error) => {
                            Err(format!("could not clear directive for run #{id}: {error}"))
                        }
                    }
                    .map(|()| {
                        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                            run.output
                                .push("directive   god-agent guidance cleared".to_string());
                        }
                        format!("run #{id} directive cleared")
                    })
                }
            };
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.push_sigil_chat_line(match result {
                    Ok(message) => format!("system  ✓ {message}"),
                    Err(error) => format!("system  control rejected: {error}"),
                });
            }
        }
    }

    fn submit_sigil_chat_message(&mut self, message: String) {
        if self.sigil_chat_job.is_some() {
            if let Some(workspace) = &mut self.rebis {
                workspace.push_sigil_chat_line("system  one god-agent turn is already running");
                workspace.set_sigil_chat_busy(true);
            }
            return;
        }
        let Some(workspace) = &self.rebis else {
            return;
        };
        let base_source = workspace.editor.source().to_string();
        let source_label = workspace.path_label();
        let run_id = workspace.sigil_chat_run_id();
        let channel_history = workspace.sigil_chat_lines().join("\n");
        let resume_after = run_id.is_some_and(|id| self.pause_run_for_sigil_chat(id));

        let bridge_dir = std::env::temp_dir().join(format!("kaos-sigil-chat-{}", self.session_id));
        if let Err(error) = std::fs::create_dir_all(&bridge_dir) {
            if let Some(workspace) = &mut self.rebis {
                workspace.push_sigil_chat_line(format!(
                    "system  could not create isolated source bridge: {error}"
                ));
            }
            if let Some(id) = run_id.filter(|_| resume_after) {
                self.resume_stopped_run(id);
            }
            return;
        }
        let source_path = bridge_dir.join("sigil.rebis");
        let context_path = bridge_dir.join("run-context.txt");
        let run_context = self.sigil_chat_run_context(run_id);
        let context = format!(
            "SIGIL: {source_label}\nMODEL: {}\n\n{run_context}\nCHANNEL HISTORY\n{channel_history}\n",
            self.model
        );
        let write_result = std::fs::write(&source_path, &base_source)
            .and_then(|()| std::fs::write(&context_path, context))
            .map_err(|error| error.to_string())
            .and_then(|()| self.write_sigil_chat_control_bridge(&bridge_dir));
        if let Err(error) = write_result {
            if let Some(workspace) = &mut self.rebis {
                workspace.push_sigil_chat_line(format!(
                    "system  could not capture source and run context: {error}"
                ));
            }
            if let Some(id) = run_id.filter(|_| resume_after) {
                self.resume_stopped_run(id);
            }
            return;
        }

        let task = format!(
            "You are the GOD AGENT supervising the complete live Rebis bot field. The current editor program is ./sigil.rebis. ./run-context.txt is a live-refreshed snapshot containing the full source, record/input, prompt checkpoint, directive, state, and retained trace of EVERY running, paused, queued, or permission-gated Rebis bot. Read both before answering. Per-run inspection files live under ./runs/ID/. One run is marked BOUND and is the only run whose source may be revised through ./sigil.rebis; peer sources are read-only. Preserve unrelated source and comments. The host rejects invalid Rebis and editor conflicts. A valid bound-source edit reconstructs that run from its prompt journal, preserving identical completed prompts.\n\nYou may control individual live bots only when the USER TURN explicitly requests it. Write validated actions to ./run-control.txt, one per line: PAUSE ID, RESUME ID, APPLY_DIRECTIVE ID, or CLEAR_DIRECTIVE ID. For APPLY_DIRECTIVE, first write the requested guidance to ./runs/ID/directive.txt. Directives are attached to that bot's next unfinished model prompt and remain active until replaced or cleared. Never invent actions, never target a completed/cancelled run, and never request cancellation or deletion. Do not directly launch, kill, or signal processes. Never claim an edit or control action unless you actually wrote the corresponding bridge file. Explain what you changed or answer directly.\n\nUSER TURN:\n{message}"
        );
        let before = self.parallel_jobs.len();
        let launched = self.spawn_job_with_input(
            vec!["code".to_string(), RAW_CHAT_TASK_ARG.to_string(), task],
            None,
            None,
            Some(false),
            true,
            Some(bridge_dir.clone()),
        );
        if !launched || self.parallel_jobs.len() != before + 1 {
            if let Some(workspace) = &mut self.rebis {
                workspace.push_sigil_chat_line("system  god agent could not be launched");
                workspace.set_sigil_chat_busy(false);
            }
            if let Some(id) = run_id.filter(|_| resume_after) {
                self.resume_stopped_run(id);
            }
            return;
        }
        let job = self.parallel_jobs.pop().expect("new god-agent job");
        self.sigil_chat_job = Some(SigilChatJob {
            job,
            base_source,
            bridge_dir,
            run_id,
            resume_after,
        });
        if let Some(workspace) = &mut self.rebis {
            workspace.set_sigil_chat_busy(true);
            let live_count = self
                .rebis_runs
                .iter()
                .filter(|run| {
                    matches!(
                        run.state,
                        RebisRunState::AwaitingPermission
                            | RebisRunState::Queued
                            | RebisRunState::Running
                    )
                })
                .count();
            workspace.push_sigil_chat_line(match run_id {
                Some(id) if resume_after => {
                    format!("system  run #{id} paused · god agent sees all {live_count} live bots")
                }
                Some(id) => format!(
                    "system  bound to run #{id} · god agent sees all {live_count} live bots"
                ),
                None => format!("system  inspecting current source and all {live_count} live bots"),
            });
        }
    }

    /// Continue a child that was stopped only to take a coherent supervisory
    /// snapshot and whose source did not need interpreter reconstruction.
    fn resume_stopped_run(&mut self, id: u64) -> bool {
        let Some(job) = self.job_for_run(id) else {
            return false;
        };
        let pid = job
            .child
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .id();
        let owns_process_group = job.owns_process_group;
        if !signal_process(pid, owns_process_group, "-CONT") {
            return false;
        }
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            resume_rebis_run_clock(run);
            run.output
                .push("resumed     ▶ god-agent inspection complete".to_string());
        }
        true
    }

    fn submit_rebis_run(&mut self, request: RunRequest, parallel: bool) {
        if !parallel
            && (self.has_active_jobs() || self.pending.is_some() || self.pending_rebis.is_some())
        {
            let label = format!("Rebis {}", request.scope.label());
            let id = self.register_rebis_run(&request, RebisRunState::Queued);
            self.queue.push(QueuedWork::Rebis { id, request });
            self.focus_selected_rebis_run();
            let position = self.queue.len();
            if let Some(workspace) = &mut self.rebis {
                workspace.panel_visible = true;
                workspace.runs_visible = true;
                workspace.message =
                    format!("{label} queued · {position} in line — runs when the working ends");
            }
            self.push_line(Line::from(vec![
                Span::styled(
                    "⧗ queued ",
                    Style::new().fg(C_OX()).add_modifier(Modifier::BOLD),
                ),
                Span::styled(label, Style::new().fg(C_BONE())),
                Span::styled(format!("   ({position} in line)"), Style::new().fg(C_ASH())),
            ]));
            return;
        }

        let id = self.register_rebis_run_with_mode(&request, RebisRunState::Queued, parallel);
        self.gate_rebis_run(id, request, parallel);
    }

    /// Capture or release the terminal mouse. Captured drags select and copy
    /// inside one pane; released drags use the terminal's row-oriented selection.
    fn set_mouse_capture(&mut self, on: bool) {
        self.mouse_captured = on;
        self.text_selection = None;
        // Under test there is no terminal — writing the control sequences would
        // leak escape codes into the test runner's output.
        #[cfg(not(test))]
        {
            if on {
                let _ = execute!(io::stdout(), EnableMouseCapture);
            } else {
                let _ = execute!(io::stdout(), DisableMouseCapture);
            }
        }
        if on {
            self.note("mouse captured — drag copies within one pane · /mouse off uses raw terminal selection");
        } else {
            self.note(
                "mouse released — raw terminal selection may cross panes · /mouse on clips drags",
            );
        }
    }

    /// Scroll the transcript up by `n` lines (into the backlog); stops following.
    fn scroll_up(&mut self, n: u16) {
        self.follow = false;
        self.scroll = self.scroll.saturating_sub(n);
    }

    /// Scroll down by `n`; re-follows the live tail once it reaches the bottom.
    fn scroll_down(&mut self, n: u16) {
        let max = self.content_rows.saturating_sub(self.view_h);
        self.scroll = (self.scroll + n).min(max);
        if self.scroll >= max {
            self.follow = true;
        }
    }

    // ── durable chat sessions ───────────────────────────────────────────────

    /// Write the conversation out. Called on every exit path, and after each
    /// completed turn so a hard kill loses at most the turn in flight.
    /// Silent on failure: a session that cannot be saved must never take the
    /// app down or interrupt the reader.
    fn save_session(&mut self) {
        self.flush_reply();
        if self.session.is_empty() {
            return;
        }
        let _ = crate::sessions::Store::default_store().save(&self.session);
    }

    /// Fold whatever the running job streamed into one model turn.
    fn flush_reply(&mut self) {
        let reply = std::mem::take(&mut self.session_reply);
        if !reply.trim().is_empty() {
            self.session
                .push(crate::sessions::Role::Model, reply.trim_end());
        }
    }

    /// Record a line of model output for the session (plain text; the styling
    /// is presentation and is not persisted).
    fn record_reply_line(&mut self, text: &str) {
        if self.session_reply.len() < 200_000 {
            self.session_reply.push_str(text);
            self.session_reply.push('\n');
        }
    }

    /// `/chat sessions` — list what can be resumed.
    fn list_sessions(&mut self) {
        let store = crate::sessions::Store::default_store();
        let list = store.list();
        if list.is_empty() {
            self.push_line(Line::from("no saved sessions yet"));
            return;
        }
        self.push_line(Line::from(vec![Span::styled(
            "sessions",
            Style::default().fg(C_RED()).add_modifier(Modifier::BOLD),
        )]));
        for (i, s) in list.iter().enumerate().take(30) {
            self.push_line(Line::from(format!(
                "  {:>2}  {}  {:>3} turns  {}",
                i + 1,
                s.id,
                s.turns,
                s.title
            )));
        }
        self.push_line(Line::from("resume with /chat resume [N | id]"));
    }

    /// `/chat resume [what]` — reopen a stored conversation.
    fn resume_session(&mut self, what: &str) {
        // Keep the current conversation before replacing it.
        self.save_session();
        let store = crate::sessions::Store::default_store();
        let Some(found) = store.resolve(what) else {
            let msg = if what.trim().is_empty() {
                "no saved sessions to resume".to_string()
            } else {
                format!("no session matching '{}'", what.trim())
            };
            self.push_line(Line::from(msg));
            return;
        };
        match store.load(&found.id) {
            Ok(session) => {
                self.transcript.clear();
                self.open_fold = None;
                self.fold_depth = 0;
                self.sel_fold = None;
                self.push_line(Line::from(vec![Span::styled(
                    format!("resumed {}  ({})", found.id, session.title()),
                    Style::default().fg(C_RED()).add_modifier(Modifier::BOLD),
                )]));
                for turn in &session.turns {
                    let (tag, colour) = match turn.role {
                        crate::sessions::Role::User => ("you", C_RED()),
                        crate::sessions::Role::Model => ("model", C_ASH()),
                    };
                    for (i, line) in turn.text.lines().enumerate() {
                        let prefix = if i == 0 { tag } else { "" };
                        self.push_line(Line::from(vec![
                            Span::styled(format!("{prefix:<7}"), Style::default().fg(colour)),
                            Span::raw(line.to_string()),
                        ]));
                    }
                }
                self.session = session;
                self.session_reply.clear();
                self.follow = true;
            }
            Err(error) => self.push_line(Line::from(format!("could not resume: {error}"))),
        }
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_string();
        self.input.clear();
        self.cursor = 0;
        self.hist_nav = None;
        if line.is_empty() {
            return;
        }
        if self.history.last().map(|h| h != &line).unwrap_or(true) {
            self.history.push(line.clone());
            append_history(&line);
        }

        // A working already underway: LOCAL commands (they touch only app state)
        // still run at once; anything that would spawn a job is QUEUED and runs,
        // in order, as jobs finish — type ahead freely.
        if self.has_active_jobs() && !is_local_command(&line) {
            self.queue.push(QueuedWork::Line(line.clone()));
            self.push_line(Line::from(vec![
                Span::styled(
                    "⧗ queued ",
                    Style::new().fg(C_OX()).add_modifier(Modifier::BOLD),
                ),
                Span::styled(line, Style::new().fg(C_BONE())),
                Span::styled(
                    format!(
                        "   ({} in line — runs when the working ends)",
                        self.queue.len()
                    ),
                    Style::new().fg(C_ASH()),
                ),
            ]));
            self.follow = true;
            return;
        }

        // A bare line is something said to the model, so it belongs in the
        // session. Slash-commands are app control, not conversation.
        if !line.starts_with('/') {
            self.flush_reply();
            self.session.push(crate::sessions::Role::User, line.clone());
        }
        self.echo_prompt(&line);
        self.dispatch(&line);
    }

    /// Echo a submitted/dequeued line into the transcript as the prompt row.
    fn echo_prompt(&mut self, line: &str) {
        self.push_line(Line::from(vec![
            Span::styled("✴ ❯ ", red_bold()),
            Span::styled(line.to_string(), Style::new().fg(C_BONE())),
        ]));
        self.follow = true;
    }

    /// Route one input line: bare lines are intents for the agent, slash-lines are
    /// commands. (Shared by direct submission and the queue drain.)
    fn dispatch(&mut self, line: &str) {
        // A bare line (no slash) is an INTENT for the agent — it works the current
        // directory with real tools. Only slash-lines are commands. This is the
        // difference between "do the task" (code) and "cast a one-shot" (/cast).
        let Some(body) = line.strip_prefix('/') else {
            self.request_job(vec![
                "code".into(),
                RAW_CHAT_TASK_ARG.into(),
                line.to_string(),
            ]);
            return;
        };

        let args = shell_split(body);
        let head = args.first().map(|s| s.as_str()).unwrap_or("");
        match head {
            "quit" | "exit" | "q" => self.quit = true,
            "clear" | "cls" => self.clear_transcript(),
            "mouse" | "select" => {
                let capture = match args.get(1).map(|s| s.as_str()) {
                    Some("on" | "capture") => true,
                    Some("off" | "release") => false,
                    _ => !self.mouse_captured,
                };
                self.set_mouse_capture(capture);
            }
            "cd" => self.change_dir(args.get(1).map(|s| s.as_str()).unwrap_or("")),
            "model" | "bind" => self.set_model(&args[1..].join(" ")),
            "config" => match args.get(1).map(String::as_str) {
                None => self.open_config(false),
                Some("restore") if args.len() == 2 => self.open_config(true),
                _ => self.note("usage: /config or /config restore"),
            },
            "chaos" => {
                self.rebis_chaos_mode = match args.get(1).map(String::as_str) {
                    Some("on" | "yes" | "1") => true,
                    Some("off" | "no" | "0") => false,
                    _ => !self.rebis_chaos_mode,
                };
                self.note(if self.rebis_chaos_mode {
                    "CHAOS mode enabled · Rebis prompts use the Kaos Conductor pipeline"
                } else {
                    "DIRECT mode enabled · one tool agent per Rebis prompt"
                });
            }
            "new" | "forget" => {
                // Keep the old conversation before starting a clean one.
                self.save_session();
                self.session = crate::sessions::Session::new(
                    self.model.clone(),
                    self.cwd.display().to_string(),
                );
                self.session_reply.clear();
                self.session_id = gen_uuid();
                self.resumed = false;
                self.rebis_authority = false;
                self.note("a fresh sigil — the adept remembers nothing prior.");
            }
            "theme" => {
                let want = args.get(1).map(String::as_str).unwrap_or("");
                match crate::theme::Mode::parse(want) {
                    Some(mode) => match crate::theme::set_mode(mode) {
                        // Persisted for both this app and `kaos visual`; the
                        // terminal picks it up on restart.
                        Ok(()) => self.note(&format!(
                            "theme {} — restart kaos to repaint the terminal",
                            mode.name()
                        )),
                        Err(error) => self.note(&format!("theme: {error}")),
                    },
                    None => self.note(&format!(
                        "theme is {} · use /theme dark or /theme light",
                        crate::theme::mode().name()
                    )),
                }
            }
            "sessions" => self.list_sessions(),
            "resume" => {
                let what = args.get(1..).map(|r| r.join(" ")).unwrap_or_default();
                self.resume_session(&what);
            }
            "forget-session" => {
                let what = args.get(1..).map(|r| r.join(" ")).unwrap_or_default();
                let store = crate::sessions::Store::default_store();
                match store.resolve(&what) {
                    Some(found) => match store.delete(&found.id) {
                        Ok(()) => self.note(&format!("deleted session {}", found.id)),
                        Err(error) => self.note(&format!("could not delete: {error}")),
                    },
                    None => self.note("no matching session"),
                }
            }
            "rebis" => self.open_rebis(args.get(1).map(String::as_str)),
            "runs" => {
                self.open_rebis(None);
                self.handle_rebis_action(WorkspaceAction::BrowseRuns);
            }
            "sigils" => {
                self.open_rebis(None);
                if let Some(workspace) = &mut self.rebis {
                    workspace.command = if args.len() > 1 {
                        format!("sigils {}", args[1..].join(" "))
                    } else {
                        "sigils".to_string()
                    };
                    workspace.execute_kaos_command();
                }
            }
            _ => self.request_job(args),
        }
    }

    /// Run the next queued message, command, or captured Rebis evaluation.
    fn drain_queue(&mut self) {
        if self.has_active_jobs()
            || self.pending.is_some()
            || self.pending_rebis.is_some()
            || !self.parallel_gate_queue.is_empty()
            || self.queue.is_empty()
        {
            return;
        }
        let work = self.queue.remove(0);
        self.push_line(Line::from(Span::styled(
            format!("⧗ from the queue ({} remain):", self.queue.len()),
            Style::new().fg(C_ASH()),
        )));
        match work {
            QueuedWork::Line(line) => {
                self.echo_prompt(&line);
                self.dispatch(&line);
            }
            QueuedWork::Rebis { id, request } => {
                self.push_line(Line::from(Span::styled(
                    format!("Rebis {}", request.scope.label()),
                    Style::new().fg(C_BONE()),
                )));
                self.gate_rebis_run(id, request, false);
            }
        }
    }

    fn gate_rebis_run(&mut self, id: u64, request: RunRequest, parallel: bool) {
        if self.rebis_authority {
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                run.expanded = true;
                run.output
                    .push("permission  granted · remembered for this sigil".to_string());
            }
            self.start_rebis_run(id, request, parallel);
            return;
        }
        if self.pending_rebis.is_some() {
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                run.state = RebisRunState::Queued;
                run.expanded = true;
                run.output = vec![
                    "permission  waiting behind an earlier Rebis authority decision".to_string(),
                ];
            }
            self.parallel_gate_queue.push(PendingRebisRun {
                id,
                request,
                parallel,
            });
            let permission_count = self.parallel_gate_queue.len();
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.panel_visible = true;
                workspace.runs_visible = true;
                workspace.graph_focus = true;
                workspace.message = format!(
                    "parallel Rebis run waiting for authority · {} permission request(s) queued",
                    permission_count
                );
            }
            self.focus_selected_rebis_run();
            return;
        }
        let scope = request.scope;
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            run.state = RebisRunState::AwaitingPermission;
            run.expanded = true;
            run.output = vec![
                format!(
                    "permission  Rebis {} agents may read, edit, and write files and run commands",
                    scope.label()
                ),
                "permission  agree to these changes? [y] once · [a] remember for this sigil · [n/Esc] deny"
                    .to_string(),
            ];
        }
        self.pending_rebis = Some(PendingRebisRun {
            id,
            request,
            parallel,
        });
        let message = format!(
            "Rebis {} agents may read/edit/write/run commands · y once · a remember for this sigil · n deny",
            scope.label()
        );
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.panel_visible = true;
            workspace.runs_visible = true;
            workspace.graph_focus = true;
            workspace.message = message;
        }
        self.focus_selected_rebis_run();
    }

    fn approve_rebis_authority(&mut self, remember: bool) {
        let Some(pending) = self.pending_rebis.take() else {
            return;
        };
        if remember {
            self.rebis_authority = true;
        }
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == pending.id) {
            run.output.push(if remember {
                "permission  granted · remembered for this sigil".to_string()
            } else {
                "permission  granted · this run only".to_string()
            });
        }
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.message = if remember {
                "Rebis agent authority remembered for this sigil".to_string()
            } else {
                "Rebis agent authority granted for this run".to_string()
            };
        }
        self.start_rebis_run(pending.id, pending.request, pending.parallel);
        self.advance_rebis_gate_queue();
    }

    fn deny_rebis_authority(&mut self) {
        let Some(pending) = self.pending_rebis.take() else {
            return;
        };
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == pending.id) {
            finish_rebis_run_clock(run);
            run.state = RebisRunState::Cancelled;
            run.expanded = true;
            run.output
                .push("permission  denied · no agent or tool was launched".to_string());
        }
        if let Some(workspace) = self.background_rebis_workspace() {
            workspace.message = "Rebis run denied · no agent or tool was launched".to_string();
        }
        self.advance_rebis_gate_queue();
        self.drain_queue();
    }

    fn advance_rebis_gate_queue(&mut self) {
        if self.pending_rebis.is_some() || self.parallel_gate_queue.is_empty() {
            return;
        }
        if self.rebis_authority {
            let queued = std::mem::take(&mut self.parallel_gate_queue);
            for pending in queued {
                if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == pending.id) {
                    run.output
                        .push("permission  granted · remembered for this sigil".to_string());
                }
                self.start_rebis_run(pending.id, pending.request, pending.parallel);
            }
        } else {
            let pending = self.parallel_gate_queue.remove(0);
            self.gate_rebis_run(pending.id, pending.request, pending.parallel);
        }
    }

    /// Start a Rebis request only after it reaches the head of the shared FIFO.
    /// The request is already a source/input snapshot, so execution is insulated
    /// from edits made while it waited.
    fn start_rebis_run(&mut self, id: u64, request: RunRequest, parallel: bool) {
        let scope = request.scope;
        let chaos = self
            .rebis_runs
            .iter()
            .find(|run| run.id == id)
            .is_some_and(|run| run.chaos);
        if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
            let restarting = run.started_at.is_some();
            if !restarting {
                if let Some(saved_checkpoint) = run.saved_checkpoint_path.clone() {
                    // Plain `/run` is fresh even for a sigil with an older
                    // unfinished snapshot. Once launched, however, write new
                    // prompt boundaries directly to the durable journal.
                    if let Some(saved_run) = &run.saved_run_path {
                        let _ = std::fs::remove_file(saved_run);
                    }
                    let _ = std::fs::remove_file(&saved_checkpoint);
                    run.checkpoint_path = saved_checkpoint;
                }
            }
            run.state = RebisRunState::Running;
            if !restarting {
                run.started_at = Some(Instant::now());
            }
            run.elapsed = None;
            run.expanded = true;
            resume_rebis_run_clock(run);
            run.output.push(if restarting {
                "checkpoint  reconstructing interpreter · completed prompts replay locally"
                    .to_string()
            } else if chaos {
                "mode        CHAOS · Kaos tool-agent expansion enabled".to_string()
            } else {
                "mode        DIRECT · one tool agent per prompt".to_string()
            });
        }
        let _ = self.persist_rebis_run(id);
        let mut args = vec!["rebis".to_string(), "run".to_string()];
        args.push("--allow-tools".to_string());
        if chaos {
            args.push("--chaos".to_string());
        }
        args.push(request.source);
        let launched = self.spawn_job_with_input(
            args,
            Some(request.input),
            Some(id),
            Some(chaos),
            parallel,
            None,
        );
        let parallel_count = self.parallel_jobs.len();
        if let Some(workspace) = self.background_rebis_workspace() {
            if launched {
                if parallel {
                    workspace.panel_visible = true;
                    workspace.runs_visible = true;
                    workspace.graph_focus = true;
                    workspace.message = format!(
                        "parallel Rebis {} started · {} concurrent run(s)",
                        scope.label(),
                        parallel_count
                    );
                } else {
                    workspace.begin_run(scope);
                }
            } else {
                workspace.message = format!("could not launch Rebis {}", scope.label());
            }
        }
        if !launched {
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                pause_rebis_run_clock(run, "could not launch replacement child");
                run.output.push(
                    "paused      ⏸ child launch failed · p retries from the last completed prompt"
                        .to_string(),
                );
            }
        }
        self.focus_selected_rebis_run();
    }

    /// Gate a job: coding tasks (`code`/`forge`) may run shell as the adept, so the
    /// first one raises the yolo question and is held in `pending` until answered.
    /// Everything else runs at once.
    fn request_job(&mut self, args: Vec<String>) {
        if self.has_active_jobs() {
            self.note("a working is already underway — ^C stops it.");
            return;
        }
        let needs_authority = matches!(
            args.first().map(|s| s.as_str()),
            Some("code") | Some("forge")
        );
        if needs_authority && self.yolo.is_none() {
            self.pending = Some(args);
            self.push_line(Line::from(Span::styled(
                "  ⚠ the adept will act on these files. grant full authority?",
                red_bold(),
            )));
            self.push_line(Line::from(Span::styled(
                "     [y] unbound — it may run shell (tests, git, anything)     [n] edits only",
                Style::new().fg(C_ASH()),
            )));
            self.follow = true;
        } else {
            self.spawn_job(args);
        }
    }

    /// Answer the yolo question: remember the choice and release the held job.
    fn decide_yolo(&mut self, yolo: bool) {
        self.yolo = Some(yolo);
        self.note(if yolo {
            "bound: unbound — the adept may run shell (change with a restart)"
        } else {
            "bound: edits only — the adept edits files but runs no shell"
        });
        if let Some(args) = self.pending.take() {
            self.spawn_job(args);
        }
    }

    fn history_prev(&mut self) {
        if self.history.is_empty() {
            return;
        }
        let idx = match self.hist_nav {
            None => {
                self.stash = self.input.clone();
                self.history.len() - 1
            }
            Some(0) => 0,
            Some(i) => i - 1,
        };
        self.hist_nav = Some(idx);
        self.input = self.history[idx].clone();
        self.cursor = self.char_len();
    }

    fn history_next(&mut self) {
        if let Some(i) = self.hist_nav {
            if i + 1 < self.history.len() {
                self.hist_nav = Some(i + 1);
                self.input = self.history[i + 1].clone();
            } else {
                self.hist_nav = None;
                self.input = self.stash.clone();
            }
            self.cursor = self.char_len();
        }
    }

    // ── commands ───────────────────────────────────────────────────

    /// Does this job actually create/resume a claude conversation? Mirrors the
    /// dispatch in `main.rs code_cmd`: only the single-adept (k=1) path on a
    /// claude-cli mind reaches `run_claude_agent`, which passes KAOS_SESSION to
    /// `claude --session-id`/`--resume`. A conclave (`xK`, or a `--` gate which
    /// defaults k to 3) drives raw completions instead, and a simulated mind
    /// defaults to the claude CLI there — so it counts too.
    fn job_creates_claude_session(&self, args: &[String]) -> bool {
        if args.first().map(|s| s.as_str()) != Some("code") {
            return false;
        }
        let kind = crate::provider::Spec::parse(&self.model).kind;
        if !matches!(
            kind,
            crate::provider::Kind::ClaudeCli | crate::provider::Kind::Simulated
        ) {
            return false;
        }
        if args.get(1).is_some_and(|arg| arg == RAW_CHAT_TASK_ARG) {
            // Literal chat text bypasses `/code`'s `[dir] [xK] task -- gate`
            // grammar in the child, so it is always the single-agent session
            // path when non-empty—even if the pasted code contains ` -- `.
            return args.get(2).is_some_and(|task| !task.trim().is_empty());
        }
        // Reassemble the arg string exactly as one-shot main() does, then peel the
        // `-- <gate>` and leading [dir]/[xK] tokens the same way code_cmd does.
        let arg = args[1..].join(" ");
        let (head, gated) = match arg.split_once(" -- ") {
            Some((h, v)) if !v.trim().is_empty() => (h.trim().to_string(), true),
            _ => (arg.trim().to_string(), false),
        };
        let mut k: Option<usize> = None;
        let mut rest = head.as_str();
        loop {
            let mut it = rest.splitn(2, char::is_whitespace);
            let first = it.next().unwrap_or("");
            let after = it.next().unwrap_or("").trim();
            if let Some(n) = first
                .strip_prefix('x')
                .and_then(|d| d.parse::<usize>().ok())
            {
                k = Some(n.max(1));
                rest = after;
            } else if self.cwd.join(first).is_dir() && !after.is_empty() {
                rest = after;
            } else {
                break;
            }
        }
        // An empty task never reaches the agent (code_cmd refuses it).
        !rest.trim().is_empty() && k.unwrap_or(if gated { 3 } else { 1 }) == 1
    }

    fn spawn_job(&mut self, args: Vec<String>) {
        let _ = self.spawn_job_with_input(args, None, None, None, false, None);
    }

    fn spawn_job_with_input(
        &mut self,
        args: Vec<String>,
        input: Option<String>,
        rebis_run_id: Option<u64>,
        authority_override: Option<bool>,
        parallel: bool,
        working_dir: Option<PathBuf>,
    ) -> bool {
        let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("kaos"));
        let mut cmd = Command::new(exe);
        let claude_session = self.job_creates_claude_session(&args);
        let transport = prepare_child_transport(args, input);
        let session_id = if parallel {
            gen_uuid()
        } else {
            self.session_id.clone()
        };
        cmd.args(&transport.args)
            .current_dir(working_dir.as_deref().unwrap_or(&self.cwd))
            .env("KAOS_MODEL", &self.model)
            .env(
                "KAOS_CLAUDE_YOLO",
                if authority_override.unwrap_or(self.yolo == Some(true)) {
                    "1"
                } else {
                    "0"
                },
            )
            .env("KAOS_SESSION", session_id)
            .env(
                "KAOS_RESUME",
                if !parallel && self.resumed { "1" } else { "0" },
            )
            // Tell the child to emit fold markers so its detailed trace renders as
            // collapsible groups here instead of a flat wall of output.
            .env("KAOS_FOLD", "1")
            .stdin(if transport.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if transport.args.first().is_some_and(|arg| arg == "code") {
            // A coding job launched by the interactive app is the `/chat` mind
            // (including an explicit `/code`). Give it Kaos's compiled Rebis
            // cookbook so it can explain, repair, and author the language.
            cmd.env("KAOS_REBIS_CONTEXT", "1");
        }
        if transport.raw_chat_task {
            cmd.env(RAW_CHAT_TASK_ENV, "1");
        }
        let owns_process_group = rebis_run_id.is_some() && cfg!(unix);
        if rebis_run_id.is_some() {
            cmd.env(crate::pause::ENABLE_ENV, "1");
            if let Some(checkpoint_path) = rebis_run_id.and_then(|id| {
                self.rebis_runs
                    .iter()
                    .find(|run| run.id == id)
                    .map(|run| run.checkpoint_path.clone())
            }) {
                cmd.env(crate::rebis_checkpoint::PATH_ENV, checkpoint_path);
            }
            if let Some(directive_path) = rebis_run_id.and_then(|id| {
                self.rebis_runs
                    .iter()
                    .find(|run| run.id == id)
                    .map(|run| run.directive_path.clone())
            }) {
                cmd.env(crate::rebis_supervisor::DIRECTIVE_PATH_ENV, directive_path);
            }
            if owns_process_group {
                cmd.env(crate::pause::PROCESS_GROUP_ENV, "1");
            }
        }
        #[cfg(unix)]
        if owns_process_group {
            cmd.process_group(0);
        }
        match cmd.spawn() {
            Ok(mut child) => {
                if let (Some(input), Some(mut stdin)) = (transport.stdin, child.stdin.take()) {
                    thread::spawn(move || {
                        let _ = stdin.write_all(input.as_bytes());
                    });
                }
                let Some(stdout) = child.stdout.take() else {
                    let _ = child.kill();
                    self.note("could not capture child stdout");
                    return false;
                };
                let Some(stderr) = child.stderr.take() else {
                    let _ = child.kill();
                    self.note("could not capture child stderr");
                    return false;
                };
                let (tx, rx) = mpsc::channel();
                spawn_reader(stdout, tx.clone(), false);
                spawn_reader(stderr, tx.clone(), true);
                let child = Arc::new(Mutex::new(child));
                {
                    let child = child.clone();
                    thread::spawn(move || {
                        let code = loop {
                            {
                                let mut c = child
                                    .lock()
                                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                                if let Ok(Some(status)) = c.try_wait() {
                                    break status.code().unwrap_or(-1);
                                }
                            }
                            thread::sleep(Duration::from_millis(30));
                        };
                        let _ = tx.send(Msg::Done(code));
                    });
                }
                let job = Job {
                    child,
                    rx,
                    label: transport.label,
                    claude_session,
                    rebis_run_id,
                    owns_process_group,
                };
                if parallel {
                    self.parallel_jobs.push(job);
                } else {
                    self.job = Some(job);
                    self.job_start = Some(Instant::now());
                }
                self.activity = "starting…".to_string();
                true
            }
            Err(e) => {
                self.note(&format!("could not launch: {e}"));
                false
            }
        }
    }

    fn quit_from_ctrl_c(&mut self) {
        // Dropping `Child` does not terminate it. Kill active work first so an
        // unconditional UI exit cannot leave an agent running in the background.
        let _ = self.cancel_job();
        self.quit = true;
    }

    /// Rebis-workspace ^C is STOP-first, like the chat screen: it cancels a run
    /// (or queued/pending work) in flight and stays in the app, so a running
    /// agent is never confused with a request to quit. Only when nothing is in
    /// flight does ^C move toward exiting — and then only after a confirmation.
    fn workspace_ctrl_c(&mut self) {
        if self.has_active_jobs()
            || !self.queue.is_empty()
            || self.pending_rebis.is_some()
            || !self.parallel_gate_queue.is_empty()
        {
            self.cancel_all_work("^C");
            self.confirm_quit = false;
            if let Some(workspace) = self.rebis.as_mut() {
                workspace.message = "run stopped · ^C twice to quit Kaos".to_string();
            }
            return;
        }
        self.request_quit();
    }

    /// Ask before quitting: the first idle ^C arms the confirmation and shows a
    /// prompt; the next ^C actually exits. Any other key disarms it (see
    /// [`Self::disarm_quit`]).
    fn request_quit(&mut self) {
        if self.confirm_quit {
            self.quit_from_ctrl_c();
            return;
        }
        self.confirm_quit = true;
        let prompt = "quit Kaos? press ^C again to confirm · any other key stays";
        if let Some(workspace) = self.rebis.as_mut() {
            workspace.message = prompt.to_string();
        } else {
            self.note(prompt);
        }
    }

    /// Cancel a pending quit confirmation because the user pressed something
    /// other than ^C. Returns whether a confirmation was actually disarmed.
    fn disarm_quit(&mut self) -> bool {
        if !self.confirm_quit {
            return false;
        }
        self.confirm_quit = false;
        if let Some(workspace) = self.rebis.as_mut() {
            workspace.message = "quit cancelled".to_string();
        } else {
            self.note("quit cancelled");
        }
        true
    }

    /// Chat-screen ^C is STOP, not quit: it cancels whatever is in flight —
    /// active and queued work first, then a pending permission question, then
    /// a typed prompt — and exits Kaos only when the chat is already idle.
    fn chat_ctrl_c(&mut self) {
        if self.has_active_jobs()
            || !self.queue.is_empty()
            || self.pending_rebis.is_some()
            || !self.parallel_gate_queue.is_empty()
        {
            self.cancel_all_work("^C");
            return;
        }
        if self.pending.take().is_some() {
            self.note("cancelled.");
            return;
        }
        if !self.input.is_empty() {
            self.input.clear();
            self.cursor = 0;
            self.hist_nav = None;
            self.command_choice = 0;
            return;
        }
        self.request_quit();
    }

    fn cancel_job(&mut self) -> usize {
        self.cancel_all_work("Kaos exited")
    }

    fn cancel_all_work(&mut self, reason: &str) -> usize {
        // A saved sigil treats app cancellation as "left here": snapshot the
        // last completed prompt before any process or temporary file is torn
        // down. Explicit removal of the saved sidecars is a separate action.
        let durable_ids = self
            .rebis_runs
            .iter()
            .filter(|run| run.state == RebisRunState::Running && run.saved_run_path.is_some())
            .map(|run| run.id)
            .collect::<Vec<_>>();
        for id in durable_ids {
            let _ = self.persist_rebis_run(id);
        }
        let mut cancelled_any = false;
        let mut scattered = 0;
        if let Some(job) = self.job.take() {
            cancelled_any = true;
            let active_rebis = job.rebis_run_id;
            let mut child = job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.owns_process_group {
                let _ = signal_process(child.id(), true, "-KILL");
            } else {
                let _ = child.kill();
            }
            drop(child);
            self.job_start = None;
            self.activity.clear();
            // A cancel can land mid-stream with a fold still open; close it (as the
            // Done path does) or every later line vanishes into the dead fold's body.
            self.open_fold = None;
            self.fold_depth = 0;
            if let Some(id) = active_rebis {
                if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                    finish_rebis_run_clock(run);
                    run.state = RebisRunState::Cancelled;
                    run.output.push(format!("cancelled   {reason}"));
                    if run.saved_checkpoint_path.as_ref() != Some(&run.checkpoint_path) {
                        let _ = std::fs::remove_file(&run.checkpoint_path);
                    }
                }
            }
        }

        if let Some(turn) = self.sigil_chat_job.take() {
            cancelled_any = true;
            let mut child = turn
                .job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if turn.job.owns_process_group {
                let _ = signal_process(child.id(), true, "-KILL");
            } else {
                let _ = child.kill();
            }
            drop(child);
            if let Some(workspace) = self.background_rebis_workspace() {
                workspace.push_sigil_chat_line(format!("system  god agent cancelled · {reason}"));
                workspace.set_sigil_chat_busy(false);
            }
        }

        // Parallel children are independent jobs, but Ctrl-C is the global exit
        // gesture: every one must be terminated before the terminal is restored.
        for job in std::mem::take(&mut self.parallel_jobs) {
            cancelled_any = true;
            let mut child = job
                .child
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if job.owns_process_group {
                let _ = signal_process(child.id(), true, "-KILL");
            } else {
                let _ = child.kill();
            }
            drop(child);
            if let Some(id) = job.rebis_run_id {
                if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == id) {
                    finish_rebis_run_clock(run);
                    run.state = RebisRunState::Cancelled;
                    run.output.push(format!("cancelled   {reason}"));
                    if run.saved_checkpoint_path.as_ref() != Some(&run.checkpoint_path) {
                        let _ = std::fs::remove_file(&run.checkpoint_path);
                    }
                }
            }
        }

        if let Some(pending) = self.pending_rebis.take() {
            cancelled_any = true;
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == pending.id) {
                finish_rebis_run_clock(run);
                run.state = RebisRunState::Cancelled;
                run.output
                    .push(format!("permission  cancelled \u{2014} {reason}"));
            }
        }
        for pending in std::mem::take(&mut self.parallel_gate_queue) {
            cancelled_any = true;
            if let Some(run) = self.rebis_runs.iter_mut().find(|run| run.id == pending.id) {
                finish_rebis_run_clock(run);
                run.state = RebisRunState::Cancelled;
                run.output
                    .push(format!("permission  cancelled \u{2014} {reason}"));
            }
        }

        // ^C means STOP — silently launching queued intents next would betray
        // that, so scatter the shared FIFO whether the active lane was serial or
        // parallel.
        if !self.queue.is_empty() {
            scattered = self.queue.len();
            let queued_ids = self
                .queue
                .iter()
                .filter_map(|work| match work {
                    QueuedWork::Rebis { id, .. } => Some(*id),
                    QueuedWork::Line(_) => None,
                })
                .collect::<Vec<_>>();
            self.queue.clear();
            self.rebis_runs.retain(|run| !queued_ids.contains(&run.id));
            self.clamp_rebis_run_choice();
        }

        if cancelled_any {
            self.note("cancelled all active work.");
        }
        if scattered > 0 {
            self.note(&format!("{scattered} queued item(s) scattered with it."));
        }
        scattered
    }

    fn change_dir(&mut self, path: &str) {
        if path.is_empty() {
            self.note(&format!("cwd: {}", self.cwd.display()));
            return;
        }
        let target = if path.starts_with('/') {
            PathBuf::from(path)
        } else if let Some(rest) = path.strip_prefix("~/") {
            dirs_home().join(rest)
        } else {
            self.cwd.join(path)
        };
        match target.canonicalize() {
            Ok(p) if p.is_dir() => {
                self.cwd = p;
                self.note(&format!("cwd → {}", self.cwd.display()));
            }
            _ => self.note(&format!("not a directory: {path}")),
        }
    }

    fn set_model(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            let spec = crate::provider::Spec::parse(&self.model);
            let message = format!("model bound: {}", spec.label());
            self.note(&format!("bound: {}   (claude[:sonnet|opus|haiku|fable] · openai[:model] · anthropic[:model] · openrouter[:vendor/model] · ollama:m · sim — /models lists all)", spec.label()));
            if let Some(workspace) = self.rebis.as_mut().or(self.suspended_rebis.as_mut()) {
                workspace.message = message;
            }
            return;
        }
        // "openai gpt-4o" → "openai:gpt-4o" so the parser reads the model.
        let spec = crate::provider::Spec::parse(&arg.replacen(' ', ":", 1));
        let warn = spec.readiness().err();
        let canonical = spec.canonical();
        self.model = canonical.clone();
        self.note(&format!("bound: {}", spec.label()));
        // Unit tests exercise the selector without touching the user's real
        // config. The production path writes the same canonical value passed to
        // every child process, so the next Kaos session starts on this model.
        let persistence = if cfg!(not(test)) {
            crate::config::set_value("KAOS_MODEL", &canonical).map(|_| ())
        } else {
            Ok(())
        };
        let status = match persistence {
            Ok(()) => {
                self.note("model remembered in the Kaos config");
                format!("model bound: {canonical} · remembered")
            }
            Err(error) => {
                self.note(&format!("could not remember model: {error}"));
                format!("model bound: {canonical} · could not remember: {error}")
            }
        };
        if let Some(workspace) = self.rebis.as_mut().or(self.suspended_rebis.as_mut()) {
            workspace.message = status;
        }
        if let Some(w) = warn {
            self.note(&format!(
                "✴ but {w} — the mind will not answer until it is set"
            ));
        }
    }

    fn note(&mut self, s: &str) {
        self.push_line(Line::from(Span::styled(
            format!("  {s}"),
            Style::new().fg(C_ASH()),
        )));
        self.follow = true;
    }

    /// Clear the transcript and all fold state, and re-follow the live tail.
    fn clear_transcript(&mut self) {
        self.transcript.clear();
        self.open_fold = None;
        self.fold_depth = 0;
        self.sel_fold = None;
        self.follow = true;
    }

    // ── small helpers ──────────────────────────────────────────────

    fn char_len(&self) -> usize {
        self.input.chars().count()
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.input
            .char_indices()
            .nth(char_idx)
            .map(|(b, _)| b)
            .unwrap_or(self.input.len())
    }
}

fn mouse_over_rebis_graph(
    workspace: &RebisWorkspace,
    column: u16,
    row: u16,
    (width, height): (u16, u16),
) -> bool {
    if !workspace.panel_visible {
        return false;
    }
    // Prefer the rectangle actually rendered. Percentage reconstruction can
    // disagree by a row/column after terminal resizing or narrow-layout
    // rounding, causing wheel events to reach the wrong pane.
    if let Some((x, y, panel_width, panel_height)) = workspace.panel_inner {
        let left = x.saturating_sub(1);
        let top = y.saturating_sub(1);
        let right = x.saturating_add(panel_width).saturating_add(1);
        let bottom = y.saturating_add(panel_height).saturating_add(1);
        return column >= left && column < right && row >= top && row < bottom;
    }
    if width >= 78 {
        column >= width.saturating_mul(56) / 100
    } else {
        let content_height = height.saturating_sub(3);
        row > content_height.saturating_mul(55) / 100
    }
}

fn point_in_rect(position: Position, area: Rect) -> bool {
    position.x >= area.x
        && position.x < area.right()
        && position.y >= area.y
        && position.y < area.bottom()
}

fn clamp_to_rect(position: Position, area: Rect) -> Position {
    Position {
        x: position.x.clamp(area.x, area.right().saturating_sub(1)),
        y: position.y.clamp(area.y, area.bottom().saturating_sub(1)),
    }
}

fn snapshot_text_pane(buffer: &Buffer, region: TextPaneRegion) -> TextPane {
    let TextPaneRegion {
        kind,
        area,
        content_left,
    } = region;
    let rows = (area.y..area.bottom())
        .map(|y| {
            (area.x..area.right())
                .map(|x| {
                    buffer
                        .cell((x, y))
                        .map(|cell| cell.symbol().to_string())
                        .unwrap_or_default()
                })
                .collect()
        })
        .collect();
    TextPane {
        kind,
        area,
        content_left,
        rows,
    }
}

fn ordered_selection(selection: &PaneSelection) -> (Position, Position) {
    if (selection.anchor.y, selection.anchor.x) <= (selection.head.y, selection.head.x) {
        (selection.anchor, selection.head)
    } else {
        (selection.head, selection.anchor)
    }
}

fn selection_span_on_row(area: Rect, selection: &PaneSelection, y: u16) -> Option<(u16, u16)> {
    let (start, end) = ordered_selection(selection);
    if y < start.y || y > end.y {
        return None;
    }
    let left = if y == start.y { start.x } else { area.x };
    let right = if y == end.y {
        end.x
    } else {
        area.right().saturating_sub(1)
    };
    Some((left, right))
}

fn selected_pane_text(pane: &TextPane, selection: &PaneSelection) -> String {
    let (start, end) = ordered_selection(selection);
    (start.y..=end.y)
        .filter_map(|y| {
            let (left, right) = selection_span_on_row(pane.content_area(), selection, y)?;
            let row = pane.rows.get((y - pane.area.y) as usize)?;
            let mut text = String::new();
            for x in left..=right {
                if let Some(symbol) = row.get((x - pane.area.x) as usize) {
                    text.push_str(symbol);
                }
            }
            Some(text.trim_end().to_string())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn highlight_pane_selection(buffer: &mut Buffer, area: Rect, selection: &PaneSelection) {
    if !selection.dragged {
        return;
    }
    let (start, end) = ordered_selection(selection);
    for y in start.y..=end.y {
        let Some((left, right)) = selection_span_on_row(area, selection, y) else {
            continue;
        };
        for x in left..=right {
            if let Some(cell) = buffer.cell_mut((x, y)) {
                cell.set_style(
                    Style::new()
                        .fg(Color::Black)
                        .bg(C_BLUE())
                        .add_modifier(Modifier::BOLD),
                );
            }
        }
    }
}

fn copy_to_terminal_clipboard(text: &str) -> io::Result<()> {
    #[cfg(not(test))]
    {
        let encoded = base64::engine::general_purpose::STANDARD.encode(text);
        let mut stdout = io::stdout();
        write!(stdout, "\x1b]52;c;{encoded}\x07")?;
        stdout.flush()
    }
    #[cfg(test)]
    {
        let _ = text;
        Ok(())
    }
}

fn signal_process(pid: u32, process_group: bool, signal: &str) -> bool {
    let target = if process_group {
        format!("-{pid}")
    } else {
        pid.to_string()
    };
    Command::new("kill")
        .arg(signal)
        .arg("--")
        .arg(target)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn pause_rebis_run_clock(run: &mut RebisRunEntry, reason: &str) {
    if !run.paused {
        run.paused = true;
        run.paused_at = Some(Instant::now());
    }
    run.pause_reason = Some(reason.to_string());
}

fn resume_rebis_run_clock(run: &mut RebisRunEntry) {
    if let Some(paused_at) = run.paused_at.take() {
        run.paused_total = run.paused_total.saturating_add(paused_at.elapsed());
    }
    run.paused = false;
    run.pause_reason = None;
}

fn active_rebis_run_elapsed(run: &RebisRunEntry) -> Duration {
    let origin = run.started_at.unwrap_or(run.queued_at);
    let current_pause = run.paused_at.map_or(Duration::ZERO, |at| at.elapsed());
    origin
        .elapsed()
        .saturating_sub(run.paused_total.saturating_add(current_pause))
}

fn finish_rebis_run_clock(run: &mut RebisRunEntry) {
    run.elapsed = Some(active_rebis_run_elapsed(run));
    resume_rebis_run_clock(run);
}

fn format_run_duration(duration: Duration) -> String {
    let seconds = duration.as_secs();
    if seconds < 60 {
        format!("{seconds}.{}s", duration.subsec_millis() / 100)
    } else {
        format!("{}m {:02}s", seconds / 60, seconds % 60)
    }
}

fn rebis_run_timer(run: &RebisRunEntry) -> String {
    if run.paused && run.state == RebisRunState::Running {
        return "PAUSED".to_string();
    }
    match run.state {
        RebisRunState::AwaitingPermission | RebisRunState::Queued => {
            format!("WAIT {}", format_run_duration(run.queued_at.elapsed()))
        }
        RebisRunState::Running => format!(
            "TIME {}",
            format_run_duration(active_rebis_run_elapsed(run))
        ),
        RebisRunState::Complete | RebisRunState::Cancelled => {
            let duration = run
                .elapsed
                .unwrap_or_else(|| run.started_at.unwrap_or(run.queued_at).elapsed());
            let label = if run.started_at.is_some() {
                "TIME"
            } else {
                "WAIT"
            };
            format!("{label} {}", format_run_duration(duration))
        }
    }
}

fn rebis_run_tree_rows(runs: &[RebisRunView]) -> Vec<RebisRunTreeRow> {
    let mut rows = Vec::new();
    for (index, run) in runs.iter().enumerate() {
        rows.push(RebisRunTreeRow::Header(index));
        if run.entry.expanded {
            if run.entry.output.is_empty() {
                let text = match run.entry.state {
                    RebisRunState::AwaitingPermission => "(waiting for agent authority)",
                    RebisRunState::Queued => "(waiting — no stream output yet)",
                    RebisRunState::Running => "(waiting for stream output…)",
                    _ => "(no text stream output)",
                };
                rows.push(RebisRunTreeRow::Output {
                    run: index,
                    depth: 0,
                    text: text.to_string(),
                });
            } else {
                let mut depth = 0usize;
                for line in &run.entry.output {
                    match crate::fold::classify(line) {
                        crate::fold::Marker::Open(title) => {
                            let title = title.trim();
                            let kind = if title.starts_with("Rebis agent ") {
                                RebisRunSectionKind::Agent
                            } else if title.starts_with("model turn ") {
                                RebisRunSectionKind::Model
                            } else {
                                RebisRunSectionKind::Step
                            };
                            rows.push(RebisRunTreeRow::Section {
                                run: index,
                                depth,
                                kind,
                                title: title.to_string(),
                            });
                            depth += 1;
                        }
                        crate::fold::Marker::Close => depth = depth.saturating_sub(1),
                        crate::fold::Marker::Line(text) => {
                            rows.push(RebisRunTreeRow::Output {
                                run: index,
                                depth,
                                text: text.to_string(),
                            });
                        }
                    }
                }
            }
        }
    }
    rows
}

/// Expand logical run-output lines into pane-width display rows. The durable
/// history remains byte-for-byte intact; only this projection wraps it, so code
/// and model text continue below the right edge instead of disappearing there.
fn rebis_run_display_rows(runs: &[RebisRunView], panel_width: usize) -> Vec<RebisRunTreeRow> {
    let content_width = panel_width.saturating_sub(2).max(1); // inner RUNS border
    let mut display = Vec::new();
    for row in rebis_run_tree_rows(runs) {
        match row {
            RebisRunTreeRow::Section {
                run,
                depth,
                kind,
                title,
            } => {
                // Rendering adds the tree rail, indentation, and `◆ AGENT` or
                // `◇ STEP`. Wrap the title itself so long prompts do not vanish.
                let prefix_width = 15usize.saturating_add(depth.saturating_mul(2));
                let available = content_width.saturating_sub(prefix_width).max(1);
                let characters = title.chars().collect::<Vec<_>>();
                for chunk in characters.chunks(available) {
                    display.push(RebisRunTreeRow::Section {
                        run,
                        depth,
                        kind,
                        title: chunk.iter().collect(),
                    });
                }
            }
            RebisRunTreeRow::Output { run, depth, text } => {
                // Rendering adds `    │ ` and two spaces per fold level.
                let prefix_width = 6usize.saturating_add(depth.saturating_mul(2));
                let available = content_width.saturating_sub(prefix_width).max(1);
                let characters = text.chars().collect::<Vec<_>>();
                if characters.is_empty() {
                    display.push(RebisRunTreeRow::Output {
                        run,
                        depth,
                        text: String::new(),
                    });
                } else {
                    for chunk in characters.chunks(available) {
                        display.push(RebisRunTreeRow::Output {
                            run,
                            depth,
                            text: chunk.iter().collect(),
                        });
                    }
                }
            }
            other => display.push(other),
        }
    }
    display
}

/// Full-panel run browser. `top` scrolls over the flattened run/output tree,
/// so expanded streams remain fully explorable without covering another
/// selected projection such as the mandala.
fn rebis_run_browser_layout(
    panel: Rect,
    row_count: usize,
    top: usize,
) -> Option<(Rect, usize, usize)> {
    if row_count == 0 || panel.width < 4 || panel.height < 3 {
        return None;
    }
    let visible = row_count
        .min(panel.height.saturating_sub(2) as usize)
        .max(1);
    let start = top.min(row_count.saturating_sub(visible));
    Some((panel, start, visible))
}

/// Draw the dedicated Rebis source/graph workspace. Both panes are views over a
/// single parsed `rebis_lang::Expr`; the right pane never reparses text.
fn draw_rebis_workspace(
    workspace: &mut RebisWorkspace,
    queue_len: usize,
    rebis_runs: &[RebisRunView],
    run_choice: usize,
    run_top: usize,
    chrome: RebisWorkspaceChrome<'_>,
    f: &mut Frame,
) -> Vec<TextPaneRegion> {
    let RebisWorkspaceChrome {
        selected_model,
        config_document,
    } = chrome;
    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(4),
        Constraint::Length(1),
        Constraint::Length(1),
    ])
    .split(f.area());

    let modified = if workspace.editor.dirty() { " [+]" } else { "" };
    let title_spans = if config_document {
        vec![
            Span::styled("⚙ CONFIG", red_bold()),
            Span::raw("   "),
            Span::styled(workspace.path_label(), Style::new().fg(C_ASH())),
            Span::styled(modified, Style::new().fg(C_GOLD())),
        ]
    } else {
        let mut spans = vec![
            Span::styled(
                "o-[]-o",
                Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD),
            ),
            Span::styled("  REBIS", red_bold()),
            Span::raw("   "),
        ];
        for symbol in rebis_workspace::REBIS_SYMBOLS {
            spans.push(Span::styled(format!("{symbol} "), rebis_operator_style()));
        }
        spans.extend([
            Span::raw("  "),
            Span::styled(workspace.path_label(), Style::new().fg(C_ASH())),
            Span::styled(modified, Style::new().fg(C_GOLD())),
        ]);
        spans
    };
    f.render_widget(Paragraph::new(Line::from(title_spans)), rows[0]);

    let panes = workspace.panel_visible.then(|| {
        if rows[1].width >= 78 {
            Layout::horizontal([Constraint::Percentage(56), Constraint::Percentage(44)])
                .split(rows[1])
        } else {
            Layout::vertical([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(rows[1])
        }
    });
    let editor_area = panes.as_ref().map_or(rows[1], |areas| areas[0]);
    let graph_area = panes.as_ref().map(|areas| areas[1]);

    let editor_block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::new().fg(C_OX()))
        .title(Span::styled(
            if config_document {
                " KAOS CONFIG "
            } else {
                " SOURCE "
            },
            Style::new().fg(C_GOLD()),
        ));
    let editor_inner = editor_block.inner(editor_area);
    f.render_widget(editor_block, editor_area);
    let editor_cursor = render_rebis_source(workspace, f, editor_inner);
    let source_content_left = editor_inner
        .x
        .saturating_add(rebis_source_number_width(workspace) as u16 + 2)
        .min(editor_inner.right());
    let mut selectable_panes = vec![TextPaneRegion {
        kind: TextPaneKind::RebisSource,
        area: editor_inner,
        content_left: source_content_left,
    }];

    let visualization_title = if workspace.sigil_chat_visible() {
        " SIGIL CHAT · GOD AGENT "
    } else if workspace.runs_visible {
        " BACKGROUND RUNS "
    } else {
        match workspace.visualization {
            Visualization::Mandala => " o-[M]-o · ~[f] MANDALA ",
            Visualization::Tree => " EXPRESSION TREE ",
            Visualization::Sigils => " SAVED SIGILS · SEARCH RESULTS ",
        }
    };
    workspace.panel_inner = None;
    let mut sigil_chat_cursor = None;
    if let Some(graph_area) = graph_area {
        let graph_block = Block::default()
            .borders(Borders::ALL)
            .border_style(Style::new().fg(if workspace.graph_focus {
                C_GOLD()
            } else {
                C_OX()
            }))
            .title(Span::styled(visualization_title, Style::new().fg(C_TEAL())));
        let graph_inner = graph_block.inner(graph_area);
        selectable_panes.push(TextPaneRegion::full(TextPaneKind::RebisPanel, graph_inner));
        workspace.panel_inner = Some((
            graph_inner.x,
            graph_inner.y,
            graph_inner.width,
            graph_inner.height,
        ));
        f.render_widget(graph_block, graph_area);
        if workspace.sigil_chat_visible() {
            let chat_areas =
                Layout::vertical([Constraint::Min(1), Constraint::Length(3)]).split(graph_inner);
            let history_width = chat_areas[0].width.saturating_sub(1) as usize;
            let history = workspace
                .sigil_chat_lines()
                .iter()
                .map(|line| {
                    let style = if line.starts_with("you     ") {
                        Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD)
                    } else if line.starts_with("system  ") || line == "GOD CHANNEL" {
                        Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD)
                    } else {
                        Style::new().fg(C_BONE())
                    };
                    Line::from(Span::styled(line.clone(), style))
                })
                .flat_map(|line| wrap_line(&line, history_width.max(1)))
                .collect::<Vec<_>>();
            let visible = chat_areas[0].height as usize;
            let max_top = history.len().saturating_sub(visible.max(1));
            workspace.graph_top = workspace.graph_top.min(max_top);
            f.render_widget(
                Paragraph::new(Text::from(history)).scroll((workspace.graph_top as u16, 0)),
                chat_areas[0],
            );
            let input_title = if workspace.sigil_chat_busy() {
                " GOD WORKING · streaming above "
            } else {
                " MESSAGE · Enter send · Esc source "
            };
            let input_block = Block::default()
                .borders(Borders::ALL)
                .border_style(Style::new().fg(if workspace.sigil_chat_busy() {
                    C_TEAL()
                } else {
                    C_GOLD()
                }))
                .title(Span::styled(input_title, Style::new().fg(C_GOLD())));
            let input_inner = input_block.inner(chat_areas[1]);
            f.render_widget(input_block, chat_areas[1]);
            f.render_widget(
                Paragraph::new(workspace.sigil_chat_input().to_string()),
                input_inner,
            );
            if workspace.graph_focus && input_inner.width > 0 {
                sigil_chat_cursor = Some(Position {
                    x: input_inner
                        .x
                        .saturating_add(workspace.sigil_chat_input().chars().count() as u16)
                        .min(input_inner.right().saturating_sub(1)),
                    y: input_inner.y,
                });
            }
        } else if !workspace.runs_visible {
            let graph_lines =
                workspace.graph_lines(graph_inner.width as usize, graph_inner.height as usize);
            let graph_text = graph_lines
                .into_iter()
                .map(|line| {
                    let style = if line.starts_with("o-[]-o") {
                        Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD)
                    } else if matches!(line.as_str(), "REGION TREE" | "DIRECTED FLOW") {
                        Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD)
                    } else if line.contains('→') || line.contains('←') || line.contains('[') {
                        Style::new().fg(C_TEAL())
                    } else {
                        Style::new().fg(C_BONE())
                    };
                    Line::from(Span::styled(line, style))
                })
                .collect::<Vec<_>>();
            f.render_widget(Paragraph::new(Text::from(graph_text)), graph_inner);
        }

        let run_rows = rebis_run_display_rows(rebis_runs, graph_inner.width as usize);
        let run_layout = if workspace.runs_visible {
            rebis_run_browser_layout(graph_inner, run_rows.len(), run_top)
        } else {
            None
        };
        if let Some((area, start, visible)) = run_layout {
            f.render_widget(Clear, area);
            let selected = run_choice.min(rebis_runs.len() - 1);
            let lines = run_rows
                .iter()
                .skip(start)
                .take(visible)
                .map(|row| match row {
                    RebisRunTreeRow::Header(index) => {
                        let run = &rebis_runs[*index];
                        let marker = if *index == selected { "❯" } else { " " };
                        let caret = if run.entry.expanded { "▾" } else { "▸" };
                        let lane = if run.entry.parallel { "∥" } else { "" };
                        let state = match run.entry.state {
                            RebisRunState::AwaitingPermission => "⚠ PERMISSION".to_string(),
                            RebisRunState::Queued => {
                                format!("⧗{} QUEUED", run.queue_position.unwrap_or_default())
                            }
                            RebisRunState::Running if run.entry.paused => "Ⅱ PAUSED".to_string(),
                            RebisRunState::Running => "● RUNNING".to_string(),
                            RebisRunState::Complete => "✓ DONE".to_string(),
                            RebisRunState::Cancelled => "× CANCELLED".to_string(),
                        };
                        let timer = rebis_run_timer(&run.entry);
                        let duration = timer
                            .split_once(' ')
                            .map_or(timer.as_str(), |(_, duration)| duration);
                        let label = format!(
                            "{marker}{caret}{lane} {state} {} {:<7} {}",
                            duration,
                            run.entry.scope.label().to_ascii_uppercase(),
                            run.entry.preview
                        );
                        Line::from(Span::styled(
                            truncate(&label, area.width.saturating_sub(2) as usize),
                            if *index == selected {
                                Style::new()
                                    .fg(Color::Black)
                                    .bg(C_GOLD())
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::new().fg(C_BONE())
                            },
                        ))
                    }
                    RebisRunTreeRow::Section {
                        run,
                        depth,
                        kind,
                        title,
                    } => {
                        let indent = "  ".repeat(*depth);
                        let (section, title) = match kind {
                            RebisRunSectionKind::Agent => (
                                "◆ AGENT",
                                title.strip_prefix("Rebis agent ").unwrap_or(title),
                            ),
                            RebisRunSectionKind::Model => ("◇ MODEL", title.as_str()),
                            RebisRunSectionKind::Step => (
                                "◇ STEP",
                                title
                                    .strip_prefix("⋯ step ")
                                    .or_else(|| title.strip_prefix("... step "))
                                    .unwrap_or(title),
                            ),
                        };
                        Line::from(Span::styled(
                            format!("    │ {indent}{section}  {title}"),
                            if *run == selected {
                                Style::new()
                                    .fg(if *kind == RebisRunSectionKind::Agent {
                                        C_TEAL()
                                    } else {
                                        C_GOLD()
                                    })
                                    .add_modifier(Modifier::BOLD)
                            } else {
                                Style::new().fg(C_OX())
                            },
                        ))
                    }
                    RebisRunTreeRow::Output { run, depth, text } => Line::from(Span::styled(
                        format!("    │ {}{text}", "  ".repeat(*depth)),
                        if *run == selected {
                            Style::new().fg(C_ASH())
                        } else {
                            Style::new().fg(C_OX())
                        },
                    )),
                })
                .collect::<Vec<_>>();
            let border = if workspace.graph_focus {
                C_GOLD()
            } else {
                C_OX()
            };
            f.render_widget(
                Paragraph::new(lines).scroll((0, 0)).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(Style::new().fg(border))
                        .title(Span::styled(
                            " RUNS · j/k RUN · ↑/↓ SCROLL · ⇧↓ TAIL · Pg SCROLL · Tab OPEN ",
                            Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD),
                        )),
                ),
                area,
            );
        }
    }

    if workspace.mode == RebisMode::KaosCommand {
        let suggestions = rebis_completions(&workspace.command);
        if !suggestions.is_empty() {
            let visible = suggestions.len().min(7);
            let height = (visible + 2) as u16;
            let width = rows[1].width.min(42);
            let area = Rect::new(
                rows[1].x,
                rows[1].y + rows[1].height.saturating_sub(height),
                width,
                height.min(rows[1].height),
            );
            f.render_widget(Clear, area);
            let selected = workspace.command_choice.min(suggestions.len() - 1);
            let start = selected.saturating_sub(visible.saturating_sub(1));
            let lines = suggestions
                .iter()
                .skip(start)
                .take(visible)
                .enumerate()
                .map(|(offset, command)| {
                    let index = start + offset;
                    Line::from(Span::styled(
                        format!("/{}", command.display),
                        if index == selected {
                            Style::new()
                                .fg(Color::Black)
                                .bg(C_TEAL())
                                .add_modifier(Modifier::BOLD)
                        } else {
                            Style::new().fg(C_BONE())
                        },
                    ))
                })
                .collect::<Vec<_>>();
            f.render_widget(
                Paragraph::new(lines).block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title(" KAOS COMMANDS "),
                ),
                area,
            );
        }
    }

    let diagnostic = if config_document {
        Line::from(vec![
            Span::styled(
                "⚙ CONFIG  ",
                Style::new().fg(C_TEAL()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                "plain key/value file · provider credentials remain separate",
                Style::new().fg(C_ASH()),
            ),
        ])
    } else if workspace.chaos_star_visible() {
        Line::raw("")
    } else if let Some(error) = workspace.diagnostic() {
        Line::from(vec![
            Span::styled(
                "✗ INVALID  ",
                Style::new().fg(C_RED()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate(error, rows[2].width.saturating_sub(11) as usize),
                Style::new().fg(C_RED()),
            ),
        ])
    } else {
        let canonical = workspace.canonical().unwrap_or_default();
        Line::from(vec![
            Span::styled(
                "✓ VALID  ",
                Style::new().fg(C_DONE()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate(canonical, rows[2].width.saturating_sub(9) as usize),
                Style::new().fg(C_ASH()),
            ),
        ])
    };
    f.render_widget(Paragraph::new(diagnostic), rows[2]);

    let (row, column) = workspace.editor.row_col();
    let status = if matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand) {
        Line::from(vec![
            Span::styled(
                if workspace.mode == RebisMode::Command {
                    ":"
                } else {
                    "/"
                },
                red_bold(),
            ),
            Span::styled(workspace.command.clone(), Style::new().fg(C_BONE())),
        ])
    } else {
        let hint = if config_document {
            "  :w save · :q return · restart Kaos to apply"
        } else {
            "  Ctrl-K commands · /graph scroll · /panel hide · v/V visual · :w/:q"
        };
        let queued = if queue_len == 0 {
            String::new()
        } else {
            format!("  ⧗{queue_len}")
        };
        let message_width = rows[3]
            .width
            .saturating_sub(24 + queued.chars().count() as u16)
            as usize;
        Line::from(vec![
            Span::styled(
                format!(
                    " {:<7} ",
                    if workspace.vim_enabled {
                        workspace.mode.label()
                    } else {
                        "EDIT"
                    }
                ),
                if workspace.mode == RebisMode::Insert {
                    Style::new()
                        .fg(Color::Black)
                        .bg(C_TEAL())
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::new()
                        .fg(C_BONE())
                        .bg(C_OX())
                        .add_modifier(Modifier::BOLD)
                },
            ),
            Span::styled(
                format!("  {}:{}  ", row + 1, column + 1),
                Style::new().fg(C_ASH()),
            ),
            Span::styled(
                queued,
                Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD),
            ),
            Span::styled(
                truncate(&workspace.message, message_width),
                Style::new().fg(C_ASH()),
            ),
            Span::styled(hint, Style::new().fg(C_OX())),
        ])
    };
    let status_area = render_footer_with_model(f, rows[3], status, selected_model);

    if status_area.width > 0
        && matches!(workspace.mode, RebisMode::Command | RebisMode::KaosCommand)
    {
        let x = rows[3]
            .x
            .saturating_add(1)
            .saturating_add(workspace.command.chars().count() as u16)
            .min(status_area.right().saturating_sub(1));
        f.set_cursor_position(Position { x, y: rows[3].y });
    } else if let Some(position) = sigil_chat_cursor {
        f.set_cursor_position(position);
    } else if workspace.mode == RebisMode::Insert {
        if let Some(position) = editor_cursor {
            f.set_cursor_position(position);
        }
    }
    selectable_panes
}

fn rebis_source_number_width(workspace: &RebisWorkspace) -> usize {
    workspace.editor.line_count().to_string().len().max(2)
}

fn render_rebis_source(
    workspace: &mut RebisWorkspace,
    f: &mut Frame,
    area: Rect,
) -> Option<Position> {
    let show_star = workspace.chaos_star_visible();
    if show_star {
        render_chaos_star(f, area);
        return None;
    }
    let number_width = rebis_source_number_width(workspace);
    let code_width = (area.width as usize)
        .saturating_sub(number_width + 2)
        .max(1);
    if workspace.source_follows_cursor() {
        workspace.ensure_visible(area.height as usize, code_width);
    }

    let source = workspace.editor.source();
    let chars: Vec<char> = source.chars().collect();
    let colours = rebis_workspace::highlights(source);
    let matched = workspace.editor.matching_parentheses();
    let error = workspace.error_char();
    let visual = match workspace.mode {
        RebisMode::Visual => workspace.editor.visual_range(false),
        RebisMode::VisualLine => workspace.editor.visual_range(true),
        _ => None,
    };
    let block = if workspace.mode == RebisMode::VisualBlock {
        workspace.editor.visual_block_range()
    } else {
        None
    };
    let cursor = workspace.editor.cursor();
    let (cursor_row, cursor_column) = workspace.editor.row_col();

    let mut starts = vec![0usize];
    for (index, character) in chars.iter().enumerate() {
        if *character == '\n' {
            starts.push(index + 1);
        }
    }
    let mut rendered = Vec::new();
    for screen_row in 0..area.height as usize {
        let row = workspace.view_top + screen_row;
        if row >= starts.len() {
            rendered.push(Line::raw(""));
            continue;
        }
        let start = starts[row];
        let end = chars[start..]
            .iter()
            .position(|character| *character == '\n')
            .map_or(chars.len(), |offset| start + offset);
        let mut spans = vec![
            Span::styled(
                format!("{:>number_width$} ", row + 1),
                Style::new().fg(if row == cursor_row { C_GOLD() } else { C_OX() }),
            ),
            Span::styled("│", Style::new().fg(C_OX())),
        ];
        let visible_start = start + workspace.view_left.min(end - start);
        let visible_end = (visible_start + code_width).min(end);
        for index in visible_start..visible_end {
            let mut style = rebis_highlight_style(colours[index]);
            if matched.is_some_and(|(left, right)| index == left || index == right) {
                style = style.bg(C_OX()).add_modifier(Modifier::BOLD);
            }
            if error == Some(index) {
                style = style.fg(C_RED()).add_modifier(Modifier::UNDERLINED);
            }
            if visual.is_some_and(|(start, end)| index >= start && index <= end) {
                style = style.bg(C_BLUE()).add_modifier(Modifier::BOLD);
            }
            if block.is_some_and(|(top, bottom, left, right)| {
                let column = index - start;
                row >= top && row <= bottom && column >= left && column <= right
            }) {
                style = style.bg(C_BLUE()).add_modifier(Modifier::BOLD);
            }
            if workspace.mode == RebisMode::Normal && cursor == index {
                style = style.add_modifier(Modifier::REVERSED);
            }
            spans.push(Span::styled(chars[index].to_string(), style));
        }
        // Give an insertion point at end-of-line (and a visible normal cursor on
        // empty lines) without altering the source.
        if row == cursor_row && cursor == end && cursor_column >= workspace.view_left {
            let style = if workspace.mode == RebisMode::Normal {
                Style::new().fg(C_BONE()).add_modifier(Modifier::REVERSED)
            } else {
                Style::new().fg(C_BONE())
            };
            spans.push(Span::styled(" ", style));
        }
        rendered.push(Line::from(spans));
    }
    f.render_widget(Paragraph::new(Text::from(rendered)), area);

    let visible_row = cursor_row.checked_sub(workspace.view_top)?;
    let visible_column = cursor_column.checked_sub(workspace.view_left)?;
    if visible_row >= area.height as usize || visible_column >= code_width {
        return None;
    }
    Some(Position {
        x: area.x + number_width as u16 + 2 + visible_column as u16,
        y: area.y + visible_row as u16,
    })
}

fn render_chaos_star(f: &mut Frame, area: Rect) {
    f.render_widget(Clear, area);
    let star = theme::chaos_star_lines();
    let star_width = star
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or_default()
        .min(area.width as usize) as u16;
    let star_height = (star.len() as u16).min(area.height);
    let star_area = Rect::new(
        area.x + area.width.saturating_sub(star_width) / 2,
        area.y + area.height.saturating_sub(star_height) / 2,
        star_width,
        star_height,
    );
    for (row, line) in star.into_iter().take(star_height as usize).enumerate() {
        for (column, glyph) in line.chars().take(star_width as usize).enumerate() {
            if !glyph.is_whitespace() {
                f.render_widget(
                    Paragraph::new(Line::from(Span::styled(glyph.to_string(), red_bold()))),
                    Rect::new(star_area.x + column as u16, star_area.y + row as u16, 1, 1),
                );
            }
        }
    }
}

fn rebis_highlight_style(highlight: Highlight) -> Style {
    match highlight {
        Highlight::Atom => Style::new().fg(C_BONE()),
        Highlight::Prompt => Style::new().fg(C_TEAL()),
        Highlight::Forward
        | Highlight::Mediate
        | Highlight::Import
        | Highlight::Backflow
        | Highlight::Parenthesis => rebis_operator_style(),
        Highlight::Whitespace => Style::new().fg(C_BONE()),
        Highlight::Comment => Style::new().fg(C_ASH()).add_modifier(Modifier::ITALIC),
        Highlight::Invalid => Style::new().fg(C_RED()).add_modifier(Modifier::UNDERLINED),
    }
}

fn rebis_operator_style() -> Style {
    Style::new().fg(C_GOLD()).add_modifier(Modifier::BOLD)
}

fn spawn_reader(r: impl Read + Send + 'static, tx: Sender<Msg>, is_err: bool) {
    thread::spawn(move || {
        let mut br = BufReader::new(r);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            match br.read_until(b'\n', &mut buf) {
                Ok(0) => break,
                Ok(_) => {
                    let s = String::from_utf8_lossy(&buf);
                    let line = s.trim_end_matches(['\n', '\r']);
                    // \r progress: show only what a terminal would (after the last \r).
                    let line = line.rsplit('\r').next().unwrap_or(line);
                    let payload = if is_err {
                        format!("\x1b[2;38;2;150;140;140m{line}\x1b[0m")
                    } else {
                        line.to_string()
                    };
                    if tx.send(Msg::Line(payload)).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });
}

/// Wrap one styled `Line` to `width` columns, preserving each span's style and
/// breaking at the last space in the row when possible (else a hard break). Returns
/// one row when it already fits (or width is 0/unknown).
fn wrap_line(line: &Line<'static>, width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line.clone()];
    }
    // Flatten to (char, style), tracking display columns.
    let mut cells: Vec<(char, Style)> = Vec::new();
    for span in &line.spans {
        for ch in span.content.chars() {
            cells.push((ch, span.style));
        }
    }
    if cells.len() <= width && !cells.iter().any(|(character, _)| *character == '\n') {
        return vec![line.clone()];
    }

    let mut rows: Vec<Vec<(char, Style)>> = Vec::new();
    let mut cur: Vec<(char, Style)> = Vec::new();
    for cell in cells {
        if cell.0 == '\n' {
            rows.push(std::mem::take(&mut cur));
            continue;
        }
        cur.push(cell);
        if cur.len() >= width {
            // Prefer a word break: split after the last space, keep it flowing.
            if let Some(pos) = cur.iter().rposition(|(c, _)| *c == ' ') {
                if pos > 0 && pos + 1 < cur.len() {
                    let rest = cur.split_off(pos + 1);
                    rows.push(std::mem::take(&mut cur));
                    cur = rest;
                    continue;
                }
            }
            rows.push(std::mem::take(&mut cur));
        }
    }
    if !cur.is_empty() || rows.is_empty() {
        rows.push(cur);
    }

    rows.into_iter().map(row_to_line).collect()
}

/// Hard-wrap a cell sequence into rows of exactly `width` columns (no word break),
/// so cursor row/col math stays exact. Always returns at least one row.
fn hard_wrap(cells: &[(char, Style)], width: usize) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![row_to_line(cells.to_vec())];
    }
    if cells.is_empty() {
        return vec![Line::default()];
    }
    let mut rows = Vec::new();
    let mut current = Vec::new();
    let mut ended_at_boundary = false;
    for cell in cells {
        if cell.0 == '\n' {
            rows.push(row_to_line(std::mem::take(&mut current)));
            ended_at_boundary = true;
            continue;
        }
        current.push(*cell);
        ended_at_boundary = false;
        if current.len() == width {
            rows.push(row_to_line(std::mem::take(&mut current)));
            ended_at_boundary = true;
        }
    }
    if !current.is_empty() || ended_at_boundary {
        rows.push(row_to_line(current));
    }
    rows
}

/// Cursor row and column within [`hard_wrap`]. `cursor_cell` is the number of
/// cells before the cursor; newlines reset the column instead of consuming it.
fn wrapped_cursor(cells: &[(char, Style)], cursor_cell: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let mut row = 0usize;
    let mut column = 0usize;
    for (character, _) in cells.iter().take(cursor_cell) {
        if *character == '\n' {
            row += 1;
            column = 0;
        } else {
            column += 1;
            if column == width {
                row += 1;
                column = 0;
            }
        }
    }
    (row, column)
}

/// Rebuild one row of (char, style) cells into a `Line`, merging equal-style runs.
fn row_to_line(row: Vec<(char, Style)>) -> Line<'static> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut it = row.into_iter();
    if let Some((c0, s0)) = it.next() {
        let mut style = s0;
        let mut buf = String::new();
        buf.push(c0);
        for (c, s) in it {
            if s == style {
                buf.push(c);
            } else {
                spans.push(Span::styled(std::mem::take(&mut buf), style));
                style = s;
                buf.push(c);
            }
        }
        spans.push(Span::styled(buf, style));
    }
    Line::from(spans)
}

/// Commands that only touch app state and are safe (and useful) to run while a
/// job streams — everything else spawns a subprocess and therefore queues.
fn is_local_command(line: &str) -> bool {
    let Some(body) = line.strip_prefix('/') else {
        return false;
    };
    matches!(
        body.split_whitespace().next().unwrap_or(""),
        "quit"
            | "exit"
            | "q"
            | "clear"
            | "cls"
            | "cd"
            | "model"
            | "bind"
            | "new"
            | "forget"
            | "rebis"
            | "runs"
            | "config"
            | "chaos"
    )
}

/// Whether a terminal key event represents the command-palette chord.
///
/// The canonical binding is `Ctrl-K` (a plain `0x0B` control byte that every
/// terminal, including macOS Terminal.app, delivers reliably). The historical
/// `Ctrl-/` chord is still accepted for muscle memory: modern keyboard
/// protocols preserve `/` (or its shifted `_` spelling), while the legacy ASCII
/// unit-separator byte is normalized by Crossterm to `Ctrl-7`.
fn command_palette_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    (modifiers.contains(KeyModifiers::CONTROL)
        && matches!(code, KeyCode::Char('k' | 'K' | '/' | '_' | '7')))
        || code == KeyCode::Char('\u{1f}')
}

/// Plain `Ctrl-C` is the one global exit chord. Shift is deliberately excluded
/// because `Ctrl-Shift-C` remains pane-local copy.
fn ctrl_c_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && !modifiers.contains(KeyModifiers::SHIFT)
        && code == KeyCode::Char('c')
}

/// `Ctrl-Shift-C` as reported by both enhanced and partially-normalized
/// terminal keyboard protocols. An uppercase `C` carries the shift intent even
/// when the terminal omits the explicit SHIFT modifier.
fn selection_copy_shortcut(code: KeyCode, modifiers: KeyModifiers) -> bool {
    modifiers.contains(KeyModifiers::CONTROL)
        && (matches!(code, KeyCode::Char('C'))
            || (modifiers.contains(KeyModifiers::SHIFT)
                && matches!(code, KeyCode::Char('c' | 'C'))))
}

/// A minimal shell-style splitter honouring "double" and 'single' quotes.
fn shell_split(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut any = false;
    for c in s.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => {
                if c == '"' || c == '\'' {
                    quote = Some(c);
                    any = true;
                } else if c.is_whitespace() {
                    if any || !cur.is_empty() {
                        out.push(std::mem::take(&mut cur));
                        any = false;
                    }
                } else {
                    cur.push(c);
                    any = true;
                }
            }
        }
    }
    if any || !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn truncate(s: &str, n: usize) -> String {
    if s.chars().count() <= n {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(n.saturating_sub(1)).collect();
        t.push('\u{2026}');
        t
    }
}

/// A compact, stable label for a captured run. Prefer the first meaningful
/// form or prompt over comments and outer grouping delimiters.
fn rebis_source_preview(source: &str) -> String {
    source
        .lines()
        .map(str::trim)
        .find(|line| {
            !line.is_empty() && !line.starts_with(';') && !matches!(*line, "(" | ")" | "[" | "]")
        })
        .unwrap_or("(empty source)")
        .to_string()
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default()
}

/// A v4-format UUID (claude requires a valid UUID for --session-id). Seeded from
/// the clock + pid; uniqueness per session is all we need, not cryptographic rigor.
fn gen_uuid() -> String {
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
        ^ ((std::process::id() as u64) << 17);
    let mut r = crate::rng::Rng::new(seed);
    let a = r.next_u64();
    let b = r.next_u64();
    format!(
        "{:08x}-{:04x}-4{:03x}-{:x}{:03x}-{:012x}",
        (a >> 32) as u32,
        ((a >> 16) & 0xffff) as u16,
        (a & 0x0fff) as u16,
        8 + (b & 0x3), // variant nibble: 8..b
        ((b >> 2) & 0x0fff) as u16,
        (b >> 16) & 0xffff_ffff_ffff,
    )
}

fn short_path(p: &std::path::Path) -> String {
    let s = p.display().to_string();
    let home = dirs_home().display().to_string();
    if !home.is_empty() && s.starts_with(&home) {
        s.replacen(&home, "~", 1)
    } else {
        s
    }
}

fn history_file() -> PathBuf {
    dirs_home().join(".kaos_history")
}

fn load_history() -> Vec<String> {
    std::fs::read_to_string(history_file())
        .map(|contents| decode_history(&contents))
        .unwrap_or_default()
}

fn decode_history(contents: &str) -> Vec<String> {
    contents
        .lines()
        .map(|entry| serde_json::from_str::<String>(entry).unwrap_or_else(|_| entry.to_string()))
        .collect()
}

fn append_history(line: &str) {
    use std::io::Write;
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(history_file())
    {
        if let Ok(encoded) = serde_json::to_string(line) {
            let _ = writeln!(f, "{encoded}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_job(command: &str, rebis_run_id: Option<u64>) -> (Job, mpsc::Sender<Msg>) {
        let child = Command::new("sh").arg("-c").arg(command).spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        (
            Job {
                child: Arc::new(Mutex::new(child)),
                rx,
                label: "parallel test job".to_string(),
                claude_session: false,
                rebis_run_id,
                owns_process_group: false,
            },
            tx,
        )
    }

    #[test]
    fn slash_completions_filter_both_command_catalogs() {
        assert_eq!(
            main_completions("/re"),
            vec![command("rebis [FILE]", "rebis ")]
        );
        assert_eq!(
            completions("panel h", REBIS_SLASH_COMMANDS),
            vec![command("panel hide", "panel hide")]
        );
        assert!(completions("does-not-exist", REBIS_SLASH_COMMANDS).is_empty());
        assert_eq!(
            completions("record ", REBIS_SLASH_COMMANDS)[0].display,
            "record [FILE]"
        );
        assert_eq!(
            completions("search ", REBIS_SLASH_COMMANDS)[0].display,
            "search [TEXT]"
        );
        let displays = REBIS_SLASH_COMMANDS
            .iter()
            .map(|command| command.display)
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(displays.len(), REBIS_SLASH_COMMANDS.len());
        assert_eq!(
            main_completions("/config r"),
            vec![command("config restore", "config restore")]
        );
        assert_eq!(
            rebis_completions("config r"),
            vec![command("config restore", "config restore")]
        );
        assert_eq!(
            rebis_completions("run p"),
            vec![command("run parallel", "run parallel")]
        );
        assert_eq!(
            rebis_completions("run block p"),
            vec![command("run block parallel", "run block parallel")]
        );
        assert_eq!(
            rebis_completions("sigil c"),
            vec![command("sigil chat", "sigil chat")]
        );
    }

    #[test]
    fn saved_rebis_run_codec_round_trips_multiline_context() {
        let saved = SavedRebisRun {
            source: "(-> \"α\" \"β\")\n".to_string(),
            input: "record line 1\nrecord line 2".to_string(),
            scope: RunScope::Block,
            parallel: true,
            chaos: true,
            output: vec!["first\nwrapped".to_string(), "final ✓".to_string()],
            elapsed: Duration::from_millis(12_345),
            pause_reason: "model allowance reached".to_string(),
        };
        assert_eq!(
            decode_saved_rebis_run(&encode_saved_rebis_run(&saved)),
            Ok(saved)
        );
    }

    #[test]
    fn god_agent_context_contains_every_live_bot_and_no_terminal_bot() {
        let mut app = App::new();
        app.open_rebis(None);
        let first = RunRequest {
            source: "(-> \"first source\" \"first result\")".to_string(),
            input: "first record".to_string(),
            scope: RunScope::Program,
        };
        let second = RunRequest {
            source: "\"second source\"".to_string(),
            input: "second record".to_string(),
            scope: RunScope::Block,
        };
        let finished = RunRequest {
            source: "\"finished source\"".to_string(),
            input: "finished record".to_string(),
            scope: RunScope::Program,
        };
        let first_id = app.register_rebis_run(&first, RebisRunState::Running);
        let second_id = app.register_rebis_run(&second, RebisRunState::Queued);
        let finished_id = app.register_rebis_run(&finished, RebisRunState::Complete);
        app.rebis_runs[0]
            .output
            .push("first bot retained trace".to_string());
        app.rebis_runs[1]
            .output
            .push("second bot queued trace".to_string());
        std::fs::write(&app.rebis_runs[0].checkpoint_path, "checkpoint one").unwrap();
        std::fs::write(&app.rebis_runs[1].directive_path, "compare with run one").unwrap();

        let context = app.sigil_chat_run_context(Some(first_id));

        assert!(context.contains("live bots: 2"), "{context}");
        assert!(context.contains(&format!("RUN #{first_id} · BOUND")));
        assert!(context.contains(&format!("RUN #{second_id} · READ-ONLY PEER")));
        assert!(!context.contains(&format!("RUN #{finished_id} ·")));
        for retained in [
            "first source",
            "second source",
            "first record",
            "second record",
            "checkpoint one",
            "compare with run one",
            "first bot retained trace",
            "second bot queued trace",
        ] {
            assert!(
                context.contains(retained),
                "missing {retained:?}: {context}"
            );
        }
        let _ = std::fs::remove_file(&app.rebis_runs[0].checkpoint_path);
        let _ = std::fs::remove_file(&app.rebis_runs[1].directive_path);
    }

    #[test]
    fn god_agent_applies_and_clears_a_directive_for_one_targeted_bot() {
        let mut app = App::new();
        app.open_rebis(None);
        let request = RunRequest {
            source: "\"work\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let bridge = std::env::temp_dir().join(format!("kaos-god-control-{}", gen_uuid()));
        std::fs::create_dir_all(&bridge).unwrap();
        app.write_sigil_chat_control_bridge(&bridge).unwrap();
        std::fs::write(
            bridge
                .join("runs")
                .join(id.to_string())
                .join("directive.txt"),
            "audit every claim before returning",
        )
        .unwrap();
        std::fs::write(
            bridge.join("run-control.txt"),
            format!("APPLY_DIRECTIVE {id}\n"),
        )
        .unwrap();

        app.apply_sigil_chat_run_controls(&bridge);
        assert_eq!(
            crate::rebis_supervisor::read_directive(&app.rebis_runs[0].directive_path).as_deref(),
            Some("audit every claim before returning")
        );
        assert!(app.rebis_runs[0]
            .output
            .iter()
            .any(|line| line.contains("directive")));

        std::fs::write(
            bridge.join("run-control.txt"),
            format!("CLEAR_DIRECTIVE {id}\n"),
        )
        .unwrap();
        app.apply_sigil_chat_run_controls(&bridge);
        assert!(!app.rebis_runs[0].directive_path.exists());
        let _ = std::fs::remove_dir_all(bridge);
    }

    #[test]
    fn god_agent_control_manifest_is_strict_and_has_no_kill_action() {
        assert_eq!(
            parse_sigil_run_controls("# requested actions\nPAUSE 2\nRESUME 3\n"),
            Ok(vec![SigilRunControl::Pause(2), SigilRunControl::Resume(3)])
        );
        assert!(parse_sigil_run_controls("KILL 2").is_err());
        assert!(parse_sigil_run_controls("PAUSE 2 now").is_err());
    }

    #[test]
    fn saved_sigil_restores_an_unfinished_run_in_a_new_app() {
        let root = std::env::temp_dir().join(format!(
            "kaos-saved-run-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let source = "(-> \"first prompt\" \"unfinished prompt\")";

        let mut first = App::new();
        first.open_rebis(None);
        {
            let workspace = first.rebis.as_mut().unwrap();
            workspace.set_sigils_root_for_test(root.clone());
            workspace.dismiss_chaos_star();
            workspace.editor.replace(source.to_string());
            workspace.refresh();
        }
        let request = RunRequest {
            source: source.to_string(),
            input: "durable record".to_string(),
            scope: RunScope::Program,
        };
        let id = first.register_rebis_run(&request, RebisRunState::Running);
        first.rebis_runs[0]
            .output
            .push("answer   first prompt complete".to_string());
        let temporary_checkpoint = first.rebis_runs[0].checkpoint_path.clone();
        let journal = b"KAOS_REBIS_PROMPTS_V1\n66697273742070726f6d7074\tS646f6e65\n";
        std::fs::write(&temporary_checkpoint, journal).unwrap();
        first.rebis_run_choice = 0;
        first.rebis.as_mut().unwrap().command = "sigil save durable/loop".to_string();
        let action = first.rebis.as_mut().unwrap().execute_kaos_command();
        first.handle_rebis_action(action);
        first.pump();

        assert_eq!(first.rebis_runs[0].id, id);
        assert_eq!(first.rebis_runs[0].sigil.as_deref(), Some("durable/loop"));
        assert!(root.join("durable/loop.run").is_file());
        assert_eq!(
            std::fs::read(root.join("durable/loop.checkpoint")).unwrap(),
            journal
        );

        let mut reopened = App::new();
        reopened.open_rebis(None);
        reopened
            .rebis
            .as_mut()
            .unwrap()
            .set_sigils_root_for_test(root.clone());
        reopened.rebis.as_mut().unwrap().command = "sigil open durable/loop".to_string();
        let action = reopened.rebis.as_mut().unwrap().execute_kaos_command();
        reopened.handle_rebis_action(action);
        reopened.pump();

        assert_eq!(reopened.rebis_runs.len(), 1);
        let restored = &reopened.rebis_runs[0];
        assert_eq!(restored.state, RebisRunState::Running);
        assert!(restored.paused);
        assert_eq!(restored.request.source, source);
        assert_eq!(restored.request.input, "durable record");
        assert_eq!(
            restored.checkpoint_path,
            root.join("durable/loop.checkpoint")
        );
        assert!(restored
            .output
            .iter()
            .any(|line| line.contains("saved sigil checkpoint")));
        assert!(reopened
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("p resumes"));

        let restored_id = restored.id;
        reopened.finish_rebis_subprocess(Some(restored_id), 0, false);
        assert!(!root.join("durable/loop.run").exists());
        assert!(!root.join("durable/loop.checkpoint").exists());

        let _ = std::fs::remove_file(temporary_checkpoint);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn chat_keeps_slash_and_ctrl_k_opens_its_command_palette() {
        let mut app = App::new();
        app.on_key(KeyCode::Char('/'), KeyModifiers::NONE);
        assert_eq!(app.input, "/");

        app.input = "discard this draft".to_string();
        app.cursor = app.input.chars().count();
        app.on_key(KeyCode::Char('k'), KeyModifiers::CONTROL);
        assert_eq!(app.input, "/");
        assert_eq!(app.cursor, 1);

        app.input = "and this one".to_string();
        app.cursor = app.input.chars().count();
        app.on_key(KeyCode::Char('_'), KeyModifiers::CONTROL);
        assert_eq!(app.input, "/");
        assert_eq!(app.cursor, 1);

        app.input = "replace this too".to_string();
        app.cursor = app.input.chars().count();
        app.on_key(KeyCode::Char('7'), KeyModifiers::CONTROL);
        assert_eq!(app.input, "/");
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn command_palette_shortcut_accepts_enhanced_and_legacy_terminal_events() {
        for code in [
            KeyCode::Char('k'),
            KeyCode::Char('K'),
            KeyCode::Char('/'),
            KeyCode::Char('_'),
            KeyCode::Char('7'),
        ] {
            assert!(command_palette_shortcut(code, KeyModifiers::CONTROL));
        }
        assert!(command_palette_shortcut(
            KeyCode::Char('\u{1f}'),
            KeyModifiers::NONE
        ));
        assert!(!command_palette_shortcut(
            KeyCode::Char('/'),
            KeyModifiers::NONE
        ));
    }

    #[test]
    fn model_command_autocompletes_provider_and_model_choices() {
        assert_eq!(
            main_completions("/model claude:o"),
            vec![command("model claude:opus", "model claude:opus")]
        );
        assert_eq!(
            main_completions("/model open"),
            vec![
                command("model openai", "model openai"),
                command("model openrouter", "model openrouter")
            ]
        );
        assert!(main_completions("/model my-custom-model").is_empty());
        assert_eq!(
            main_completions("/model claude:f"),
            vec![command("model claude:fable", "model claude:fable")]
        );
        assert_eq!(
            rebis_completions("model claude:h"),
            vec![command("model claude:haiku", "model claude:haiku")]
        );
    }

    #[test]
    fn model_command_executes_without_leaving_rebis() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().command = "model claude:opus".to_string();
        let action = app.rebis.as_mut().unwrap().execute_kaos_command();

        app.handle_rebis_action(action);

        assert_eq!(app.model, "claude:opus");
        assert!(app.rebis.is_some());
        assert!(app.rebis.as_ref().unwrap().message.contains("model bound"));
    }

    #[test]
    fn fable_selection_uses_the_claude_cli_namespace() {
        let mut app = App::new();
        app.set_model("claude:fable");

        assert_eq!(app.model, "claude:fable");
        let spec = crate::provider::Spec::parse(&app.model);
        assert_eq!(spec.kind, crate::provider::Kind::ClaudeCli);
        assert_eq!(spec.claude_tag(), Some("fable"));
        assert!(spec.readiness().is_ok());
    }

    /// The app paints its own page, so a light theme is genuinely light rather
    /// than dark ink on whatever the terminal happens to be set to.
    #[test]
    fn every_cell_is_painted_with_the_configured_ground() {
        let mut app = App::new();
        let backend = ratatui::backend::TestBackend::new(80, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();

        let ground = c_ground();
        let buffer = terminal.backend().buffer();
        // Widgets may set their own background; what must never happen is a
        // cell left on the terminal's default, which is what makes a light
        // theme unreadable.
        let unpainted = buffer
            .content()
            .iter()
            .filter(|cell| cell.bg == Color::Reset)
            .count();
        assert_eq!(
            unpainted, 0,
            "{unpainted} cells left on the terminal's own background"
        );
        assert!(
            buffer.content().iter().any(|cell| cell.bg == ground),
            "nothing was painted with the configured ground"
        );
    }

    #[test]
    fn current_model_is_reserved_in_chat_and_rebis_footers() {
        fn footer(terminal: &ratatui::Terminal<ratatui::backend::TestBackend>) -> String {
            let area = terminal.backend().buffer().area;
            let start = (area.height.saturating_sub(1) * area.width) as usize;
            terminal.backend().buffer().content()[start..]
                .iter()
                .map(|cell| cell.symbol())
                .collect()
        }

        let mut rebis = App::new();
        rebis.model = "claude:opus".to_string();
        rebis.open_rebis(None);
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| rebis.draw(frame)).unwrap();
        assert!(footer(&terminal).contains("MODEL claude:opus"));

        let workspace = rebis.rebis.as_mut().unwrap();
        workspace.dismiss_chaos_star();
        workspace.mode = RebisMode::KaosCommand;
        workspace.command = "runs".to_string();
        terminal.draw(|frame| rebis.draw(frame)).unwrap();
        let command_footer = footer(&terminal);
        assert!(command_footer.contains("/runs"));
        assert!(command_footer.contains("MODEL claude:opus"));

        let mut chat = App::new();
        chat.model = "openai:gpt-5".to_string();
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| chat.draw(frame)).unwrap();
        assert!(footer(&terminal).contains("MODEL openai:gpt-5"));
    }

    #[test]
    fn direct_tool_agent_mode_is_default_and_chaos_is_explicitly_toggleable() {
        let mut app = App::new();
        assert!(!app.rebis_chaos_mode);
        app.open_rebis(None);

        app.rebis.as_mut().unwrap().command = "chaos on".to_string();
        let action = app.rebis.as_mut().unwrap().execute_kaos_command();
        app.handle_rebis_action(action);
        assert!(app.rebis_chaos_mode);
        assert!(app.rebis.as_ref().unwrap().message.contains("CHAOS mode"));

        app.rebis.as_mut().unwrap().command = "chaos off".to_string();
        let action = app.rebis.as_mut().unwrap().execute_kaos_command();
        app.handle_rebis_action(action);
        assert!(!app.rebis_chaos_mode);
        assert!(app.rebis.as_ref().unwrap().message.contains("DIRECT mode"));
    }

    #[test]
    fn rebis_workspace_survives_switching_to_chat() {
        let mut app = App::new();
        app.open_rebis(None);
        let workspace = app.rebis.as_mut().unwrap();
        workspace.editor.insert('x');
        workspace.view_top = 7;
        workspace.graph_left = 11;
        app.handle_rebis_action(WorkspaceAction::Suspend);
        assert!(app.rebis.is_none());
        assert!(app.suspended_rebis.is_some());

        app.open_rebis(None);
        let restored = app.rebis.as_ref().unwrap();
        assert!(restored.editor.source().starts_with('x'));
        assert_eq!(restored.view_top, 7);
        assert_eq!(restored.graph_left, 11);
    }

    #[test]
    fn config_editor_saves_and_returns_to_the_previous_rebis_workspace() {
        let root = std::env::temp_dir().join(format!("kaos-tui-config-{}", gen_uuid()));
        let path = root.join("kaos/config");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "KAOS_MODEL = sim\n").unwrap();

        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.rebis
            .as_mut()
            .unwrap()
            .editor
            .replace("\"preserved workspace\"".to_string());
        app.rebis.as_mut().unwrap().begin_run(RunScope::Program);

        app.open_config_path(path.clone(), false);
        assert!(app.config_editor);
        assert_eq!(
            app.rebis.as_ref().unwrap().editor.source(),
            "KAOS_MODEL = sim\n"
        );
        assert!(!app.rebis.as_ref().unwrap().panel_visible);
        app.background_rebis_workspace()
            .unwrap()
            .push_run_output("streamed behind config");
        assert!(app
            .config_return_rebis
            .as_ref()
            .unwrap()
            .graph_lines(80, 10)
            .iter()
            .any(|line| line.contains("streamed behind config")));

        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("CONFIG"));
        assert!(!screen.contains("INVALID"));

        let workspace = app.rebis.as_mut().unwrap();
        workspace
            .editor
            .replace("KAOS_MODEL = claude:opus\n".to_string());
        workspace.command = "w".to_string();
        assert_eq!(workspace.execute_command(), WorkspaceAction::None);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "KAOS_MODEL = claude:opus\n"
        );

        app.rebis.as_mut().unwrap().command = "q".to_string();
        let action = app.rebis.as_mut().unwrap().execute_command();
        app.handle_rebis_action(action);
        assert!(!app.config_editor);
        assert_eq!(
            app.rebis.as_ref().unwrap().editor.source(),
            "\"preserved workspace\""
        );
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .graph_lines(80, 10)
            .iter()
            .any(|line| line.contains("streamed behind config")));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn rebis_run_streams_and_finishes_while_chat_or_another_panel_is_open() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"background agent\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        app.rebis.as_mut().unwrap().begin_run(RunScope::Program);

        let child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let (tx, rx) = mpsc::channel();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx,
            label: "Rebis background test".to_string(),
            claude_session: false,
            rebis_run_id: Some(id),
            owns_process_group: false,
        });
        app.job_start = Some(Instant::now());

        app.handle_rebis_action(WorkspaceAction::Suspend);
        assert!(app.rebis.is_none());
        assert!(app.job.is_some());
        tx.send(Msg::Line("agent   streamed in chat".to_string()))
            .unwrap();
        app.pump();
        assert!(app.job.is_some());
        assert_eq!(app.rebis_runs[0].output, vec!["agent   streamed in chat"]);
        assert!(app
            .suspended_rebis
            .as_ref()
            .unwrap()
            .graph_lines(200, 20)
            .iter()
            .any(|line| line.contains("streamed in chat")));

        // `/runs` is local in chat: it restores the workspace immediately
        // without queueing behind or interrupting the running subprocess.
        assert!(is_local_command("/runs"));
        app.dispatch("/runs");
        assert!(app.rebis.is_some());
        assert!(app.job.is_some());
        assert!(app.rebis.as_ref().unwrap().graph_focus);
        assert_eq!(app.rebis_run_choice, 0);

        app.rebis.as_mut().unwrap().command = "mandala".to_string();
        let action = app.rebis.as_mut().unwrap().execute_kaos_command();
        app.handle_rebis_action(action);
        tx.send(Msg::Line("agent   streamed behind mandala".to_string()))
            .unwrap();
        app.pump();
        assert_eq!(
            app.rebis_runs[0].output,
            vec![
                "agent   streamed in chat",
                "agent   streamed behind mandala"
            ]
        );
        assert_eq!(
            app.rebis.as_ref().unwrap().visualization,
            Visualization::Mandala
        );

        tx.send(Msg::Done(0)).unwrap();
        app.pump();
        assert!(app.job.is_none());
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Complete);
    }

    #[test]
    fn p_suspends_and_resumes_the_selected_run_subprocess() {
        // The Linux process state character in /proc/<pid>/stat: 'T' is stopped.
        fn proc_state(pid: u32) -> Option<char> {
            let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
            // Field 3 is the state, right after the ")" that closes the comm field.
            let after = stat.rsplit_once(')')?.1;
            after.trim_start().chars().next()
        }

        let mut app = App::new();
        let request = RunRequest {
            source: "\"long agent\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx,
            label: "pause test".to_string(),
            claude_session: false,
            rebis_run_id: Some(id),
            owns_process_group: false,
        });
        app.rebis_run_choice = 0;

        // `p` suspends: the run is flagged paused and the process actually stops.
        assert!(app.toggle_pause_selected_rebis_run());
        assert!(app.rebis_runs[0].paused);
        assert_eq!(
            proc_state(pid),
            Some('T'),
            "the subprocess should be stopped"
        );
        assert_eq!(rebis_run_timer(&app.rebis_runs[0]), "PAUSED");

        // `p` again resumes: the flag clears and the process leaves the stopped state.
        assert!(app.toggle_pause_selected_rebis_run());
        assert!(!app.rebis_runs[0].paused);
        assert_ne!(
            proc_state(pid),
            Some('T'),
            "the subprocess should be running"
        );

        // A run with no live subprocess cannot be paused.
        app.job = None;
        assert!(!app.toggle_pause_selected_rebis_run());

        // Clean up the sleeper.
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }

    #[test]
    fn sigil_chat_rewrites_a_paused_run_without_deleting_its_checkpoint() {
        let mut app = App::new();
        app.open_rebis(None);
        let request = RunRequest {
            source: "(-> \"old\" \"report\")".to_string(),
            input: "retained record".to_string(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let checkpoint = app.rebis_runs[0].checkpoint_path.clone();
        std::fs::write(&checkpoint, "checkpoint sentinel").unwrap();
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx: mpsc::channel().1,
            label: "sigil chat rewrite test".to_string(),
            claude_session: false,
            rebis_run_id: Some(id),
            owns_process_group: false,
        });

        app.handle_rebis_action(WorkspaceAction::OpenSigilChat);
        assert_eq!(app.rebis.as_ref().unwrap().sigil_chat_run_id(), Some(id));
        assert!(app.pause_run_for_sigil_chat(id));
        app.rewrite_rebis_run_source(id, "(-> \"old\" \"revised report\")");
        app.retire_rebis_child_for_rewrite(id);

        assert!(app.job.is_none());
        assert_eq!(
            app.rebis_runs[0].request.source,
            "(-> \"old\" \"revised report\")"
        );
        assert_eq!(app.rebis_runs[0].request.input, "retained record");
        assert!(app.rebis_runs[0].paused);
        assert_eq!(
            std::fs::read_to_string(&checkpoint).unwrap(),
            "checkpoint sentinel"
        );
        let _ = std::fs::remove_file(checkpoint);
    }

    #[test]
    fn completed_god_turn_merges_only_valid_unchanged_base_source() {
        let mut app = App::new();
        app.open_rebis(None);
        let base = "(-> \"inspect\" \"old report\")";
        let revised = "(-> \"inspect\" \"revised report\")";
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.dismiss_chaos_star();
            workspace.editor.replace(base.to_string());
            workspace.refresh();
        }
        let request = RunRequest {
            source: base.to_string(),
            input: "evidence stays".to_string(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let bridge_dir =
            std::env::temp_dir().join(format!("kaos-sigil-chat-merge-test-{}", std::process::id()));
        std::fs::create_dir_all(&bridge_dir).unwrap();
        std::fs::write(bridge_dir.join("sigil.rebis"), revised).unwrap();
        let (job, _tx) = test_job("exit 0", None);

        app.finish_sigil_chat_turn(
            SigilChatJob {
                job,
                base_source: base.to_string(),
                bridge_dir: bridge_dir.clone(),
                run_id: Some(id),
                resume_after: false,
            },
            0,
        );

        assert_eq!(app.rebis.as_ref().unwrap().editor.source(), revised);
        assert_eq!(app.rebis_runs[0].request.source, revised);
        assert_eq!(app.rebis_runs[0].request.input, "evidence stays");
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .sigil_chat_lines()
            .iter()
            .any(|line| line.contains("valid source revision applied")));
        let _ = std::fs::remove_dir_all(bridge_dir);
    }

    #[test]
    fn child_pause_marker_keeps_the_rebis_job_live_and_resumable() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"slow model\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let child = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = child.id();
        let (tx, rx) = mpsc::channel();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx,
            label: "automatic pause test".to_string(),
            claude_session: false,
            rebis_run_id: Some(id),
            owns_process_group: false,
        });
        app.job_start = Some(Instant::now());

        tx.send(Msg::Line(format!(
            "{}model turn timed out after 30s",
            crate::pause::PAUSED_MARKER
        )))
        .unwrap();
        app.pump();

        let run = &app.rebis_runs[0];
        assert!(app.job.is_some(), "a pause must retain the live child");
        assert!(run.paused);
        assert_eq!(run.state, RebisRunState::Running);
        assert_eq!(
            run.pause_reason.as_deref(),
            Some("model turn timed out after 30s")
        );
        assert!(run.output.iter().any(|line| line.contains("p resumes")));
        assert!(run
            .output
            .iter()
            .all(|line| !line.contains(crate::pause::PAUSED_MARKER)));

        assert!(app.toggle_pause_selected_rebis_run());
        assert!(!app.rebis_runs[0].paused);
        assert!(app.job.is_some());
        let _ = Command::new("kill").arg(pid.to_string()).status();
    }

    #[test]
    fn p_relaunches_a_vanished_child_from_its_prompt_checkpoint() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"first\" \"failed prompt\"".to_string(),
            input: "original record".to_string(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);

        // The subprocess is already gone. A non-success status must preserve a
        // resumable run instead of creating a terminal failure state.
        app.finish_rebis_subprocess(Some(id), 9, false);
        assert!(app.rebis_runs[0].paused);
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Running);
        assert!(
            app.has_active_jobs(),
            "the paused serial run keeps its lane"
        );
        assert!(app.job.is_none());

        // Under test current_exe is the test harness, but the production launch
        // path and retained argv/stdin/checkpoint wiring are the same. The job
        // remains owned until pump observes its status.
        assert!(app.toggle_pause_selected_rebis_run());
        assert!(!app.rebis_runs[0].paused);
        assert_eq!(app.rebis_runs[0].request, request);
        assert!(app.job.is_some(), "p must launch the replacement child");
        assert!(app.rebis_runs[0]
            .output
            .iter()
            .any(|line| line.contains("completed prompts replay locally")));

        app.cancel_all_work("test cleanup");
    }

    #[test]
    fn parallel_rebis_jobs_finish_or_pause_independently() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let left = RunRequest {
            source: "\"inspect parser\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let right = RunRequest {
            source: "\"inspect runtime\"".to_string(),
            input: String::new(),
            scope: RunScope::Block,
        };
        let left_id = app.register_rebis_run_with_mode(&left, RebisRunState::Running, true);
        let right_id = app.register_rebis_run_with_mode(&right, RebisRunState::Running, true);
        let (left_job, left_tx) = test_job("exit 0", Some(left_id));
        let (right_job, right_tx) = test_job("exit 0", Some(right_id));
        app.parallel_jobs.extend([left_job, right_job]);

        left_tx
            .send(Msg::Line(
                "\u{1e}FOLD_OPEN\u{1f}Rebis agent 1 · parser".to_string(),
            ))
            .unwrap();
        right_tx
            .send(Msg::Line(
                "\u{1e}FOLD_OPEN\u{1f}Rebis agent 1 · runtime".to_string(),
            ))
            .unwrap();
        left_tx
            .send(Msg::Line("model    parser result".to_string()))
            .unwrap();
        right_tx
            .send(Msg::Line("model    runtime result".to_string()))
            .unwrap();
        left_tx.send(Msg::Done(0)).unwrap();
        right_tx.send(Msg::Done(7)).unwrap();

        app.pump();

        assert!(app.parallel_jobs.is_empty());
        let left = app.rebis_runs.iter().find(|run| run.id == left_id).unwrap();
        let right = app
            .rebis_runs
            .iter()
            .find(|run| run.id == right_id)
            .unwrap();
        assert_eq!(left.state, RebisRunState::Complete);
        assert_eq!(right.state, RebisRunState::Running);
        assert!(right.paused);
        assert!(right
            .pause_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("unexpectedly (7)")));
        assert!(left.parallel && right.parallel);
        assert!(left
            .output
            .iter()
            .any(|line| line.contains("parser result")));
        assert!(!left
            .output
            .iter()
            .any(|line| line.contains("runtime result")));
        assert!(right
            .output
            .iter()
            .any(|line| line.contains("runtime result")));
        assert!(!right
            .output
            .iter()
            .any(|line| line.contains("parser result")));

        let rows = rebis_run_tree_rows(&app.rebis_run_views());
        assert!(rows.iter().any(|row| matches!(
            row,
            RebisRunTreeRow::Section { run, title, .. }
                if *run == 0 && title.contains("parser")
        )));
        assert!(rows.iter().any(|row| matches!(
            row,
            RebisRunTreeRow::Section { run, title, .. }
                if *run == 1 && title.contains("runtime")
        )));
    }

    #[test]
    fn serial_rebis_run_still_queues_behind_parallel_work() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let active = RunRequest {
            source: "\"active parallel\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let active_id = app.register_rebis_run_with_mode(&active, RebisRunState::Running, true);
        let (job, _tx) = test_job("sleep 30", Some(active_id));
        app.parallel_jobs.push(job);

        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"serial follower\"".to_string(),
            input: String::new(),
            scope: RunScope::Block,
        }));

        assert_eq!(app.parallel_jobs.len(), 1);
        assert_eq!(app.queue.len(), 1);
        assert_eq!(app.rebis_runs.len(), 2);
        assert_eq!(app.rebis_runs[1].state, RebisRunState::Queued);
        app.quit_from_ctrl_c();
        assert!(app.parallel_jobs.is_empty());
        assert!(app.queue.is_empty());
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Cancelled);
    }

    #[test]
    fn parallel_chaos_runs_queue_only_their_authority_prompts() {
        let mut app = App::new();
        app.rebis_chaos_mode = true;
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        for prompt in ["first", "second"] {
            app.handle_rebis_action(WorkspaceAction::RunParallel(RunRequest {
                source: format!("\"{prompt}\""),
                input: String::new(),
                scope: RunScope::Program,
            }));
        }

        assert!(app.parallel_jobs.is_empty());
        assert_eq!(app.rebis_runs[0].state, RebisRunState::AwaitingPermission);
        assert_eq!(app.rebis_runs[1].state, RebisRunState::Queued);
        assert_eq!(app.parallel_gate_queue.len(), 1);
        assert!(app.rebis_runs.iter().all(|run| run.parallel));

        app.deny_rebis_authority();
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Cancelled);
        assert_eq!(app.rebis_runs[1].state, RebisRunState::AwaitingPermission);
        assert!(app.parallel_gate_queue.is_empty());
        app.deny_rebis_authority();
        assert!(app.pending_rebis.is_none());
        assert!(app
            .rebis_runs
            .iter()
            .all(|run| run.state == RebisRunState::Cancelled));
    }

    #[test]
    fn entry_renders_only_the_chaos_star_then_reveals_an_empty_editor() {
        let mut workspace = RebisWorkspace::open(PathBuf::from("."), None).unwrap();
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                render_rebis_source(&mut workspace, frame, frame.area());
            })
            .unwrap();

        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!screen.contains("std/reflexion"));
        assert!(screen.contains('◯'));

        workspace.dismiss_chaos_star();
        terminal
            .draw(|frame| {
                render_rebis_source(&mut workspace, frame, frame.area());
            })
            .unwrap();
        let revealed = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(!revealed.contains('◯'));
        assert!(!revealed.contains("std/reflexion"));
        assert_eq!(workspace.editor.source(), "");
    }

    #[test]
    fn initial_rebis_frame_keeps_the_star_in_the_left_editor_pane() {
        let mut app = App::new();
        app.open_rebis(None);
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains('◯'));
        assert!(screen.contains("REBIS"));
        assert!(screen.contains("SOURCE"));
        assert!(screen.contains("SIGILS"));
        assert!(!screen.contains("std/reflexion"));

        let panel_x = app.rebis.as_ref().unwrap().panel_inner.unwrap().0;
        let star_x = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .position(|cell| cell.symbol() == "◯")
            .map(|index| index as u16 % 120)
            .unwrap();
        assert!(star_x < panel_x, "Chaos Star escaped the left editor pane");
    }

    #[test]
    fn rebis_top_bar_lists_every_symbol_and_operators_share_one_color() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("( ) [ ] ~ # ' , $ -> <- ; \""));

        let operator_style = rebis_operator_style();
        for highlight in [
            Highlight::Forward,
            Highlight::Mediate,
            Highlight::Import,
            Highlight::Backflow,
            Highlight::Parenthesis,
        ] {
            assert_eq!(rebis_highlight_style(highlight), operator_style);
        }
    }

    #[test]
    fn rebis_programs_and_blocks_queue_as_immutable_snapshots() {
        let mut app = App::new();
        app.open_rebis(None);
        let workspace = app.rebis.as_mut().unwrap();
        workspace.dismiss_chaos_star();
        workspace.begin_run(RunScope::Program);
        workspace.push_run_output("active trace stays visible");
        // A held authority decision stands in for any active/pending working;
        // both prevent another subprocess from starting immediately.
        app.pending = Some(vec!["code".to_string()]);

        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"captured program\"".to_string(),
            input: "program record".to_string(),
            scope: rebis_workspace::RunScope::Program,
        }));
        app.queue
            .push(QueuedWork::Line("queued chat remains".to_string()));
        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"captured block\"".to_string(),
            input: "block record".to_string(),
            scope: rebis_workspace::RunScope::Block,
        }));

        assert_eq!(app.queue.len(), 3);
        let QueuedWork::Rebis {
            request: program, ..
        } = &app.queue[0]
        else {
            panic!("program should be queued as Rebis work")
        };
        let QueuedWork::Rebis { request: block, .. } = &app.queue[2] else {
            panic!("block should be queued as Rebis work")
        };
        assert_eq!(program.source, "\"captured program\"");
        assert_eq!(program.input, "program record");
        assert_eq!(block.source, "\"captured block\"");
        assert_eq!(block.input, "block record");
        assert_eq!(
            app.rebis.as_ref().unwrap().graph_lines(80, 5),
            vec!["active trace stays visible"]
        );
        assert!(app.rebis.as_ref().unwrap().message.contains("3 in line"));

        // The newest queued run is selected. Queue browsing mirrors the file
        // tree, and deleting it leaves interleaved chat work untouched.
        app.rebis.as_mut().unwrap().graph_focus = true;
        app.on_rebis_key(KeyCode::Char('k'), KeyModifiers::NONE);
        assert_eq!(app.rebis_run_choice, 0);
        app.on_rebis_key(KeyCode::Char('j'), KeyModifiers::NONE);
        assert_eq!(app.rebis_run_choice, 1);
        app.on_rebis_key(KeyCode::Delete, KeyModifiers::NONE);
        assert_eq!(app.queue.len(), 2);
        assert!(
            matches!(&app.queue[0], QueuedWork::Rebis { request, .. } if request.scope == RunScope::Program)
        );
        assert!(matches!(&app.queue[1], QueuedWork::Line(line) if line == "queued chat remains"));
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("block removed"));

        app.on_rebis_key(KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(app.queue.len(), 1);
        assert!(matches!(&app.queue[0], QueuedWork::Line(_)));
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("program removed"));
    }

    #[test]
    fn rebis_agents_wait_for_authority_and_denial_launches_nothing() {
        let mut app = App::new();
        assert!(
            !app.rebis_chaos_mode,
            "one direct tool agent per prompt must be the editor default"
        );
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();

        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"inspect the workspace\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        }));

        assert!(app.job.is_none());
        assert!(app.pending_rebis.is_some());
        assert_eq!(app.rebis_runs[0].state, RebisRunState::AwaitingPermission);
        assert!(app.rebis_runs[0].expanded);
        assert!(app.rebis_runs[0]
            .output
            .iter()
            .any(|line| line.contains("agree to these changes")));
        assert!(app.rebis.as_ref().unwrap().message.contains("y once"));

        app.on_key(KeyCode::Char('n'), KeyModifiers::NONE);
        assert!(app.job.is_none());
        assert!(app.pending_rebis.is_none());
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Cancelled);
        assert!(app.rebis_runs[0]
            .output
            .iter()
            .any(|line| line.contains("permission  denied")));
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("no agent or tool was launched"));
    }

    #[test]
    fn remembered_rebis_authority_ends_with_the_current_sigil() {
        let mut app = App::new();
        let previous = app.session_id.clone();
        app.rebis_authority = true;

        app.dispatch("/new");

        assert_ne!(app.session_id, previous);
        assert!(!app.rebis_authority);
    }

    #[test]
    fn queued_runs_are_visible_in_the_right_panel_browser() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.pending = Some(vec!["code".to_string()]);
        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"design the experiment\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        }));
        app.handle_rebis_action(WorkspaceAction::Run(RunRequest {
            source: "\"critique the controls\"".to_string(),
            input: String::new(),
            scope: RunScope::Block,
        }));
        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();

        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("RUNS"));
        assert!(screen.contains("QUEUED"));
        assert!(screen.contains("0.0s"));
        assert!(screen.contains("PROGRAM"));
        assert!(screen.contains("design the experiment"));
        assert!(screen.contains("BLOCK"));

        // Rows are mouse-selectable just like entries in the sigil tree.
        let (x, y, width, height) = app.rebis.as_ref().unwrap().panel_inner.unwrap();
        let views = app.rebis_run_views();
        let rows = rebis_run_tree_rows(&views);
        let (area, start, _) = rebis_run_browser_layout(
            Rect::new(x, y, width, height),
            rows.len(),
            app.rebis_run_top,
        )
        .unwrap();
        app.on_rebis_click(area.x + 2, area.y + 1, (120, 30));
        let expected = match rows[start] {
            RebisRunTreeRow::Header(run)
            | RebisRunTreeRow::Section { run, .. }
            | RebisRunTreeRow::Output { run, .. } => run,
        };
        assert_eq!(app.rebis_run_choice, expected);
    }

    #[test]
    fn run_timer_tracks_waiting_and_freezes_the_execution_duration() {
        let mut app = App::new();
        let request = RunRequest {
            source: "\"timed run\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Queued);
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.queued_at = Instant::now() - Duration::from_millis(2_340);
        assert!(rebis_run_timer(run).starts_with("WAIT 2.3s"));

        run.state = RebisRunState::Running;
        run.started_at = Some(Instant::now() - Duration::from_secs(65));
        assert_eq!(rebis_run_timer(run), "TIME 1m 05s");

        run.paused_total = Duration::from_secs(5);
        assert_eq!(rebis_run_timer(run), "TIME 1m 00s");

        finish_rebis_run_clock(run);
        run.state = RebisRunState::Complete;
        let frozen = rebis_run_timer(run);
        assert_eq!(frozen, "TIME 1m 00s");
        assert!(run.elapsed.is_some());
    }

    #[test]
    fn completed_runs_remain_expandable_until_removed() {
        let mut app = App::new();
        app.open_rebis(None);
        let workspace = app.rebis.as_mut().unwrap();
        workspace.dismiss_chaos_star();
        workspace.runs_visible = true;
        workspace.graph_focus = true;
        let request = RunRequest {
            source: "(std/reflexion reflect \"kept\")".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Running);
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.state = RebisRunState::Complete;
        run.expanded = false;
        run.output = vec!["first streamed line".to_string(), "final value".to_string()];

        app.on_rebis_key(KeyCode::Tab, KeyModifiers::NONE);
        assert!(app.rebis_runs[0].expanded);
        assert!(rebis_run_tree_rows(&app.rebis_run_views())
            .iter()
            .any(|row| matches!(row, RebisRunTreeRow::Output { text, .. } if text == "first streamed line")));

        app.on_rebis_key(KeyCode::Delete, KeyModifiers::NONE);
        assert!(app.rebis_runs.is_empty());
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("removed from run history"));
    }

    #[test]
    fn expanded_runs_group_chat_equivalent_activity_by_agent_and_step() {
        let request = RunRequest {
            source: "(\"inspect\" \"repair\")".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let mut app = App::new();
        app.open_rebis(None);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.dismiss_chaos_star();
            workspace.begin_run(RunScope::Program);
        }
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = vec![
            "event    module std/reflexion loaded · 3 definition(s)".to_string(),
            "\u{1e}FOLD_OPEN\u{1f}Rebis agent 1 · inspect".to_string(),
            "model    generating turn 1".to_string(),
            "\u{1e}FOLD_OPEN\u{1f}model turn 1 · complete response".to_string(),
            "model    I will inspect the parser first.".to_string(),
            "\u{1e}FOLD_CLOSE".to_string(),
            "     ☽ I will inspect the parser first.".to_string(),
            "   1 ± edit src/parser.rs".to_string(),
            "\u{1e}FOLD_OPEN\u{1f}⋯ step 1 in full — edit src/parser.rs".to_string(),
            "  the exact change:".to_string(),
            "    - old".to_string(),
            "    + new".to_string(),
            "\u{1e}FOLD_CLOSE".to_string(),
            "   2 $ cargo test".to_string(),
            "model    parser repaired and verified".to_string(),
            "\u{1e}FOLD_CLOSE".to_string(),
        ];

        let rows = rebis_run_tree_rows(&app.rebis_run_views());
        assert!(rows.iter().any(|row| matches!(
            row,
            RebisRunTreeRow::Section { depth: 0, title, .. }
                if title == "Rebis agent 1 · inspect"
        )));
        assert!(rows.iter().any(|row| matches!(
            row,
            RebisRunTreeRow::Section { depth: 1, title, .. }
                if title.contains("step 1 in full")
        )));
        let event = rows.iter().position(
            |row| matches!(row, RebisRunTreeRow::Output { text, .. } if text.contains("module std/reflexion")),
        ).unwrap();
        let agent = rows
            .iter()
            .position(|row| matches!(row, RebisRunTreeRow::Section { depth: 0, .. }))
            .unwrap();
        let model = rows.iter().position(
            |row| matches!(row, RebisRunTreeRow::Section { title, .. } if title.starts_with("model turn 1")),
        ).unwrap();
        let step = rows.iter().position(
            |row| matches!(row, RebisRunTreeRow::Section { title, .. } if title.contains("step 1 in full")),
        ).unwrap();
        assert!(event < agent && agent < model && model < step);
        for expected in [
            "I will inspect",
            "edit src/parser.rs",
            "cargo test",
            "parser repaired and verified",
        ] {
            assert!(rows.iter().any(
                |row| matches!(row, RebisRunTreeRow::Output { text, .. } if text.contains(expected))
            ));
        }

        let backend = ratatui::backend::TestBackend::new(120, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("◆ AGENT"));
        assert!(screen.contains("◇ MODEL"));
        assert!(screen.contains("◇ STEP"));
        assert!(!screen.contains("FOLD_OPEN"));
    }

    #[test]
    fn runs_command_returns_from_mandala_to_the_active_agent() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let completed = RunRequest {
            source: "\"finished\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let active = RunRequest {
            source: "\"active agent\"".to_string(),
            input: String::new(),
            scope: RunScope::Block,
        };
        app.register_rebis_run(&completed, RebisRunState::Complete);
        app.register_rebis_run(&active, RebisRunState::Running);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.visualization = Visualization::Mandala;
            workspace.panel_visible = false;
            workspace.graph_focus = false;
            workspace.command = "runs".to_string();
        }

        let action = app.rebis.as_mut().unwrap().execute_kaos_command();
        app.handle_rebis_action(action);

        assert_eq!(app.rebis_run_choice, 1);
        assert!(app.rebis.as_ref().unwrap().panel_visible);
        assert!(app.rebis.as_ref().unwrap().runs_visible);
        assert!(app.rebis.as_ref().unwrap().graph_focus);
        assert!(app.rebis.as_ref().unwrap().message.contains("running"));
    }

    #[test]
    fn expanded_run_output_wraps_without_losing_the_right_hand_side() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.rebis.as_mut().unwrap().runs_visible = true;
        app.rebis.as_mut().unwrap().graph_focus = true;
        let request = RunRequest {
            source: "\"wide output\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let output = format!("BEGIN-{}-END-UNTRUNCATED", "x".repeat(160));
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = vec![output.clone()];
        let backend = ratatui::backend::TestBackend::new(100, 30);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal.draw(|frame| app.draw(frame)).unwrap();
        let first_view = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(first_view.contains("BEGIN-"));
        assert!(first_view.contains("-END"));
        assert!(first_view.contains("-UNTRUNCATED"));

        let wrapped = rebis_run_display_rows(&app.rebis_run_views(), 44);
        let reconstructed = wrapped
            .iter()
            .filter_map(|row| match row {
                RebisRunTreeRow::Output { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<String>();
        assert_eq!(reconstructed, output);
    }

    #[test]
    fn long_agent_heading_wraps_without_losing_prompt_text() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"long agent prompt\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let title = "Rebis agent 1 · Design a falsifiable ritual whose complete instruction must remain visible";
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = vec![format!("\u{1e}FOLD_OPEN\u{1f}{title}")];

        let rows = rebis_run_display_rows(&app.rebis_run_views(), 34);
        let chunks = rows
            .iter()
            .filter_map(|row| match row {
                RebisRunTreeRow::Section { title, .. } => Some(title.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert!(chunks.len() > 1);
        assert_eq!(chunks.concat(), title);
    }

    #[test]
    fn wrapped_model_heading_remains_a_model_tree_node() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"model reply\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let title = "model turn 123 · complete response that wraps across several narrow rows";
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = vec![format!("\u{1e}FOLD_OPEN\u{1f}{title}")];

        let rows = rebis_run_display_rows(&app.rebis_run_views(), 32);
        let model_chunks = rows
            .iter()
            .filter(|row| {
                matches!(
                    row,
                    RebisRunTreeRow::Section {
                        kind: RebisRunSectionKind::Model,
                        ..
                    }
                )
            })
            .count();
        assert!(model_chunks > 1);
    }

    #[test]
    fn shifted_arrows_jump_to_log_ends_while_plain_arrows_scroll_run_output() {
        let mut app = App::new();
        app.open_rebis(None);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.dismiss_chaos_star();
            workspace.runs_visible = true;
            workspace.graph_focus = true;
        }
        let request = RunRequest {
            source: "\"long output\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = (0..60)
            .map(|index| format!("retained output line {index:02}"))
            .collect();
        let backend = ratatui::backend::TestBackend::new(100, 20);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let max_top = app.rebis_run_max_top();
        assert!(max_top > 10);
        assert!(app.rebis.as_ref().unwrap().graph_focus);

        app.on_rebis_key(KeyCode::Down, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, 1);
        app.on_rebis_key(KeyCode::Down, KeyModifiers::SHIFT);
        assert_eq!(app.rebis_run_top, max_top);
        assert_eq!(app.rebis_run_choice, 0);
        app.on_rebis_key(KeyCode::Up, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, max_top - 1);
        app.on_rebis_key(KeyCode::Up, KeyModifiers::SHIFT);
        assert_eq!(app.rebis_run_top, 0);
        assert_eq!(app.rebis_run_choice, 0);
        app.on_rebis_key(KeyCode::Down, KeyModifiers::SHIFT);
        assert_eq!(app.rebis_run_top, max_top);
        app.on_rebis_key(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, 0);
        app.on_rebis_key(KeyCode::PageDown, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, 10);
        app.on_rebis_key(KeyCode::End, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, max_top);

        terminal.draw(|frame| app.draw(frame)).unwrap();
        let final_view = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(final_view.contains("retained output line 59"));

        app.on_rebis_key(KeyCode::Home, KeyModifiers::NONE);
        assert_eq!(app.rebis_run_top, 0);
    }

    #[test]
    fn active_run_must_be_cancelled_before_it_can_be_removed() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.rebis.as_mut().unwrap().runs_visible = true;
        app.rebis.as_mut().unwrap().graph_focus = true;
        let request = RunRequest {
            source: "\"still working\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        app.register_rebis_run(&request, RebisRunState::Running);

        app.on_rebis_key(KeyCode::Delete, KeyModifiers::NONE);

        assert_eq!(app.rebis_runs.len(), 1);
        assert_eq!(app.rebis_runs[0].state, RebisRunState::Running);
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("active run cannot be removed"));
    }

    #[test]
    fn ctrl_w_moves_between_source_and_mandala_windows() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().vim_enabled = true;
        app.rebis.as_mut().unwrap().mode = RebisMode::Normal;
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.on_rebis_key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        app.on_rebis_key(KeyCode::Char('l'), KeyModifiers::NONE);
        assert!(app.rebis.as_ref().unwrap().graph_focus);
        app.on_rebis_key(KeyCode::Char('w'), KeyModifiers::CONTROL);
        app.on_rebis_key(KeyCode::Char('h'), KeyModifiers::NONE);
        assert!(!app.rebis.as_ref().unwrap().graph_focus);
    }

    #[test]
    fn direct_editing_is_default_and_vim_can_be_enabled_by_command() {
        let mut app = App::new();
        app.open_rebis(None);
        assert!(app.rebis.as_ref().unwrap().chaos_star_visible());
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.vim_enabled = false;
            workspace.mode = RebisMode::Insert;
        }
        app.on_rebis_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(!app.rebis.as_ref().unwrap().chaos_star_visible());
        assert!(!app.rebis.as_ref().unwrap().editor.source().starts_with('x'));
        // The dismissing interaction is consumed; the next one edits normally.
        app.on_rebis_key(KeyCode::Char('x'), KeyModifiers::NONE);
        assert!(app.rebis.as_ref().unwrap().editor.source().starts_with('x'));
        app.rebis.as_mut().unwrap().command = "vim on".to_string();
        app.rebis.as_mut().unwrap().execute_kaos_command();
        assert!(app.rebis.as_ref().unwrap().vim_enabled);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Normal);
    }

    #[test]
    fn vim_cw_changes_a_word_and_undo_restores_the_whole_change() {
        let mut app = App::new();
        app.open_rebis(None);
        let workspace = app.rebis.as_mut().unwrap();
        workspace.dismiss_chaos_star();
        workspace.vim_enabled = true;
        workspace.mode = RebisMode::Normal;
        workspace.editor = rebis_workspace::Editor::new("alpha beta");

        app.on_rebis_key(KeyCode::Char('c'), KeyModifiers::NONE);
        app.on_rebis_key(KeyCode::Char('w'), KeyModifiers::NONE);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Insert);
        for character in "omega".chars() {
            app.on_rebis_key(KeyCode::Char(character), KeyModifiers::NONE);
        }
        app.on_rebis_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.rebis.as_ref().unwrap().editor.source(), "omega beta");
        assert_eq!(app.rebis.as_ref().unwrap().editor.cursor(), 4);

        app.on_rebis_key(KeyCode::Char('u'), KeyModifiers::NONE);
        assert_eq!(app.rebis.as_ref().unwrap().editor.source(), "alpha beta");
    }

    #[test]
    fn vim_insert_entry_and_escape_follow_normal_mode_boundaries() {
        let mut app = App::new();
        app.open_rebis(None);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.dismiss_chaos_star();
            workspace.vim_enabled = true;
            workspace.mode = RebisMode::Normal;
            workspace.editor = rebis_workspace::Editor::new("  alpha\n\nbeta");
        }

        app.on_rebis_key(KeyCode::Char('I'), KeyModifiers::SHIFT);
        assert_eq!(app.rebis.as_ref().unwrap().editor.row_col(), (0, 2));
        app.on_rebis_key(KeyCode::Char('X'), KeyModifiers::SHIFT);
        app.on_rebis_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Normal);
        assert!(!app.quit);
        assert_eq!(
            app.rebis.as_ref().unwrap().editor.source(),
            "  Xalpha\n\nbeta"
        );

        // `a` on an empty line inserts before its newline instead of crossing
        // into the following line.
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.editor = rebis_workspace::Editor::new("\nbeta");
            workspace.mode = RebisMode::Normal;
        }
        app.on_rebis_key(KeyCode::Char('a'), KeyModifiers::NONE);
        app.on_rebis_key(KeyCode::Char('X'), KeyModifiers::SHIFT);
        app.on_rebis_key(KeyCode::Esc, KeyModifiers::NONE);
        assert_eq!(app.rebis.as_ref().unwrap().editor.source(), "X\nbeta");
    }

    #[test]
    fn first_command_shortcut_only_dismisses_the_editor_star() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().vim_enabled = false;
        app.rebis.as_mut().unwrap().mode = RebisMode::Insert;

        app.on_rebis_key(KeyCode::Char('/'), KeyModifiers::CONTROL);

        // The entry veil consumes commands too: the first shortcut only
        // dismisses the star and leaves both source and mode untouched.
        assert!(!app.rebis.as_ref().unwrap().chaos_star_visible());
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Insert);
        // Legacy terminals send the Ctrl-/ unit-separator byte, which
        // Crossterm normalizes to Ctrl-7.
        app.on_rebis_key(KeyCode::Char('7'), KeyModifiers::CONTROL);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::KaosCommand);
        assert_eq!(app.rebis.as_ref().unwrap().editor.source(), "");
    }

    #[test]
    fn bare_slash_is_source_text_in_the_rebis_editor() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().vim_enabled = false;
        app.rebis.as_mut().unwrap().mode = RebisMode::Insert;
        app.rebis.as_mut().unwrap().dismiss_chaos_star();

        app.on_rebis_key(KeyCode::Char('/'), KeyModifiers::NONE);

        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Insert);
        assert!(app.rebis.as_ref().unwrap().editor.source().starts_with('/'));
    }

    #[test]
    fn paste_and_vim_interaction_dismiss_the_editor_star() {
        let mut pasted = App::new();
        pasted.open_rebis(None);
        pasted.rebis.as_mut().unwrap().vim_enabled = false;
        pasted.rebis.as_mut().unwrap().mode = RebisMode::Insert;
        pasted.on_paste("\"consumed\"");
        // The first paste only lifts the star; none of its text is inserted.
        assert!(!pasted.rebis.as_ref().unwrap().chaos_star_visible());
        assert!(!pasted
            .rebis
            .as_ref()
            .unwrap()
            .editor
            .source()
            .contains("consumed"));
        pasted.on_paste("\"pasted\"");
        let source = pasted.rebis.as_ref().unwrap().editor.source();
        assert_eq!(source, "\"pasted\"");

        let mut vim = App::new();
        vim.open_rebis(None);
        vim.rebis.as_mut().unwrap().vim_enabled = true;
        vim.rebis.as_mut().unwrap().mode = RebisMode::Normal;
        vim.on_rebis_key(KeyCode::Char('i'), KeyModifiers::NONE);
        // The first i lifts the star without also entering Insert.
        assert!(!vim.rebis.as_ref().unwrap().chaos_star_visible());
        assert_eq!(vim.rebis.as_ref().unwrap().mode, RebisMode::Normal);
        vim.on_rebis_key(KeyCode::Char('i'), KeyModifiers::NONE);
        assert_eq!(vim.rebis.as_ref().unwrap().mode, RebisMode::Insert);
        assert_eq!(vim.rebis.as_ref().unwrap().editor.source(), "");
    }

    #[test]
    fn ctrl_v_inserts_command_characters_literally() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().vim_enabled = false;
        app.rebis.as_mut().unwrap().mode = RebisMode::Insert;
        app.rebis.as_mut().unwrap().dismiss_chaos_star();

        app.on_rebis_key(KeyCode::Char('v'), KeyModifiers::CONTROL);
        app.on_rebis_key(KeyCode::Char('/'), KeyModifiers::NONE);

        assert!(app.rebis.as_ref().unwrap().editor.source().starts_with('/'));
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Insert);
    }

    #[test]
    fn ctrl_v_enters_and_toggles_visual_block_mode() {
        let mut app = App::new();
        app.open_rebis(None);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.vim_enabled = true;
            workspace.mode = RebisMode::Normal;
            workspace.dismiss_chaos_star();
        }

        // Ctrl-V from Normal opens a rectangular block selection, distinct from
        // `v` (character-wise) and `V` (line-wise).
        app.on_rebis_key(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::VisualBlock);

        // A second Ctrl-V toggles the block selection back off.
        app.on_rebis_key(KeyCode::Char('v'), KeyModifiers::CONTROL);
        assert_eq!(app.rebis.as_ref().unwrap().mode, RebisMode::Normal);
    }

    #[test]
    fn ctrl_c_asks_before_quitting_when_idle() {
        let mut app = App::new();
        app.open_rebis(None);

        // The first idle ^C in the workspace arms the quit confirmation.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "the first idle ^C asks before quitting");
        assert!(app.confirm_quit);
        // The confirming second ^C exits.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit);

        // Idle chat behaves the same: ask, then confirm.
        let mut chat = App::new();
        chat.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!chat.quit, "the first idle ^C only asks");
        chat.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(chat.quit, "an idle, empty chat exits on the confirming ^C");

        // A pending permission question is cancelled, not the whole app; only
        // once idle does ^C begin the ask-then-confirm exit.
        let mut permission_prompt = App::new();
        permission_prompt.pending = Some(vec!["code".to_string()]);
        permission_prompt.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!permission_prompt.quit);
        assert!(permission_prompt.pending.is_none());
        permission_prompt.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!permission_prompt.quit, "next idle ^C only asks");
        permission_prompt.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(permission_prompt.quit);
    }

    #[test]
    fn workspace_ctrl_c_stops_a_run_instead_of_quitting() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let child = Command::new("sh")
            .arg("-c")
            .arg("sleep 30")
            .spawn()
            .unwrap();
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx,
            label: "rebis run".to_string(),
            claude_session: false,
            rebis_run_id: None,
            owns_process_group: false,
        });
        app.job_start = Some(Instant::now());

        // ^C with a run in flight is STOP, not quit: the child is killed and the
        // app stays open, so it never begins the exit confirmation.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "^C stops the run, it does not quit the app");
        assert!(app.job.is_none(), "the run's child was cancelled");
        assert!(!app.confirm_quit);
    }

    #[test]
    fn a_non_ctrl_c_key_cancels_a_pending_quit_confirmation() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();

        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.confirm_quit, "idle ^C arms the confirmation");

        // Any other key answers "stay": the confirmation disarms, so the next ^C
        // starts the ask again rather than quitting outright.
        app.on_key(KeyCode::Right, KeyModifiers::NONE);
        assert!(!app.confirm_quit);
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "after disarming, ^C only re-asks");
        assert!(app.confirm_quit);
    }

    #[test]
    fn chat_ctrl_c_clears_typed_input_before_quitting() {
        let mut app = App::new();
        app.input = "half-typed intent".to_string();
        app.cursor = app.input.chars().count();

        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "^C over a draft only discards the draft");
        assert!(app.input.is_empty());
        assert_eq!(app.cursor, 0);

        // Now idle: the next ^C asks, and the one after confirms.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "idle chat: the next ^C asks");
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit, "the confirming ^C exits");
    }

    #[test]
    fn chat_ctrl_c_cancels_active_work_without_quitting() {
        let mut app = App::new();
        let child = Command::new("sh").arg("-c").arg("exit 0").spawn().unwrap();
        let (_tx, rx) = mpsc::channel();
        app.job = Some(Job {
            child: Arc::new(Mutex::new(child)),
            rx,
            label: "active test work".to_string(),
            claude_session: false,
            rebis_run_id: None,
            owns_process_group: false,
        });
        app.job_start = Some(Instant::now());
        app.input = "the next intent, mid-typing".to_string();
        app.cursor = app.input.chars().count();

        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);

        assert!(!app.quit, "^C stops the message, not the app");
        assert!(app.job.is_none());
        assert!(app.job_start.is_none());
        assert_eq!(
            app.input, "the next intent, mid-typing",
            "cancelling work must not eat the draft"
        );

        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit);
        assert!(app.input.is_empty());
        // Idle now: ask, then confirm.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "the first idle ^C asks");
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit, "an idle chat exits on the confirming ^C");
    }

    #[test]
    fn mouse_wheel_target_tracks_the_rebis_pane_under_pointer() {
        let mut workspace = RebisWorkspace::open(PathBuf::from("."), None).unwrap();
        assert!(!mouse_over_rebis_graph(&workspace, 20, 10, (100, 30)));
        assert!(mouse_over_rebis_graph(&workspace, 80, 10, (100, 30)));
        assert!(!mouse_over_rebis_graph(&workspace, 20, 5, (70, 30)));
        assert!(mouse_over_rebis_graph(&workspace, 20, 24, (70, 30)));

        // Once drawn, exact pane geometry wins over percentage estimates and
        // includes the panel border without stealing the adjacent source row.
        workspace.panel_inner = Some((1, 18, 68, 9));
        assert!(!mouse_over_rebis_graph(&workspace, 20, 16, (70, 30)));
        assert!(mouse_over_rebis_graph(&workspace, 20, 17, (70, 30)));
    }

    #[test]
    fn mouse_wheel_scroll_persists_away_from_the_source_cursor_and_is_bounded() {
        let mut app = App::new();
        app.open_rebis(None);
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.dismiss_chaos_star();
            workspace.editor = rebis_workspace::Editor::new(
                (0..80)
                    .map(|line| format!("line {line:02}"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            workspace.refresh();
        }
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let source = app
            .text_panes
            .iter()
            .find(|pane| pane.kind == TextPaneKind::RebisSource)
            .unwrap()
            .clone();
        let column = source.area.x + 1;
        let row = source.area.y + 1;

        app.on_mouse_scroll(1, column, row, KeyModifiers::NONE, (120, 24));
        assert_eq!(app.rebis.as_ref().unwrap().view_top, 3);
        terminal.draw(|frame| app.draw(frame)).unwrap();
        assert_eq!(
            app.rebis.as_ref().unwrap().view_top,
            3,
            "drawing must not snap a manually scrolled viewport back to the cursor"
        );

        app.on_mouse_scroll(100, column, row, KeyModifiers::NONE, (120, 24));
        let top = app.rebis.as_ref().unwrap().view_top;
        app.on_mouse_scroll(1, column, row, KeyModifiers::NONE, (120, 24));
        assert_eq!(app.rebis.as_ref().unwrap().view_top, top);
        app.on_mouse_scroll(-1, column, row, KeyModifiers::NONE, (120, 24));
        assert_eq!(app.rebis.as_ref().unwrap().view_top, top - 3);
    }

    #[test]
    fn mouse_wheel_scrolls_the_drawn_run_pane_in_both_directions() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        let request = RunRequest {
            source: "\"wheel output\"".to_string(),
            input: String::new(),
            scope: RunScope::Program,
        };
        let id = app.register_rebis_run(&request, RebisRunState::Complete);
        let run = app.rebis_runs.iter_mut().find(|run| run.id == id).unwrap();
        run.expanded = true;
        run.output = (0..60).map(|line| format!("log {line:02}")).collect();
        {
            let workspace = app.rebis.as_mut().unwrap();
            workspace.runs_visible = true;
            workspace.graph_focus = true;
        }
        let backend = ratatui::backend::TestBackend::new(120, 24);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let panel = app
            .text_panes
            .iter()
            .find(|pane| pane.kind == TextPaneKind::RebisPanel)
            .unwrap()
            .clone();
        let column = panel.area.x + 1;
        let row = panel.area.y + 1;

        app.on_mouse_scroll(1, column, row, KeyModifiers::NONE, (120, 24));
        assert_eq!(app.rebis_run_top, 3);
        app.on_mouse_scroll(-1, column, row, KeyModifiers::NONE, (120, 24));
        assert_eq!(app.rebis_run_top, 0);
    }

    #[test]
    fn mouse_command_toggles_capture_from_chat_and_rebis() {
        let mut app = App::new();
        assert!(app.mouse_captured, "pane-local selection is the default");
        app.dispatch("/mouse");
        assert!(!app.mouse_captured);
        app.dispatch("/mouse on");
        assert!(app.mouse_captured);
        // From the Rebis workspace the command routes through the Kaos seam
        // and the status line reports the new state.
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().command = "mouse on".to_string();
        let action = app.rebis.as_mut().unwrap().execute_kaos_command();
        app.handle_rebis_action(action);
        assert!(app.mouse_captured);
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("within one pane"));
    }

    #[test]
    fn mouse_selection_is_clipped_to_the_pane_where_the_drag_began() {
        let mut app = App::new();
        app.text_panes = vec![
            TextPane {
                kind: TextPaneKind::RebisSource,
                area: Rect::new(0, 0, 5, 2),
                content_left: 0,
                rows: ["left1", "left2"]
                    .into_iter()
                    .map(|line| line.chars().map(|ch| ch.to_string()).collect())
                    .collect(),
            },
            TextPane {
                kind: TextPaneKind::RebisPanel,
                area: Rect::new(5, 0, 5, 2),
                content_left: 5,
                rows: ["right", "panel"]
                    .into_iter()
                    .map(|line| line.chars().map(|ch| ch.to_string()).collect())
                    .collect(),
            },
        ];

        assert!(app.begin_pane_selection(1, 0));
        assert!(app.drag_pane_selection(99, 99));
        let (dragged, pane, text) = app.finish_pane_selection(99, 99).unwrap();

        assert!(dragged);
        assert_eq!(pane, TextPaneKind::RebisSource);
        assert_eq!(text, "eft1\nleft2");
        assert!(!text.contains("right"));
        assert_eq!(
            app.text_selection.as_ref().unwrap().head,
            Position::new(4, 1)
        );

        app.on_key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );
        assert!(!app.quit);
        assert!(app.activity.contains("copied 10 character(s) from source"));
        assert!(app.text_selection.is_some());
    }

    #[test]
    fn dragging_a_chat_selection_stops_following_the_live_tail() {
        let mut app = App::new();
        app.text_panes = vec![TextPane {
            kind: TextPaneKind::Chat,
            area: Rect::new(0, 0, 5, 2),
            content_left: 0,
            rows: ["alpha", "bravo"]
                .into_iter()
                .map(|line| line.chars().map(|ch| ch.to_string()).collect())
                .collect(),
        }];

        // A bare click (down then up on the same cell) must not disturb the
        // live tail — output should keep auto-scrolling.
        assert!(app.follow);
        assert!(app.begin_pane_selection(1, 0));
        app.finish_pane_selection(1, 0);
        assert!(app.follow, "a bare click must not stop following");

        // A genuine drag-selection freezes the tail so the transcript does not
        // scroll out from under the highlighted range while output streams in.
        assert!(app.begin_pane_selection(1, 0));
        assert!(app.drag_pane_selection(4, 1));
        assert!(
            !app.follow,
            "a chat drag-selection must stop following the live tail"
        );
    }

    #[test]
    fn source_selection_excludes_the_line_number_gutter() {
        let mut app = App::new();
        app.text_panes = vec![TextPane {
            kind: TextPaneKind::RebisSource,
            area: Rect::new(0, 0, 9, 2),
            content_left: 4,
            rows: [" 1 │alpha", " 2 │beta "]
                .into_iter()
                .map(|line| {
                    line.chars()
                        .map(|character| character.to_string())
                        .collect()
                })
                .collect(),
        }];

        // Starting the drag on a line number is allowed, but its anchor snaps
        // to the first document column and copied text contains no gutter.
        assert!(app.begin_pane_selection(0, 0));
        assert_eq!(
            app.text_selection.as_ref().unwrap().anchor,
            Position::new(4, 0)
        );
        assert!(app.drag_pane_selection(8, 1));
        let (_, pane, text) = app.finish_pane_selection(8, 1).unwrap();

        assert_eq!(pane, TextPaneKind::RebisSource);
        assert_eq!(text, "alpha\nbeta");
        assert!(!text.contains('1'));
        assert!(!text.contains('2'));
        assert!(!text.contains('│'));
    }

    #[test]
    fn ctrl_shift_c_copies_a_rebis_selection_before_ctrl_c_can_cancel() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.text_panes = vec![TextPane {
            kind: TextPaneKind::RebisSource,
            area: Rect::new(0, 0, 6, 1),
            content_left: 0,
            rows: vec!["source"
                .chars()
                .map(|character| character.to_string())
                .collect()],
        }];
        assert!(app.begin_pane_selection(0, 0));
        assert!(app.drag_pane_selection(5, 0));
        assert!(app.finish_pane_selection(5, 0).unwrap().0);

        app.on_key(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT,
        );

        assert!(!app.quit);
        assert!(app
            .rebis
            .as_ref()
            .unwrap()
            .message
            .contains("copied 6 character(s) from source"));
        assert!(app.text_selection.is_some());
        assert!(selection_copy_shortcut(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL | KeyModifiers::SHIFT
        ));
        assert!(!selection_copy_shortcut(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL
        ));
    }

    #[test]
    fn plain_ctrl_c_quits_even_with_a_right_panel_selection() {
        let mut app = App::new();
        app.open_rebis(None);
        app.rebis.as_mut().unwrap().dismiss_chaos_star();
        app.text_panes = vec![TextPane {
            kind: TextPaneKind::RebisPanel,
            area: Rect::new(0, 0, 12, 1),
            content_left: 0,
            rows: vec!["run output 7"
                .chars()
                .map(|character| character.to_string())
                .collect()],
        }];
        assert!(app.begin_pane_selection(0, 0));
        assert!(app.drag_pane_selection(11, 0));
        assert!(app.finish_pane_selection(11, 0).unwrap().0);

        // ^C is not swallowed by an active pane selection: it drives the exit
        // (ask, then confirm) rather than being treated as a copy.
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(!app.quit, "the first ^C asks before quitting");
        assert!(app.confirm_quit);
        app.on_key(KeyCode::Char('c'), KeyModifiers::CONTROL);
        assert!(app.quit);
        assert!(!app.rebis.as_ref().unwrap().message.contains("copied"));
        assert!(app.text_selection.is_some());
    }

    #[test]
    fn click_focuses_the_rebis_pane_under_the_pointer() {
        let mut app = App::new();
        app.open_rebis(None);
        assert!(!app.rebis.as_ref().unwrap().graph_focus);
        app.on_rebis_click(80, 10, (100, 30));
        assert!(!app.rebis.as_ref().unwrap().graph_focus);
        app.on_rebis_click(80, 10, (100, 30));
        assert!(app.rebis.as_ref().unwrap().graph_focus);
        app.on_rebis_click(20, 10, (100, 30));
        assert!(!app.rebis.as_ref().unwrap().graph_focus);
        // With the panel hidden the whole surface is source — a click on the
        // right half must not focus a pane that is not on screen.
        app.rebis.as_mut().unwrap().panel_visible = false;
        app.on_rebis_click(80, 10, (100, 30));
        assert!(!app.rebis.as_ref().unwrap().graph_focus);
    }

    #[test]
    fn wrap_line_wraps_and_preserves_content() {
        let long = "the quick brown fox jumps over the lazy dog again and again";
        let line = Line::from(long);
        let rows = wrap_line(&line, 20);
        assert!(rows.len() > 1, "should wrap into multiple rows");
        for r in &rows {
            let w: usize = r.spans.iter().map(|s| s.content.chars().count()).sum();
            assert!(w <= 20, "row too wide: {w}");
        }
        // No characters lost (spaces at break points may shift but content survives).
        let joined: String = rows
            .iter()
            .flat_map(|r| r.spans.iter().map(|s| s.content.to_string()))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!(joined.replace(' ', ""), long.replace(' ', ""));
    }

    #[test]
    fn wrap_line_short_is_untouched() {
        let line = Line::from("short");
        assert_eq!(wrap_line(&line, 40).len(), 1);
        assert_eq!(wrap_line(&line, 0).len(), 1); // unknown width → one row
    }

    #[test]
    fn multiline_chat_prompt_wraps_losslessly_and_keeps_its_tail_visible() {
        let prompt = (0..14)
            .map(|index| format!("specialist line {index:02}\n"))
            .collect::<String>()
            + "VISIBLE-TAIL";
        let mut app = App::new();
        app.on_paste(&prompt);
        assert_eq!(app.input, prompt);

        let mut cells = "✴ ❯ "
            .chars()
            .map(|character| (character, red_bold()))
            .collect::<Vec<_>>();
        cells.extend(prompt.chars().map(|character| (character, Style::new())));
        let rows = hard_wrap(&cells, 32);
        let displayed = rows
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert_eq!(
            displayed,
            format!("✴ ❯ {}", prompt.replace('\n', "")),
            "wrapping must not discard prompt characters"
        );
        assert_eq!(wrapped_cursor(&cells, cells.len(), 32), (14, 12));

        let backend = ratatui::backend::TestBackend::new(32, 10);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();
        terminal.draw(|frame| app.draw(frame)).unwrap();
        let screen = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(|cell| cell.symbol())
            .collect::<String>();
        assert!(screen.contains("VISIBLE-TAIL"));

        app.echo_prompt(&prompt);
        let echoed = app.rendered_lines().pop().unwrap();
        let echoed_rows = wrap_line(&echoed, 32);
        let echoed_text = echoed_rows
            .iter()
            .flat_map(|line| line.spans.iter().map(|span| span.content.as_ref()))
            .collect::<String>();
        assert_eq!(echoed_text, format!("✴ ❯ {}", prompt.replace('\n', "")));
    }

    #[test]
    fn long_raw_chat_task_crosses_the_child_boundary_on_stdin() {
        // Linux commonly caps one argv element near 128 KiB. Exercise well
        // beyond that and include text `/code` would otherwise reinterpret.
        let prompt = format!(
            "BEGIN-SENTINEL\nx3 src -- cargo test --all\n{}\nEND-SENTINEL",
            "0123456789abcdef".repeat(20_000)
        );
        let args = vec![
            "code".to_string(),
            RAW_CHAT_TASK_ARG.to_string(),
            prompt.clone(),
        ];
        let mut app = App::new();
        app.model = "claude:sonnet".to_string();
        assert!(app.job_creates_claude_session(&args));
        let transport = prepare_child_transport(args, None);

        assert_eq!(transport.args, ["code"]);
        assert!(transport.raw_chat_task);
        assert_eq!(transport.stdin.as_deref(), Some(prompt.as_str()));
        assert!(transport
            .stdin
            .as_deref()
            .unwrap()
            .ends_with("END-SENTINEL"));
        assert!(!transport.label.contains(RAW_CHAT_TASK_ARG));
    }

    #[test]
    fn durable_chat_history_round_trips_multiline_prompts() {
        let prompt = "fn main() {\n    println!(\"full prompt\");\n}";
        let encoded = serde_json::to_string(prompt).unwrap();
        let history = decode_history(&format!("{encoded}\nlegacy one-line entry\n"));

        assert_eq!(history, [prompt, "legacy one-line entry"]);
    }

    #[test]
    fn stream_lines_build_a_collapsible_fold() {
        let mut app = App::new();
        let base = app.transcript.len();
        // A child streams: open a fold, two body lines, close it.
        app.push_stream_line("\u{1e}FOLD_OPEN\u{1f}adept 1 — working");
        app.push_stream_line("  read sol.py");
        app.push_stream_line("  edit sol.py");
        app.push_stream_line("\u{1e}FOLD_CLOSE");
        // Exactly one new Fold entry, collapsed, holding both body lines.
        let new: Vec<&Entry> = app.transcript[base..].iter().collect();
        assert_eq!(
            new.len(),
            1,
            "the body lines must live inside the fold, not at top level"
        );
        let Entry::Fold(f) = new[0] else {
            panic!("expected a fold entry")
        };
        assert!(f.collapsed, "folds arrive collapsed");
        assert_eq!(f.body.len(), 2);
        assert!(app.open_fold.is_none(), "close balances open");

        // Collapsed → the body is hidden; expanding reveals it.
        let collapsed_rows = app.rendered_lines().len();
        app.move_fold_selection(1);
        app.toggle_selected_fold();
        let expanded_rows = app.rendered_lines().len();
        assert!(
            expanded_rows > collapsed_rows,
            "expanding a fold shows its body"
        );
    }

    #[test]
    fn local_commands_bypass_the_queue() {
        // These act on app state and must run even while a job streams…
        for l in [
            "/model ollama:qwen2.5:3b",
            "/cd ..",
            "/clear",
            "/new",
            "/runs",
            "/quit",
        ] {
            assert!(is_local_command(l), "{l} should be local");
        }
        // …while intents and job commands queue.
        for l in [
            "fix the failing test",
            "/code . fix it",
            "/bench 100",
            "/cast x",
        ] {
            assert!(!is_local_command(l), "{l} should queue");
        }
    }

    #[test]
    fn shell_split_handles_quotes() {
        assert_eq!(
            shell_split("code . \"fix the bug\""),
            vec!["code", ".", "fix the bug"]
        );
        assert_eq!(shell_split("roster"), vec!["roster"]);
        assert_eq!(shell_split("  a   b  "), vec!["a", "b"]);
        assert_eq!(shell_split("say ''"), vec!["say", ""]);
    }
}
