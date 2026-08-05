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
    scales_uniformly, BorderGrab, Form, Legend, Mandala, Node, NodeId, Shape, Sizing,
    SpatialLayout, Stroke, View, WorldRect, BORDER_BAND,
};
use kaos_workspace::rebis_workspace::{
    handle_edit_key, highlights, EditKey, EditModifiers, Editor as SourceEditor,
    Highlight as SourceHighlight, Mode as VimMode,
};

mod actions;
mod automata;
mod music;
mod pdf;
mod torn;
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
    spatial_axis: SpatialAxis,
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
    /// Flow operators the user chose to draw as a right-angle circuit trace
    /// instead of the default straight line.
    ///
    /// Straight is the default because a chain has to read as a chain. A
    /// right-angle trace elbows at the midpoint between its ends, so consecutive
    /// hops of one chain share their vertical run and merge into a single trunk —
    /// which reads as several arrows leaving one form rather than as `1 → 2 → 3`.
    /// A straight line between two boundaries can only be read one way.
    ///
    /// Presentation only, like node positions; it never affects generated Rebis.
    squared: std::collections::HashSet<NodeId>,
    /// The editable source, kept in step with the drawing in both directions.
    text: String,
    /// What the drawing last generated. Comparing against it is how we tell an
    /// edit made on the canvas from one typed into the panel, without either
    /// overwriting the other mid-keystroke.
    generated: String,
    /// Why the source box is not keeping up with the drawing, when it is not.
    ///
    /// A drawing can be in a state that has no source at all — a loop, a form
    /// with the wrong number of operands — and the panel used to answer that by
    /// showing the previous program, which is indistinguishable from being
    /// broken. Saying so is the whole fix.
    source_note: Option<String>,
    /// Frame the whole drawing the next time the canvas is painted.
    ///
    /// Fitting needs the window, and a drawing is loaded long before there is
    /// one, so the intent is recorded here and spent by the first canvas frame
    /// that knows its own size.
    fit_pending: bool,
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

impl SpatialCamera {
    /// The smallest scale the projection still survives, mirroring
    /// [`View::MIN_ZOOM`] for the flat canvas.
    const MIN_ZOOM: f32 = f32::MIN_POSITIVE;

    /// Scale the view by `factor`, with no stop at either end.
    ///
    /// The structural view is the same drawing seen from somewhere else, so it
    /// gets the same unbounded wheel: pulling back has no artificial floor and
    /// pushing in has no ceiling. Only a scale that stops being representable
    /// is refused.
    fn zoom_by(&mut self, factor: f32) {
        if !factor.is_finite() || factor <= 0.0 {
            return;
        }
        let zoom = (self.zoom * factor).max(Self::MIN_ZOOM);
        if zoom.is_finite() {
            self.zoom = zoom;
        }
    }
}

/// Constraint used by the 3D move gizmo. `Free` moves in the camera plane;
/// X/Y/Z follow the structural world's actual axes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
enum SpatialAxis {
    #[default]
    Free,
    X,
    Y,
    Z,
}

impl SpatialAxis {
    const fn label(self) -> &'static str {
        match self {
            Self::Free => "free",
            Self::X => "X",
            Self::Y => "Y",
            Self::Z => "Z",
        }
    }

    const fn component(self) -> Option<usize> {
        match self {
            Self::Free => None,
            Self::X => Some(0),
            Self::Y => Some(1),
            Self::Z => Some(2),
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
enum SpatialDrag {
    #[default]
    None,
    Orbit,
    Move {
        id: NodeId,
        axis: SpatialAxis,
        pointer: Pos2,
        original: [f64; 3],
        scale: f32,
    },
}

#[derive(Clone, Copy)]
struct ProjectedNode {
    id: NodeId,
    position: Pos2,
    scale: f32,
    camera_depth: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SpatialShadowStyle {
    offset: Vec2,
    softness: f32,
    opacity: f32,
}

fn spatial_shadow_style(depth: usize, scale: f32, dark: bool) -> SpatialShadowStyle {
    let lift = depth.min(8) as f32;
    let scale = scale.clamp(0.55, 1.65);
    SpatialShadowStyle {
        // A fixed upper-left key light makes orbiting the structure readable:
        // the graph turns while its studio light remains stable.
        offset: Vec2::new(7.0 + lift * 1.65, 10.0 + lift * 2.55) * scale,
        softness: (6.5 + lift * 1.55) * scale.sqrt(),
        opacity: if dark { 0.78 } else { 0.48 },
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct SpatialLayerPlate {
    depth: usize,
    centre: Pos2,
    radius: Vec2,
    camera_depth: f32,
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
}

fn complete_flow(mandala: &Mandala, id: NodeId) -> bool {
    mandala.flow_result(id).is_some()
}

/// Whether a flow endpoint is something a connection can be drawn to.
///
/// Anything actually on the canvas is. A nested flow is a circle in its own
/// right, so it is the block the arrow above it points at — following the chain
/// through to its innermost result instead would flatten the nesting the source
/// actually wrote.
fn is_flow_boundary(mandala: &Mandala, id: NodeId) -> bool {
    mandala.node(id).is_some()
}

/// Project stored syntax links into the operator connections shown by both
/// canvases.
///
/// Parent/operand structure is read from nesting, so it never receives a grey
/// arrow. A complete `->`/`<-` is the only visible connection and is shown only
/// between the square/circle blocks it routes; leaf forms remain arranged
/// inside those blocks without a second, invented relationship.
fn visible_edges(mandala: &Mandala) -> Vec<VisibleEdge> {
    let mut edges = Vec::new();
    for node in mandala.nodes() {
        if !node.form.is_flow() || !complete_flow(mandala, node.id) {
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
        // A connection attaches to what is DRAWN, and a flow is not drawn — it IS
        // the arrow. So each end resolves to the form that carries the value
        // there: prefix sigils to what they mark, a nested flow to its own first
        // or last stage. Otherwise an arrow arrives from empty canvas with a head
        // on the end of it.
        let from = mandala.flow_endpoint(from, true);
        let to = mandala.flow_endpoint(to, false);
        if !is_flow_boundary(mandala, from) || !is_flow_boundary(mandala, to) {
            continue;
        }
        edges.push(VisibleEdge {
            from,
            to,
            owner: node.id,
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
    volumetric: bool,
}

#[derive(Clone, Copy)]
struct GlyphPaint {
    position: Pos2,
    /// Canvas/projection scale, independent of this symbol's hand-set size.
    view_scale: f32,
    /// Per-axis scale away from the symbol's natural geometry.
    resize: Vec2,
    outline: UiStroke,
    hot: bool,
}

#[derive(Clone, Debug, PartialEq)]
struct CaptionLayout {
    lines: Vec<String>,
    font_size: f32,
    line_height: f32,
    offset: Vec2,
}

/// The outline of an imaginary space: a box whose four sides are each drawn as
/// a brace.
///
/// One side runs corner to corner while its distance from that straight line
/// follows a brace's own profile — a slight curl inward near each end, swinging
/// out to a cusp at the middle. The cusp points AWAY from the interior, which
/// is what `{` does relative to the text it encloses, so the four sides read as
/// the delimiter the form is written with rather than as a decorated square.
///
/// Returned as a closed polyline in screen space, sampled finely enough that
/// the curve carries at any zoom the canvas allows.
fn brace_box_points(rect: Rect, pinch: f32) -> Vec<Pos2> {
    /// Samples per side. Enough that the cusp stays a curve, few enough that a
    /// page full of spaces is still cheap to tessellate.
    const STEPS: usize = 18;

    // How far off the straight line the outline sits, as a fraction of `pinch`,
    // at parameter `s` along one half of a side. Zero at the corner, one at the
    // cusp, dipping slightly negative between them — that dip is the curl.
    fn profile(s: f32) -> f32 {
        s * s - 0.30 * (std::f32::consts::PI * s).sin()
    }

    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let centre = rect.center();
    let mut points = Vec::with_capacity(corners.len() * STEPS);
    for (index, from) in corners.iter().enumerate() {
        let to = corners[(index + 1) % corners.len()];
        let mid = *from + (to - *from) * 0.5;
        // Outward is away from the middle of the box, so every side pinches in
        // the same sense however the rectangle is proportioned.
        let outward = (mid - centre).normalized();
        for step in 0..STEPS {
            let t = step as f32 / STEPS as f32;
            // Mirror the half-profile about the midpoint so the side is
            // symmetric and the cusp lands exactly on it.
            let s = if t <= 0.5 { t * 2.0 } else { (1.0 - t) * 2.0 };
            points.push(*from + (to - *from) * t + outward * (pinch * profile(s)));
        }
    }
    points
}

/// The largest conservative rectangle that stays inside a text-bearing
/// outline. It is intentionally expressed in half-extents so resizing a form
/// expands its typography in the same proportion as its geometry.
fn caption_box(shape: Shape, half: Vec2) -> (Vec2, Vec2) {
    match shape {
        Shape::Circle => (Vec2::ZERO, Vec2::new(half.x * 1.34, half.y * 1.34)),
        Shape::Diamond => (Vec2::ZERO, Vec2::new(half.x * 1.12, half.y * 1.12)),
        Shape::Square | Shape::Brace => (Vec2::ZERO, Vec2::new(half.x * 1.72, half.y * 1.72)),
        Shape::Parallelogram => (Vec2::ZERO, Vec2::new(half.x * 1.42, half.y * 1.54)),
        Shape::Amp => (
            Vec2::new(half.x * 0.10, 0.0),
            Vec2::new(half.x * 1.18, half.y * 1.54),
        ),
        Shape::Hexagon => (Vec2::ZERO, Vec2::new(half.x * 1.50, half.y * 1.55)),
        _ => (Vec2::ZERO, half * 2.0),
    }
}

/// Wrap a caption without deleting or replacing any character. Breaks prefer
/// existing whitespace, but long identifiers still split so an unbroken model
/// or path cannot force an ellipsis.
fn wrap_caption(text: &str, columns: usize) -> Vec<String> {
    let columns = columns.max(1);
    let mut wrapped = Vec::new();
    for source_line in text.split('\n') {
        let mut remaining = source_line.chars().collect::<Vec<_>>();
        if remaining.is_empty() {
            wrapped.push(String::new());
            continue;
        }
        while remaining.len() > columns {
            let split = remaining[..columns]
                .iter()
                .rposition(|character| character.is_whitespace())
                .map(|index| index + 1)
                .filter(|index| *index > 0)
                .unwrap_or(columns);
            wrapped.push(remaining.drain(..split).collect());
        }
        wrapped.push(remaining.into_iter().collect());
    }
    wrapped
}

/// Fit every caption character into its outline. Monospace text is estimated
/// at 0.62 em per character; both the egui painter and PDF renderer consume
/// this same layout, keeping line breaks and scale stable across export.
///
/// `ceiling` is the largest type the consumer can actually set. The on-screen
/// painter has one, because its glyphs are rasterized and the canvas zoom is
/// unbounded; the PDF renderer passes infinity, because its text is curves and
/// a page has no zoom to run away with.
fn fit_caption(text: &str, shape: Shape, half: Vec2, ceiling: f32) -> CaptionLayout {
    let (offset, area) = caption_box(shape, half);
    let fits = |font_size: f32| {
        let columns = (area.x / (font_size * 0.62).max(f32::EPSILON))
            .floor()
            .max(1.0) as usize;
        let lines = wrap_caption(text, columns);
        let line_height = font_size * 1.16;
        let tallest = line_height * lines.len().max(1) as f32;
        let widest = lines
            .iter()
            .map(|line| line.chars().count())
            .max()
            .unwrap_or_default() as f32
            * font_size
            * 0.62;
        (tallest <= area.y && widest <= area.x, lines, line_height)
    };

    // Bounded above by what the consumer will actually draw, so a deeply
    // zoomed-in shape settles on the largest type its renderer can set rather
    // than asking for one the size of its own outline.
    let mut low = MIN_GLYPH_PX;
    let mut high = area.y.clamp(MIN_GLYPH_PX, ceiling.max(MIN_GLYPH_PX));
    for _ in 0..18 {
        let middle = (low + high) * 0.5;
        if fits(middle).0 {
            low = middle;
        } else {
            high = middle;
        }
    }
    // The search lands on an arbitrary fraction of a pixel, and every distinct
    // fraction is a separate rasterisation the atlas keeps forever. Settling on
    // a rung of the shared ladder is what lets a zoom reuse glyphs it has
    // already paid for; it settles DOWNWARD, so the text that fitted still fits.
    let low = glyph_px(low);
    let (_, lines, line_height) = fits(low);
    CaptionLayout {
        lines,
        font_size: low,
        line_height,
        offset,
    }
}

/// The largest glyph the text rasterizer is ever asked for, in pixels.
///
/// The canvas is vectors and its zoom has no ceiling, so a caption's natural
/// size has none either. Outlines can keep growing without complaint; a font
/// atlas cannot. It does not run out of memory — epaint's atlas is capped at
/// 2048×2048 — it *asserts*: a glyph too wide to place trips `x < w && y < h`
/// in its glyph allocator and takes the process with it. Measured on epaint
/// 0.29, 1536px still lands and 1792px panics, so the zoom that grows a caption
/// past roughly 1600px is a crash, not a blurry letter.
///
/// Rasterizing is not free below that either: one glyph costs about 16ms at
/// this size and rises with it, so the ceiling is set where a first use is
/// still worth one frame and there is 5× headroom to the assert. A letter
/// already taller than the window carries no more meaning for being taller
/// still, so raster text stops here while the geometry around it goes on.
const MAX_GLYPH_PX: f32 = 320.0;

/// The smallest glyph worth asking for. Below this a line is a smudge, and the
/// rasterizer should not be woken for it.
const MIN_GLYPH_PX: f32 = 0.35;

/// Distinct glyph sizes the canvas may ask for, per doubling.
///
/// The font atlas caches by exact size and never evicts, so a caption sized
/// `11.0 * zoom` asks for a size no one has ever asked for on every single
/// frame of a wheel-zoom — each one rasterised, stored, and never reused. That
/// is the memory: not a leak, but an atlas filled with hundreds of one-shot
/// renderings until it saturates.
///
/// Snapping onto a ladder makes zooming reuse what it has already paid for. At
/// sixteen rungs per doubling the neighbours are 4.4% apart — below the
/// threshold where growth reads as stepped — and the whole range from a smudge
/// to the ceiling is about 160 sizes, which the atlas holds comfortably.
const GLYPH_RUNGS_PER_DOUBLING: f32 = 16.0;

/// The nearest rung at or below `size`, clamped to what the rasterizer will
/// actually draw.
///
/// Downward, never up: callers have already checked that their text fits at the
/// size they asked for, and a rung below that still fits.
fn glyph_px(size: f32) -> f32 {
    if !size.is_finite() {
        return MIN_GLYPH_PX;
    }
    let size = size.clamp(MIN_GLYPH_PX, MAX_GLYPH_PX);
    let step = 1.0 / GLYPH_RUNGS_PER_DOUBLING;
    // A hair of tolerance in rung space, so a size already standing on a rung
    // stays there instead of sliding down one every time it is settled again.
    // Settling has to be idempotent: it runs over sizes that came out of it.
    let mut rung = (size.log2() / step + 1e-3).floor() * step;
    let mut settled = rung.exp2();
    if settled > size * (1.0 + 1e-6) {
        rung -= step;
        settled = rung.exp2();
    }
    settled.clamp(MIN_GLYPH_PX, MAX_GLYPH_PX)
}

fn node_resize(mandala: &Mandala, node: &Node) -> Vec2 {
    let (half_w, half_h) = mandala.extent(node.id);
    let (base_w, base_h) = node.base_extent();
    Vec2::new(
        (half_w / base_w.max(f64::EPSILON)) as f32,
        (half_h / base_h.max(f64::EPSILON)) as f32,
    )
}

fn resize_weight(resize: Vec2) -> f32 {
    (resize.x * resize.y).sqrt().max(0.1)
}

const fn node_outline_width(shape: Shape, emphasized: bool) -> f32 {
    if matches!(
        shape,
        Shape::Circle
            | Shape::Square
            | Shape::Brace
            | Shape::Parallelogram
            | Shape::Amp
            | Shape::Hexagon
    ) {
        if emphasized {
            2.6
        } else {
            1.8
        }
    } else if emphasized {
        2.0
    } else {
        1.3
    }
}

/// The size of a form's one-token label on the canvas, before the view zoom.
const LABEL_PX: f32 = 11.0;

/// Size the line-number gutter is written at. A shade under the code it numbers,
/// so the numbers read as chrome beside the text rather than as part of it.
const GUTTER_PX: f32 = 11.0;

/// The size of the sigil written on an indentation's ring.
///
/// Larger than an ordinary label, because it is the one mark that says what
/// kind of boundary this is, and it is read against the whole circle rather
/// than against the word beside it.
/// The drawn height of an unresized ring mark. Mirrors the model's
/// [`kaos_core::visual::MARK_HEIGHT`], which is what reserves room for it.
const MARK_PX: f32 = kaos_core::visual::MARK_HEIGHT as f32;

/// The thinnest a drawn line may become, in screen pixels.
///
/// Every stroke is scaled by the view, and a stroke scaled to less than a pixel
/// does not become a fine line — it becomes a faint smear, or nothing. Zoomed
/// out to fit a real program, a 1.8px outline lands at 0.34px and the drawing
/// fades to almost blank: the connections disappear entirely, and the circles
/// survive only because there are many of them.
///
/// So the pen has a floor. Geometry keeps shrinking with the zoom; the ink it
/// is drawn with does not go below what a screen can show.
const HAIRLINE_PX: f32 = 1.15;

/// A stroke width that stays visible however far the canvas is pushed away.
fn pen(width: f32) -> f32 {
    if width.is_finite() {
        width.max(HAIRLINE_PX)
    } else {
        HAIRLINE_PX
    }
}

/// How much a hand-resized symbol is allowed to thicken its own outline.
///
/// A wall's weight belongs to the view, not to how much the wall encloses. Left
/// proportional, a boundary grown to hold a nested program drew a border five
/// times heavier than the same boundary holding one prompt — the drawing got
/// louder the more it contained. A little response is still wanted, so a symbol
/// deliberately made small reads as finer; past that the pen stops growing.
fn pen_weight(resize: Vec2) -> f32 {
    resize_weight(resize).clamp(0.7, 1.25)
}

/// How much a boundary's own size enlarges the sigil written on its ring.
///
/// Unlike the pen, the mark *does* measure the boundary: a wide outer circle
/// wears a larger sigil than a circle nested deep inside it, which is what says
/// at a glance which level of the drawing the eye is on. Damped by a square
/// root so a boundary a hundred times the size of a symbol does not wear a mark
/// a hundred times the size of a letter.
/// How much a ring mark is magnified. Delegates to the model, because the layout
/// reserves room for the mark from this same number — if the two disagreed, a
/// sigil would be drawn larger than the space its container left for it.
fn mark_weight(resize: Vec2) -> f32 {
    kaos_core::visual::mark_weight((f64::from(resize.x), f64::from(resize.y))) as f32
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

    /// Select the whole block rooted at `id` — its operands recursively plus
    /// anything it explicitly holds as visual content. The clicked node stays
    /// the primary (inspector) selection.
    fn select_only(&mut self, id: NodeId) {
        self.selection.clear();
        if self.mandala.node(id).is_some() {
            self.selection = self.mandala.subtree(id);
            self.selection.extend(self.mandala.interior(id));
            // A run of arrows is one thing, so picking up any stage picks up the
            // whole chain: `a → b → c` moves and copies together rather than leaving
            // an arrow stretched from a form that stayed behind. Ctrl-click is still
            // how a single stage is trimmed out of the selection.
            //
            // The chain's ROOT, not its stages one by one. Taking the stages alone
            // left the `->` nodes that join them out of the selection, and a
            // selection of three stages with no arrow between them is not an
            // expression — the panel could only report that it failed to generate
            // source for it.
            if let Some(root) = self.mandala.flow_chain_root(id) {
                for part in self.mandala.subtree(root) {
                    self.selection.insert(part);
                    self.selection.extend(self.mandala.interior(part));
                }
            }
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

    /// A tab holding a drawing that has just been laid out.
    ///
    /// It opens framed: the first canvas frame fits the whole program on screen
    /// rather than dropping the reader at the origin with the drawing somewhere
    /// off the edge.
    fn drawn(mandala: Mandala) -> Self {
        Self {
            mandala,
            fit_pending: true,
            ..Self::default()
        }
    }

    /// Ask for the whole drawing to be framed on the next canvas frame.
    fn show_everything(&mut self) {
        self.fit_pending = true;
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
    /// Messages typed while a turn was still running.
    ///
    /// They are not sent yet and not part of the transcript yet — the model has
    /// not been asked them. They wait here, visible, and go out as the next turn
    /// the moment the current one lands.
    queued: Vec<String>,
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
            queued: Vec::new(),
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
    Sound(String),
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

/// The drawing surface's state.
///
/// The marks in progress live here rather than on the editor, so two drawing
/// tabs are two drawings — the same way two mandala tabs are two programs.
#[derive(Default)]
struct InkPane {
    /// What is being drawn now, in the pane's own coordinates.
    sigil: kaos_core::ink::Sigil,
    /// The mark under the pointer, if one is being made.
    drawing: Option<kaos_core::ink::Stroke>,
    /// Undone marks, newest first, so undo has something to give back.
    undone: Vec<kaos_core::ink::Stroke>,
    /// What to call it on the wall.
    name: String,
    /// Sigils set aside for the next ask.
    stack: kaos_core::ink::Stack,
    /// The last thing the wall said back.
    notice: String,
    /// How wide the pen is.
    pen: f32,
    /// The desire, for Carroll's word method.
    ///
    /// The sentence a person writes before drawing. It produces the faint
    /// letter scaffold, and — under `Retention::Keep` — it is what the wall
    /// remembers the glyph was for.
    desire: String,
    /// Whether the scaffold is drawn behind the strokes.
    ///
    /// A guide, never a mark: it is not in [`InkPane::sigil`], so it cannot
    /// reach the raster a model is shown. A model reading the letters would be
    /// reading the desire, which is the opposite of what a sigil is for.
    scaffold: bool,
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
            ui.add_enabled(
                false,
                egui::Button::new(egui::RichText::new("delete").color(ink.danger)).small(),
            )
            .on_disabled_hover_text("embedded std sigils cannot be deleted");
            return;
        }
        let confirming = pane.pending_delete.as_deref() == Some(&entry.name);
        if ui
            .small_button(
                egui::RichText::new(if confirming {
                    "confirm delete"
                } else {
                    "delete"
                })
                .color(ink.danger),
            )
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

/// Whether this pane can stand in a window of its own.
///
/// Every pane owns enough state to travel into a detached editor. The
/// independent viewport owns that editor rather than borrowing this one, so
/// the same action works for document, conversation, source, settings, run,
/// and action tabs as well as the animated panes.
fn can_tear(_pane: &Pane) -> bool {
    true
}

/// The title a pane is put back on the bar under.
fn pane_title(pane: &Pane) -> String {
    match pane {
        Pane::Mandala(_) => "mandala".to_string(),
        Pane::Chat(_) => "chat".to_string(),
        Pane::Ink(_) => "sigils · drawn".to_string(),
        Pane::Sigils(_) => "sigils".to_string(),
        Pane::Source(_) => "source".to_string(),
        Pane::Settings(_) => "settings".to_string(),
        Pane::Automata(pane) => format!("generation · {}", pane.origin),
        Pane::Music(pane) => format!("sound · {}", pane.origin),
        Pane::Runs => "runs".to_string(),
        Pane::Actions => "actions".to_string(),
    }
}

/// What a tab holds. The tab machinery is generic, so adding a kind is adding
/// a variant here rather than another parallel list.
enum Pane {
    Mandala(Doc),
    Chat(ChatPane),
    /// A surface for DRAWN sigils, and the wall they are kept on.
    ///
    /// Its own pane rather than a tool on the mandala, because the two are not
    /// the same kind of thing. A mandala IS a program — every node is a form and
    /// the picture can be read back as source. A drawn sigil is intent a model
    /// interprets, and nothing in it corresponds to anything in the language.
    /// Sharing a canvas would have made the mandala stop meaning what it means.
    Ink(InkPane),
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
    /// Boxed: the lattice it carries is an order of magnitude larger than any
    /// other pane, and an unboxed variant would set that size as the cost of
    /// every tab in the editor.
    Automata(Box<AutomataPane>),
    /// A program as sound: the Fibonacci reading of its text, played through a
    /// small additive synth and drawn as the wave it makes. Boxed for the same
    /// reason the generation is — it carries a rendered piece, which is
    /// megabytes of samples, and no other tab should pay for that.
    Music(Box<music::MusicPane>),
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
#[derive(Clone, Debug, PartialEq)]
enum Tool {
    /// Place this form.
    Place(Form),
    /// Click two shapes to link them with a flow node (`->` or `<-`). The node
    /// is created behind the arrow, so drawing the arrow *is* writing the form.
    Flow(Form),
    /// Click a shape to select it.
    Select,
    /// Drag anywhere to move the view.
    ///
    /// A tool rather than a held key. Panning by holding space depended on the
    /// key reaching the canvas on the exact frame a drag began, which it did not
    /// do reliably — the hand cursor appeared and the drawing stayed put. A tool
    /// is either chosen or not, and a drag under it always moves the view.
    Pan,
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

/// The world-space thickness of a wall's grab band under this view.
///
/// [`BORDER_BAND`] is a screen measurement, so it divides by the zoom: a wall
/// stays the same number of pixels wide whether the canvas is magnified a
/// thousandfold or pushed far away. Zoom is floored well above zero by
/// [`View`], so this cannot divide by nothing.
fn grab_band(view: View) -> f64 {
    BORDER_BAND / view.zoom.max(View::MIN_ZOOM)
}

/// An in-progress pointer gesture.
#[derive(Clone, Copy, PartialEq)]
struct HolderSnapshot {
    id: NodeId,
    centre: (f64, f64),
    extent: (f64, f64),
    circular: bool,
    /// The boundary's own hand-set size before the drag pinned it, so the wall
    /// can be handed back to its contents when the piece lands.
    size: Option<(f64, f64)>,
}

impl HolderSnapshot {
    fn contains(self, x: f64, y: f64) -> bool {
        let (dx, dy) = (x - self.centre.0, y - self.centre.1);
        if self.circular {
            dx * dx + dy * dy <= self.extent.0 * self.extent.0
        } else {
            dx.abs() <= self.extent.0 && dy.abs() <= self.extent.1
        }
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Drag {
    None,
    /// Moving a shape. `grab` is the offset from the shape's centre, in world
    /// units, so the shape does not jump to the cursor.
    Node {
        id: NodeId,
        grab: (f64, f64),
        holder: Option<HolderSnapshot>,
    },
    /// Dragging a symbol's dashed scale outline. The centre stays put; a visual
    /// container's contents therefore stay still while its boundary changes.
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
    /// A scale outline offered for resizing wins first, then whatever shape the point
    /// landed on, and bare canvas pans. `Mandala::resize_grab` is what keeps a
    /// block dropped across a wall grabbable as a block.
    ///
    /// `band` is the world-space wall tolerance for this view — see
    /// [`grab_band`].
    fn beginning_at(mandala: &Mandala, wx: f64, wy: f64, band: f64) -> Self {
        if let Some(grab) = mandala.resize_grab(wx, wy, band) {
            return Drag::Resize(grab);
        }
        match mandala.hit(wx, wy) {
            Some(id) => {
                let (x, y) = mandala.node(id).map(|n| (n.x, n.y)).unwrap_or((wx, wy));
                let holder = mandala.holder(id).and_then(|holder| {
                    mandala.node(holder).map(|node| HolderSnapshot {
                        id: holder,
                        centre: (node.x, node.y),
                        extent: mandala.extent(holder),
                        circular: node.form == Form::Compose,
                        size: mandala.hand_size(holder),
                    })
                });
                Drag::Node {
                    id,
                    grab: (wx - x, wy - y),
                    holder,
                }
            }
            None => Drag::Pan,
        }
    }
}

/// One chat turn the pane has accepted, ready to be dispatched once the
/// borrow on the active tab ends.
struct ChatSubmission {
    said: String,
    session: String,
    resume: bool,
    run_id: Option<u64>,
    history: Vec<(String, String)>,
}

fn recent_chat_history(turns: &[kaos_core::sessions::Turn]) -> Vec<(String, String)> {
    let limit = kaos_core::chat::MAX_CHAT_HISTORY_BYTES;
    let mut answer: Option<&str> = None;
    let mut bytes = 0usize;
    let mut recent = Vec::new();
    for turn in turns.iter().rev() {
        match turn.role {
            kaos_core::sessions::Role::Model => {
                answer.get_or_insert(&turn.text);
            }
            kaos_core::sessions::Role::User => {
                let answer = answer.take().unwrap_or_default();
                let turn_bytes = turn.text.len().saturating_add(answer.len());
                if !recent.is_empty() && bytes.saturating_add(turn_bytes) > limit {
                    break;
                }
                recent.push((
                    bounded_context_copy(&turn.text, limit / 2),
                    bounded_context_copy(answer, limit / 2),
                ));
                bytes = bytes.saturating_add(turn_bytes);
                if bytes >= limit {
                    break;
                }
            }
        }
    }
    recent.reverse();
    recent
}

fn bounded_context_copy(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_string();
    }
    const MARKER: &str = "[... earlier text omitted ...]\n";
    let mut start = text.len() - max_bytes.saturating_sub(MARKER.len());
    while !text.is_char_boundary(start) {
        start += 1;
    }
    format!("{MARKER}{}", &text[start..])
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
pub fn open(arg: &str) -> Opened {
    let arg = arg.trim();
    if arg.is_empty() {
        return Opened::Drawing(Mandala::new());
    }
    let (source, path) = match std::fs::read_to_string(arg) {
        Ok(text) => (text, Some(arg.to_string())),
        Err(_) => (arg.to_string(), None),
    };
    match Mandala::from_rebis(&source) {
        Ok(mandala) => Opened::Drawing(mandala),
        // A program that does not parse still opens — as text, where it can be
        // repaired. Refusing at the door sends the reader back to whatever they
        // were using before, which is exactly the moment the editor is wanted.
        Err(error) => Opened::Source {
            text: source,
            path,
            error: error.to_string(),
        },
    }
}

/// What [`open`] made of its argument.
///
/// A drawing and a broken program are both openable; only the drawing has a
/// graph. Keeping the two apart here means the window decides which surface to
/// show, rather than the caller deciding whether to open at all.
pub enum Opened {
    /// Source that parses: it has a mandala.
    Drawing(Mandala),
    /// Source that does not. It opens in the Source tab with its diagnostic —
    /// there is no drawing, because a broken program has no graph.
    Source {
        text: String,
        /// The file it came from, when the argument named one, so saving the
        /// repair goes back where it belongs.
        path: Option<String>,
        error: String,
    },
}

/// Open the editor window on `start`. Blocks until the window closes.
pub fn run(start: Opened) {
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
            let editor = Editor::opened(start);
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
    /// Tabs pulled out into windows of their own. See [`torn`].
    torn: Vec<torn::Torn>,
    /// Distinguishes one torn window from the next; only has to be unique
    /// within this session.
    torn_sequence: u64,
    tabs: Tabs<Pane>,
    /// Stands in while a chat tab is active, so the canvas code never has to
    /// ask whether there is a drawing. It is never drawn.
    scratch: Doc,
    /// The chaos stance for conversations: compose the turn as a Rebis program
    /// and run that, instead of answering it.
    ///
    /// Runs already had their own three-way mode in the launch modal
    /// (dry/direct/chaos) and keep it — a run is already a program, so "chaos"
    /// there means something narrower and well-defined. This is the same stance
    /// for the surface that had none. It is deliberately NOT derived from
    /// `runs.mode`: wanting composed chats while still dry-running the result is
    /// an ordinary way to work, and collapsing the two would forbid it.
    ///
    /// Defaults from `KAOS_CHAOS`/config, like every other surface.
    chaos: bool,
    /// The tool is deliberately shared across tabs: it is a mode of working,
    /// not a property of a drawing.
    tool: Tool,
    drag: Drag,
    spatial_drag: SpatialDrag,
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
    /// Space is held down for panning, having gone down while nothing was being
    /// typed into.
    ///
    /// Armed on the PRESS rather than tested while held, because the canvas
    /// deliberately drops text focus when a drag starts: a space typed into a
    /// prompt would otherwise become a pan the moment the pointer touched the
    /// canvas, since by then nothing wants the keyboard any more.
    space_pan: bool,
    /// Children whose full text the panel is showing rather than its first
    /// token.
    ///
    /// View state, not document state: it is deliberately not on [`Doc`], so
    /// opening a caption is not an edit and undo does not step back through it.
    /// A drawing shows one token per form on the canvas because a drawing is a
    /// map — the panel is where the whole of it lives, and until now the panel
    /// truncated it too, which left nowhere to read a long prompt.
    unfolded: std::collections::HashSet<NodeId>,
}

impl Editor {
    #[cfg(test)]
    fn new(mandala: Mandala) -> Self {
        Self::opened(Opened::Drawing(mandala))
    }

    /// Build the editor on whatever [`open`] resolved.
    ///
    /// A drawing lands on the canvas. A program that does not parse lands in the
    /// Source tab beside an empty canvas, carrying its diagnostic: the editor is
    /// where it gets fixed, so refusing to open it would be refusing the reader
    /// the one tool that helps.
    fn opened(start: Opened) -> Self {
        let mut tabs = Tabs::new();
        let mut notice = None;
        match start {
            Opened::Drawing(mandala) => {
                tabs.open("mandala", Pane::Mandala(Doc::drawn(mandala)));
            }
            Opened::Source { text, path, error } => {
                // The empty canvas comes first so the drawing surface still
                // exists — the repaired program has somewhere to be drawn.
                tabs.open("mandala", Pane::Mandala(Doc::default()));
                let name = path
                    .as_deref()
                    .and_then(|path| {
                        std::path::Path::new(path)
                            .file_name()
                            .map(|name| name.to_string_lossy().into_owned())
                    })
                    .unwrap_or_else(|| "source".to_string());
                tabs.open(
                    name,
                    Pane::Source(SourcePane {
                        file_path: path.unwrap_or_default(),
                        notice: Some(format!("does not parse yet · {error}")),
                        ..SourcePane::with_text(text)
                    }),
                );
                notice = Some(format!("opened as source · {error}"));
            }
        }
        let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        Self {
            notice,
            ink: Ink::load(),
            cwd_edit: cwd.display().to_string(),
            torn: Vec::new(),
            torn_sequence: 0,
            cwd,
            tabs,
            scratch: Doc::default(),
            chaos: kaos_core::chaos::enabled(),
            tool: Tool::Select,
            drag: Drag::None,
            spatial_drag: SpatialDrag::None,
            pending_run: None,
            run_notice: None,
            clipboard: None,
            runs: runs::Desk::default(),
            actions: actions::Desk::default(),
            unfolded: std::collections::HashSet::new(),
            space_pan: false,
        }
    }

    /// Build the small editor that lives behind a torn-off viewport.
    ///
    /// A viewport callback cannot borrow the main editor: it is a deferred
    /// paint pass, possibly on another thread. Keeping a complete editor
    /// around the moved pane lets every tab keep using its existing controls
    /// and state, including source actions, chat, runs, and settings, rather
    /// than reducing the less independent tabs to a static preview.
    fn detached(
        pane: Pane,
        ink: Ink,
        cwd: std::path::PathBuf,
        runs: &runs::Desk,
        actions: &actions::Desk,
    ) -> Self {
        let mut editor = Self::opened(Opened::Drawing(Mandala::new()));
        editor.tabs = Tabs::new();
        editor.tabs.open(pane_title(&pane), pane);
        editor.ink = ink;
        editor.cwd_edit = cwd.display().to_string();
        editor.cwd = cwd;
        editor.runs = runs.snapshot();
        editor.actions = actions.snapshot();
        editor.torn.clear();
        editor.torn_sequence = 0;
        editor.notice = None;
        editor
    }

    /// Title the viewport after the pane it owns.
    fn detached_title(&self) -> String {
        self.tabs
            .active()
            .map(pane_title)
            .unwrap_or_else(|| "kaos visual".to_string())
    }

    /// Paint the active pane in a deferred viewport.
    ///
    /// The normal window surrounds this with its header, palette, side panel,
    /// and footer. A torn window deliberately has none of that chrome, but it
    /// still runs the same pane renderer and polls its own process desks so a
    /// chat or run remains live after leaving the main window.
    fn show_detached(&mut self, ui: &mut egui::Ui, ink: Ink) {
        self.ink = ink;
        self.poll_run(ui.ctx());
        self.poll_actions(ui.ctx());
        self.detached_stop_bar(ui);
        if self.on_mandala() {
            self.sync();
        }
        match self.tabs.active() {
            Some(Pane::Mandala(_)) => self.canvas(ui),
            Some(Pane::Chat(_)) => self.chat(ui),
            Some(Pane::Source(_)) => self.source(ui),
            Some(Pane::Ink(_)) => self.ink_tab(ui),
            Some(Pane::Sigils(_)) => self.sigils(ui),
            Some(Pane::Automata(_)) => self.automata_tab(ui),
            Some(Pane::Music(_)) => self.detached_music_tab(ui),
            Some(Pane::Settings(_)) => self.settings(ui),
            Some(Pane::Runs) => self.runs_tab(ui),
            Some(Pane::Actions) => self.actions_tab(ui),
            None => {}
        }
        self.run_modal(ui.ctx());
        self.run_status_modal(ui.ctx());
    }

    /// A detached window has no main header, so keep its stop affordance local
    /// as well. This covers model work launched from a detached chat, source,
    /// runs, or actions pane.
    fn detached_stop_bar(&mut self, ui: &mut egui::Ui) {
        let active = self.runs.active_count() + self.actions.active_count();
        if active == 0 {
            return;
        }
        let mut stop = false;
        ui.horizontal(|ui| {
            ui.colored_label(self.ink.secondary, format!("{active} model task(s) active"));
            if ui
                .button(egui::RichText::new("stop all").color(self.ink.danger))
                .on_hover_text("cancel every active model or tool process in this window")
                .clicked()
            {
                stop = true;
            }
        });
        if stop {
            self.stop_all_work();
        }
        ui.separator();
    }

    /// Stop every model/tool child owned by this editor and discard queued chat
    /// turns, while leaving text currently being drafted untouched.
    fn stop_all_work(&mut self) {
        let runs = self.runs.active_count();
        let actions = self.actions.active_count();
        self.runs.cancel_all(&self.cwd);
        self.actions.cancel_all(&self.cwd);
        let mut queued = 0;
        for tab in self.tabs.iter_mut() {
            if let Pane::Chat(chat) = &mut tab.content {
                queued += chat.queued.len();
                chat.queued.clear();
            }
        }
        let total = runs + actions + queued;
        if total > 0 {
            self.notice = Some(format!("stopped {total} active/queued model task(s)"));
        }
    }

    /// Return every tab still owned by a detached editor when its viewport
    /// closes. A pane may have opened a related tab while it was away (for
    /// example, source can open a mandala), so reclaiming only the active one
    /// would silently discard user work.
    fn reclaim_detached(mut self) -> Vec<Pane> {
        let mut panes = Vec::new();
        while let Some(id) = self.tabs.active_id() {
            let Some(pane) = self.tabs.close(id) else {
                break;
            };
            panes.push(pane);
        }
        panes.reverse();
        panes
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
                    .rect_filled(screen, 0.0, frost_shadow(k, 140.0 / 255.0));
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
                    .rect_filled(screen, 0.0, frost_shadow(self.ink, 140.0 / 255.0));
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
                ui.colored_label(run_state_tone(run.state, run.paused, self.ink), state);
                ui.label(format!("{} · {}", run.mode.label(), run.scope.label()));
                ui.label(preview);
                if let Some(last_output) = &last_output {
                    ui.separator();
                    paint_stream_line(ui, last_output, self.ink);
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
                    session.push(kaos_core::sessions::Role::Model, reply.clone());
                    let _ = store.save(&session);
                }
            }
            self.adopt_composed_program(&reply);
        }
        self.dispatch_queued_chat();
    }

    /// In chaos mode a chat answers with a program, so open the program where
    /// programs are opened — in a Source tab, beside the canvas.
    ///
    /// The terminal app queues the same program as a run; here the editor is
    /// the natural landing place, because the next thing a person does with a
    /// composed program is read it, and the run modal is one button away from
    /// the tab it lands in. Either way the program is not started: composing
    /// costs one call, running it costs whatever it says on the page, and that
    /// second decision stays the user's.
    ///
    /// Guarded on both the stance and the parser, for the reasons given on the
    /// terminal app's version: a direct chat quoting Rebis is answering a
    /// question, and an unparseable fence is not a program.
    fn adopt_composed_program(&mut self, reply: &str) {
        if !self.chaos {
            return;
        }
        let Some(source) = kaos_core::chaos::extract_program(reply) else {
            return;
        };
        if rebis_lang::parse(&source).is_err() {
            self.notice =
                Some("chaos mode composed a program that does not parse — left in the chat".into());
            return;
        }
        self.tabs
            .open("composed", Pane::Source(SourcePane::with_text(source)));
        self.notice = Some(
            "chaos mode composed a program · opened as a tab, not started — run it when you have read it"
                .into(),
        );
    }

    /// Send whatever was typed while the model was working.
    ///
    /// A message written mid-answer does not stop the turn in flight and does
    /// not open a second one beside it: it waits, visible, and goes out as the
    /// next turn the moment that one lands. Several of them go out as one turn,
    /// in the order they were written, because that is how they were meant to
    /// be read.
    fn dispatch_queued_chat(&mut self) {
        let ready = self
            .tabs
            .iter_mut()
            .filter_map(|tab| match &mut tab.content {
                Pane::Chat(chat) if !chat.queued.is_empty() => {
                    Some((chat.session.id.clone(), std::mem::take(&mut chat.queued)))
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        for (session, queued) in ready {
            if self.actions.session_active(&session) {
                // Still busy: put them back and wait for the next landing.
                if let Some(chat) = self.chat_pane_mut(&session) {
                    chat.queued = queued;
                }
                continue;
            }
            self.dispatch_chat_turn(&session, queued.join("\n\n"));
        }
    }

    fn chat_pane_mut(&mut self, session: &str) -> Option<&mut ChatPane> {
        self.tabs.iter_mut().find_map(|tab| match &mut tab.content {
            Pane::Chat(chat) if chat.session.id == session => Some(chat),
            _ => None,
        })
    }

    /// Ask the model one thing, recording it in the transcript first.
    ///
    /// The same path the input box takes, so a queued message and a typed one
    /// are the same turn in every respect.
    fn dispatch_chat_turn(&mut self, session: &str, said: String) {
        let Some(chat) = self.chat_pane_mut(session) else {
            return;
        };
        let resume = chat
            .session
            .turns
            .iter()
            .any(|turn| turn.role == kaos_core::sessions::Role::Model);
        chat.session
            .push(kaos_core::sessions::Role::User, said.clone());
        let prior = chat.session.turns.len().saturating_sub(1);
        let history = recent_chat_history(&chat.session.turns[..prior]);
        let run_id = chat.run_id;
        let _ = kaos_core::sessions::Store::default_store().save(&chat.session);

        let history_text = history
            .iter()
            .map(|(user, assistant)| format!("USER: {user}\nASSISTANT: {assistant}"))
            .collect::<Vec<_>>()
            .join("\n\n");
        // A run chat keeps its own envelope in either stance: it is a question
        // ABOUT a run, and the useful answer is prose about what happened, not
        // a program that would re-do it.
        let prompt = run_id
            .and_then(|id| self.run_chat_context(id, &history, &said))
            .unwrap_or_else(|| {
                kaos_core::chat::DEFAULT_CONTEXT.render_turn(self.chaos, &history_text, &said)
            });
        self.actions
            .submit_chat(prompt, session.to_string(), resume, &self.cwd);
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

/// Operator arrows remain blue in both projections; selection changes their
/// weight, never the colour carrying direction.
pub(crate) const fn edge_colour(k: Ink) -> Color32 {
    k.secondary
}

/// Semantic roles carried by the first column of a retained run stream.
///
/// The process deliberately emits plain text so it can be piped, copied, and
/// saved without frontend markup. The visual projection can still recover the
/// small vocabulary at the start of each line and spend colour on meaning
/// rather than painting the entire transcript one undifferentiated grey.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StreamKind {
    Signal,
    Success,
    Progress,
    Caution,
    Plain,
}

impl StreamKind {
    const fn marker(self) -> &'static str {
        match self {
            Self::Signal => "◇",
            Self::Success => "●",
            Self::Progress => "→",
            Self::Caution => "△",
            Self::Plain => "·",
        }
    }

    const fn tone(self, k: Ink) -> Color32 {
        match self {
            Self::Signal => k.secondary,
            Self::Success | Self::Progress => k.accent,
            Self::Caution => k.danger,
            Self::Plain => k.faint,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StreamLine<'a> {
    tag: Option<&'a str>,
    body: &'a str,
    kind: StreamKind,
}

/// Split one machine-readable stream line into its visual label and content.
///
/// Only known prefixes become labels. Arbitrary provider output remains one
/// intact body, so the renderer never mistakes the first word of an answer for
/// metadata.
fn stream_line(line: &str) -> StreamLine<'_> {
    let trimmed = line.trim_end();
    let content = trimmed.trim_start();
    let boundary = content.find(char::is_whitespace);
    let (candidate, rest) = boundary.map_or((content, ""), |at| {
        (&content[..at], content[at..].trim_start())
    });
    let rest_lower = rest.to_ascii_lowercase();
    let negative_body = ["failed", "error", "cancelled", "refused", "denied"]
        .iter()
        .any(|word| rest_lower.starts_with(word));
    let kind = if negative_body {
        Some(StreamKind::Caution)
    } else {
        match candidate {
            "event" | "prompt" | "model" | "chat" | "firing" | "directive" => {
                Some(StreamKind::Signal)
            }
            "answer" | "result" | "complete" | "received" | "reply" | "score" => {
                Some(StreamKind::Success)
            }
            "started" | "resumed" | "input" => Some(StreamKind::Progress),
            "diagnostic" | "paused" | "awaiting" | "permission" | "cancelled" | "failed"
            | "error" | "note" => Some(StreamKind::Caution),
            _ => None,
        }
    };
    kind.map_or(
        StreamLine {
            tag: None,
            body: trimmed,
            kind: StreamKind::Plain,
        },
        |kind| StreamLine {
            tag: Some(candidate),
            body: rest,
            kind,
        },
    )
}

/// Paint a retained output line as a compact semantic row.
fn paint_stream_line(ui: &mut egui::Ui, line: &str, k: Ink) {
    let parsed = stream_line(line);
    ui.horizontal_top(|ui| {
        if let Some(tag) = parsed.tag {
            ui.add_sized(
                [102.0, 18.0],
                egui::Label::new(
                    egui::RichText::new(format!(
                        "{} {}",
                        parsed.kind.marker(),
                        tag.to_ascii_uppercase()
                    ))
                    .monospace()
                    .strong()
                    .size(11.0)
                    .color(parsed.kind.tone(k)),
                ),
            );
        } else {
            ui.add_space(102.0);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(parsed.body)
                    .monospace()
                    .size(12.5)
                    .color(if parsed.kind == StreamKind::Plain {
                        k.faint
                    } else {
                        k.ink
                    }),
            )
            .wrap(),
        );
    });
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum StreamSection {
    Line(String),
    Fold {
        title: String,
        body: Vec<StreamSection>,
    },
}

/// Turn the shared child stream into a small tree. Folds are deliberately
/// parsed at paint time: retained logs can grow while a model is working, and
/// egui's stable header ids preserve a user's open/closed choice between frames.
fn stream_sections(lines: &[String]) -> Vec<StreamSection> {
    let mut roots = Vec::new();
    let mut stack: Vec<(String, Vec<StreamSection>)> = Vec::new();

    for raw in lines {
        match kaos_core::fold::classify(raw) {
            kaos_core::fold::Marker::Open(title) => {
                stack.push((kaos_core::chat::strip_terminal_escapes(title), Vec::new()));
            }
            kaos_core::fold::Marker::Close => {
                let Some((title, body)) = stack.pop() else {
                    continue;
                };
                let section = StreamSection::Fold { title, body };
                if let Some((_, parent)) = stack.last_mut() {
                    parent.push(section);
                } else {
                    roots.push(section);
                }
            }
            kaos_core::fold::Marker::Line(line) => {
                let line = kaos_core::chat::strip_terminal_escapes(line);
                if line.trim().is_empty() {
                    continue;
                }
                if let Some((_, body)) = stack.last_mut() {
                    body.push(StreamSection::Line(line));
                } else {
                    roots.push(StreamSection::Line(line));
                }
            }
        }
    }

    // A killed child can leave an open section without its CLOSE marker. Keep
    // that visible as a collapsible section rather than losing its partial work.
    while let Some((title, body)) = stack.pop() {
        let section = StreamSection::Fold { title, body };
        if let Some((_, parent)) = stack.last_mut() {
            parent.push(section);
        } else {
            roots.push(section);
        }
    }
    roots
}

fn stream_section_lines(items: &[StreamSection]) -> usize {
    items
        .iter()
        .map(|item| match item {
            StreamSection::Line(_) => 1,
            StreamSection::Fold { body, .. } => stream_section_lines(body),
        })
        .sum()
}

fn paint_stream_sections(ui: &mut egui::Ui, lines: &[String], k: Ink, salt: &str) {
    let sections = stream_sections(lines);
    for (index, section) in sections.iter().enumerate() {
        paint_stream_section(ui, section, k, salt, &[index]);
    }
}

fn paint_stream_section(
    ui: &mut egui::Ui,
    section: &StreamSection,
    k: Ink,
    salt: &str,
    path: &[usize],
) {
    match section {
        StreamSection::Line(line) => paint_stream_line(ui, line, k),
        StreamSection::Fold { title, body } => {
            let line_count = stream_section_lines(body);
            let header = if line_count == 0 {
                title.clone()
            } else {
                format!(
                    "{}  ·  {} line{}",
                    title,
                    line_count,
                    if line_count == 1 { "" } else { "s" }
                )
            };
            egui::CollapsingHeader::new(
                egui::RichText::new(header)
                    .monospace()
                    .size(11.5)
                    .color(k.secondary),
            )
            .id_salt(("model_work", salt, path.to_vec()))
            .default_open(false)
            .show(ui, |ui| {
                for (index, child) in body.iter().enumerate() {
                    let mut child_path = path.to_vec();
                    child_path.push(index);
                    paint_stream_section(ui, child, k, salt, &child_path);
                }
            });
        }
    }
}

fn role_wash(tone: Color32, alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(tone.r(), tone.g(), tone.b(), alpha)
}

/// A shadow stays in the palette's navy end instead of introducing pure black.
fn frost_shadow(k: Ink, opacity: f32) -> Color32 {
    let tone = if k.ground.r() < k.ink.r() {
        k.ground
    } else {
        k.ink
    };
    role_wash(tone, (opacity.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// A readable, selectable code surface shared by chat context and run detail.
fn paint_code_panel(ui: &mut egui::Ui, text: &str, k: Ink, tone: Color32) {
    egui::Frame::none()
        .fill(k.fill)
        .stroke(UiStroke::new(1.0, role_wash(tone, 86)))
        .rounding(egui::Rounding::same(3.0))
        .inner_margin(egui::Margin::symmetric(9.0, 7.0))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.0).max(80.0));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .monospace()
                        .size(12.5)
                        .color(k.ink),
                )
                .wrap(),
            );
        });
}

/// Source gets the same green/blue syntax roles as every editable Rebis surface,
/// even when it is shown read-only inside a run.
fn paint_rebis_panel(ui: &mut egui::Ui, source: &str, k: Ink) {
    egui::Frame::none()
        .fill(k.fill)
        .stroke(UiStroke::new(1.0, role_wash(k.secondary, 86)))
        .rounding(egui::Rounding::same(3.0))
        .inner_margin(egui::Margin::symmetric(9.0, 7.0))
        .show(ui, |ui| {
            ui.set_min_width((ui.available_width() - 1.0).max(80.0));
            let mut job = rebis_layout_job(source, k, 12.5, &|_| false, None);
            job.wrap.max_width = ui.available_width();
            ui.add(egui::Label::new(job).wrap());
        });
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum ChatBlock {
    Prose(String),
    Code { language: String, text: String },
}

/// Split only fenced code from prose. It is intentionally a tiny renderer,
/// not a Markdown parser: headings, lists, and quotes are styled below, while
/// everything else remains exactly the text the model returned.
fn chat_blocks(text: &str) -> Vec<ChatBlock> {
    let mut blocks = Vec::new();
    let mut prose = Vec::new();
    let mut code = Vec::new();
    let mut language = String::new();
    let mut in_code = false;

    for line in text.lines() {
        let trimmed = line.trim_start();
        if let Some(after) = trimmed.strip_prefix("```") {
            if in_code {
                blocks.push(ChatBlock::Code {
                    language: std::mem::take(&mut language),
                    text: code.join("\n"),
                });
                code.clear();
            } else {
                if !prose.is_empty() {
                    blocks.push(ChatBlock::Prose(prose.join("\n")));
                    prose.clear();
                }
                language = after.trim().to_string();
            }
            in_code = !in_code;
        } else if in_code {
            code.push(line);
        } else {
            prose.push(line);
        }
    }
    if in_code {
        blocks.push(ChatBlock::Code {
            language,
            text: code.join("\n"),
        });
    } else if !prose.is_empty() {
        blocks.push(ChatBlock::Prose(prose.join("\n")));
    }
    if blocks.is_empty() {
        blocks.push(ChatBlock::Prose(String::new()));
    }
    blocks
}

fn paint_chat_prose(ui: &mut egui::Ui, prose: &str, k: Ink) {
    ui.spacing_mut().item_spacing.y = 2.0;
    for line in prose.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            ui.add_space(5.0);
            continue;
        }
        let heading_marks = trimmed.chars().take_while(|c| *c == '#').count();
        if (1..=3).contains(&heading_marks)
            && trimmed
                .chars()
                .nth(heading_marks)
                .is_some_and(char::is_whitespace)
        {
            let heading = trimmed[heading_marks..].trim_start();
            ui.add(
                egui::Label::new(
                    egui::RichText::new(heading)
                        .strong()
                        .size(16.0 - heading_marks as f32)
                        .color(k.accent),
                )
                .wrap(),
            );
        } else if let Some(item) = trimmed
            .strip_prefix("- ")
            .or_else(|| trimmed.strip_prefix("* "))
        {
            ui.horizontal_top(|ui| {
                ui.colored_label(k.secondary, "◆");
                ui.add(egui::Label::new(egui::RichText::new(item).size(13.5).color(k.ink)).wrap());
            });
        } else if let Some(quote) = trimmed.strip_prefix("> ") {
            ui.horizontal_top(|ui| {
                ui.colored_label(k.secondary, "│");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(quote)
                            .italics()
                            .size(13.5)
                            .color(k.faint),
                    )
                    .wrap(),
                );
            });
        } else {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(line)
                        .size(13.5)
                        .line_height(Some(18.0))
                        .color(k.ink),
                )
                .wrap(),
            );
        }
    }
}

fn paint_chat_text(ui: &mut egui::Ui, text: &str, k: Ink) {
    for block in chat_blocks(text) {
        match block {
            ChatBlock::Prose(prose) => paint_chat_prose(ui, &prose, k),
            ChatBlock::Code { language, text } => {
                if !language.is_empty() {
                    ui.label(
                        egui::RichText::new(language.to_ascii_uppercase())
                            .monospace()
                            .strong()
                            .size(10.5)
                            .color(k.secondary),
                    );
                }
                paint_code_panel(ui, &text, k, k.secondary);
            }
        }
    }
}

fn paint_chat_turn(ui: &mut egui::Ui, role: kaos_core::sessions::Role, text: &str, k: Ink) {
    let (label, marker, tone) = match role {
        kaos_core::sessions::Role::User => ("YOU", "●", k.accent),
        kaos_core::sessions::Role::Model => ("MODEL", "◇", k.secondary),
    };
    let width = ui.available_width();
    egui::Frame::none()
        .fill(role_wash(tone, 13))
        .stroke(UiStroke::new(1.0, role_wash(tone, 72)))
        .rounding(egui::Rounding::same(4.0))
        .inner_margin(egui::Margin::symmetric(11.0, 9.0))
        .show(ui, |ui| {
            ui.set_min_width((width - 24.0).max(120.0));
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(marker).monospace().strong().color(tone));
                ui.label(
                    egui::RichText::new(label)
                        .monospace()
                        .strong()
                        .size(11.0)
                        .color(tone),
                );
            });
            ui.add_space(2.0);
            paint_chat_text(ui, text, k);
        });
    ui.add_space(4.0);
}

fn run_state_tone(state: runs::State, paused: bool, k: Ink) -> Color32 {
    match (state, paused) {
        (runs::State::Complete, _) | (runs::State::Running, false) => k.accent,
        (runs::State::AwaitingPermission, _) | (runs::State::Running, true) => k.secondary,
        (runs::State::Cancelled, _) => k.danger,
        // Its own colour, and deliberately neither of its neighbours': not the
        // accent a completed run gets, because nothing was shipped, and not the
        // danger a cancelled one gets, because nothing went wrong.
        (runs::State::Abstained, _) => k.secondary,
        (runs::State::Queued, _) => k.faint,
    }
}

fn run_header_job(run: &runs::Run, expanded: bool, k: Ink) -> egui::text::LayoutJob {
    let marker = if expanded { "▾" } else { "▸" };
    let lane = if run.parallel() { "∥" } else { "│" };
    let mut job = egui::text::LayoutJob::default();
    let format = |color| egui::TextFormat {
        font_id: FontId::monospace(12.5),
        color,
        ..egui::TextFormat::default()
    };
    job.append(
        &format!("{marker} {lane} #{:<3} ", run.id),
        0.0,
        format(k.faint),
    );
    job.append(
        &format!("{:<10}", run.state.label(run.paused)),
        0.0,
        format(run_state_tone(run.state, run.paused, k)),
    );
    job.append(
        &format!(
            "  {}  {:<7} {:<6}  ",
            run.timer(),
            run.scope.label(),
            run.mode.label()
        ),
        0.0,
        format(k.faint),
    );
    job.append(&run.preview(), 0.0, format(k.ink));
    job
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
        // Torn-off windows are drawn every frame but repaint on their own
        // clock; this call is what keeps them declared, and takes back the ones
        // the user closed.
        self.torn_windows(ctx);
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
                Some(Pane::Ink(_)) => self.ink_tab(ui),
                Some(Pane::Sigils(_)) => self.sigils(ui),
                Some(Pane::Automata(_)) => self.automata_tab(ui),
                Some(Pane::Music(_)) => self.music_tab(ui),
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
            // Wrapped: the header carries a dense row of controls, and a narrow
            // window used to cut the last of them off the right edge.
            ui.horizontal_wrapped(|ui| {
                ui.colored_label(self.ink.accent, "KAOS VISUAL");
                let active = self.runs.active_count() + self.actions.active_count();
                if active > 0 {
                    let mut stop = false;
                    if ui
                        .button(egui::RichText::new("stop all").color(self.ink.danger))
                        .on_hover_text("cancel every active model or tool process")
                        .clicked()
                    {
                        stop = true;
                    }
                    ui.colored_label(self.ink.secondary, format!("{active} active"));
                    if stop {
                        self.stop_all_work();
                    }
                }
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
                        CanvasMode::Planar => self.doc().view.zoom,
                        CanvasMode::Spatial => f64::from(self.doc().camera.zoom),
                    };
                    ui.colored_label(self.ink.faint, zoom_label(zoom));
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
                        let can_export = self.on_mandala() && !self.doc().mandala.is_empty();
                        if ui
                            .add_enabled(can_export, egui::Button::new("export PDF"))
                            .on_hover_text(
                                "save the complete planar mandala as a one-page vector PDF",
                            )
                            .clicked()
                        {
                            self.export_mandala_pdf();
                        }
                        if self.on_mandala() && ui.button("reset view").clicked() {
                            let doc = self.doc_mut();
                            match doc.canvas_mode {
                                CanvasMode::Planar => doc.show_everything(),
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

    fn export_mandala_pdf(&mut self) {
        let selected = rfd::FileDialog::new()
            .set_title("Export mandala to PDF")
            .set_directory(&self.cwd)
            .set_file_name("mandala.pdf")
            .add_filter("PDF document", &["pdf"])
            .save_file();
        let Some(mut path) = selected else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("pdf");
        }
        let result = {
            let doc = self.doc();
            pdf::save(&doc.mandala, &doc.squared, self.ink, &path)
        };
        self.notice = Some(match result {
            Ok(()) => format!("exported {}", path.display()),
            Err(error) => error,
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
                self.tabs.open(
                    format!("generation · {origin}"),
                    Pane::Automata(Box::new(pane)),
                );
            }
            Err(error) => {
                self.notice = Some(format!(
                    "that program does not parse, so it has no lattice: {error}"
                ));
            }
        }
    }

    /// Open a Sound tab on whatever program the active tab is holding.
    ///
    /// The drawing or the source, and only then the selected run — the opposite
    /// preference to the generation, and for a plain reason: a generation is
    /// built out of a run's *answers*, while the music is a reading of the
    /// program's own text. What is on screen is what should sound.
    fn open_music(&mut self) {
        let from = self.tabs.active_id();
        let program = match self.tabs.active() {
            Some(Pane::Source(pane)) => {
                Some((pane.editor.source().to_string(), "source".to_string(), from))
            }
            Some(Pane::Mandala(_)) => match self.doc().mandala.to_rebis() {
                Ok(text) => Some((text, "drawing".to_string(), from)),
                Err(error) => {
                    self.notice = Some(error.to_string());
                    return;
                }
            },
            _ => self
                .runs
                .selected
                .and_then(|id| self.runs.runs.iter().find(|run| run.id == id))
                .map(|run| (run.source.clone(), format!("run #{}", run.id), None)),
        };
        let Some((source, origin, from)) = program else {
            self.notice = Some(
                "open a drawing or a source tab, or select a run — sound is a reading of a \
                 program"
                    .to_string(),
            );
            return;
        };
        match music::MusicPane::from_source(&source, origin.clone(), from) {
            Ok(pane) => {
                self.tabs
                    .open(format!("sound · {origin}"), Pane::Music(Box::new(pane)));
            }
            Err(error) => {
                self.notice = Some(format!(
                    "that program does not parse, so it has no sound: {error}"
                ));
            }
        }
    }

    /// Open a Sound tab directly from a source surface, the way the `sound`
    /// button beside the source does — that button must hear *that* source,
    /// whatever else is selected elsewhere.
    fn open_music_from_source(&mut self, source: String, from: Option<TabId>) {
        match music::MusicPane::from_source(&source, "source".to_string(), from) {
            Ok(pane) => {
                self.tabs
                    .open("sound · source", Pane::Music(Box::new(pane)));
            }
            Err(error) => {
                self.set_source_notice(format!("that program has no sound: {error}"));
            }
        }
    }

    /// The Sound tab. The pane draws itself; this hands it the two things it
    /// cannot reach from inside a tab — the program in another tab, and a file
    /// dialog.
    fn music_tab(&mut self, ui: &mut egui::Ui) {
        self.music_tab_with_controls(ui, true);
    }

    /// Draw sound in a detached editor. Its original source tab belongs to the
    /// main editor, so re-read would otherwise resolve a stale numeric tab id.
    /// Export is a file dialog owned by the main window as well; play, tuning,
    /// looping, and the score remain available here.
    fn detached_music_tab(&mut self, ui: &mut egui::Ui) {
        self.music_tab_with_controls(ui, false);
    }

    fn music_tab_with_controls(&mut self, ui: &mut egui::Ui, full: bool) {
        let ink = self.ink;
        let Some(Pane::Music(pane)) = self.tabs.active_mut() else {
            return;
        };
        let reaction = music::tab(ui, pane, &ink, full);
        if full && reaction.reread {
            self.reread_music();
        }
        if full && reaction.export {
            self.export_music();
        }
    }

    /// Take the program again from the tab this one was opened on.
    fn reread_music(&mut self) {
        let from = match self.tabs.active() {
            Some(Pane::Music(pane)) => pane.from,
            _ => None,
        };
        let program = match from.and_then(|id| self.tabs.get(id)) {
            Some(Pane::Source(pane)) => Ok(pane.editor.source().to_string()),
            Some(Pane::Mandala(doc)) => doc.mandala.to_rebis().map_err(|error| error.to_string()),
            _ => Err("the tab this sound came from is gone".to_string()),
        };
        let Some(Pane::Music(pane)) = self.tabs.active_mut() else {
            return;
        };
        match program.and_then(|source| pane.desk.read(&source)) {
            Ok(()) => {}
            Err(error) => pane.desk.message = error,
        }
    }

    /// Write the piece out as a WAV file.
    fn export_music(&mut self) {
        let selected = rfd::FileDialog::new()
            .set_title("Export sound to WAV")
            .set_directory(&self.cwd)
            .set_file_name("mandala.wav")
            .add_filter("WAV audio", &["wav"])
            .save_file();
        let Some(mut path) = selected else {
            return;
        };
        if path.extension().is_none() {
            path.set_extension("wav");
        }
        let Some(Pane::Music(pane)) = self.tabs.active_mut() else {
            return;
        };
        if let Err(error) = pane.desk.export(&path) {
            pane.desk.message = error;
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
                ui.colored_label(k.danger, format!("{dead} cells killed by a refusal"));
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
            // Tabs are opened by runs, chats, source projections, and tools, so
            // a real session can outgrow the window quickly. Keep the whole
            // command row in one horizontal scroller: every tab and every
            // action remains reachable instead of the right side disappearing
            // past the panel edge. New tabs append at the visible end until the
            // user drags the strip elsewhere.
            egui::ScrollArea::horizontal()
                .id_salt("tabs_scroll")
                .auto_shrink([false, true])
                .stick_to_right(true)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let active = self.tabs.active_id();
                        let mut select: Option<TabId> = None;
                        let mut close: Option<TabId> = None;
                        let mut reorder: Option<(TabId, usize)> = None;
                        let mut settings = false;
                        let mut runs = false;
                        let mut actions = false;
                        let mut generation = false;
                        let mut music = false;
                        let mut tear: Option<TabId> = None;
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
                            // The tear-off action is available for every tab kind: the
                            // detached window carries a private editor state for it.
                            if on
                                && can_tear(&tab.content)
                                && ui
                                    .small_button("⇱")
                                    .on_hover_text("open this tab in a window of its own")
                                    .clicked()
                            {
                                tear = Some(tab.id);
                            }
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
                        if ui.button("sigils · drawn").clicked() {
                            self.tabs
                                .open("sigils · drawn", Pane::Ink(InkPane::default()));
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
                        if ui
                            .small_button("+ sound")
                            .on_hover_text(
                                "hear the program: time is indentation, every word is a Fibonacci \
                         number, and its decomposition is the tone",
                            )
                            .clicked()
                        {
                            music = true;
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
                        if let Some(id) = tear {
                            self.tear_off(id);
                        }
                        if generation {
                            self.open_generation();
                        }
                        if music {
                            self.open_music();
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
        });
    }

    /// Pull a tab out into a window of its own.
    ///
    /// The pane MOVES: a view cannot be in two places, and a copy would be two
    /// things drifting apart under one name. Closing the window puts it back on
    /// the bar, so tearing off is a rearrangement rather than a decision.
    fn tear_off(&mut self, id: TabId) {
        let Some(pane) = self.tabs.close(id) else {
            return;
        };
        let torn = torn::TornPane::Editor(Box::new(Self::detached(
            pane,
            self.ink,
            self.cwd.clone(),
            &self.runs,
            &self.actions,
        )));
        self.torn_sequence += 1;
        self.torn.push(torn::Torn::new(torn, self.torn_sequence));
        self.notice =
            Some("torn off into its own window · closing it brings the tab back".to_string());
    }

    /// Draw every torn-off window, and take back the ones that were closed.
    fn torn_windows(&mut self, ctx: &egui::Context) {
        for window in &self.torn {
            window.show(ctx, self.ink);
        }
        // A closed window's pane returns to the bar. Doing it here rather than
        // in the window's own callback is what keeps the tab list owned by one
        // thing: that callback does not run inside this frame.
        let mut returning = Vec::new();
        let mut index = 0;
        while index < self.torn.len() {
            if self.torn[index].closed() {
                returning.push(self.torn.remove(index));
            } else {
                index += 1;
            }
        }
        for window in returning {
            let Some(pane) = window.reclaim() else {
                continue;
            };
            match pane {
                torn::TornPane::Editor(editor) => {
                    for pane in (*editor).reclaim_detached() {
                        let title = pane_title(&pane);
                        self.tabs.open(title, pane);
                    }
                }
            }
        }
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
        let mut refresh_models = false;
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
        let model_choices = pane.model_choices.clone();

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.colored_label(k.accent, "SETTINGS");
            ui.colored_label(k.faint, kaos_core::config::path().display().to_string());
            let dirty = pane.dirty();
            if dirty > 0 {
                ui.colored_label(k.secondary, format!("{dirty} unsaved"));
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
                                    } else if key == "KAOS_MODEL" {
                                        let value = pane.values.entry(key.to_string()).or_default();
                                        ui.add(
                                            egui::TextEdit::singleline(value)
                                                .desired_width(300.0)
                                                .hint_text("ollama:qwen3:4b or openrouter:vendor/model"),
                                        );
                                        if ui
                                            .small_button("refresh Ollama")
                                            .on_hover_text("re-run `ollama ls` and update the suggestions")
                                            .clicked()
                                        {
                                            refresh_models = true;
                                        }
                                        let query = value.trim().to_ascii_lowercase();
                                        egui::ComboBox::from_id_salt(("setting-model-picker", key))
                                            .selected_text("suggest model")
                                            .width(220.0)
                                            .show_ui(ui, |ui| {
                                                let mut shown = 0;
                                                for choice in model_choices.iter().filter(|choice| {
                                                    query.is_empty()
                                                        || choice.to_ascii_lowercase().contains(&query)
                                                }) {
                                                    shown += 1;
                                                    ui.selectable_value(
                                                        value,
                                                        choice.clone(),
                                                        choice,
                                                    );
                                                }
                                                if shown == 0 {
                                                    ui.colored_label(k.faint, "no matching model");
                                                }
                                            });
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
        if refresh_models {
            pane.refresh_models();
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
        let mut submission: Option<ChatSubmission> = None;
        let mut stop_chat = false;
        let session_id = match self.tabs.active() {
            Some(Pane::Chat(chat)) => Some(chat.session.id.clone()),
            _ => None,
        };
        let run_id = self.tabs.active().and_then(|pane| match pane {
            Pane::Chat(chat) if !chat.browsing => chat.run_id,
            _ => None,
        });
        let retained_run = run_id.and_then(|id| self.runs.runs.iter().find(|run| run.id == id));
        let chat_busy = session_id
            .as_deref()
            .is_some_and(|id| self.actions.session_active(id));
        // What the turn in flight has written so far, if there is one.
        let streaming = session_id
            .as_deref()
            .and_then(|id| self.actions.session_stream(id));
        // Copied out before the pane borrow and written back after it, because
        // the toggle sits in the same row as the input and `chat` holds
        // `self.tabs` for the whole of it.
        let mut chaos = self.chaos;
        let Some(Pane::Chat(chat)) = self.tabs.active_mut() else {
            return;
        };

        if chat.browsing {
            let store = kaos_core::sessions::Store::default_store();
            let list = store.list();
            let mut forget = None;
            ui.add_space(8.0);
            ui.colored_label(k.faint, "SESSIONS");
            if chat_busy
                && ui
                    .button(egui::RichText::new("stop model").color(k.danger))
                    .on_hover_text("cancel this conversation's active model turn")
                    .clicked()
            {
                stop_chat = true;
            }
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
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!("{:>3}", s.turns))
                                    .monospace()
                                    .strong()
                                    .color(k.accent),
                            );
                            ui.label(
                                egui::RichText::new("turns")
                                    .monospace()
                                    .size(11.0)
                                    .color(k.faint),
                            );
                            if ui
                                .selectable_label(false, egui::RichText::new(&s.title).color(k.ink))
                                .clicked()
                            {
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
                ui.label(
                    egui::RichText::new(chat.session.title())
                        .strong()
                        .size(15.0)
                        .color(k.ink),
                );
                ui.label(
                    egui::RichText::new(format!("◇ {}", chat.session.model))
                        .monospace()
                        .size(11.0)
                        .color(k.secondary),
                );
                if let Some(run_id) = chat.run_id {
                    ui.colored_label(
                        k.accent,
                        format!("run #{run_id} · retained content shown below · refreshed live"),
                    );
                }
                if chat_busy {
                    ui.colored_label(k.secondary, "◇ model working");
                    ui.colored_label(k.faint, "· keep typing, it goes out next");
                    if ui
                        .button(egui::RichText::new("stop model").color(k.danger))
                        .on_hover_text("cancel this conversation's active model turn")
                        .clicked()
                    {
                        stop_chat = true;
                    }
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
                    if let Some(run) = retained_run {
                        egui::Frame::none()
                            .fill(role_wash(k.accent, 9))
                            .stroke(UiStroke::new(1.0, role_wash(k.accent, 64)))
                            .rounding(egui::Rounding::same(4.0))
                            .inner_margin(egui::Margin::symmetric(11.0, 9.0))
                            .show(ui, |ui| {
                                ui.horizontal_wrapped(|ui| {
                                    ui.label(
                                        egui::RichText::new("● RUN")
                                            .monospace()
                                            .strong()
                                            .size(11.0)
                                            .color(run_state_tone(run.state, run.paused, k)),
                                    );
                                    ui.label(
                                        egui::RichText::new(format!(
                                            "#{} · {} · {} · {} · {}",
                                            run.id,
                                            run.state.label(run.paused),
                                            run.scope.label(),
                                            run.mode.label(),
                                            run.timer()
                                        ))
                                        .monospace()
                                        .color(k.ink),
                                    );
                                });
                                egui::CollapsingHeader::new(
                                    egui::RichText::new("SOURCE").monospace().color(k.secondary),
                                )
                                .id_salt(("chat_run_source", run.id))
                                .show(ui, |ui| paint_rebis_panel(ui, &run.source, k));
                                if !run.input.is_empty() {
                                    egui::CollapsingHeader::new(
                                        egui::RichText::new("RECORD / INPUT")
                                            .monospace()
                                            .color(k.accent),
                                    )
                                    .id_salt(("chat_run_input", run.id))
                                    .show(ui, |ui| paint_code_panel(ui, &run.input, k, k.accent));
                                }
                                let hidden = run.output.len().saturating_sub(500);
                                if hidden > 0 {
                                    ui.colored_label(
                                        k.faint,
                                        format!("… {hidden} older retained output lines hidden"),
                                    );
                                }
                                paint_stream_sections(
                                    ui,
                                    &run.output.as_slice()[hidden..],
                                    k,
                                    &format!("chat-run-output-{}", run.id),
                                );
                            });
                        ui.add_space(7.0);
                    }
                    for turn in &chat.session.turns {
                        paint_chat_turn(ui, turn.role, &turn.text, k);
                    }
                    // The turn in flight, as it arrives. A chat is a child
                    // process streaming into a retained log, so the answer
                    // exists long before it is delivered; watching a still
                    // window until it finished was hiding work already done.
                    if let Some((lines, timer)) = streaming.as_ref() {
                        ui.add_space(7.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(k.secondary, "◇ MODEL");
                            ui.colored_label(k.faint, format!("working · {timer}"));
                        });
                        if lines.is_empty() {
                            ui.colored_label(k.faint, "…");
                        }
                        paint_stream_sections(ui, lines, k, "chat-live-stream");
                    }
                    for waiting in &chat.queued {
                        ui.add_space(5.0);
                        ui.horizontal(|ui| {
                            ui.colored_label(k.faint, "● YOU");
                            ui.colored_label(k.faint, "queued");
                        });
                        ui.colored_label(k.faint, waiting);
                    }
                });
        }

        ui.separator();
        ui.horizontal(|ui| {
            ui.checkbox(&mut chaos, "chaos")
                .on_hover_text(kaos_core::chaos::CHAOS_HINT);
            let send = ui
                .add(
                    egui::TextEdit::singleline(&mut chat.input)
                        .desired_width((ui.available_width() - 70.0).max(80.0))
                        .hint_text(if chat_busy {
                            "say something — it queues behind the answer in progress"
                        } else if chaos {
                            "state an intent — it becomes a Rebis program"
                        } else {
                            "say something"
                        }),
                )
                .lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (send || ui.button("send").clicked()) && !chat.input.trim().is_empty() {
                chat.browsing = false;
                let said = std::mem::take(&mut chat.input);
                if chat_busy {
                    // The turn in flight keeps going; this rides behind it and
                    // goes out the moment it lands.
                    chat.queued.push(said);
                    return;
                }
                let resume = chat
                    .session
                    .turns
                    .iter()
                    .any(|turn| turn.role == kaos_core::sessions::Role::Model);
                chat.session
                    .push(kaos_core::sessions::Role::User, said.clone());
                let prior_turns = chat.session.turns.len().saturating_sub(1);
                let history = recent_chat_history(&chat.session.turns[..prior_turns]);
                let run_id = chat.run_id;
                // Persist immediately: the terminal app saves on every turn for
                // the same reason, so a crash loses nothing already said.
                let _ = kaos_core::sessions::Store::default_store().save(&chat.session);
                submission = Some(ChatSubmission {
                    said,
                    session: chat.session.id.clone(),
                    resume,
                    run_id,
                    history,
                });
            }
        });
        let _ = chat;
        self.chaos = chaos;
        if stop_chat {
            if let Some(session) = session_id.as_deref() {
                let cancelled = self.actions.cancel_session(session, &self.cwd);
                if let Some(Pane::Chat(chat)) = self.tabs.active_mut() {
                    let queued = chat.queued.len();
                    chat.queued.clear();
                    chat.notice = Some(if cancelled {
                        if queued == 0 {
                            "model turn stopped".to_string()
                        } else {
                            format!("model turn stopped · {queued} queued message(s) cleared")
                        }
                    } else {
                        "queued chat messages cleared".to_string()
                    });
                }
            }
        }
        if let Some(ChatSubmission {
            said,
            session,
            resume,
            run_id,
            history,
        }) = submission
        {
            let history_text = history
                .iter()
                .map(|(user, assistant)| format!("USER: {user}\nASSISTANT: {assistant}"))
                .collect::<Vec<_>>()
                .join("\n\n");
            let prompt = run_id
                .and_then(|id| self.run_chat_context(id, &history, &said))
                .unwrap_or_else(|| {
                    kaos_core::chat::DEFAULT_CONTEXT.render_turn(self.chaos, &history_text, &said)
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
            output: run.output.as_slice(),
        };
        Some(render(&snapshot))
    }

    #[cfg(test)]
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
        let chat = ChatPane {
            browsing: false,
            run_id: Some(id),
            notice: Some(
                "each message refreshes source, state, and the complete retained run output"
                    .to_string(),
            ),
            ..ChatPane::default()
        };
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
                                self.tabs.open(name, Pane::Mandala(Doc::drawn(mandala)));
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
            if ui
                .button("sound")
                .on_hover_text("hear this program: indentation is time, its words are the tones")
                .clicked()
            {
                actions.push(SourceAction::Sound(pane.editor.source().to_string()));
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
        // Which tab these actions came from, so a Sound tab opened here knows
        // where to re-read its program from later.
        let from = self.tabs.active_id();
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
                        self.tabs
                            .open("mandala", Pane::Mandala(Doc::drawn(mandala)));
                    }
                    Err(error) => self.set_source_notice(error.to_string()),
                },
                SourceAction::Generation(text) => {
                    self.open_generation_from_source(text);
                }
                SourceAction::Sound(text) => {
                    self.open_music_from_source(text, from);
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
                // One scroll area over the whole palette. Reserving a fixed
                // slice for the tools below and scrolling only the forms meant a
                // short window squeezed that slice until `connect flow`, `select`
                // and `drag` were off the bottom with no way to reach them.
                // Everything scrolls together, so nothing is ever unreachable.
                egui::ScrollArea::vertical()
                    .id_salt("palette_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        ui.colored_label(self.ink.faint, "FORMS");
                        for (label, make, _) in Form::ALL {
                            let form = make();
                            // Flow is an operator between two indentation
                            // blocks, so it is made by the two-click link tool,
                            // never left as a disconnected arrow glyph. A prefix
                            // sigil is a mark on a form rather than a form, and
                            // has its own section below.
                            if form.is_flow() || form.is_prefix_sigil() {
                                continue;
                            }
                            let on = self.tool == Tool::Place(form.clone());
                            if ui.selectable_label(on, *label).clicked() {
                                self.tool = Tool::Place(form.clone());
                                self.doc_mut().pending = None;
                            }
                        }
                        ui.colored_label(
                            self.ink.faint,
                            "□ and ○ are indentation; dropping inside writes ordered nesting",
                        );
                        ui.add_space(6.0);
                        // `'` and `,` are written on the front of a form, not
                        // dropped somewhere of their own. Listing them beside
                        // the shapes invited placing one on bare canvas, where
                        // it became a sigil marking nothing.
                        ui.colored_label(self.ink.faint, "MARKS");
                        for (label, make, _) in Form::ALL {
                            let form = make();
                            if !form.is_prefix_sigil() {
                                continue;
                            }
                            let on = self.tool == Tool::Place(form.clone());
                            if ui.selectable_label(on, *label).clicked() {
                                self.tool = Tool::Place(form.clone());
                                self.doc_mut().pending = None;
                            }
                        }
                        ui.colored_label(
                            self.ink.faint,
                            "click a form to write the mark on it; click again to take it off",
                        );
                        ui.separator();
                        ui.colored_label(self.ink.faint, "LINK");
                        ui.colored_label(self.ink.faint, "flow operators connect □ / ○ blocks");
                        ui.colored_label(self.ink.faint, "drag moves the view, over a form or not");
                        for (label, color, tool) in [
                            // One two-click shortcut. `(<- a b)` is `(-> b a)`,
                            // so the direction you see is the direction in the
                            // source.
                            (
                                "→  connect flow",
                                self.ink.secondary,
                                Tool::Flow(Form::Forward),
                            ),
                            ("▹  select", self.ink.ink, Tool::Select),
                            ("↔↕ drag", self.ink.ink, Tool::Pan),
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
            });
    }

    /// Keep the drawing and its source in step.
    ///
    /// Whichever side changed wins: a canvas edit regenerates the text, and
    /// text that parses replaces the drawing. Text that does not parse is left
    /// alone — you are mid-sentence, and throwing the buffer away would be the
    /// wrong response to an incomplete one.
    fn sync(&mut self) {
        // The PAGE, not the single expression: a drawing is several roots for
        // the whole of the gesture that joins them, and the strict reading left
        // the panel showing the last thing that happened to parse — which read
        // as the source box being broken rather than as the drawing being
        // half-made. See [`Mandala::to_rebis_page`].
        let fresh = self.doc().mandala.to_rebis_page();
        let note = fresh.as_ref().err().map(ToString::to_string);
        let fresh = fresh.ok();
        let doc = self.doc_mut();
        doc.source_note = note;
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
            doc.generated = doc.mandala.to_rebis_page().unwrap_or_default();
            doc.reset_interaction();
            // Source adopted from the panel is a fresh layout, so frame it.
            doc.show_everything();
        }
    }

    /// Show and edit the stages of the arrow chain the selected form belongs to.
    ///
    /// A run of arrows is one figure spread over several `->` nodes, so the panel
    /// lists it as one ordered thing — the same numbered rows the children get, on
    /// the same principle: what the drawing shows as an order should be editable as
    /// an order rather than by rewriting the source.
    ///
    /// Nothing is shown for a form outside any chain, so the section appears exactly
    /// when there is a chain to reorder.
    fn arrow_order_editor(&mut self, ui: &mut egui::Ui, id: NodeId, editable: bool) {
        let Some(root) = self.doc().mandala.flow_chain_root(id) else {
            return;
        };
        let stages = self.doc().mandala.flow_stages(root);
        if stages.len() < 2 {
            return;
        }
        let backflow = self
            .doc()
            .mandala
            .node(root)
            .is_some_and(|node| node.form == Form::Backflow);
        ui.add_space(4.0);
        ui.colored_label(
            self.ink.mid,
            format!("ARROW CHAIN · {} stages · 1–n", stages.len()),
        );
        ui.colored_label(
            self.ink.faint,
            if backflow {
                "written order; a backflow reads right to left"
            } else {
                "each stage's answer becomes the next one's input"
            },
        );
        let mut requested = None;
        for (index, stage) in stages.iter().copied().enumerate() {
            let caption = self
                .doc()
                .mandala
                .node(stage)
                .map(|node| {
                    let caption = node.caption();
                    if caption.is_empty() {
                        node.form.name().to_string()
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
                        egui::DragValue::new(&mut number).range(1..=stages.len()),
                    )
                    .changed()
                {
                    requested = Some((stage, number));
                }
                // The arrow between the stages, drawn in the list as it is on the
                // canvas, so the order reads as a chain rather than as a numbered set.
                ui.colored_label(
                    if stage == id {
                        self.ink.accent
                    } else {
                        self.ink.ink
                    },
                    caption,
                );
                if index + 1 < stages.len() {
                    ui.colored_label(self.ink.faint, if backflow { "←" } else { "→" });
                }
            });
        }
        if let Some((stage, number)) = requested {
            let doc = self.doc_mut();
            doc.checkpoint();
            if !doc.mandala.set_flow_stage_number(stage, number) {
                doc.undo();
            }
        }
    }

    /// What one child row says: the child's name, and what stands inside it.
    ///
    /// A form's word alone does not distinguish one child from the next — a
    /// parent holding three macros listed `macro`, `macro`, `macro`, and the
    /// numbers beside them could then only be permuted blind. So the row carries
    /// the two things that identify a child: what it is called — a macro's name
    /// and parameters, a call's callee, a symbol — and, since an indentation
    /// writes nothing inside itself, the first of what it holds.
    ///
    /// The name is always shown; the second half is what the unfold control
    /// reveals in full, because that is the part that can run long.
    fn child_row(&self, id: NodeId) -> (String, String) {
        let mandala = &self.doc().mandala;
        let Some(child) = mandala.node(id) else {
            return ("missing".to_string(), String::new());
        };
        let mut name = child.form.name().to_string();
        // A macro is `(~ f (p …) body)`: its name and its parameters are what
        // one macro has and the next does not.
        if let Form::Function(params) = &child.form {
            if !child.text.trim().is_empty() {
                name.push(' ');
                name.push_str(child.text.trim());
            }
            if !params.is_empty() {
                name.push_str(&format!(" ({})", params.join(" ")));
            }
        } else if child.form.uses_text() && !matches!(child.form, Form::Prompt) {
            // A callee, a module path, a port, a symbol: short, and the whole
            // of the identity, so it belongs with the word rather than after it.
            if !child.text.trim().is_empty() {
                name.push(' ');
                name.push_str(child.text.trim());
            }
        }
        // A prompt's payload is its text; an indentation's is what it indents.
        let inside = if matches!(child.form, Form::Prompt) {
            child.text.trim().to_string()
        } else {
            mandala
                .children(id)
                .iter()
                .filter_map(|kid| mandala.node(*kid))
                .map(preview_token)
                .collect::<Vec<_>>()
                .join(" · ")
        };
        (name, inside)
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
        ui.colored_label(self.ink.mid, "CHILD ORDER · 1–n");
        ui.colored_label(
            self.ink.faint,
            "link order is the default; numbers are configurable",
        );
        let mut requested = None;
        let mut toggled = None;
        for (index, child_id) in children.iter().copied().enumerate() {
            // The full text and the token stand side by side: the row shows the
            // token, and the button is offered only when there is more to see, so
            // a short caption is not decorated with a control that does nothing.
            let (name, full) = self.child_row(child_id);
            let short = truncate_chars(&full, 22);
            let has_more = short != full;
            let open = self.unfolded.contains(&child_id);
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
                if has_more {
                    let arrow = if open { "⏷" } else { "⏵" };
                    if ui
                        .small_button(arrow)
                        .on_hover_text(if open {
                            "show the first token only"
                        } else {
                            "show the whole text"
                        })
                        .clicked()
                    {
                        toggled = Some(child_id);
                    }
                }
                ui.colored_label(self.ink.ink, &name);
                if !full.is_empty() {
                    ui.colored_label(
                        self.ink.secondary,
                        if open { full.clone() } else { short.clone() },
                    );
                }
            });
        }
        if let Some(child) = toggled {
            if !self.unfolded.remove(&child) {
                self.unfolded.insert(child);
            }
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
        let mut set_legend = None;
        let mut set_sizing = None;
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
                // One scrollbar for the whole panel. The sections stack — a
                // selection's form controls, its child order, its arrow chain, its
                // source, then the program's own source and controls — and on a short
                // window the ones at the bottom were simply unreachable. The inner
                // boxes below grow to their content instead of scrolling separately,
                // so there is exactly one thing to scroll and the wheel always means
                // the same thing.
                egui::ScrollArea::vertical()
                    .id_salt("side_panel_scroll")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        ui.add_space(6.0);
                        let selection_len = self.doc().selection_len();
                        if selection_len > 1 {
                            ui.colored_label(
                                self.ink.accent,
                                format!("{selection_len} FORMS SELECTED"),
                            );
                            ui.colored_label(
                                self.ink.faint,
                                "Ctrl-click toggles · right-drag replaces the block",
                            );
                        }
                        if let Some(id) = self.doc().primary_selected() {
                            if let Some(node) = self.doc_mut().mandala.node(id).cloned() {
                                ui.colored_label(self.ink.mid, node.form.name().to_uppercase());
                                if text_editable && node.form.uses_text() {
                                    let mut text = node.text.clone();
                                    // A tall, wrapping field inside a height-capped
                                    // scroll area: long text wraps and scrolls instead
                                    // of being clipped to one line.
                                    let response = ui.add(
                                        egui::TextEdit::multiline(&mut text)
                                            .desired_rows(4)
                                            .desired_width(f32::INFINITY),
                                    );
                                    keep_focus_over_escape(ui, response.id);
                                    let changed = response.changed();
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
                                            let ps: Vec<String> = joined
                                                .split_whitespace()
                                                .map(str::to_string)
                                                .collect();
                                            let doc = self.doc_mut();
                                            doc.checkpoint();
                                            doc.mandala.set_form(id, Form::Function(ps));
                                        }
                                        ui.colored_label(
                                            self.ink.faint,
                                            "parameters, space separated",
                                        );
                                    }
                                }
                                if text_editable {
                                    let mut model = node.model.clone().unwrap_or_default();
                                    ui.colored_label(
                                        self.ink.secondary,
                                        "MODEL OVERRIDE · optional /provider:model",
                                    );
                                    if ui
                                        .add(
                                            egui::TextEdit::singleline(&mut model)
                                                .desired_width(f32::INFINITY)
                                                .hint_text("ollama:qwen4:4b"),
                                        )
                                        .changed()
                                    {
                                        let model = model.replace(['\n', '\r'], "");
                                        let model = model.trim();
                                        let model = (!model.is_empty()).then(|| model.to_string());
                                        let doc = self.doc_mut();
                                        doc.checkpoint();
                                        doc.mandala.set_model(id, model);
                                    }
                                    ui.colored_label(
                                        self.ink.faint,
                                        "blank inherits the run default",
                                    );
                                }
                                ui.colored_label(
                                    self.ink.faint,
                                    format!("takes {} ordered children", node.form.arity()),
                                );
                                self.child_order_editor(ui, id, graph_editable);
                                self.arrow_order_editor(ui, id, graph_editable);
                                // The code of the selected block — the exact Rebis the
                                // selection generates on its own, so selecting a shape
                                // shows what that shape (and its operands) is.
                                ui.add_space(4.0);
                                ui.colored_label(
                                    self.ink.mid,
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
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&code)
                                                    .monospace()
                                                    .color(self.ink.ink),
                                            )
                                            .wrap(),
                                        );
                                        if ui.small_button("copy block").clicked() {
                                            ui.ctx().copy_text(code);
                                        }
                                    }
                                    Ok(None) => {}
                                    Err(error) => {
                                        ui.colored_label(
                                            self.ink.danger,
                                            format!("block is not one exact form: {error}"),
                                        );
                                    }
                                }
                                ui.add_space(4.0);
                                if graph_editable
                                    && ui
                                        .button(
                                            egui::RichText::new(if selection_len > 1 {
                                                "delete selection"
                                            } else {
                                                "delete shape"
                                            })
                                            .color(self.ink.danger),
                                        )
                                        .clicked()
                                {
                                    self.doc_mut().delete_selected();
                                }
                                // A mark written on this form, with the way to take it
                                // off. Clicking the same tool again also removes it, but
                                // that is only findable if you already know it; the form
                                // itself is where you look when you want it undone.
                                if graph_editable && selection_len == 1 {
                                    let marked = self
                                        .doc()
                                        .selected
                                        .map(|id| (id, self.doc().mandala.written_prefix(id)));
                                    if let Some((id, written)) =
                                        marked.filter(|(_, w)| !w.is_empty())
                                    {
                                        ui.horizontal(|ui| {
                                            ui.colored_label(self.ink.faint, "MARKED");
                                            ui.colored_label(self.ink.ink, &written);
                                            if ui.small_button("remove").clicked() {
                                                if let Some(outer) =
                                                    self.doc().mandala.father(id).filter(|f| {
                                                        self.doc().mandala.is_written_prefix(*f)
                                                    })
                                                {
                                                    self.doc_mut().checkpoint();
                                                    self.doc_mut().mandala.unwrap_form(outer);
                                                    self.doc_mut().select_only(id);
                                                }
                                            }
                                        });
                                    }
                                }
                                // Deleting a boundary takes everything inside it with
                                // it. Removing a level of nesting while keeping what it
                                // held is a different move, and had none.
                                if graph_editable
                                    && selection_len == 1
                                    && ui.button("remove this level, keep its contents").clicked()
                                {
                                    if let Some(id) = self.doc().selected {
                                        self.doc_mut().checkpoint();
                                        let kept = self.doc_mut().mandala.unwrap_form(id);
                                        self.doc_mut().reset_interaction();
                                        self.notice = Some(format!(
                                            "removed one level, kept {} form{}",
                                            kept.len(),
                                            plural(kept.len())
                                        ));
                                    }
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
                            let status_tone = if typed.trim().is_empty() {
                                k.faint
                            } else if exact.is_err() || rebis_lang::parse(&typed).is_err() {
                                k.danger
                            } else {
                                k.accent
                            };
                            ui.colored_label(status_tone, status);
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
                        // Two readings of the same drawing. Both change what the layout
                        // produces rather than only how it is painted, so both are
                        // applied by re-laying it out — and both are therefore one
                        // undoable step, like `format mandala` beside them.
                        ui.horizontal_wrapped(|ui| {
                            let mut whole = self.doc().mandala.legend() == Legend::Whole;
                            if ui
                        .add_enabled(graph_editable, egui::Checkbox::new(&mut whole, "whole text"))
                        .on_hover_text(
                            "grow every form until its whole text fits inside it, instead of \
                             writing one token and keeping the rest in this panel",
                        )
                        .changed()
                    {
                        set_legend = Some(if whole { Legend::Whole } else { Legend::Token });
                    }
                            // `normalize` is the per-level equalisation: brothers drawn
                            // at one size, each boundary grown until what it holds
                            // fills it. That is what reads as normalized on the page —
                            // a level whose forms all match, and no circle standing
                            // half empty. `Sizing::Even` is the flatter thing, one size
                            // shared by every depth; it is the default because it costs
                            // a quarter of the area, but a drawing that ignores levels
                            // is not what the word means, so it is the unticked state
                            // rather than the ticked one.
                            let mut normalized = self.doc().mandala.sizing() == Sizing::ByLevel;
                            if ui
                        .add_enabled(
                            graph_editable,
                            egui::Checkbox::new(&mut normalized, "normalize"),
                        )
                        .on_hover_text(
                            "even the sizes out level by level: brothers drawn at one size, and \
                             each boundary grown until its contents fill it. Unticked, one size \
                             holds at every depth — smaller, and readable at one magnification",
                        )
                        .changed()
                    {
                        set_sizing = Some(if normalized {
                            Sizing::ByLevel
                        } else {
                            Sizing::Even
                        });
                    }
                        });
                        let edited;
                        // Vertical only: a long line wraps onto the next row instead of
                        // running off the right edge, so there is nothing to the right to
                        // scroll to.
                        {
                            // Same colouring and the same parenthesis matching as the
                            // Source tab: this box holds Rebis too, and one without
                            // them reads as a different editor.
                            let id = egui::Id::new("side_source_edit");
                            let pair = matched_pair(ui.ctx(), id, &self.doc().text);
                            let mut layouter = |ui: &egui::Ui, text: &str, wrap: f32| {
                                ui.fonts(|fonts| {
                                    let mut job = rebis_layout_job(text, k, 12.5, &|_| false, pair);
                                    // Wrapping is the layouter's job: without this the job
                                    // reports infinite width and the box scrolls instead.
                                    job.wrap.max_width = wrap;
                                    fonts.layout_job(job)
                                })
                            };
                            let gutter = gutter_px(ui, self.doc().text.lines().count());
                            let available = ui.available_width();
                            let output = ui
                                .horizontal(|ui| {
                                    // The gutter is reserved first and painted afterwards
                                    // from the galley, so the numbers are never part of the
                                    // text and a selection cannot pick them up.
                                    let (rect, _) = ui.allocate_exact_size(
                                        egui::vec2(gutter, ui.available_height().max(1.0)),
                                        egui::Sense::hover(),
                                    );
                                    let doc = self.doc_mut();
                                    let output = egui::TextEdit::multiline(&mut doc.text)
                                        .id(id)
                                        .code_editor()
                                        .interactive(text_editable)
                                        .desired_width((available - gutter).max(80.0))
                                        .desired_rows(24)
                                        .layouter(&mut layouter)
                                        .show(ui);
                                    (rect, output)
                                })
                                .inner;
                            let (gutter_rect, output) = output;
                            edited = output.response.changed();
                            keep_focus_over_escape(ui, output.response.id);
                            paint_line_numbers(ui, k, gutter_rect, &output);
                        }
                        source_status(ui, k, &self.doc().text);
                        // When the drawing has no source at all, say which state it is
                        // in. Silence here reads as a broken panel; the reason reads as
                        // a drawing that is not finished yet.
                        if let Some(note) = self.doc().source_note.clone() {
                            ui.colored_label(
                                k.danger,
                                format!("the drawing has no source yet · {note}"),
                            );
                        }
                        // Typing redraws the canvas as soon as what you have typed is a
                        // program.
                        if edited {
                            self.adopt_text();
                        }
                    });
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
                    doc.generated = doc.mandala.to_rebis_page().unwrap_or_default();
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
        // A reading is only half applied until the drawing is re-laid out at the
        // sizes it implies: outlines resize themselves, but a form grown where
        // it stands would push through its neighbour.
        if let Some(legend) = set_legend {
            let doc = self.doc_mut();
            doc.checkpoint();
            if doc.mandala.set_legend(legend) {
                doc.mandala.relayout();
            }
            self.notice = Some(match legend {
                Legend::Whole => "every form grown to hold its whole text".to_string(),
                Legend::Token => "one token per form; the panel holds the rest".to_string(),
            });
        }
        if let Some(sizing) = set_sizing {
            let doc = self.doc_mut();
            doc.checkpoint();
            if doc.mandala.set_sizing(sizing) {
                doc.mandala.relayout();
            }
            self.notice = Some(match sizing {
                Sizing::Even => "one size at every depth — smaller, and readable at one \
                                 magnification"
                    .to_string(),
                Sizing::ByLevel => "normalized level by level — brothers at one size, each \
                                    boundary filled by what it holds"
                    .to_string(),
            });
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
            if self.runs.has_active()
                && ui
                    .button(egui::RichText::new("cancel all").color(k.danger))
                    .clicked()
            {
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
                    ui.colored_label(if valid { k.accent } else { k.danger }, diagnostic);
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
                // Ordered as a tree by the shared model, so a run sits under
                // the run that opened it here exactly as it does in the
                // terminal — one algorithm, two renderings.
                let shape = kaos_core::run_model::tree_order(
                    &self
                        .runs
                        .runs
                        .iter()
                        .map(|run| (run.id, run.lineage.parent))
                        .collect::<Vec<_>>(),
                );
                for (depth, id) in shape {
                    let Some((position, run)) = self
                        .runs
                        .runs
                        .iter()
                        .enumerate()
                        .find(|(_, run)| run.id == id)
                    else {
                        continue;
                    };
                    let chosen = self.runs.selected == Some(run.id);
                    ui.horizontal(|ui| {
                        // Indentation is the whole nesting cue: a child run is
                        // one step in from whatever opened it.
                        if depth > 0 {
                            ui.add_space(14.0 * depth as f32);
                            ui.colored_label(k.faint, "\u{2514}");
                        }
                        if ui
                            .selectable_label(chosen, run_header_job(run, run.expanded, k))
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
                            && ui
                                .small_button(egui::RichText::new("✕").color(k.danger))
                                .on_hover_text("remove run")
                                .clicked()
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
                        // The price, before it is paid. This is the payoff of
                        // the whole cost-readability design and it was the one
                        // number a person could not see: the browser showed a
                        // queued program's source and not what running it would
                        // cost. The count is exact; anything the run decides is
                        // named on hover rather than folded into the number.
                        let price = rebis_lang::parse(&run.source)
                            .ok()
                            .map(|expression| kaos_core::cost::of(&expression));
                        ui.horizontal(|ui| {
                            ui.colored_label(k.faint, format!("      queue position {queue}"));
                            if let Some(price) = price {
                                let label = ui.colored_label(
                                    if price.is_exact() {
                                        k.faint
                                    } else {
                                        k.secondary
                                    },
                                    format!("· {}", price.summary()),
                                );
                                label.on_hover_text(if price.is_exact() {
                                    format!(
                                        "{} model calls, counted off the page before launch",
                                        price.firings
                                    )
                                } else {
                                    format!(
                                        "at least {} model calls.\n{}",
                                        price.firings,
                                        price.conditional.join("\n")
                                    )
                                });
                            }
                        });
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
                                ui.colored_label(k.secondary, reason);
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
                                    if ui
                                        .button(egui::RichText::new("deny").color(k.danger))
                                        .clicked()
                                    {
                                        deny_run = Some(id);
                                    }
                                }
                                if state == runs::State::Running
                                    && ui.button(if paused { "resume" } else { "pause" }).clicked()
                                {
                                    select = Some(id);
                                    pause = true;
                                }
                                if !state.terminal()
                                    && ui
                                        .button(egui::RichText::new("cancel").color(k.danger))
                                        .clicked()
                                {
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
                            egui::CollapsingHeader::new(
                                egui::RichText::new(format!("SOURCE #{id}"))
                                    .monospace()
                                    .color(k.secondary),
                            )
                            .id_salt(("run_source", id))
                            .show(ui, |ui| paint_rebis_panel(ui, &run.source, k));
                            if !run.input.is_empty() {
                                egui::CollapsingHeader::new(
                                    egui::RichText::new(format!("RECORD / INPUT #{id}"))
                                        .monospace()
                                        .color(k.accent),
                                )
                                .id_salt(("run_input", id))
                                .show(ui, |ui| paint_code_panel(ui, &run.input, k, k.accent));
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
                            let hidden = run.output.len().saturating_sub(2_000);
                            if hidden > 0 {
                                ui.colored_label(
                                    k.faint,
                                    format!("… {hidden} older retained lines hidden in this view"),
                                );
                            }
                            paint_stream_sections(
                                ui,
                                &run.output.as_slice()[hidden..],
                                k,
                                &format!("run-output-{id}"),
                            );
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
                                    // Driven by the credential store itself, so a
                                    // provider added there appears in both front
                                    // ends rather than in whichever was edited.
                                    for (provider, _) in kaos_agent::auth::PROVIDERS {
                                        ui.selectable_value(
                                            &mut self.actions.provider,
                                            (*provider).to_string(),
                                            *provider,
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
                        let hidden = output.len().saturating_sub(2_000);
                        if hidden > 0 {
                            ui.colored_label(
                                k.faint,
                                format!("… {hidden} older retained lines hidden in this view"),
                            );
                        }
                        for line in output.iter().skip(hidden) {
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
                    "drag a piece to move · drag empty space to orbit · G/X/Y/Z constrain · wheel zoom"
                } else {
                    "drag/pan · the drag tool or middle-drag moves the view · right-drag marquee · Ctrl-click toggles · Ctrl-C/V block · Delete · Ctrl-Z"
                };
                ui.colored_label(
                    self.ink.faint,
                    hint,
                );
                if self.on_mandala() && self.doc().canvas_mode == CanvasMode::Planar {
                    if let Some(id) = self.doc().pending {
                        let message = match &self.tool {
                            Tool::Flow(_) => {
                                format!(
                                    "flow from #{} — click the destination · shift for a right-angle trace",
                                    id.0
                                )
                            }
                            Tool::Place(_) | Tool::Select | Tool::Pan => String::new(),
                        };
                        ui.colored_label(self.ink.ink, message);
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
        // A drawing is laid out before there is a window to lay it out in, so
        // the request to frame it waits here for the first frame that knows how
        // big the canvas is. An empty drawing has nothing to frame and simply
        // spends the request.
        if self.doc().fit_pending {
            let size = response.rect.size();
            if size.x > 1.0 && size.y > 1.0 {
                if let Some(bounds) = self.doc().mandala.bounds() {
                    self.doc_mut().view =
                        View::fitting(bounds, f64::from(size.x), f64::from(size.y));
                }
                self.doc_mut().fit_pending = false;
            }
        }
        // When the right-click menu is open, the click that dismisses it must
        // only close the menu — never also select, place, or move on the
        // canvas. This is true coming into the frame; egui closes the menu as
        // it processes the click, so we suppress canvas actions for it.
        let menu_open = ui.ctx().is_context_menu_open();
        // Holding space pans, whatever tool is selected — and it works by turning
        // ON the drag tool for as long as the key is down, rather than by running a
        // second pan mechanism beside it. Three earlier attempts wrote that second
        // mechanism and none of them worked; this one reuses the path the drag
        // button already proves, so there is exactly one way the view gets moved.
        //
        // Gated on nothing wanting the keyboard, or a space typed into a prompt
        // would pan the canvas underneath it.
        let (pressed, held) = ui.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_down(egui::Key::Space),
            )
        });
        if !held {
            self.space_pan = false;
        } else if pressed && !ui.ctx().wants_keyboard_input() {
            self.space_pan = true;
        }
        let space_held = self.space_pan;
        let panning = self.tool == Tool::Pan || space_held;
        if space_held {
            // The drag has to be able to START while the key is down, so the grab
            // hand appears immediately rather than after the first movement.
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }
        if response.hovered() && panning {
            ui.ctx().set_cursor_icon(if response.dragged() {
                egui::CursorIcon::Grabbing
            } else {
                egui::CursorIcon::Grab
            });
        }
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
        // begins — otherwise resizing is invisible until stumbled on. While
        // space is held the pointer is not going to resize anything, so it must
        // not promise to.
        if response.hovered() && !panning && matches!(self.drag, Drag::None) {
            if let Some(p) = response.hover_pos() {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc().view.to_world(sx, sy);
                let band = grab_band(self.doc().view);
                if let Some(grab) = self.doc().mandala.resize_grab(wx, wy, band) {
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
        } else if !menu_open && response.drag_started_by(PointerButton::Middle) {
            // The middle button always moves the view. It is the one panning
            // gesture that touches no keyboard at all, so nothing focused
            // anywhere can quietly take it away.
            self.drag = Drag::Pan;
        } else if !menu_open && response.drag_started_by(PointerButton::Primary) && panning {
            // The drag tool: the pointer moves the view wherever it is resting.
            self.drag = Drag::Pan;
        } else if !menu_open && response.drag_started_by(PointerButton::Primary) {
            // What the gesture grabbed is decided from where the button went
            // DOWN, not from where the pointer has already travelled to by the
            // time egui calls it a drag rather than a click. A wall is only a
            // few pixels wide, so resolving it after that threshold made every
            // brisk pull off a border pan the canvas instead of resizing.
            let origin = ui
                .input(|input| input.pointer.press_origin())
                .or_else(|| response.interact_pointer_pos());
            if let Some(p) = origin {
                let (sx, sy) = local(p);
                let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                let band = grab_band(self.doc().view);
                let drag = Drag::beginning_at(&self.doc().mandala, wx, wy, band);
                // Moving or sizing a shape changes the drawing;
                // panning only changes where it is looked at.
                if !matches!(drag, Drag::Pan) {
                    self.doc_mut().checkpoint();
                }
                if let Drag::Node { id, holder, .. } = drag {
                    // Membership is reassigned at drop time. Releasing it
                    // while it moves prevents its old boundary from expanding
                    // forever to chase a block that is being pulled out.
                    //
                    // But a boundary is the indentation itself, so it must not
                    // collapse either: releasing alone left the wall to shrink
                    // to nothing under a piece merely being rearranged inside
                    // it. Holding the boundary at the size it had keeps it drawn
                    // around its contents for the length of the gesture, and the
                    // drop hands it back to them.
                    if let Some(holder) = holder {
                        self.doc_mut()
                            .mandala
                            .resize(holder.id, holder.extent.0, holder.extent.1);
                    }
                    self.doc_mut().mandala.release(id);
                }
                self.drag = drag;
            }
        }
        if response.dragged_by(PointerButton::Middle) && matches!(self.drag, Drag::Pan) {
            let d = response.drag_delta();
            self.doc_mut().view.pan(f64::from(d.x), f64::from(d.y));
        }
        if response.drag_stopped_by(PointerButton::Middle) {
            self.drag = Drag::None;
        }
        if response.dragged_by(PointerButton::Primary) && panning {
            // Checked every frame, not only where the drag began, so the view
            // keeps moving for as long as the tool is the drag tool.
            let d = response.drag_delta();
            self.doc_mut().view.pan(f64::from(d.x), f64::from(d.y));
        } else if response.dragged_by(PointerButton::Primary) {
            match self.drag {
                Drag::Resize(grab) => {
                    if let Some(p) = response.interact_pointer_pos() {
                        let (sx, sy) = local(p);
                        let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                        let (mut half_w, mut half_h) = self.doc().mandala.extent(grab.id);
                        if let Some(node) = self.doc().mandala.node(grab.id) {
                            let (dx, dy) = ((wx - node.x).abs(), (wy - node.y).abs());
                            if !scales_uniformly(&node.form) {
                                // A square alone has two free walls.
                                if grab.wide {
                                    half_w = dx;
                                }
                                if grab.tall {
                                    half_h = dy;
                                }
                            } else if node.shape() == Shape::Circle {
                                // A circle is dragged by its circumference, so
                                // the pointer's distance from the centre IS the
                                // radius it is asking for.
                                let radius = dx.hypot(dy);
                                half_w = radius;
                                half_h = radius;
                            } else {
                                // Any other outline scales whole. Whichever axis
                                // the pointer reached furthest past sets one
                                // factor, and `resize` applies it to both while
                                // keeping the form's own proportions.
                                let (base_w, base_h) = node.base_extent();
                                let scale = (dx / base_w.max(f64::EPSILON))
                                    .max(dy / base_h.max(f64::EPSILON));
                                half_w = base_w * scale;
                                half_h = base_h * scale;
                            }
                        }
                        // A boundary and what it holds read as one object, so
                        // the wall carries the forms indented inside it: they
                        // travel and grow with it rather than staying a speck
                        // in a room that keeps getting larger.
                        self.doc_mut().mandala.resize_group(grab.id, half_w, half_h);
                    }
                }
                Drag::Node { id, grab, .. } => {
                    if let Some(p) = response.interact_pointer_pos() {
                        let (sx, sy) = local(p);
                        let (wx, wy) = self.doc_mut().view.to_world(sx, sy);
                        // A container and what it holds read as one object, so
                        // dragging it carries its interior along.
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
            if let Drag::Node { id, holder, .. } = self.drag {
                let position = self.doc().mandala.node(id).map(|node| (node.x, node.y));
                if let Some((x, y)) = position {
                    let previous = holder.map(|snapshot| snapshot.id);
                    let destination = self
                        .doc()
                        .mandala
                        .holder_at(id, x, y)
                        .or_else(|| holder.filter(|old| old.contains(x, y)).map(|old| old.id));
                    if let Some(container) = destination {
                        // The visible indentation is the structural gesture.
                        // Re-dropping within the same boundary merely moves the
                        // piece; crossing into a new one reparents the complete
                        // subtree before assigning its presentation membership.
                        let nested = previous == Some(container)
                            || self.doc_mut().mandala.reparent(container, id)
                            || self.doc().mandala.children(container).contains(&id);
                        if nested && self.doc_mut().mandala.hold(container, id) {
                            self.doc_mut().mandala.make_room_for_container(container);
                        }
                    } else if previous.is_some() {
                        self.doc_mut().mandala.detach(id);
                    }
                }
                // The gesture is over, so the boundary it was pinned at goes
                // back to being decided by what it holds — which is now the
                // right answer whether the piece stayed, moved on, or left.
                if let Some(holder) = holder {
                    self.doc_mut().mandala.set_size(holder.id, holder.size);
                }
            }
            self.drag = Drag::None;
        }

        if !menu_open && !panning && response.clicked_by(PointerButton::Primary) {
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
            self.doc_mut().show_everything();
        }

        // A turning ring needs a clock and a reason to redraw. egui only
        // repaints on input, so ask for frames while something is running.
        if !self.doc().running.is_empty() {
            ui.ctx().request_repaint();
        }
        let hovered = match self.drag {
            Drag::Resize(grab) => Some(grab.id),
            Drag::None => response.hover_pos().and_then(|pointer| {
                let (sx, sy) = local(pointer);
                let (wx, wy) = self.doc().view.to_world(sx, sy);
                self.doc()
                    .mandala
                    .resize_grab(wx, wy, grab_band(self.doc().view))
                    .map(|grab| grab.id)
                    .or_else(|| self.doc().mandala.hit(wx, wy))
            }),
            Drag::Node { .. } | Drag::Pan | Drag::Marquee { .. } => None,
        };
        self.paint_2d(&painter, origin, ui.input(|i| i.time) as f32, hovered);
    }

    /// Orbitable and minimally editable structural projection of the mandala.
    ///
    /// Empty-space drag orbits. Dragging a piece moves its presentation-only
    /// 3D offset, optionally constrained to the world X/Y/Z gizmo. These
    /// offsets participate in ordinary undo but never alter generated Rebis or
    /// the hand-authored 2D arrangement.
    fn canvas_3d(&mut self, ui: &mut egui::Ui) {
        let mut picked_axis = None;
        let mut reset_pieces = false;
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("3D EDIT")
                    .monospace()
                    .strong()
                    .color(self.ink.accent),
            );
            ui.colored_label(self.ink.faint, "move");
            for axis in [
                SpatialAxis::Free,
                SpatialAxis::X,
                SpatialAxis::Y,
                SpatialAxis::Z,
            ] {
                if ui
                    .selectable_label(self.doc().spatial_axis == axis, axis.label())
                    .clicked()
                {
                    picked_axis = Some(axis);
                }
            }
            if ui
                .add_enabled(
                    self.doc()
                        .mandala
                        .nodes()
                        .iter()
                        .any(|node| node.spatial_offset != [0.0; 3]),
                    egui::Button::new("reset pieces"),
                )
                .clicked()
            {
                reset_pieces = true;
            }
            ui.colored_label(
                self.ink.faint,
                "drag piece · drag empty to orbit · wheel zoom",
            );
        });
        if let Some(axis) = picked_axis {
            self.doc_mut().spatial_axis = axis;
            if let SpatialDrag::Move {
                id,
                pointer,
                original,
                scale,
                ..
            } = self.spatial_drag
            {
                self.spatial_drag = SpatialDrag::Move {
                    id,
                    axis,
                    pointer,
                    original,
                    scale,
                };
            }
        }
        if reset_pieces {
            self.doc_mut().checkpoint();
            self.doc_mut().mandala.reset_spatial_offsets();
        }

        let (response, painter) = ui.allocate_painter(ui.available_size(), Sense::click_and_drag());
        // A click that dismisses the right-click menu must only close it, never
        // also orbit or select. True coming into the frame; egui closes the
        // menu as it handles the click.
        let menu_open = ui.ctx().is_context_menu_open();

        if response.hovered() {
            let scroll = ui.input(|input| input.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                let factor = if scroll > 0.0 { 1.1 } else { 1.0 / 1.1 };
                self.doc_mut().camera.zoom_by(factor);
            }
        }

        let shortcut_axis = ui.input(|input| {
            use egui::Key;
            if input.key_pressed(Key::G) {
                Some(SpatialAxis::Free)
            } else if input.key_pressed(Key::X) {
                Some(SpatialAxis::X)
            } else if input.key_pressed(Key::Y) {
                Some(SpatialAxis::Y)
            } else if input.key_pressed(Key::Z) {
                Some(SpatialAxis::Z)
            } else {
                None
            }
        });
        if let Some(axis) = shortcut_axis {
            self.doc_mut().spatial_axis = axis;
            if let SpatialDrag::Move {
                id,
                pointer,
                original,
                scale,
                ..
            } = self.spatial_drag
            {
                self.spatial_drag = SpatialDrag::Move {
                    id,
                    axis,
                    pointer,
                    original,
                    scale,
                };
            }
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

        let mut layout = self.doc().mandala.spatial_layout();
        let mut projected = project_spatial(
            &self.doc().mandala,
            &layout,
            response.rect,
            self.doc().camera,
        );

        if !menu_open && response.drag_started() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let selected = self.doc().selected;
                let gizmo_axis = selected
                    .and_then(|id| projected.iter().find(|node| node.id == id))
                    .and_then(|node| spatial_gizmo_hit(pointer, node.position, self.doc().camera));
                let target = gizmo_axis
                    .and(selected)
                    .or_else(|| spatial_hit(&self.doc().mandala, &projected, pointer));
                if let Some(id) = target {
                    self.doc_mut().select_only(id);
                    self.doc_mut().checkpoint();
                    let original = self
                        .doc()
                        .mandala
                        .node(id)
                        .map(|node| node.spatial_offset)
                        .unwrap_or([0.0; 3]);
                    let scale = projected
                        .iter()
                        .find(|node| node.id == id)
                        .map(|node| node.scale)
                        .unwrap_or(1.0)
                        .max(0.05);
                    self.spatial_drag = SpatialDrag::Move {
                        id,
                        axis: gizmo_axis.unwrap_or(self.doc().spatial_axis),
                        pointer,
                        original,
                        scale,
                    };
                } else {
                    self.spatial_drag = SpatialDrag::Orbit;
                }
            }
        }

        let cancel_move = ui.input(|input| input.key_pressed(egui::Key::Escape));
        if cancel_move {
            if let SpatialDrag::Move { id, original, .. } = self.spatial_drag {
                self.doc_mut().mandala.set_spatial_offset(id, original);
            }
            self.spatial_drag = SpatialDrag::None;
        } else if !menu_open && response.dragged() {
            match self.spatial_drag {
                SpatialDrag::Orbit => {
                    let delta = ui.input(|input| input.pointer.delta());
                    let camera = &mut self.doc_mut().camera;
                    camera.yaw += delta.x * 0.008;
                    camera.pitch = (camera.pitch - delta.y * 0.008).clamp(-1.35, 1.35);
                    ui.ctx().request_repaint();
                }
                SpatialDrag::Move {
                    id,
                    axis,
                    pointer: start,
                    original,
                    scale,
                } => {
                    if let Some(pointer) = response.interact_pointer_pos() {
                        let delta = pointer - start;
                        let mut offset = original;
                        if let Some(component) = axis.component() {
                            let direction = spatial_axis_direction(self.doc().camera, axis);
                            offset[component] += f64::from(delta.dot(direction) / scale);
                        } else {
                            let camera = self.doc().camera;
                            let (cy, sy) = (camera.yaw.cos(), camera.yaw.sin());
                            let (cp, sp) = (camera.pitch.cos(), camera.pitch.sin());
                            let right = [cy, 0.0, sy];
                            let down = [sp * sy, cp, -sp * cy];
                            let dx = delta.x / scale;
                            let dy = delta.y / scale;
                            for component in 0..3 {
                                offset[component] +=
                                    f64::from(right[component] * dx + down[component] * dy);
                            }
                        }
                        self.doc_mut().mandala.set_spatial_offset(id, offset);
                        ui.ctx().request_repaint();
                    }
                }
                SpatialDrag::None => {}
            }
        }
        if response.drag_stopped() {
            self.spatial_drag = SpatialDrag::None;
        }

        // Camera and object motion above change projection immediately in this
        // same frame; recompute before hit testing and painting.
        layout = self.doc().mandala.spatial_layout();
        projected = project_spatial(
            &self.doc().mandala,
            &layout,
            response.rect,
            self.doc().camera,
        );
        if !menu_open && response.clicked() {
            if let Some(pointer) = response.interact_pointer_pos() {
                let selected = spatial_hit(&self.doc().mandala, &projected, pointer);
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
        let mut reset_offsets = false;
        response.context_menu(|ui| {
            if ui.button("reset view").clicked() {
                reset_view = true;
                ui.close_menu();
            }
            if ui.button("reset piece positions").clicked() {
                reset_offsets = true;
                ui.close_menu();
            }
        });
        if reset_view {
            self.doc_mut().camera = SpatialCamera::default();
            ui.ctx().request_repaint();
        }
        if reset_offsets {
            self.doc_mut().checkpoint();
            self.doc_mut().mandala.reset_spatial_offsets();
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
        paint_spatial_gizmo(
            &painter,
            response.rect.left_bottom() + Vec2::new(58.0, -58.0),
            self.doc().camera,
            self.ink,
            34.0,
        );
        if let Some(selected) = self
            .doc()
            .selected
            .and_then(|id| projected.iter().find(|node| node.id == id))
        {
            paint_spatial_gizmo(
                &painter,
                selected.position,
                self.doc().camera,
                self.ink,
                62.0,
            );
        }
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
            (Some(id), Tool::Flow(_)) if !is_flow_boundary(&self.doc().mandala, id) => {
                self.doc_mut().pending = None;
                self.notice =
                    Some("flow endpoints are indentation blocks: choose a circle or square".into());
            }
            (Some(id), Tool::Flow(form)) => match self.doc_mut().pending {
                None => self.doc_mut().pending = Some(id),
                Some(from) => {
                    if from != id {
                        self.doc_mut().checkpoint();
                    }
                    if let Some(made) = self.doc_mut().mandala.flow(from, id, form) {
                        // Shift completes the connection as a right-angle circuit
                        // trace; otherwise it keeps the default straight line.
                        if angle {
                            self.doc_mut().squared.insert(made);
                        }
                        self.doc_mut().select_only(made);
                    }
                    self.doc_mut().pending = None;
                }
            },
            // A prefix sigil is not a form you drop somewhere — it is a mark on
            // a form. Clicking one onto a shape writes it there, and clicking it
            // again takes it off, which is the only way to reach `'x` now that
            // the sigil is drawn on its operand rather than beside it.
            (Some(id), Tool::Place(form)) if form.is_prefix_sigil() => {
                self.doc_mut().checkpoint();
                match self.doc().mandala.prefixed_with(id, &form) {
                    Some(existing) => {
                        self.doc_mut().mandala.unwrap_form(existing);
                        self.notice = Some(format!("removed {}", form.prefix_sigil()));
                        self.doc_mut().select_only(id);
                    }
                    None => match self.doc_mut().mandala.wrap(id, form.clone()) {
                        Some(_) => {
                            self.notice = Some(format!("wrote {}", form.prefix_sigil()));
                            self.doc_mut().select_only(id);
                        }
                        None => self.doc_mut().select_only(id),
                    },
                }
            }
            // Placing a form on a boundary draws it INSIDE, as that boundary's
            // newest content — a click inside a circle is how you fill it in,
            // and it used to do nothing but reselect the circle it landed on.
            // Shift draws the new boundary AROUND it instead, which is the rarer
            // and far more disruptive of the two. Either way it is one edit.
            (Some(id), Tool::Place(form))
                if self
                    .doc()
                    .mandala
                    .node(id)
                    .is_some_and(|node| node.form.opens_indentation()) =>
            {
                self.doc_mut().checkpoint();
                let wrapping = angle && form.opens_indentation();
                let made = if wrapping {
                    self.doc_mut().mandala.wrap(id, form)
                } else {
                    let text = default_text(&form);
                    self.doc_mut().mandala.nest(id, form, text)
                };
                match made {
                    Some(made) => {
                        self.notice = Some(if wrapping {
                            "drew it around".into()
                        } else {
                            "drew it inside".into()
                        });
                        self.doc_mut().select_only(made);
                    }
                    None => self.doc_mut().select_only(id),
                }
            }
            (Some(id), _) => self.doc_mut().select_only(id),
            // Clicked empty canvas.
            (None, Tool::Place(form)) if form.is_prefix_sigil() => {
                self.notice = Some(format!(
                    "{} is written on a form — click one",
                    form.prefix_sigil()
                ));
            }
            (None, Tool::Place(form)) => {
                let text = default_text(&form);
                self.doc_mut().checkpoint();
                let id = self.doc_mut().mandala.add(form, text, wx, wy);
                self.doc_mut().select_only(id);
            }
            (None, Tool::Flow(_)) => self.doc_mut().pending = None,
            (None, Tool::Select) => self.doc_mut().clear_selection(),
            // The drag tool only moves the view; a click under it changes
            // nothing, so a stray click cannot place or deselect anything.
            (_, Tool::Pan) => {}
        }
    }

    /// The drawing surface and the wall.
    ///
    /// Left is the paper; right is what is kept and what is stacked. Nothing
    /// here touches the mandala — a drawn sigil is intent, and the program it
    /// asks for arrives as source you read before running.
    fn ink_tab(&mut self, ui: &mut egui::Ui) {
        let k = self.ink;
        let Some(Pane::Ink(pane)) = self.tabs.active_mut() else {
            return;
        };
        if pane.pen <= 0.0 {
            pane.pen = 3.0;
        }
        // Taken out so the surface and the side can borrow the editor freely.
        let mut asked: Option<String> = None;

        egui::SidePanel::right("ink_side")
            .default_width(300.0)
            .width_range(240.0..=520.0)
            .resizable(true)
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.add_space(6.0);
                    ui.colored_label(k.accent, "SIGIL");
                    ui.label(
                        egui::RichText::new(
                            "A drawn mark standing for an intent. The model reads it and \
                             answers with a program — which you read before it runs.",
                        )
                        .small(),
                    );

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("pen");
                        ui.add(egui::Slider::new(&mut pane.pen, 1.0..=12.0).show_value(false));
                        ui.label(format!("{:.0}", pane.pen));
                    });

                    let marks = pane.sigil.strokes.len();
                    ui.horizontal(|ui| {
                        ui.label(format!("{marks} mark(s)"));
                        if ui
                            .add_enabled(marks > 0, egui::Button::new("undo"))
                            .clicked()
                        {
                            if let Some(stroke) = pane.sigil.strokes.pop() {
                                pane.undone.push(stroke);
                            }
                        }
                        if ui
                            .add_enabled(!pane.undone.is_empty(), egui::Button::new("redo"))
                            .clicked()
                        {
                            if let Some(stroke) = pane.undone.pop() {
                                pane.sigil.strokes.push(stroke);
                            }
                        }
                        if ui
                            .add_enabled(marks > 0, egui::Button::new("erase"))
                            .clicked()
                        {
                            pane.undone.clear();
                            pane.sigil.strokes.clear();
                        }
                    });

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        ui.label("name");
                        ui.add(
                            egui::TextEdit::singleline(&mut pane.name)
                                .desired_width(110.0)
                                .hint_text("the-queue"),
                        );
                    });
                    // Carroll's word method: the desire, with repeated letters
                    // struck out, drawn faint to draw over and then discard.
                    ui.horizontal(|ui| {
                        ui.colored_label(k.faint, "desire");
                        ui.add(
                            egui::TextEdit::singleline(&mut pane.desire)
                                .desired_width(150.0)
                                .hint_text("I wish to \u{2026}"),
                        )
                        .on_hover_text(
                            "Carroll's word method: the letters that survive become a \
                             scaffold to draw over. The scaffold is never saved and never \
                             reaches a model.",
                        );
                    });
                    let skeleton = kaos_core::ink::letter_skeleton(&pane.desire);
                    if !skeleton.is_empty() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut pane.scaffold, "");
                            ui.colored_label(k.secondary, &skeleton);
                        });
                    }

                    // Keeping and stacking are one action: `&:` reads a file,
                    // so a sigil that is not on the wall cannot be handed to a
                    // program.
                    //
                    // Two of them, because the second is irreversible and a
                    // control that can lose a person's desire has to say so
                    // before it does it rather than after.
                    let mut commit: Option<kaos_core::ink::Retention> = None;
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(marks > 0, egui::Button::new("keep & stack"))
                            .on_hover_text("the glyph, and what it was for")
                            .clicked()
                        {
                            commit = Some(kaos_core::ink::Retention::Keep);
                        }
                        if ui
                            .add_enabled(marks > 0, egui::Button::new("charge & lose"))
                            .on_hover_text(
                                "Keeps the glyph and DISCARDS the desire, unwritten \u{2014} \
                                 nothing on the wall or in any run record will say what this \
                                 sigil was for. There is no undo.",
                            )
                            .clicked()
                        {
                            commit = Some(kaos_core::ink::Retention::Lose);
                        }
                    });
                    if let Some(retention) = commit {
                        let wall = kaos_core::ink::Wall::default_wall();
                        let name = if pane.name.trim().is_empty() {
                            format!("sigil-{}", pane.stack.pictures.len() + 1)
                        } else {
                            pane.name.trim().to_string()
                        };
                        match wall.commit(&name, &pane.sigil, &pane.desire, retention, 512) {
                            Ok(picture) => {
                                pane.stack.push(picture);
                                // The stack's own retention follows the strictest
                                // commit in it: one lost sigil is enough to keep
                                // the sentence out of the program, and a stack
                                // that carried it anyway would have lost nothing.
                                if retention == kaos_core::ink::Retention::Lose {
                                    pane.stack.retention = kaos_core::ink::Retention::Lose;
                                } else if !pane.desire.trim().is_empty()
                                    && pane.stack.said.trim().is_empty()
                                {
                                    pane.stack.said.clone_from(&pane.desire);
                                }
                                pane.sigil.strokes.clear();
                                pane.undone.clear();
                                pane.name.clear();
                                pane.desire.clear();
                                pane.notice = match retention {
                                    kaos_core::ink::Retention::Keep => {
                                        format!("`{name}` kept and stacked")
                                    }
                                    kaos_core::ink::Retention::Lose => {
                                        format!("`{name}` stacked \u{00b7} the desire is gone")
                                    }
                                };
                            }
                            Err(error) => pane.notice = error.to_string(),
                        }
                    }

                    let wall = kaos_core::ink::Wall::default_wall();
                    let saved = wall.names();
                    ui.add_space(8.0);
                    egui::CollapsingHeader::new(format!("WALL — {}", saved.len()))
                        .id_salt("ink_wall")
                        .default_open(true)
                        .show(ui, |ui| {
                            if saved.is_empty() {
                                ui.colored_label(k.faint, "nothing kept yet");
                            }
                            for name in saved {
                                ui.horizontal(|ui| {
                                    ui.label(&name);
                                    if ui.small_button("stack").clicked() {
                                        if let Ok((_, picture)) = wall.paths(&name) {
                                            pane.stack.push(picture);
                                            pane.notice = format!("`{name}` stacked");
                                        }
                                    }
                                    if ui.small_button("open").clicked() {
                                        match wall.load(&name) {
                                            Ok(drawn) => {
                                                pane.sigil = drawn;
                                                pane.undone.clear();
                                                pane.name.clone_from(&name);
                                                pane.notice = format!("`{name}` opened");
                                            }
                                            Err(error) => pane.notice = error.to_string(),
                                        }
                                    }
                                    if ui.small_button("forget").clicked() {
                                        let _ = wall.forget(&name);
                                        pane.notice = format!("`{name}` forgotten");
                                    }
                                });
                            }
                        });

                    ui.add_space(8.0);
                    ui.colored_label(
                        k.secondary,
                        format!("STACK — {}", pane.stack.pictures.len()),
                    );
                    if pane.stack.is_empty() {
                        ui.colored_label(k.faint, "draw, then keep & stack");
                    }
                    let mut drop_at: Option<usize> = None;
                    for (index, picture) in pane.stack.pictures.iter().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(format!(
                                    "{}. {}",
                                    index + 1,
                                    picture
                                        .file_stem()
                                        .map(|s| s.to_string_lossy().into_owned())
                                        .unwrap_or_default()
                                ))
                                .small(),
                            );
                            if ui.small_button("×").clicked() {
                                drop_at = Some(index);
                            }
                        });
                    }
                    if let Some(index) = drop_at {
                        pane.stack.pictures.remove(index);
                    }

                    if !pane.stack.is_empty() {
                        ui.add_space(4.0);
                        ui.label(egui::RichText::new("said (optional)").small());
                        ui.add(
                            egui::TextEdit::multiline(&mut pane.stack.said)
                                .desired_rows(2)
                                .desired_width(f32::INFINITY)
                                .hint_text("a sigil that needs a sentence is a sentence"),
                        );
                    }

                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(!pane.stack.is_empty(), egui::Button::new("ASK"))
                            .clicked()
                        {
                            asked = Some(pane.stack.program());
                        }
                        if ui
                            .add_enabled(!pane.stack.is_empty(), egui::Button::new("clear"))
                            .clicked()
                        {
                            pane.stack.clear();
                        }
                    });
                    if !pane.notice.is_empty() {
                        ui.add_space(4.0);
                        ui.colored_label(k.faint, pane.notice.clone());
                    }
                });
            });

        // ── the paper ──────────────────────────────────────────────────
        egui::CentralPanel::default().show_inside(ui, |ui| {
            let (response, painter) = ui.allocate_painter(ui.available_size(), egui::Sense::drag());
            let rect = response.rect;
            // White paper regardless of theme: a sigil is drawn in ink, and
            // what the model is shown is black on white, so what you draw on
            // should be what it reads.
            painter.rect_filled(rect, 0.0, egui::Color32::from_gray(250));
            painter.rect_stroke(rect, 0.0, UiStroke::new(1.0, k.rule));
            if response.hovered() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }

            let at = |p: Pos2| (p.x - rect.left(), p.y - rect.top());
            if response.drag_started() {
                pane.drawing = Some(kaos_core::ink::Stroke::new(pane.pen));
                // A new mark ends the redo trail, as everywhere else.
                pane.undone.clear();
            }
            if response.dragged() {
                if let Some(p) = response.interact_pointer_pos() {
                    let (x, y) = at(p);
                    if let Some(stroke) = pane.drawing.as_mut() {
                        stroke.add(x, y);
                    }
                }
            }
            if response.drag_stopped() {
                if let Some(stroke) = pane.drawing.take() {
                    if !stroke.is_empty() {
                        pane.sigil.strokes.push(stroke);
                    }
                }
            }

            let ribbon = |stroke: &kaos_core::ink::Stroke| {
                let points: Vec<Pos2> = stroke
                    .points
                    .iter()
                    .map(|(x, y)| Pos2::new(rect.left() + x, rect.top() + y))
                    .collect();
                let ink = egui::Color32::from_gray(20);
                match points.len() {
                    0 => {}
                    1 => {
                        painter.circle_filled(points[0], stroke.width / 2.0, ink);
                    }
                    _ => {
                        painter.add(egui::Shape::line(points, UiStroke::new(stroke.width, ink)));
                    }
                }
            };
            // The scaffold, under everything. Carroll's surviving letters, laid
            // out to draw over and then discard — a guide, never a mark. It is
            // painted straight onto the canvas rather than pushed into
            // `pane.sigil`, which is what keeps it out of the saved strokes and
            // out of the raster a model is shown.
            if pane.scaffold {
                let skeleton = kaos_core::ink::letter_skeleton(&pane.desire);
                if !skeleton.is_empty() {
                    let letters: Vec<char> = skeleton.chars().collect();
                    // A ring, so the letters are a shape to trace rather than a
                    // line to read — the scaffold should stop looking like the
                    // sentence as soon as possible.
                    let centre = rect.center();
                    let radius = rect.width().min(rect.height()) * 0.32;
                    let size = (radius * 0.42).clamp(14.0, 64.0);
                    for (index, letter) in letters.iter().enumerate() {
                        let turn = index as f32 / letters.len() as f32 * std::f32::consts::TAU
                            - std::f32::consts::FRAC_PI_2;
                        painter.text(
                            centre + egui::vec2(turn.cos() * radius, turn.sin() * radius),
                            egui::Align2::CENTER_CENTER,
                            letter,
                            egui::FontId::proportional(size),
                            egui::Color32::from_gray(215),
                        );
                    }
                }
            }
            for stroke in &pane.sigil.strokes {
                ribbon(stroke);
            }
            if let Some(stroke) = pane.drawing.as_ref() {
                ribbon(stroke);
            }
            if pane.sigil.is_empty() && pane.drawing.is_none() {
                painter.text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "drag to draw",
                    egui::FontId::proportional(14.0),
                    k.faint,
                );
            }
        });

        // The ask goes through the ordinary run path, so it is budgeted,
        // traced and recorded like any other program.
        if let Some(program) = asked {
            self.request_run_options(
                program,
                None,
                std::collections::HashSet::new(),
                runs::Scope::Program,
            );
        }
    }

    /// A rotating dashed ring around a node that is being evaluated.
    ///
    /// Dashes are drawn as short arcs stepped around the circle and offset by
    /// the clock, so the ring turns. It reads as motion without animating the
    /// node itself, which must stay legible while it runs.
    fn running_ring(
        &self,
        painter: &egui::Painter,
        centre: Pos2,
        radius: Vec2,
        stroke_scale: f32,
        spin: f32,
    ) {
        const DASHES: usize = 12;
        const SEGMENTS: usize = 4;
        let stroke = UiStroke::new(2.2 * stroke_scale, self.ink.accent);
        let arc = std::f32::consts::TAU / DASHES as f32;
        for dash in 0..DASHES {
            if dash % 2 == 1 {
                continue; // the gaps
            }
            let start = spin + dash as f32 * arc;
            let mut previous = None;
            for step in 0..=SEGMENTS {
                let a = start + arc * step as f32 / SEGMENTS as f32;
                let p = Pos2::new(centre.x + radius.x * a.cos(), centre.y + radius.y * a.sin());
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
            view_scale: zoom,
            resize,
            outline,
            hot,
        } = paint;
        let glyph_scale = resize * zoom;
        let shape = node.shape();
        match shape {
            Shape::Circle => {
                let radius = self.doc().mandala.extent(node.id).0 as f32 * zoom;
                painter.circle(centre, radius, k.fill, outline);
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
            Shape::Brace => {
                let (half_w, half_h) = self.doc().mandala.extent(node.id);
                let rect = Rect::from_center_size(
                    centre,
                    Vec2::new(half_w as f32 * 2.0, half_h as f32 * 2.0) * zoom,
                );
                // Fill a plain rounded box first: the braces are an outline,
                // and a concave path is not something to ask a fill for.
                painter.rect_filled(rect, 4.0 * zoom, k.fill);
                let pinch = (rect.width().min(rect.height()) * 0.08).clamp(3.0, 14.0);
                let mut points = brace_box_points(rect, pinch);
                points.push(points[0]);
                // Dash-dotted, because a space is speculative: its working is
                // real while it runs and leaves no evidence behind. A solid
                // outline would draw it as part of the finished object.
                dash_dotted_path(painter, &points, zoom, outline);
            }
            Shape::Diamond => {
                let points = Shape::diamond_points()
                    .iter()
                    .map(|(x, y)| {
                        Pos2::new(centre.x + x * glyph_scale.x, centre.y + y * glyph_scale.y)
                    })
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Parallelogram => {
                let points = Shape::parallelogram_points()
                    .iter()
                    .map(|(x, y)| {
                        Pos2::new(centre.x + x * glyph_scale.x, centre.y + y * glyph_scale.y)
                    })
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Hexagon => {
                let points = Shape::hexagon_points()
                    .map(|(x, y)| centre + Vec2::new(x * glyph_scale.x, y * glyph_scale.y))
                    .to_vec();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Amp => {
                let points = Shape::inlet_points()
                    .iter()
                    .map(|(x, y)| {
                        Pos2::new(centre.x + x * glyph_scale.x, centre.y + y * glyph_scale.y)
                    })
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            Shape::Arrow => {
                let reverse = node.form == Form::Backflow;
                let points = Shape::arrow_points()
                    .iter()
                    .map(|(x, y)| {
                        let x = if reverse { -*x } else { *x };
                        Pos2::new(centre.x + x * glyph_scale.x, centre.y + y * glyph_scale.y)
                    })
                    .collect();
                painter.add(egui::Shape::convex_polygon(points, k.fill, outline));
            }
            // A sigil is its own visible shape. Its generous round click target
            // remains model-only (see Shape::contains), so compose alone owns
            // a circular outline.
            _ => {
                let pen = UiStroke::new(
                    pen(5.0 * zoom * resize_weight(resize)),
                    if hot { k.accent } else { k.ink },
                );
                for stroke in shape.strokes() {
                    match stroke {
                        Stroke::Poly(points) => {
                            let points = points
                                .iter()
                                .map(|(x, y)| {
                                    Pos2::new(
                                        centre.x + x * glyph_scale.x,
                                        centre.y + y * glyph_scale.y,
                                    )
                                })
                                .collect();
                            painter.add(egui::Shape::line(points, pen));
                        }
                        Stroke::Cubic(points) => {
                            let points = [
                                Pos2::new(
                                    centre.x + points[0].0 * glyph_scale.x,
                                    centre.y + points[0].1 * glyph_scale.y,
                                ),
                                Pos2::new(
                                    centre.x + points[1].0 * glyph_scale.x,
                                    centre.y + points[1].1 * glyph_scale.y,
                                ),
                                Pos2::new(
                                    centre.x + points[2].0 * glyph_scale.x,
                                    centre.y + points[2].1 * glyph_scale.y,
                                ),
                                Pos2::new(
                                    centre.x + points[3].0 * glyph_scale.x,
                                    centre.y + points[3].1 * glyph_scale.y,
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

    /// Give indentation circles and mediator squares real volume in the 3D
    /// projection. The structural renderer is painter-based rather than a mesh
    /// engine, so a sphere is built from a shaded terminator/highlight and a
    /// square from a projected back face plus four side faces. Both retain the
    /// exact screen footprint used by hit testing and connections.
    fn paint_3d_volume_body(&self, painter: &egui::Painter, node: &Node, paint: GlyphPaint) {
        let k = self.ink;
        let GlyphPaint {
            position: centre,
            view_scale: zoom,
            outline,
            hot,
            ..
        } = paint;
        let mix = |from: Color32, to: Color32, amount: f32| {
            let amount = amount.clamp(0.0, 1.0);
            let channel = |a: u8, b: u8| {
                (f32::from(a) + (f32::from(b) - f32::from(a)) * amount).round() as u8
            };
            Color32::from_rgb(
                channel(from.r(), to.r()),
                channel(from.g(), to.g()),
                channel(from.b(), to.b()),
            )
        };
        match node.shape() {
            Shape::Circle => {
                let radius = self.doc().mandala.extent(node.id).0 as f32 * zoom;
                let dark = mix(k.fill, k.ground, 0.46);
                let middle = mix(k.fill, k.accent, if hot { 0.24 } else { 0.11 });
                let light = mix(k.ink, k.accent, 0.20);
                painter.circle_filled(centre, radius, dark);
                // The shifted middle disc creates a curved terminator; two
                // smaller ellipses make the upper-left key light explicit.
                painter.circle_filled(
                    centre - Vec2::new(radius * 0.10, radius * 0.12),
                    radius * 0.88,
                    middle,
                );
                painter.add(egui::Shape::ellipse_filled(
                    centre - Vec2::new(radius * 0.28, radius * 0.30),
                    Vec2::new(radius * 0.32, radius * 0.20),
                    role_wash(light, 105),
                ));
                painter.add(egui::Shape::ellipse_filled(
                    centre + Vec2::new(radius * 0.24, radius * 0.30),
                    Vec2::new(radius * 0.48, radius * 0.15),
                    frost_shadow(k, 38.0 / 255.0),
                ));
                painter.circle_stroke(centre, radius, outline);
                painter.circle_stroke(
                    centre - Vec2::new(radius * 0.10, radius * 0.12),
                    radius * 0.88,
                    UiStroke::new((0.7 * zoom).max(0.7), role_wash(k.accent, 44)),
                );
            }
            Shape::Square => {
                let (half_w, half_h) = self.doc().mandala.extent(node.id);
                let half = Vec2::new(half_w as f32, half_h as f32) * zoom;
                let front = Rect::from_center_size(centre, half * 2.0);
                let depth = (half.x.min(half.y) * 0.28).clamp(10.0, 34.0);
                let extrusion = -spatial_axis_direction(self.doc().camera, SpatialAxis::Z) * depth;
                let front_points = [
                    front.left_top(),
                    front.right_top(),
                    front.right_bottom(),
                    front.left_bottom(),
                ];
                let back_points = front_points.map(|point| point + extrusion);
                let back_fill = mix(k.fill, k.ground, 0.55);
                let front_fill = mix(k.fill, k.accent, if hot { 0.20 } else { 0.08 });
                painter.add(egui::Shape::convex_polygon(
                    back_points.to_vec(),
                    back_fill,
                    UiStroke::new(outline.width * 0.7, role_wash(outline.color, 150)),
                ));
                for side in 0..4 {
                    let next = (side + 1) % 4;
                    let amount = match side {
                        0 => 0.22,
                        1 => 0.38,
                        2 => 0.55,
                        _ => 0.45,
                    };
                    painter.add(egui::Shape::convex_polygon(
                        vec![
                            back_points[side],
                            back_points[next],
                            front_points[next],
                            front_points[side],
                        ],
                        mix(k.fill, k.ground, amount),
                        UiStroke::new(0.8 * zoom.max(0.7), role_wash(k.faint, 120)),
                    ));
                }
                painter.rect(front, 3.0 * zoom, front_fill, outline);
                painter.line_segment(
                    [
                        front.left_top() + Vec2::new(3.0, 3.0) * zoom,
                        front.right_top() + Vec2::new(-3.0, 3.0) * zoom,
                    ],
                    UiStroke::new(1.1 * zoom.max(0.7), role_wash(k.ink, 78)),
                );
            }
            _ => self.paint_node_body(painter, node, paint),
        }
    }

    /// The pen a form's outline is drawn with, shared by the body pass and the
    /// wall pass so a re-stroked boundary is exactly the line it replaces.
    fn node_outline(&self, node: &Node, zoom: f32, recursive: bool) -> UiStroke {
        let k = self.ink;
        let shape = node.shape();
        let hot = self.doc().is_selected(node.id) || self.doc().pending == Some(node.id);
        let accented = hot || recursive;
        // An arrow is blue whatever else is true of it. Selecting a block must
        // not repaint the one form whose colour carries its meaning; the
        // thicker outline, and the accent ring for recursion, already say
        // everything selection needs to say.
        //
        // A selected quantity is RED, and it is the one exception. The numeric
        // plane is the other pole of the language, so it takes the other colour
        // on a screen that has exactly two — the same reason the emblem has two
        // circles rather than one. Unselected it stays grey like everything
        // else: the hue marks which plane you are TOUCHING, not which plane a
        // form belongs to, and a canvas where every numeric form burned red
        // would be a canvas that shouts about arithmetic.
        let colour = match (hot || recursive, node.form == Form::Numeric) {
            (true, true) => k.danger,
            (true, false) => k.accent,
            (false, _) => k.faint,
        };
        let scale = zoom * pen_weight(node_resize(&self.doc().mandala, node));
        UiStroke::new(pen(node_outline_width(shape, accented) * scale), colour)
    }

    /// Re-stroke a boundary's wall with no fill behind it.
    ///
    /// A circle or square IS the indentation, so its wall has to stay visible
    /// while its contents move — including the moment a piece is dragged across
    /// it. Drawing the boundaries first and the contents afterwards is what puts
    /// held forms inside; this pass puts the wall back on top of them, so a
    /// piece can overlap the border without erasing the container it belongs to.
    fn paint_boundary_wall(&self, painter: &egui::Painter, node: &Node, centre: Pos2, zoom: f32) {
        let outline = self.node_outline(node, zoom, false);
        let (half_w, half_h) = self.doc().mandala.extent(node.id);
        match node.shape() {
            Shape::Circle => {
                painter.circle_stroke(centre, half_w as f32 * zoom, outline);
            }
            Shape::Square => {
                painter.rect_stroke(
                    Rect::from_center_size(
                        centre,
                        Vec2::new(half_w as f32 * 2.0, half_h as f32 * 2.0) * zoom,
                    ),
                    4.0 * zoom,
                    outline,
                );
            }
            _ => {}
        }
    }

    /// Write one indentation's sigil, in the pass that runs after everything.
    ///
    /// The mark used to ride along with the wall, which put it in competition
    /// with the arrows for who gets painted last — and whichever lost was the one
    /// struck through. Neither has to lose: an arrow goes over the walls, and the
    /// marks go over the arrows, so a sigil is never crossed and an arrow is never
    /// erased by a chip.
    fn paint_boundary_mark(&self, painter: &egui::Painter, node: &Node, centre: Pos2, zoom: f32) {
        let (half_w, half_h) = self.doc().mandala.extent(node.id);
        self.paint_indentation_mark(
            painter,
            node,
            centre,
            Vec2::new(half_w as f32, half_h as f32) * zoom,
            zoom * mark_weight(node_resize(&self.doc().mandala, node)),
        );
    }

    /// Write an indentation's notation on its own ring.
    ///
    /// `($ …)` is one circle, and the `$` names it from the boundary rather than
    /// floating loose among the forms the parentheses hold. The wall is cleared
    /// behind the sigil so the outline reads as broken by it rather than struck
    /// through it, which is how a label on a boundary is drawn everywhere else.
    fn paint_indentation_mark(
        &self,
        painter: &egui::Painter,
        node: &Node,
        centre: Pos2,
        extent: Vec2,
        scale: f32,
    ) {
        let mark = format!(
            "{}{}",
            self.doc().mandala.written_prefix(node.id),
            node.mark()
        );
        if mark.is_empty() {
            return;
        }
        let k = self.ink;
        // The mark grows with the boundary it names. A drawing is levels of
        // nesting, and the sigil on a wide outer circle reading larger than the
        // one on a circle deep inside it is how the eye is told which level it
        // is looking at. `glyph_px` still stops it where the rasterizer does.
        let font = FontId::monospace(glyph_px(MARK_PX * scale));
        let galley = painter.layout_no_wrap(mark, font, k.ink);
        let at = Pos2::new(centre.x, centre.y - extent.y);
        let pad = Vec2::new(4.0, 1.0) * scale.clamp(0.5, 2.0);
        painter.rect_filled(
            Rect::from_center_size(at, galley.size() + pad * 2.0),
            2.0 * scale,
            k.fill,
        );
        painter.galley(at - galley.size() * 0.5, galley, k.ink);
    }

    fn paint_graph_node(&self, painter: &egui::Painter, node: &Node, paint: NodePaint) {
        let NodePaint {
            position: centre,
            scale: zoom,
            spin,
            arrow_body,
            recursive,
            volumetric,
        } = paint;
        let shape = node.shape();
        if node.form.is_flow() && !arrow_body {
            return;
        }
        let k = self.ink;
        let resize = node_resize(&self.doc().mandala, node);
        let symbol_weight = resize_weight(resize);
        let symbol_scale = zoom * symbol_weight;
        let (half_w, half_h) = self.doc().mandala.extent(node.id);
        let screen_extent = Vec2::new(half_w as f32, half_h as f32) * zoom;
        let hot = self.doc().is_selected(node.id) || self.doc().pending == Some(node.id);
        let outline = self.node_outline(node, zoom, recursive);
        if recursive {
            painter.add(egui::Shape::ellipse_stroke(
                centre,
                screen_extent + Vec2::splat(8.0 * symbol_scale),
                UiStroke::new(symbol_scale, k.accent),
            ));
        }
        let glyph = GlyphPaint {
            position: centre,
            view_scale: zoom,
            resize,
            outline,
            hot,
        };
        if volumetric && matches!(shape, Shape::Circle | Shape::Square) {
            self.paint_3d_volume_body(painter, node, glyph);
        } else {
            self.paint_node_body(painter, node, glyph);
        }
        if self.doc().running.contains(&node.id) {
            self.running_ring(
                painter,
                centre,
                screen_extent + Vec2::splat(9.0 * symbol_scale),
                symbol_scale,
                spin,
            );
        }
        if let Some(model) = &node.model {
            painter.text(
                Pos2::new(centre.x, centre.y - screen_extent.y - 13.0 * symbol_scale),
                Align2::CENTER_BOTTOM,
                truncate_model(&format!("/{model}")),
                FontId::monospace(glyph_px(9.0 * symbol_scale)),
                k.secondary,
            );
        }

        // One token, at one readable size. A form's complete text lives in the
        // panel, so nothing here has to grow with the shape — which is what
        // used to bury whatever the shape contained under its own caption.
        let caption = node.caption();
        if caption.is_empty() || (node.form == Form::Program && !hot) {
            return;
        }
        let font = FontId::monospace(glyph_px(LABEL_PX * zoom));
        match shape {
            Shape::Circle
            | Shape::Square
            | Shape::Diamond
            | Shape::Parallelogram
            | Shape::Amp
            | Shape::Hexagon => {
                let prefix = self.doc().mandala.written_prefix(node.id);
                // Under `Legend::Whole` the outline was sized to hold all of
                // this text, so all of it is written — set to the largest type
                // that still fits, which is the same layout the PDF exporter
                // uses, so a sheet and a screen break the lines in the same
                // places.
                if self.doc().mandala.legend() == Legend::Whole {
                    let layout = fit_caption(
                        &format!("{prefix}{caption}"),
                        shape,
                        screen_extent,
                        MAX_GLYPH_PX,
                    );
                    let centre = centre + layout.offset;
                    let first = centre.y
                        - layout.line_height * (layout.lines.len().saturating_sub(1) as f32) * 0.5;
                    for (index, line) in layout.lines.iter().enumerate() {
                        painter.text(
                            Pos2::new(centre.x, first + index as f32 * layout.line_height),
                            Align2::CENTER_CENTER,
                            line,
                            FontId::monospace(layout.font_size),
                            k.ink,
                        );
                    }
                    return;
                }
                let written = format!("{prefix}{}", node.glyph());
                painter.text(centre, Align2::CENTER_CENTER, written, font, k.ink);
            }
            _ => {
                painter.text(
                    Pos2::new(centre.x, centre.y + screen_extent.y + 12.0 * zoom),
                    Align2::CENTER_CENTER,
                    format!(
                        "{}{}",
                        self.doc().mandala.written_prefix(node.id),
                        node.glyph()
                    ),
                    font,
                    k.faint,
                );
            }
        }
    }

    fn paint_3d_layers(
        &self,
        painter: &egui::Painter,
        layout: &SpatialLayout,
        projected: &[ProjectedNode],
    ) {
        let k = self.ink;
        let dark = k.ground.r() < 128;
        let fill = k.fill.gamma_multiply(if dark { 0.34 } else { 0.18 });
        let rim = k.faint.gamma_multiply(if dark { 0.28 } else { 0.20 });
        let axis = k.faint.gamma_multiply(if dark { 0.16 } else { 0.12 });
        for plate in spatial_layer_plates(&self.doc().mandala, layout, projected) {
            painter.add(egui::Shape::ellipse_filled(
                plate.centre,
                plate.radius,
                fill,
            ));
            painter.add(egui::Shape::ellipse_stroke(
                plate.centre,
                plate.radius,
                UiStroke::new(1.0, rim),
            ));
            painter.line_segment(
                [
                    plate.centre - Vec2::new(plate.radius.x, 0.0),
                    plate.centre + Vec2::new(plate.radius.x, 0.0),
                ],
                UiStroke::new(0.8, axis),
            );
            painter.text(
                plate.centre - Vec2::new(plate.radius.x - 9.0, 0.0),
                Align2::LEFT_BOTTOM,
                format!("L{}", plate.depth),
                FontId::monospace(8.0),
                rim,
            );
        }
    }

    fn paint_3d_node_shadow(
        &self,
        painter: &egui::Painter,
        node: &Node,
        projected: ProjectedNode,
        depth: usize,
    ) {
        let k = self.ink;
        let dark = self.ink.ground.r() < 128;
        let style = spatial_shadow_style(depth, projected.scale, dark);
        let (half_w, half_h) = self.doc().mandala.extent(node.id);
        let footprint = Vec2::new(
            (half_w as f32 * projected.scale * 0.78).max(10.0),
            (half_h as f32 * projected.scale * 0.24).max(4.5),
        );
        // Broad penumbra first, tight contact shadow last. Several translucent
        // ellipses give us a soft shadow without a texture or a second render
        // target, and remain cheap enough to orbit continuously.
        for (travel, spread, opacity) in
            [(0.72, 1.00, 0.18), (0.88, 0.55, 0.28), (1.00, 0.12, 0.44)]
        {
            let radius =
                footprint + Vec2::new(style.softness * spread, style.softness * spread * 0.42);
            painter.add(egui::Shape::ellipse_filled(
                projected.position + style.offset * travel,
                radius,
                frost_shadow(k, style.opacity * opacity),
            ));
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
        let title =
            "STRUCTURAL 3D  ·  drag pieces / empty space orbits  ·  G X Y Z constrain movement";
        if projected.is_empty() {
            self.chaos_star(painter);
            painter.text(
                painter.clip_rect().left_top() + Vec2::new(14.0, 14.0),
                Align2::LEFT_TOP,
                title,
                FontId::monospace(10.0),
                k.faint,
            );
            painter.text(
                painter.clip_rect().center(),
                Align2::CENTER_CENTER,
                "build the mandala in 2D, then orbit its structural form here",
                FontId::monospace(13.0),
                k.faint,
            );
            return;
        }

        self.paint_3d_layers(painter, layout, projected);
        let find = |id: NodeId| projected.iter().find(|node| node.id == id);
        for edge in visible_edges(&self.doc().mandala) {
            let (Some(from), Some(to)) = (find(edge.from), find(edge.to)) else {
                continue;
            };
            let recursive = layout
                .recursive_edges
                .iter()
                .any(|source| source.to == edge.owner);
            let delta = to.position - from.position;
            let length = delta.length();
            let direction = if length > 0.001 {
                delta / length
            } else {
                Vec2::RIGHT
            };
            let from_extent = self.doc().mandala.extent(edge.from);
            let to_extent = self.doc().mandala.extent(edge.to);
            let from_extent = Vec2::new(from_extent.0 as f32, from_extent.1 as f32) * from.scale;
            let to_extent = Vec2::new(to_extent.0 as f32, to_extent.1 as f32) * to.scale;
            let start = from.position + direction * ray_to_extent(from_extent, direction);
            let end =
                to.position - direction * (ray_to_extent(to_extent, direction) + 4.0 * to.scale);
            let hot = self.doc().is_selected(edge.owner) || self.doc().pending == Some(edge.owner);
            let touches =
                hot || (self.doc().is_selected(edge.from) && self.doc().is_selected(edge.to));
            let link_color = edge_colour(k);
            let depth = layout
                .node(edge.from)
                .map(|node| node.depth)
                .unwrap_or_default()
                .max(
                    layout
                        .node(edge.to)
                        .map(|node| node.depth)
                        .unwrap_or_default(),
                );
            let shadow =
                spatial_shadow_style(depth, (from.scale + to.scale) * 0.5, k.ground.r() < 128);
            let shadow_offset = shadow.offset * 0.38;
            let shadow_color = frost_shadow(k, shadow.opacity * 0.34);
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
                let shadow_points = [
                    start + shadow_offset,
                    control_a + shadow_offset,
                    control_b + shadow_offset,
                    end + shadow_offset,
                ];
                let shadow_stroke = UiStroke::new(stroke.width + 2.2, shadow_color);
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    shadow_points,
                    false,
                    Color32::TRANSPARENT,
                    shadow_stroke,
                ));
                self.arrow_head(
                    painter,
                    end + shadow_offset,
                    end - control_b,
                    10.0,
                    shadow_stroke,
                );
                painter.add(egui::epaint::CubicBezierShape::from_points_stroke(
                    [start, control_a, control_b, end],
                    false,
                    Color32::TRANSPARENT,
                    stroke,
                ));
                self.arrow_head(painter, end, end - control_b, 10.0, stroke);
            } else {
                let stroke = UiStroke::new(if touches { 1.9 } else { 1.35 }, link_color);
                let shadow_stroke = UiStroke::new(stroke.width + 1.8, shadow_color);
                painter.line_segment([start + shadow_offset, end + shadow_offset], shadow_stroke);
                self.arrow_head(
                    painter,
                    end + shadow_offset,
                    end - start,
                    8.0,
                    shadow_stroke,
                );
                painter.line_segment([start, end], stroke);
                self.arrow_head(painter, end, end - start, 8.0, stroke);
            }
            if let Some(model) = self
                .doc()
                .mandala
                .node(edge.owner)
                .and_then(|node| node.model.as_deref())
            {
                let scale = from.scale.min(to.scale).clamp(0.65, 1.3);
                let label_position = start + (end - start) * 0.5 - Vec2::new(0.0, 8.0 * scale);
                painter.text(
                    label_position + Vec2::new(1.5, 2.0),
                    Align2::CENTER_BOTTOM,
                    truncate_model(&format!("/{model}")),
                    FontId::monospace(9.0 * scale),
                    shadow_color,
                );
                painter.text(
                    label_position,
                    Align2::CENTER_BOTTOM,
                    truncate_model(&format!("/{model}")),
                    FontId::monospace(9.0 * scale),
                    k.secondary,
                );
            }
        }

        let mut nodes = projected.to_vec();
        // Far forms first lets nearer forms cover their edges and produces a
        // stable depth cue without changing the underlying graph order.
        nodes.sort_by(|left, right| right.camera_depth.total_cmp(&left.camera_depth));
        for projected_node in &nodes {
            let Some(node) = self.doc().mandala.node(projected_node.id) else {
                continue;
            };
            if node.form.is_flow() {
                continue;
            }
            let depth = layout
                .node(node.id)
                .map(|node| node.depth)
                .unwrap_or_default();
            self.paint_3d_node_shadow(painter, node, *projected_node, depth);
        }
        for projected_node in nodes {
            let Some(node) = self.doc().mandala.node(projected_node.id) else {
                continue;
            };
            if node.form.is_flow() {
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
                    arrow_body: !complete_flow(&self.doc().mandala, node.id),
                    recursive,
                    volumetric: true,
                },
            );
            if let Some(spatial) = spatial.filter(|_| {
                node.form != Form::Program
                    || self.doc().is_selected(node.id)
                    || self.doc().pending == Some(node.id)
            }) {
                let extent = self.doc().mandala.extent(node.id);
                painter.text(
                    projected_node.position
                        + Vec2::new(
                            extent.0 as f32 * projected_node.scale,
                            -extent.1 as f32 * projected_node.scale,
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
                    let extent = self.doc().mandala.extent(child_id);
                    let badge = child.position
                        + Vec2::new(
                            -(extent.0 as f32 * child.scale + 9.0 * scale),
                            -(extent.1 as f32 * child.scale + 9.0 * scale),
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
        self.chaos_star(painter);
        painter.text(
            painter.clip_rect().left_top() + Vec2::new(14.0, 14.0),
            Align2::LEFT_TOP,
            title,
            FontId::monospace(10.0),
            k.faint,
        );
    }

    fn paint_resize_outline(&self, painter: &egui::Painter, node: &Node, centre: Pos2, zoom: f32) {
        let extent = self.doc().mandala.extent(node.id);
        let radius = Vec2::new(extent.0 as f32, extent.1 as f32) * zoom
            + Vec2::splat((2.0 * zoom).clamp(1.0, 4.0));
        let stroke = UiStroke::new(
            (1.35 * zoom).clamp(0.9, 2.2),
            self.ink.accent.gamma_multiply(0.82),
        );
        let dash = (9.0 * zoom).clamp(4.0, 13.0);
        let gap = (6.0 * zoom).clamp(3.0, 10.0);
        if node.form == Form::Compose {
            dashed_ellipse(painter, centre, radius, dash, gap, stroke);
        } else {
            dashed_rect(
                painter,
                Rect::from_center_size(centre, radius * 2.0),
                dash,
                gap,
                stroke,
            );
        }
    }

    fn paint_2d(&self, painter: &egui::Painter, origin: Pos2, time: f32, hovered: Option<NodeId>) {
        let spin = time * 1.6;
        let k = self.ink;
        let v = self.doc().view;
        let zoom = v.zoom as f32;
        // World point to on-screen position.
        let at = |x: f64, y: f64| {
            let (sx, sy) = v.to_screen(x, y);
            Pos2::new(origin.x + sx as f32, origin.y + sy as f32)
        };
        // What a form occupies on screen, generous enough to include the labels,
        // rings and traces that hang off its edges.
        let footprint = |node: &Node| {
            let (half_w, half_h) = self.doc().mandala.extent(node.id);
            let margin = Vec2::splat(24.0 * zoom.max(1.0));
            Rect::from_center_size(
                at(node.x, node.y),
                (Vec2::new(half_w as f32, half_h as f32) * zoom + margin) * 2.0,
            )
        };
        // Zoom has no ceiling, so most of a drawing is usually nowhere near the
        // window. Skipping what cannot be seen keeps a deep zoom as cheap as a
        // shallow one, and keeps coordinates far outside f32's comfortable range
        // away from the tessellator.
        let canvas = painter.clip_rect();
        let showing = |rect: Rect| {
            rect.min.x.is_finite()
                && rect.min.y.is_finite()
                && rect.max.x.is_finite()
                && rect.max.y.is_finite()
                && canvas.intersects(rect)
        };
        self.chaos_star(painter);

        // The semantic projection is shared with 3D: nesting supplies structure
        // and complete flow operators become directional blue connections
        // between circle/square blocks. Boundaries go down first, then those
        // operator connections, then the content resting inside.
        let paint_edges = |want_interior: bool| {
            for edge in visible_edges(&self.doc().mandala) {
                let interior = self.doc().mandala.is_inlined(edge.from)
                    && self.doc().mandala.is_inlined(edge.to);
                if interior != want_interior {
                    continue;
                }
                let (Some(from), Some(to)) = (
                    self.doc().mandala.node(edge.from),
                    self.doc().mandala.node(edge.to),
                ) else {
                    continue;
                };
                if !showing(footprint(from).union(footprint(to))) {
                    continue;
                }
                let hot =
                    self.doc().is_selected(edge.owner) || self.doc().pending == Some(edge.owner);
                let touches =
                    hot || (self.doc().is_selected(edge.from) && self.doc().is_selected(edge.to));
                let stroke =
                    UiStroke::new(pen(if touches { 2.6 } else { 1.8 } * zoom), edge_colour(k));
                let from_extent = self.doc().mandala.extent(from.id);
                let to_extent = self.doc().mandala.extent(to.id);
                circuit_trace(
                    painter,
                    at(from.x, from.y),
                    at(to.x, to.y),
                    [
                        Vec2::new(from_extent.0 as f32, from_extent.1 as f32) * zoom,
                        Vec2::new(to_extent.0 as f32, to_extent.1 as f32) * zoom,
                    ],
                    // The head says which way the answer flows, so it keeps a
                    // readable size even when the whole program is on screen.
                    (11.0 * zoom).max(6.0),
                    stroke,
                    !self.doc().squared.contains(&edge.owner),
                );
                if let Some(model) = self
                    .doc()
                    .mandala
                    .node(edge.owner)
                    .and_then(|node| node.model.as_deref())
                {
                    let midpoint = at(from.x, from.y).lerp(at(to.x, to.y), 0.5);
                    painter.text(
                        midpoint - Vec2::new(0.0, 8.0 * zoom),
                        Align2::CENTER_BOTTOM,
                        truncate_model(&format!("/{model}")),
                        FontId::monospace(glyph_px(9.0 * zoom)),
                        k.secondary,
                    );
                }
            }
        };

        let paint_nodes = |pass: bool, boundaries: bool| {
            for id in self.doc().mandala.paint_order(pass) {
                let Some(n) = self.doc().mandala.node(id) else {
                    continue;
                };
                if n.form.opens_indentation() != boundaries {
                    continue;
                }
                if self.doc().mandala.is_written_prefix(n.id) {
                    // Drawn on the front of its operand, not beside it.
                    continue;
                }
                if !showing(footprint(n)) {
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
                        // A finished flow is drawn as the arrow between the two forms
                        // it routes and as nothing else. Painting its node too put a
                        // little arrow glyph on the canvas beside every connection —
                        // a symbol for a thing that is already a line.
                        arrow_body: !complete_flow(&self.doc().mandala, n.id),
                        recursive: false,
                        volumetric: false,
                    },
                );
            }
        };

        let paint_walls = |pass: bool| {
            for id in self.doc().mandala.paint_order(pass) {
                let Some(n) = self.doc().mandala.node(id) else {
                    continue;
                };
                if !n.form.opens_indentation() {
                    continue;
                }
                if !showing(footprint(n)) {
                    continue;
                }
                self.paint_boundary_wall(painter, n, at(n.x, n.y), zoom);
            }
        };

        let paint_marks = |pass: bool| {
            for id in self.doc().mandala.paint_order(pass) {
                let Some(n) = self.doc().mandala.node(id) else {
                    continue;
                };
                if self.doc().mandala.is_written_prefix(id) {
                    continue;
                }
                if n.form.is_flow() || !showing(footprint(n)) {
                    continue;
                }
                self.paint_boundary_mark(painter, n, at(n.x, n.y), zoom);
            }
        };

        // Every visual boundary is a receiving surface: outer and nested
        // circle/square fills go down first, then the symbols resting on them.
        paint_nodes(false, true);
        paint_nodes(true, true);
        paint_nodes(false, false);
        paint_nodes(true, false);
        // Then the walls, so a boundary stays drawn around its contents even
        // while a piece is dragged across it.
        paint_walls(false);
        paint_walls(true);
        // Then the connections, over the walls. An arrow's whole meaning is the
        // two forms it joins, so it must never be the thing that gets covered —
        // and it was, by the opaque chip a ring label clears the wall with.
        paint_edges(false);
        paint_edges(true);
        // And the sigils last of all. These two used to fight over which went
        // down after the other, and whichever lost was drawn through: with the
        // marks in the wall pass an arrow was erased, and with the arrows moved
        // after the walls the marks were crossed instead. Three passes, no
        // contest — a wall, then what connects it, then what names it.
        paint_marks(false);
        paint_marks(true);

        if let Some(id) = hovered {
            if let Some(node) = self.doc().mandala.node(id) {
                self.paint_resize_outline(painter, node, at(node.x, node.y), zoom);
            }
        }

        if let Some(father) = self.doc().selected.or(self.doc().pending) {
            let children = self.doc().mandala.children(father);
            for (index, child_id) in children.into_iter().enumerate() {
                let Some(child) = self.doc().mandala.node(child_id) else {
                    continue;
                };
                let centre = at(child.x, child.y);
                let scale = zoom.clamp(0.65, 1.2);
                let extent = self.doc().mandala.extent(child_id);
                let badge = centre
                    + Vec2::new(
                        -(extent.0 as f32 * zoom + 9.0 * scale),
                        -(extent.1 as f32 * zoom + 9.0 * scale),
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

// Dashed scale guides are deliberately composed from ordinary line segments,
// so they remain crisp under every canvas zoom without a texture dependency.

/// How many dashes one guide may be cut into.
///
/// A guide's length grows with the zoom and its dashes are held near a fixed
/// pixel size, so the count between them is what runs away: an unbounded zoom
/// would otherwise ask the tessellator for billions of segments and never come
/// back. Past this many, the dashes are lengthened instead of multiplied — at
/// that magnification a single dash is already wider than the window, so the
/// guide reads as the continuous line it has effectively become.
const MAX_DASHES: f32 = 400.0;

/// Dash and gap grown, when needed, so a guide of `length` stays within
/// [`MAX_DASHES`].
fn bounded_dash(length: f32, dash: f32, gap: f32) -> (f32, f32) {
    let period = (dash + gap).max(f32::EPSILON);
    if !length.is_finite() || length <= period * MAX_DASHES {
        return (dash, gap);
    }
    let growth = length / (period * MAX_DASHES);
    (dash * growth, gap * growth)
}

/// A dash-dot outline, walked as ONE run along the whole path.
///
/// The pattern is carried across corners rather than restarted at each, so a
/// closed shape reads as a single stroke instead of four that happen to meet.
/// Restarting per edge is what makes hand-rolled dashes look like a mistake.
///
/// Used for `{ }`, the imaginary space. Dash-dot is the draughtsman's line for
/// something that is present but not part of the finished object — which is
/// exactly what a space is: its working is real while it runs and leaves no
/// evidence behind, so a solid outline would claim more than the form does.
fn dash_dotted_path(painter: &egui::Painter, points: &[Pos2], scale: f32, stroke: UiStroke) {
    let dash = (7.0 * scale).clamp(3.0, 11.0);
    let dot = (1.6 * scale).clamp(1.0, 3.0);
    let gap = (3.2 * scale).clamp(2.0, 5.0);
    // dash, gap, dot, gap — and then round again.
    let pattern = [(dash, true), (gap, false), (dot, true), (gap, false)];

    let (mut step, mut left) = (0usize, pattern[0].0);
    for pair in points.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let delta = to - from;
        let length = delta.length();
        if !length.is_finite() || length <= f32::EPSILON {
            continue;
        }
        let direction = delta / length;
        let mut travelled = 0.0;
        while travelled < length {
            let run = left.min(length - travelled);
            if pattern[step].1 {
                painter.line_segment(
                    [
                        from + direction * travelled,
                        from + direction * (travelled + run),
                    ],
                    stroke,
                );
            }
            travelled += run;
            left -= run;
            if left <= f32::EPSILON {
                step = (step + 1) % pattern.len();
                left = pattern[step].0;
            }
        }
    }
}

fn dashed_segment(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    dash: f32,
    gap: f32,
    stroke: UiStroke,
) {
    let delta = to - from;
    let length = delta.length();
    if !length.is_finite() || length <= f32::EPSILON {
        return;
    }
    let (dash, gap) = bounded_dash(length, dash, gap);
    let direction = delta / length;
    let mut travelled = 0.0;
    while travelled < length {
        let end = (travelled + dash).min(length);
        painter.line_segment(
            [from + direction * travelled, from + direction * end],
            stroke,
        );
        let step = dash + gap;
        if step <= f32::EPSILON {
            return;
        }
        travelled += step;
    }
}

fn dashed_rect(painter: &egui::Painter, rect: Rect, dash: f32, gap: f32, stroke: UiStroke) {
    let corners = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    for index in 0..corners.len() {
        dashed_segment(
            painter,
            corners[index],
            corners[(index + 1) % corners.len()],
            dash,
            gap,
            stroke,
        );
    }
}

fn dashed_ellipse(
    painter: &egui::Painter,
    centre: Pos2,
    radius: Vec2,
    dash: f32,
    gap: f32,
    stroke: UiStroke,
) {
    let circumference =
        std::f32::consts::TAU * ((radius.x * radius.x + radius.y * radius.y) * 0.5).sqrt();
    if !circumference.is_finite() {
        return;
    }
    let (dash, gap) = bounded_dash(circumference, dash, gap);
    let periods = (circumference / (dash + gap))
        .round()
        .clamp(6.0, MAX_DASHES) as usize;
    let dash_angle = std::f32::consts::TAU / periods as f32 * dash / (dash + gap).max(f32::EPSILON);
    let period = std::f32::consts::TAU / periods as f32;
    for index in 0..periods {
        let start = index as f32 * period;
        let mut previous = Pos2::new(
            centre.x + radius.x * start.cos(),
            centre.y + radius.y * start.sin(),
        );
        for step in 1..=4 {
            let angle = start + dash_angle * step as f32 / 4.0;
            let point = Pos2::new(
                centre.x + radius.x * angle.cos(),
                centre.y + radius.y * angle.sin(),
            );
            painter.line_segment([previous, point], stroke);
            previous = point;
        }
    }
}

/// Draw a connection as a right-angle circuit trace between two node centres,
/// with an arrowhead entering the target. The dominant axis decides whether the
/// trace leaves horizontally (H–V–H) or vertically (V–H–V), so it reads like a
/// board trace routed between components rather than a diagonal wire. The two
/// extents keep the trace outside resized symbols; `head` is the arrow size.
fn circuit_trace(
    painter: &egui::Painter,
    from: Pos2,
    to: Pos2,
    extents: [Vec2; 2],
    head: f32,
    stroke: UiStroke,
    straight: bool,
) {
    for segment in circuit_trace_geometry(from, to, extents, head, straight) {
        painter.line_segment(segment, stroke);
    }
}

/// Screen-neutral line segments for one operator trace. PDF export consumes
/// this exact geometry as the canvas painter, including resize clearance,
/// right-angle routing, optional straight routing, and arrowhead barbs.
fn circuit_trace_geometry(
    from: Pos2,
    to: Pos2,
    extents: [Vec2; 2],
    head: f32,
    straight: bool,
) -> Vec<[Pos2; 2]> {
    let [from_extent, to_extent] = extents;
    let (dx, dy) = (to.x - from.x, to.y - from.y);
    if straight {
        // The default: one line from the source's boundary to the target's, so a
        // chain of hops reads as a chain. Barbs sweep back from the tip along it.
        let len = (dx * dx + dy * dy).sqrt().max(1.0);
        let (ux, uy) = (dx / len, dy / len);
        let from_clearance = ray_to_extent(from_extent, Vec2::new(ux, uy));
        let to_clearance = ray_to_extent(to_extent, Vec2::new(ux, uy));
        let p0 = Pos2::new(from.x + ux * from_clearance, from.y + uy * from_clearance);
        let p1 = Pos2::new(to.x - ux * to_clearance, to.y - uy * to_clearance);
        let mut segments = vec![[p0, p1]];
        for side in [-0.45f32, 0.45] {
            let (cs, sn) = (side.cos(), side.sin());
            let (bx, by) = (ux * cs - uy * sn, ux * sn + uy * cs);
            segments.push([p1, Pos2::new(p1.x - bx * head, p1.y - by * head)]);
        }
        return segments;
    }
    if dx.abs() >= dy.abs() {
        let dir = if dx >= 0.0 { 1.0 } else { -1.0 };
        let p0 = Pos2::new(from.x + dir * from_extent.x, from.y);
        let p1 = Pos2::new(to.x - dir * to_extent.x, to.y);
        let mid = (p0.x + p1.x) * 0.5;
        vec![
            [p0, Pos2::new(mid, p0.y)],
            [Pos2::new(mid, p0.y), Pos2::new(mid, p1.y)],
            [Pos2::new(mid, p1.y), p1],
            [p1, Pos2::new(p1.x - dir * head, p1.y - head * 0.6)],
            [p1, Pos2::new(p1.x - dir * head, p1.y + head * 0.6)],
        ]
    } else {
        let dir = if dy >= 0.0 { 1.0 } else { -1.0 };
        let p0 = Pos2::new(from.x, from.y + dir * from_extent.y);
        let p1 = Pos2::new(to.x, to.y - dir * to_extent.y);
        let mid = (p0.y + p1.y) * 0.5;
        vec![
            [p0, Pos2::new(p0.x, mid)],
            [Pos2::new(p0.x, mid), Pos2::new(p1.x, mid)],
            [Pos2::new(p1.x, mid), p1],
            [p1, Pos2::new(p1.x - head * 0.6, p1.y - dir * head)],
            [p1, Pos2::new(p1.x + head * 0.6, p1.y - dir * head)],
        ]
    }
}

fn ray_to_extent(extent: Vec2, direction: Vec2) -> f32 {
    let x = if direction.x.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        extent.x / direction.x.abs()
    };
    let y = if direction.y.abs() <= f32::EPSILON {
        f32::INFINITY
    } else {
        extent.y / direction.y.abs()
    };
    x.min(y)
}

/// Screen direction of one world axis under the current orbit camera.
fn spatial_axis_direction(camera: SpatialCamera, axis: SpatialAxis) -> Vec2 {
    let (cy, sy) = (camera.yaw.cos(), camera.yaw.sin());
    let sp = camera.pitch.sin();
    let projected = match axis {
        SpatialAxis::X => Vec2::new(cy, sp * sy),
        SpatialAxis::Y => Vec2::new(0.0, camera.pitch.cos()),
        SpatialAxis::Z => Vec2::new(sy, -sp * cy),
        SpatialAxis::Free => Vec2::ZERO,
    };
    if projected.length_sq() <= 1e-5 {
        Vec2::RIGHT
    } else {
        projected.normalized()
    }
}

fn screen_segment_distance(point: Pos2, from: Pos2, to: Pos2) -> f32 {
    let segment = to - from;
    let length_sq = segment.length_sq();
    if length_sq <= f32::EPSILON {
        return point.distance(from);
    }
    let along = ((point - from).dot(segment) / length_sq).clamp(0.0, 1.0);
    point.distance(from + segment * along)
}

fn spatial_gizmo_hit(pointer: Pos2, centre: Pos2, camera: SpatialCamera) -> Option<SpatialAxis> {
    [SpatialAxis::X, SpatialAxis::Y, SpatialAxis::Z]
        .into_iter()
        .filter_map(|axis| {
            let direction = spatial_axis_direction(camera, axis);
            let distance = screen_segment_distance(
                pointer,
                centre + direction * 13.0,
                centre + direction * 68.0,
            );
            (distance <= 8.0).then_some((axis, distance))
        })
        .min_by(|left, right| left.1.total_cmp(&right.1))
        .map(|(axis, _)| axis)
}

fn paint_spatial_gizmo(
    painter: &egui::Painter,
    centre: Pos2,
    camera: SpatialCamera,
    k: Ink,
    length: f32,
) {
    for (axis, tone) in [
        (SpatialAxis::X, k.accent),
        (SpatialAxis::Y, k.secondary),
        (SpatialAxis::Z, k.ink),
    ] {
        let direction = spatial_axis_direction(camera, axis);
        let end = centre + direction * length;
        let stroke = UiStroke::new(2.0, tone);
        painter.line_segment([centre, end], stroke);
        let normal = Vec2::new(-direction.y, direction.x);
        painter.add(egui::Shape::convex_polygon(
            vec![
                end,
                end - direction * 9.0 + normal * 4.0,
                end - direction * 9.0 - normal * 4.0,
            ],
            tone,
            UiStroke::NONE,
        ));
        painter.text(
            end + direction * 8.0,
            Align2::CENTER_CENTER,
            axis.label(),
            FontId::monospace(10.0),
            tone,
        );
    }
    painter.circle_filled(centre, 4.0, k.fill);
    painter.circle_stroke(centre, 4.0, UiStroke::new(1.0, k.faint));
}

fn spatial_hit(mandala: &Mandala, projected: &[ProjectedNode], pointer: Pos2) -> Option<NodeId> {
    projected
        .iter()
        .filter_map(|projected_node| {
            let node = mandala.node(projected_node.id)?;
            // A complete flow is selected by its visible block-to-block edge,
            // not by an invisible cone-tree handle.
            if node.form.is_flow() {
                return None;
            }
            let scale = projected_node.scale.max(0.6);
            let resize = node_resize(mandala, node);
            let offset = (pointer - projected_node.position) / scale;
            node.shape()
                .contains(
                    f64::from(offset.x / resize.x.max(f32::EPSILON)),
                    f64::from(offset.y / resize.y.max(f32::EPSILON)),
                )
                .then_some((
                    node.id,
                    projected_node.position.distance(pointer),
                    projected_node.camera_depth,
                ))
        })
        .min_by(|left, right| {
            left.1
                .total_cmp(&right.1)
                .then_with(|| left.2.total_cmp(&right.2))
        })
        .map(|(id, _, _)| id)
}

/// Perspective projection for the derived structural model.
///
/// Camera math is kept outside the egui event handler so it stays a pure,
/// testable transformation. Existing canvas placement supplies X/Y while
/// structural depth supplies Z.
fn project_spatial(
    mandala: &Mandala,
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
            let extent = mandala.extent(node.id);
            // Framing follows the derived structure, not hand-moved pieces.
            // Otherwise every gizmo tick recentres/rescales the scene under
            // the pointer, making direct manipulation feel rubbery.
            let offset = mandala
                .node(node.id)
                .map(|node| node.spatial_offset)
                .unwrap_or([0.0; 3]);
            let base_x = node.x - offset[0];
            let base_y = node.y - offset[1];
            let base_z = node.z - offset[2];
            (
                min_x.min(base_x - extent.0),
                max_x.max(base_x + extent.0),
                min_y.min(base_y - extent.1),
                max_y.max(base_y + extent.1),
                min_z.min(base_z),
                max_z.max(base_z),
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
    // A fit has to fit. Holding it above a floor meant a drawing larger than
    // that floor allowed was scaled *up* and opened part-way off screen —
    // exactly what nesting every parenthesised form made routine. It still
    // refuses to magnify past natural size, so a small structure is shown at
    // full size in the middle rather than blown across the window.
    let fit = (available / span).clamp(f32::MIN_POSITIVE, 1.0);
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

fn spatial_layer_plates(
    mandala: &Mandala,
    layout: &SpatialLayout,
    projected: &[ProjectedNode],
) -> Vec<SpatialLayerPlate> {
    let mut depths = layout
        .nodes
        .iter()
        .map(|node| node.depth)
        .collect::<Vec<_>>();
    depths.sort_unstable();
    depths.dedup();

    let mut plates = depths
        .into_iter()
        .filter_map(|depth| {
            let mut count = 0usize;
            let mut min = Pos2::new(f32::INFINITY, f32::INFINITY);
            let mut max = Pos2::new(f32::NEG_INFINITY, f32::NEG_INFINITY);
            let mut camera_depth = 0.0f32;
            for spatial in layout.nodes.iter().filter(|node| node.depth == depth) {
                let Some(node) = projected.iter().find(|node| node.id == spatial.id) else {
                    continue;
                };
                let extent = mandala.extent(node.id);
                let pad = Vec2::new(extent.0 as f32, extent.1 as f32) * node.scale;
                min.x = min.x.min(node.position.x - pad.x);
                min.y = min.y.min(node.position.y - pad.y);
                max.x = max.x.max(node.position.x + pad.x);
                max.y = max.y.max(node.position.y + pad.y);
                camera_depth += node.camera_depth;
                count += 1;
            }
            if count == 0 {
                return None;
            }
            let centre = Pos2::new((min.x + max.x) * 0.5, (min.y + max.y) * 0.5 + 12.0);
            Some(SpatialLayerPlate {
                depth,
                centre,
                radius: Vec2::new(
                    ((max.x - min.x) * 0.5 + 34.0).max(76.0),
                    ((max.y - min.y) * 0.5 + 18.0).max(30.0),
                ),
                camera_depth: camera_depth / count as f32,
            })
        })
        .collect::<Vec<_>>();
    // Paint far receiving planes first, matching the node painter's order.
    plates.sort_by(|left, right| right.camera_depth.total_cmp(&left.camera_depth));
    plates
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
        let tone = if state.is_valid() { k.accent } else { k.danger };
        ui.colored_label(tone, state.mark());
        let detail = state.detail();
        if !detail.is_empty() {
            ui.colored_label(tone, detail);
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
/// Symbols carry green, operators and delimiters blue, invalid syntax is red,
/// and prompt text stays ink. Extracted so the Source tab, the mandala's source
/// box, and the run draft cannot drift into three different-looking editors.
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
            // Operators and delimiters share blue, as the
            // language legend in the top bar does, parentheses included.
            SourceHighlight::Forward
            | SourceHighlight::Backflow
            | SourceHighlight::Mediate
            | SourceHighlight::Import
            | SourceHighlight::Invert
            | SourceHighlight::Model
            | SourceHighlight::Parenthesis => ink.secondary,
            SourceHighlight::Whitespace | SourceHighlight::Comment => ink.faint,
            // Symbols carry green; invalid syntax is red; prompt text stays ink.
            SourceHighlight::Atom => ink.accent,
            SourceHighlight::Invalid => ink.danger,
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
    let mut job = rebis_layout_job(
        source,
        ink,
        14.0,
        &selected,
        pane.editor.matching_parentheses(),
    );
    // A long line wraps onto the next row rather than running off the right edge,
    // and the gutter beside it keeps a wrapped row distinguishable from a new
    // source line.
    let gutter = gutter_px(ui, pane.editor.line_count());
    job.wrap.max_width = (ui.available_width() - gutter - 8.0).max(120.0);
    let galley = ui.painter().layout_job(job);
    let viewport_height = ui.available_height().max(260.0);
    let interaction = egui::ScrollArea::vertical()
        .id_salt("visual_vim_source")
        .auto_shrink([false, false])
        .max_height(viewport_height)
        .show(ui, |ui| {
            let desired = Vec2::new(
                (galley.size().x + gutter).max(ui.available_width()),
                galley.size().y.max(viewport_height - 8.0),
            );
            let (rect, response) = ui.allocate_exact_size(desired, Sense::click_and_drag());
            ui.painter().rect_filled(rect, 0.0, ink.ground);
            // The text starts to the right of the gutter, and the numbers are
            // painted into the gutter rather than laid out with the text — so no
            // drag over the code can reach them and no copy can contain them.
            let text_origin = rect.min + Vec2::new(gutter, 0.0);
            ui.painter().galley(text_origin, galley.clone(), ink.ink);
            paint_galley_line_numbers(
                ui,
                ink,
                Rect::from_min_size(rect.min, Vec2::new(gutter, rect.height())),
                &galley,
                rect.min.y,
            );
            if response.has_focus() {
                let cursor =
                    galley.pos_from_ccursor(egui::text::CCursor::new(pane.editor.cursor()));
                let x = rect.left() + gutter + cursor.left();
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
                .cursor_from_pos(pointer - rect.min - Vec2::new(gutter, 0.0))
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
/// Keep a text box focused when Escape is pressed.
///
/// egui treats Escape as focus navigation and surrenders a `TextEdit`'s focus over
/// it before the widget is polled. In a code box that reads as the caret FREEZING:
/// it stays where it was, and every key after it goes nowhere, until the box is
/// clicked again. Nothing in this editor wants that — Escape means "stop what I am
/// doing", never "stop typing".
///
/// The filter has to be installed every frame; it takes hold from the frame after
/// the box is first focused, which is soon enough because the surrender it prevents
/// can only happen while the box already has focus.
fn keep_focus_over_escape(ui: &egui::Ui, id: egui::Id) {
    ui.memory_mut(|memory| {
        memory.set_focus_lock_filter(
            id,
            egui::EventFilter {
                escape: true,
                // Left alone: Tab and the arrows keep egui's ordinary meaning in a
                // plain text box, which is what a reader expects of one.
                tab: false,
                horizontal_arrows: true,
                vertical_arrows: true,
            },
        );
    });
}

/// Width in pixels the line-number gutter needs for a document of `lines` lines.
///
/// Measured in the monospace face the numbers are drawn in, so the rule is the
/// same one the terminal uses — [`kaos_workspace::rebis_workspace::gutter_width`]
/// digits, plus a little air on each side of them.
fn gutter_px(ui: &egui::Ui, lines: usize) -> f32 {
    let digits = kaos_workspace::rebis_workspace::gutter_width(lines.max(1));
    let glyph = ui.fonts(|fonts| fonts.glyph_width(&FontId::monospace(GUTTER_PX), '0'));
    glyph * digits as f32 + 10.0
}

/// Write the line numbers beside a wrapped code box.
///
/// Painted from the laid-out galley rather than inserted into the text, which is
/// what makes them unselectable: they are not text the `TextEdit` owns, so a drag
/// across the code cannot reach them and a copy cannot contain them.
///
/// A number appears on the row that STARTS a source line and on no other, so a
/// long line that wrapped over three rows is numbered once. The eye reads "still
/// the same line" from the absence — the same rule the terminal gutter follows.
fn paint_line_numbers(
    ui: &egui::Ui,
    k: Ink,
    gutter: Rect,
    output: &egui::text_edit::TextEditOutput,
) {
    let galley = &output.galley;
    let origin = output.galley_pos;
    let font = FontId::monospace(GUTTER_PX);
    // Which source line each row begins, and whether it begins one at all: a row
    // starts a line exactly when the row above it ended with a newline.
    let mut line = 0usize;
    let painter = ui.painter().with_clip_rect(gutter);
    for (index, row) in galley.rows.iter().enumerate() {
        if index > 0 {
            // `ends_with_newline` is the galley's own record of where the text
            // broke because the author pressed return, as opposed to where it ran
            // out of width.
            if galley.rows[index - 1].ends_with_newline {
                line += 1;
            } else {
                // A continuation of the line above: no number, and deliberately
                // nothing else either.
                continue;
            }
        }
        let y = origin.y + row.rect.center().y;
        painter.text(
            Pos2::new(gutter.right() - 5.0, y),
            Align2::RIGHT_CENTER,
            format!("{}", line + 1),
            font.clone(),
            k.faint,
        );
    }
}

/// Write line numbers beside a galley the caller paints itself.
///
/// The same rule as [`paint_line_numbers`], for the Source tab's hand-painted vim
/// editor: a number on the row that starts a source line and on no other row, so a
/// line wrapped over three rows is numbered once and the absence of a number is
/// what says "still the same line".
fn paint_galley_line_numbers(ui: &egui::Ui, k: Ink, gutter: Rect, galley: &egui::Galley, top: f32) {
    let font = FontId::monospace(GUTTER_PX);
    let painter = ui.painter().with_clip_rect(gutter);
    let mut line = 0usize;
    for (index, row) in galley.rows.iter().enumerate() {
        if index > 0 {
            if galley.rows[index - 1].ends_with_newline {
                line += 1;
            } else {
                continue;
            }
        }
        painter.text(
            Pos2::new(gutter.right() - 6.0, top + row.rect.center().y),
            Align2::RIGHT_CENTER,
            format!("{}", line + 1),
            font.clone(),
            k.faint,
        );
    }
}

fn truncate(label: &str) -> String {
    truncate_chars(label, 11)
}

/// The one token that stands for a form inside a child row's preview.
///
/// The canvas glyph wherever there is one, so the list reads as the drawing
/// does; the form's own word where the drawing says it with the outline alone —
/// a bare `( )` and a quote's sigil draw nothing to borrow.
fn preview_token(node: &Node) -> String {
    let glyph = node.glyph();
    if !glyph.is_empty() {
        return truncate(&glyph);
    }
    match node.form {
        Form::Quote => "'".to_string(),
        Form::Unquote => ",".to_string(),
        Form::Compose => "( )".to_string(),
        _ => node.form.name().to_string(),
    }
}

/// Model selectors sit outside their shapes, so retain enough of the provider
/// and model name to distinguish nearby per-block overrides.
fn truncate_model(label: &str) -> String {
    truncate_chars(label, 28)
}

/// The readout for a scale with no stop at either end.
///
/// Percent while a percent still says something; scientific once the wheel has
/// carried the canvas past where "0%" or a seven-digit percentage would be the
/// only thing left to print.
fn zoom_label(zoom: f64) -> String {
    if (0.001..10_000.0).contains(&zoom) {
        format!("{:.0}%", zoom * 100.0)
    } else {
        format!("{zoom:.2e}×")
    }
}

fn truncate_chars(label: &str, max: usize) -> String {
    if label.chars().count() <= max {
        return label.to_string();
    }
    let head: String = label.chars().take(max - 1).collect();
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
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(width, 900.0),
            )),
            ..Default::default()
        };
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

    #[test]
    fn every_tab_kind_has_a_detached_window_action() {
        let panes = vec![
            Pane::Mandala(Doc::default()),
            Pane::Chat(ChatPane::default()),
            Pane::Ink(InkPane::default()),
            Pane::Sigils(SigilPane::default()),
            Pane::Source(SourcePane::default()),
            Pane::Settings(settings::SettingsPane::default()),
            Pane::Automata(Box::new(
                AutomataPane::from_source("([m] \"one\" \"two\")", "test")
                    .expect("test source should build a generation"),
            )),
            Pane::Music(Box::new(music::MusicPane {
                desk: kaos_core::music::Desk::default(),
                origin: "test".to_string(),
                from: None,
                reading: None,
                looping: false,
            })),
            Pane::Runs,
            Pane::Actions,
        ];
        assert!(panes.iter().all(can_tear));
    }

    #[test]
    fn model_work_stream_becomes_collapsible_sections() {
        let lines = vec![
            "\u{1e}FOLD_OPEN\u{1f}model turn 1 · working".to_string(),
            "model    reading the task".to_string(),
            "\u{1e}FOLD_OPEN\u{1f}step 1 in full · read file".to_string(),
            "  contents".to_string(),
            "\u{1e}FOLD_CLOSE".to_string(),
            "\u{1e}FOLD_CLOSE".to_string(),
            "chat    final answer".to_string(),
            "done".to_string(),
        ];
        let sections = stream_sections(&lines);
        assert!(matches!(sections[0], StreamSection::Fold { .. }));
        assert!(
            matches!(sections[1], StreamSection::Line(ref line) if line == "chat    final answer")
        );
        assert!(matches!(sections[2], StreamSection::Line(ref line) if line == "done"));
    }

    #[test]
    fn fitted_shape_text_keeps_every_character_and_grows_with_the_shape() {
        let text = "a-complete-prompt-that-must-never-be-trimmed";
        let compact = fit_caption(text, Shape::Hexagon, Vec2::new(24.0, 18.0), MAX_GLYPH_PX);
        let expanded = fit_caption(text, Shape::Hexagon, Vec2::new(120.0, 90.0), MAX_GLYPH_PX);

        assert_eq!(compact.lines.concat(), text);
        assert!(!compact.lines.iter().any(|line| line.contains('…')));
        assert!(
            expanded.font_size > compact.font_size,
            "resizing the shape should give its complete text more room"
        );
    }

    #[test]
    fn zooming_reuses_glyph_sizes_instead_of_asking_for_a_new_one_every_frame() {
        // The font atlas caches by exact size and never evicts, so a caption
        // sized straight from the zoom asks for a brand-new rasterisation on
        // every frame of a wheel-zoom. Every size the canvas requests must land
        // on the shared ladder, which is what makes zooming reuse the atlas.
        let mut asked = std::collections::HashSet::new();
        for frame in 0..600 {
            let smooth = MIN_GLYPH_PX + (MAX_GLYPH_PX - MIN_GLYPH_PX) * frame as f32 / 599.0;
            let settled = glyph_px(smooth);
            asked.insert(settled.to_bits());
            // Never above what was asked for: text that fitted still fits.
            assert!(
                settled <= smooth * (1.0 + 1e-5),
                "{settled} exceeds the {smooth} it settled from"
            );
            assert!((MIN_GLYPH_PX..=MAX_GLYPH_PX).contains(&settled));
            // Idempotent, so a size that has already settled stays put.
            assert_eq!(glyph_px(settled).to_bits(), settled.to_bits());
        }
        assert!(
            asked.len() < 120,
            "a full zoom sweep asked for {} distinct glyph sizes",
            asked.len()
        );

        // A fitted caption settles on the ladder too — the binary search lands
        // on an arbitrary fraction of a pixel otherwise.
        let layout = fit_caption(
            "prompt",
            Shape::Hexagon,
            Vec2::new(97.3, 61.7),
            MAX_GLYPH_PX,
        );
        assert_eq!(
            layout.font_size.to_bits(),
            glyph_px(layout.font_size).to_bits()
        );

        // Nothing pathological gets through to the rasterizer.
        for bad in [f32::NAN, f32::INFINITY, -1.0, 0.0, 1e30] {
            let settled = glyph_px(bad);
            assert!(settled.is_finite() && (MIN_GLYPH_PX..=MAX_GLYPH_PX).contains(&settled));
        }
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
    fn symbols_use_green_and_operators_use_blue_in_every_editor() {
        // One layout function serves the Source tab, the mandala's source box,
        // and the run draft, so pinning it here pins all three.
        let k = crate::theme::Ink::load();
        //             0 12 3 45 6 789 10
        let job = rebis_layout_job("(-> ab \"p\")", k, 14.0, &|_| false, None);
        let colour = |i: usize| job.sections[i].format.color;
        for (i, what) in [(0, "("), (1, "-"), (2, ">"), (10, ")")] {
            assert_eq!(colour(i), k.secondary, "operator {what} must use blue");
        }
        for i in [4, 5] {
            assert_eq!(colour(i), k.accent, "a symbol must use green");
        }
        // The quote marks themselves are classified Atom by the shared
        // highlighter, so they take symbol green; the text between them is
        // Prompt and stays ink.
        assert_eq!(colour(8), k.ink, "prompt text stays ink");
        for i in [7, 9] {
            assert_eq!(colour(i), k.accent, "quote marks follow the symbols");
        }
        assert_ne!(k.accent, k.secondary);
    }

    #[test]
    fn retained_output_is_split_into_semantic_labels_without_changing_its_body() {
        let k = crate::theme::Ink::load();
        assert_eq!(StreamKind::Success.tone(k), k.accent);
        assert_eq!(StreamKind::Signal.tone(k), k.secondary);
        assert_eq!(StreamKind::Caution.tone(k), k.danger);
        assert_eq!(
            stream_line("event    prompt started · abstraction 2"),
            StreamLine {
                tag: Some("event"),
                body: "prompt started · abstraction 2",
                kind: StreamKind::Signal,
            }
        );
        assert_eq!(
            stream_line("complete    ✓ run finished"),
            StreamLine {
                tag: Some("complete"),
                body: "✓ run finished",
                kind: StreamKind::Success,
            }
        );
        assert_eq!(
            stream_line("paused      process exited 2"),
            StreamLine {
                tag: Some("paused"),
                body: "process exited 2",
                kind: StreamKind::Caution,
            }
        );
        assert_eq!(
            stream_line("model       failed · provider unavailable"),
            StreamLine {
                tag: Some("model"),
                body: "failed · provider unavailable",
                kind: StreamKind::Caution,
            }
        );
        assert_eq!(
            stream_line("provider prose remains intact"),
            StreamLine {
                tag: None,
                body: "provider prose remains intact",
                kind: StreamKind::Plain,
            }
        );
    }

    #[test]
    fn a_message_typed_mid_answer_waits_its_turn_instead_of_starting_a_second_one() {
        use kaos_core::sessions::Role;
        let mut editor = Editor::new(Mandala::new());
        let chat = ChatPane {
            browsing: false,
            queued: vec!["and the dead letters?".into(), "with an example".into()],
            ..ChatPane::default()
        };
        let session = chat.session.id.clone();
        editor.tabs.open("chat", Pane::Chat(chat));

        // Nothing queued has been asked yet, so nothing queued is in the
        // transcript — it is waiting, not said.
        let turns = |editor: &Editor, session: &str| {
            editor
                .tabs
                .iter()
                .find_map(|tab| match &tab.content {
                    Pane::Chat(chat) if chat.session.id == session => Some(
                        chat.session
                            .turns
                            .iter()
                            .map(|turn| (turn.role, turn.text.clone()))
                            .collect::<Vec<_>>(),
                    ),
                    _ => None,
                })
                .unwrap_or_default()
        };
        assert!(turns(&editor, &session).is_empty());

        editor.dispatch_queued_chat();

        // They go out together, in the order they were written, as one turn.
        assert_eq!(
            turns(&editor, &session),
            vec![(
                Role::User,
                "and the dead letters?\n\nwith an example".to_string()
            )],
            "queued messages are asked as one turn, in order"
        );
        let drained = editor
            .tabs
            .iter()
            .find_map(|tab| match &tab.content {
                Pane::Chat(chat) if chat.session.id == session => Some(chat.queued.len()),
                _ => None,
            })
            .unwrap();
        assert_eq!(drained, 0, "and the queue is emptied by sending them");
    }

    #[test]
    fn visual_chat_separates_fenced_code_from_readable_prose() {
        assert_eq!(
            chat_blocks("A result:\n```rust\nfn main() {}\n```\nDone."),
            vec![
                ChatBlock::Prose("A result:".to_string()),
                ChatBlock::Code {
                    language: "rust".to_string(),
                    text: "fn main() {}".to_string(),
                },
                ChatBlock::Prose("Done.".to_string()),
            ]
        );
        assert_eq!(
            chat_blocks("```\nunfinished"),
            vec![ChatBlock::Code {
                language: String::new(),
                text: "unfinished".to_string(),
            }]
        );
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
    fn a_flow_arrow_keeps_its_blue_when_the_block_is_selected() {
        // Nesting has no line of its own. Blue is therefore reserved for the
        // one remaining connection: an explicit flow operator.
        let k = crate::theme::Ink::load();
        assert_eq!(edge_colour(k), k.secondary);
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
    fn a_program_that_does_not_parse_still_opens_as_source() {
        // Refusing at the door sent the reader back to whatever they were using
        // before — at exactly the moment the editor is the tool that helps.
        let broken = "(-> \"unclosed";
        let opened = open(broken);
        let Opened::Source { text, path, error } = opened else {
            panic!("a broken program must open as source, not be refused");
        };
        assert_eq!(text, broken, "the source is carried through verbatim");
        assert_eq!(path, None, "inline source came from no file");
        assert!(!error.is_empty(), "and it says what is wrong");

        // It lands in a Source tab, with the diagnostic on it, beside a canvas
        // the repaired program can be drawn on.
        let editor = Editor::opened(open(broken));
        let titles: Vec<&str> = editor.tabs.iter().map(|tab| tab.title.as_str()).collect();
        assert_eq!(titles, vec!["mandala", "source"]);
        let Some(Pane::Source(pane)) = editor.tabs.active() else {
            panic!("the source tab is the one to look at: {titles:?}");
        };
        assert_eq!(pane.editor.source(), broken);
        assert!(
            pane.notice.as_deref().is_some_and(|n| n.contains("parse")),
            "the pane says why there is no drawing: {:?}",
            pane.notice
        );
        assert!(editor.notice.is_some(), "and so does the window");
    }

    #[test]
    fn a_broken_file_remembers_where_to_save_the_repair() {
        let path =
            std::env::temp_dir().join(format!("kaos-visual-broken-{}.rebis", std::process::id()));
        std::fs::write(&path, "(-> \"unclosed").expect("write");

        let editor = Editor::opened(open(&path.display().to_string()));
        let Some(Pane::Source(pane)) = editor.tabs.active() else {
            panic!("a broken file opens in the source tab");
        };
        assert_eq!(
            pane.file_path,
            path.display().to_string(),
            "the repair saves back to the file it came from"
        );
        // The tab is named for the file, not for the fact that it is broken.
        assert_eq!(
            editor.tabs.active_id().and_then(|id| editor
                .tabs
                .iter()
                .find(|tab| tab.id == id)
                .map(|tab| tab.title.clone())),
            path.file_name().map(|n| n.to_string_lossy().into_owned())
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_program_that_parses_still_opens_as_a_drawing() {
        // The repair is the point: once it parses it is a drawing again.
        let Opened::Drawing(mandala) = open("(-> \"a\" \"b\")") else {
            panic!("a valid program is a drawing");
        };
        assert_eq!(mandala.nodes().len(), 3);

        let editor = Editor::opened(open("(-> \"a\" \"b\")"));
        assert_eq!(editor.tabs.len(), 1, "no source tab is needed");
        assert!(matches!(editor.tabs.active(), Some(Pane::Mandala(_))));
        assert!(editor.notice.is_none(), "nothing to report");

        // And an empty argument is an empty canvas, as before.
        assert!(matches!(open("  "), Opened::Drawing(_)));
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
        let band = grab_band(doc.view);
        for x in [half_w - 1.0, half_w, half_w + 1.0] {
            let wall = Drag::beginning_at(&doc.mandala, x, 0.0, band);
            let Drag::Resize(grab) = wall else {
                panic!("the wall at {x} must begin a resize, not a pan or a move");
            };
            assert_eq!(grab.id, square);
            assert!(grab.wide && !grab.tall);
        }

        // In the middle: the box moves, exactly as before.
        assert!(
            matches!(
                Drag::beginning_at(&doc.mandala, 0.0, 0.0, band),
                Drag::Node { id, .. } if id == square
            ),
            "the middle of the box still moves it"
        );

        // Off the drawing: the canvas pans.
        assert!(matches!(
            Drag::beginning_at(&doc.mandala, 900.0, 900.0, band),
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
    fn a_compose_drag_resizes_from_its_circular_border() {
        let mut doc = Doc::default();
        let compose = doc.mandala.add(Form::Compose, "", 40.0, -20.0);
        let radius = doc.mandala.extent(compose).0;

        for angle in [
            0.0,
            std::f64::consts::FRAC_PI_4,
            std::f64::consts::FRAC_PI_2,
        ] {
            let drag = Drag::beginning_at(
                &doc.mandala,
                40.0 + radius * angle.cos(),
                -20.0 + radius * angle.sin(),
                grab_band(doc.view),
            );
            let Drag::Resize(grab) = drag else {
                panic!("the circumference must begin a radial resize");
            };
            assert_eq!(grab.id, compose);
            assert!(grab.wide && grab.tall);
        }
    }

    #[test]
    fn a_wall_stays_the_same_thickness_under_the_pointer_at_every_zoom() {
        let mut doc = Doc::default();
        let square = doc.mandala.add(Form::Square, "", 0.0, 0.0);
        let (half_w, _) = doc.mandala.extent(square);

        // The band is a screen measurement. Two pixels outside the drawn wall is
        // a resize whether the canvas is magnified or pushed away — before this,
        // the band was fixed in world units, so zooming out put the wall out of
        // reach and zooming in swallowed the whole shape into its own border.
        for zoom in [0.2, 1.0, 6.0, 1_000.0, 1e9] {
            doc.view.zoom = zoom;
            let band = grab_band(doc.view);
            let outside = half_w + 2.0 / zoom;
            assert!(
                matches!(
                    Drag::beginning_at(&doc.mandala, outside, 0.0, band),
                    Drag::Resize(grab) if grab.id == square
                ),
                "two pixels outside the wall must resize at zoom {zoom}"
            );
            // And the centre never becomes wall, however wide the band would be
            // in world units.
            assert!(
                matches!(
                    Drag::beginning_at(&doc.mandala, 0.0, 0.0, band),
                    Drag::Node { id, .. } if id == square
                ),
                "the middle of the box must still move it at zoom {zoom}"
            );
        }
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
    fn every_flow_operator_is_drawn_as_the_line_between_its_two_operands() {
        let mut mandala = Mandala::new();
        let square = mandala.add(Form::Square, "", 0.0, 0.0);
        let circle = mandala.add(Form::Compose, "", 200.0, 0.0);
        let flow = mandala.flow(square, circle, Form::Forward).unwrap();
        let leaf_a = mandala.add(Form::Prompt, "a", 0.0, 200.0);
        let leaf_b = mandala.add(Form::Prompt, "b", 200.0, 200.0);
        let leaf_flow = mandala.flow(leaf_a, leaf_b, Form::Forward).unwrap();
        // Stored syntax still has ordered operand relations, and none of those
        // relations becomes an invented line.
        mandala.father_of(circle, leaf_a);

        let edges = visible_edges(&mandala);
        let drawn = edges
            .iter()
            .map(|edge| (edge.from, edge.to, edge.owner))
            .collect::<Vec<_>>();
        // A flow between two leaves is a connection just as much as one between
        // two boundaries. Drawing only the latter left the former as an arrow
        // glyph standing between its own operands.
        assert_eq!(drawn.len(), 2);
        assert!(drawn.contains(&(square, circle, flow)));
        assert!(drawn.contains(&(leaf_a, leaf_b, leaf_flow)));
        // The plain parent relation invented nothing.
        assert!(!drawn
            .iter()
            .any(|(from, to, _)| *from == circle && *to == leaf_a));
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
    fn selected_symbol_copy_paste_keeps_every_hand_set_size() {
        let mut doc = Doc::default();
        let circle = doc.mandala.add(Form::Compose, "", 0.0, 0.0);
        let square = doc.mandala.add(Form::Square, "", 240.0, 0.0);
        let prompt = doc.mandala.add(Form::Prompt, "large", 480.0, 0.0);
        doc.mandala.resize(circle, 170.0, 100.0);
        doc.mandala.resize(square, 190.0, 120.0);
        doc.mandala.resize(prompt, 88.0, 64.0);
        doc.select_many([circle, square, prompt], false);

        let copied = doc.copied_selection().unwrap();
        let pasted = doc.paste_graph(&copied, (28.0, 28.0));

        assert_eq!(
            doc.mandala.node(pasted[0]).unwrap().size,
            Some((170.0, 170.0))
        );
        assert_eq!(
            doc.mandala.node(pasted[1]).unwrap().size,
            Some((190.0, 120.0))
        );
        // A hexagon scales whole, so the taller of the two requested factors
        // governs both axes and the form keeps its own proportions.
        let base = doc.mandala.node(pasted[2]).unwrap().base_extent();
        let scale = (88.0f64 / base.0).max(64.0f64 / base.1);
        let kept = doc.mandala.node(pasted[2]).unwrap().size.unwrap();
        assert!(
            (kept.0 - base.0 * scale).abs() < 1e-9 && (kept.1 - base.1 * scale).abs() < 1e-9,
            "the pasted hexagon kept {kept:?} rather than its own proportions"
        );
    }

    #[test]
    fn selecting_and_copying_a_container_keeps_its_visual_content() {
        let mut doc = Doc::default();
        let circle = doc.mandala.add(Form::Compose, "", 0.0, 0.0);
        let content = doc.mandala.add(Form::Prompt, "inside", 30.0, 0.0);
        doc.mandala.hold(circle, content);
        doc.select_only(circle);
        assert_eq!(doc.selected_ids(), BTreeSet::from([circle, content]));

        let copied = doc.copied_selection().unwrap();
        let pasted = doc.paste_graph(&copied, (28.0, 28.0));

        assert_eq!(pasted.len(), 2);
        assert_eq!(doc.mandala.holder(pasted[1]), Some(pasted[0]));
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
        let mut mandala = Mandala::new();
        let first = mandala.add(Form::Prompt, "first", 0.0, 0.0);
        let second = mandala.add(Form::Prompt, "second", 0.0, 0.0);
        let layout = SpatialLayout {
            nodes: vec![
                kaos_core::visual::SpatialNode {
                    id: first,
                    x: 0.0,
                    y: 0.0,
                    z: 0.0,
                    depth: 0,
                    recursive: false,
                },
                kaos_core::visual::SpatialNode {
                    id: second,
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
        let projected = project_spatial(&mandala, &layout, rect, camera);
        assert_eq!(projected.len(), 2);
        assert_ne!(projected[0].position, projected[1].position);
        assert_ne!(projected[0].camera_depth, projected[1].camera_depth);
    }

    #[test]
    fn the_3d_move_gizmo_exposes_each_world_axis_after_orbiting() {
        let camera = SpatialCamera {
            yaw: 0.71,
            pitch: 0.43,
            ..SpatialCamera::default()
        };
        let centre = Pos2::new(200.0, 180.0);
        for axis in [SpatialAxis::X, SpatialAxis::Y, SpatialAxis::Z] {
            let direction = spatial_axis_direction(camera, axis);
            assert!((direction.length() - 1.0).abs() < 1e-5);
            let pointer = centre + direction * 48.0;
            assert_eq!(spatial_gizmo_hit(pointer, centre, camera), Some(axis));
        }
    }

    #[test]
    fn the_palette_scrolls_rather_than_hiding_its_tools_in_a_short_window() {
        // Reserving a fixed slice below the forms and scrolling only the forms
        // meant a short window squeezed that slice until `connect flow`,
        // `select` and `drag` sat below the panel with no way to reach them.
        // Everything is in one scroll area now, so a palette taller than its
        // panel scrolls instead of clipping.
        //
        // Laid out at a height far too short for the whole thing: every tool
        // must survive the pass, since the list the palette renders from is the
        // one the canvas dispatches on.
        for tool in [
            Tool::Select,
            Tool::Pan,
            Tool::Flow(Form::Forward),
            Tool::Place(Form::Compose),
            Tool::Place(Form::Quote),
        ] {
            for height in [240.0f32, 400.0, 1000.0] {
                let mut editor = Editor::new(Mandala::new());
                editor.tool = tool.clone();
                let ctx = egui::Context::default();
                let input = egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1100.0, height),
                    )),
                    ..Default::default()
                };
                // Two passes: egui sizes a panel from the previous frame.
                for _ in 0..2 {
                    let _ = ctx.run(input.clone(), |ctx| editor.palette(ctx));
                }
                assert_eq!(
                    editor.tool, tool,
                    "{tool:?} did not survive the palette at {height}px"
                );
            }
        }
    }

    #[test]
    fn source_line_numbers_are_chrome_the_text_cannot_own() {
        // The numbers are painted into a reserved gutter from the laid-out galley,
        // never inserted into the document. So they cannot be dragged over, cannot
        // be copied, and — the reason for doing it this way — a wrapped row shows
        // no number at all, which is what says it is still the same line.
        let long = "(\"a prompt long enough that it has to wrap onto another row to be read\")";
        let source = format!("{long}\nx");

        // The document itself never gains a digit or a gutter rule.
        let pane = SourcePane {
            editor: kaos_workspace::rebis_workspace::Editor::new(&source),
            ..SourcePane::default()
        };
        assert_eq!(
            pane.editor.source(),
            source,
            "the gutter must not be part of the text"
        );

        // Laid out at a width narrower than the line, the galley really does wrap,
        // and exactly one of its rows starts each source line.
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(420.0, 600.0),
            )),
            ..Default::default()
        };
        let mut rows = 0usize;
        let mut numbered = Vec::new();
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let mut job = rebis_layout_job(&source, Ink::load(), 14.0, &|_| false, None);
                job.wrap.max_width = 240.0;
                let galley = ui.painter().layout_job(job);
                rows = galley.rows.len();
                let mut line = 0usize;
                for (index, _) in galley.rows.iter().enumerate() {
                    if index > 0 {
                        if galley.rows[index - 1].ends_with_newline {
                            line += 1;
                        } else {
                            continue;
                        }
                    }
                    numbered.push((index, line + 1));
                }
            });
        });
        assert!(rows > 2, "the long line did not wrap: {rows} rows");
        assert_eq!(
            numbered.iter().map(|(_, line)| *line).collect::<Vec<_>>(),
            vec![1, 2],
            "exactly one number per source line, whatever the wrapping: {numbered:?}"
        );
        assert_eq!(numbered[0].0, 0, "line 1 is numbered on the first row");
        assert_eq!(
            numbered[1].0,
            rows - 1,
            "line 2 is numbered on the row after the wrapped ones, not the second row"
        );
    }

    #[test]
    fn a_child_row_names_the_form_and_the_first_of_what_it_holds() {
        // Two macros under one parent both read `macro`, and the numbers beside
        // them could then only be permuted blind. The row has to carry what
        // tells them apart.
        let mandala = Mandala::from_rebis("((~ greet (name) ($ \"hello \" name)) (~ leave () x))")
            .expect("the fixture parses");
        let father = mandala
            .nodes()
            .iter()
            .find(|node| {
                node.form == Form::Compose
                    && mandala.children(node.id).iter().any(|child| {
                        matches!(
                            mandala.node(*child).map(|node| &node.form),
                            Some(Form::Function(_))
                        )
                    })
            })
            .expect("the two macros share a parent")
            .id;
        let children = mandala.children(father);
        let editor = Editor::new(mandala);

        let (name, inside) = editor.child_row(children[0]);
        assert_eq!(name, "macro greet (name)", "the macro is not named");
        assert_eq!(inside, "$", "the macro's body is not previewed");

        // A macro with no parameters says so by having none written, and its
        // body is a bare symbol rather than an indentation.
        let (name, inside) = editor.child_row(children[1]);
        assert_eq!(name, "macro leave", "empty parameters were written anyway");
        assert_eq!(inside, "x");
    }

    #[test]
    fn a_long_child_caption_can_be_unfolded_in_the_panel() {
        // The canvas shows one token per form because a drawing is a map. The
        // panel is where the whole of it is supposed to live — and it truncated
        // too, which left a long prompt with nowhere to be read.
        let long = "Fetch this pull request in full: its title, description, \
                    the complete diff, and every review comment";
        let mandala = Mandala::from_rebis(&format!("({long:?} x)")).expect("the fixture parses");
        let father = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Compose)
            .expect("the group is drawn")
            .id;
        let children = mandala.children(father);
        assert_eq!(children.len(), 2, "a long prompt and a short symbol");
        let child = children[0];

        let mut editor = Editor::new(mandala);
        let ctx = egui::Context::default();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1100.0, 900.0),
            )),
            ..Default::default()
        };
        // egui nests shapes, so the walk has to recurse; and it sizes a panel
        // from the previous frame, so the probe runs twice and reads the second.
        fn harvest(shape: &egui::epaint::Shape, into: &mut String) {
            match shape {
                egui::epaint::Shape::Text(t) => {
                    into.push_str(t.galley.text());
                    into.push('\n');
                }
                egui::epaint::Shape::Vec(shapes) => {
                    for shape in shapes {
                        harvest(shape, into);
                    }
                }
                _ => {}
            }
        }
        let shown = |editor: &mut Editor, ctx: &egui::Context| {
            let mut text = String::new();
            for pass in 0..2 {
                let output = ctx.run(input.clone(), |ctx| {
                    egui::SidePanel::right("probe").show(ctx, |ui| {
                        editor.child_order_editor(ui, father, true);
                    });
                });
                if pass == 1 {
                    for shape in &output.shapes {
                        harvest(&shape.shape, &mut text);
                    }
                }
            }
            text
        };

        // Folded: the token, and a control offered because there is more to see.
        let folded = shown(&mut editor, &ctx);
        assert!(
            !folded.contains("complete diff"),
            "the whole prompt was already shown: {folded}"
        );
        assert!(folded.contains('⏵'), "no unfold control offered: {folded}");
        // The short symbol gets no control: nothing is hidden behind it.
        assert_eq!(
            folded.matches('⏵').count(),
            1,
            "a caption that fits was decorated anyway: {folded}"
        );

        // Unfolded: every character.
        editor.unfolded.insert(child);
        let open = shown(&mut editor, &ctx);
        assert!(
            open.contains("complete diff") && open.contains("review comment"),
            "unfolding did not reveal the text: {open}"
        );
        assert!(open.contains('⏷'), "the control did not flip: {open}");

        // And it is view state: the drawing is untouched, so undo has nothing to
        // step back through.
        assert_eq!(
            editor.doc().mandala.node(child).unwrap().text,
            long,
            "unfolding edited the drawing"
        );
    }

    #[test]
    fn the_drag_tool_moves_the_view_and_leaves_the_drawing_alone() {
        use egui::{Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));

        // Dragged from the middle of a form, so a tool that moved the drawing
        // instead of the view would be plainly wrong.
        let run = |tool: Tool| {
            let mut editor = Editor::new(Mandala::from_rebis("(\"a\" \"b\")").unwrap());
            editor.tool = tool;
            let ctx = egui::Context::default();
            let mut origin = Pos2::ZERO;
            let frame = |editor: &mut Editor, events: Vec<Event>, origin: &mut Pos2| {
                let _ = ctx.run(
                    RawInput {
                        events,
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ctx| {
                        editor.header(ctx);
                        editor.tab_bar(ctx);
                        editor.palette(ctx);
                        editor.side(ctx);
                        editor.footer(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            *origin = ui.max_rect().min;
                            editor.canvas(ui);
                        });
                    },
                );
            };
            frame(&mut editor, Vec::new(), &mut origin);
            frame(&mut editor, Vec::new(), &mut origin);

            let held = editor.doc().mandala.nodes()[1].id;
            let node = editor.doc().mandala.node(held).unwrap();
            let (sx, sy) = editor.doc().view.to_screen(node.x, node.y);
            let start = origin + Vec2::new(sx as f32, sy as f32);
            let framed = editor.doc().view;
            let before = (node.x, node.y);

            frame(
                &mut editor,
                vec![
                    Event::PointerMoved(start),
                    Event::PointerButton {
                        pos: start,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::default(),
                    },
                ],
                &mut origin,
            );
            for step in [40.0f32, 90.0] {
                frame(
                    &mut editor,
                    vec![Event::PointerMoved(start + Vec2::splat(step))],
                    &mut origin,
                );
            }
            let after = editor.doc().mandala.node(held).map(|n| (n.x, n.y)).unwrap();
            (framed, editor.doc().view, before, after)
        };

        // Under the drag tool the view moves and the form does not.
        let (framed, view, before, after) = run(Tool::Pan);
        assert!(
            (view.tx - framed.tx).abs() > 1.0 || (view.ty - framed.ty).abs() > 1.0,
            "the drag tool did not move the view: ({}, {})",
            view.tx,
            view.ty
        );
        assert_eq!(before, after, "and it must not move the form it started on");

        // Under select, the same drag moves the form and leaves the view where
        // framing put it.
        let (framed, view, before, after) = run(Tool::Select);
        assert_ne!(before, after, "select drags the form");
        assert_eq!((view.tx, view.ty), (framed.tx, framed.ty));
    }

    #[test]
    fn the_models_mark_advance_is_never_narrower_than_the_real_face() {
        // The model reserves room for a mark from its character count, because it
        // has no font to measure. That estimate must be an OVER-estimate of the face
        // the mark is actually drawn in: too much leaves a gap, too little puts one
        // circle's name through its neighbour's.
        let ctx = egui::Context::default();
        let mut measured = 0.0_f32;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let font = FontId::monospace(MARK_PX);
                // Widest of the characters a mark is actually made of: sigils, and
                // the letters, digits and dashes of a callee's name.
                for character in "$?^%&~'-abcdefghijklmnopqrstuvwxyz0123456789".chars() {
                    measured = measured.max(ui.fonts(|fonts| fonts.glyph_width(&font, character)));
                }
            });
        });
        let assumed = kaos_core::visual::MARK_ADVANCE as f32 * MARK_PX;
        assert!(
            measured > 0.0,
            "no glyph was measured, so this proves nothing"
        );
        assert!(
            assumed >= measured,
            "the model assumes {assumed:.2}px per mark character but the face draws \
             up to {measured:.2}px — a long mark will overrun the room reserved for it"
        );
        // And not wildly generous, or every marked circle carries dead space.
        assert!(
            assumed <= measured * 1.35,
            "the estimate {assumed:.2}px is far wider than the face's {measured:.2}px"
        );
    }

    #[test]
    fn the_side_panel_reaches_its_bottom_in_a_short_window() {
        // The panel stacks a lot: a selection's form controls, its child order, its
        // arrow chain, its source, then the program's own source and its buttons. On a
        // short window the ones at the bottom were simply unreachable — the same
        // failure the palette had, in the pane that holds most of the controls.
        use egui::{Pos2, RawInput, Rect, Vec2};
        let mandala = Mandala::from_rebis("(-> \"a\" \"b\" \"c\")").expect("parses");
        let painted = |height: f32| {
            let mut editor = Editor::new(mandala.clone());
            let first = editor.doc().mandala.nodes()[1].id;
            editor.doc_mut().select_only(first);
            let ctx = egui::Context::default();
            let input = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(1100.0, height))),
                ..Default::default()
            };
            let mut text = String::new();
            // Two passes: egui sizes a panel from the previous frame.
            for pass in 0..2 {
                let output = ctx.run(input.clone(), |ctx| editor.side(ctx));
                if pass == 1 {
                    fn harvest(shape: &egui::epaint::Shape, into: &mut String) {
                        match shape {
                            egui::epaint::Shape::Text(t) => {
                                into.push_str(t.galley.text());
                                into.push('\n');
                            }
                            egui::epaint::Shape::Vec(shapes) => {
                                for shape in shapes {
                                    harvest(shape, into);
                                }
                            }
                            _ => {}
                        }
                    }
                    for shape in &output.shapes {
                        harvest(&shape.shape, &mut text);
                    }
                }
            }
            text
        };

        // Tall enough for everything: the last controls are painted.
        let tall = painted(1400.0);
        assert!(
            tall.contains("format mandala"),
            "even a tall panel did not reach its own bottom:\n{tall}"
        );

        // Short: the panel must still be scrollable rather than clipped, which egui
        // reports by the content being taller than the viewport. The section headings
        // near the top stay painted, and nothing panics laying it out.
        for height in [240.0f32, 400.0, 700.0] {
            let short = painted(height);
            assert!(
                short.contains("ARROW CHAIN") || short.contains("FORWARD"),
                "the panel painted nothing at {height}px:\n{short}"
            );
        }
    }

    #[test]
    fn escape_does_not_freeze_the_caret_in_a_text_box() {
        // egui surrenders a TextEdit's focus over Escape, which reads as the caret
        // freezing: it stays where it was and every key after it goes nowhere until
        // the box is clicked again. Escape means "stop what I am doing", never
        // "stop typing".
        use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let mut editor = Editor::new(Mandala::from_rebis("(\"a\")").unwrap());
        let ctx = egui::Context::default();
        let id = egui::Id::new("side_source_edit");
        let frame = |editor: &mut Editor, events: Vec<Event>| {
            let _ = ctx.run(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    editor.side(ctx);
                    // Somewhere for focus navigation to go, or the bug hides.
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let _ = ui.button("elsewhere");
                    });
                },
            );
        };
        frame(&mut editor, Vec::new());
        ctx.memory_mut(|memory| memory.request_focus(id));
        frame(&mut editor, Vec::new());
        assert!(
            ctx.memory(|memory| memory.has_focus(id)),
            "the box under test never took focus"
        );

        let before = editor.doc().text.clone();
        frame(
            &mut editor,
            vec![Event::Key {
                key: Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            }],
        );
        assert!(
            ctx.memory(|memory| memory.has_focus(id)),
            "Escape surrendered the caret"
        );

        // And the proof that matters: a key after Escape still lands.
        frame(&mut editor, vec![Event::Text("Z".into())]);
        assert_ne!(
            editor.doc().text,
            before,
            "typing after Escape went nowhere — the caret is frozen"
        );
        assert!(
            editor.doc().text.contains('Z'),
            "got {:?}",
            editor.doc().text
        );
    }

    #[test]
    fn a_chain_draws_one_arrow_per_hop_from_each_stage_to_the_next() {
        // `(-> a b c d)` is three hops: a→b, b→c, c→d. Every stage but the last is
        // the source of exactly one arrow and every stage but the first is the target
        // of exactly one, so no form has two arrows leaving it — which is what "all
        // the arrows part from the first item" looks like.
        let mandala = Mandala::from_rebis("(-> \"a\" \"b\" \"c\" \"d\")").expect("parses");
        let at = |text: &str| {
            mandala
                .nodes()
                .iter()
                .find(|node| node.text == text)
                .unwrap_or_else(|| panic!("no form {text:?}"))
                .id
        };
        let edges = visible_edges(&mandala);
        let mut hops: Vec<(NodeId, NodeId)> =
            edges.iter().map(|edge| (edge.from, edge.to)).collect();
        hops.sort_by_key(|(from, _)| from.0);
        assert_eq!(
            hops,
            vec![(at("a"), at("b")), (at("b"), at("c")), (at("c"), at("d")),],
            "the chain must be one arrow per hop"
        );
        for text in ["a", "b", "c"] {
            assert_eq!(
                edges.iter().filter(|edge| edge.from == at(text)).count(),
                1,
                "{text} is the source of more than one arrow"
            );
        }
        assert_eq!(edges.iter().filter(|edge| edge.from == at("d")).count(), 0);

        // And each hop is drawn as ONE straight run between the two boundaries. A
        // right-angle trace elbows at the midpoint between its ends, so consecutive
        // hops share their vertical run and merge into a single trunk — which is what
        // made three separate arrows look like a fan out of the first form.
        let ends = [Vec2::splat(20.0), Vec2::splat(20.0)];
        let straight = circuit_trace_geometry(
            Pos2::new(0.0, 0.0),
            Pos2::new(200.0, 120.0),
            ends,
            10.0,
            true,
        );
        let squared = circuit_trace_geometry(
            Pos2::new(0.0, 0.0),
            Pos2::new(200.0, 120.0),
            ends,
            10.0,
            false,
        );
        // One run plus two barbs, against a run per leg of the elbow plus barbs.
        assert_eq!(
            straight.len(),
            3,
            "a straight trace is one line and two barbs"
        );
        assert!(
            squared.len() > straight.len(),
            "the right-angle trace should have more legs: {} vs {}",
            squared.len(),
            straight.len()
        );
        // The straight run really does join the two ends, trimmed to their walls.
        let run = straight[0];
        assert!(run[0].x > 0.0 && run[0].y > 0.0, "left the source's wall");
        assert!(
            run[1].x < 200.0 && run[1].y < 120.0,
            "stopped at the target's wall"
        );
    }

    #[test]
    fn clicking_one_stage_selects_the_whole_arrow_chain() {
        // A run of arrows is one figure. Selecting a single stage left an arrow
        // stretched from a form that stayed behind whenever the selection was moved
        // or copied, so a click takes the chain — and Ctrl-click is still how one
        // stage is trimmed back out.
        let mandala = Mandala::from_rebis("((-> \"a\" \"b\" \"c\") \"elsewhere\")")
            .expect("the fixture parses");
        let at = |text: &str| {
            mandala
                .nodes()
                .iter()
                .find(|node| node.text == text)
                .unwrap_or_else(|| panic!("no form {text:?}"))
                .id
        };
        let (a, b, c, other) = (at("a"), at("b"), at("c"), at("elsewhere"));

        let mut doc = Doc::drawn(mandala);
        doc.select_only(b);
        for stage in [a, b, c] {
            assert!(
                doc.selection.contains(&stage),
                "clicking the middle stage must take the whole chain"
            );
        }
        assert_eq!(doc.selected, Some(b), "the clicked stage stays primary");
        assert!(
            !doc.selection.contains(&other),
            "a form outside the chain must not be dragged in"
        );

        // And the selection is a real expression: the arrows that join the stages are
        // in it too, so the panel can show the block instead of an error.
        let source = doc
            .selected_source()
            .expect("a chain selection must generate source");
        let code = source.expect("a non-empty selection has source");
        assert!(
            rebis_lang::parse(&code).is_ok(),
            "the block did not parse: {code}"
        );
        assert!(code.contains("->"), "the arrows were left out: {code}");

        // Ctrl-click still trims exactly one stage back out.
        doc.toggle_selection(c);
        assert!(!doc.selection.contains(&c));
        assert!(doc.selection.contains(&a) && doc.selection.contains(&b));

        // And a form in no chain selects only its own block, as before.
        doc.select_only(other);
        assert_eq!(doc.selection.len(), 1, "{:?}", doc.selection);
    }

    #[test]
    fn holding_space_pans_whatever_tool_is_selected() {
        use egui::{Event, Key, Modifiers, PointerButton, Pos2, RawInput, Rect, Vec2};
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1000.0, 800.0));

        // Same drive as the drag tool's own test, with space held down across the
        // gesture. Started from the middle of a form, so a mechanism that moved the
        // drawing instead of the view would be plainly wrong — which is exactly the
        // failure the three earlier attempts had to be checked against.
        let run = |hold_space: bool| {
            let mut editor = Editor::new(Mandala::from_rebis("(\"a\" \"b\")").unwrap());
            editor.tool = Tool::Select;
            let ctx = egui::Context::default();
            let mut origin = Pos2::ZERO;
            let frame = |editor: &mut Editor, events: Vec<Event>, origin: &mut Pos2| {
                let _ = ctx.run(
                    RawInput {
                        events,
                        // `key_down` is read from the modifier/key state egui keeps
                        // across frames, so the press event is enough — it does not
                        // have to be repeated every frame.
                        screen_rect: Some(screen),
                        ..Default::default()
                    },
                    |ctx| {
                        editor.header(ctx);
                        editor.tab_bar(ctx);
                        editor.palette(ctx);
                        editor.side(ctx);
                        editor.footer(ctx);
                        egui::CentralPanel::default().show(ctx, |ui| {
                            *origin = ui.max_rect().min;
                            editor.canvas(ui);
                        });
                    },
                );
            };
            let press_space = vec![Event::Key {
                key: Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            }];
            frame(&mut editor, Vec::new(), &mut origin);
            frame(&mut editor, Vec::new(), &mut origin);
            if hold_space {
                frame(&mut editor, press_space, &mut origin);
            }

            let held = editor.doc().mandala.nodes()[1].id;
            let node = editor.doc().mandala.node(held).unwrap();
            let (sx, sy) = editor.doc().view.to_screen(node.x, node.y);
            let start = origin + Vec2::new(sx as f32, sy as f32);
            let framed = editor.doc().view;
            let before = (node.x, node.y);

            frame(
                &mut editor,
                vec![
                    Event::PointerMoved(start),
                    Event::PointerButton {
                        pos: start,
                        button: PointerButton::Primary,
                        pressed: true,
                        modifiers: Modifiers::default(),
                    },
                ],
                &mut origin,
            );
            for step in [40.0f32, 90.0] {
                frame(
                    &mut editor,
                    vec![Event::PointerMoved(start + Vec2::splat(step))],
                    &mut origin,
                );
            }
            let after = editor.doc().mandala.node(held).map(|n| (n.x, n.y)).unwrap();
            (framed, editor.doc().view, before, after)
        };

        // Space down: the view moves and the form under the pointer does not, even
        // though the selected tool is Select.
        let (framed, view, before, after) = run(true);
        assert!(
            (view.tx - framed.tx).abs() > 1.0 || (view.ty - framed.ty).abs() > 1.0,
            "holding space did not pan: ({}, {})",
            view.tx,
            view.ty
        );
        assert_eq!(before, after, "and it must not drag the form it started on");

        // The same drive without space is the control: Select drags the form.
        let (framed, view, before, after) = run(false);
        assert_ne!(before, after, "without space, select still drags the form");
        assert_eq!((view.tx, view.ty), (framed.tx, framed.ty));
    }

    #[test]
    fn a_space_typed_into_a_text_box_does_not_pan_the_canvas() {
        // The gate on the space-hold pan. Without it, writing a prompt would drag
        // the drawing out from under the box being typed into — which is worse than
        // having no shortcut at all.
        use egui::{Event, Key, Modifiers, Pos2, RawInput, Rect, Vec2};
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(1200.0, 800.0));
        let mut editor = Editor::new(Mandala::from_rebis("(\"a\" \"b\")").unwrap());
        editor.tool = Tool::Select;
        let ctx = egui::Context::default();
        let mut origin = Pos2::ZERO;
        let frame = |editor: &mut Editor, events: Vec<Event>, origin: &mut Pos2| {
            let _ = ctx.run(
                RawInput {
                    events,
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |ctx| {
                    editor.side(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| {
                        *origin = ui.max_rect().min;
                        editor.canvas(ui);
                    });
                },
            );
        };
        frame(&mut editor, Vec::new(), &mut origin);
        // Put the caret in the source box, exactly as writing a prompt does.
        ctx.memory_mut(|memory| memory.request_focus(egui::Id::new("side_source_edit")));
        frame(&mut editor, Vec::new(), &mut origin);
        assert!(
            ctx.wants_keyboard_input(),
            "the box never took the keyboard"
        );

        // The space goes to the box, so it must not arm the pan. Dragging a FORM
        // afterwards has to keep doing what the selected tool says — moving the
        // form — rather than dragging the view out from under the box being typed
        // into. A form, not empty canvas: an empty-canvas drag moves the view under
        // every tool, so it cannot tell the two apart.
        frame(
            &mut editor,
            vec![Event::Key {
                key: Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::default(),
            }],
            &mut origin,
        );
        assert!(
            !editor.space_pan,
            "a space typed into a text box armed the canvas pan"
        );

        let held = editor.doc().mandala.nodes()[1].id;
        let node = editor.doc().mandala.node(held).unwrap();
        let (sx, sy) = editor.doc().view.to_screen(node.x, node.y);
        let start = origin + Vec2::new(sx as f32, sy as f32);
        let framed = editor.doc().view;
        let before = (node.x, node.y);
        frame(
            &mut editor,
            vec![
                Event::PointerMoved(start),
                Event::PointerButton {
                    pos: start,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::default(),
                },
            ],
            &mut origin,
        );
        for step in [40.0f32, 90.0] {
            frame(
                &mut editor,
                vec![Event::PointerMoved(start + Vec2::splat(step))],
                &mut origin,
            );
        }
        let after = editor.doc().mandala.node(held).map(|n| (n.x, n.y)).unwrap();
        assert_ne!(
            before, after,
            "the selected tool stopped working after a space was typed"
        );
        let view = editor.doc().view;
        assert_eq!(
            (view.tx, view.ty),
            (framed.tx, framed.ty),
            "a space meant for the text box panned the canvas"
        );
    }

    #[test]
    fn every_credential_the_agents_can_hold_is_offered_in_the_visual_editor() {
        // The picker is driven by the credential store rather than a list
        // written out beside it, so the guarantee is structural: a provider
        // added to the store reaches BOTH front ends. The terminal palette is
        // pinned to the same source by its own test.
        for (provider, var) in kaos_agent::auth::PROVIDERS {
            assert_eq!(
                kaos_agent::auth::var_for(provider),
                Some(*var),
                "{provider} does not resolve to its own credential"
            );
        }
        // The web tools' providers are among them, so a search key is set the
        // same way a model key is.
        for provider in ["brave", "serper", "tavily"] {
            assert!(
                kaos_agent::auth::PROVIDERS
                    .iter()
                    .any(|(name, _)| *name == provider),
                "{provider} cannot be given a key"
            );
        }
    }

    #[test]
    fn the_palette_runs_in_the_same_order_as_the_language_legend() {
        use kaos_workspace::rebis_workspace::REBIS_SYMBOLS;

        // Indentation leads: a drawing is nesting before it is anything else.
        let order = Form::ALL
            .iter()
            .map(|(_, make, _)| make())
            .collect::<Vec<_>>();
        assert_eq!(order[0], Form::Compose, "compose comes first");
        assert_eq!(order[1], Form::Square, "then the square");

        // After that the palette follows the punctuation the terminal app lists
        // across its top bar, so the alphabet is learned once and found in the
        // same sequence in either front end.
        let punctuation = |form: &Form| match form {
            Form::Compose => Some("("),
            Form::Square => Some("["),
            Form::Imaginary => Some("{"),
            Form::Source => Some("<>"),
            Form::Meta => Some("><"),
            Form::Numeric => Some("|"),
            Form::Supersede => Some("*"),
            Form::Invariant => Some("@"),
            Form::Function(_) => Some("~"),
            Form::Import => Some("#"),
            Form::Quote => Some("'"),
            Form::Unquote => Some(","),
            Form::Concat => Some("$"),
            Form::Flashback => Some("?"),
            Form::Dream => Some("!"),
            Form::Bind => Some("="),
            Form::Load => Some("&:"),
            Form::Context => Some("+"),
            Form::Route => Some("/"),
            Form::Conditional => Some("%"),
            Form::Invert => Some("^"),
            Form::Input => Some("&"),
            Form::Forward => Some("->"),
            Form::Backflow => Some("<-"),
            Form::Prompt => Some("\""),
            // Written with no punctuation of its own.
            Form::Symbol | Form::Call | Form::Program => None,
        };
        let rank = |symbol: &str| {
            REBIS_SYMBOLS
                .iter()
                .position(|listed| *listed == symbol)
                .unwrap_or_else(|| panic!("{symbol} is not in the language legend"))
        };

        let mut last = None;
        let mut seen_unpunctuated = false;
        for form in &order {
            match punctuation(form) {
                Some(symbol) => {
                    assert!(
                        !seen_unpunctuated,
                        "{form:?} is written with {symbol} but sits after the \
                         forms that have no punctuation"
                    );
                    let here = rank(symbol);
                    if let Some(before) = last {
                        assert!(
                            here > before,
                            "{form:?} ({symbol}) breaks the legend's order"
                        );
                    }
                    last = Some(here);
                }
                None => seen_unpunctuated = true,
            }
        }
        assert!(last.is_some(), "no punctuated form was checked at all");
    }

    #[test]
    fn a_mark_is_never_placed_on_bare_canvas() {
        // `'` and `,` are written on a form. Placing one where there is nothing
        // to mark used to leave a sigil with no operand: invalid source, and no
        // gesture that could ever attach it to anything.
        let mut editor = Editor::new(Mandala::new());
        for form in [Form::Quote, Form::Unquote] {
            editor.tool = Tool::Place(form.clone());
            editor.click(40.0, 40.0, false, false);
            assert!(
                editor.doc().mandala.is_empty(),
                "{form:?} left something behind on bare canvas"
            );
            assert!(editor.notice.is_some(), "and said nothing about why");
        }

        // On a form, the same click writes the mark, and again removes it.
        let mut editor = Editor::new(Mandala::from_rebis("(\"a\" \"b\")").unwrap());
        let marked = editor
            .doc()
            .mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Prompt)
            .unwrap()
            .id;
        let at = editor
            .doc()
            .mandala
            .node(marked)
            .map(|node| (node.x, node.y))
            .unwrap();
        editor.tool = Tool::Place(Form::Unquote);
        editor.click(at.0, at.1, false, false);
        assert_eq!(editor.doc().mandala.written_prefix(marked), ",");
        editor.click(at.0, at.1, false, false);
        assert_eq!(editor.doc().mandala.written_prefix(marked), "");
    }

    #[test]
    fn a_wider_boundary_wears_a_larger_sigil() {
        // The mark measures the boundary it names: nesting is read by seeing
        // the outer circle's sigil larger than the one on a circle inside it.
        let natural = mark_weight(Vec2::splat(1.0));
        let wide = mark_weight(Vec2::splat(9.0));
        let vast = mark_weight(Vec2::splat(400.0));
        assert!(wide > natural, "a grown boundary wears a larger mark");
        assert!(vast > wide, "and a larger one larger still");

        // Damped, so a boundary a hundred times a symbol's size does not wear a
        // mark a hundred times a letter's, and bounded so it cannot run away.
        assert!(wide < 9.0, "the growth is damped, not proportional");
        assert!(vast <= 8.0, "and it stops");

        // Never below natural: a boundary is not made illegible by being snug.
        assert_eq!(mark_weight(Vec2::splat(0.2)), 1.0);

        // And whatever it works out to, the rasterizer still caps it.
        assert!(glyph_px(MARK_PX * vast * 40.0) <= MAX_GLYPH_PX);
    }

    #[test]
    fn nothing_drawn_thins_below_a_pixel_however_far_the_canvas_is_pushed_away() {
        // Every stroke is scaled by the view. At the zoom that fits a real
        // program — around a fifth — an unfloored 1.8px outline lands at 0.34px
        // and the drawing fades to almost blank: the flow connections vanish
        // outright, which is how a program full of arrows came to show none.
        let fitted = 0.19_f32;
        assert!(
            1.8 * fitted < 0.5,
            "the unfloored width really is sub-pixel"
        );
        for zoom in [1e-6, 0.05, fitted, 0.5, 1.0, 12.0] {
            for width in [
                node_outline_width(Shape::Circle, false) * zoom,
                node_outline_width(Shape::Diamond, false) * zoom,
                1.8 * zoom,
                5.0 * zoom,
            ] {
                let drawn = pen(width);
                assert!(drawn >= HAIRLINE_PX, "{drawn} at zoom {zoom} is invisible");
                assert!(drawn >= width - 1e-6, "the floor must never thin a line");
            }
        }
        // Zoomed in, the pen is the geometry's own width again, not the floor.
        assert!(pen(node_outline_width(Shape::Circle, false) * 12.0) > HAIRLINE_PX);
        // And nothing pathological reaches the tessellator.
        for bad in [f32::NAN, f32::INFINITY, -3.0] {
            assert!(pen(bad).is_finite() && pen(bad) >= HAIRLINE_PX);
        }
    }

    #[test]
    fn the_structural_view_opens_framed_and_its_wheel_has_no_stop() {
        // A big drawing must be scaled DOWN to the window. Holding the fit above
        // a floor scaled it up instead and opened it part-way off screen, which
        // nesting every parenthesised form made routine.
        let mut sprawling = Mandala::new();
        let root = sprawling.add(Form::Compose, "", 0.0, 0.0);
        for step in 0..24 {
            let leaf = sprawling.add(Form::Prompt, "p", f64::from(step) * 400.0, 0.0);
            sprawling.father_of(root, leaf);
        }
        let layout = sprawling.spatial_layout();
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(900.0, 640.0));
        let projected = project_spatial(&sprawling, &layout, rect, SpatialCamera::default());
        let frame = rect.expand(1.0);
        assert!(
            projected
                .iter()
                .all(|node| frame.contains(node.position)
                    || rect.expand(140.0).contains(node.position)),
            "a drawing wider than the window must be fitted into it, not enlarged"
        );

        // And the wheel runs out of arithmetic rather than out of permission,
        // in both directions, exactly as the flat canvas does.
        let mut camera = SpatialCamera::default();
        for _ in 0..200 {
            camera.zoom_by(1.0 / 1.1);
        }
        assert!(
            camera.zoom < 1e-6,
            "pulling back stopped at {}",
            camera.zoom
        );
        assert!(camera.zoom > 0.0 && camera.zoom.is_finite());

        let mut camera = SpatialCamera::default();
        for _ in 0..200 {
            camera.zoom_by(1.1);
        }
        assert!(camera.zoom > 1e6, "pushing in stopped at {}", camera.zoom);
        assert!(camera.zoom.is_finite());

        // Nothing pathological moves it.
        let mut camera = SpatialCamera::default();
        for bad in [f32::NAN, f32::INFINITY, 0.0, -2.0] {
            camera.zoom_by(bad);
            assert_eq!(camera.zoom, 1.0, "{bad} must leave the camera alone");
        }
    }

    #[test]
    fn deeper_spatial_nodes_cast_longer_softer_shadows() {
        let root = spatial_shadow_style(0, 1.0, true);
        let nested = spatial_shadow_style(5, 1.0, true);
        assert!(nested.offset.length() > root.offset.length());
        assert!(nested.softness > root.softness);
        assert!(root.opacity > spatial_shadow_style(0, 1.0, false).opacity);
    }

    #[test]
    fn structural_depths_receive_separate_shadow_plates() {
        let mut mandala = Mandala::new();
        let first = mandala.add(Form::Prompt, "first", 0.0, 0.0);
        let second = mandala.add(Form::Square, "", 0.0, 0.0);
        let layout = SpatialLayout {
            nodes: vec![
                kaos_core::visual::SpatialNode {
                    id: first,
                    x: -100.0,
                    y: 0.0,
                    z: 0.0,
                    depth: 0,
                    recursive: false,
                },
                kaos_core::visual::SpatialNode {
                    id: second,
                    x: 100.0,
                    y: 0.0,
                    z: 300.0,
                    depth: 1,
                    recursive: false,
                },
            ],
            recursive_edges: Vec::new(),
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(800.0, 600.0));
        let projected = project_spatial(&mandala, &layout, rect, SpatialCamera::default());
        let plates = spatial_layer_plates(&mandala, &layout, &projected);

        assert_eq!(plates.len(), 2);
        assert_eq!(
            plates
                .iter()
                .map(|plate| plate.depth)
                .collect::<BTreeSet<_>>(),
            BTreeSet::from([0, 1])
        );
        assert!(plates
            .iter()
            .all(|plate| plate.radius.x >= 76.0 && plate.radius.y >= 30.0));
        assert!(plates
            .windows(2)
            .all(|pair| pair[0].camera_depth >= pair[1].camera_depth));
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
        let ordinary = node_outline_width(Shape::Diamond, false);
        for shape in [Shape::Square, Shape::Circle] {
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
