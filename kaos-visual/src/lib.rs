//! The `kaos visual` application: native egui projections over the shared Kaos
//! core, workspace, and agent runtime.
//!
//! Every rule about what a drawing *means*—forms, arity, exact code generation,
//! loading, hit-testing, marquee geometry, glyph geometry, and structural depth—lives
//! in [`kaos_core::visual`] and is tested without a window. This crate paints
//! those projections and provides typed visual surfaces for source, sessions,
//! sigils, runs, settings, and the remaining Kaos actions.
//!
//! Rendering is native (egui on glow), not a webview, so the editor needs no
//! system libraries beyond the OpenGL and windowing ones any desktop already
//! has.
//!
//! It is its own application. `kaos-visual [program-or-file]` runs the editor
//! with no terminal app involved; the `kaos visual` subcommand is a second
//! front door onto the same [`open`] and [`run`] pair.

use eframe::egui;
use egui::{Align2, Color32, FontId, PointerButton, Pos2, Rect, Sense, Stroke as UiStroke, Vec2};
use std::collections::BTreeSet;

use kaos_core::tabs::{TabId, Tabs};
use kaos_core::visual::{
    Arrow, BorderGrab, Form, Mandala, Node, NodeId, Shape, SpatialLayout, Stroke, View, WorldRect,
    NODE_R, NODE_RY,
};
use kaos_workspace::rebis_workspace::{
    handle_edit_key, highlights, EditKey, EditModifiers, Editor as SourceEditor,
    Highlight as SourceHighlight, Mode as VimMode,
};

mod actions;
mod automata;
/// The automaton's geometry, re-exported for `examples/generation_preview.rs`.
///
/// That example renders the same composition to a PNG so the figure can be
/// reviewed on a machine with no compositor — which is the only way to see it
/// without launching a window. It shares `compose` with the pane rather than
/// re-implementing it, so the preview cannot drift from what the pane draws.
pub mod automata_preview {
    pub use crate::automata::{ramp, Automaton, BinaryStream, Cell, Mark, Ramp, Site};
}
mod process;
mod runs;
mod settings;
mod theme;

use theme::{install_symbol_fallback, install_theme, Ink};

/// One open drawing. Each tab keeps its own canvas *and its own viewport and
/// selection*, so switching tabs returns you to exactly where you were.
#[derive(Default)]
struct Doc {
    mandala: Mandala,
    view: View,
    canvas_mode: CanvasMode,
    camera: SpatialCamera,
    pending: Option<NodeId>,
    /// Last-selected node, used as the inspector's primary object.
    selected: Option<NodeId>,
    /// Complete block selection. The primary node is always included.
    selection: BTreeSet<NodeId>,
    /// Per-tab canvas history. View transforms are intentionally excluded:
    /// undo changes the drawing, not where the user is looking at it.
    undo: Vec<Mandala>,
    redo: Vec<Mandala>,
    /// Nodes currently being evaluated. Driven by a real run on a background
    /// thread, so the ring means work is actually in flight rather than
    /// standing in for it.
    running: std::collections::HashSet<NodeId>,
    /// Connections the user chose to draw as a straight angled line instead of
    /// the default right-angle trace. Keyed by the connection's own node — the
    /// child of a father-of link, or the flow node itself. Presentation only,
    /// like node positions; it never affects generated Rebis.
    angled: std::collections::HashSet<NodeId>,
    /// The editable source, kept in step with the drawing in both directions.
    text: String,
    /// What the drawing last generated. Comparing against it is how we tell an
    /// edit made on the canvas from one typed into the panel, without either
    /// overwriting the other mid-keystroke.
    generated: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
enum CanvasMode {
    #[default]
    Planar,
    Spatial,
}

impl CanvasMode {
    const fn label(self) -> &'static str {
        match self {
            Self::Planar => "2D · EDIT",
            Self::Spatial => "3D · STRUCTURE",
        }
    }
}

/// An orbit camera over the structural projection: yaw/pitch spin the graph
/// around its centre, the wheel zooms, and the arrow keys move through the
/// space. `pan` is a **world-space** offset of the viewpoint: up/down travel
/// forward and backward along the camera's look direction and left/right strafe
/// sideways, so movement is real navigation rather than a slide of the flat
/// projection.
#[derive(Clone, Copy, Debug)]
struct SpatialCamera {
    yaw: f32,
    pitch: f32,
    zoom: f32,
    pan: [f32; 3],
}

impl Default for SpatialCamera {
    fn default() -> Self {
        Self {
            yaw: -0.62,
            pitch: 0.34,
            zoom: 1.0,
            pan: [0.0, 0.0, 0.0],
        }
    }
}

#[derive(Clone, Copy)]
struct ProjectedNode {
    id: NodeId,
    position: Pos2,
    scale: f32,
    camera_depth: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum VisibleEdgeKind {
    Father,
    Flow,
}

/// A semantic connection shared by the planar and spatial renderers.
///
/// Complete flow nodes are syntax handles, not extra visible arrows: their two
/// stored child links collapse into one directional flow edge. Keeping this
/// projection shared prevents 2D and 3D from presenting different programs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct VisibleEdge {
    from: NodeId,
    to: NodeId,
    owner: NodeId,
    source: Option<Arrow>,
    kind: VisibleEdgeKind,
}

fn complete_flow(mandala: &Mandala, id: NodeId) -> bool {
    mandala.flow_result(id).is_some()
}

fn resolved_flow_result(mandala: &Mandala, mut id: NodeId) -> NodeId {
    for _ in 0..16 {
        let Some(next) = mandala.flow_result(id) else {
            break;
        };
        id = next;
    }
    id
}

/// Project stored child-to-parent links into the semantic connections shown
/// by both canvases. A complete flow is one source form, not two grey links
/// plus a blue link: its operand links collapse into one blue edge.
fn visible_edges(mandala: &Mandala) -> Vec<VisibleEdge> {
    let mut edges = Vec::new();
    for arrow in mandala.arrows() {
        if complete_flow(mandala, arrow.to) {
            continue;
        }
        edges.push(VisibleEdge {
            from: arrow.to,
            to: resolved_flow_result(mandala, arrow.from),
            owner: arrow.from,
            source: Some(*arrow),
            kind: VisibleEdgeKind::Father,
        });
    }
    for node in mandala.nodes() {
        if node.shape() != Shape::Arrow || !complete_flow(mandala, node.id) {
            continue;
        }
        let kids = mandala.children(node.id);
        let [first, second] = kids[..] else {
            continue;
        };
        let (from, to) = if node.form == Form::Backflow {
            (second, first)
        } else {
            (first, second)
        };
        edges.push(VisibleEdge {
            from: resolved_flow_result(mandala, from),
            to: resolved_flow_result(mandala, to),
            owner: node.id,
            source: None,
            kind: VisibleEdgeKind::Flow,
        });
    }
    edges
}

#[derive(Clone, Copy)]
struct NodePaint {
    position: Pos2,
    scale: f32,
    spin: f32,
    arrow_body: bool,
    recursive: bool,
}

#[derive(Clone, Copy)]
struct GlyphPaint {
    position: Pos2,
    scale: f32,
    outline: UiStroke,
    hot: bool,
}

const fn node_outline_width(shape: Shape, emphasized: bool) -> f32 {
    if matches!(
        shape,
        Shape::Square | Shape::Oval | Shape::Parallelogram | Shape::Amp | Shape::Hexagon
    ) {
        if emphasized {
            5.5
        } else {
            4.5
        }
    } else if emphasized {
        2.5
    } else {
        1.5
    }
}

impl Doc {
    const HISTORY_LIMIT: usize = 128;

    fn push_bounded(history: &mut Vec<Mandala>, state: Mandala) {
        if history.len() == Self::HISTORY_LIMIT {
            history.remove(0);
        }
        history.push(state);
    }

    /// Capture the current drawing immediately before one user edit.
    fn checkpoint(&mut self) {
        Self::push_bounded(&mut self.undo, self.mandala.clone());
        self.redo.clear();
    }

    fn undo(&mut self) -> bool {
        let Some(previous) = self.undo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.mandala, previous);
        Self::push_bounded(&mut self.redo, current);
        self.reset_interaction();
        true
    }

    fn redo(&mut self) -> bool {
        let Some(next) = self.redo.pop() else {
            return false;
        };
        let current = std::mem::replace(&mut self.mandala, next);
        Self::push_bounded(&mut self.undo, current);
        self.reset_interaction();
        true
    }

    fn delete_selected(&mut self) -> bool {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return false;
        }
        self.checkpoint();
        for id in ids {
            self.mandala.remove(id);
        }
        self.reset_interaction();
        true
    }

    fn selected_ids(&self) -> BTreeSet<NodeId> {
        let mut ids = self.selection.clone();
        if let Some(id) = self.selected {
            ids.insert(id);
        }
        ids.retain(|id| self.mandala.node(*id).is_some());
        ids
    }

    fn selection_len(&self) -> usize {
        self.selected_ids().len()
    }

    fn primary_selected(&self) -> Option<NodeId> {
        self.selected
            .filter(|id| self.mandala.node(*id).is_some())
            .or_else(|| self.selected_ids().into_iter().next())
    }

    fn is_selected(&self, id: NodeId) -> bool {
        self.selected == Some(id) || self.selection.contains(&id)
    }

    /// Select the whole block rooted at `id` — the node and all its operands,
    /// recursively — so a single click almost always yields a valid block. The
    /// clicked node stays the primary (inspector) selection.
    fn select_only(&mut self, id: NodeId) {
        self.selection.clear();
        if self.mandala.node(id).is_some() {
            self.selection = self.mandala.subtree(id);
            self.selected = Some(id);
        } else {
            self.selected = None;
        }
        self.pending = None;
    }

    /// Toggle a whole block in or out of the selection. A block already fully
    /// selected is removed; otherwise its subtree is added, so blocks compose
    /// and decompose as units.
    /// Ctrl-click toggles a *single* element in or out of the selection —
    /// unlike a plain click, which selects the whole block. This is how you
    /// trim one node from, or add one node to, an otherwise block-sized
    /// selection.
    fn toggle_selection(&mut self, id: NodeId) {
        if self.mandala.node(id).is_none() {
            return;
        }
        if self.selection.remove(&id) {
            if self.selected == Some(id) {
                self.selected = self.selection.iter().next_back().copied();
            }
        } else {
            self.selection.insert(id);
            self.selected = Some(id);
        }
        self.pending = None;
    }

    fn select_many(&mut self, ids: impl IntoIterator<Item = NodeId>, additive: bool) {
        if !additive {
            self.selection.clear();
            self.selected = None;
        }
        for id in ids {
            if self.mandala.node(id).is_some() {
                self.selection.insert(id);
                self.selected = Some(id);
            }
        }
        self.pending = None;
    }

    fn selected_source(&self) -> Result<Option<String>, String> {
        let ids = self.selected_ids();
        if ids.is_empty() {
            return Ok(None);
        }
        self.mandala
            .induced_subgraph(ids)
            .to_rebis()
            .map(Some)
            .map_err(|error| error.to_string())
    }

    fn copied_selection(&self) -> Option<Mandala> {
        let ids = self.selected_ids();
        (!ids.is_empty()).then(|| self.mandala.induced_subgraph(ids))
    }

    fn paste_graph(&mut self, graph: &Mandala, offset: (f64, f64)) -> Vec<NodeId> {
        if graph.is_empty() {
            return Vec::new();
        }
        self.checkpoint();
        let pasted = self.mandala.append_copy(graph, offset);
        self.select_many(pasted.iter().copied(), false);
        pasted
    }

    fn clear_selection(&mut self) {
        self.pending = None;
        self.selected = None;
        self.selection.clear();
    }

    fn reset_interaction(&mut self) {
        self.clear_selection();
        self.running.clear();
    }
}

/// A conversation, with the same durable sessions the terminal app writes.
/// Opening one here and resuming it there is the same store and the same
/// format — the transcript is not a second, parallel notion of a chat.
struct ChatPane {
    session: kaos_core::sessions::Session,
    input: String,
    /// Showing the session list rather than a transcript.
    browsing: bool,
    notice: Option<String>,
    /// When present, each submitted turn receives a fresh snapshot of this
    /// retained run, including its complete source and output.
    run_id: Option<u64>,
}

impl Default for ChatPane {
    fn default() -> Self {
        Self {
            session: kaos_core::sessions::Session::new(
                kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".into()),
                std::env::current_dir()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
            ),
            input: String::new(),
            browsing: true,
            notice: None,
            run_id: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
enum SourceProjection {
    #[default]
    Editor,
    Tree,
    Mandala,
}

/// The automaton view's own state: the lattice, and how far the run transcript
/// has been consumed.
struct AutomataPane {
    machine: automata::Automaton,
    /// Which run is being watched. `None` means the lattice was built from
    /// source and evolves on the default rule until a run supplies one.
    run: Option<u64>,
    /// Where the program came from, for the header.
    origin: String,
    /// Transcript entries already fed in, so a live run is consumed
    /// incrementally instead of re-read from the start every frame.
    consumed: usize,
    /// Advance generations automatically.
    running: bool,
    /// Seconds between generations.
    interval: f32,
    /// Time of the last generation, so stepping is wall-clock paced rather
    /// than frame-paced — the view must look the same on any refresh rate.
    last_step: std::time::Instant,
}

impl AutomataPane {
    /// Build the lattice by parsing `source`. The program's shape is the
    /// lattice, so a program that does not parse has no lattice.
    fn from_source(source: &str, origin: impl Into<String>) -> Result<Self, String> {
        let expr = rebis_lang::parse(source).map_err(|error| error.to_string())?;
        Ok(Self {
            machine: automata::Automaton::from_program(&expr),
            run: None,
            origin: origin.into(),
            consumed: 0,
            running: true,
            interval: 0.10,
            last_step: std::time::Instant::now(),
        })
    }
}

struct SourcePane {
    /// Saved-hypersigil name, independent from an ordinary file path.
    name: String,
    editor: SourceEditor,
    /// The cursor position most recently revealed by the editor. Scrolling the
    /// viewport must not count as a cursor move, or the next frame will pull
    /// the viewport back to the caret.
    revealed_cursor: Option<usize>,
    vim_enabled: bool,
    mode: VimMode,
    command: String,
    notice: Option<String>,
    file_path: String,
    output_path: String,
    record_path: String,
    search: String,
    projection: SourceProjection,
}

impl Default for SourcePane {
    fn default() -> Self {
        let vim_enabled = kaos_core::config::value("vim_mode")
            .and_then(|value| value.parse::<bool>().ok())
            .unwrap_or(false);
        Self {
            name: String::new(),
            editor: SourceEditor::new(""),
            revealed_cursor: None,
            vim_enabled,
            mode: if vim_enabled {
                VimMode::Normal
            } else {
                VimMode::Insert
            },
            command: String::new(),
            notice: None,
            file_path: String::new(),
            output_path: String::new(),
            record_path: String::new(),
            search: String::new(),
            projection: SourceProjection::Editor,
        }
    }
}

impl SourcePane {
    fn with_text(text: impl Into<String>) -> Self {
        Self {
            editor: SourceEditor::new(text),
            ..Self::default()
        }
    }

    fn run_program_source(&self) -> Result<(String, runs::Scope), String> {
        if let Some(selected) = self.editor.selected_text(self.mode) {
            return kaos_workspace::rebis_workspace::scoped_block_source(
                self.editor.source(),
                &selected,
            )
            .map(|source| (source, runs::Scope::Block))
            .map_err(|error| format!("run selection: {error}"));
        }
        Ok((self.editor.source().to_string(), runs::Scope::Program))
    }

    fn run_block_source(&self) -> Result<String, String> {
        let Some((left, right)) = kaos_workspace::rebis_workspace::matching_form(
            self.editor.source(),
            self.editor.cursor(),
        ) else {
            return Err("run block: put the caret on the block's ( ) or [ ]".to_string());
        };
        let block = self
            .editor
            .source()
            .chars()
            .skip(left)
            .take(right - left + 1)
            .collect::<String>();
        kaos_workspace::rebis_workspace::scoped_block_source(self.editor.source(), &block)
            .map_err(|error| format!("run block: {error}"))
    }

    fn run_options_source(&self) -> Result<(String, Option<String>), String> {
        let program = self.editor.source().to_string();
        let block = if self.editor.selected_text(self.mode).is_some() {
            Some(self.run_program_source()?.0)
        } else {
            self.run_block_source().ok()
        };
        Ok((program, block))
    }

    fn set_vim_enabled(&mut self, enabled: bool) {
        if self.vim_enabled && self.mode == VimMode::Insert {
            self.editor.end_insert_session();
        }
        self.editor.end_visual();
        self.editor.clear_pending();
        self.command.clear();
        self.vim_enabled = enabled;
        self.mode = if enabled {
            VimMode::Normal
        } else {
            VimMode::Insert
        };
    }
}

enum SourceAction {
    SaveSigil {
        name: String,
        text: String,
    },
    OpenFile(String),
    SaveFile {
        path: String,
        text: String,
    },
    ChooseSaveFile {
        suggested: String,
        text: String,
    },
    Format,
    Draw(String),
    Generation(String),
    Run {
        program: String,
        block: Option<String>,
    },
    Copy(String),
    WriteProjection {
        path: String,
        text: String,
    },
    LoadRecord(String),
    OpenSigilChat(String),
    VimCommand(String),
}

#[derive(Default)]
struct SigilPane {
    query: String,
    notice: Option<String>,
    pending_delete: Option<String>,
}

enum SigilAction {
    Draw(kaos_core::sigils::Entry),
    Edit(kaos_core::sigils::Entry),
    Chat(kaos_core::sigils::Entry),
    Delete(kaos_core::sigils::Entry),
}

fn sigil_catalog_row(
    ui: &mut egui::Ui,
    ink: Ink,
    pane: &mut SigilPane,
    entry: &kaos_core::sigils::Entry,
    action: &mut Option<SigilAction>,
) {
    ui.horizontal(|ui| {
        ui.monospace(format!("{}   {} bytes", entry.name, entry.bytes));
        if entry.read_only {
            ui.colored_label(ink.secondary, "embedded · read only");
        }
        if ui.small_button("mandala").clicked() {
            *action = Some(SigilAction::Draw(entry.clone()));
        }
        if ui.small_button("edit source").clicked() {
            *action = Some(SigilAction::Edit(entry.clone()));
        }
        if ui.small_button("chat").clicked() {
            *action = Some(SigilAction::Chat(entry.clone()));
        }
        if entry.read_only {
            ui.add_enabled(false, egui::Button::new("delete").small())
                .on_disabled_hover_text("embedded std sigils cannot be deleted");
            return;
        }
        let confirming = pane.pending_delete.as_deref() == Some(&entry.name);
        if ui
            .small_button(if confirming {
                "confirm delete"
            } else {
                "delete"
            })
            .clicked()
        {
            if confirming {
                *action = Some(SigilAction::Delete(entry.clone()));
            } else {
                pane.pending_delete = Some(entry.name.clone());
                pane.notice = Some(format!("delete {}? click confirm delete", entry.name));
            }
        }
    });
}

/// What a tab holds. The tab machinery is generic, so adding a kind is adding
/// a variant here rather than another parallel list.
enum Pane {
    Mandala(Doc),
    Chat(ChatPane),
    /// Personal sigils plus embedded read-only `std/` — the same catalog the
    /// terminal explorer browses. Opening one draws it on a new canvas.
    Sigils(SigilPane),
    /// Rebis source, as text. The same buffer the terminal workspace edits and
    /// the same library it saves to, checked with the same parser.
    Source(SourcePane),
    /// Every non-secret Kaos preference, backed by the same persistent file as
    /// the terminal `/config` editor.
    Settings(settings::SettingsPane),
    /// A run as the cellular automaton it generates: the program's geometry
    /// supplies the lattice, the model's own bytes supply the rule. See the
    /// `automata` module for why this is a generation and not a diagram.
    Automata(AutomataPane),
    /// Retained Rebis executions shared by every drawing and source tab.
    Runs,
    /// Kaos rites and inspection commands that are not a document surface.
    Actions,
}

// ── palette ─────────────────────────────────────────────────────────────────
//
// Shared semantic palette from `theme.rs`, so the editor and terminal app wear
// the same mode. `/theme dark|light` persists the choice for both.

const fn plural(count: usize) -> &'static str {
    if count == 1 {
        ""
    } else {
        "s"
    }
}

/// What a click does.
#[derive(Clone, PartialEq)]
enum Tool {
    /// Place this form.
    Place(Form),
    /// Click two shapes to link them with a flow node (`->` or `<-`). The node
    /// is created behind the arrow, so drawing the arrow *is* writing the form.
    Flow(Form),
    /// Click a parent, then the child that becomes its next ordered operand.
    /// This supplies operands to `[]`, `( )`, `$`, calls, quotes, and macros.
    Father,
    /// Click a shape to select it.
    Select,
}

/// A run captured by a run button, held until the user picks its mode and lane
/// in the modal. The source and scope are fixed at click time; mode and lane
/// are chosen in the modal and default to the desk's last choice.
struct PendingRun {
    program_source: String,
    block_source: Option<String>,
    scope: runs::Scope,
    /// Node ids to light with the working ring, when the run came from a
    /// mandala (whole graph or a selected block). Empty for source-tab runs.
    ring: std::collections::HashSet<NodeId>,
    mode: runs::Mode,
    lane: runs::Lane,
}

/// An in-progress pointer gesture.
#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// Moving a shape. `grab` is the offset from the shape's centre, in world
    /// units, so the shape does not jump to the cursor.
    Node {
        id: NodeId,
        grab: (f64, f64),
    },
    /// Dragging a square's wall to size its box. The centre stays put and the
    /// grabbed walls follow the pointer, so the blocks inside do not move.
    Resize(BorderGrab),
    /// Panning the canvas.
    Pan,
    /// Right-button world-space marquee. Ctrl preserves the existing set.
    Marquee {
        start: (f64, f64),
        current: (f64, f64),
        additive: bool,
    },
}

impl Drag {
    /// What a primary drag starting at this world point does.
    ///
    /// A wall offered for resizing wins first, then whatever shape the point
    /// landed on, and bare canvas pans. `Mandala::resize_grab` is what keeps a
    /// block dropped across a wall grabbable as a block.
    fn beginning_at(mandala: &Mandala, wx: f64, wy: f64) -> Self {
        if let Some(grab) = mandala.resize_grab(wx, wy) {
            return Drag::Resize(grab);
        }
        match mandala.hit(wx, wy) {
            Some(id) => {
                let (x, y) = mandala.node(id).map(|n| (n.x, n.y)).unwrap_or((wx, wy));
                Drag::Node {
                    id,
                    grab: (wx - x, wy - y),
                }
            }
            None => Drag::Pan,
        }
    }
}

/// Exact in-app graph clipboard plus the text mirrored to the system
/// clipboard. The text lets a valid Rebis block cross process boundaries;
/// the graph preserves incomplete selections and canvas placement in-process.
struct MandalaClipboard {
    graph: Mandala,
    system_text: String,
    pastes: u32,
}

/// Resolve what an argument names into a drawing to open.
///
/// Same convention as `rebis run`: a readable path loads, anything else is
/// treated as inline Rebis source, and nothing at all is an empty canvas.
/// Kept here rather than in a caller so every way of starting the editor
/// agrees about what its argument means.
pub fn open(arg: &str) -> Result<Mandala, String> {
    let arg = arg.trim();
    if arg.is_empty() {
        return Ok(Mandala::new());
    }
    let source = std::fs::read_to_string(arg).unwrap_or_else(|_| arg.to_string());
    Mandala::from_rebis(&source).map_err(|e| e.to_string())
}

/// Open the editor window on `start`. Blocks until the window closes.
pub fn run(start: Mandala) {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1280.0, 820.0])
            .with_title("kaos visual — mandala editor"),
        ..Default::default()
    };
    if let Err(error) = eframe::run_native(
        "kaos visual",
        options,
        Box::new(|cc| {
            let editor = Editor::new(start);
            install_symbol_fallback(&cc.egui_ctx);
            install_theme(&cc.egui_ctx, editor.ink);
            Ok(Box::new(editor))
        }),
    ) {
        eprintln!("visual: {error}");
    }
}

struct Editor {
    ink: Ink,
    /// Where kaos was started. Runs, relative reads, imports and output paths
    /// all resolve from here, exactly as in the terminal app, so a program
    /// drawn here means the same thing when it is run.
    cwd: std::path::PathBuf,
    /// Editable session working directory shown in Settings. It is kept
    /// separate until Apply so an incomplete path never moves live work.
    cwd_edit: String,
    tabs: Tabs<Pane>,
    /// Stands in while a chat tab is active, so the canvas code never has to
    /// ask whether there is a drawing. It is never drawn.
    scratch: Doc,
    /// The tool is deliberately shared across tabs: it is a mode of working,
    /// not a property of a drawing.
    tool: Tool,
    drag: Drag,
    /// Shared across drawing tabs, like an ordinary application clipboard.
    clipboard: Option<MandalaClipboard>,
    notice: Option<String>,
    /// A run awaiting its mode/lane choice in the modal. Every run button sets
    /// this instead of launching immediately, so the user always picks dry vs.
    /// live-with-tools vs. chaos rather than silently getting the dry default.
    pending_run: Option<PendingRun>,
    /// The run-status modal shown immediately after a configured run launches.
    run_notice: Option<u64>,
    /// Process-backed run history and controls, shared by all source surfaces.
    runs: runs::Desk,
    /// Streamed chat/code/cast/conclave and inspection task history.
    actions: actions::Desk,
}

impl Editor {
    fn new(mandala: Mandala) -> Self {
        let mut tabs = Tabs::new();
        tabs.open(
            "mandala",
            Pane::Mandala(Doc {
                mandala,
                ..Doc::default()
            }),
        );
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self {
            ink: Ink::load(),
            cwd_edit: cwd.display().to_string(),
            cwd,
            tabs,
            scratch: Doc::default(),
            tool: Tool::Select,
            drag: Drag::None,
            pending_run: None,
            run_notice: None,
            clipboard: None,
            notice: None,
            runs: runs::Desk::default(),
            actions: actions::Desk::default(),
        }
    }

    fn request_run_options(
        &mut self,
        program_source: String,
        block_source: Option<String>,
        ring: std::collections::HashSet<NodeId>,
        preferred_scope: runs::Scope,
    ) {
        let block_available = self.pending_block_available(&block_source);
        self.pending_run = Some(PendingRun {
            program_source,
            block_source,
            scope: if preferred_scope == runs::Scope::Block && block_available {
                runs::Scope::Block
            } else {
                runs::Scope::Program
            },
            ring,
            mode: self.runs.mode,
            lane: self.runs.lane,
        });
    }

    fn pending_block_available(&self, block: &Option<String>) -> bool {
        block
            .as_ref()
            .is_some_and(|source| !source.trim().is_empty())
    }

    fn run_mandala(&mut self) {
        let program = match self.doc().mandala.to_rebis() {
            Ok(source) => source,
            Err(error) => {
                self.notice = Some(error.to_string());
                return;
            }
        };
        let ids = self.doc().selected_ids();
        let block = match self.doc().selected_source() {
            Ok(Some(selected)) => {
                kaos_workspace::rebis_workspace::scoped_block_source(&program, &selected)
                    .map(Some)
                    .map_err(|error| error.to_string())
            }
            Ok(None) => Ok(None),
            Err(error) => Err(error),
        };
        let block = match block {
            Ok(block) => block,
            Err(error) => {
                self.notice = Some(format!("selected block: {error}"));
                return;
            }
        };
        let ring = if ids.is_empty() {
            self.doc()
                .mandala
                .nodes()
                .iter()
                .map(|node| node.id)
                .collect()
        } else {
            ids.into_iter().collect()
        };
        let preferred_scope = if block.is_some() {
            runs::Scope::Block
        } else {
            runs::Scope::Program
        };
        self.request_run_options(program, block, ring, preferred_scope);
    }

    /// Draw the run-options modal when a run is pending. Returns nothing; on
    /// confirm it launches the run with the chosen mode, lane, and scope.
    fn run_modal(&mut self, ctx: &egui::Context) {
        if self.pending_run.is_none() {
            return;
        }
        let k = self.ink;
        let mut launch = false;
        let mut cancel = false;
        // Dim the app behind the modal so it reads as modal.
        egui::Area::new("run_modal_backdrop".into())
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(140));
                // Clicking the dimmed backdrop (anywhere outside the window)
                // dismisses the modal, like tapping away from a dialog.
                if ui.allocate_rect(screen, Sense::click()).clicked() {
                    cancel = true;
                }
            });
        // The backdrop is an Area at the same Order as a Window, so which one
        // ends up on top is a tie. Losing it paints the scrim OVER the modal —
        // the screen goes flat grey — and the backdrop then swallows every
        // click as "outside", dismissing the dialog you were trying to use.
        // Lifting the window each frame settles the tie in the only direction
        // that makes sense.
        let window = egui::Window::new("RUN")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                let pending = self.pending_run.as_mut().expect("pending run");
                ui.colored_label(k.faint, "SCOPE");
                ui.radio_value(
                    &mut pending.scope,
                    runs::Scope::Program,
                    "program · complete source",
                );
                if pending.block_source.is_some() {
                    ui.radio_value(
                        &mut pending.scope,
                        runs::Scope::Block,
                        "block · selected/caret form",
                    );
                } else {
                    ui.colored_label(k.faint, "block unavailable for this source");
                    pending.scope = runs::Scope::Program;
                }
                ui.add_space(6.0);
                ui.colored_label(k.faint, "MODE");
                ui.radio_value(
                    &mut pending.mode,
                    runs::Mode::Dry,
                    "dry — deterministic, no model or tools",
                );
                ui.radio_value(
                    &mut pending.mode,
                    runs::Mode::Direct,
                    "direct — one live tool agent per prompt",
                );
                ui.radio_value(
                    &mut pending.mode,
                    runs::Mode::Chaos,
                    "chaos — full Kaos tool-agent expansion",
                );
                ui.add_space(6.0);
                ui.colored_label(k.faint, "LANE");
                ui.horizontal(|ui| {
                    ui.radio_value(&mut pending.lane, runs::Lane::Serial, "serial");
                    ui.radio_value(&mut pending.lane, runs::Lane::Parallel, "parallel");
                });
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("run").clicked() {
                        launch = true;
                    }
                    if ui.button("cancel").clicked() {
                        cancel = true;
                    }
                });
            });
        if let Some(window) = window {
            ctx.move_to_top(window.response.layer_id);
        }
        // Esc cancels the modal.
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            cancel = true;
        }
        if launch {
            if let Some(pending) = self.pending_run.take() {
                // The chosen mode/lane become the desk's default for next time.
                self.runs.mode = pending.mode;
                self.runs.lane = pending.lane;
                self.runs.scope = pending.scope;
                let ring = pending.ring;
                let source = match pending.scope {
                    runs::Scope::Program => pending.program_source,
                    runs::Scope::Block => pending.block_source.unwrap_or(pending.program_source),
                };
                let id = self.runs.submit(source, Some(pending.lane), &self.cwd);
                // Keep the working ring on the drawing before opening the
                // generation tab; `Tabs::open` makes that new tab active.
                if !ring.is_empty() {
                    if let Some(Pane::Mandala(doc)) = self.tabs.active_mut() {
                        doc.running = ring;
                    }
                }
                // Every configured run gets its own generation tab immediately
                // so the live output can start shaping the figure while the
                // process is still running.
                self.open_generation();
                self.run_notice = Some(id);
            }
        } else if cancel {
            self.pending_run = None;
        }
    }

    fn run_status_modal(&mut self, ctx: &egui::Context) {
        let Some(id) = self.run_notice else {
            return;
        };
        let Some(run) = self.runs.runs.iter().find(|run| run.id == id) else {
            self.run_notice = None;
            return;
        };
        let state = run.state.label(run.paused);
        let preview = run.preview();
        let last_output = run.output.last().cloned();
        let terminal = run.state.terminal();
        let awaiting = run.state == runs::State::AwaitingPermission;
        let mut close = false;
        let mut go_to_runs = false;
        let mut go_to_generation = false;
        let mut permission = None;
        let mut deny = false;

        egui::Area::new("run_status_modal_backdrop".into())
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::ZERO)
            .show(ctx, |ui| {
                let screen = ctx.screen_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, Color32::from_black_alpha(140));
                ui.allocate_rect(screen, Sense::hover());
            });
        // Same tie as the run modal: lift the window above its own scrim.
        let window = egui::Window::new(format!("RUN #{id}"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.heading(if awaiting {
                    "run needs your authority"
                } else if terminal {
                    "run finished"
                } else {
                    "run is running"
                });
                ui.colored_label(self.ink.accent, state);
                ui.label(format!("{} · {}", run.mode.label(), run.scope.label()));
                ui.label(preview);
                if let Some(last_output) = &last_output {
                    ui.separator();
                    ui.colored_label(self.ink.faint, last_output);
                }
                // A live run stops for authority the instant it is submitted,
                // which is while this modal is on screen. Asking here is asking
                // where the reader already is; the runs pane keeps the same
                // decision for runs whose modal has been closed.
                if awaiting {
                    ui.add_space(8.0);
                    ui.colored_label(
                        self.ink.faint,
                        "a live model may read, edit, and write files and run commands",
                    );
                    ui.horizontal(|ui| {
                        if ui.button("allow once").clicked() {
                            permission = Some(runs::Authority::Once);
                        }
                        if ui.button("allow session").clicked() {
                            permission = Some(runs::Authority::Session);
                        }
                        if ui.button("deny · Esc").clicked() {
                            deny = true;
                        }
                    });
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("watch generation").clicked() {
                        go_to_generation = true;
                    }
                    if ui.button("close").clicked() {
                        close = true;
                    }
                    if ui.button("go to runs").clicked() {
                        go_to_runs = true;
                    }
                });
            });
        if let Some(window) = window {
            ctx.move_to_top(window.response.layer_id);
        }
        // While the question stands, Escape answers it by denying, exactly as it
        // does in the terminal — the same key must not grant power on one screen
        // and refuse it on the other. The button says so, because a key that
        // decides authority may not be a secret.
        let escaped = ctx.input(|input| input.key_pressed(egui::Key::Escape));
        if awaiting {
            deny |= escaped;
        } else {
            close |= escaped;
        }
        // The decision acts on THIS run, not on whatever was last selected.
        if let Some(authority) = permission {
            self.runs.selected = Some(id);
            self.runs.grant_selected(authority, &self.cwd);
        } else if deny {
            self.runs.selected = Some(id);
            self.runs.deny_selected();
        }
        if go_to_generation {
            self.runs.selected = Some(id);
            self.open_generation();
            self.run_notice = None;
        } else if go_to_runs {
            self.runs.selected = Some(id);
            self.open_runs();
            self.run_notice = None;
        } else if close {
            self.run_notice = None;
        }
    }

    fn copy_selected(&mut self, ctx: &egui::Context) {
        let Some(graph) = self.doc().copied_selection() else {
            self.notice = Some("select one or more forms to copy".to_string());
            return;
        };
        let count = graph.nodes().len();
        let system_text = graph.to_rebis().unwrap_or_else(|_| {
            format!("; kaos visual block · {count} forms · paste in kaos visual")
        });
        ctx.copy_text(system_text.clone());
        self.clipboard = Some(MandalaClipboard {
            graph,
            system_text,
            pastes: 0,
        });
        self.notice = Some(format!("copied {count} form{}", plural(count)));
    }

    fn paste_selected(&mut self, system_text: Option<&str>) {
        if let Some(text) = system_text {
            let owns_text = self
                .clipboard
                .as_ref()
                .is_some_and(|clipboard| clipboard.system_text == text);
            if !owns_text {
                match Mandala::from_rebis(text) {
                    Ok(graph) => {
                        self.clipboard = Some(MandalaClipboard {
                            graph,
                            system_text: text.to_string(),
                            pastes: 0,
                        });
                    }
                    Err(error) => {
                        self.notice = Some(format!("clipboard is not Rebis: {error}"));
                        return;
                    }
                }
            }
        }
        let Some(clipboard) = &mut self.clipboard else {
            self.notice = Some("copy a mandala block or Rebis source first".to_string());
            return;
        };
        clipboard.pastes = clipboard.pastes.saturating_add(1);
        let step = 28.0 * f64::from(clipboard.pastes);
        let graph = clipboard.graph.clone();
        let pasted = self.doc_mut().paste_graph(&graph, (step, step));
        self.notice = Some(format!(
            "pasted {} form{}",
            pasted.len(),
            plural(pasted.len())
        ));
    }

    /// Collect streamed output and advance the shared serial queue.
    fn poll_run(&mut self, ctx: &egui::Context) {
        if self.runs.poll(&self.cwd) {
            ctx.request_repaint();
        }
        if self.runs.has_active() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else {
            for tab in self.tabs.iter_mut() {
                if let Pane::Mandala(doc) = &mut tab.content {
                    doc.running.clear();
                }
            }
        }
    }

    fn poll_actions(&mut self, ctx: &egui::Context) {
        if self.actions.poll(&self.cwd) {
            ctx.request_repaint();
        }
        if self.actions.active_count() > 0 {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        for (session_id, reply) in self.actions.take_chat_replies() {
            let reply = kaos_core::chat::clean_chat_reply(&reply);
            let mut delivered = false;
            for tab in self.tabs.iter_mut() {
                if let Pane::Chat(chat) = &mut tab.content {
                    if chat.session.id == session_id {
                        chat.session
                            .push(kaos_core::sessions::Role::Model, reply.clone());
                        let _ = kaos_core::sessions::Store::default_store().save(&chat.session);
                        delivered = true;
                        break;
                    }
                }
            }
            if !delivered {
                let store = kaos_core::sessions::Store::default_store();
                if let Ok(mut session) = store.load(&session_id) {
                    session.push(kaos_core::sessions::Role::Model, reply);
                    let _ = store.save(&session);
                }
            }
        }
    }

    /// The open drawing, or a stand-in while a conversation is on screen — so
    /// the canvas code never has to ask which kind of tab is active.
    fn doc(&self) -> &Doc {
        match self.tabs.active() {
            Some(Pane::Mandala(d)) => d,
            _ => &self.scratch,
        }
    }

    fn doc_mut(&mut self) -> &mut Doc {
        match self.tabs.active_mut() {
            Some(Pane::Mandala(d)) => d,
            _ => &mut self.scratch,
        }
    }

    /// Whether a drawing is on screen, as opposed to a conversation.
    fn on_mandala(&self) -> bool {
        matches!(self.tabs.active(), Some(Pane::Mandala(_)))
    }
}

/// The colour an edge is drawn in.
///
/// Selection is expressed as WEIGHT, and — for the grey `father of` links —
/// as the accent. A flow keeps its red either way: the hue is what separates a
/// flow from a structural link, and a selection is exactly when the drawing is
/// being read closest. Both the 2D and 3D projections call this, so they
/// cannot drift apart on colour.
pub(crate) fn edge_colour(kind: VisibleEdgeKind, touches: bool, k: Ink) -> Color32 {
    match (kind, touches) {
        (VisibleEdgeKind::Flow, _) => k.secondary,
        (VisibleEdgeKind::Father, true) => k.accent,
        (VisibleEdgeKind::Father, false) => k.faint,
    }
}

impl eframe::App for Editor {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_keys(ctx);
        self.poll_run(ctx);
        self.poll_actions(ctx);
        if self.on_mandala() {
            self.sync();
        }
        self.header(ctx);
        self.tab_bar(ctx);
        if self.on_mandala() {
            self.palette(ctx);
            self.side(ctx);
        }
        self.footer(ctx);
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(self.ink.ground))
            .show(ctx, |ui| match self.tabs.active() {
                Some(Pane::Mandala(_)) => self.canvas(ui),
                Some(Pane::Chat(_)) => self.chat(ui),
                Some(Pane::Source(_)) => self.source(ui),
                Some(Pane::Sigils(_)) => self.sigils(ui),
                Some(Pane::Automata(_)) => self.automata_tab(ui),
                Some(Pane::Settings(_)) => self.settings(ui),
                Some(Pane::Runs) => self.runs_tab(ui),
                Some(Pane::Actions) => self.actions_tab(ui),
                None => {}
            });
        self.run_modal(ctx);
        self.run_status_modal(ctx);
    }
}

impl Editor {
    fn header(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            // Wrapped: the header carries up to seven controls, and a narrow
            // window used to cut the last of them off the right edge.
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(self.ink.accent, "KAOS VISUAL");
                ui.separator();
                if self.on_mandala() {
                    let mut mode = self.doc().canvas_mode;
                    ui.colored_label(self.ink.faint, "VIEW");
                    egui::ComboBox::from_id_salt("mandala_projection")
                        .selected_text(mode.label())
                        .width(132.0)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut mode,
                                CanvasMode::Planar,
                                CanvasMode::Planar.label(),
                            );
                            ui.selectable_value(
                                &mut mode,
                                CanvasMode::Spatial,
                                CanvasMode::Spatial.label(),
                            );
                        });
                    self.doc_mut().canvas_mode = mode;
                    let zoom = match mode {
                        CanvasMode::Planar => self.doc().view.zoom as f32,
                        CanvasMode::Spatial => self.doc().camera.zoom,
                    };
                    ui.colored_label(self.ink.faint, format!("{}%", (zoom * 100.0).round()));
                }
                // `with_main_wrap` is the part that matters: a right-to-left
                // group nested in a horizontal row is ONE item to the outer
                // row, so wrapping the outer row cannot reflow these buttons —
                // the group has to wrap on its own or the last button is cut
                // off the right edge.
                ui.with_layout(
                    egui::Layout::right_to_left(egui::Align::Center).with_main_wrap(true),
                    |ui| {
                        let editable =
                            self.on_mandala() && self.doc().canvas_mode == CanvasMode::Planar;
                        if editable && ui.button("clear").clicked() {
                            let doc = self.doc_mut();
                            if !doc.mandala.is_empty() {
                                doc.checkpoint();
                                doc.mandala = Mandala::new();
                                doc.reset_interaction();
                            }
                        }
                        if self.on_mandala() && ui.button("edit as text").clicked() {
                            self.open_generated_source();
                        }
                        if self.on_mandala() && ui.button("run").clicked() {
                            self.run_mandala();
                        }
                        if self.on_mandala() && ui.button("reset view").clicked() {
                            let doc = self.doc_mut();
                            match doc.canvas_mode {
                                CanvasMode::Planar => doc.view = View::new(),
                                CanvasMode::Spatial => doc.camera = SpatialCamera::default(),
                            }
                        }
                        let built = self.doc_mut().mandala.to_rebis();
                        // Only offer the hand-off when there is source to hand over.
                        ui.add_enabled_ui(built.is_ok(), |ui| {
                            if ui.button("open in terminal").clicked() {
                                match &built {
                                    Ok(src) => {
                                        self.notice = Some(match open_in_terminal(src, &self.cwd) {
                                            Ok(()) => "opened in terminal".to_string(),
                                            Err(e) => e,
                                        })
                                    }
                                    Err(e) => self.notice = Some(e.to_string()),
                                }
                            }
                        });
                        if let Some(note) = &self.notice {
                            ui.colored_label(self.ink.faint, note);
                        }
                    },
                );
            });
        });
    }

    /// Open a generation tab for the selected run, or for whatever program the
    /// active tab is holding.
    ///
    /// A run is preferred because a run has answers, and the answers are what
    /// build the rule. Without one the lattice is real but the rule is the
    /// default, and the header says so rather than implying the model spoke.
    fn open_generation(&mut self) {
        let from_run = self
            .runs
            .selected
            .and_then(|id| self.runs.runs.iter().find(|run| run.id == id))
            .map(|run| (run.source.clone(), run.id));

        let (source, run, origin) = match from_run {
            Some((source, id)) => (source, Some(id), format!("run #{id}")),
            None => match self.tabs.active() {
                Some(Pane::Source(pane)) => {
                    (pane.editor.source().to_string(), None, "source".to_string())
                }
                Some(Pane::Mandala(_)) => match self.doc().mandala.to_rebis() {
                    Ok(text) => (text, None, "drawing".to_string()),
                    Err(error) => {
                        self.notice = Some(error.to_string());
                        return;
                    }
                },
                _ => {
                    self.notice = Some(
                        "select a run, or open a source or drawing tab — a generation needs a \
                         program to build its lattice from"
                            .to_string(),
                    );
                    return;
                }
            },
        };

        self.open_generation_for(source, run, origin);
    }

    /// Open a generation directly from a source surface. This deliberately
    /// bypasses the selected-run preference used by the global `+ generation`
    /// button: pressing `generation` beside source must visualize that source,
    /// even when an older run remains selected in the Runs desk.
    fn open_generation_from_source(&mut self, source: String) {
        self.open_generation_for(source, None, "source".to_string());
    }

    fn open_generation_for(&mut self, source: String, run: Option<u64>, origin: String) {
        match AutomataPane::from_source(&source, origin.clone()) {
            Ok(mut pane) => {
                pane.run = run;
                self.tabs
                    .open(format!("generation · {origin}"), Pane::Automata(pane));
            }
            Err(error) => {
                self.notice = Some(format!(
                    "that program does not parse, so it has no lattice: {error}"
                ));
            }
        }
    }

    /// The generation: a run drawn as the automaton it computes.
    ///
    /// Two feeds meet here. The lattice came from the program's geometry when
    /// the tab opened and never changes. The rule comes from the run's
    /// transcript and keeps arriving, so each frame consumes whatever new lines
    /// landed before advancing a generation.
    fn automata_tab(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;

        // Two-phase, because the pane and the run desk are both behind `self`:
        // read what the pane still needs, take it from the desk, then hand it
        // over. Cloning only the tail keeps a long transcript cheap.
        let watching = match self.tabs.active() {
            Some(Pane::Automata(pane)) => Some((pane.run, pane.consumed)),
            _ => None,
        };
        let (fresh, state) = match watching {
            Some((Some(id), consumed)) => {
                self.runs
                    .runs
                    .iter()
                    .find(|run| run.id == id)
                    .map_or((Vec::new(), None), |run| {
                        // Only the tail. This runs every frame, so cloning the whole
                        // stream would make a long run cost more the longer it got.
                        let from = consumed.min(run.output.len());
                        (run.output[from..].to_vec(), Some((run.state, run.paused)))
                    })
            }
            _ => (Vec::new(), None),
        };
        let Some(Pane::Automata(pane)) = self.tabs.active_mut() else {
            return;
        };
        if !fresh.is_empty() {
            // The cursor comes back relative to the tail it was given.
            pane.consumed += pane.machine.consume(&fresh, 0);
        }

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(k.faint, "GENERATION");
            ui.colored_label(k.ink, &pane.origin);
            ui.separator();
            if ui
                .small_button(if pane.running { "pause" } else { "play" })
                .clicked()
            {
                pane.running = !pane.running;
            }
            if ui.small_button("step").clicked() {
                pane.machine.step();
            }
            ui.add(
                egui::Slider::new(&mut pane.interval, 0.02..=1.0)
                    .logarithmic(true)
                    .text("s/gen"),
            );
            ui.separator();
            ui.colored_label(k.faint, "GEN");
            ui.colored_label(k.ink, pane.machine.generation.to_string());
            ui.colored_label(k.faint, "CELLS");
            ui.colored_label(k.ink, pane.machine.cells.len().to_string());
            ui.colored_label(k.faint, "PROMPTS");
            ui.colored_label(k.ink, pane.machine.prompts_seen.to_string());
            ui.colored_label(k.faint, "ANSWERS");
            ui.colored_label(k.ink, pane.machine.answers_seen.to_string());
            ui.colored_label(k.faint, "H");
            // The entropy of the model's bytes IS the mixing rate, so it is the
            // one number here that changes what you are looking at.
            ui.colored_label(k.accent, format!("{:.2} bits/byte", pane.machine.entropy));
            let dead = pane.machine.dead_count();
            if dead > 0 {
                ui.separator();
                ui.colored_label(k.accent, format!("{dead} cells killed by a refusal"));
            }
        });
        ui.horizontal_wrapped(|ui| {
            if pane.machine.answers_seen == 0 {
                ui.colored_label(
                    k.faint,
                    "no answers yet — the lattice is the program's, the rule is still the default",
                );
            } else {
                ui.colored_label(
                    k.faint,
                    "the rule table is this model's byte sequence · a settled figure means terse \
                     output, not a good reading",
                );
            }
            if let Some((run_state, paused)) = state {
                ui.separator();
                ui.colored_label(k.faint, run_state.label(paused));
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.colored_label(k.secondary, "PROMPT BIN");
            ui.monospace(
                pane.machine
                    .binary_preview(automata::BinaryStream::Prompt, 3),
            );
            ui.colored_label(k.faint, format!("{} B", pane.machine.prompt_bytes_seen()));
            ui.separator();
            ui.colored_label(k.accent, "RESPONSE BIN");
            ui.monospace(
                pane.machine
                    .binary_preview(automata::BinaryStream::Response, 3),
            );
            ui.colored_label(k.faint, format!("{} B", pane.machine.response_bytes_seen()));
        });
        ui.separator();

        if pane.machine.is_empty() {
            ui.colored_label(k.faint, "that program has no cells");
            return;
        }

        // Wall-clock paced, not frame-paced: the same run must look the same on
        // a 60Hz and a 144Hz display.
        if pane.running && pane.last_step.elapsed().as_secs_f32() >= pane.interval {
            pane.machine.step();
            pane.last_step = std::time::Instant::now();
        }
        if pane.running {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_secs_f32(pane.interval.min(0.25)));
        }

        draw_generation(ui, &pane.machine, k);
    }

    /// Open the active drawing's exact one-to-one source in an ordinary tab.
    fn open_generated_source(&mut self) {
        match self.doc().mandala.to_rebis() {
            Ok(text) => {
                self.tabs
                    .open("source", Pane::Source(SourcePane::with_text(text)));
            }
            Err(error) => self.notice = Some(error.to_string()),
        }
    }

    /// The open drawings. Each keeps its own canvas, viewport and selection,
    /// so switching back returns you exactly where you were.
    fn tab_bar(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let active = self.tabs.active_id();
                let mut select: Option<TabId> = None;
                let mut close: Option<TabId> = None;
                let mut reorder: Option<(TabId, usize)> = None;
                let mut settings = false;
                let mut runs = false;
                let mut actions = false;
                let mut generation = false;
                let ids: Vec<TabId> = self.tabs.iter().map(|tab| tab.id).collect();
                for (index, tab) in self.tabs.iter().enumerate() {
                    let on = Some(tab.id) == active;
                    // A short press is a click; a held/moved press becomes a
                    // drag payload and is accepted by the other tab zones.
                    let dropped = ui
                        .dnd_drop_zone::<usize, ()>(egui::Frame::none(), |ui| {
                            let response = ui
                                .selectable_label(on, &tab.title)
                                .interact(Sense::click_and_drag());
                            response.dnd_set_drag_payload(index);
                            if response.clicked() {
                                select = Some(tab.id);
                            }
                        })
                        .1;
                    if let Some(from) = dropped {
                        if let Some(&from_id) = ids.get(*from) {
                            reorder = Some((from_id, index));
                        }
                    }
                    // Only the active tab offers its close button, so the bar
                    // stays quiet and a stray click cannot shut the wrong one.
                    if on && self.tabs.len() > 1 && ui.small_button("×").clicked() {
                        close = Some(tab.id);
                    }
                    ui.separator();
                }
                if ui.small_button("+ mandala").clicked() {
                    let n = self.tabs.len() + 1;
                    self.tabs
                        .open(format!("mandala {n}"), Pane::Mandala(Doc::default()));
                }
                if ui.small_button("+ chat").clicked() {
                    self.tabs.open("chat", Pane::Chat(ChatPane::default()));
                }
                if ui.small_button("+ source").clicked() {
                    self.tabs
                        .open("source", Pane::Source(SourcePane::default()));
                }
                if ui.small_button("+ sigils").clicked() {
                    self.tabs.open("sigils", Pane::Sigils(SigilPane::default()));
                }
                if ui
                    .small_button("+ generation")
                    .on_hover_text(
                        "run the selected run as a cellular automaton: its geometry is the \
                         lattice, the model's own bytes are the rule",
                    )
                    .clicked()
                {
                    generation = true;
                }
                if ui.small_button("settings").clicked() {
                    settings = true;
                }
                let run_label = if self.runs.active_count() == 0 {
                    "runs".to_string()
                } else {
                    format!("runs {}", self.runs.active_count())
                };
                if ui.small_button(run_label).clicked() {
                    runs = true;
                }
                let action_label = if self.actions.active_count() == 0 {
                    "actions".to_string()
                } else {
                    format!("actions {}", self.actions.active_count())
                };
                if ui.small_button(action_label).clicked() {
                    actions = true;
                }
                if let Some(id) = select {
                    self.tabs.select(id);
                }
                if let Some((id, to)) = reorder {
                    self.tabs.reorder(id, to);
                }
                if let Some(id) = close {
                    self.tabs.close(id);
                }
                if generation {
                    self.open_generation();
                }
                if settings {
                    self.open_settings();
                }
                if runs {
                    self.open_runs();
                }
                if actions {
                    self.open_actions();
                }
            });
        });
    }

    fn open_settings(&mut self) {
        let existing = self
            .tabs
            .iter()
            .find_map(|tab| matches!(tab.content, Pane::Settings(_)).then_some(tab.id));
        if let Some(id) = existing {
            self.tabs.select(id);
        } else {
            self.tabs
                .open("settings", Pane::Settings(settings::SettingsPane::load()));
        }
    }

    fn open_runs(&mut self) {
        let existing = self
            .tabs
            .iter()
            .find_map(|tab| matches!(tab.content, Pane::Runs).then_some(tab.id));
        if let Some(id) = existing {
            self.tabs.select(id);
        } else {
            self.tabs.open("runs", Pane::Runs);
        }
    }

    fn open_actions(&mut self) {
        let existing = self
            .tabs
            .iter()
            .find_map(|tab| matches!(tab.content, Pane::Actions).then_some(tab.id));
        if let Some(id) = existing {
            self.tabs.select(id);
        } else {
            self.tabs.open("actions", Pane::Actions);
        }
    }

    /// All persistent configuration plus the settings that only make sense for
    /// this open visual session. Persistent values use the exact config keys
    /// documented by Kaos; no visual-only shadow copy is introduced.
    fn settings(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut save = false;
        let mut reload = false;
        let mut restore = false;
        let mut theme_change = None;
        let mut apply_cwd = false;
        // Deferred out of the closure for the same reason every other mutation
        // here is: the pane is borrowed while it draws.
        let mut credential_store: Option<(String, String)> = None;
        let mut credential_forget: Option<String> = None;
        let cwd_now = self.cwd.display().to_string();
        let cwd_edit = &mut self.cwd_edit;
        let Some(Pane::Settings(pane)) = self.tabs.active_mut() else {
            return;
        };

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.accent, "SETTINGS");
            ui.colored_label(k.faint, kaos_core::config::path().display().to_string());
            let dirty = pane.dirty();
            if dirty > 0 {
                ui.colored_label(k.accent, format!("{dirty} unsaved"));
            }
            if ui.button("save persistent").clicked() {
                save = true;
            }
            if ui.button("reload").clicked() {
                reload = true;
            }
            if ui.button("restore defaults").clicked() {
                restore = true;
            }
        });
        if let Some(note) = &pane.notice {
            ui.colored_label(k.faint, note);
        }
        ui.colored_label(
            k.faint,
            "Precedence: explicit uppercase environment values override the file; file-only editor settings stay local.",
        );
        ui.separator();

        egui::ScrollArea::vertical().show(ui, |ui| {
            egui::CollapsingHeader::new("SESSION ONLY")
                .default_open(true)
                .show(ui, |ui| {
                    ui.colored_label(
                        k.faint,
                        "These affect this window and are not written to the config file.",
                    );
                    ui.horizontal(|ui| {
                        ui.label("working directory");
                        ui.add(
                            egui::TextEdit::singleline(cwd_edit)
                                .desired_width(420.0)
                                .hint_text(cwd_now),
                        );
                        if ui.button("apply").clicked() {
                            apply_cwd = true;
                        }
                    });
                    ui.colored_label(
                        k.faint,
                        "Run mode, authority, lane, and input are kept in the Runs tab.",
                    );
                });

            // ── credentials ─────────────────────────────────────────────
            //
            // Not config keys, and rendered from their own store on purpose:
            // the settings file is ordinary text, the credential file is 0600,
            // and a key that reaches the wrong one of those is a key that ends
            // up in a pasted bug report. Nothing here ever renders a stored
            // value — presence and length only.
            ui.add_space(8.0);
            egui::CollapsingHeader::new("CREDENTIALS")
                .default_open(true)
                .show(ui, |ui| {
                    ui.colored_label(
                        k.faint,
                        "Provider keys, stored 0600 outside the settings file. A stored key is never shown again — only whether one is present.",
                    );
                    for (provider, var, live, saved_key) in kaos_agent::auth::status() {
                        ui.horizontal(|ui| {
                            ui.colored_label(
                                if live { k.accent } else { k.faint },
                                format!("{provider:<11}"),
                            );
                            ui.colored_label(
                                k.faint,
                                match (live, saved_key) {
                                    (true, true) => "set".to_string(),
                                    // An env-only key works now and vanishes
                                    // with the shell, which is worth saying.
                                    (true, false) => "set in this environment only".to_string(),
                                    (false, true) => "saved, not in this environment".to_string(),
                                    (false, false) => "unset".to_string(),
                                },
                            );
                            ui.colored_label(k.faint, var);
                            let edit = pane
                                .credentials
                                .entry(provider.to_string())
                                .or_default();
                            ui.add(
                                egui::TextEdit::singleline(&mut edit.entry)
                                    .desired_width(240.0)
                                    .password(true)
                                    .hint_text("paste a key"),
                            );
                            let typed = !edit.entry.trim().is_empty();
                            if ui.add_enabled(typed, egui::Button::new("store")).clicked() {
                                credential_store =
                                    Some((provider.to_string(), edit.entry.trim().to_string()));
                            }
                            if ui.add_enabled(saved_key, egui::Button::new("forget")).clicked() {
                                credential_forget = Some(provider.to_string());
                            }
                        });
                    }
                    ui.colored_label(
                        k.faint,
                        "The claude CLI authenticates through its own login and needs no key here.",
                    );
                });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.colored_label(k.faint, "PERSISTENT");
                ui.add(
                    egui::TextEdit::singleline(&mut pane.filter)
                        .desired_width(280.0)
                        .hint_text("filter settings"),
                );
            });
            ui.colored_label(
                k.faint,
                "Every entry below shows its type, documented default, behavior, and a copyable example.",
            );
            let needle = pane.filter.trim().to_ascii_lowercase();
            for group in settings::Group::ALL {
                let keys = kaos_core::config::CONFIG_KEYS
                    .iter()
                    .copied()
                    .filter(|key| settings::group(key) == group)
                    .filter(|key| {
                        needle.is_empty()
                            || key.to_ascii_lowercase().contains(&needle)
                            || settings::documentation(key).is_some_and(|doc| {
                                doc.summary.to_ascii_lowercase().contains(&needle)
                                    || doc.details.to_ascii_lowercase().contains(&needle)
                                    || doc.example.to_ascii_lowercase().contains(&needle)
                            })
                    })
                    .collect::<Vec<_>>();
                if keys.is_empty() {
                    continue;
                }
                egui::CollapsingHeader::new(group.label())
                    .default_open(matches!(
                        group,
                        settings::Group::Appearance | settings::Group::Mind
                    ))
                    .show(ui, |ui| {
                        for key in keys {
                            let doc = settings::documentation(key)
                                .expect("every persistent setting has documentation");
                            ui.push_id(key, |ui| {
                                ui.horizontal(|ui| {
                                    ui.monospace(key);
                                    ui.colored_label(k.faint, doc.kind.label());
                                    if key == "theme" {
                                        let value = pane
                                            .values
                                            .entry(key.to_string())
                                            .or_insert_with(|| "dark".to_string());
                                        let before = value.clone();
                                        egui::ComboBox::from_id_salt("theme")
                                            .selected_text(value.as_str())
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    value,
                                                    "dark".to_string(),
                                                    "dark",
                                                );
                                                ui.selectable_value(
                                                    value,
                                                    "light".to_string(),
                                                    "light",
                                                );
                                            });
                                        if *value != before {
                                            theme_change =
                                                kaos_core::theme::Mode::parse(value.as_str());
                                        }
                                    } else if settings::is_tristate(key) {
                                        let value = pane.values.entry(key.to_string()).or_default();
                                        let selected = match value.trim() {
                                            "1" | "true" | "yes" | "on" => "shell authority",
                                            "0" | "false" | "no" => "edits only",
                                            "" => "ask when needed",
                                            _ => "custom value",
                                        };
                                        egui::ComboBox::from_id_salt(("setting-tristate", key))
                                            .selected_text(selected)
                                            .show_ui(ui, |ui| {
                                                ui.selectable_value(
                                                    value,
                                                    String::new(),
                                                    "ask when needed",
                                                );
                                                ui.selectable_value(
                                                    value,
                                                    "1".to_string(),
                                                    "shell authority",
                                                );
                                                ui.selectable_value(
                                                    value,
                                                    "0".to_string(),
                                                    "edits only",
                                                );
                                            });
                                    } else if settings::is_boolean(key) {
                                        let value = pane.values.entry(key.to_string()).or_default();
                                        let mut enabled = matches!(
                                            value.trim().to_ascii_lowercase().as_str(),
                                            "1" | "true" | "yes" | "on"
                                        );
                                        if ui.checkbox(&mut enabled, "").changed() {
                                            *value = enabled.to_string();
                                        }
                                    } else {
                                        let value = pane.values.entry(key.to_string()).or_default();
                                        ui.add(
                                            egui::TextEdit::singleline(value).desired_width(360.0),
                                        );
                                    }
                                });
                                let default = kaos_core::config::default_value(key)
                                    .unwrap_or_default();
                                let default = if default.is_empty() {
                                    "(empty)".to_string()
                                } else {
                                    default
                                };
                                ui.colored_label(
                                    k.faint,
                                    format!("{} · default: {default}", doc.summary),
                                );
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(doc.details).color(k.faint),
                                    )
                                    .wrap(),
                                );
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(format!("example: {}", doc.example))
                                            .monospace(),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(4.0);
                            });
                        }
                    });
            }

            let environment_docs = settings::ENVIRONMENT_DOCS
                .iter()
                .filter(|doc| {
                    needle.is_empty()
                        || doc.key.to_ascii_lowercase().contains(&needle)
                        || doc.details.to_ascii_lowercase().contains(&needle)
                })
                .collect::<Vec<_>>();
            if !environment_docs.is_empty() {
                egui::CollapsingHeader::new("ENVIRONMENT ONLY & SECRETS")
                    .default_open(false)
                    .show(ui, |ui| {
                        ui.colored_label(
                            k.faint,
                            "These values are intentionally not editable or persisted by the Settings tab.",
                        );
                        for doc in environment_docs {
                            ui.push_id(doc.key, |ui| {
                                ui.monospace(doc.key);
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(doc.details).color(k.faint),
                                    )
                                    .wrap(),
                                );
                                ui.add_space(3.0);
                            });
                        }
                    });
            }
        });

        if save {
            pane.notice = Some(match pane.save() {
                Ok(0) => "persistent configuration is already saved".to_string(),
                Ok(count) => format!("saved {count} persistent setting(s)"),
                Err(error) => error,
            });
        }
        if reload {
            pane.reload();
        }
        if restore {
            pane.notice = Some(match pane.restore() {
                Ok(()) => "restored documented defaults".to_string(),
                Err(error) => error,
            });
            theme_change = Some(kaos_core::theme::mode());
        }

        // End the pane borrow before changing editor-wide state.
        let _ = pane;
        if let Some(mode) = theme_change {
            let result = if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                pane.save_key("theme")
            } else {
                Ok(())
            };
            match result {
                Ok(()) => {
                    self.ink = Ink::load();
                    install_theme(ui.ctx(), self.ink);
                    ui.ctx().request_repaint();
                }
                Err(error) => {
                    if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                        pane.notice = Some(error);
                    }
                }
            }
            let _ = mode;
        }
        // Credentials, applied after the draw. The typed value is cleared on
        // both paths — a key that stays in UI state is a key in a screenshot.
        if let Some((provider, key)) = credential_store {
            let outcome = match kaos_agent::auth::store(&provider, &key) {
                Ok((var, path)) => format!("stored {provider} as {var} in {}", path.display()),
                Err(error) => format!("could not store {provider}: {error}"),
            };
            if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                pane.credentials.remove(&provider);
                pane.notice = Some(outcome);
            }
        }
        if let Some(provider) = credential_forget {
            let outcome = match kaos_agent::auth::forget(&provider) {
                Ok(var) => format!("forgot {provider} ({var})"),
                Err(error) => format!("could not forget {provider}: {error}"),
            };
            if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                pane.credentials.remove(&provider);
                pane.notice = Some(outcome);
            }
        }
        if apply_cwd {
            let candidate = std::path::PathBuf::from(self.cwd_edit.trim());
            match candidate.canonicalize() {
                Ok(path) if path.is_dir() => {
                    self.cwd = path;
                    self.cwd_edit = self.cwd.display().to_string();
                    if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                        pane.notice = Some("session working directory changed".to_string());
                    }
                }
                _ => {
                    if let Some(Pane::Settings(pane)) = self.tabs.active_mut() {
                        pane.notice = Some(format!("not a directory: {}", self.cwd_edit.trim()));
                    }
                }
            }
        }
    }

    /// A conversation tab: browse the saved sessions, or read and extend one.
    ///
    /// These are the same sessions `/resume` reads in the terminal app — same
    /// store, same format — so a conversation started in either interface can
    /// be picked up in the other.
    fn chat(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut submission: Option<(String, String, bool, Option<u64>, Vec<(String, String)>)> =
            None;
        let session_id = match self.tabs.active() {
            Some(Pane::Chat(chat)) => Some(chat.session.id.clone()),
            _ => None,
        };
        let run_id = self.tabs.active().and_then(|pane| match pane {
            Pane::Chat(chat) if !chat.browsing => chat.run_id,
            _ => None,
        });
        let run_snapshot = run_id.and_then(|id| self.run_chat_snapshot(id));
        let chat_busy = session_id
            .as_deref()
            .is_some_and(|id| self.actions.session_active(id));
        let Some(Pane::Chat(chat)) = self.tabs.active_mut() else {
            return;
        };

        if chat.browsing {
            let store = kaos_core::sessions::Store::default_store();
            let list = store.list();
            let mut forget = None;
            ui.add_space(8.0);
            ui.colored_label(k.faint, "SESSIONS");
            if let Some(notice) = &chat.notice {
                ui.colored_label(k.faint, notice);
            }
            if list.is_empty() {
                ui.colored_label(k.faint, "none saved yet — start typing below");
            }
            let mut resume = None;
            egui::ScrollArea::vertical()
                .max_height(ui.available_height() - 90.0)
                .show(ui, |ui| {
                    for s in &list {
                        let line = format!("{:>3} turns   {}", s.turns, s.title);
                        ui.horizontal(|ui| {
                            if ui.selectable_label(false, line).clicked() {
                                resume = Some(s.id.clone());
                            }
                            if ui.small_button("forget").clicked() {
                                forget = Some(s.id.clone());
                            }
                        });
                    }
                });
            if let Some(id) = forget {
                chat.notice = Some(match store.delete(&id) {
                    Ok(()) => format!("forgot session {id}"),
                    Err(error) => format!("could not forget session: {error}"),
                });
            }
            if let Some(id) = resume {
                if let Ok(loaded) = store.load(&id) {
                    chat.session = loaded;
                    chat.browsing = false;
                }
            }
            if ui.button("new conversation").clicked() {
                chat.session = kaos_core::sessions::Session::new(
                    kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".to_string()),
                    self.cwd.display().to_string(),
                );
                chat.browsing = false;
            }
        } else {
            ui.add_space(8.0);
            // The run notice is a long line; wrapping stops it pushing the
            // `sessions` button past the panel edge.
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(k.faint, chat.session.title());
                if let Some(run_id) = chat.run_id {
                    ui.colored_label(
                        k.accent,
                        format!("run #{run_id} · retained content shown below · refreshed live"),
                    );
                }
                if chat_busy {
                    ui.colored_label(k.accent, "● working");
                }
                if ui.small_button("sessions").clicked() {
                    chat.browsing = true;
                }
                if ui.small_button("new").clicked() {
                    chat.session = kaos_core::sessions::Session::new(
                        kaos_core::config::value("KAOS_MODEL").unwrap_or_else(|| "sim".to_string()),
                        self.cwd.display().to_string(),
                    );
                }
            });
            ui.separator();
            egui::ScrollArea::vertical()
                .stick_to_bottom(true)
                .max_height(ui.available_height() - 60.0)
                .show(ui, |ui| {
                    if let Some(snapshot) = run_snapshot.as_deref() {
                        ui.horizontal_top(|ui| {
                            ui.colored_label(k.accent, "run");
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(snapshot).monospace().color(k.faint),
                                )
                                .wrap(),
                            );
                        });
                        ui.separator();
                    }
                    for turn in &chat.session.turns {
                        let (who, tone) = match turn.role {
                            kaos_core::sessions::Role::User => ("you", k.ink),
                            kaos_core::sessions::Role::Model => ("model", k.faint),
                        };
                        ui.horizontal_top(|ui| {
                            ui.colored_label(tone, format!("{who:<6}"));
                            ui.add(egui::Label::new(egui::RichText::new(&turn.text)).wrap());
                        });
                    }
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            let send = ui
                .add(
                    egui::TextEdit::singleline(&mut chat.input)
                        .desired_width((ui.available_width() - 70.0).max(80.0))
                        .hint_text("say something"),
                )
                .lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (send || ui.button("send").clicked()) && !chat.input.trim().is_empty() {
                chat.browsing = false;
                let said = std::mem::take(&mut chat.input);
                let resume = chat
                    .session
                    .turns
                    .iter()
                    .any(|turn| turn.role == kaos_core::sessions::Role::Model);
                chat.session
                    .push(kaos_core::sessions::Role::User, said.clone());
                let mut history = Vec::new();
                let prior_turns = chat.session.turns.len().saturating_sub(1);
                for turn in chat.session.turns.iter().take(prior_turns) {
                    match turn.role {
                        kaos_core::sessions::Role::User => {
                            history.push((turn.text.clone(), String::new()));
                        }
                        kaos_core::sessions::Role::Model => {
                            if let Some((_, answer)) = history.last_mut() {
                                *answer = turn.text.clone();
                            }
                        }
                    }
                }
                let run_id = chat.run_id;
                // Persist immediately: the terminal app saves on every turn for
                // the same reason, so a crash loses nothing already said.
                let _ = kaos_core::sessions::Store::default_store().save(&chat.session);
                submission = Some((said, chat.session.id.clone(), resume, run_id, history));
            }
        });
        let _ = chat;
        if let Some((said, session, resume, run_id, history)) = submission {
            let history_text = history
                .iter()
                .map(|(user, assistant)| format!("USER: {user}\nASSISTANT: {assistant}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = run_id
                .and_then(|id| self.run_chat_context(id, &history, &said))
                .unwrap_or_else(|| {
                    kaos_core::chat::DEFAULT_CONTEXT.render_chat(&history_text, &said)
                });
            self.actions.submit_chat(prompt, session, resume, &self.cwd);
        }
    }

    fn run_chat_context(
        &self,
        id: u64,
        history: &[(String, String)],
        question: &str,
    ) -> Option<String> {
        self.with_run_snapshot(id, |snapshot| {
            kaos_core::chat::render_run_chat(snapshot, history, question)
        })
    }

    fn with_run_snapshot<T>(
        &self,
        id: u64,
        render: impl FnOnce(&kaos_core::chat::RunSnapshot<'_>) -> T,
    ) -> Option<T> {
        let child = self.runs.has_live_process(id);
        let run = self.runs.runs.iter().find(|run| run.id == id)?;
        let timer = run.timer();
        let snapshot = kaos_core::chat::RunSnapshot {
            id: run.id,
            state: run.state.label(run.paused),
            paused: run.paused,
            pause_reason: run.pause_reason.as_deref().unwrap_or("none"),
            scope: run.scope.label(),
            mode: run.mode.label(),
            lane: if run.parallel() { "parallel" } else { "serial" },
            timer: &timer,
            child: if child { "yes" } else { "no" },
            source: &run.source,
            input: &run.input,
            output: &run.output,
        };
        Some(render(&snapshot))
    }

    fn run_chat_snapshot(&self, id: u64) -> Option<String> {
        self.with_run_snapshot(id, kaos_core::chat::render_run_snapshot)
    }

    fn open_run_chat(&mut self, id: u64) {
        if !self.runs.runs.iter().any(|run| run.id == id) {
            self.notice = Some(format!("run #{id} is no longer retained"));
            return;
        }
        let existing = self.tabs.iter().find_map(|tab| {
            matches!(&tab.content, Pane::Chat(chat) if chat.run_id == Some(id)).then_some(tab.id)
        });
        if let Some(tab_id) = existing {
            self.tabs.select(tab_id);
            return;
        }
        let mut chat = ChatPane::default();
        chat.browsing = false;
        chat.run_id = Some(id);
        chat.notice = Some(
            "each message refreshes source, state, and the complete retained run output"
                .to_string(),
        );
        self.tabs.open(format!("run #{id} chat"), Pane::Chat(chat));
    }

    /// The sigil library. Opening one parses it and lays it out as a drawing,
    /// so a saved program becomes a mandala without a round trip through text.
    fn sigils(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let Some(Pane::Sigils(pane)) = self.tabs.active_mut() else {
            return;
        };
        let mut action = None;
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.faint, "SIGILS");
            ui.add(
                egui::TextEdit::singleline(&mut pane.query)
                    .desired_width(220.0)
                    .hint_text("search"),
            );
        });
        if let Some(note) = pane.notice.clone() {
            ui.colored_label(k.faint, note);
        }
        ui.separator();
        let lib = kaos_core::sigils::Library::default_library();
        let found = lib.search_catalog(&pane.query);
        if found.is_empty() {
            ui.colored_label(k.faint, "no personal or collection sigils match");
        }
        let personal = found
            .iter()
            .filter(|entry| !entry.read_only)
            .collect::<Vec<_>>();
        let standard = found
            .iter()
            .filter(|entry| entry.name.starts_with("std/"))
            .collect::<Vec<_>>();
        let collection = found
            .iter()
            .filter(|entry| entry.read_only && !entry.name.starts_with("std/"))
            .collect::<Vec<_>>();
        egui::ScrollArea::vertical().show(ui, |ui| {
            if !personal.is_empty() {
                ui.colored_label(k.faint, "PERSONAL");
                for entry in personal {
                    sigil_catalog_row(ui, k, pane, entry, &mut action);
                }
                ui.add_space(8.0);
            }
            if !standard.is_empty() {
                egui::CollapsingHeader::new(format!(
                    "std/ · {} embedded read-only sigils",
                    standard.len()
                ))
                // Open by default: the embedded standard library is part of the
                // catalog, not an appendix to it, so every module is in reach
                // without first expanding a header.
                .default_open(true)
                .show(ui, |ui| {
                    for entry in standard {
                        sigil_catalog_row(ui, k, pane, entry, &mut action);
                    }
                });
            }
            if !collection.is_empty() {
                egui::CollapsingHeader::new(format!(
                    "rebis-collection/ · {} read-only modules",
                    collection.len()
                ))
                .default_open(true)
                .show(ui, |ui| {
                    for entry in collection {
                        sigil_catalog_row(ui, k, pane, entry, &mut action);
                    }
                });
            }
        });

        let _ = pane;
        if let Some(action) = action {
            match action {
                SigilAction::Draw(entry) => {
                    let name = entry.name;
                    match lib.load_catalog(&name) {
                        Ok(source) => match Mandala::from_rebis(&source) {
                            Ok(mandala) => {
                                self.tabs.open(
                                    name,
                                    Pane::Mandala(Doc {
                                        mandala,
                                        ..Doc::default()
                                    }),
                                );
                            }
                            Err(error) => {
                                self.set_sigil_notice(format!("{name}: {error}"));
                            }
                        },
                        Err(error) => self.set_sigil_notice(format!("{name}: {error}")),
                    }
                }
                SigilAction::Edit(entry) => {
                    let read_only = entry.read_only;
                    let name = entry.name;
                    match lib.load_catalog(&name) {
                        Ok(text) => {
                            self.tabs.open(
                                name.clone(),
                                Pane::Source(SourcePane {
                                    name,
                                    notice: read_only.then(|| {
                                        "embedded std · read only · choose a new sigil name to save a copy"
                                            .to_string()
                                    }),
                                    ..SourcePane::with_text(text)
                                }),
                            );
                        }
                        Err(error) => self.set_sigil_notice(error.to_string()),
                    }
                }
                SigilAction::Chat(entry) => {
                    let name = entry.name;
                    match lib.load_catalog(&name) {
                        Ok(text) => {
                            self.actions.attach_text(format!("{name}.rebis"), text);
                            self.tabs.open("chat", Pane::Chat(ChatPane::default()));
                            if let Some(Pane::Chat(chat)) = self.tabs.active_mut() {
                                chat.browsing = false;
                                chat.input = format!(
                                    "Inspect the attached {name} sigil and propose a concrete improvement."
                                );
                            }
                        }
                        Err(error) => self.set_sigil_notice(error.to_string()),
                    }
                }
                SigilAction::Delete(entry) => {
                    let name = entry.name;
                    let message = match lib.delete(&name) {
                        Ok(()) => format!("deleted {name}"),
                        Err(error) => format!("could not delete {name}: {error}"),
                    };
                    if let Some(Pane::Sigils(pane)) = self.tabs.active_mut() {
                        pane.pending_delete = None;
                        pane.notice = Some(message);
                    }
                }
            }
        }
    }

    fn set_sigil_notice(&mut self, notice: String) {
        if let Some(Pane::Sigils(pane)) = self.tabs.active_mut() {
            pane.notice = Some(notice);
        } else {
            self.notice = Some(notice);
        }
    }

    /// A source tab: Rebis as text, checked as you type.
    ///
    /// Validation, saving and drawing all go through the same code the
    /// terminal app uses — `rebis_lang::parse`, `sigils::Library`, and
    /// `Mandala::from_rebis` — so a program means one thing in both.
    fn source(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut actions = Vec::new();
        let Some(Pane::Source(pane)) = self.tabs.active_mut() else {
            return;
        };

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.faint, "SIGIL");
            ui.add(
                egui::TextEdit::singleline(&mut pane.name)
                    .desired_width(240.0)
                    .hint_text("team/reviews"),
            );
            if ui.button("save sigil").clicked() {
                actions.push(SourceAction::SaveSigil {
                    name: pane.name.clone(),
                    text: pane.editor.source().to_string(),
                });
            }
            if ui.button("mandala").clicked() {
                actions.push(SourceAction::Draw(pane.editor.source().to_string()));
            }
            if ui
                .button("generation")
                .on_hover_text("open this Rebis source as a live byte automaton on a new tab")
                .clicked()
            {
                actions.push(SourceAction::Generation(pane.editor.source().to_string()));
            }
            if ui.button("format").clicked() {
                actions.push(SourceAction::Format);
            }
            if ui.button("run").clicked() {
                match pane.run_options_source() {
                    Ok((program, block)) => actions.push(SourceAction::Run { program, block }),
                    Err(error) => pane.notice = Some(error),
                }
            }
            if ui.button("sigil chat").clicked() {
                actions.push(SourceAction::OpenSigilChat(
                    pane.editor.source().to_string(),
                ));
            }
        });

        egui::CollapsingHeader::new("FILE, SEARCH & OUTPUT").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.colored_label(k.faint, "FILE");
                ui.add(
                    egui::TextEdit::singleline(&mut pane.file_path)
                        .desired_width(420.0)
                        .hint_text("program.rebis"),
                );
                if ui.button("open").clicked() {
                    actions.push(SourceAction::OpenFile(pane.file_path.clone()));
                }
                if ui.button("save file").clicked() {
                    actions.push(SourceAction::SaveFile {
                        path: pane.file_path.clone(),
                        text: pane.editor.source().to_string(),
                    });
                }
                if ui.button("save as…").clicked() {
                    actions.push(SourceAction::ChooseSaveFile {
                        suggested: pane.file_path.clone(),
                        text: pane.editor.source().to_string(),
                    });
                }
            });
            ui.horizontal(|ui| {
                ui.colored_label(k.faint, "SEARCH");
                ui.add(
                    egui::TextEdit::singleline(&mut pane.search)
                        .desired_width(300.0)
                        .hint_text("text"),
                );
                let matches = if pane.search.is_empty() {
                    0
                } else {
                    pane.editor
                        .source()
                        .lines()
                        .filter(|line| line.contains(&pane.search))
                        .count()
                };
                ui.colored_label(k.faint, format!("{matches} matching line(s)"));
            });
            ui.horizontal(|ui| {
                ui.colored_label(k.faint, "RECORD");
                ui.add(
                    egui::TextEdit::singleline(&mut pane.record_path)
                        .desired_width(320.0)
                        .hint_text("evidence.txt"),
                );
                if ui.button("load into runs").clicked() {
                    actions.push(SourceAction::LoadRecord(pane.record_path.clone()));
                }
                ui.separator();
                ui.colored_label(k.faint, "OUTPUT");
                ui.add(
                    egui::TextEdit::singleline(&mut pane.output_path)
                        .desired_width(260.0)
                        .hint_text("projection.txt"),
                );
            });
        });

        let parsed = rebis_lang::parse(pane.editor.source());
        if let Some(note) = pane.notice.clone() {
            ui.colored_label(k.faint, note);
        }
        source_status(ui, k, pane.editor.source());
        ui.horizontal(|ui| {
            let mut vim_enabled = pane.vim_enabled;
            if ui
                .toggle_value(
                    &mut vim_enabled,
                    if pane.vim_enabled {
                        "Vim mode · ON"
                    } else {
                        "Vim mode · OFF"
                    },
                )
                .on_hover_text("Toggle this source session; Settings controls the default")
                .changed()
            {
                pane.set_vim_enabled(vim_enabled);
            }
            ui.colored_label(
                if pane.vim_enabled { k.accent } else { k.faint },
                pane.mode.label(),
            );
            let (row, column) = pane.editor.row_col();
            ui.colored_label(k.faint, format!("{}:{}", row + 1, column + 1));
            ui.separator();
            ui.selectable_value(&mut pane.projection, SourceProjection::Editor, "source");
            ui.selectable_value(&mut pane.projection, SourceProjection::Tree, "tree");
            ui.selectable_value(
                &mut pane.projection,
                SourceProjection::Mandala,
                "terminal mandala",
            );
        });
        ui.separator();

        let projection = match (&pane.projection, &parsed) {
            (SourceProjection::Editor, _) => None,
            (SourceProjection::Tree, Ok(expr)) => Some(rebis_lang::tree(expr)),
            (SourceProjection::Mandala, Ok(expr)) => Some(rebis_lang::mandala(expr)),
            (_, Err(error)) => Some(error.to_string()),
        };
        if let Some(text) = &projection {
            ui.horizontal(|ui| {
                if ui.button("copy projection").clicked() {
                    actions.push(SourceAction::Copy(text.clone()));
                }
                if ui.button("write projection").clicked() {
                    actions.push(SourceAction::WriteProjection {
                        path: pane.output_path.clone(),
                        text: text.clone(),
                    });
                }
            });
        }
        if let Some(projection) = projection {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.add(egui::Label::new(egui::RichText::new(projection).monospace()).wrap());
            });
        } else {
            draw_source_editor(ui, pane, k, &mut actions);
            egui::ScrollArea::vertical()
                .max_height(120.0)
                .show(ui, |ui| {
                    if !pane.search.is_empty() {
                        ui.separator();
                        for (line, text) in pane
                            .editor
                            .source()
                            .lines()
                            .enumerate()
                            .filter(|(_, text)| text.contains(&pane.search))
                        {
                            ui.monospace(format!("{:>4}  {text}", line + 1));
                        }
                    }
                });
        }

        let _ = pane;
        for action in actions {
            match action {
                SourceAction::SaveSigil { name, text } => {
                    let result = kaos_core::sigils::Library::default_library().save(&name, &text);
                    self.set_source_notice(match result {
                        Ok(path) => format!("saved {}", path.display()),
                        Err(error) => error.to_string(),
                    });
                }
                SourceAction::OpenFile(raw) => {
                    let path = self.resolve_path(&raw);
                    match std::fs::read_to_string(&path) {
                        Ok(text) => {
                            if let Some(Pane::Source(pane)) = self.tabs.active_mut() {
                                pane.editor = SourceEditor::new(text);
                                pane.file_path = path.display().to_string();
                                pane.projection = SourceProjection::Editor;
                                pane.notice = Some(format!("opened {}", path.display()));
                            }
                        }
                        Err(error) => {
                            self.set_source_notice(format!(
                                "could not open {}: {error}",
                                path.display()
                            ));
                        }
                    }
                }
                SourceAction::SaveFile { path: raw, text } => {
                    let path = self.resolve_path(&raw);
                    if raw.trim().is_empty() || path.is_dir() {
                        self.choose_source_save_path(&raw, text);
                    } else {
                        self.save_source_file(path, text);
                    }
                }
                SourceAction::ChooseSaveFile { suggested, text } => {
                    self.choose_source_save_path(&suggested, text);
                }
                SourceAction::Format => {
                    if let Some(Pane::Source(pane)) = self.tabs.active_mut() {
                        pane.notice = Some(match rebis_lang::parse(pane.editor.source()) {
                            Ok(expr) => {
                                pane.editor.replace(rebis_lang::pretty_format(&expr));
                                "formatted canonical Rebis".to_string()
                            }
                            Err(error) => format!("format: {error}"),
                        });
                    }
                }
                SourceAction::Draw(text) => match Mandala::from_rebis(&text) {
                    Ok(mandala) => {
                        self.tabs.open(
                            "mandala",
                            Pane::Mandala(Doc {
                                mandala,
                                ..Doc::default()
                            }),
                        );
                    }
                    Err(error) => self.set_source_notice(error.to_string()),
                },
                SourceAction::Generation(text) => {
                    self.open_generation_from_source(text);
                }
                SourceAction::Run { program, block } => {
                    let scope = if block.is_some() {
                        runs::Scope::Block
                    } else {
                        runs::Scope::Program
                    };
                    self.request_run_options(
                        program,
                        block,
                        std::collections::HashSet::new(),
                        scope,
                    );
                }
                SourceAction::Copy(text) => {
                    ui.ctx().copy_text(text);
                    self.set_source_notice("copied projection".to_string());
                }
                SourceAction::WriteProjection { path: raw, text } => {
                    let path = self.resolve_path(&raw);
                    self.set_source_notice(match std::fs::write(&path, text) {
                        Ok(()) => format!("wrote {}", path.display()),
                        Err(error) => format!("could not write {}: {error}", path.display()),
                    });
                }
                SourceAction::LoadRecord(raw) => {
                    let path = self.resolve_path(&raw);
                    match std::fs::read_to_string(&path) {
                        Ok(record) => {
                            self.runs.input = record;
                            self.set_source_notice(format!(
                                "loaded record from {}",
                                path.display()
                            ));
                        }
                        Err(error) => self.set_source_notice(format!(
                            "could not load record {}: {error}",
                            path.display()
                        )),
                    }
                }
                SourceAction::OpenSigilChat(text) => {
                    self.actions.attach_text("active-sigil.rebis", text);
                    self.tabs.open("chat", Pane::Chat(ChatPane::default()));
                    if let Some(Pane::Chat(chat)) = self.tabs.active_mut() {
                        chat.browsing = false;
                        chat.input = "Inspect this Rebis sigil and propose a concrete improvement."
                            .to_string();
                    }
                }
                SourceAction::VimCommand(command) => {
                    self.execute_visual_vim_command(&command);
                }
            }
        }
    }

    fn set_source_notice(&mut self, notice: String) {
        if let Some(Pane::Source(pane)) = self.tabs.active_mut() {
            pane.notice = Some(notice);
        } else {
            self.notice = Some(notice);
        }
    }

    fn save_source_file(&mut self, path: std::path::PathBuf, text: String) -> bool {
        match std::fs::write(&path, &text) {
            Ok(()) => {
                if let Some(Pane::Source(pane)) = self.tabs.active_mut() {
                    pane.editor.mark_clean();
                    pane.file_path = path.display().to_string();
                }
                self.set_source_notice(format!("saved {}", path.display()));
                true
            }
            Err(error) => {
                self.set_source_notice(format!("could not save {}: {error}", path.display()));
                false
            }
        }
    }

    fn choose_source_save_path(&mut self, suggested: &str, text: String) -> bool {
        let candidate = self.resolve_path(suggested);
        let directory = if candidate.is_dir() {
            candidate.clone()
        } else {
            candidate
                .parent()
                .filter(|path| path.is_dir())
                .map(|path| path.to_path_buf())
                .unwrap_or_else(|| self.cwd.clone())
        };
        let file_name = if candidate.is_dir() {
            "program.rebis".to_string()
        } else {
            candidate
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.is_empty())
                .unwrap_or("program.rebis")
                .to_string()
        };
        let selected = rfd::FileDialog::new()
            .set_title("Save Rebis source")
            .set_directory(directory)
            .set_file_name(file_name)
            .add_filter("Rebis source", &["rebis"])
            .save_file();
        match selected {
            Some(path) => self.save_source_file(path, text),
            None => {
                self.set_source_notice("save canceled".to_string());
                false
            }
        }
    }

    fn execute_visual_vim_command(&mut self, raw: &str) {
        let command = raw.trim();
        if command.is_empty() {
            return;
        }
        let Some(id) = self.tabs.active_id() else {
            return;
        };
        let Some(Pane::Source(pane)) = self.tabs.get(id) else {
            return;
        };
        let dirty = pane.editor.dirty();
        let current_path = pane.file_path.clone();
        let source = pane.editor.source().to_string();

        let save = |editor: &mut Self, requested: Option<&str>| -> bool {
            let raw_path = requested
                .filter(|path| !path.is_empty())
                .unwrap_or(&current_path);
            if raw_path.is_empty() {
                return editor.choose_source_save_path(raw_path, source.clone());
            }
            let path = editor.resolve_path(raw_path);
            if path.is_dir() {
                return editor.choose_source_save_path(raw_path, source.clone());
            }
            editor.save_source_file(path, source.clone())
        };

        match command {
            "q" | "quit" if dirty => {
                self.set_source_notice("unsaved changes · use :q! to discard or :w".to_string());
            }
            "q" | "quit" | "q!" => {
                if self.tabs.len() > 1 {
                    self.tabs.close(id);
                } else {
                    self.set_source_notice("the last visual tab stays open".to_string());
                }
            }
            "w" => {
                save(self, None);
            }
            "wq" => {
                if save(self, None) && self.tabs.len() > 1 {
                    self.tabs.close(id);
                }
            }
            _ if command.starts_with("w ") => {
                save(self, Some(command[2..].trim()));
            }
            _ if command.starts_with("e ") => {
                if dirty {
                    self.set_source_notice(
                        "unsaved changes · :w first or leave with :q!".to_string(),
                    );
                    return;
                }
                let requested = command[2..].trim();
                let path = self.resolve_path(requested);
                match std::fs::read_to_string(&path) {
                    Ok(source) => {
                        if let Some(Pane::Source(pane)) = self.tabs.get_mut(id) {
                            pane.editor = SourceEditor::new(source);
                            pane.file_path = path.display().to_string();
                            pane.projection = SourceProjection::Editor;
                            pane.notice = Some(format!("opened {}", path.display()));
                        }
                    }
                    Err(error) => self
                        .set_source_notice(format!("could not open {}: {error}", path.display())),
                }
            }
            _ => self.set_source_notice(format!("unknown Vim command :{command}")),
        }
    }

    fn resolve_path(&self, raw: &str) -> std::path::PathBuf {
        let path = std::path::Path::new(raw.trim());
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.cwd.join(path)
        }
    }

    fn palette(&mut self, ctx: &egui::Context) {
        egui::SidePanel::left("palette")
            .exact_width(180.0)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                ui.colored_label(self.ink.faint, "FORMS");
                for (label, make, _) in Form::ALL {
                    let form = make();
                    let on = self.tool == Tool::Place(form.clone());
                    if ui.selectable_label(on, *label).clicked() {
                        self.tool = Tool::Place(form.clone());
                        self.doc_mut().pending = None;
                    }
                }
                ui.colored_label(
                    self.ink.faint,
                    "place →/← alone, then use `father of` to add its ordered children",
                );
                ui.add_space(10.0);
                ui.colored_label(self.ink.faint, "LINK");
                ui.colored_label(self.ink.faint, "complete a flow between two forms");
                ui.colored_label(self.ink.faint, "father of: parent, then children in order");
                for (label, color, tool) in [
                    // One two-click shortcut. `(<- a b)` is `(-> b a)`, so the
                    // direction you see is the direction in the source.
                    (
                        "→  connect flow",
                        self.ink.secondary,
                        Tool::Flow(Form::Forward),
                    ),
                    ("┈  father of", self.ink.faint, Tool::Father),
                    ("▹  select", self.ink.ink, Tool::Select),
                ] {
                    if ui
                        .selectable_label(
                            self.tool == tool,
                            egui::RichText::new(label).color(color),
                        )
                        .clicked()
                    {
                        self.tool = tool;
                        self.doc_mut().pending = None;
                    }
                }
            });
    }

    /// Keep the drawing and its source in step.
    ///
    /// Whichever side changed wins: a canvas edit regenerates the text, and
    /// text that parses replaces the drawing. Text that does not parse is left
    /// alone — you are mid-sentence, and throwing the buffer away would be the
    /// wrong response to an incomplete one.
    fn sync(&mut self) {
        let fresh = self.doc().mandala.to_rebis().ok();
        let doc = self.doc_mut();
        match fresh {
            Some(src) if src != doc.generated => {
                // `generated` stays the canonical one-line form, so drift
                // detection above compares like with like. The panel shows the
                // readable, indented form: source generated from the drawing
                // (or loaded) always appears formatted.
                doc.generated = src.clone();
                doc.text = rebis_lang::parse(&src)
                    .map(|expr| rebis_lang::pretty_format(&expr))
                    .unwrap_or(src);
            }
            _ => {}
        }
    }

    /// Adopt source typed into the panel, if it parses.
    fn adopt_text(&mut self) {
        let text = self.doc().text.clone();
        if let Ok(mandala) = Mandala::from_rebis(&text) {
            let doc = self.doc_mut();
            doc.checkpoint();
            doc.mandala = mandala;
            doc.generated = doc.mandala.to_rebis().unwrap_or_default();
            doc.reset_interaction();
        }
    }

    /// Show and edit the selected form's one-based child positions. Positions
    /// are local to this parent: a child is `1..=n`, where `n` is the number of
    /// children attached to the selected form. Link creation supplies the
    /// initial order; changing a number only reorders those siblings.
    fn child_order_editor(&mut self, ui: &mut egui::Ui, father: NodeId, editable: bool) {
        let children = self.doc().mandala.children(father);
        if children.is_empty() {
            return;
        }
        ui.add_space(4.0);
        ui.colored_label(self.ink.faint, "CHILD ORDER · 1–n");
        ui.colored_label(
            self.ink.faint,
            "link order is the default; numbers are configurable",
        );
        let mut requested = None;
        for (index, child_id) in children.iter().copied().enumerate() {
            let caption = self
                .doc()
                .mandala
                .node(child_id)
                .map(|child| {
                    let caption = child.caption();
                    if caption.is_empty() {
                        child.form.name().to_string()
                    } else {
                        truncate(&caption)
                    }
                })
                .unwrap_or_else(|| "missing".to_string());
            let mut number = index + 1;
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        editable,
                        egui::DragValue::new(&mut number).range(1..=children.len()),
                    )
                    .changed()
                {
                    requested = Some((child_id, number));
                }
                ui.colored_label(self.ink.ink, caption);
            });
        }
        if let Some((child, number)) = requested {
            if self.doc().mandala.child_number(father, child) != Some(number) {
                let doc = self.doc_mut();
                doc.checkpoint();
                doc.mandala.set_child_number(father, child, number);
            }
        }
    }

    fn side(&mut self, ctx: &egui::Context) {
        let mut open_in_editor = false;
        let mut format_source = false;
        let mut format_drawing = false;
        let graph_editable = self.doc().canvas_mode == CanvasMode::Planar;
        // Spatial mode is read-only for graph topology, but it is still the
        // same selected mandala. Labels, prompts, macro names, and the source
        // buffer therefore remain editable while the user inspects the 3D
        // projection.
        let text_editable = true;
        egui::SidePanel::right("side")
            // Resizable, not exact: the source box and the REBIS controls are
            // as wide as the program in them, and a pane that cannot grow can
            // only clip. 330 stays the default so nothing moves on first run.
            .default_width(330.0)
            .width_range(300.0..=760.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.add_space(6.0);
                let selection_len = self.doc().selection_len();
                if selection_len > 1 {
                    ui.colored_label(self.ink.accent, format!("{selection_len} FORMS SELECTED"));
                    ui.colored_label(
                        self.ink.faint,
                        "Ctrl-click toggles · right-drag replaces the block",
                    );
                }
                if let Some(id) = self.doc().primary_selected() {
                    if let Some(node) = self.doc_mut().mandala.node(id).cloned() {
                        ui.colored_label(self.ink.faint, node.form.name().to_uppercase());
                        if text_editable && node.form.uses_text() {
                            let mut text = node.text.clone();
                            // A tall, wrapping field inside a height-capped
                            // scroll area: long text wraps and scrolls instead
                            // of being clipped to one line.
                            let changed = egui::ScrollArea::vertical()
                                .max_height(150.0)
                                .id_salt(("node-text", id.0))
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::TextEdit::multiline(&mut text)
                                            .desired_rows(4)
                                            .desired_width(f32::INFINITY),
                                    )
                                    .changed()
                                })
                                .inner;
                            if changed {
                                // Only a prompt may span lines; a name or path
                                // stays on one line so the source stays valid.
                                if !matches!(node.form, Form::Prompt) {
                                    text = text.replace(['\n', '\r'], "");
                                }
                                let doc = self.doc_mut();
                                doc.checkpoint();
                                doc.mandala.set_text(id, text);
                            }
                        }
                        if text_editable {
                            if let Form::Function(params) = &node.form {
                                let mut joined = params.join(" ");
                                if ui.text_edit_singleline(&mut joined).changed() {
                                    let ps: Vec<String> =
                                        joined.split_whitespace().map(str::to_string).collect();
                                    let doc = self.doc_mut();
                                    doc.checkpoint();
                                    doc.mandala.set_form(id, Form::Function(ps));
                                }
                                ui.colored_label(self.ink.faint, "parameters, space separated");
                            }
                        }
                        ui.colored_label(
                            self.ink.faint,
                            format!("takes {} ordered children", node.form.arity()),
                        );
                        self.child_order_editor(ui, id, graph_editable);
                        // The code of the selected block — the exact Rebis the
                        // selection generates on its own, so selecting a shape
                        // shows what that shape (and its operands) is.
                        ui.add_space(4.0);
                        ui.colored_label(
                            self.ink.faint,
                            if selection_len > 1 {
                                "SELECTED BLOCK"
                            } else {
                                "THIS BLOCK"
                            },
                        );
                        match self.doc().selected_source() {
                            Ok(Some(code)) => {
                                // Show the block in the readable, indented form.
                                let code = rebis_lang::parse(&code)
                                    .map(|expr| rebis_lang::pretty_format(&expr))
                                    .unwrap_or(code);
                                egui::ScrollArea::vertical()
                                    .max_height(160.0)
                                    .id_salt(("selected-source", id.0))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&code)
                                                    .monospace()
                                                    .color(self.ink.ink),
                                            )
                                            .wrap(),
                                        );
                                    });
                                if ui.small_button("copy block").clicked() {
                                    ui.ctx().copy_text(code);
                                }
                            }
                            Ok(None) => {}
                            Err(error) => {
                                ui.colored_label(
                                    self.ink.accent,
                                    format!("block is not one exact form: {error}"),
                                );
                            }
                        }
                        ui.add_space(4.0);
                        if graph_editable
                            && ui
                                .button(if selection_len > 1 {
                                    "delete selection"
                                } else {
                                    "delete shape"
                                })
                                .clicked()
                        {
                            self.doc_mut().delete_selected();
                        }
                        if !graph_editable {
                            ui.colored_label(
                                self.ink.faint,
                                "3D inspection · text edits are live; switch to 2D for graph edits",
                            );
                        }
                        ui.separator();
                    }
                }
                let k = self.ink;
                let exact = self.doc().mandala.to_rebis();
                // `open in editor` + `format` + `format mandala` overflow 330px
                // together; wrapping keeps the last button reachable.
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(k.faint, "REBIS");
                    let typed = self.doc().text.clone();
                    let status = if typed.trim().is_empty() {
                        String::new()
                    } else if let Err(error) = &exact {
                        error.to_string()
                    } else if rebis_lang::parse(&typed).is_err() {
                        "unparsed — the mandala is unchanged".to_string()
                    } else {
                        "exact · 1:1".to_string()
                    };
                    ui.colored_label(k.faint, status);
                    if exact.is_ok() && ui.small_button("open in editor").clicked() {
                        open_in_editor = true;
                    }
                    // Format the written source: reparse what is in the box and
                    // rewrite it in canonical indented form. Only ever applied
                    // to source that parses, so a half-typed program is never
                    // mangled.
                    if text_editable
                        && ui
                            .small_button("format")
                            .on_hover_text("rewrite the source in canonical form")
                            .clicked()
                    {
                        format_source = true;
                    }
                    // Redraw the drawing itself with the standard circuit
                    // layout, so a hand-dragged graph snaps back onto the grid.
                    if graph_editable
                        && ui
                            .small_button("format mandala")
                            .on_hover_text("re-lay the mandala out as a circuit")
                            .clicked()
                    {
                        format_drawing = true;
                    }
                });
                let mut edited = false;
                // Both directions: Rebis indentation carries meaning, so the
                // code box must not soft-wrap — which leaves scrolling as the
                // only way to reach a long line.
                egui::ScrollArea::both().show(ui, |ui| {
                    // Same colouring and the same parenthesis matching as the
                    // Source tab: this box holds Rebis too, and one without
                    // them reads as a different editor.
                    let id = egui::Id::new("side_source_edit");
                    let pair = matched_pair(ui.ctx(), id, &self.doc().text);
                    let mut layouter = |ui: &egui::Ui, text: &str, _wrap: f32| {
                        ui.fonts(|fonts| {
                            fonts.layout_job(rebis_layout_job(text, k, 12.5, &|_| false, pair))
                        })
                    };
                    let doc = self.doc_mut();
                    edited = ui
                        .add(
                            egui::TextEdit::multiline(&mut doc.text)
                                .id(id)
                                .code_editor()
                                .interactive(text_editable)
                                .desired_width(f32::INFINITY)
                                .desired_rows(24)
                                .layouter(&mut layouter),
                        )
                        .changed();
                });
                source_status(ui, k, &self.doc().text);
                // Typing redraws the canvas as soon as what you have typed is a
                // program.
                if edited {
                    self.adopt_text();
                }
            });
        if format_source {
            let typed = self.doc().text.clone();
            self.notice = Some(match rebis_lang::parse(&typed) {
                Ok(expr) => {
                    let formatted = rebis_lang::pretty_format(&expr);
                    let doc = self.doc_mut();
                    doc.text = formatted;
                    // Formatting only rewrites the *text*; the drawing is
                    // unchanged. `generated` must stay equal to what the drawing
                    // produces (`to_rebis`), so `sync` sees no change and leaves
                    // the formatted text alone instead of overwriting it back to
                    // the one-line form on the next frame.
                    doc.generated = doc.mandala.to_rebis().unwrap_or_default();
                    "formatted the source".to_string()
                }
                Err(error) => format!("format: {error}"),
            });
        }
        if format_drawing {
            let doc = self.doc_mut();
            doc.checkpoint();
            doc.mandala.relayout();
            self.notice = Some("redrew the mandala as a circuit".to_string());
        }
        if open_in_editor {
            self.open_generated_source();
        }
    }

    /// Full run supervisor. This is the visual projection of the terminal run
    /// browser: the same captured source/input, lane, authority, process,
    /// stream, timing, pause/retry, cancellation, and retained history.
    fn runs_tab(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut submit: Option<runs::Lane> = None;
        let mut select = None;
        let mut permission: Option<runs::Authority> = None;
        let mut pause = false;
        let mut cancel = false;
        let mut cancel_all = false;
        let mut remove = false;
        let mut deny_run: Option<u64> = None;
        // Typing into a waiting run's input box, and sending it. The run list is
        // walked immutably, so both are collected here and applied after it.
        let mut input_edit: Option<(u64, String)> = None;
        let mut deliver: Option<(u64, String)> = None;
        let mut copy = false;
        let mut write = false;
        let mut rerun = None;
        let mut run_chat = None;
        let mut auto_generation = None;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.accent, "RUNS");
            ui.colored_label(
                k.faint,
                format!(
                    "{} retained · {} active",
                    self.runs.runs.len(),
                    self.runs.active_count()
                ),
            );
            if self.runs.has_active() && ui.button("cancel all").clicked() {
                cancel_all = true;
            }
            if let Some(note) = &self.runs.notice {
                ui.colored_label(k.faint, note);
            }
        });
        // Export the selected run's stream to a file — the terminal's file
        // output, kept here so the visual runs desk stays 1:1 with it.
        if self.runs.selected.is_some() {
            ui.horizontal(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut self.runs.output_path)
                        .desired_width(300.0)
                        .hint_text("write selected stream to file"),
                );
                if ui.button("write stream").clicked() {
                    write = true;
                }
            });
        }
        ui.separator();

        egui::CollapsingHeader::new("NEW RUN")
            .default_open(self.runs.runs.is_empty())
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut self.runs.mode, runs::Mode::Dry, "dry / deterministic");
                    ui.radio_value(&mut self.runs.mode, runs::Mode::Direct, "live direct");
                    ui.radio_value(
                        &mut self.runs.mode,
                        runs::Mode::Chaos,
                        "live chaos orchestrator",
                    );
                    ui.separator();
                    ui.radio_value(&mut self.runs.scope, runs::Scope::Program, "program");
                    ui.radio_value(&mut self.runs.scope, runs::Scope::Block, "block");
                    ui.separator();
                    ui.radio_value(&mut self.runs.lane, runs::Lane::Serial, "serial lane");
                    ui.radio_value(&mut self.runs.lane, runs::Lane::Parallel, "parallel lane");
                });
                if self.runs.mode.live() {
                    ui.horizontal(|ui| {
                        ui.colored_label(k.faint, "AUTHORITY");
                        if self.runs.authority_remembered {
                            ui.colored_label(k.accent, "granted for this session");
                            if ui.small_button("forget").clicked() {
                                self.runs.authority_remembered = false;
                                self.runs.authority = runs::Authority::Ask;
                            }
                        } else {
                            ui.colored_label(
                                k.faint,
                                "each live run waits for allow once / allow session",
                            );
                        }
                    });
                }
                ui.columns(2, |columns| {
                    columns[0].colored_label(k.faint, "REBIS SOURCE");
                    let id = egui::Id::new("run_draft_source");
                    let pair = matched_pair(columns[0].ctx(), id, &self.runs.draft_source);
                    let mut layouter = |ui: &egui::Ui, text: &str, _wrap: f32| {
                        ui.fonts(|fonts| {
                            fonts.layout_job(rebis_layout_job(text, k, 12.5, &|_| false, pair))
                        })
                    };
                    columns[0].add(
                        egui::TextEdit::multiline(&mut self.runs.draft_source)
                            .id(id)
                            .code_editor()
                            .desired_width(f32::INFINITY)
                            .desired_rows(7)
                            .hint_text("\"prompt\"")
                            .layouter(&mut layouter),
                    );
                    source_status(&mut columns[0], k, &self.runs.draft_source);
                    columns[1].colored_label(k.faint, "RECORD / INPUT");
                    columns[1].add(
                        egui::TextEdit::multiline(&mut self.runs.input)
                            .desired_width(f32::INFINITY)
                            .desired_rows(7)
                            .hint_text("one line of evidence per line"),
                    );
                });
                let diagnostic = if self.runs.draft_source.trim().is_empty() {
                    "source is empty".to_string()
                } else {
                    match rebis_lang::parse(&self.runs.draft_source) {
                        Ok(_) => "valid Rebis".to_string(),
                        Err(error) => error.to_string(),
                    }
                };
                ui.horizontal(|ui| {
                    let valid = rebis_lang::parse(&self.runs.draft_source).is_ok();
                    ui.add_enabled_ui(valid, |ui| {
                        if ui.button("run").clicked() {
                            submit = Some(self.runs.lane);
                        }
                    });
                    ui.colored_label(if valid { k.faint } else { k.accent }, diagnostic);
                });
            });

        ui.separator();
        ui.colored_label(k.faint, "HISTORY");
        if self.runs.runs.is_empty() {
            ui.colored_label(k.faint, "No runs yet.");
        }
        // One full-width vertical list, mirroring the terminal: every run is a
        // row; clicking it selects and toggles its expansion; an expanded run
        // shows its controls and captured stream inline beneath its header.
        let mut toggle = None;
        let mut run_generation = None;
        egui::ScrollArea::vertical()
            .id_salt("run_list")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                for (position, run) in self.runs.runs.iter().enumerate() {
                    let chosen = self.runs.selected == Some(run.id);
                    let lane = if run.parallel() { "∥" } else { "│" };
                    let marker = if run.expanded { "▾" } else { "▸" };
                    let label = format!(
                        "{marker} {lane} #{:<3} {:<12} {} {:<7} {}",
                        run.id,
                        run.state.label(run.paused),
                        run.timer(),
                        run.scope.label(),
                        run.preview()
                    );
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(chosen, egui::RichText::new(label).monospace())
                            .clicked()
                        {
                            select = Some(run.id);
                            toggle = Some(run.id);
                        }
                        if ui
                            .small_button("chat")
                            .on_hover_text("ask about this run with a fresh live snapshot")
                            .clicked()
                        {
                            run_chat = Some(run.id);
                        }
                        if ui
                            .small_button("generation")
                            .on_hover_text(
                                "watch this run as the automaton it computes — its geometry is \
                                 the lattice, its answers are the rule",
                            )
                            .clicked()
                        {
                            run_generation = Some(run.id);
                        }
                        // Remove is always available in the list itself, on any
                        // run that is not currently running — the terminal's
                        // `u`/Delete, one click.
                        if run.state != runs::State::Running
                            && ui.small_button("✕").on_hover_text("remove run").clicked()
                        {
                            select = Some(run.id);
                            remove = true;
                        }
                    });
                    if run.state == runs::State::Queued {
                        let queue = self.runs.runs[..position]
                            .iter()
                            .filter(|prior| {
                                prior.state == runs::State::Queued && prior.lane == run.lane
                            })
                            .count()
                            + 1;
                        ui.colored_label(k.faint, format!("      queue position {queue}"));
                    }

                    if !run.expanded {
                        continue;
                    }
                    // ── expanded: controls + captured state, indented ──
                    let id = run.id;
                    let state = run.state;
                    let paused = run.paused;
                    egui::Frame::none()
                        .outer_margin(egui::Margin {
                            left: 18.0,
                            ..egui::Margin::symmetric(0.0, 2.0)
                        })
                        .show(ui, |ui| {
                            if let Some(reason) = &run.pause_reason {
                                ui.colored_label(k.accent, reason);
                            }
                            // A run stopped on an `&` port is waiting for a
                            // person, not for a process. This is where that
                            // person answers: the value goes to the port and the
                            // run continues from exactly where it stopped.
                            if let Some(port) = run.awaiting_port() {
                                ui.horizontal(|ui| {
                                    ui.colored_label(k.accent, format!("⌷ {port} ←"));
                                    let mut draft = run.input_draft.clone();
                                    let field = ui.add(
                                        egui::TextEdit::singleline(&mut draft)
                                            .id(egui::Id::new(("run_input", id)))
                                            .desired_width(300.0)
                                            .hint_text("value for this port · Enter sends"),
                                    );
                                    if draft != run.input_draft {
                                        input_edit = Some((id, draft.clone()));
                                    }
                                    let entered = field.lost_focus()
                                        && ui.input(|input| input.key_pressed(egui::Key::Enter));
                                    let sent = ui.button("send").clicked() || entered;
                                    // An empty value is not an answer: the port
                                    // would receive nothing and the run would
                                    // park again with no sign of why.
                                    if sent && !draft.trim().is_empty() {
                                        deliver = Some((id, draft));
                                    }
                                });
                            }
                            ui.horizontal_wrapped(|ui| {
                                if state == runs::State::AwaitingPermission {
                                    if ui.button("allow once").clicked() {
                                        select = Some(id);
                                        permission = Some(runs::Authority::Once);
                                    }
                                    if ui.button("allow session").clicked() {
                                        select = Some(id);
                                        permission = Some(runs::Authority::Session);
                                    }
                                    if ui.button("deny").clicked() {
                                        deny_run = Some(id);
                                    }
                                }
                                if state == runs::State::Running
                                    && ui.button(if paused { "resume" } else { "pause" }).clicked()
                                {
                                    select = Some(id);
                                    pause = true;
                                }
                                if !state.terminal() && ui.button("cancel").clicked() {
                                    select = Some(id);
                                    cancel = true;
                                }
                                if state.terminal() && ui.button("run again").clicked() {
                                    rerun = Some(run.source.clone());
                                }
                                if ui.button("copy stream").clicked() {
                                    select = Some(id);
                                    copy = true;
                                }
                            });
                            egui::CollapsingHeader::new(format!("source #{id}"))
                                .id_salt(("run_source", id))
                                .show(ui, |ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&run.source).monospace(),
                                        )
                                        .wrap(),
                                    );
                                });
                            if !run.input.is_empty() {
                                egui::CollapsingHeader::new(format!("input #{id}"))
                                    .id_salt(("run_input", id))
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&run.input).monospace(),
                                            )
                                            .wrap(),
                                        );
                                    });
                            }
                            if run.output.is_empty() {
                                ui.colored_label(
                                    k.faint,
                                    match state {
                                        runs::State::AwaitingPermission => {
                                            "(waiting for agent authority)"
                                        }
                                        runs::State::Queued => "(waiting in the serial queue)",
                                        runs::State::Running => "(waiting for stream output…)",
                                        _ => "(no stream output)",
                                    },
                                );
                            }
                            for line in &run.output {
                                let tone =
                                    if line.starts_with("result") || line.starts_with("complete") {
                                        k.ink
                                    } else if line.starts_with("diagnostic")
                                        || line.starts_with("paused")
                                    {
                                        k.accent
                                    } else {
                                        k.faint
                                    };
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(line).monospace().color(tone),
                                    )
                                    .wrap(),
                                );
                            }
                        });
                    ui.separator();
                }
            });
        if let Some(id) = toggle {
            if let Some(run) = self.runs.runs.iter_mut().find(|run| run.id == id) {
                run.expanded = !run.expanded;
            }
        }

        // Selection is set by any row or action click. Expansion is toggled
        // separately (above), so selecting to act on a run never forces it open.
        if let Some(id) = select {
            self.runs.selected = Some(id);
        }
        if let Some(parallel) = submit {
            let source = self.runs.draft_source.clone();
            auto_generation = Some(self.runs.submit(source, Some(parallel), &self.cwd));
        }
        // The draft lands before the send, so a value typed and sent in the same
        // frame is the value delivered — and `deliver_input` clears it after.
        if let Some((id, draft)) = input_edit {
            if let Some(run) = self.runs.runs.iter_mut().find(|run| run.id == id) {
                run.input_draft = draft;
            }
        }
        if let Some((id, value)) = deliver {
            self.runs.deliver_input(id, &value);
        }
        if let Some(authority) = permission {
            self.runs.grant_selected(authority, &self.cwd);
        }
        if let Some(id) = deny_run {
            self.runs.selected = Some(id);
            self.runs.deny_selected();
        }
        if pause {
            self.runs.toggle_pause_selected(&self.cwd);
        }
        if cancel {
            self.runs.cancel_selected(&self.cwd);
        }
        if cancel_all {
            self.runs.cancel_all(&self.cwd);
        }
        if remove {
            self.runs.remove_selected();
        }
        if let Some(source) = rerun {
            auto_generation = Some(self.runs.submit(source, None, &self.cwd));
        }
        if let Some(id) = run_generation {
            self.runs.selected = Some(id);
            self.open_generation();
        }
        if let Some(id) = run_chat {
            self.open_run_chat(id);
        }
        if let Some(id) = auto_generation {
            self.runs.selected = Some(id);
            self.open_generation();
        }
        if copy {
            ui.ctx().copy_text(self.runs.selected_output());
            self.runs.notice = Some("copied selected stream".to_string());
        }
        if write {
            self.runs.write_selected_output(&self.cwd);
        }
    }

    /// Native controls for the terminal application's remaining rites. The
    /// capability selector is typed; all jobs share one process supervisor and
    /// one retained history instead of each button owning bespoke thread code.
    fn actions_tab(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let mut surface = None;
        let mut submit = false;
        let mut attach = false;
        let mut remove_attachment = None;
        let mut select = None;
        let mut grant = None;
        let mut cancel = false;
        let mut remove = false;
        let mut copy = false;

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.accent, "ACTIONS");
            ui.colored_label(
                k.faint,
                format!(
                    "{} retained · {} active",
                    self.actions.tasks.len(),
                    self.actions.active_count()
                ),
            );
            if let Some(note) = &self.actions.notice {
                ui.colored_label(k.faint, note);
            }
        });
        ui.separator();
        ui.colored_label(k.faint, "NATIVE VISUAL SURFACES");
        ui.horizontal_wrapped(|ui| {
            for target in actions::Surface::ALL {
                if ui.button(target.label()).clicked() {
                    surface = Some(target);
                }
            }
        });

        egui::CollapsingHeader::new("NEW KAOS ACTION")
            .default_open(true)
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("rite");
                    egui::ComboBox::from_id_salt("action_kind")
                        .selected_text(self.actions.kind.label())
                        .show_ui(ui, |ui| {
                            for kind in actions::Kind::ALL {
                                if kind != actions::Kind::Chat {
                                    ui.selectable_value(
                                        &mut self.actions.kind,
                                        kind,
                                        kind.label(),
                                    );
                                }
                            }
                        });
                    ui.radio_value(
                        &mut self.actions.lane,
                        kaos_core::run_model::Lane::Serial,
                        "serial",
                    );
                    ui.radio_value(
                        &mut self.actions.lane,
                        kaos_core::run_model::Lane::Parallel,
                        "parallel",
                    );
                });

                match self.actions.kind {
                    actions::Kind::Code => {
                        ui.horizontal(|ui| {
                            ui.label("path");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.actions.path)
                                    .desired_width(220.0),
                            );
                            ui.label("adepts");
                            ui.add(
                                egui::DragValue::new(&mut self.actions.quorum).range(1..=64),
                            );
                            ui.label("verification gate");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.actions.gate)
                                    .desired_width(260.0)
                                    .hint_text("optional command"),
                            );
                        });
                    }
                    actions::Kind::AuthSet
                    | actions::Kind::AuthForget
                    | actions::Kind::AuthStatus => {
                        ui.horizontal(|ui| {
                            ui.label("provider");
                            egui::ComboBox::from_id_salt("credential_provider")
                                .selected_text(&self.actions.provider)
                                .show_ui(ui, |ui| {
                                    for provider in
                                        ["openrouter", "openai", "anthropic", "claude"]
                                    {
                                        ui.selectable_value(
                                            &mut self.actions.provider,
                                            provider.to_string(),
                                            provider,
                                        );
                                    }
                                });
                            if self.actions.kind == actions::Kind::AuthSet
                                && self.actions.provider != "claude"
                            {
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.actions.secret)
                                        .password(true)
                                        .desired_width(360.0)
                                        .hint_text("API key"),
                                );
                            }
                        });
                        ui.colored_label(
                            k.faint,
                            "Secrets use Kaos's credential store and are never copied into settings.",
                        );
                    }
                    _ => {}
                }
                if self.actions.kind.needs_intent() {
                    ui.add(
                        egui::TextEdit::multiline(&mut self.actions.intent)
                            .desired_width(f32::INFINITY)
                            .desired_rows(4)
                            .hint_text("intent"),
                    );
                }
                if self.actions.kind.may_use_tools() {
                    ui.horizontal(|ui| {
                        ui.colored_label(k.faint, "TOOL AUTHORITY");
                        ui.radio_value(
                            &mut self.actions.tools,
                            actions::ToolAccess::Ask,
                            "ask",
                        );
                        ui.radio_value(
                            &mut self.actions.tools,
                            actions::ToolAccess::EditsOnly,
                            "edits only",
                        );
                        ui.radio_value(
                            &mut self.actions.tools,
                            actions::ToolAccess::Shell,
                            "edits + shell",
                        );
                    });
                }
                if ui.button(format!("run {}", self.actions.kind.label())).clicked() {
                    submit = true;
                }
            });

        egui::CollapsingHeader::new(format!("ATTACHMENTS · {}", self.actions.attachments.len()))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.actions.attachment_path)
                            .desired_width(420.0)
                            .hint_text("file path"),
                    );
                    if ui.button("attach").clicked() {
                        attach = true;
                    }
                    if !self.actions.attachments.is_empty() && ui.button("clear all").clicked() {
                        self.actions.attachments.clear();
                    }
                });
                for (index, attachment) in self.actions.attachments.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.monospace(attachment.path.display().to_string());
                        ui.colored_label(k.faint, format!("{} bytes", attachment.bytes));
                        if ui.small_button("remove").clicked() {
                            remove_attachment = Some(index);
                        }
                    });
                }
            });

        ui.separator();
        let available = ui.available_size();
        ui.horizontal_top(|ui| {
            ui.allocate_ui(
                Vec2::new((available.x * 0.34).max(280.0), available.y),
                |ui| {
                    ui.colored_label(k.faint, "TASK HISTORY");
                    egui::ScrollArea::vertical()
                        .id_salt("task_list")
                        .show(ui, |ui| {
                            for task in &self.actions.tasks {
                                let chosen = self.actions.selected == Some(task.id);
                                let lane = if task.lane == kaos_core::run_model::Lane::Parallel {
                                    "∥"
                                } else {
                                    "│"
                                };
                                let label = format!(
                                    "{lane} #{:<3} {:<10} {} {}",
                                    task.id,
                                    task.state.label(false),
                                    task.timer(),
                                    task.label
                                );
                                if ui.selectable_label(chosen, label).clicked() {
                                    select = Some(task.id);
                                }
                            }
                        });
                },
            );
            ui.separator();
            ui.allocate_ui(Vec2::new(ui.available_width(), available.y), |ui| {
                let Some(task) = self.actions.selected_task() else {
                    ui.colored_label(k.faint, "Select a task to inspect its stream.");
                    return;
                };
                let state = task.state;
                let output = task.output.clone();
                ui.colored_label(
                    k.secondary,
                    format!(
                        "#{} · {} · {} · {}",
                        task.id,
                        state.label(false),
                        task.timer(),
                        task.kind.label()
                    ),
                );
                ui.horizontal(|ui| {
                    if state == kaos_core::run_model::State::AwaitingPermission {
                        if ui.button("edits only").clicked() {
                            grant = Some(actions::ToolAccess::EditsOnly);
                        }
                        if ui.button("allow shell").clicked() {
                            grant = Some(actions::ToolAccess::Shell);
                        }
                    }
                    if !state.terminal() && ui.button("cancel").clicked() {
                        cancel = true;
                    }
                    if state != kaos_core::run_model::State::Running
                        && ui.button("remove").clicked()
                    {
                        remove = true;
                    }
                    if ui.button("copy stream").clicked() {
                        copy = true;
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical()
                    .id_salt("task_stream")
                    .stick_to_bottom(state == kaos_core::run_model::State::Running)
                    .show(ui, |ui| {
                        if output.is_empty() {
                            ui.colored_label(k.faint, "(waiting for output)");
                        }
                        for line in output {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(line).monospace().color(k.faint),
                                )
                                .wrap(),
                            );
                        }
                    });
            });
        });

        if let Some(target) = surface {
            self.open_surface(target);
        }
        if submit {
            if matches!(
                self.actions.kind,
                actions::Kind::AuthStatus | actions::Kind::AuthSet | actions::Kind::AuthForget
            ) {
                self.actions.submit_auth(self.actions.kind);
            } else {
                self.actions.submit_current(&self.cwd);
            }
        }
        if attach {
            self.actions.add_attachment(&self.cwd);
        }
        if let Some(index) = remove_attachment {
            self.actions.attachments.remove(index);
        }
        if let Some(id) = select {
            self.actions.selected = Some(id);
        }
        if let Some(access) = grant {
            self.actions.grant_selected(access, &self.cwd);
        }
        if cancel {
            self.actions.cancel_selected(&self.cwd);
        }
        if remove {
            self.actions.remove_selected();
        }
        if copy {
            if let Some(task) = self.actions.selected_task() {
                ui.ctx().copy_text(task.output.join("\n"));
                self.actions.notice = Some("copied selected task stream".to_string());
            }
        }
    }

    fn open_surface(&mut self, surface: actions::Surface) {
        match surface {
            actions::Surface::Mandala => {
                let existing = self
                    .tabs
                    .iter()
                    .find_map(|tab| matches!(tab.content, Pane::Mandala(_)).then_some(tab.id));
                if let Some(id) = existing {
                    self.tabs.select(id);
                } else {
                    self.tabs.open("mandala", Pane::Mandala(Doc::default()));
                }
            }
            actions::Surface::Chat => {
                self.tabs.open("chat", Pane::Chat(ChatPane::default()));
            }
            actions::Surface::Source => {
                self.tabs
                    .open("source", Pane::Source(SourcePane::default()));
            }
            actions::Surface::Runs => self.open_runs(),
            actions::Surface::Sigils => {
                self.tabs.open("sigils", Pane::Sigils(SigilPane::default()));
            }
            actions::Surface::Settings => self.open_settings(),
        }
    }

    fn footer(&mut self, ctx: &egui::Context) {
        egui::TopBottomPanel::bottom("footer").show(ctx, |ui| {
            ui.horizontal(|ui| {
                let hint = if self.on_mandala()
                    && self.doc().canvas_mode == CanvasMode::Spatial
                {
                    "drag to orbit · arrows move · wheel to zoom · click selects · edit text in the side panel"
                } else {
                    "drag/pan · right-drag marquee · Ctrl-click toggles · Ctrl-C/V block · Delete · Ctrl-Z"
                };
                ui.colored_label(
                    self.ink.faint,
                    hint,
                );
                if self.on_mandala() && self.doc().canvas_mode == CanvasMode::Planar {
                    if let Some(id) = self.doc().pending {
                        let message = match &self.tool {
                            Tool::Flow(_) => {
                                format!("flow from #{} — click the destination · shift for an angled line", id.0)
                            }
                            Tool::Father => format!(
                                "father #{} — click its next child · shift for an angled line",
                                id.0
                            ),
                            Tool::Place(_) | Tool::Select => String::new(),
                        };
                        ui.colored_label(self.ink.ink, message);
                    }
                    if matches!(self.tool, Tool::Place(Form::Forward | Form::Backflow)) {
                        ui.colored_label(
                            self.ink.ink,
                            "standalone arrow: place it, then link one child at a time",
                        );
                    }
                }
                ui.separator();
                // The working context, shown for the same reason the terminal
                // app shows it: it decides what relative reads, imports and
                // output paths mean.
                ui.colored_label(self.ink.faint, self.cwd.display().to_string());
            });
        });
    }

    // ── canvas ─────────────────────────────────────────────────────────────

    /// Selection shortcuts are ignored while a text field has focus, so
    /// editing a label is never mistaken for a graph command.
    fn handle_keys(&mut self, ctx: &egui::Context) {
        if ctx.memory(|m| m.focused()).is_some() {
            return;
        }
        // Tab cycling and closing go through `Tabs`, so the terminal app can
        // bind the same behaviour to its own keys without reimplementing it.
        let (next, prev, close, open, undo, redo, copy, paste, pasted_text, delete) =
            ctx.input(|i| {
                (
                    i.modifiers.ctrl && i.key_pressed(egui::Key::Tab),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::ArrowLeft),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::W),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::T),
                    i.modifiers.ctrl && !i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    i.modifiers.ctrl && i.modifiers.shift && i.key_pressed(egui::Key::Z),
                    (i.modifiers.ctrl && i.key_pressed(egui::Key::C))
                        || i.events
                            .iter()
                            .any(|event| matches!(event, egui::Event::Copy)),
                    i.modifiers.ctrl && i.key_pressed(egui::Key::V),
                    i.events.iter().find_map(|event| match event {
                        egui::Event::Paste(text) => Some(text.clone()),
                        _ => None,
                    }),
                    i.key_pressed(egui::Key::Delete),
                )
            });
        if next {
            self.tabs.next();
        }
        if prev {
            self.tabs.prev();
        }
        if open {
            let n = self.tabs.len() + 1;
            self.tabs
                .open(format!("mandala {n}"), Pane::Mandala(Doc::default()));
        }
        if close && self.tabs.len() > 1 {
            if let Some(id) = self.tabs.active_id() {
                self.tabs.close(id);
            }
        }
        let editable = self.on_mandala() && self.doc().canvas_mode == CanvasMode::Planar;
        if copy && self.on_mandala() {
            self.copy_selected(ctx);
        }
        if (paste || pasted_text.is_some()) && editable {
            self.paste_selected(pasted_text.as_deref());
        }
        if undo && editable {
            if let Some(Pane::Mandala(doc)) = self.tabs.active_mut() {
                doc.undo();
            }
        }
        if redo && editable {
            if let Some(Pane::Mandala(doc)) = self.tabs.active_mut() {
                doc.redo();
            }
        }
        if delete && editable {
            let count = self.doc().selection_len();
            if self.doc_mut().delete_selected() {
                self.notice = Some(format!("deleted {count} form{}", plural(count)));
            }
        }
    }

    fn canvas(&mut self, ui: &mut egui::Ui) {
        match self.doc().canvas_mode {
            CanvasMode::Planar => self.canvas_2d(ui),
            CanvasMode::Spatial => self.canvas_3d(ui),
        }
    }

    fn canvas_2d(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        let origin = response.rect.min;
        // When the right-click menu is open, the click that dismisses it must
        // only close the menu — never also select, place, or move on the
        // canvas. This is true coming into the frame; egui closes the menu as
        // it processes the click, so we suppress canvas actions for it.
        let menu_open = ui.ctx().is_context_menu_open();
        if response.clicked() || response.drag_started() {
            // Canvas interaction takes editing intent away from any previous
            // text widget. Leaving that stale focus alive would make the
            // global Delete guard mistake a selected shape for typed text.
            ui.memory_mut(|memory| {
                if let Some(id) = memory.focused() {
                    memory.surrender_focus(id);
                }
            });
        }
        // Canvas-local screen coordinates, which is the space `View` works in.
        let local = |p: Pos2| (f64::from(p.x - origin.x), f64::from(p.y - origin.y));

        // Zoom about the pointer, so the canvas grows where you are looking.
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                if let Some(p) = response.hover_pos() {
                    let (sx, sy) = local(p);
                    let factor = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
                    self.doc_mut().view.zoom_at(sx, sy, factor);
                }
            }
        }

        // A wall is a narrow target, so the cursor names it before the drag
        // begins — otherwise resizing is invisible until stumbled on.
        if response.hovered() && matches!(self.drag, Drag::None) {
            if let Some(p) = response.hover_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc().view.to_world(sx, sy);
                if let Some(grab) = self.doc().mandala.resize_grab(wx, wy) {
                    let centre = self
                        .doc()
                        .mandala
                        .node(grab.id)
                        .map(|node| (node.x, node.y))
                        .unwrap_or((wx, wy));
                    ui.ctx().set_cursor_icon(match (grab.wide, grab.tall) {
                        // On a corner the cursor names the diagonal it lies on.
                        (true, true) if (wx - centre.0 > 0.0) == (wy - centre.1 > 0.0) => {
                            egui::CursorIcon::ResizeNwSe
                        }
                        (true, true) => egui::CursorIcon::ResizeNeSw,
                        (true, false) => egui::CursorIcon::ResizeHorizontal,
                        _ => egui::CursorIcon::ResizeVertical,
                    });
                }
            }
        }

        // Secondary drag is always a marquee, independent of the active
        // drawing tool. Primary drag retains node movement and canvas panning.
        if !menu_open && response.drag_started_by(PointerButton::Secondary) {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let world = self.doc().view.to_world(sx, sy);
                let additive = ui.input(|input| input.modifiers.ctrl || input.modifiers.command);
                self.drag = Drag::Marquee {
                    start: world,
                    current: world,
                    additive,
                };
            }
        } else if !menu_open && response.drag_started_by(PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                let drag = Drag::beginning_at(&self.doc().mandala, wx, wy);
                // Moving a shape and sizing a box both change the drawing;
                // panning only changes where it is looked at.
                if !matches!(drag, Drag::Pan) {
                    self.doc_mut().checkpoint();
                }
                self.drag = drag;
            }
        }
        if response.dragged_by(PointerButton::Primary) {
            match self.drag {
                Drag::Resize(grab) => {
                    if let Some(p) = response.interact_pointer_pos() {
                        let (sx, sy) = local(p);
                        let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                        let (mut half_w, mut half_h) = self.doc().mandala.extent(grab.id);
                        if let Some(node) = self.doc().mandala.node(grab.id) {
                            if grab.wide {
                                half_w = (wx - node.x).abs();
                            }
                            if grab.tall {
                                half_h = (wy - node.y).abs();
                            }
                        }
                        self.doc_mut().mandala.resize(grab.id, half_w, half_h);
                    }
                }
                Drag::Node { id, grab } => {
                    if let Some(p) = response.interact_pointer_pos() {
                        let (sx, sy) = local(p);
                        let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                        // A box and what it holds read as one object, so
                        // dragging the box carries its interior along.
                        self.doc_mut()
                            .mandala
                            .move_group_to(id, wx - grab.0, wy - grab.1);
                    }
                }
                Drag::Pan => {
                    let d = response.drag_delta();
                    self.doc_mut().view.pan(f64::from(d.x), f64::from(d.y));
                }
                Drag::None | Drag::Marquee { .. } => {}
            }
        }
        if response.dragged_by(PointerButton::Secondary) {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let world = self.doc().view.to_world(sx, sy);
                if let Drag::Marquee { current, .. } = &mut self.drag {
                    *current = world;
                }
            }
        }
        if response.drag_stopped_by(PointerButton::Secondary) {
            if let Drag::Marquee {
                start,
                current,
                additive,
            } = self.drag
            {
                let ids = self
                    .doc()
                    .mandala
                    .nodes_in_rect(WorldRect::from_points(start, current));
                self.doc_mut().select_many(ids, additive);
            }
            self.drag = Drag::None;
        } else if response.drag_stopped_by(PointerButton::Primary) {
            self.drag = Drag::None;
        }

        if !menu_open && response.clicked_by(PointerButton::Primary) {
            if let Some(p) = response.interact_pointer_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                let additive = ui.input(|input| input.modifiers.ctrl || input.modifiers.command);
                let angle = ui.input(|input| input.modifiers.shift);
                self.click(wx, wy, additive, angle);
            }
        }

        // Right-click opens a context menu. A right *drag* is still the
        // marquee; egui only opens the menu on a click that did not drag.
        let mut reset_view = false;
        let mut copy = false;
        let mut paste = false;
        let has_selection = self.doc().selection_len() > 0;
        // Paste is offered whenever something is on either clipboard — the
        // in-app block or Rebis source on the OS clipboard.
        let can_paste = self.clipboard.is_some();
        response.context_menu(|ui| {
            if ui
                .add_enabled(has_selection, egui::Button::new("copy block"))
                .clicked()
            {
                copy = true;
                ui.close_menu();
            }
            if ui
                .add_enabled(can_paste, egui::Button::new("paste"))
                .clicked()
            {
                paste = true;
                ui.close_menu();
            }
            ui.separator();
            if ui.button("reset view").clicked() {
                reset_view = true;
                ui.close_menu();
            }
        });
        if copy {
            self.copy_selected(ui.ctx());
        }
        if paste {
            // Prefer OS-clipboard source so a block copied in another window
            // pastes here; fall back to the in-app clipboard.
            let os_text = ui.ctx().input(|i| {
                i.events.iter().find_map(|event| match event {
                    egui::Event::Paste(text) => Some(text.clone()),
                    _ => None,
                })
            });
            self.paste_selected(os_text.as_deref());
        }
        if reset_view {
            self.doc_mut().view = View::new();
        }

        // A turning ring needs a clock and a reason to redraw. egui only
        // repaints on input, so ask for frames while something is running.
        if !self.doc().running.is_empty() {
            ui.ctx().request_repaint();
        }
        self.paint_2d(&painter, origin, ui.input(|i| i.time) as f32);
    }

    /// Orbitable structural projection of the same mandala.
    ///
    /// This surface deliberately owns no graph mutations. It derives a fresh
    /// layout every frame from the 2D source of truth, so switching modes,
    /// orbiting, and zooming cannot alter generated Rebis or its undo history.
    fn canvas_3d(&mut self, ui: &mut egui::Ui) {
        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        // A click that dismisses the right-click menu must only close it, never
        // also orbit or select. True coming into the frame; egui closes the
        // menu as it handles the click.
        let menu_open = ui.ctx().is_context_menu_open();

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let factor = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
                self.doc_mut().camera.zoom = (self.doc().camera.zoom * factor).clamp(0.25, 4.0);
            }
        }
        if !menu_open && response.dragged() {
            let delta = ui.input(|input| input.pointer.delta());
            let camera = &mut self.doc_mut().camera;
            camera.yaw += delta.x * 0.008;
            camera.pitch = (camera.pitch - delta.y * 0.008).clamp(-1.35, 1.35);
            ui.ctx().request_repaint();
        }

        // Arrow keys move the viewpoint through the space: up/down travel
        // forward and backward along the camera's look direction (into and out
        // of the scene), left/right strafe sideways. Held keys repaint for
        // smooth motion.
        let (strafe, forward_amount, held) = ui.input(|input| {
            use egui::Key;
            let dt = input.stable_dt.clamp(0.0, 0.1);
            let speed = 340.0 * dt;
            let mut strafe = 0.0f32;
            let mut forward_amount = 0.0f32;
            if input.key_down(Key::ArrowLeft) {
                strafe -= speed;
            }
            if input.key_down(Key::ArrowRight) {
                strafe += speed;
            }
            if input.key_down(Key::ArrowUp) {
                forward_amount += speed;
            }
            if input.key_down(Key::ArrowDown) {
                forward_amount -= speed;
            }
            (
                strafe,
                forward_amount,
                strafe != 0.0 || forward_amount != 0.0,
            )
        });
        if held {
            let camera = &mut self.doc_mut().camera;
            let (cy, sy) = (camera.yaw.cos(), camera.yaw.sin());
            let (cp, sp) = (camera.pitch.cos(), camera.pitch.sin());
            // Forward is the camera's look direction; right is horizontal.
            let forward = [-cp * sy, sp, cp * cy];
            let right = [cy, 0.0, sy];
            for axis in 0..3 {
                camera.pan[axis] += forward[axis] * forward_amount + right[axis] * strafe;
            }
            ui.ctx().request_repaint();
        }

        let layout = self.doc().mandala.spatial_layout();
        let projected = project_spatial(&layout, response.rect, self.doc().camera);
        if !menu_open && response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                // Look up a projected node's on-screen centre by id.
                let pos_of = |id: NodeId| {
                    projected
                        .iter()
                        .find(|n| n.id == id)
                        .map(|n| (n.position, n.scale))
                };
                let selected = projected
                    .iter()
                    .filter_map(|node| {
                        let form = self.doc().mandala.node(node.id)?;
                        if form.shape() == Shape::Arrow {
                            // A complete flow is drawn as the arrow *between its
                            // two children*, not at its own cone position, so
                            // that midpoint is where it is clickable.
                            let (point, scale) = self
                                .doc()
                                .mandala
                                .flow_result(node.id)
                                .and(match self.doc().mandala.children(node.id)[..] {
                                    [a, b] => Some((a, b)),
                                    _ => None,
                                })
                                .and_then(|(a, b)| Some((pos_of(a)?, pos_of(b)?)))
                                .map(|((pa, sa), (pb, sb))| (pa + (pb - pa) * 0.5, (sa + sb) * 0.5))
                                .unwrap_or((node.position, node.scale));
                            let distance = point.distance(pointer);
                            return (distance <= 20.0 * scale.max(0.6))
                                .then_some((node.id, distance));
                        }
                        let scale = node.scale.max(0.6);
                        let offset = (pointer - node.position) / scale;
                        form.shape()
                            .contains(f64::from(offset.x), f64::from(offset.y))
                            .then_some((node.id, node.position.distance(pointer)))
                    })
                    .min_by(|left, right| left.1.total_cmp(&right.1))
                    .map(|(id, _)| id);
                if let Some(id) = selected {
                    self.doc_mut().select_only(id);
                } else {
                    self.doc_mut().clear_selection();
                }
            }
        }

        // Right-click opens a context menu: reset the camera to its default
        // orbit and framing.
        let mut reset_view = false;
        response.context_menu(|ui| {
            if ui.button("reset view").clicked() {
                reset_view = true;
                ui.close_menu();
            }
        });
        if reset_view {
            self.doc_mut().camera = SpatialCamera::default();
            ui.ctx().request_repaint();
        }

        if !self.doc().running.is_empty() {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(33));
        }
        self.paint_3d(
            &painter,
            &layout,
            &projected,
            ui.input(|input| input.time) as f32,
        );
    }

    fn click(&mut self, wx: f64, wy: f64, additive: bool, angle: bool) {
        let hit = self.doc().mandala.hit(wx, wy);
        if additive {
            if let Some(id) = hit {
                self.doc_mut().toggle_selection(id);
            }
            return;
        }
        match (hit, self.tool.clone()) {
            // Clicked a shape.
            (Some(id), Tool::Flow(form)) => match self.doc_mut().pending {
                None => self.doc_mut().pending = Some(id),
                Some(from) => {
                    if from != id {
                        self.doc_mut().checkpoint();
                    }
                    if let Some(made) = self.doc_mut().mandala.flow(from, id, form) {
                        // Shift completes the connection as an angled straight
                        // line; otherwise it keeps the default 90° routing.
                        if angle {
                            self.doc_mut().angled.insert(made);
                        }
                        self.doc_mut().select_only(made);
                    }
                    self.doc_mut().pending = None;
                }
            },
            (Some(id), Tool::Father) => match self.doc_mut().pending {
                None => self.doc_mut().pending = Some(id),
                Some(father) => {
                    if father != id {
                        self.doc_mut().checkpoint();
                    }
                    self.doc_mut().mandala.father_of(father, id);
                    // The father-of link is keyed by its child node.
                    if angle {
                        self.doc_mut().angled.insert(id);
                    }
                    self.doc_mut().pending = None;
                }
            },
            (Some(id), _) => self.doc_mut().select_only(id),
            // Clicked empty canvas.
            (None, Tool::Place(form)) => {
                let text = default_text(&form);
                self.doc_mut().checkpoint();
                let id = self.doc_mut().mandala.add(form, text, wx, wy);
                self.doc_mut().select_only(id);
            }
            (None, Tool::Flow(_) | Tool::Father) => self.doc_mut().pending = None,
            (None, Tool::Select) => self.doc_mut().clear_selection(),
        }
    }

    /// A rotating dashed ring around a node that is being evaluated.
    ///
    /// Dashes are drawn as short arcs stepped around the circle and offset by
    /// the clock, so the ring turns. It reads as motion without animating the
    /// node itself, which must stay legible while it runs.
    fn running_ring(&self, painter: &egui::Painter, centre: Pos2, zoom: f32, spin: f32) {
        const DASHES: usize = 12;
        const SEGMENTS: usize = 4;
        let r = (NODE_R as f32 + 9.0) * zoom;
        let stroke = UiStroke::new(2.2 * zoom, self.ink.accent);
        let arc = std::f32::consts::TAU / DASHES as f32;
        for dash in 0..DASHES {
            if dash % 2 == 1 {
                continue; // the gaps
            }
            let start = spin + dash as f32 * arc;
            let mut previous = None;
            for step in 0..=SEGMENTS {
                let a = start + arc * step as f32 / SEGMENTS as f32;
                let p = Pos2::new(centre.x + r * a.cos(), centre.y + r * a.sin());
                if let Some(q) = previous {
                    painter.line_segment([q, p], stroke);
                }
                previous = Some(p);
            }
        }
    }

    /// A quiet eight-rayed chaos star in the canvas chrome. It is fixed to the
    /// viewport, painted beneath the graph, and deliberately faint enough not
    /// to look like a selectable Rebis form.
    fn chaos_star(&self, painter: &egui::Painter) {
        let rect = painter.clip_rect();
        if rect.width() < 120.0 || rect.height() < 120.0 {
            return;
        }
        let centre = Pos2::new(rect.right() - 46.0, rect.bottom() - 46.0);
        let radius = 21.0;
        let head = 5.5;
        let accent = self.ink.accent;
        let wash = Color32::from_rgba_unmultiplied(accent.r(), accent.g(), accent.b(), 38);
        let stroke = UiStroke::new(1.15, wash);
        for ray in 0..8 {
            let angle = std::f32::consts::TAU * ray as f32 / 8.0;
            let tip = Pos2::new(
                centre.x + radius * angle.cos(),
                centre.y + radius * angle.sin(),
            );
            painter.line_segment([centre, tip], stroke);
            for side in [-0.48f32, 0.48] {
                let back = angle + std::f32::consts::PI + side;
                let barb = Pos2::new(tip.x + head * back.cos(), tip.y + head * back.sin());
                painter.line_segment([tip, barb], stroke);
            }
        }
        painter.circle_filled(centre, 1.7, wash);
    }

    fn arrow_head(
        &self,
        painter: &egui::Painter,
        tip: Pos2,
        direction: Vec2,
        size: f32,
        stroke: UiStroke,
    ) {
        let length = direction.length().max(0.001);
        let direction = direction / length;
        for side in [-0.48f32, 0.48] {
            let (cos, sin) = (side.cos(), side.sin());
            let swept = Vec2::new(
                direction.x * cos - direction.y * sin,
                direction.x * sin + direction.y * cos,
            );
            painter.line_segment([tip, tip - swept * size], stroke);
        }
    }

    /// Paint one Rebis glyph. Both projections call this, keeping the visual
    /// alphabet—including nested circle/`[]` marks—identical in 2D and 3D.
    fn paint_node_body(&self, painter: &egui::Painter, node: &Node, paint: GlyphPaint) {
        let k = self.ink;
        let GlyphPaint {
            position: centre,
            scale: zoom,
            outline,
            hot,
        } = paint;
        let shape = node.shape();
        match shape {
            Shape::Circle => {
                painter.circle(centre, NODE_R as f32 * zoom, k.fill, outline);
            }
            Shape::Square => {
                // `[m]` writes its mediator inside the brackets, so the box
                // grows to hold it rather than pushing it out as a child.
                let (half_w, half_h) = self.doc().mandala.extent(node.id);
                painter.rect(
                    Rect::from_center_size(
                        centre,
                        Vec2::new(half_w as f32 * 2.0, half_h as f32 * 2.0) * zoom,
                    ),
                    4.0 * zoom,
                    k.fill,
                    outline,
                );
            }
            Shape::Oval => {
                let radius = Vec2::new(NODE_R as f32, NODE_RY as f32) * zoom;
                painter.add(egui::Shape::ellipse_filled(centre, radius, k.fill));
                painter.add(egui::Shape::ellipse_stroke(centre, radius, outline));
            }
            Shape::Diamond => {
                let points = Shape::diamond_points()
                    .iter()
                    .map(|(x, y)| Pos2::new(centre.x + x * zoom, centre.y + y * zoom))
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Parallelogram => {
                let points = Shape::parallelogram_points()
                    .iter()
                    .map(|(x, y)| Pos2::new(centre.x + x * zoom, centre.y + y * zoom))
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Hexagon => {
                let points = Shape::hexagon_points()
                    .map(|(x, y)| centre + Vec2::new(x * zoom, y * zoom))
                    .to_vec();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Amp => {
                let points = Shape::inlet_points()
                    .iter()
                    .map(|(x, y)| Pos2::new(centre.x + x * zoom, centre.y + y * zoom))
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Arrow => {
                let radius = 18.0 * zoom;
                painter.circle(centre, radius, k.fill, outline);
                painter.text(
                    centre,
                    Align2::CENTER_CENTER,
                    if node.form == Form::Backflow {
                        "←"
                    } else {
                        "→"
                    },
                    FontId::monospace(18.0 * zoom),
                    k.secondary,
                );
            }
            // A sigil is the shape; the disc behind it is the click target
            // (see Shape::contains), drawn faintly so it reads as one mark.
            _ => {
                painter.circle(
                    centre,
                    NODE_R as f32 * zoom,
                    k.fill,
                    UiStroke::new(1.0 * zoom, if hot { k.accent } else { k.chrome }),
                );
                let pen = UiStroke::new(5.0 * zoom, if hot { k.accent } else { k.ink });
                for stroke in shape.strokes() {
                    match stroke {
                        Stroke::Poly(points) => {
                            let points = points
                                .iter()
                                .map(|(x, y)| Pos2::new(centre.x + x * zoom, centre.y + y * zoom))
                                .collect();
                            painter.add(egui::Shape::line(points, pen));
                        }
                        Stroke::Cubic(points) => {
                            let points = [
                                Pos2::new(
                                    centre.x + points[0].0 * zoom,
                                    centre.y + points[0].1 * zoom,
                                ),
                                Pos2::new(
                                    centre.x + points[1].0 * zoom,
                                    centre.y + points[1].1 * zoom,
                                ),
                                Pos2::new(
                                    centre.x + points[2].0 * zoom,
                                    centre.y + points[2].1 * zoom,
                                ),
                                Pos2::new(
                                    centre.x + points[3].0 * zoom,
                                    centre.y + points[3].1 * zoom,
                                ),
                            ];
                            painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                                points,
                                false,
                                Color32::TRANSPARENT,
                                pen,
                            ));
                        }
                    }
                }
            }
        }
    }

    fn paint_graph_node(&self, painter: &egui::Painter, node: &Node, paint: NodePaint) {
        let NodePaint {
            position: centre,
            scale: zoom,
            spin,
            arrow_body,
            recursive,
        } = paint;
        let shape = node.shape();
        if shape == Shape::Arrow && !arrow_body {
            return;
        }
        let k = self.ink;
        let hot = self.doc().is_selected(node.id) || self.doc().pending == Some(node.id);
        let accented = hot || recursive || shape == Shape::Arrow;
        // An arrow is red whatever else is true of it. Selecting a block must
        // not repaint the one form whose colour carries its meaning; the
        // thicker outline below, and the accent ring for recursion, already
        // say everything selection needs to say.
        let outline_color = if shape == Shape::Arrow {
            k.secondary
        } else if hot || recursive {
            k.accent
        } else {
            k.faint
        };
        let outline = UiStroke::new(node_outline_width(shape, accented) * zoom, outline_color);
        if recursive {
            painter.circle_stroke(
                centre,
                (NODE_R as f32 + 8.0) * zoom,
                UiStroke::new(1.0 * zoom, k.accent),
            );
        }
        self.paint_node_body(
            painter,
            node,
            GlyphPaint {
                position: centre,
                scale: zoom,
                outline,
                hot,
            },
        );

        if self.doc().running.contains(&node.id) {
            self.running_ring(painter, centre, zoom, spin);
        }
        if shape == Shape::Arrow {
            return;
        }

        let caption = node.caption();
        if caption.is_empty() {
            return;
        }
        let caption = truncate(&caption);
        let font = FontId::monospace(11.0 * zoom);
        match shape {
            Shape::Circle
            | Shape::Square
            | Shape::Oval
            | Shape::Diamond
            | Shape::Parallelogram
            | Shape::Amp => {
                painter.text(centre, Align2::CENTER_CENTER, caption, font, k.ink);
            }
            _ => {
                painter.text(
                    Pos2::new(centre.x, centre.y + (NODE_R as f32 + 12.0) * zoom),
                    Align2::CENTER_CENTER,
                    caption,
                    font,
                    k.faint,
                );
            }
        }
    }

    fn paint_3d(
        &self,
        painter: &egui::Painter,
        layout: &SpatialLayout,
        projected: &[ProjectedNode],
        time: f32,
    ) {
        let k = self.ink;
        let spin = time * 1.6;
        self.chaos_star(painter);
        painter.text(
            painter.clip_rect().left_top() + Vec2::new(14.0, 14.0),
            Align2::LEFT_TOP,
            "STRUCTURAL 3D  ·  Z = nesting  ·  lifted arcs = recursion",
            FontId::monospace(10.0),
            k.faint,
        );
        if projected.is_empty() {
            painter.text(
                painter.clip_rect().center(),
                Align2::CENTER_CENTER,
                "build the mandala in 2D, then orbit its structural form here",
                FontId::monospace(13.0),
                k.faint,
            );
            return;
        }

        let find = |id: NodeId| projected.iter().find(|node| node.id == id);
        for edge in visible_edges(&self.doc().mandala) {
            let (Some(from), Some(to)) = (find(edge.from), find(edge.to)) else {
                continue;
            };
            let recursive = edge
                .source
                .is_some_and(|source| layout.recursive_edges.contains(&source));
            let delta = to.position - from.position;
            let length = delta.length();
            let direction = if length > 0.001 {
                delta / length
            } else {
                Vec2::RIGHT
            };
            let start = from.position + direction * NODE_R as f32 * from.scale.min(1.0);
            let end = to.position - direction * (NODE_R as f32 + 4.0) * to.scale.min(1.0);
            let touches = match edge.kind {
                VisibleEdgeKind::Father => {
                    self.doc().is_selected(edge.from) && self.doc().is_selected(edge.owner)
                }
                VisibleEdgeKind::Flow => {
                    let hot = self.doc().is_selected(edge.owner)
                        || self.doc().pending == Some(edge.owner);
                    hot || (self.doc().is_selected(edge.from) && self.doc().is_selected(edge.to))
                }
            };
            let link_color = edge_colour(edge.kind, touches, k);
            if recursive {
                let stroke = UiStroke::new(if touches { 2.8 } else { 2.2 }, link_color);
                let lift = (start.distance(end) * 0.42).clamp(48.0, 150.0);
                let (start, control_a, control_b, end) = if length < 2.0 {
                    let radius = 28.0 * from.scale;
                    (
                        from.position - Vec2::new(radius * 0.5, 0.0),
                        from.position + Vec2::new(-radius, -radius * 2.5),
                        from.position + Vec2::new(radius, -radius * 2.5),
                        from.position + Vec2::new(radius * 0.5, 0.0),
                    )
                } else {
                    (
                        start,
                        start - Vec2::new(0.0, lift),
                        end - Vec2::new(0.0, lift),
                        end,
                    )
                };
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [start, control_a, control_b, end],
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ));
                self.arrow_head(painter, end, end - control_b, 10.0, stroke);
            } else {
                let stroke = UiStroke::new(if touches { 1.9 } else { 1.35 }, link_color);
                painter.line_segment([start, end], stroke);
                self.arrow_head(painter, end, end - start, 8.0, stroke);
            }
        }

        let mut nodes = projected.to_vec();
        // Far forms first lets nearer forms cover their edges and produces a
        // stable depth cue without changing the underlying graph order.
        nodes.sort_by(|left, right| right.camera_depth.total_cmp(&left.camera_depth));
        for projected_node in nodes {
            let Some(node) = self.doc().mandala.node(projected_node.id) else {
                continue;
            };
            if node.shape() == Shape::Arrow && complete_flow(&self.doc().mandala, node.id) {
                continue;
            }
            let spatial = layout.node(node.id);
            let recursive = spatial.is_some_and(|node| node.recursive);
            self.paint_graph_node(
                painter,
                node,
                NodePaint {
                    position: projected_node.position,
                    scale: projected_node.scale,
                    spin,
                    arrow_body: true,
                    recursive,
                },
            );
            if let Some(spatial) = spatial {
                painter.text(
                    projected_node.position
                        + Vec2::new(
                            NODE_R as f32 * projected_node.scale,
                            -NODE_R as f32 * projected_node.scale,
                        ),
                    Align2::LEFT_BOTTOM,
                    format!("z{}", spatial.depth),
                    FontId::monospace(8.5 * projected_node.scale.clamp(0.75, 1.3)),
                    if recursive { k.accent } else { k.chrome },
                );
            }
        }
        if let Some(father) = self.doc().selected.or(self.doc().pending) {
            if let Some(parent) = self.doc().mandala.node(father) {
                let children = self.doc().mandala.children(parent.id);
                for (index, child_id) in children.into_iter().enumerate() {
                    let Some(child) = find(child_id) else {
                        continue;
                    };
                    let scale = child.scale.clamp(0.65, 1.2);
                    let badge = child.position
                        + Vec2::new(
                            -(NODE_R as f32 + 9.0) * scale,
                            -(NODE_R as f32 + 9.0) * scale,
                        );
                    painter.circle_filled(badge, 8.0 * scale, k.fill);
                    painter.circle_stroke(badge, 8.0 * scale, UiStroke::new(1.0, k.accent));
                    painter.text(
                        badge,
                        Align2::CENTER_CENTER,
                        (index + 1).to_string(),
                        FontId::monospace(9.0 * scale),
                        k.accent,
                    );
                }
            }
        }
    }

    fn paint_2d(&self, painter: &egui::Painter, origin: Pos2, time: f32) {
        let spin = time * 1.6;
        let k = self.ink;
        let v = self.doc().view;
        let zoom = v.zoom as f32;
        // World point to on-screen position.
        let at = |x: f64, y: f64| {
            let (sx, sy) = v.to_screen(x, y);
            Pos2::new(origin.x + sx as f32, origin.y + sy as f32)
        };
        self.chaos_star(painter);

        // The semantic projection is shared with 3D: complete flows collapse
        // into one directional blue edge, while father-of links remain grey.
        // Edges and forms are drawn in two passes. Everything outside the boxes
        // goes down first, then the boxes themselves, then the contents — edges
        // included, or a box's own fill would paint over the arrows inside it.
        let paint_edges = |want_interior: bool| {
            for edge in visible_edges(&self.doc().mandala) {
                let interior = self.doc().mandala.is_inlined(edge.from)
                    && self.doc().mandala.is_inlined(edge.to);
                if interior != want_interior {
                    continue;
                }
                // A mediator drawn inside its square needs no link to it: the
                // containment already says what the grey arrow would have said.
                if edge.kind == VisibleEdgeKind::Father
                    && (self.doc().mandala.inlined_mediator(edge.from) == Some(edge.to)
                        || self.doc().mandala.inlined_mediator(edge.to) == Some(edge.from))
                {
                    continue;
                }
                let (Some(from), Some(to)) = (
                    self.doc().mandala.node(edge.from),
                    self.doc().mandala.node(edge.to),
                ) else {
                    continue;
                };
                let (touches, width, head) = match edge.kind {
                    VisibleEdgeKind::Father => (
                        self.doc().is_selected(edge.from) && self.doc().is_selected(edge.owner),
                        1.8,
                        10.0,
                    ),
                    VisibleEdgeKind::Flow => {
                        let hot = self.doc().is_selected(edge.owner)
                            || self.doc().pending == Some(edge.owner);
                        (
                            hot || (self.doc().is_selected(edge.from)
                                && self.doc().is_selected(edge.to)),
                            1.8,
                            11.0,
                        )
                    }
                };
                let stroke = UiStroke::new(
                    if touches {
                        if edge.kind == VisibleEdgeKind::Flow {
                            2.6
                        } else {
                            2.4
                        }
                    } else {
                        width
                    } * zoom,
                    edge_colour(edge.kind, touches, k),
                );
                circuit_trace(
                    painter,
                    at(from.x, from.y),
                    at(to.x, to.y),
                    NODE_R as f32 * zoom,
                    head * zoom,
                    stroke,
                    self.doc().angled.contains(&edge.owner),
                );
            }
        };

        let paint_nodes = |pass: bool| {
            for id in self.doc().mandala.paint_order(pass) {
                let Some(n) = self.doc().mandala.node(id) else {
                    continue;
                };
                if n.shape() == Shape::Arrow && complete_flow(&self.doc().mandala, n.id) {
                    continue;
                }
                let centre = at(n.x, n.y);
                self.paint_graph_node(
                    painter,
                    n,
                    NodePaint {
                        position: centre,
                        scale: zoom,
                        spin,
                        arrow_body: true,
                        recursive: false,
                    },
                );
            }
        };

        paint_edges(false);
        paint_nodes(false);
        paint_edges(true);
        paint_nodes(true);

        if let Some(father) = self.doc().selected.or(self.doc().pending) {
            let children = self.doc().mandala.children(father);
            for (index, child_id) in children.into_iter().enumerate() {
                let Some(child) = self.doc().mandala.node(child_id) else {
                    continue;
                };
                let centre = at(child.x, child.y);
                let scale = zoom.clamp(0.65, 1.2);
                let badge = centre
                    + Vec2::new(
                        -(NODE_R as f32 + 9.0) * scale,
                        -(NODE_R as f32 + 9.0) * scale,
                    );
                painter.circle_filled(badge, 8.0 * scale, k.fill);
                painter.circle_stroke(badge, 8.0 * scale, UiStroke::new(1.0, k.accent));
                painter.text(
                    badge,
                    Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    FontId::monospace(9.0 * scale),
                    k.accent,
                );
            }
        }
        if let Drag::Marquee { start, current, .. } = self.drag {
            let first = at(start.0, start.1);
            let second = at(current.0, current.1);
            let rect = Rect::from_two_pos(first, second);
            painter.rect_filled(rect, 0.0, k.accent.gamma_multiply(0.08));
            painter.rect_stroke(rect, 0.0, UiStroke::new(1.2, k.accent));
        }
    }
}

/// Perspective projection for the derived structural model.
///
/// Camera math is kept outside the egui event handler so it stays a pure,
/// testable transformation. Existing canvas placement supplies X/Y while
/// structural depth supplies Z.
/// The layout's centre of mass and largest extent, in world units. Shared by
/// the projection and the fly controls so movement speed and framing scale
/// with the graph.
/// Draw a connection as a right-angle circuit trace between two node centres,
/// with an arrowhead entering the target. The dominant axis decides whether the
/// trace leaves horizontally (H–V–H) or vertically (V–H–V), so it reads like a
/// board trace routed between components rather than a diagonal wire. `r` is the
/// node radius (where the trace clears the shape) and `head` the arrow size.
fn circuit_trace(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    r: f32,
    head: f32,
    stroke: UiStroke,
    angled: bool,
) {
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    if angled {
        // A straight diagonal line, drawn on request with Shift. Barbs sweep
        // back from the tip along the line.
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (ux, uy) = (dx / len, dy / len);
        let p0 = Pos2::new(from.x + ux * r, from.y + uy * r);
        let p1 = Pos2::new(to.x - ux * r, to.y - uy * r);
        painter.line_segment([p0, p1], stroke);
        for side in [-0.45f32, 0.45] {
            let (cs, sn) = (side.cos(), side.sin());
            let (bx, by) = (ux * cs - uy * sn, ux * sn + uy * cs);
            painter.line_segment([p1, Pos2::new(p1.x - bx * head, p1.y - by * head)], stroke);
        }
        return;
    }
    if dx.abs() >= dy.abs() {
        let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
        let p0 = Pos2::new(from.x + dir * r, from.y);
        let p1 = Pos2::new(to.x - dir * r, to.y);
        let mid = (p0.x + p1.x) * 0.5;
        painter.line_segment([p0, Pos2::new(mid, p0.y)], stroke);
        painter.line_segment([Pos2::new(mid, p0.y), Pos2::new(mid, p1.y)], stroke);
        painter.line_segment([Pos2::new(mid, p1.y), p1], stroke);
        painter.line_segment(
            [p1, Pos2::new(p1.x - dir * head, p1.y - head * 0.6)],
            stroke,
        );
        painter.line_segment(
            [p1, Pos2::new(p1.x - dir * head, p1.y + head * 0.6)],
            stroke,
        );
    } else {
        let dir = if dy >= 0.0 { 1.0 } else { -1.0 };
        let p0 = Pos2::new(from.x, from.y + dir * r);
        let p1 = Pos2::new(to.x, to.y - dir * r);
        let mid = (p0.y + p1.y) * 0.5;
        painter.line_segment([p0, Pos2::new(p0.x, mid)], stroke);
        painter.line_segment([Pos2::new(p0.x, mid), Pos2::new(p1.x, mid)], stroke);
        painter.line_segment([Pos2::new(p1.x, mid), p1], stroke);
        painter.line_segment(
            [p1, Pos2::new(p1.x - head * 0.6, p1.y - dir * head)],
            stroke,
        );
        painter.line_segment(
            [p1, Pos2::new(p1.x + head * 0.6, p1.y - dir * head)],
            stroke,
        );
    }
}

fn project_spatial(
    layout: &SpatialLayout,
    rect: Rect,
    camera: SpatialCamera,
) -> Vec<ProjectedNode> {
    if layout.nodes.is_empty() {
        return Vec::new();
    }
    let bounds = layout.nodes.iter().fold(
        (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ),
        |(min_x, max_x, min_y, max_y, min_z, max_z), node| {
            (
                min_x.min(node.x),
                max_x.max(node.x),
                min_y.min(node.y),
                max_y.max(node.y),
                min_z.min(node.z),
                max_z.max(node.z),
            )
        },
    );
    let centre = (
        (bounds.0 + bounds.1) * 0.5,
        (bounds.2 + bounds.3) * 0.5,
        (bounds.4 + bounds.5) * 0.5,
    );
    let span = (bounds.1 - bounds.0)
        .max(bounds.3 - bounds.2)
        .max(bounds.5 - bounds.4)
        .max(120.0) as f32;
    let available = (rect.width() - 140.0).min(rect.height() - 110.0).max(80.0);
    let fit = (available / span).clamp(0.35, 1.2);
    let camera_distance = span * 2.4 + 220.0;
    let (yaw_cos, yaw_sin) = (camera.yaw.cos(), camera.yaw.sin());
    let (pitch_cos, pitch_sin) = (camera.pitch.cos(), camera.pitch.sin());

    layout
        .nodes
        .iter()
        .map(|node| {
            // The world-space viewpoint offset moves the eye through the scene,
            // so a vertical move stays vertical and a move never slides the flat
            // image along the flow axis.
            let x = (node.x - centre.0) as f32 - camera.pan[0];
            let y = (node.y - centre.1) as f32 - camera.pan[1];
            let z = (node.z - centre.2) as f32 - camera.pan[2];
            let yaw_x = yaw_cos * x + yaw_sin * z;
            let yaw_z = -yaw_sin * x + yaw_cos * z;
            let pitch_y = pitch_cos * y - pitch_sin * yaw_z;
            let camera_depth = pitch_sin * y + pitch_cos * yaw_z;
            let perspective =
                camera_distance / (camera_distance + camera_depth).max(camera_distance * 0.25);
            let screen_scale = fit * camera.zoom * perspective;
            ProjectedNode {
                id: node.id,
                position: rect.center() + Vec2::new(yaw_x * screen_scale, pitch_y * screen_scale),
                // Glyph size tracks the SAME `fit` the positions use, so a node
                // and the gap to its neighbour scale together. Otherwise a large
                // graph packs the positions while the glyphs stay full size, and
                // the arrow shaft between two shapes collapses to nothing.
                scale: screen_scale.clamp(0.4, 1.8),
                camera_depth,
            }
        })
        .collect()
}

/// Paint and drive the shared screen-neutral Vim editor.
///
/// egui supplies focus, pointer hit-testing, clipboard events, and pixels; all
/// editing state transitions are delegated to `kaos-workspace`, the same core
/// used by the terminal frontend.
/// The live parse state, drawn under an editor.
///
/// Every text surface that holds Rebis answers the same question in the same
/// words — the ones the terminal app already uses — so the answer reads the
/// same wherever you meet it.
pub(crate) fn source_status(ui: &mut egui::Ui, k: Ink, source: &str) {
    let state = kaos_workspace::rebis_workspace::SourceState::of(source);
    if matches!(state, kaos_workspace::rebis_workspace::SourceState::Empty) {
        return;
    }
    ui.horizontal_wrapped(|ui| {
        let tone = if state.is_valid() {
            k.faint
        } else {
            k.secondary
        };
        ui.colored_label(tone, state.mark());
        let detail = state.detail();
        if !detail.is_empty() {
            ui.colored_label(k.secondary, detail);
        }
    });
}

/// The parenthesis pair around a plain `TextEdit`'s cursor, if any.
///
/// The Source tab owns a real editor and can be asked directly. These boxes are
/// ordinary egui widgets, so the cursor has to be read back out of the state
/// egui stored for them last frame — which is why each one needs a stable id.
pub(crate) fn matched_pair(
    ctx: &egui::Context,
    id: egui::Id,
    source: &str,
) -> Option<(usize, usize)> {
    let state = egui::text_edit::TextEditState::load(ctx, id)?;
    let range = state.cursor.char_range()?;
    kaos_workspace::rebis_workspace::matching_form(source, range.primary.index)
}

/// One Rebis syntax colouring, shared by every text surface in the editor.
///
/// Symbols carry the blue, operators and delimiters the red, prompt text stays
/// ink. Extracted so the Source tab, the mandala's source box, and the run
/// draft cannot drift into three different-looking editors.
pub(crate) fn rebis_layout_job(
    source: &str,
    ink: Ink,
    font: f32,
    selected: &dyn Fn(usize) -> bool,
    matched: Option<(usize, usize)>,
) -> egui::text::LayoutJob {
    let syntax = highlights(source);
    let mut job = egui::text::LayoutJob::default();
    // Code does not soft-wrap: Rebis indentation carries meaning, so a long
    // line is reached by scrolling, not by being folded.
    job.wrap.max_width = f32::INFINITY;
    for (index, character) in source.chars().enumerate() {
        let tone = match syntax.get(index).copied().unwrap_or(SourceHighlight::Atom) {
            // Operators and delimiters share one colour — the red — as the
            // language legend in the top bar does, parentheses included.
            SourceHighlight::Forward
            | SourceHighlight::Backflow
            | SourceHighlight::Mediate
            | SourceHighlight::Import
            | SourceHighlight::Invert
            | SourceHighlight::Parenthesis => ink.secondary,
            SourceHighlight::Whitespace | SourceHighlight::Comment => ink.faint,
            // Symbols carry the blue; prompt text is content and stays ink.
            SourceHighlight::Atom | SourceHighlight::Invalid => ink.accent,
            SourceHighlight::Prompt => ink.ink,
        };
        // A matched pair is marked on the delimiters themselves. Selection
        // wins where they overlap: a selection is something you are doing, a
        // match is something you are being told.
        let pair = matched.is_some_and(|(left, right)| index == left || index == right);
        job.append(
            &character.to_string(),
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(font),
                color: tone,
                background: if selected(index) {
                    ink.accent.gamma_multiply(0.28)
                } else if pair {
                    ink.accent.gamma_multiply(0.20)
                } else {
                    Color32::TRANSPARENT
                },
                underline: if pair {
                    UiStroke::new(1.0, tone)
                } else {
                    UiStroke::NONE
                },
                ..egui::TextFormat::default()
            },
        );
    }
    if source.is_empty() {
        job.append(
            " ",
            0.0,
            egui::TextFormat {
                font_id: FontId::monospace(font),
                color: ink.ink,
                ..egui::TextFormat::default()
            },
        );
    }
    job
}

fn draw_source_editor(
    ui: &mut egui::Ui,
    pane: &mut SourcePane,
    ink: Ink,
    actions: &mut Vec<SourceAction>,
) {
    let source = pane.editor.source();
    let cursor_before_frame = pane.editor.cursor();
    let reveal_cursor = pane.revealed_cursor != Some(cursor_before_frame);
    let selections = pane.editor.selection_ranges(pane.mode);
    let selected = |index: usize| {
        selections
            .iter()
            .any(|(from, to)| *from <= index && index < *to)
    };
    let job = rebis_layout_job(
        source,
        ink,
        14.0,
        &selected,
        pane.editor.matching_parentheses(),
    );
    let galley = ui.painter().layout_job(job);
    let viewport_height = ui.available_height().max(260.0);
    let interaction = egui::ScrollArea::both()
        .id_salt("visual_vim_source")
        .auto_shrink([false, false])
        .max_height(viewport_height)
        .show(ui, |ui| {
            let desired = Vec2::new(
                galley.size().x.max(ui.available_width()),
                galley.size().y.max(viewport_height - 8.0),
            );
            let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, ink.ground);
            ui.painter().galley(rect.min, galley.clone(), ink.ink);
            if response.has_focus() {
                let cursor =
                    galley.pos_from_ccursor(egui::text::CCursor::new(pane.editor.cursor()));
                let x = rect.left() + cursor.left();
                let top = rect.top() + cursor.top();
                let bottom = rect.top() + cursor.bottom();
                // Vim's non-insert modes (normal, visual) show a block cursor
                // over the character; insert mode and non-Vim editing keep the
                // thin bar. The block is one monospace cell wide.
                let block = pane.vim_enabled && pane.mode != VimMode::Insert;
                if reveal_cursor {
                    if block {
                        let cell = ui.fonts(|f| f.glyph_width(&FontId::monospace(14.0), 'M'));
                        let rect =
                            Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + cell, bottom));
                        ui.scroll_to_rect(rect, None);
                    } else {
                        ui.scroll_to_rect(
                            Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + 2.0, bottom)),
                            None,
                        );
                    }
                }
                if block {
                    let cell = ui.fonts(|f| f.glyph_width(&FontId::monospace(14.0), 'M'));
                    let rect = Rect::from_min_max(Pos2::new(x, top), Pos2::new(x + cell, bottom));
                    // Translucent so the character under the cursor stays legible.
                    ui.painter()
                        .rect_filled(rect, 1.0, ink.accent.gamma_multiply(0.55));
                } else {
                    ui.painter().line_segment(
                        [Pos2::new(x, top), Pos2::new(x, bottom)],
                        UiStroke::new(1.5, ink.accent),
                    );
                }
            }
            (rect, response)
        })
        .inner;
    let (rect, response) = interaction;

    let source_len = pane.editor.source().chars().count();
    let pointer_cursor = || {
        response.interact_pointer_pos().map(|pointer| {
            galley
                .cursor_from_pos(pointer - rect.min)
                .ccursor
                .index
                .min(source_len)
        })
    };
    if response.clicked() {
        response.request_focus();
        if pane.vim_enabled
            && matches!(
                pane.mode,
                VimMode::Visual | VimMode::VisualLine | VimMode::VisualBlock
            )
        {
            pane.editor.end_visual();
            pane.mode = VimMode::Normal;
        }
        if let Some(cursor) = pointer_cursor() {
            pane.editor.set_cursor(cursor);
        }
    }
    if response.drag_started() {
        response.request_focus();
        if let Some(cursor) = pointer_cursor() {
            pane.editor.set_cursor(cursor);
            if pane.vim_enabled {
                pane.editor.begin_visual(false);
                pane.mode = VimMode::Visual;
            }
        }
    } else if response.dragged() && pane.vim_enabled {
        if let Some(cursor) = pointer_cursor() {
            pane.editor.set_cursor(cursor);
        }
    }

    // egui treats these six keys as focus navigation and surrenders the
    // widget's focus over them before it is ever polled — Escape and Tab at the
    // top of the pass, the arrows during the interact. All six belong to the
    // editor: Escape leaves insert mode, Tab indents, the arrows move the
    // caret. Claim them so egui keeps its hands off.
    const CLAIMED: [egui::Key; 6] = [
        egui::Key::Escape,
        egui::Key::Tab,
        egui::Key::ArrowUp,
        egui::Key::ArrowDown,
        egui::Key::ArrowLeft,
        egui::Key::ArrowRight,
    ];
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        );
    });
    // The lock only takes hold once the widget has been focused for a whole
    // frame, so the first claimed key after clicking in still arrives with the
    // focus already handed away. Take it back and handle the key anyway.
    let stolen = response.lost_focus()
        && ui.input(|input| CLAIMED.iter().any(|key| input.key_pressed(*key)));
    if stolen {
        response.request_focus();
    }
    let focused = response.has_focus() || stolen;

    if focused {
        let events = ui.input(|input| input.events.clone());
        for event in events {
            if pane.mode == VimMode::Command {
                match event {
                    egui::Event::Key {
                        key: egui::Key::Escape,
                        pressed: true,
                        ..
                    } => {
                        pane.command.clear();
                        pane.mode = if pane.vim_enabled {
                            VimMode::Normal
                        } else {
                            VimMode::Insert
                        };
                    }
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        pressed: true,
                        ..
                    } => {
                        actions.push(SourceAction::VimCommand(std::mem::take(&mut pane.command)));
                        pane.mode = if pane.vim_enabled {
                            VimMode::Normal
                        } else {
                            VimMode::Insert
                        };
                    }
                    egui::Event::Key {
                        key: egui::Key::Backspace,
                        pressed: true,
                        ..
                    } => {
                        pane.command.pop();
                    }
                    egui::Event::Text(text) => pane.command.push_str(&text),
                    _ => {}
                }
                continue;
            }

            if matches!(event, egui::Event::Copy) {
                if let Some(text) = pane.editor.selected_text(pane.mode) {
                    pane.editor.set_yank(text.clone());
                    ui.ctx().copy_text(text);
                }
                continue;
            }
            let mapped = match event {
                egui::Event::Paste(text) => {
                    if pane.vim_enabled && pane.mode != VimMode::Insert {
                        Some((
                            EditKey::Char('v'),
                            EditModifiers {
                                ctrl: true,
                                shift: false,
                            },
                        ))
                    } else {
                        Some((EditKey::Paste(text), EditModifiers::default()))
                    }
                }
                egui::Event::Text(text) if pane.mode == VimMode::Insert => {
                    Some((EditKey::Paste(text), EditModifiers::default()))
                }
                egui::Event::Text(text) => {
                    for character in text.chars() {
                        apply_visual_edit_key(
                            ui,
                            pane,
                            EditKey::Char(character),
                            EditModifiers::default(),
                            actions,
                        );
                    }
                    None
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => egui_edit_key(key, modifiers),
                _ => None,
            };
            if let Some((key, modifiers)) = mapped {
                apply_visual_edit_key(ui, pane, key, modifiers, actions);
            }
        }
    }

    if focused && pane.editor.cursor() == cursor_before_frame {
        pane.revealed_cursor = Some(cursor_before_frame);
    }

    if pane.mode == VimMode::Command {
        ui.colored_label(
            ink.accent,
            egui::RichText::new(format!(":{}", pane.command)).monospace(),
        );
    }
}

fn apply_visual_edit_key(
    ui: &mut egui::Ui,
    pane: &mut SourcePane,
    key: EditKey,
    modifiers: EditModifiers,
    actions: &mut Vec<SourceAction>,
) {
    let effect = handle_edit_key(
        &mut pane.editor,
        &mut pane.mode,
        pane.vim_enabled,
        key,
        modifiers,
    );
    if effect.yanked {
        ui.ctx().copy_text(pane.editor.yank().to_string());
        pane.notice = Some("selection yanked and copied".to_string());
    }
    if effect.command {
        pane.command.clear();
    }
    if effect.save {
        actions.push(SourceAction::SaveFile {
            path: pane.file_path.clone(),
            text: pane.editor.source().to_string(),
        });
    }
    if effect.unmatched_parenthesis {
        pane.notice = Some("no matching structural parenthesis".to_string());
    }
}

fn egui_edit_key(key: egui::Key, modifiers: egui::Modifiers) -> Option<(EditKey, EditModifiers)> {
    let modifiers = EditModifiers {
        ctrl: modifiers.ctrl || modifiers.command,
        shift: modifiers.shift,
    };
    let key = match key {
        egui::Key::Escape => EditKey::Escape,
        egui::Key::Enter => EditKey::Enter,
        egui::Key::Tab => EditKey::Tab,
        egui::Key::Backspace => EditKey::Backspace,
        egui::Key::Delete => EditKey::Delete,
        egui::Key::ArrowLeft => EditKey::Left,
        egui::Key::ArrowRight => EditKey::Right,
        egui::Key::ArrowUp => EditKey::Up,
        egui::Key::ArrowDown => EditKey::Down,
        egui::Key::Home => EditKey::Home,
        egui::Key::End => EditKey::End,
        egui::Key::R if modifiers.ctrl => EditKey::Char('r'),
        egui::Key::S if modifiers.ctrl => EditKey::Char('s'),
        egui::Key::V if modifiers.ctrl => EditKey::Char('v'),
        egui::Key::C if modifiers.ctrl => EditKey::Char('c'),
        egui::Key::OpenBracket if modifiers.ctrl => EditKey::Char('['),
        _ => return None,
    };
    Some((key, modifiers))
}

fn default_text(form: &Form) -> String {
    Form::ALL
        .iter()
        .find(|(_, make, _)| make() == *form)
        .map(|(_, _, text)| (*text).to_string())
        .unwrap_or_default()
}

/// Keep long captions from overflowing their shape on the canvas.
fn truncate(label: &str) -> String {
    const MAX: usize = 11;
    if label.chars().count() <= MAX {
        return label.to_string();
    }
    let head: String = label.chars().take(MAX - 1).collect();
    format!("{head}…")
}

// ── the generation ──────────────────────────────────────────────────────────

/// Paint the automaton.
///
/// The composition itself lives in [`automata::Automaton::compose`] — this is
/// the translation from its marks into egui shapes plus the palette. Keeping the
/// split means the offline preview in `examples/generation_preview.rs` renders
/// the same figure this does, rather than an approximation of it.
fn draw_generation(ui: &mut egui::Ui, machine: &automata::Automaton, k: Ink) {
    use automata::Mark;

    let available = ui.available_size();
    let rect = ui.allocate_response(available, Sense::hover()).rect;
    let painter = ui.painter_at(rect);
    let centre = rect.center();
    // Leave a margin for the rim glyphs, which sit outside the outermost ring.
    let extent = (rect.width().min(rect.height()) / 2.0 - 34.0).max(12.0);

    let at = |(x, y): (f32, f32)| Pos2::new(x, y);
    // A state maps to a colour along the shared ramp: faint → ink, then ink →
    // accent only at the top of the range.
    let tint = |state: u8, alpha: f32| -> Color32 {
        let (from, to, local) = match automata::ramp(state) {
            automata::Ramp::Dim(t) => (k.ground, k.faint, t),
            automata::Ramp::Quiet(t) => (k.faint, k.ink, t),
            automata::Ramp::Loud(t) => (k.ink, k.accent, t),
        };
        let lerp = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * local) as u8;
        Color32::from_rgba_unmultiplied(
            lerp(from.r(), to.r()),
            lerp(from.g(), to.g()),
            lerp(from.b(), to.b()),
            (alpha.clamp(0.0, 1.0) * 255.0) as u8,
        )
    };

    for mark in machine.compose((centre.x, centre.y), extent) {
        match mark {
            Mark::Dot {
                at: point,
                radius,
                state,
                alpha,
                filled,
            } => {
                let colour = tint(state, alpha);
                if filled {
                    painter.circle_filled(at(point), radius, colour);
                } else {
                    painter.circle_stroke(at(point), radius, UiStroke::new(1.0, colour));
                }
            }
            Mark::Line {
                from,
                to,
                width,
                state,
                alpha,
            } => {
                painter.line_segment([at(from), at(to)], UiStroke::new(width, tint(state, alpha)));
            }
            Mark::Poly {
                points,
                state,
                alpha,
                filled,
                width,
            } => {
                let colour = tint(state, alpha);
                let points: Vec<Pos2> = points.into_iter().map(at).collect();
                painter.add(egui::Shape::convex_polygon(
                    points,
                    if filled { colour } else { Color32::TRANSPARENT },
                    if filled {
                        UiStroke::NONE
                    } else {
                        UiStroke::new(width, colour)
                    },
                ));
            }
            Mark::Cross {
                at: point,
                arm,
                state,
                alpha,
            } => {
                let stroke = UiStroke::new(1.0, tint(state, alpha));
                painter.line_segment(
                    [
                        Pos2::new(point.0 - arm, point.1 - arm),
                        Pos2::new(point.0 + arm, point.1 + arm),
                    ],
                    stroke,
                );
                painter.line_segment(
                    [
                        Pos2::new(point.0 - arm, point.1 + arm),
                        Pos2::new(point.0 + arm, point.1 - arm),
                    ],
                    stroke,
                );
            }
            Mark::Bit {
                from,
                to,
                value,
                on,
                stream,
                alpha,
                ..
            } => {
                // The two tapes get separate semantic colours. Brightness is
                // still derived from the byte itself, so the halo does not
                // reduce the response to a decorative on/off animation.
                let base = match stream {
                    automata::BinaryStream::Prompt => k.secondary,
                    automata::BinaryStream::Response => k.accent,
                };
                let byte_weight = 0.35 + value as f32 / 255.0 * 0.65;
                let width = if on { 2.0 } else { 0.65 };
                painter.line_segment(
                    [at(from), at(to)],
                    UiStroke::new(width, base.gamma_multiply(alpha * byte_weight)),
                );
            }
        }
    }

    // The legend, in the corner rather than over the figure.
    let mut y = rect.top() + 6.0;
    for (name, count) in machine.census() {
        painter.text(
            Pos2::new(rect.left() + 8.0, y),
            Align2::LEFT_TOP,
            format!("{name} {count}"),
            FontId::monospace(10.0),
            k.faint,
        );
        y += 13.0;
    }
    painter.text(
        Pos2::new(rect.left() + 8.0, rect.bottom() - 31.0),
        Align2::LEFT_BOTTOM,
        format!(
            "P {}",
            machine.binary_preview(automata::BinaryStream::Prompt, 3)
        ),
        FontId::monospace(10.0),
        k.secondary,
    );
    painter.text(
        Pos2::new(rect.right() - 8.0, rect.bottom() - 8.0),
        Align2::RIGHT_BOTTOM,
        format!(
            "R {}",
            machine.binary_preview(automata::BinaryStream::Response, 3)
        ),
        FontId::monospace(10.0),
        k.accent,
    );
}

// ── terminal hand-off ───────────────────────────────────────────────────────

/// Hand the current drawing to a terminal session: write it out, then open a
/// terminal running `kaos rebis edit` on it.
///
/// The drawing leaves as ordinary Rebis source in a real file, so the terminal
/// side needs to know nothing about the canvas.
fn open_in_terminal(source: &str, cwd: &std::path::Path) -> Result<(), String> {
    let path = std::env::temp_dir().join(format!("kaos-visual-{}.rebis", std::process::id()));
    std::fs::write(&path, source)
        .map_err(|e| format!("could not write {}: {e}", path.display()))?;
    let exe = runs::kaos_executable();
    // The session opens in the directory the editor was started from, so
    // relative reads, imports and output paths mean there what they mean here.
    launch_terminal(
        &format!(
            "{} rebis edit {}",
            shell_quote(&exe.to_string_lossy()),
            shell_quote(&path.to_string_lossy())
        ),
        cwd,
    )
}

/// Single-quote a path for `sh -c`, so spaces and metacharacters survive.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

#[cfg(target_os = "macos")]
fn launch_terminal(command: &str, cwd: &std::path::Path) -> Result<(), String> {
    // AppleScript is the only reliable way to get a *new* Terminal window
    // running a command; `open -a Terminal` cannot take arguments. `cd` first
    // so the session starts in the same working context.
    let full = format!("cd {} && {command}", shell_quote(&cwd.to_string_lossy()));
    let escaped = full.replace('\\', r"\\").replace('"', r#"\""#);
    let script = format!(
        r#"tell application "Terminal"
             activate
             do script "{escaped}"
           end tell"#
    );
    std::process::Command::new("osascript")
        .arg("-e")
        .arg(script)
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("could not open Terminal: {e}"))
}

#[cfg(not(target_os = "macos"))]
fn launch_terminal(command: &str, cwd: &std::path::Path) -> Result<(), String> {
    // There is no standard terminal on Linux/BSD, so try the usual suspects in
    // order. `x-terminal-emulator` is the Debian alternatives entry and so
    // respects the user's own choice when it exists.
    const TERMINALS: &[(&str, &[&str])] = &[
        ("x-terminal-emulator", &["-e"]),
        ("gnome-terminal", &["--"]),
        ("konsole", &["-e"]),
        ("xfce4-terminal", &["-e"]),
        ("alacritty", &["-e"]),
        ("kitty", &["-e"]),
        ("wezterm", &["start", "--"]),
        ("foot", &["-e"]),
        ("xterm", &["-e"]),
    ];
    // Keep the shell open afterwards so the session is usable, not a flash.
    let inner = format!("{command}; exec \"$SHELL\"");
    for (bin, args) in TERMINALS {
        let spawned = std::process::Command::new(bin)
            .args(*args)
            .arg("sh")
            .arg("-c")
            .arg(&inner)
            // The session inherits the editor's working context.
            .current_dir(cwd)
            .spawn();
        if spawned.is_ok() {
            return Ok(());
        }
    }
    Err("no terminal found (tried gnome-terminal, konsole, alacritty, kitty, xterm…)".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Height of the top header at a given window width, laid out headlessly.
    fn header_height(width: f32) -> f32 {
        let mut editor = Editor::new(Mandala::new());
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, 900.0),
        ));
        // egui sizes a panel from the previous frame's content, so settle it.
        let mut top = 0.0;
        for _ in 0..3 {
            let _ = ctx.run(input.clone(), |ctx| {
                editor.header(ctx);
                // Inside the pass: the space the header left for everything
                // else starts exactly where the header ends.
                top = ctx.available_rect().top();
            });
        }
        top
    }

    #[test]
    fn the_header_wraps_in_a_narrow_window_instead_of_clipping() {
        // A plain `horizontal` row overflows its panel silently — the last
        // control is simply cut off the right edge, with nothing in the layout
        // to show for it. A wrapped row reflows onto a second line, so HEIGHT
        // is the observable that tells the two apart.
        let narrow = header_height(420.0);
        let wide = header_height(1600.0);
        assert!(
            narrow > wide,
            "header must reflow when narrow: narrow={narrow} wide={wide}"
        );
    }

    /// Drive the Source tab headlessly: click into it, wait `idle` frames, then
    /// press `pressed`. Returns the pane the interaction left behind.
    ///
    /// A button is laid out beside the editor because focus navigation needs
    /// somewhere to go — with the editor alone on screen, egui has no other
    /// widget to hand the focus to and the bug hides.
    fn drive_source_tab(mode: VimMode, idle: usize, pressed: egui::Key) -> SourcePane {
        let mut pane = SourcePane::with_text("(-> a b)");
        pane.vim_enabled = true;
        pane.mode = mode;
        let ink = crate::theme::Ink::load();

        let ctx = egui::Context::default();
        let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(900.0, 600.0));
        let centre = screen.center();
        let click = |pressed: bool| egui::Event::PointerButton {
            pos: centre,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::default(),
        };
        let key = |key: egui::Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::default(),
        };

        // A click is a press in one frame and a release in the next; the
        // release is what focuses the editor. Idle frames in between stand for
        // the repaints a real session does before the next keystroke.
        let mut frames = vec![
            vec![],
            vec![egui::Event::PointerMoved(centre), click(true)],
            vec![click(false)],
        ];
        frames.extend(std::iter::repeat_n(vec![], idle));
        frames.push(vec![key(pressed)]);

        for events in frames {
            let input = egui::RawInput {
                events,
                screen_rect: Some(screen),
                ..Default::default()
            };
            let mut actions = Vec::new();
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    draw_source_editor(ui, &mut pane, ink, &mut actions);
                    let _ = ui.button("elsewhere");
                });
            });
        }
        pane
    }

    #[test]
    fn escape_leaves_insert_mode_in_the_visual_source_tab() {
        // egui claims Escape for focus navigation and surrenders the widget's
        // focus at the top of the pass, before the editor is polled — so a
        // has_focus() gate over the event loop drops the key and Vim never
        // leaves insert mode. Both the immediate Escape (focus already gone
        // this frame) and the settled one (focus lock installed) must land.
        for idle in [0, 3] {
            let pane = drive_source_tab(VimMode::Insert, idle, egui::Key::Escape);
            assert_eq!(
                pane.mode,
                VimMode::Normal,
                "Escape after {idle} idle frames must leave insert mode"
            );
        }
    }

    #[test]
    fn tab_indents_in_the_visual_source_tab_instead_of_moving_focus() {
        // Escape is not the only key egui takes for focus navigation: Tab and
        // the arrows go the same way. Tab is the one with a visible effect in
        // the buffer, so it stands in for all three.
        for idle in [0, 3] {
            let pane = drive_source_tab(VimMode::Insert, idle, egui::Key::Tab);
            // The click that focuses the editor lands past the end of this
            // one-line buffer, so the caret — and the indent — sit at the end.
            assert_eq!(
                pane.editor.source(),
                "(-> a b)  ",
                "Tab after {idle} idle frames must indent, not move focus"
            );
        }
    }

    #[test]
    fn symbols_are_blue_and_operators_red_in_every_editor() {
        // One layout function serves the Source tab, the mandala's source box,
        // and the run draft, so pinning it here pins all three.
        let k = crate::theme::Ink::load();
        //             0 12 3 45 6 789 10
        let job = rebis_layout_job("(-> ab \"p\")", k, 14.0, &|_| false, None);
        let colour = |i: usize| job.sections[i].format.color;
        for (i, what) in [(0, "("), (1, "-"), (2, ">"), (10, ")")] {
            assert_eq!(colour(i), k.secondary, "operator {what} must be red");
        }
        for i in [4, 5] {
            assert_eq!(colour(i), k.accent, "a symbol must be blue");
        }
        // The quote marks themselves are classified Atom by the shared
        // highlighter, so they take the symbol red; the text between them is
        // Prompt and stays ink.
        assert_eq!(colour(8), k.ink, "prompt text stays ink");
        for i in [7, 9] {
            assert_eq!(colour(i), k.accent, "quote marks follow the symbols");
        }
        assert_ne!(k.accent, k.secondary);
    }

    #[test]
    fn a_matched_pair_is_marked_and_nothing_else_is() {
        // One layout function serves all three editors, so marking it here
        // marks it in the Source tab, the mandala's source box, and the run
        // draft alike.
        let k = crate::theme::Ink::load();
        let source = "(-> a b)";
        let pair = kaos_workspace::rebis_workspace::matching_form(source, 0);
        assert_eq!(pair, Some((0, 7)), "the outer parentheses are the pair");

        let job = rebis_layout_job(source, k, 14.0, &|_| false, pair);
        let marked: Vec<usize> = (0..source.chars().count())
            .filter(|index| job.sections[*index].format.underline != UiStroke::NONE)
            .collect();
        assert_eq!(marked, vec![0, 7]);

        // With the cursor away from any delimiter, nothing is marked.
        let none = rebis_layout_job(source, k, 14.0, &|_| false, None);
        assert!((0..source.chars().count())
            .all(|index| none.sections[index].format.underline == UiStroke::NONE));
    }

    #[test]
    fn a_flow_arrow_keeps_its_red_when_the_block_is_selected() {
        // The hue is what separates a flow from a structural link. Selection is
        // allowed to thicken an edge and to colour the otherwise-grey `father
        // of` links, but repainting a flow would erase the one cue that says
        // what it is — and it does so exactly when the drawing is being read
        // closest.
        let k = crate::theme::Ink::load();
        for touches in [false, true] {
            assert_eq!(
                edge_colour(VisibleEdgeKind::Flow, touches, k),
                k.secondary,
                "a flow must stay red with touches={touches}"
            );
        }
        assert_eq!(edge_colour(VisibleEdgeKind::Father, false, k), k.faint);
        assert_eq!(edge_colour(VisibleEdgeKind::Father, true, k), k.accent);
        assert_ne!(k.secondary, k.accent, "the two roles must be distinct");
    }

    fn prompt_doc() -> Doc {
        let mut doc = Doc::default();
        doc.mandala.add(Form::Prompt, "first", 0.0, 0.0);
        doc
    }

    /// Run one frame of the run-status modal over an editor, feeding `events`.
    fn frame_of_run_status(editor: &mut Editor, ctx: &egui::Context, events: Vec<egui::Event>) {
        let input = egui::RawInput {
            events,
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(900.0, 700.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| editor.run_status_modal(ctx));
    }

    /// An editor showing the status modal for one freshly submitted live run,
    /// which is therefore waiting for authority.
    fn editor_awaiting_authority() -> (Editor, u64) {
        let mut editor = Editor::new(Mandala::new());
        editor.runs.mode = runs::Mode::Direct;
        editor.runs.authority_remembered = false;
        let id = editor
            .runs
            .submit("\"work\"".to_string(), None, &editor.cwd.clone());
        assert_eq!(
            editor.runs.runs[0].state,
            runs::State::AwaitingPermission,
            "a live run must stop for authority"
        );
        editor.run_notice = Some(id);
        (editor, id)
    }

    #[test]
    fn escape_denies_the_authority_the_run_modal_is_asking_for() {
        // The authority question is raised the instant a live run is submitted —
        // while this modal is the thing on screen. Escape answers it by denying,
        // as it does in the terminal; the same key must not refuse power on one
        // screen and dismiss the question on the other.
        let (mut editor, _) = editor_awaiting_authority();
        let ctx = egui::Context::default();
        let escape = || {
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::default(),
            }]
        };

        frame_of_run_status(&mut editor, &ctx, escape());
        assert_eq!(
            editor.runs.runs[0].state,
            runs::State::Cancelled,
            "Escape denied the run"
        );
        assert!(
            editor.runs.runs[0]
                .output
                .iter()
                .any(|line| line.contains("denied")),
            "the denial is recorded on the run: {:?}",
            editor.runs.runs[0].output
        );

        // With no question standing, the same key goes back to closing the modal.
        assert!(editor.run_notice.is_some(), "the modal stayed to show why");
        frame_of_run_status(&mut editor, &ctx, escape());
        assert_eq!(editor.run_notice, None, "Escape now closes the modal");
    }

    #[test]
    fn granting_from_the_modal_acts_on_that_run_not_the_selected_one() {
        // `grant_selected` works on the desk's selection, which is not
        // necessarily the run this modal is about — a second submission moves
        // it. Granting here must free the run the reader is looking at.
        let (mut editor, first) = editor_awaiting_authority();
        let cwd = editor.cwd.clone();
        let second = editor.runs.submit("\"other\"".to_string(), None, &cwd);
        assert_eq!(
            editor.runs.selected,
            Some(second),
            "the newer run is selected"
        );

        // The modal is still the one for the first run.
        assert_eq!(editor.run_notice, Some(first));
        editor.runs.selected = Some(first);
        editor.runs.grant_selected(runs::Authority::Once, &cwd);

        let state_of = |editor: &Editor, id: u64| {
            editor
                .runs
                .runs
                .iter()
                .find(|run| run.id == id)
                .map(|run| run.state)
        };
        assert_ne!(
            state_of(&editor, first),
            Some(runs::State::AwaitingPermission),
            "the modal's run was granted"
        );
        assert_eq!(
            state_of(&editor, second),
            Some(runs::State::AwaitingPermission),
            "the other run still waits on its own decision"
        );
    }

    #[test]
    fn a_primary_drag_resizes_from_a_wall_and_moves_from_anywhere_else() {
        let mut doc = Doc::default();
        let square = doc.mandala.add(Form::Square, "", 0.0, 0.0);
        let (half_w, half_h) = doc.mandala.extent(square);

        // On the wall: a resize, naming the axis the wall governs. Aiming at the
        // drawn line lands either side of it, so both sides must resize — a miss
        // to the outside used to reach bare canvas and pan the whole view.
        for x in [half_w - 1.0, half_w, half_w + 1.0] {
            let wall = Drag::beginning_at(&doc.mandala, x, 0.0);
            let Drag::Resize(grab) = wall else {
                panic!("the wall at {x} must begin a resize, not a pan or a move");
            };
            assert_eq!(grab.id, square);
            assert!(grab.wide && !grab.tall);
        }

        // In the middle: the box moves, exactly as before.
        assert!(
            matches!(
                Drag::beginning_at(&doc.mandala, 0.0, 0.0),
                Drag::Node { id, .. } if id == square
            ),
            "the middle of the box still moves it"
        );

        // Off the drawing: the canvas pans.
        assert!(matches!(
            Drag::beginning_at(&doc.mandala, 900.0, 900.0),
            Drag::Pan
        ));

        // Carrying that wall outward widens the box and leaves it where it is.
        doc.mandala.resize(square, half_w + 50.0, half_h);
        assert_eq!(doc.mandala.extent(square), (half_w + 50.0, half_h));
        assert_eq!(
            doc.mandala.node(square).map(|node| (node.x, node.y)),
            Some((0.0, 0.0))
        );
    }

    #[test]
    fn drawing_history_undoes_and_redoes_one_semantic_edit() {
        let mut doc = prompt_doc();
        doc.checkpoint();
        doc.mandala.add(Form::Symbol, "second", 80.0, 0.0);
        assert_eq!(doc.mandala.nodes().len(), 2);

        assert!(doc.undo());
        assert_eq!(doc.mandala.nodes().len(), 1);
        assert!(doc.redo());
        assert_eq!(doc.mandala.nodes().len(), 2);
    }

    #[test]
    fn deleting_the_selected_shape_is_undoable() {
        let mut doc = prompt_doc();
        let selected = doc.mandala.nodes()[0].id;
        doc.selected = Some(selected);

        assert!(doc.delete_selected());
        assert!(doc.mandala.is_empty());
        assert_eq!(doc.selected, None);
        assert!(doc.undo());
        assert_eq!(doc.mandala.nodes().len(), 1);
        assert_eq!(doc.mandala.nodes()[0].id, selected);
    }

    #[test]
    fn block_selection_toggles_and_deletes_as_one_undoable_edit() {
        let mut doc = Doc::default();
        let first = doc.mandala.add(Form::Prompt, "first", 0.0, 0.0);
        let second = doc.mandala.add(Form::Prompt, "second", 100.0, 0.0);
        let outside = doc.mandala.add(Form::Prompt, "outside", 200.0, 0.0);
        doc.select_many([first, second], false);
        assert_eq!(doc.selection_len(), 2);
        doc.toggle_selection(second);
        assert_eq!(doc.selected_ids(), BTreeSet::from([first]));
        doc.toggle_selection(second);

        assert!(doc.delete_selected());
        assert_eq!(doc.mandala.nodes().len(), 1);
        assert_eq!(doc.mandala.nodes()[0].id, outside);
        assert!(doc.undo());
        assert_eq!(doc.mandala.nodes().len(), 3);
    }

    #[test]
    fn selecting_a_flow_selects_its_whole_block_and_deletes_it_as_one() {
        let mut doc = Doc::default();
        let left = doc.mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = doc.mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = doc.mandala.flow(left, right, Form::Forward).unwrap();
        // Selecting the flow pulls in its two operands — the whole block.
        doc.select_only(flow);
        assert_eq!(doc.selected_ids(), BTreeSet::from([left, right, flow]));

        assert!(doc.delete_selected());
        assert!(doc.mandala.nodes().is_empty());
        assert!(doc.mandala.arrows().is_empty());
        assert!(doc.undo());
        assert_eq!(doc.mandala.nodes().len(), 3);
        assert_eq!(doc.mandala.arrows().len(), 2);
    }

    #[test]
    fn visible_edges_collapse_a_complete_flow_to_one_directional_edge() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = mandala.flow(left, right, Form::Forward).unwrap();
        let parent = mandala.add(Form::Compose, "", 0.0, -200.0);
        mandala.father_of(parent, flow);

        let edges = visible_edges(&mandala);
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge.kind == VisibleEdgeKind::Flow)
                .count(),
            1
        );
        assert_eq!(
            edges
                .iter()
                .filter(|edge| edge.kind == VisibleEdgeKind::Father)
                .count(),
            1
        );
        let flow_edge = edges
            .iter()
            .find(|edge| edge.kind == VisibleEdgeKind::Flow)
            .unwrap();
        assert_eq!(
            (flow_edge.from, flow_edge.to, flow_edge.owner),
            (left, right, flow)
        );
    }

    #[test]
    fn selected_block_copy_pastes_forms_arrows_and_one_undo_unit() {
        let mut doc = Doc::default();
        let left = doc.mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = doc.mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = doc.mandala.flow(left, right, Form::Forward).unwrap();
        doc.select_many([left, right, flow], false);
        let copied = doc.copied_selection().unwrap();
        let originals = doc.selected_ids();

        let pasted = doc.paste_graph(&copied, (28.0, 28.0));
        assert_eq!(pasted.len(), 3);
        assert_eq!(doc.mandala.nodes().len(), 6);
        assert_eq!(doc.mandala.arrows().len(), 4);
        assert_eq!(doc.selected_ids(), pasted.iter().copied().collect());
        assert!(pasted.iter().all(|id| !originals.contains(id)));
        assert_eq!(
            doc.selected_source().unwrap().unwrap(),
            "(-> \"left\" \"right\")"
        );

        assert!(doc.undo());
        assert_eq!(doc.mandala.nodes().len(), 3);
        assert_eq!(doc.mandala.arrows().len(), 2);
    }

    #[test]
    fn selected_source_is_the_exact_induced_subgraph() {
        let mut doc = Doc::default();
        let left = doc.mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = doc.mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = doc.mandala.flow(left, right, Form::Forward).expect("flow");
        doc.mandala.add(Form::Prompt, "outside", 400.0, 0.0);
        doc.select_many([left, right, flow], false);

        assert_eq!(
            doc.selected_source().unwrap().as_deref(),
            Some("(-> \"left\" \"right\")")
        );
        doc.toggle_selection(left);
        assert!(doc.selected_source().is_err());
    }

    #[test]
    fn a_new_edit_after_undo_clears_the_redo_branch() {
        let mut doc = prompt_doc();
        doc.checkpoint();
        doc.mandala.add(Form::Symbol, "discarded", 80.0, 0.0);
        assert!(doc.undo());

        doc.checkpoint();
        doc.mandala.add(Form::Import, "std/flow", 80.0, 0.0);
        assert!(!doc.redo());
        assert_eq!(doc.mandala.nodes().len(), 2);
    }

    #[test]
    fn histories_are_owned_by_their_document_tabs() {
        let mut left = prompt_doc();
        let mut right = Doc::default();
        left.checkpoint();
        left.mandala.add(Form::Symbol, "left only", 80.0, 0.0);
        assert!(left.undo());
        assert!(!right.undo());
        assert!(right.mandala.is_empty());
    }

    #[test]
    fn spatial_projection_turns_nesting_depth_into_visible_separation() {
        let layout = SpatialLayout {
            nodes: vec![
                kaos_core::visual::SpatialNode {
                    id: NodeId(1),
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    depth: 0,
                    recursive: false,
                },
                kaos_core::visual::SpatialNode {
                    id: NodeId(2),
                    x: 0.0,
                    y: 0.0,
                    z: 140.0,
                    depth: 1,
                    recursive: false,
                },
            ],
            recursive_edges: Vec::new(),
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let camera = SpatialCamera {
            yaw: 0.7,
            pitch: 0.2,
            zoom: 1.0,
            pan: [0.0, 0.0, 0.0],
        };
        let projected = project_spatial(&layout, rect, camera);
        assert_eq!(projected.len(), 2);
        assert_ne!(projected[0].position, projected[1].position);
        assert_ne!(projected[0].camera_depth, projected[1].camera_depth);
    }

    #[test]
    fn orbit_state_is_not_part_of_semantic_undo_history() {
        let mut doc = prompt_doc();
        let source = doc.mandala.to_rebis().unwrap();
        doc.camera.yaw += 0.5;
        doc.camera.pitch -= 0.2;
        doc.canvas_mode = CanvasMode::Spatial;
        assert_eq!(doc.mandala.to_rebis().unwrap(), source);
        assert!(doc.undo.is_empty());
    }

    #[test]
    fn brackets_and_parentheses_use_symbol_weight_outlines() {
        let ordinary = node_outline_width(Shape::Circle, false);
        for shape in [Shape::Square, Shape::Oval] {
            assert!(node_outline_width(shape, false) > ordinary);
            assert!(node_outline_width(shape, true) > node_outline_width(shape, false));
        }
    }

    #[test]
    fn visual_block_run_uses_the_caret_and_preserves_top_level_definitions() {
        let text = "(~ inspect (x) (-> x \"report\"))\n(inspect \"parser\")".to_string();
        let cursor = text.find("(inspect \"parser\")").unwrap();
        let mut pane = SourcePane::with_text(text);
        pane.editor.set_cursor(cursor);
        let source = pane.run_block_source().unwrap();
        assert!(source.contains("~ inspect"));
        assert!(source.matches("inspect").count() >= 2);
        assert!(source.contains("\"parser\""));
        assert!(rebis_lang::parse(&source).is_ok());
    }

    #[test]
    fn visual_program_run_uses_a_text_selection_as_block_scope() {
        let text = "(~ inspect (x) (-> x \"report\"))\n(inspect \"parser\")".to_string();
        let start = text.find("(inspect \"parser\")").unwrap();
        let mut pane = SourcePane::with_text(text);
        pane.editor.set_cursor(start);
        pane.editor.begin_visual(false);
        pane.mode = VimMode::Visual;
        pane.editor
            .set_cursor(start + "(inspect \"parser\")".chars().count() - 1);
        let (source, scope) = pane.run_program_source().unwrap();
        assert_eq!(scope, runs::Scope::Block);
        assert!(source.contains("~ inspect"));
        assert!(rebis_lang::parse(&source).is_ok());
    }

    /// The generation opens on its own tab, built from the selected run's own
    /// program — the lattice must be that run's geometry and nothing else.
    #[test]
    fn opening_a_generation_builds_the_lattice_from_the_selected_runs_program() {
        let mut editor = Editor::new(Mandala::new());
        editor.runs.runs.push(runs::Run::test_fixture(
            11,
            "([m] \"one\" \"two\" \"three\")",
            "",
            vec![
                "event    prompt started · abstraction 1 · one".to_string(),
                "answer   a varied answer with many distinct bytes".to_string(),
                "complete    ✓ run finished".to_string(),
            ],
        ));
        editor.runs.selected = Some(11);

        editor.open_generation();

        let Some(Pane::Automata(pane)) = editor.tabs.active() else {
            panic!("the generation did not open on a new tab");
        };
        assert_eq!(pane.run, Some(11));
        assert_eq!(pane.origin, "run #11");
        // A three-branch square: mediator plus three prompts, all under one root.
        assert_eq!(
            pane.machine
                .cells
                .iter()
                .filter(|cell| cell.site == automata::Site::Prompt)
                .count(),
            3
        );
        assert_eq!(pane.consumed, 0, "the transcript is consumed while drawing");
    }

    /// The run's answers are what build the rule, so consuming its transcript
    /// must change how the lattice evolves.
    #[test]
    fn a_runs_answers_drive_the_generation() {
        let mut pane = AutomataPane::from_source("([m] \"one\" \"two\")", "test").unwrap();
        assert_eq!(pane.machine.entropy, 0.0);

        let transcript = vec![
            "event    prompt started · abstraction 1 · one".to_string(),
            "answer   many different characters here 0123456789".to_string(),
            "complete    ✓ run finished".to_string(),
        ];
        pane.consumed = pane.machine.consume(&transcript, pane.consumed);

        assert_eq!(pane.consumed, transcript.len());
        assert_eq!(pane.machine.prompts_seen, 1);
        assert!(
            pane.machine.entropy > 3.0,
            "a varied answer has real entropy"
        );
    }

    /// The pane consumes only the tail of a growing transcript, so a long run
    /// does not cost more each frame. Feeding it in slices must land exactly the
    /// same automaton as feeding it whole.
    #[test]
    fn a_growing_transcript_is_consumed_by_the_tail() {
        let transcript = vec![
            "event    prompt started · abstraction 1 · one".to_string(),
            "answer   a varied first answer".to_string(),
            "event    prompt started · abstraction 1 · two".to_string(),
            "answer   a second answer, differently varied".to_string(),
            "complete    \u{2713} run finished".to_string(),
        ];

        let mut whole = AutomataPane::from_source("([m] \"one\" \"two\")", "test").unwrap();
        whole.consumed += whole.machine.consume(&transcript, 0);
        assert_eq!(whole.consumed, transcript.len());

        let mut tailed = AutomataPane::from_source("([m] \"one\" \"two\")", "test").unwrap();
        for upto in 1..=transcript.len() {
            let tail = &transcript[tailed.consumed.min(upto)..upto];
            tailed.consumed += tailed.machine.consume(tail, 0);
        }

        assert_eq!(tailed.consumed, whole.consumed);
        assert_eq!(tailed.machine.prompts_seen, whole.machine.prompts_seen);
        assert_eq!(tailed.machine.answers_seen, whole.machine.answers_seen);
        assert_eq!(tailed.machine.entropy, whole.machine.entropy);
    }

    /// A program that does not parse has no geometry, so there is nothing to
    /// build a lattice from and the pane must not open empty.
    #[test]
    fn an_unparsable_program_has_no_generation() {
        assert!(AutomataPane::from_source("([m] \"unclosed", "test").is_err());

        let mut editor = Editor::new(Mandala::new());
        editor
            .runs
            .runs
            .push(runs::Run::test_fixture(3, "([m] \"unclosed", "", vec![]));
        editor.runs.selected = Some(3);
        let before = editor.tabs.len();

        editor.open_generation();

        assert_eq!(
            editor.tabs.len(),
            before,
            "no tab opens for a broken program"
        );
        assert!(
            editor
                .notice
                .as_deref()
                .unwrap_or_default()
                .contains("lattice"),
            "the failure explains itself: {:?}",
            editor.notice
        );
    }

    /// With no run selected the generation falls back to the program in front of
    /// you, rather than refusing or opening something empty.
    #[test]
    fn without_a_run_the_generation_uses_the_active_source() {
        let mut editor = Editor::new(Mandala::new());
        editor.tabs.open(
            "source",
            Pane::Source(SourcePane::with_text("(-> \"a\" \"b\")".to_string())),
        );

        editor.open_generation();

        let Some(Pane::Automata(pane)) = editor.tabs.active() else {
            panic!("the generation did not open from the source tab");
        };
        assert_eq!(pane.origin, "source");
        assert_eq!(pane.run, None, "there is no run to watch");
        assert!(!pane.machine.is_empty());
    }

    #[test]
    fn source_generation_ignores_an_older_selected_run() {
        let mut editor = Editor::new(Mandala::new());
        editor.runs.runs.push(runs::Run::test_fixture(
            12,
            "(-> \"old\" \"run\")",
            "",
            vec![],
        ));
        editor.runs.selected = Some(12);

        editor.open_generation_from_source("(-> \"source\" \"tab\")".to_string());

        let Some(Pane::Automata(pane)) = editor.tabs.active() else {
            panic!("the source generation did not open");
        };
        assert_eq!(pane.origin, "source");
        assert_eq!(pane.run, None);
        assert_eq!(
            pane.machine
                .cells
                .iter()
                .filter(|cell| cell.site == automata::Site::Prompt)
                .count(),
            2
        );
    }

    #[test]
    fn opening_a_visual_run_chat_exposes_the_complete_snapshot() {
        let mut editor = Editor::new(Mandala::new());
        editor.runs.runs.push(runs::Run::test_fixture(
            7,
            "(-> \"question\" \"answer\")",
            "captured record",
            vec![
                "prompt   question".to_string(),
                "result   answer".to_string(),
            ],
        ));

        editor.open_run_chat(7);

        let snapshot = editor.run_chat_snapshot(7).unwrap();
        assert!(snapshot.contains("SOURCE\n(-> \"question\" \"answer\")"));
        assert!(snapshot.contains("RECORD / INPUT\ncaptured record"));
        assert!(snapshot.contains("prompt   question\nresult   answer"));
        let Some(Pane::Chat(chat)) = editor.tabs.active() else {
            panic!("run chat did not open");
        };
        assert_eq!(chat.run_id, Some(7));
        assert!(!chat.browsing);
    }
}
