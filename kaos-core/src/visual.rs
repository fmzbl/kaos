//! The mandala canvas as data, plus Rebis code generation and loading.
//!
//! [`rebis_lang::mandala`] projects Rebis source *into* the `o-[]-o` notation.
//! This module is the inverse: a drawable graph that generates Rebis source,
//! and loads it back. It is what `kaos visual` edits.
//!
//! # The abstraction
//!
//! Every Rebis expression, without exception, is three things: a **form** tag,
//! a **text** payload, and an **ordered list of children**.
//!
//! ```text
//! "prompt"            Prompt    text            no children
//! name                Symbol    text            no children
//! (# module)          Import    module          no children
//! 'x                  Quote     —               1
//! ,x                  Unquote   —               1
//! (^ x)               Invert    —               1
//! (-> a b)            Forward   —               2
//! (<- a b)            Backflow  —               2
//! ([m] a b …)         Square    —               mediator, then branches
//! ($ a b …)           Concat    —               list
//! (a b …)             Compose   —               list
//! (f a b …)           Call      name            arguments
//! (~ f (p …) body)    Function  name + params   1 (the body)
//! a b …               Program   —               list (top level only)
//! ```
//!
//! So the whole language is one node type — [`Form`] plus text — and edges.
//! A node's children are the nodes attached with the `father of` gesture, in
//! the order the links were drawn. [`Mandala::to_rebis`] folds that into source and
//! [`Mandala::from_rebis`] unfolds source back onto the canvas; every form
//! round-trips one-to-one. Invalid or incomplete graphs are reported rather
//! than repaired with invisible expressions.
//!
//! The whiteboard alphabet is a *rendering* of this, not a restriction on it:
//! prompts, symbols, and combining forms use the `o-[]-o` outlines; source
//! sigils—including `^`—are their own drawn shapes; every father-to-child edge
//! is an arrow ([`Form::shape`]).
//!
//! This module is pure and std-only — no UI, no rendering, no I/O — so the
//! editor front-end is a thin shell over it.

use std::collections::{HashMap, HashSet};
use std::fmt;

/// Stable handle for a node. Ids are never reused, so a handle held by the UI
/// stays valid (or stays dangling) across edits rather than silently retargeting.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub struct NodeId(pub u32);

/// Which Rebis form a node is. The text payload and children live on the node;
/// only `Function` carries extra structure of its own (its parameter names).
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Form {
    /// `"text"` — a raw model prompt.
    Prompt,
    /// `name` — a symbol or macro parameter.
    Symbol,
    /// `(# module)` — a module import.
    Import,
    /// `'x` — quoted, inert syntax.
    Quote,
    /// `,x` — syntax spliced into the surrounding quote.
    Unquote,
    /// `(^ x)` — the recursive orientation dual of `x`.
    Invert,
    /// `(-> a b)` — left-to-right value flow.
    Forward,
    /// `(<- a b)` — right-to-left value flow.
    Backflow,
    /// `([m] a b …)` — the first child mediates the rest.
    Square,
    /// `($ a b …)` — string interpolation.
    Concat,
    /// `(a b …)` — an abstraction boundary.
    Compose,
    /// `(f a b …)` — a call to a named macro.
    Call,
    /// `(~ f (p …) body)` — a named structural macro.
    Function(Vec<String>),
    /// `(& port body)` — receive an external input under `port` into `body`.
    Input,
    /// Several top-level forms. Valid only at the root.
    Program,
}

/// How many children a form accepts.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Arity {
    Exactly(usize),
    AtLeast(usize),
    Any,
}

impl Arity {
    pub const fn accepts(self, n: usize) -> bool {
        match self {
            Arity::Exactly(k) => n == k,
            Arity::AtLeast(k) => n >= k,
            Arity::Any => true,
        }
    }
}

impl fmt::Display for Arity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Arity::Exactly(k) => write!(f, "exactly {k}"),
            Arity::AtLeast(k) => write!(f, "at least {k}"),
            Arity::Any => write!(f, "any number of"),
        }
    }
}

/// Palette entry: human label, form constructor, and initial editable text.
pub type FormSpec = (&'static str, fn() -> Form, &'static str);

impl Form {
    /// Forms that are *placed* on the canvas, in palette order, with a default
    /// text payload.
    ///
    /// `Forward` and `Backflow` are deliberately absent: they are created by
    /// drawing an arrow between two shapes ([`Mandala::flow`]), because the
    /// arrow and the node are the same idea. They remain full forms in every
    /// other respect — loaded, rendered and generated like the rest.
    pub const ALL: &'static [FormSpec] = &[
        ("o prompt", || Form::Prompt, "prompt"),
        ("◇ symbol", || Form::Symbol, "x"),
        ("[] square", || Form::Square, ""),
        ("( ) compose", || Form::Compose, ""),
        ("$ concat", || Form::Concat, ""),
        ("call", || Form::Call, "f"),
        ("~ macro", || Form::Function(vec!["x".into()]), "f"),
        ("# import", || Form::Import, "std/flow"),
        ("& input", || Form::Input, "input"),
        ("' quote", || Form::Quote, ""),
        (", unquote", || Form::Unquote, ""),
        ("^ invert", || Form::Invert, ""),
        ("program", || Form::Program, ""),
    ];

    /// How the form is drawn.
    ///
    /// Forms whose source syntax *is* a sigil are drawn as that sigil, so the
    /// canvas reads like the language: `$`, `~`, `#`, `'`, `,`, and `^` are their own
    /// shapes rather than boxes with a caption. The rest fall back to the
    /// whiteboard alphabet — terminals are `o`, combining forms are `[]`.
    pub fn shape(&self) -> Shape {
        match self {
            Form::Prompt => Shape::Circle,
            Form::Symbol => Shape::Diamond,
            Form::Concat => Shape::Dollar,
            Form::Function(_) => Shape::Tilde,
            Form::Import => Shape::Hash,
            Form::Quote => Shape::Quote,
            Form::Unquote => Shape::Comma,
            Form::Invert => Shape::Caret,
            // Flow is drawn as the connecting arrow itself, never as a box
            // sitting between two arrows.
            Form::Forward | Form::Backflow => Shape::Arrow,
            Form::Compose => Shape::Oval,
            // A call is a parallelogram — a box in motion, distinct from the
            // square that combines its own children.
            Form::Call => Shape::Parallelogram,
            // An input port is drawn as an inlet pointing into the graph.
            Form::Input => Shape::Amp,
            Form::Square | Form::Program => Shape::Square,
        }
    }

    pub fn arity(&self) -> Arity {
        match self {
            Form::Prompt | Form::Symbol | Form::Import => Arity::Exactly(0),
            Form::Quote | Form::Unquote | Form::Invert | Form::Function(_) | Form::Input => {
                Arity::Exactly(1)
            }
            Form::Forward | Form::Backflow => Arity::Exactly(2),
            Form::Square => Arity::AtLeast(2),
            Form::Program => Arity::AtLeast(2),
            Form::Concat | Form::Compose => Arity::AtLeast(1),
            Form::Call => Arity::Any,
        }
    }

    /// Whether the form's text payload is meaningful (and so editable).
    pub fn uses_text(&self) -> bool {
        matches!(
            self,
            Form::Prompt
                | Form::Symbol
                | Form::Import
                | Form::Call
                | Form::Function(_)
                | Form::Input
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            Form::Prompt => "prompt",
            Form::Symbol => "symbol",
            Form::Import => "import",
            Form::Quote => "quote",
            Form::Unquote => "unquote",
            Form::Invert => "invert",
            Form::Forward => "forward",
            Form::Backflow => "backflow",
            Form::Square => "square",
            Form::Concat => "concat",
            Form::Compose => "compose",
            Form::Call => "call",
            Form::Function(_) => "macro",
            Form::Input => "input",
            Form::Program => "program",
        }
    }
}

/// How a node is drawn. Derived from [`Form::shape`]; the model is the form.
///
/// The sigil shapes carry no extra meaning — they are the same node, drawn as
/// the character the form is written with.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    /// `o` — a prompt terminal.
    Circle,
    /// `◇` — a symbol: a name rather than a literal.
    Diamond,
    /// `[]` — a form that combines children.
    Square,
    /// `( )` — an ordered composition boundary.
    Oval,
    /// `$` — string interpolation.
    Dollar,
    /// `~` — a macro definition.
    Tilde,
    /// `#` — a module import.
    Hash,
    /// `'` — a quote.
    Quote,
    /// `,` — an unquote.
    Comma,
    /// `^` — recursive syntax orientation inversion.
    Caret,
    /// `->` / `<-` — drawn as the arrow between its two children, with a small
    /// handle at the midpoint so it can still be selected and deleted.
    Arrow,
    /// A slanted box — a macro call, a square set in motion.
    Parallelogram,
    /// `&` — an input inlet, a box with a leftward point where a value enters.
    Amp,
}

/// One pen stroke of a sigil, in node-local coordinates centred on the origin.
///
/// Geometry, not pixels: a renderer turns these into whatever its drawing API
/// wants (SVG paths, an egui `Painter`, a canvas). Keeping the shapes here as
/// data means the sigils are defined once and are testable without a window.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stroke {
    /// A connected run of straight segments.
    Poly(&'static [(f32, f32)]),
    /// A cubic Bézier: start, two controls, end.
    Cubic([(f32, f32); 4]),
}

impl Shape {
    /// The strokes that draw this shape's sigil, or empty for the shapes that
    /// are outlines ([`Shape::Circle`], [`Shape::Square`], [`Shape::Oval`],
    /// [`Shape::Diamond`]).
    pub fn strokes(self) -> &'static [Stroke] {
        match self {
            // Two slanted uprights crossed by two bars.
            Shape::Hash => &[
                Stroke::Poly(&[(-15.0, -7.0), (15.0, -7.0)]),
                Stroke::Poly(&[(-15.0, 7.0), (15.0, 7.0)]),
                Stroke::Poly(&[(-6.0, -18.0), (-10.0, 18.0)]),
                Stroke::Poly(&[(10.0, -18.0), (6.0, 18.0)]),
            ],
            // An S through a vertical bar.
            Shape::Dollar => &[
                Stroke::Cubic([(11.0, -11.0), (11.0, -18.0), (-11.0, -19.0), (-11.0, -8.0)]),
                Stroke::Cubic([(-11.0, -8.0), (-11.0, 0.0), (11.0, 1.0), (11.0, 9.0)]),
                Stroke::Cubic([(11.0, 9.0), (11.0, 19.0), (-11.0, 19.0), (-11.0, 11.0)]),
                Stroke::Poly(&[(0.0, -19.0), (0.0, 19.0)]),
            ],
            // A single wave.
            Shape::Tilde => &[
                Stroke::Cubic([(-16.0, 3.0), (-11.0, -9.0), (-5.0, -9.0), (0.0, 0.0)]),
                Stroke::Cubic([(0.0, 0.0), (5.0, 9.0), (11.0, 9.0), (16.0, -3.0)]),
            ],
            // A comma sitting high — an apostrophe.
            Shape::Quote => &[Stroke::Cubic([
                (3.0, -16.0),
                (3.0, -16.0),
                (1.0, -8.0),
                (-3.0, -4.0),
            ])],
            // The same stroke, dropped to the baseline.
            Shape::Comma => &[Stroke::Cubic([
                (3.0, 4.0),
                (3.0, 4.0),
                (1.0, 12.0),
                (-3.0, 16.0),
            ])],
            // A crisp caret, kept open so it remains legible at low zoom.
            Shape::Caret => &[Stroke::Poly(&[(-16.0, 10.0), (0.0, -12.0), (16.0, 10.0)])],
            Shape::Circle
            | Shape::Square
            | Shape::Oval
            | Shape::Diamond
            | Shape::Arrow
            | Shape::Parallelogram
            | Shape::Amp => &[],
        }
    }

    /// The four corners of the diamond, in node-local coordinates.
    pub fn diamond_points() -> [(f32, f32); 4] {
        let r = NODE_R as f32;
        [(0.0, -r), (r, 0.0), (0.0, r), (-r, 0.0)]
    }

    /// The four corners of the call parallelogram: a box sheared to the right.
    pub fn parallelogram_points() -> [(f32, f32); 4] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        let s = ry * 0.55;
        [(-r + s, -ry), (r + s, -ry), (r - s, ry), (-r - s, ry)]
    }

    /// The five corners of the input inlet: a box with a leftward point where
    /// the received value enters.
    pub fn inlet_points() -> [(f32, f32); 5] {
        let (r, ry) = (NODE_R as f32, NODE_RY as f32);
        let tip = ry * 0.85;
        [
            (-r - tip, 0.0),
            (-r, -ry),
            (r, -ry),
            (r, ry),
            (-r, ry),
        ]
    }

    /// Whether a point offset from the node's centre is inside the shape.
    ///
    /// Shapes with a real outline — the box and the diamond — are tested
    /// against that outline, so clicking near a diamond's corner correctly
    /// misses. The sigils are drawn as thin strokes that would be
    /// near-impossible to hit, so they keep a full round target instead: what
    /// is drawn and what is clickable are deliberately different there.
    pub fn contains(self, dx: f64, dy: f64) -> bool {
        match self {
            // The slant and the inlet point extend a little past the box; a
            // box-sized target is close enough and keeps clicking predictable.
            Shape::Square | Shape::Parallelogram | Shape::Amp => {
                dx.abs() <= NODE_R && dy.abs() <= NODE_RY
            }
            Shape::Oval => {
                let x = dx / NODE_R;
                let y = dy / NODE_RY;
                x * x + y * y <= 1.0
            }
            Shape::Diamond => dx.abs() + dy.abs() <= NODE_R,
            // Only a small handle: the arrow is a line, and a full disc here
            // would swallow clicks meant for the shapes it runs between.
            Shape::Arrow => dx * dx + dy * dy <= ARROW_HANDLE * ARROW_HANDLE,
            _ => dx * dx + dy * dy <= NODE_R * NODE_R,
        }
    }
}

/// A placed form. `x`/`y` are canvas coordinates, carried for the editor's
/// benefit; they never affect generated code.
#[derive(Clone, Debug)]
pub struct Node {
    pub id: NodeId,
    pub form: Form,
    /// The form's text payload: prompt text, symbol/call/function name, or
    /// module path. Ignored by forms where [`Form::uses_text`] is false.
    pub text: String,
    pub x: f64,
    pub y: f64,
}

impl Node {
    pub fn shape(&self) -> Shape {
        self.form.shape()
    }

    /// Short canvas caption.
    ///
    /// Empty when the shape already says everything — a `$` or `,` needs no
    /// label written across it. Forms drawn as a sigil but carrying a name
    /// (`~ f`, `# std/flow`) return that name, which the renderer places
    /// outside the glyph.
    pub fn caption(&self) -> String {
        if self.form.uses_text() {
            return self.text.clone();
        }
        match self.form {
            Form::Forward => "->".into(),
            Form::Backflow => "<-".into(),
            Form::Square => "[ ]".into(),
            Form::Compose => "( )".into(),
            Form::Program => "program".into(),
            // Drawn as their own sigil; nothing to write on top.
            Form::Concat | Form::Quote | Form::Unquote | Form::Invert => String::new(),
            _ => self.form.name().into(),
        }
    }
}

/// A directed arrow: `from` is a child of `to`. Drawing a `←` on the canvas is
/// the same arrow with its endpoints swapped, so the model needs only one
/// representation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Arrow {
    pub from: NodeId,
    pub to: NodeId,
}

/// Normalized world-space rectangle used by marquee selection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct WorldRect {
    pub min_x: f64,
    pub min_y: f64,
    pub max_x: f64,
    pub max_y: f64,
}

impl WorldRect {
    #[must_use]
    pub fn from_points(a: (f64, f64), b: (f64, f64)) -> Self {
        Self {
            min_x: a.0.min(b.0),
            min_y: a.1.min(b.1),
            max_x: a.0.max(b.0),
            max_y: a.1.max(b.1),
        }
    }

    fn contains(self, point: (f64, f64)) -> bool {
        point.0 >= self.min_x
            && point.0 <= self.max_x
            && point.1 >= self.min_y
            && point.1 <= self.max_y
    }

    fn intersects_node(self, node: &Node) -> bool {
        node.x + NODE_R >= self.min_x
            && node.x - NODE_R <= self.max_x
            && node.y + NODE_RY >= self.min_y
            && node.y - NODE_RY <= self.max_y
    }

    fn intersects_segment(self, from: (f64, f64), to: (f64, f64)) -> bool {
        if self.contains(from) || self.contains(to) {
            return true;
        }
        let (dx, dy) = (to.0 - from.0, to.1 - from.1);
        let mut near = 0.0f64;
        let mut far = 1.0f64;
        for (direction, distance) in [
            (-dx, from.0 - self.min_x),
            (dx, self.max_x - from.0),
            (-dy, from.1 - self.min_y),
            (dy, self.max_y - from.1),
        ] {
            if direction.abs() <= f64::EPSILON {
                if distance < 0.0 {
                    return false;
                }
                continue;
            }
            let ratio = distance / direction;
            if direction < 0.0 {
                near = near.max(ratio);
            } else {
                far = far.min(ratio);
            }
            if near > far {
                return false;
            }
        }
        true
    }
}

fn point_segment_distance_squared(point: (f64, f64), from: (f64, f64), to: (f64, f64)) -> f64 {
    let segment = (to.0 - from.0, to.1 - from.1);
    let length_squared = segment.0 * segment.0 + segment.1 * segment.1;
    if length_squared <= f64::EPSILON {
        let delta = (point.0 - from.0, point.1 - from.1);
        return delta.0 * delta.0 + delta.1 * delta.1;
    }
    let projection =
        ((point.0 - from.0) * segment.0 + (point.1 - from.1) * segment.1) / length_squared;
    let projection = projection.clamp(0.0, 1.0);
    let closest = (
        from.0 + projection * segment.0,
        from.1 + projection * segment.1,
    );
    let delta = (point.0 - closest.0, point.1 - closest.1);
    delta.0 * delta.0 + delta.1 * delta.1
}

/// One node in the derived structural 3D projection.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SpatialNode {
    pub id: NodeId,
    /// Existing whiteboard placement remains the first two axes.
    pub x: f64,
    pub y: f64,
    /// Structural nesting is the third axis.
    pub z: f64,
    pub depth: usize,
    /// The node participates in at least one recursive back-edge.
    pub recursive: bool,
}

/// A deterministic 3D reading of a mandala.
///
/// This is derived data: orbiting it or switching projections never changes
/// source. Recursive edges are named explicitly so a renderer can lift them
/// out of the ordinary edge plane.
#[derive(Clone, PartialEq, Debug, Default)]
pub struct SpatialLayout {
    pub nodes: Vec<SpatialNode>,
    pub recursive_edges: Vec<Arrow>,
}

impl SpatialLayout {
    pub fn node(&self, id: NodeId) -> Option<&SpatialNode> {
        self.nodes.iter().find(|node| node.id == id)
    }
}

/// Why a mandala could not be turned into Rebis source.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum MandalaError {
    /// Nothing drawn yet.
    Empty,
    /// Every node feeds another one, so there is no result to return.
    NoRoot,
    /// Several nodes have no outgoing arrow; the program has no single answer.
    ManyRoots(Vec<NodeId>),
    /// Arrows form a loop, which cannot be written as a finite expression.
    Cycle,
    /// One visual expression is attached to several parents, which would
    /// duplicate it into several Rebis AST positions.
    Shared(NodeId),
    /// A form has the wrong number of incoming arrows.
    WrongArity {
        id: NodeId,
        form: Form,
        want: Arity,
        got: usize,
    },
    /// `Program` groups top-level forms and cannot be nested.
    NestedProgram(NodeId),
    /// The visual payload would not parse as Rebis source.
    InvalidSource(String),
}

/// Why a selected visual block cannot be folded into a new parent form.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum FoldError {
    EmptySelection,
    NoRoots,
    WrongArity { form: Form, want: Arity, got: usize },
    SeveralFathers,
    InvalidSource(String),
}

impl fmt::Display for FoldError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptySelection => write!(f, "select one or more forms first"),
            Self::NoRoots => write!(f, "the selected block has no finite root"),
            Self::WrongArity {
                form, want, got, ..
            } => write!(
                f,
                "{} takes {want} children, but the selection has {got} roots",
                form.name()
            ),
            Self::SeveralFathers => {
                write!(f, "the selected block crosses several outside fathers")
            }
            Self::InvalidSource(message) => {
                write!(f, "the folded form is not exact Rebis: {message}")
            }
        }
    }
}

impl std::error::Error for FoldError {}

impl fmt::Display for MandalaError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => write!(f, "nothing drawn yet"),
            Self::NoRoot => write!(f, "every shape feeds another one — no final answer"),
            Self::ManyRoots(ids) => write!(
                f,
                "{} shapes have no outgoing arrow — connect them into one result",
                ids.len()
            ),
            Self::Cycle => write!(f, "arrows form a loop"),
            Self::Shared(id) => write!(
                f,
                "shape #{} has several parents — duplicate it for a one-to-one AST",
                id.0
            ),
            Self::WrongArity {
                form, want, got, ..
            } => write!(
                f,
                "{} takes {want} incoming arrows, but has {got}",
                form.name()
            ),
            Self::NestedProgram(_) => {
                write!(f, "program groups top-level forms and cannot be nested")
            }
            Self::InvalidSource(message) => write!(f, "generated Rebis is invalid: {message}"),
        }
    }
}

impl std::error::Error for MandalaError {}

/// Circle radius, and the square's half-width, in world units.
pub const NODE_R: f64 = 34.0;
/// The square's half-height. Squares are a little flatter than circles are tall.
pub const NODE_RY: f64 = NODE_R * 0.72;
/// Grab radius of the handle on a drawn arrow.
pub const ARROW_HANDLE: f64 = 11.0;
/// Selection tolerance around a rendered flow line.
pub const ARROW_HIT_SLOP: f64 = 7.0;

/// Which part of the infinite canvas is on screen.
///
/// `tx`/`ty` are a screen-space translation and `zoom` a scale, so a renderer
/// maps this straight onto one transform. Pan is unbounded in every direction;
/// zoom is clamped so the canvas can never be lost entirely.
///
/// Pure math, deliberately UI-agnostic, so the coordinate handling is testable
/// without a window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    pub tx: f64,
    pub ty: f64,
    pub zoom: f64,
}

impl Default for View {
    fn default() -> Self {
        Self {
            tx: 0.0,
            ty: 0.0,
            zoom: 1.0,
        }
    }
}

impl View {
    pub const MIN_ZOOM: f64 = 0.15;
    pub const MAX_ZOOM: f64 = 5.0;

    pub fn new() -> Self {
        Self::default()
    }

    /// Screen point (relative to the canvas element) to world coordinates.
    pub fn to_world(&self, sx: f64, sy: f64) -> (f64, f64) {
        ((sx - self.tx) / self.zoom, (sy - self.ty) / self.zoom)
    }

    /// World point to screen.
    pub fn to_screen(&self, wx: f64, wy: f64) -> (f64, f64) {
        (wx * self.zoom + self.tx, wy * self.zoom + self.ty)
    }

    /// Drag the canvas by a screen-space delta. Unbounded.
    pub fn pan(&mut self, dx: f64, dy: f64) {
        self.tx += dx;
        self.ty += dy;
    }

    /// Scale by `factor` while keeping the world point under `(sx, sy)` fixed,
    /// so zooming follows the pointer instead of the origin.
    pub fn zoom_at(&mut self, sx: f64, sy: f64, factor: f64) {
        let (wx, wy) = self.to_world(sx, sy);
        self.zoom = (self.zoom * factor).clamp(Self::MIN_ZOOM, Self::MAX_ZOOM);
        self.tx = sx - wx * self.zoom;
        self.ty = sy - wy * self.zoom;
    }
}

/// A drawn mandala: forms plus the arrows between them.
#[derive(Clone, Debug, Default)]
pub struct Mandala {
    nodes: Vec<Node>,
    arrows: Vec<Arrow>,
    next_id: u32,
}

impl Mandala {
    pub fn new() -> Self {
        Self::default()
    }

    /// Place a form and return its handle.
    pub fn add(&mut self, form: Form, text: impl Into<String>, x: f64, y: f64) -> NodeId {
        let id = NodeId(self.next_id);
        self.next_id += 1;
        self.nodes.push(Node {
            id,
            form,
            text: text.into(),
            x,
            y,
        });
        id
    }

    /// Make `father` the parent of `child`, appending `child` as its next
    /// ordered operand.
    ///
    /// This is the canvas's `father of` relation. Geometry is deliberately
    /// ignored: overlap, containment, proximity, and screen order never create
    /// composition. Duplicates and self-links are ignored. Storage remains
    /// child-to-parent because that is the direction source generation walks.
    pub fn father_of(&mut self, father: NodeId, child: NodeId) {
        if father == child || !self.has(father) || !self.has(child) {
            return;
        }
        let arrow = Arrow {
            from: child,
            to: father,
        };
        if !self.arrows.contains(&arrow) {
            self.arrows.push(arrow);
        }
    }

    /// Compatibility name using the internal child-to-parent order.
    ///
    /// New UI code should say [`Self::father_of`], because that is the visible
    /// relation. A flow arrow has the distinct semantics in [`Self::flow`].
    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        self.father_of(to, from);
    }

    /// Link two shapes with a flow node, creating the `->` or `<-` behind the
    /// arrow rather than making the user place it.
    ///
    /// `(-> a b)` is a node with two children, so drawing an arrow from `a` to
    /// `b` means "make a Forward whose children are a and b" — the arrow the
    /// user drew and the node the language needs are the same gesture. The new
    /// node lands between the two shapes and becomes the result in their place.
    ///
    /// Returns `None` if either endpoint is missing or they are the same shape.
    pub fn flow(&mut self, from: NodeId, to: NodeId, form: Form) -> Option<NodeId> {
        if from == to || !self.has(from) || !self.has(to) {
            return None;
        }
        let (a, b) = (self.node(from)?, self.node(to)?);
        // Sit the flow node between its endpoints, nudged clear of the line.
        let (mx, my) = ((a.x + b.x) / 2.0, (a.y + b.y) / 2.0);
        let id = self.add(form, "", mx, my - NODE_R * 1.6);
        self.father_of(id, from);
        self.father_of(id, to);
        Some(id)
    }

    pub fn disconnect(&mut self, from: NodeId, to: NodeId) {
        self.arrows.retain(|a| a.from != from || a.to != to);
    }

    /// Remove a node and every arrow touching it.
    pub fn remove(&mut self, id: NodeId) {
        self.nodes.retain(|n| n.id != id);
        self.arrows.retain(|a| a.from != id && a.to != id);
    }

    pub fn set_text(&mut self, id: NodeId, text: impl Into<String>) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.text = text.into();
        }
    }

    pub fn set_form(&mut self, id: NodeId, form: Form) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.form = form;
        }
    }

    pub fn move_to(&mut self, id: NodeId, x: f64, y: f64) {
        if let Some(n) = self.nodes.iter_mut().find(|n| n.id == id) {
            n.x = x;
            n.y = y;
        }
    }

    pub fn nodes(&self) -> &[Node] {
        &self.nodes
    }

    pub fn arrows(&self) -> &[Arrow] {
        &self.arrows
    }

    /// Nodes touched by a world-space marquee.
    ///
    /// Ordinary forms use their visual bounds. Flow forms are special: they
    /// have only a small model handle but render as the whole line between
    /// operands, so crossing any part of that line selects the flow node.
    #[must_use]
    pub fn nodes_in_rect(&self, rect: WorldRect) -> Vec<NodeId> {
        self.nodes
            .iter()
            .filter(|node| {
                if node.shape() != Shape::Arrow {
                    return rect.intersects_node(node);
                }
                let children = self.children(node.id);
                let [first, second] = children[..] else {
                    return rect.intersects_node(node);
                };
                let Some(first) = self.node(first) else {
                    return rect.intersects_node(node);
                };
                let Some(second) = self.node(second) else {
                    return rect.intersects_node(node);
                };
                rect.intersects_node(node)
                    || rect.intersects_segment((first.x, first.y), (second.x, second.y))
            })
            .map(|node| node.id)
            .collect()
    }

    /// Copy the induced subgraph over `ids`, retaining original draw order,
    /// stable node ids, and only links whose two ends are selected.
    #[must_use]
    pub fn induced_subgraph(&self, ids: impl IntoIterator<Item = NodeId>) -> Self {
        let ids = ids.into_iter().collect::<HashSet<_>>();
        Self {
            nodes: self
                .nodes
                .iter()
                .filter(|node| ids.contains(&node.id))
                .cloned()
                .collect(),
            arrows: self
                .arrows
                .iter()
                .filter(|arrow| ids.contains(&arrow.from) && ids.contains(&arrow.to))
                .copied()
                .collect(),
            next_id: self.next_id,
        }
    }

    /// Append an exact copy of `source`, assigning fresh ids and translating
    /// every node by `offset`.
    ///
    /// Node and link order are retained, so ordered operands mean the same
    /// thing after a block is pasted. Links to anything outside `source`
    /// cannot leak into the copy.
    pub fn append_copy(&mut self, source: &Self, offset: (f64, f64)) -> Vec<NodeId> {
        let mut remap = HashMap::new();
        let mut pasted = Vec::with_capacity(source.nodes.len());
        for node in &source.nodes {
            let id = self.add(
                node.form.clone(),
                node.text.clone(),
                node.x + offset.0,
                node.y + offset.1,
            );
            remap.insert(node.id, id);
            pasted.push(id);
        }
        for arrow in &source.arrows {
            let (Some(&child), Some(&father)) = (remap.get(&arrow.from), remap.get(&arrow.to))
            else {
                continue;
            };
            self.father_of(father, child);
        }
        pasted
    }

    /// Top-level expressions inside a selection, in stable canvas order.
    ///
    /// Nested selected operands are already represented through their selected
    /// father, so folding uses only these roots as the new form's children.
    #[must_use]
    pub fn roots_in(&self, ids: impl IntoIterator<Item = NodeId>) -> Vec<NodeId> {
        let ids = ids.into_iter().collect::<HashSet<_>>();
        self.nodes
            .iter()
            .filter(|node| {
                ids.contains(&node.id)
                    && !self
                        .arrows
                        .iter()
                        .any(|arrow| arrow.from == node.id && ids.contains(&arrow.to))
            })
            .map(|node| node.id)
            .collect()
    }

    /// Fold the selected subgraph into one new father form.
    ///
    /// Existing internal structure is preserved. If the selection occupied
    /// one slot (or several sibling slots) under an outside father, that
    /// boundary is rewired through the new form at the first selected slot.
    /// Crossing several outside fathers is rejected instead of creating a
    /// shared visual node.
    pub fn fold_selection(
        &mut self,
        ids: impl IntoIterator<Item = NodeId>,
        form: Form,
        text: impl Into<String>,
    ) -> Result<NodeId, FoldError> {
        let original = self.clone();
        let ids = ids
            .into_iter()
            .filter(|id| self.has(*id))
            .collect::<HashSet<_>>();
        if ids.is_empty() {
            return Err(FoldError::EmptySelection);
        }
        let roots = self.roots_in(ids.iter().copied());
        if roots.is_empty() {
            return Err(FoldError::NoRoots);
        }
        let arity = form.arity();
        if !arity.accepts(roots.len()) {
            return Err(FoldError::WrongArity {
                form,
                want: arity,
                got: roots.len(),
            });
        }

        let boundary = self
            .arrows
            .iter()
            .enumerate()
            .filter(|(_, arrow)| ids.contains(&arrow.from) && !ids.contains(&arrow.to))
            .map(|(index, arrow)| (index, *arrow))
            .collect::<Vec<_>>();
        let outside_fathers = boundary
            .iter()
            .map(|(_, arrow)| arrow.to)
            .collect::<HashSet<_>>();
        if outside_fathers.len() > 1 {
            return Err(FoldError::SeveralFathers);
        }

        let selected_nodes = self
            .nodes
            .iter()
            .filter(|node| ids.contains(&node.id))
            .collect::<Vec<_>>();
        let x = selected_nodes.iter().map(|node| node.x).sum::<f64>() / selected_nodes.len() as f64;
        let y = selected_nodes
            .iter()
            .map(|node| node.y)
            .fold(f64::INFINITY, f64::min)
            - NODE_R * 2.2;
        let folded = self.add(form, text, x, y);

        if let Some(&outside_father) = outside_fathers.iter().next() {
            let insertion = boundary
                .iter()
                .map(|(index, _)| *index)
                .min()
                .unwrap_or(self.arrows.len());
            let removed = boundary
                .iter()
                .map(|(_, arrow)| *arrow)
                .collect::<HashSet<_>>();
            self.arrows.retain(|arrow| !removed.contains(arrow));
            self.arrows.insert(
                insertion.min(self.arrows.len()),
                Arrow {
                    from: folded,
                    to: outside_father,
                },
            );
        }
        for root in roots {
            self.father_of(folded, root);
        }
        let folded_block = self.induced_subgraph(ids.iter().copied().chain([folded]));
        if let Err(error) = folded_block.to_rebis() {
            *self = original;
            return Err(FoldError::InvalidSource(error.to_string()));
        }
        Ok(folded)
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn node(&self, id: NodeId) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }

    fn has(&self, id: NodeId) -> bool {
        self.nodes.iter().any(|n| n.id == id)
    }

    /// The topmost shape containing a world point, if any.
    ///
    /// Hit-testing lives here rather than in the renderer so the canvas can
    /// route every pointer event from one element and resolve the target
    /// itself — which keeps pointer coordinates in one space and makes the
    /// interaction testable without a window.
    pub fn hit(&self, x: f64, y: f64) -> Option<NodeId> {
        let shape = self
            .nodes
            .iter()
            .rev()
            .find_map(|n| n.shape().contains(x - n.x, y - n.y).then_some(n.id));
        if shape.is_some() {
            return shape;
        }
        // A flow expression is rendered as the complete blue line between its
        // children. Its midpoint handle is useful but must not be the only
        // selectable part of the form.
        self.nodes.iter().rev().find_map(|node| {
            if node.shape() != Shape::Arrow {
                return None;
            }
            let children = self.children(node.id);
            let [first, second] = children[..] else {
                return None;
            };
            let (Some(first), Some(second)) = (self.node(first), self.node(second)) else {
                return None;
            };
            (point_segment_distance_squared((x, y), (first.x, first.y), (second.x, second.y))
                <= ARROW_HIT_SLOP * ARROW_HIT_SLOP)
                .then_some(node.id)
        })
    }

    /// Children of `id`: the nodes whose arrows point at it, in draw order.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.arrows
            .iter()
            .filter(|a| a.to == id)
            .map(|a| a.from)
            .collect()
    }

    /// Every node in the block rooted at `id`: the node itself plus all of its
    /// operands, recursively. This is the set that renders as `id`'s own
    /// expression, so selecting it always yields a valid, round-trippable
    /// block. A visited guard keeps a shared node or a recursive cycle finite.
    pub fn subtree(&self, id: NodeId) -> std::collections::BTreeSet<NodeId> {
        let mut out = std::collections::BTreeSet::new();
        let mut stack = vec![id];
        while let Some(node) = stack.pop() {
            if self.node(node).is_none() || !out.insert(node) {
                continue;
            }
            stack.extend(self.children(node));
        }
        out
    }

    /// Derive the program's structural 3D form.
    ///
    /// Re-place every node with the standard layout, deriving each node's
    /// structural depth from the drawing itself.
    ///
    /// This is the drawing's own "format": the same circuit arrangement
    /// [`Mandala::from_rebis`] produces, applied to a graph that has since been
    /// drawn or dragged by hand. Only coordinates change — structure, ids, and
    /// generated source are untouched.
    pub fn relayout(&mut self) {
        let depths: Vec<(NodeId, usize)> = self
            .spatial_layout()
            .nodes
            .iter()
            .map(|node| (node.id, node.depth))
            .collect();
        self.layout(&depths);
    }

    /// Only explicit flow/child links contribute structure: roots occupy depth
    /// zero and each operand is one layer deeper. X/Y remain presentation. A
    /// path that reaches an ancestor is a recursive back-edge. Nodes in that
    /// recursive component receive a small deterministic helical offset so
    /// recursion reads as a loop rather than a flat self-crossing line.
    pub fn spatial_layout(&self) -> SpatialLayout {
        use std::collections::{HashMap, HashSet};

        /// Leaves carried by a subtree, sizing how much of a parent's ring it
        /// deserves. A provisional value is memoised before descending, so a
        /// shared node or a cycle reads that instead of recursing forever.
        fn subtree_leaves(
            mandala: &Mandala,
            id: NodeId,
            depth: usize,
            depths: &HashMap<NodeId, usize>,
            memo: &mut HashMap<NodeId, f64>,
        ) -> f64 {
            if let Some(known) = memo.get(&id) {
                return *known;
            }
            memo.insert(id, 1.0);
            let kids = layer_children(mandala, id, depth, depths);
            let total = if kids.is_empty() {
                1.0
            } else {
                kids.iter()
                    .map(|kid| subtree_leaves(mandala, *kid, depth + 1, depths, memo))
                    .sum()
            };
            memo.insert(id, total);
            total
        }

        /// The operands one structural layer below `id` — the same rule the
        /// depth walk used, so shared nodes and back-edges cannot descend.
        fn layer_children(
            mandala: &Mandala,
            id: NodeId,
            depth: usize,
            depths: &HashMap<NodeId, usize>,
        ) -> Vec<NodeId> {
            mandala
                .children(id)
                .into_iter()
                .filter(|kid| depths.get(kid).copied() == Some(depth + 1))
                .collect()
        }

        /// Place `id` at `centre`, then fan its operands onto a ring around it
        /// in the next layer's plane, each taking an angular share of the ring
        /// proportional to the subtree it carries.
        #[allow(clippy::too_many_arguments)]
        fn spread(
            mandala: &Mandala,
            id: NodeId,
            depth: usize,
            centre: (f64, f64),
            phase: f64,
            depths: &HashMap<NodeId, usize>,
            memo: &mut HashMap<NodeId, f64>,
            placed: &mut HashMap<NodeId, (f64, f64)>,
        ) {
            if placed.contains_key(&id) {
                return;
            }
            placed.insert(id, centre);
            let kids: Vec<NodeId> = layer_children(mandala, id, depth, depths)
                .into_iter()
                .filter(|kid| !placed.contains_key(kid))
                .collect();
            if kids.is_empty() {
                return;
            }
            let weights: Vec<f64> = kids
                .iter()
                .map(|kid| subtree_leaves(mandala, *kid, depth + 1, depths, memo))
                .collect();
            let total: f64 = weights.iter().sum::<f64>().max(1.0);
            // Wide subtrees earn a wider ring; each layer draws its cone a
            // golden step tighter, with a floor so rings never collapse onto
            // the node itself.
            let ring = ((CONE_ARC_PER_LEAF * total / std::f64::consts::TAU).max(CONE_MIN_RING)
                * CONE_SHRINK.powf(depth as f64 * 0.5))
            .max(NODE_R * 2.2);
            let mut angle = phase;
            for (kid, weight) in kids.into_iter().zip(weights) {
                let share = std::f64::consts::TAU * weight / total;
                let at = angle + share / 2.0;
                spread(
                    mandala,
                    kid,
                    depth + 1,
                    (centre.0 + ring * at.cos(), centre.1 + ring * at.sin()),
                    at + GOLDEN_ANGLE,
                    depths,
                    memo,
                    placed,
                );
                angle += share;
            }
        }

        fn descend(
            mandala: &Mandala,
            id: NodeId,
            depth: usize,
            path: &mut Vec<NodeId>,
            depths: &mut HashMap<NodeId, usize>,
            recursive_edges: &mut HashSet<Arrow>,
            recursive_nodes: &mut HashSet<NodeId>,
        ) {
            depths
                .entry(id)
                .and_modify(|known| *known = (*known).max(depth))
                .or_insert(depth);
            path.push(id);
            for child in mandala.children(id) {
                if let Some(at) = path.iter().position(|ancestor| *ancestor == child) {
                    recursive_edges.insert(Arrow {
                        from: child,
                        to: id,
                    });
                    recursive_nodes.extend(path[at..].iter().copied());
                    recursive_nodes.insert(child);
                    continue;
                }
                // A finite graph can be shared. Once a node has been expanded
                // at an equal or deeper layer, repeating it cannot reveal a
                // new structural depth; this also bounds mutually recursive
                // components entered through several roots.
                if depths.get(&child).is_some_and(|known| *known > depth) {
                    continue;
                }
                descend(
                    mandala,
                    child,
                    depth + 1,
                    path,
                    depths,
                    recursive_edges,
                    recursive_nodes,
                );
            }
            path.pop();
        }

        let mut roots = self
            .nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| !self.arrows.iter().any(|arrow| arrow.from == *id))
            .collect::<Vec<_>>();
        let mut depths = HashMap::new();
        let mut recursive_edges = HashSet::new();
        let mut recursive_nodes = HashSet::new();
        for root in roots.iter().copied() {
            descend(
                self,
                root,
                0,
                &mut Vec::new(),
                &mut depths,
                &mut recursive_edges,
                &mut recursive_nodes,
            );
        }
        // A closed recursive component has no ordinary root. Its first drawn
        // node is a stable synthetic root.
        for id in self.nodes.iter().map(|node| node.id) {
            if !depths.contains_key(&id) {
                roots.push(id);
                descend(
                    self,
                    id,
                    0,
                    &mut Vec::new(),
                    &mut depths,
                    &mut recursive_edges,
                    &mut recursive_nodes,
                );
            }
        }

        // ── structural placement: a golden cone tree ────────────────────────
        // The 3D reading is not the 2D drawing extruded. Each nesting layer is
        // its own plane, and every form fans its operands onto a ring around
        // itself in the next plane down. Ring radius grows with how much
        // subtree a child carries and shrinks by the golden ratio per layer, so
        // a subtree nests inside its parent's cone instead of colliding with
        // its siblings — the figure occupies real volume, and a branch's shape
        // is its syntax's shape.
        let mut leaves = HashMap::new();
        let mut placed: HashMap<NodeId, (f64, f64)> = HashMap::new();
        let mut root_ids = Vec::new();
        let mut seen_root = HashSet::new();
        for id in roots {
            if seen_root.insert(id) {
                root_ids.push(id);
            }
        }
        let root_count = root_ids.len().max(1);
        let root_ring = (CONE_ARC_PER_LEAF * root_count as f64 / std::f64::consts::TAU)
            .max(CONE_MIN_RING)
            * 1.6;
        for (index, root) in root_ids.iter().enumerate() {
            // A lone program sits on the axis; several roots share a ring so
            // independent top-level forms read as separate structures.
            let centre = if root_count == 1 {
                (0.0, 0.0)
            } else {
                let angle = std::f64::consts::TAU * index as f64 / root_count as f64;
                (root_ring * angle.cos(), root_ring * angle.sin())
            };
            spread(
                self,
                *root,
                0,
                centre,
                GOLDEN_ANGLE * index as f64,
                &depths,
                &mut leaves,
                &mut placed,
            );
        }

        let recursive_order = self
            .nodes
            .iter()
            .map(|node| node.id)
            .filter(|id| recursive_nodes.contains(id))
            .collect::<Vec<_>>();
        let recursive_count = recursive_order.len().max(1) as f64;
        let nodes = self
            .nodes
            .iter()
            .map(|node| {
                let depth = depths.get(&node.id).copied().unwrap_or_default();
                let recursive = recursive_nodes.contains(&node.id);
                let phase = recursive_order
                    .iter()
                    .position(|id| *id == node.id)
                    .map(|index| std::f64::consts::TAU * index as f64 / recursive_count)
                    .unwrap_or_default();
                let (x, y) = placed.get(&node.id).copied().unwrap_or((0.0, 0.0));
                SpatialNode {
                    id: node.id,
                    x: x + if recursive { phase.cos() * 28.0 } else { 0.0 },
                    y: y + if recursive { phase.sin() * 28.0 } else { 0.0 },
                    z: depth as f64 * LAYER_GAP
                        + if recursive {
                            phase / std::f64::consts::TAU * 90.0
                        } else {
                            0.0
                        },
                    depth,
                    recursive,
                }
            })
            .collect();
        let mut recursive_edges = recursive_edges.into_iter().collect::<Vec<_>>();
        recursive_edges.sort_by_key(|edge| (edge.from, edge.to));
        SpatialLayout {
            nodes,
            recursive_edges,
        }
    }

    /// The single node with no outgoing arrow — the program's result.
    fn root(&self) -> Result<NodeId, MandalaError> {
        if self.nodes.is_empty() {
            return Err(MandalaError::Empty);
        }
        let roots: Vec<NodeId> = self
            .nodes
            .iter()
            .map(|n| n.id)
            .filter(|id| !self.arrows.iter().any(|a| a.from == *id))
            .collect();
        match roots.len() {
            0 => Err(MandalaError::NoRoot),
            1 => Ok(roots[0]),
            _ => Err(MandalaError::ManyRoots(roots)),
        }
    }

    /// Generate Rebis source for this mandala.
    pub fn to_rebis(&self) -> Result<String, MandalaError> {
        let root = self.root()?;
        let mut on_path = HashSet::new();
        let mut seen = HashSet::new();
        let source = self.render(root, true, &mut on_path, &mut seen)?;
        if seen.len() != self.nodes.len() {
            return Err(MandalaError::Cycle);
        }
        let expression = rebis_lang::parse(&source)
            .map_err(|error| MandalaError::InvalidSource(error.to_string()))?;
        if !self.matches_expression(root, &expression) {
            return Err(MandalaError::InvalidSource(
                "a source payload changed the expression structure".to_string(),
            ));
        }
        Ok(source)
    }

    fn matches_expression(&self, id: NodeId, expression: &rebis_lang::Expr) -> bool {
        use rebis_lang::Expr;
        let Some(node) = self.node(id) else {
            return false;
        };
        let children = self.children(id);
        let child_matches = |index: usize, expression: &Expr| {
            children
                .get(index)
                .is_some_and(|id| self.matches_expression(*id, expression))
        };
        match (&node.form, expression) {
            (Form::Prompt, Expr::Prompt(text)) | (Form::Symbol, Expr::Symbol(text)) => {
                node.text == *text
            }
            (Form::Import, Expr::Import { module }) => node.text == module.to_string(),
            (Form::Quote, Expr::Quote(inner))
            | (Form::Unquote, Expr::Unquote(inner))
            | (Form::Invert, Expr::Invert(inner)) => child_matches(0, inner),
            (Form::Forward, Expr::Forward(left, right))
            | (Form::Backflow, Expr::Backflow(left, right)) => {
                child_matches(0, left) && child_matches(1, right)
            }
            (Form::Square, Expr::Square { mediator, branches }) => {
                children.len() == branches.len() + 1
                    && child_matches(0, mediator)
                    && branches
                        .iter()
                        .enumerate()
                        .all(|(index, branch)| child_matches(index + 1, branch))
            }
            (Form::Concat, Expr::Concat(items))
            | (Form::Compose, Expr::Compose(items))
            | (Form::Program, Expr::Program(items)) => {
                children.len() == items.len()
                    && items
                        .iter()
                        .enumerate()
                        .all(|(index, item)| child_matches(index, item))
            }
            (Form::Call, Expr::Call { name, args }) => {
                node.text == *name
                    && children.len() == args.len()
                    && args
                        .iter()
                        .enumerate()
                        .all(|(index, arg)| child_matches(index, arg))
            }
            (
                Form::Function(params),
                Expr::Function {
                    name,
                    params: parsed,
                    body,
                },
            ) => node.text == *name && params == parsed && child_matches(0, body),
            (Form::Input, Expr::Input { name, body }) => {
                node.text == *name && child_matches(0, body)
            }
            _ => false,
        }
    }

    fn render(
        &self,
        id: NodeId,
        at_root: bool,
        on_path: &mut HashSet<NodeId>,
        seen: &mut HashSet<NodeId>,
    ) -> Result<String, MandalaError> {
        if on_path.contains(&id) {
            return Err(MandalaError::Cycle);
        }
        if !seen.insert(id) {
            return Err(MandalaError::Shared(id));
        }
        on_path.insert(id);
        let node = self.node(id).ok_or(MandalaError::Empty)?;
        let kids = self.children(id);

        let arity = node.form.arity();
        if !arity.accepts(kids.len()) {
            return Err(MandalaError::WrongArity {
                id,
                form: node.form.clone(),
                want: arity,
                got: kids.len(),
            });
        }
        if matches!(node.form, Form::Program) && !at_root {
            return Err(MandalaError::NestedProgram(id));
        }

        let mut parts = Vec::with_capacity(kids.len());
        for kid in kids {
            parts.push(self.render(kid, false, on_path, seen)?);
        }
        on_path.remove(&id);

        let text = &node.text;
        let out = match &node.form {
            Form::Prompt => quote(text),
            Form::Symbol => text.clone(),
            Form::Import => format!("(# {text})"),
            Form::Quote => format!("'{}", parts[0]),
            Form::Unquote => format!(",{}", parts[0]),
            Form::Invert => format!("(^ {})", parts[0]),
            Form::Forward => format!("(-> {} {})", parts[0], parts[1]),
            Form::Backflow => format!("(<- {} {})", parts[0], parts[1]),
            Form::Square => format!("([{}] {})", parts[0], parts[1..].join(" ")),
            Form::Concat => format!("($ {})", parts.join(" ")),
            Form::Compose => format!("({})", parts.join(" ")),
            Form::Call => format!("({text} {})", parts.join(" ")).replace(" )", ")"),
            Form::Function(params) => {
                format!("(~ {text} ({}) {})", params.join(" "), parts[0])
            }
            Form::Input => format!("(& {text} {})", parts[0]),
            Form::Program => parts.join("\n"),
        };
        Ok(out)
    }
}

// ── loading ─────────────────────────────────────────────────────────────────

/// Why a Rebis program could not be loaded onto the canvas.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum LoadError {
    /// The source is not valid Rebis.
    Parse(String),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Parse(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for LoadError {}

/// Deterministic schematic layout: the syntax tree becomes a left-to-right
/// circuit. Nesting depth is the column; a tidy row packing stacks subtrees so
/// each form's operands sit in the next column and wire back to it.
const CIRCUIT_ORIGIN: (f64, f64) = (150.0, 130.0);
/// Vertical gap between rows (component pitch).
const ROW_GAP: f64 = 132.0;
/// The golden ratio. The column pitch is the row pitch times PHI, so the grid
/// the circuit sits on keeps a golden aspect — the earlier "divine geometry"
/// proportion, now expressed as the board's cell shape.
const PHI: f64 = 1.618_033_988_749_895;
/// Horizontal gap between columns (one nesting level), a golden step wider than
/// the row pitch so signals have room to route between stages.
const COL_GAP: f64 = ROW_GAP * PHI;

/// Structural 3D projection: one plane per nesting layer. Generous so the
/// arrow between two layers has room to draw its head between the shapes.
const LAYER_GAP: f64 = 300.0;
/// Ring circumference a single leaf claims on its parent's cone. Wide enough
/// that siblings never touch and the link between them stays visible.
const CONE_ARC_PER_LEAF: f64 = 210.0;
/// Smallest ring a parent will fan its operands onto.
const CONE_MIN_RING: f64 = 180.0;
/// Each layer draws its cone this much tighter, so a subtree nests inside its
/// parent's cone rather than sprawling across it.
const CONE_SHRINK: f64 = 1.0 / PHI;
/// The golden angle. Successive layers start their rings a golden turn apart,
/// so cones never line up and hide one another.
const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

impl Mandala {
    /// Load a Rebis program onto the canvas — the inverse of [`Self::to_rebis`].
    ///
    /// Every form in the language is drawable, so the only failure is source
    /// that does not parse.
    pub fn from_rebis(src: &str) -> Result<Self, LoadError> {
        let expr = rebis_lang::parse(src).map_err(|e| LoadError::Parse(e.to_string()))?;
        let mut m = Mandala::new();
        let mut depths: Vec<(NodeId, usize)> = Vec::new();
        m.build(&expr, 0, &mut depths);
        m.layout(&depths);
        Ok(m)
    }

    fn build(
        &mut self,
        expr: &rebis_lang::Expr,
        depth: usize,
        depths: &mut Vec<(NodeId, usize)>,
    ) -> NodeId {
        use rebis_lang::Expr;

        // Every arm is the same move: pick the form and text, then attach the
        // ordered children. The uniformity is the point of the abstraction.
        let (form, text, kids): (Form, String, Vec<&Expr>) = match expr {
            Expr::Prompt(s) => (Form::Prompt, s.clone(), vec![]),
            Expr::Symbol(s) => (Form::Symbol, s.clone(), vec![]),
            Expr::Import { module } => (Form::Import, module.to_string(), vec![]),
            Expr::Quote(x) => (Form::Quote, String::new(), vec![x]),
            Expr::Unquote(x) => (Form::Unquote, String::new(), vec![x]),
            Expr::Invert(x) => (Form::Invert, String::new(), vec![x]),
            Expr::Forward(a, b) => (Form::Forward, String::new(), vec![a, b]),
            Expr::Backflow(a, b) => (Form::Backflow, String::new(), vec![a, b]),
            Expr::Square { mediator, branches } => {
                let mut kids: Vec<&Expr> = vec![mediator];
                kids.extend(branches.iter());
                (Form::Square, String::new(), kids)
            }
            Expr::Concat(v) => (Form::Concat, String::new(), v.iter().collect()),
            Expr::Compose(v) => (Form::Compose, String::new(), v.iter().collect()),
            Expr::Call { name, args } => (Form::Call, name.clone(), args.iter().collect()),
            Expr::Function { name, params, body } => (
                Form::Function(params.clone()),
                name.clone(),
                vec![body.as_ref()],
            ),
            Expr::Input { name, body } => (Form::Input, name.clone(), vec![body.as_ref()]),
            Expr::Program(v) => (Form::Program, String::new(), v.iter().collect()),
        };

        let id = self.add(form, text, 0.0, 0.0);
        depths.push((id, depth));
        for kid in kids {
            let child = self.build(kid, depth + 1, depths);
            self.father_of(id, child);
        }
        id
    }

    /// Lay the syntax tree out as a left-to-right circuit.
    ///
    /// Nesting depth is the **column**: a form sits one column left of its
    /// operands, so the source reads as stages wired rightward. A tidy row
    /// packing gives every leaf its own row and centres each form on its
    /// children's rows, so subtrees stack without overlap and a form sits level
    /// with the block of operands it drives. The grid's cell is golden (columns
    /// are [`PHI`]× the row pitch). Coordinates are presentation only and never
    /// affect generated Rebis.
    fn layout(&mut self, depths: &[(NodeId, usize)]) {
        use std::collections::{HashMap, HashSet};

        let depth_of: HashMap<NodeId, usize> =
            depths.iter().map(|(id, depth)| (*id, *depth)).collect();

        // Tidy row packing: post-order over the syntax tree. A leaf claims the
        // next free row; a form centres on the rows of the operands it drives.
        // A visited guard keeps shared nodes and cycles finite.
        fn pack(
            m: &Mandala,
            id: NodeId,
            depth_of: &HashMap<NodeId, usize>,
            rows: &mut HashMap<NodeId, f64>,
            seen: &mut HashSet<NodeId>,
            next_leaf: &mut f64,
        ) {
            if !seen.insert(id) {
                return;
            }
            let depth = depth_of.get(&id).copied().unwrap_or(0);
            let kids: Vec<NodeId> = m
                .children(id)
                .into_iter()
                .filter(|kid| depth_of.get(kid).copied() == Some(depth + 1))
                .collect();
            for kid in &kids {
                pack(m, *kid, depth_of, rows, seen, next_leaf);
            }
            let child_rows: Vec<f64> = kids.iter().filter_map(|kid| rows.get(kid).copied()).collect();
            let row = if child_rows.is_empty() {
                let row = *next_leaf;
                *next_leaf += 1.0;
                row
            } else {
                child_rows.iter().sum::<f64>() / child_rows.len() as f64
            };
            rows.insert(id, row);
        }

        let mut rows = HashMap::new();
        let mut seen = HashSet::new();
        let mut next_leaf = 0.0;
        for (id, depth) in depths {
            if *depth == 0 {
                pack(self, *id, &depth_of, &mut rows, &mut seen, &mut next_leaf);
            }
        }
        // Any node not reached from a root (defensive) gets a trailing row.
        for (id, _) in depths {
            if !rows.contains_key(id) {
                rows.insert(*id, next_leaf);
                next_leaf += 1.0;
            }
        }

        let placed: Vec<(NodeId, f64, f64)> = depths
            .iter()
            .map(|(id, depth)| {
                let x = CIRCUIT_ORIGIN.0 + *depth as f64 * COL_GAP;
                let y = CIRCUIT_ORIGIN.1 + rows.get(id).copied().unwrap_or(0.0) * ROW_GAP;
                (*id, x, y)
            })
            .collect();
        for (id, x, y) in placed {
            self.move_to(id, x, y);
        }
    }
}

/// Render a label as a Rebis string literal.
fn quote(label: &str) -> String {
    let mut out = String::with_capacity(label.len() + 2);
    out.push('"');
    for ch in label.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            _ => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The contract that matters: source loaded onto the canvas and written
    /// back must parse to the *same expression*. Compared as ASTs, so
    /// incidental spacing differences never mask a real change.
    fn assert_round_trip(src: &str) {
        let want = rebis_lang::parse(src).unwrap_or_else(|e| panic!("fixture {src}: {e}"));
        let m = Mandala::from_rebis(src).unwrap_or_else(|e| panic!("load {src}: {e}"));
        let out = m.to_rebis().unwrap_or_else(|e| panic!("render {src}: {e}"));
        let got = rebis_lang::parse(&out)
            .unwrap_or_else(|e| panic!("regenerated source did not parse: {out}: {e}"));
        assert_eq!(want, got, "{src}\n  regenerated as: {out}");
    }

    #[test]
    fn every_form_round_trips() {
        // One fixture per Expr variant — the whole grammar.
        assert_round_trip("\"a prompt\""); // Prompt
        assert_round_trip("(~ f (x) x)"); // Function + Symbol
        assert_round_trip("(# std/flow)"); // Import
        assert_round_trip("(~ f (x) '(-> x x))"); // Quote
        assert_round_trip("(~ f (x) '(-> ,x ,x))"); // Unquote
        assert_round_trip("(^ (-> a b))"); // Invert
        assert_round_trip("(-> \"a\" \"b\")"); // Forward
        assert_round_trip("(<- \"a\" \"b\")"); // Backflow
        assert_round_trip("([\"m\"] \"a\" \"b\")"); // Square
        assert_round_trip("($ \"x\" \"y\")"); // Concat
        assert_round_trip("((\"local\") \"sub\")"); // Compose
        assert_round_trip("(f \"a\" \"b\")"); // Call
        assert_round_trip("\"p\" \"q\""); // Program
    }

    #[test]
    fn round_trips_a_realistic_program() {
        assert_round_trip(
            "((# std/flow)\n\
             (~ investigate (target) (-> target \"Write a verified report\"))\n\
             ([\"Combine both reports\"]\n\
              (investigate \"Inspect the oven\")\n\
              (investigate \"Analyze the refunds\")))",
        );
    }

    #[test]
    fn round_trips_nested_macro_loops() {
        // The macro-loop example from docs/REBIS.md.
        assert_round_trip(
            "((~ step (value) (-> value \"Improve once.\"))\n\
             (~ done (value) (-> value \"Is it finished?\"))\n\
             (~ loop (value work stop)\n\
               ([(stop value)] value (loop (work value) work stop)))\n\
             (loop \"Initial implementation\" step done))",
        );
    }

    #[test]
    fn round_trips_escaped_prompts() {
        assert_round_trip("\"say \\\"hi\\\" \\\\ now\"");
    }

    #[test]
    fn call_with_no_arguments_round_trips() {
        assert_round_trip("(f)");
    }

    #[test]
    fn syntax_inverter_round_trips_as_a_drawn_unary_form() {
        assert_round_trip("(^ (-> \"source\" (<- a b)))");

        let mandala = Mandala::from_rebis("(^ (-> a b))").unwrap();
        let inverter = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Invert)
            .expect("inverter node");
        assert_eq!(inverter.shape(), Shape::Caret);
        assert_eq!(mandala.children(inverter.id).len(), 1);
    }

    // ── shapes are a rendering of the form ─────────────────────────────────

    #[test]
    fn sigil_forms_are_drawn_as_their_sigil() {
        assert_eq!(Form::Concat.shape(), Shape::Dollar);
        assert_eq!(Form::Function(vec![]).shape(), Shape::Tilde);
        assert_eq!(Form::Import.shape(), Shape::Hash);
        assert_eq!(Form::Quote.shape(), Shape::Quote);
        assert_eq!(Form::Unquote.shape(), Shape::Comma);
        assert_eq!(Form::Invert.shape(), Shape::Caret);
    }

    #[test]
    fn geometry_never_composes_forms_but_father_of_does() {
        let mut mandala = Mandala::new();
        // Exact overlap must still be two disconnected expressions.
        let evidence = mandala.add(Form::Prompt, "evidence", 40.0, 40.0);
        let branch = mandala.add(Form::Prompt, "branch", 40.0, 40.0);
        let square = mandala.add(Form::Square, "", 40.0, 40.0);
        let disconnected_error = mandala.to_rebis().unwrap_err().to_string();
        let disconnected_layout = mandala.spatial_layout();
        assert!(mandala.children(square).is_empty());
        assert_eq!(disconnected_layout.node(evidence).unwrap().depth, 0);
        assert_eq!(disconnected_layout.node(square).unwrap().depth, 0);

        // Moving either form—even visibly inside the other—does not alter the
        // program. Only the explicit relation below creates composition.
        mandala.move_to(evidence, 41.0, 41.0);
        assert_eq!(
            mandala.to_rebis().unwrap_err().to_string(),
            disconnected_error
        );
        mandala.father_of(square, evidence);
        mandala.father_of(square, branch);
        assert_eq!(mandala.children(square), vec![evidence, branch]);
        let linked_layout = mandala.spatial_layout();
        assert_eq!(linked_layout.node(square).unwrap().depth, 0);
        assert_eq!(linked_layout.node(evidence).unwrap().depth, 1);
        assert_eq!(mandala.to_rebis().unwrap(), "([\"evidence\"] \"branch\")");
    }

    #[test]
    fn spatial_layout_uses_structural_nesting_as_depth() {
        let mandala = Mandala::from_rebis("(^ (-> a b))").unwrap();
        let layout = mandala.spatial_layout();
        let inverter = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Invert)
            .unwrap();
        let flow = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Forward)
            .unwrap();
        assert_eq!(layout.node(inverter.id).unwrap().depth, 0);
        assert_eq!(layout.node(flow.id).unwrap().depth, 1);
        for child in mandala.children(flow.id) {
            assert_eq!(layout.node(child).unwrap().depth, 2);
        }
    }

    #[test]
    fn spatial_layout_occupies_volume_rather_than_extruding_the_drawing() {
        // Two sibling subtrees under one square. In an extruded 2D layout the
        // siblings would share the drawing's plane; as a cone tree they fan
        // around their parent, so they differ on every axis.
        let mandala =
            Mandala::from_rebis("([\"m\"] (-> \"a\" \"b\") (-> \"c\" \"d\"))").unwrap();
        let layout = mandala.spatial_layout();
        let root = mandala
            .nodes()
            .iter()
            .find(|node| node.form == Form::Square)
            .unwrap();
        let branches: Vec<&SpatialNode> = mandala
            .children(root.id)
            .into_iter()
            .filter_map(|id| layout.node(id))
            .collect();
        assert!(branches.len() >= 2);
        // Siblings share a layer (same z) but sit apart in that layer's plane.
        assert!(branches.windows(2).all(|pair| pair[0].z == pair[1].z));
        assert!(branches
            .windows(2)
            .all(|pair| pair[0].x != pair[1].x || pair[0].y != pair[1].y));
        // The figure has real extent on all three axes, not a flat sheet.
        let spread = |pick: fn(&SpatialNode) -> f64| {
            let values: Vec<f64> = layout.nodes.iter().map(pick).collect();
            values.iter().cloned().fold(f64::MIN, f64::max)
                - values.iter().cloned().fold(f64::MAX, f64::min)
        };
        assert!(spread(|n| n.x) > 1.0, "no width");
        assert!(spread(|n| n.y) > 1.0, "no height");
        assert!(spread(|n| n.z) > 1.0, "no depth");
        // Deeper syntax is genuinely deeper in space.
        let deepest = layout.nodes.iter().map(|n| n.depth).max().unwrap();
        assert!(deepest >= 2, "nesting should reach a third layer");
    }

    #[test]
    fn spatial_layout_lifts_closed_recursion_without_mutating_the_graph() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Compose, "", 0.0, 0.0);
        let right = mandala.add(Form::Compose, "", 100.0, 0.0);
        mandala.connect(left, right);
        mandala.connect(right, left);
        let before = mandala.clone();

        let layout = mandala.spatial_layout();

        assert_eq!(mandala.arrows, before.arrows);
        assert_eq!(mandala.next_id, before.next_id);
        assert_eq!(mandala.nodes.len(), before.nodes.len());
        for (actual, expected) in mandala.nodes.iter().zip(&before.nodes) {
            assert_eq!(actual.id, expected.id);
            assert_eq!(actual.form, expected.form);
            assert_eq!(actual.text, expected.text);
            assert_eq!((actual.x, actual.y), (expected.x, expected.y));
        }
        assert_eq!(layout.nodes.len(), 2);
        assert!(!layout.recursive_edges.is_empty());
        assert!(layout.nodes.iter().all(|node| node.recursive));
        assert_ne!(layout.nodes[0].z, layout.nodes[1].z);
    }

    // ── drawing an arrow creates the flow node ─────────────────────────────

    #[test]
    fn an_arrow_between_two_shapes_writes_a_forward() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "Reproduce the bug", 0.0, 0.0);
        let b = m.add(Form::Prompt, "Write the fix", 300.0, 0.0);
        // One gesture; the `->` appears behind it.
        m.flow(a, b, Form::Forward).unwrap();
        assert_eq!(
            m.to_rebis().unwrap(),
            "(-> \"Reproduce the bug\" \"Write the fix\")"
        );
    }

    #[test]
    fn marquee_crossing_a_flow_line_selects_its_arrow_node() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = mandala.flow(left, right, Form::Forward).unwrap();

        let selected = mandala.nodes_in_rect(WorldRect::from_points((95.0, -3.0), (105.0, 3.0)));
        assert_eq!(selected, vec![flow]);
    }

    #[test]
    fn induced_subgraph_retains_only_selected_nodes_and_internal_links() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = mandala.flow(left, right, Form::Forward).unwrap();
        mandala.add(Form::Prompt, "outside", 400.0, 0.0);

        let selected = mandala.induced_subgraph([left, right, flow]);
        assert_eq!(selected.nodes().len(), 3);
        assert_eq!(selected.arrows().len(), 2);
        assert_eq!(selected.to_rebis().unwrap(), "(-> \"left\" \"right\")");
    }

    #[test]
    fn appending_a_copy_remaps_ids_positions_and_internal_links() {
        let mut source = Mandala::new();
        let father = source.add(Form::Compose, "", 10.0, 20.0);
        let first = source.add(Form::Symbol, "a", -30.0, 20.0);
        let second = source.add(Form::Prompt, "b", 50.0, 20.0);
        source.father_of(father, first);
        source.father_of(father, second);

        let mut target = Mandala::new();
        let existing = target.add(Form::Symbol, "existing", 0.0, 0.0);
        let pasted = target.append_copy(&source, (24.0, 32.0));

        assert_eq!(pasted.len(), 3);
        assert!(pasted.iter().all(|id| *id != existing));
        let copied_father = pasted[0];
        assert_eq!(target.children(copied_father), vec![pasted[1], pasted[2]]);
        let copied = target.node(copied_father).unwrap();
        assert_eq!((copied.x, copied.y), (34.0, 52.0));
        assert_eq!(copied.form, Form::Compose);
        assert_eq!(target.node(pasted[1]).unwrap().text, "a");
        assert_eq!(target.node(pasted[2]).unwrap().text, "b");
    }

    #[test]
    fn a_rendered_flow_line_is_selectable_away_from_its_handle() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = mandala.flow(left, right, Form::Forward).unwrap();

        assert_eq!(mandala.hit(45.0, 4.0), Some(flow));
        assert_eq!(mandala.hit(45.0, ARROW_HIT_SLOP + 2.0), None);
        // Endpoint forms retain priority over the line running into them.
        assert_eq!(mandala.hit(5.0, 0.0), Some(left));
    }

    #[test]
    fn folding_uses_selected_roots_instead_of_flattening_nested_nodes() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Prompt, "left", 0.0, 0.0);
        let right = mandala.add(Form::Prompt, "right", 200.0, 0.0);
        let flow = mandala.flow(left, right, Form::Forward).unwrap();

        assert_eq!(mandala.roots_in([left, right, flow]), vec![flow]);
        let quote = mandala
            .fold_selection([left, right, flow], Form::Quote, "")
            .unwrap();
        assert_eq!(mandala.children(quote), vec![flow]);
        assert_eq!(mandala.to_rebis().unwrap(), "'(-> \"left\" \"right\")");
    }

    #[test]
    fn folding_rewires_one_outside_father_and_checks_arity() {
        let mut mandala = Mandala::new();
        let outer = mandala.add(Form::Compose, "", 100.0, -100.0);
        let left = mandala.add(Form::Symbol, "left", 0.0, 0.0);
        let right = mandala.add(Form::Symbol, "right", 200.0, 0.0);
        mandala.father_of(outer, left);
        mandala.father_of(outer, right);

        let before = mandala.clone();
        assert!(matches!(
            mandala.fold_selection([left, right], Form::Quote, ""),
            Err(FoldError::WrongArity { got: 2, .. })
        ));
        assert_eq!(mandala.nodes().len(), before.nodes().len());

        let square = mandala
            .fold_selection([left, right], Form::Square, "")
            .unwrap();
        assert_eq!(mandala.children(square), vec![left, right]);
        assert_eq!(mandala.children(outer), vec![square]);
        assert_eq!(mandala.to_rebis().unwrap(), "(([left] right))");
    }

    #[test]
    fn folding_rejects_ambiguous_source_and_rolls_back_atomically() {
        let mut mandala = Mandala::new();
        let left = mandala.add(Form::Symbol, "left", 0.0, 0.0);
        let right = mandala.add(Form::Symbol, "right", 200.0, 0.0);
        let before_nodes = mandala.nodes().len();

        assert!(matches!(
            mandala.fold_selection([left, right], Form::Compose, ""),
            Err(FoldError::InvalidSource(_))
        ));
        assert_eq!(mandala.nodes().len(), before_nodes);
        assert!(mandala.arrows().is_empty());
    }

    #[test]
    fn a_reverse_arrow_writes_a_backflow() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Prompt, "b", 300.0, 0.0);
        m.flow(a, b, Form::Backflow).unwrap();
        assert_eq!(m.to_rebis().unwrap(), "(<- \"a\" \"b\")");
    }

    #[test]
    fn the_flow_node_lands_between_its_endpoints() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 100.0);
        let b = m.add(Form::Prompt, "b", 400.0, 100.0);
        let f = m.flow(a, b, Form::Forward).unwrap();
        let n = m.node(f).unwrap();
        assert_eq!(n.x, 200.0, "horizontally centred between a and b");
        assert!(n.y < 100.0, "nudged clear of the line between them");
    }

    #[test]
    fn flows_chain_into_nested_arrows() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Prompt, "b", 100.0, 0.0);
        let c = m.add(Form::Prompt, "c", 200.0, 0.0);
        let first = m.flow(a, b, Form::Forward).unwrap();
        // Arrow from the first flow into c chains them, matching how `->`
        // folds left in the language.
        m.flow(first, c, Form::Forward).unwrap();
        assert_eq!(m.to_rebis().unwrap(), "(-> (-> \"a\" \"b\") \"c\")");
    }

    #[test]
    fn flow_refuses_a_self_link_or_a_missing_shape() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let ghost = NodeId(999);
        assert!(m.flow(a, a, Form::Forward).is_none());
        assert!(m.flow(a, ghost, Form::Forward).is_none());
        assert!(m.flow(ghost, a, Form::Forward).is_none());
        // Nothing was added by the refused attempts.
        assert_eq!(m.nodes().len(), 1);
    }

    #[test]
    fn flow_is_drawn_as_an_arrow_not_a_box() {
        // The arrow between two shapes *is* the form; there is no mediator
        // block to place, and none appears on the canvas.
        assert_eq!(Form::Forward.shape(), Shape::Arrow);
        assert_eq!(Form::Backflow.shape(), Shape::Arrow);
        assert!(Shape::Arrow.strokes().is_empty());
    }

    #[test]
    fn an_arrow_keeps_only_a_small_handle() {
        // A full-size target would swallow clicks meant for the shapes the
        // arrow runs between.
        assert!(Shape::Arrow.contains(0.0, 0.0));
        assert!(Shape::Arrow.contains(ARROW_HANDLE - 1.0, 0.0));
        assert!(!Shape::Arrow.contains(ARROW_HANDLE + 1.0, 0.0));
        assert!(!Shape::Arrow.contains(NODE_R - 1.0, 0.0));
    }

    #[test]
    fn a_reversed_arrow_is_the_same_program_as_a_backflow() {
        // `(<- a b)` is defined as `(-> b a)`, so direction alone expresses
        // both and only one arrow tool is needed.
        let mut fwd = Mandala::new();
        let a = fwd.add(Form::Prompt, "a", 0.0, 0.0);
        let b = fwd.add(Form::Prompt, "b", 100.0, 0.0);
        fwd.flow(b, a, Form::Forward).unwrap();

        let mut back = Mandala::new();
        let a2 = back.add(Form::Prompt, "a", 0.0, 0.0);
        let b2 = back.add(Form::Prompt, "b", 100.0, 0.0);
        back.flow(a2, b2, Form::Backflow).unwrap();

        // Same value flow: b into a.
        let f = rebis_lang::parse(&fwd.to_rebis().unwrap()).unwrap();
        let bk = rebis_lang::parse(&back.to_rebis().unwrap()).unwrap();
        assert_eq!(fwd.to_rebis().unwrap(), "(-> \"b\" \"a\")");
        assert_eq!(back.to_rebis().unwrap(), "(<- \"a\" \"b\")");
        // They are distinct syntax for the same routing, so both must load.
        assert!(matches!(f, rebis_lang::Expr::Forward(..)));
        assert!(matches!(bk, rebis_lang::Expr::Backflow(..)));
    }

    #[test]
    fn a_symbol_is_a_diamond() {
        assert_eq!(Form::Symbol.shape(), Shape::Diamond);
    }

    #[test]
    fn the_diamond_is_hit_tested_against_its_corners() {
        let mut m = Mandala::new();
        let id = m.add(Form::Symbol, "x", 0.0, 0.0);
        // Centre and the four points are inside.
        assert_eq!(m.hit(0.0, 0.0), Some(id));
        assert_eq!(m.hit(NODE_R - 1.0, 0.0), Some(id));
        assert_eq!(m.hit(0.0, NODE_R - 1.0), Some(id));
        // The corner of the bounding box is outside a diamond, unlike a circle
        // or a square.
        assert_eq!(m.hit(NODE_R * 0.8, NODE_R * 0.8), None);
        assert_eq!(m.hit(NODE_R + 1.0, 0.0), None);
    }

    #[test]
    fn structural_forms_use_their_declared_outlines() {
        assert_eq!(Form::Prompt.shape(), Shape::Circle);
        assert_eq!(Form::Compose.shape(), Shape::Oval);
        for f in [Form::Square, Form::Program] {
            assert_eq!(f.shape(), Shape::Square, "{}", f.name());
        }
        // A call is a box in motion; an input port is an inlet.
        assert_eq!(Form::Call.shape(), Shape::Parallelogram);
        assert_eq!(Form::Input.shape(), Shape::Amp);
    }

    #[test]
    fn every_form_has_a_distinct_enough_drawing() {
        // A distinct shape is used by exactly one form, so the canvas is
        // unambiguous: seeing a `$` can only mean concat, a `◇` only a symbol.
        let mut distinct = Vec::new();
        for (_, make, _) in Form::ALL {
            let s = make().shape();
            if !matches!(s, Shape::Circle | Shape::Square) {
                assert!(!distinct.contains(&s), "two forms share the shape {s:?}");
                distinct.push(s);
            }
        }
        assert_eq!(
            distinct.len(),
            10,
            "expected ◇ oval $ ~ # ' , ^ plus the call parallelogram and & inlet"
        );
    }

    #[test]
    fn every_sigil_has_strokes_and_the_outlines_have_none() {
        for s in [
            Shape::Dollar,
            Shape::Tilde,
            Shape::Hash,
            Shape::Quote,
            Shape::Comma,
            Shape::Caret,
        ] {
            assert!(!s.strokes().is_empty(), "{s:?} draws nothing");
        }
        for s in [Shape::Circle, Shape::Square, Shape::Oval, Shape::Diamond] {
            assert!(s.strokes().is_empty(), "{s:?} is an outline, not a sigil");
        }
    }

    #[test]
    fn sigil_strokes_stay_inside_the_shape() {
        // A stroke wandering outside the node would paint over its neighbours
        // and break the illusion that the sigil *is* the node.
        let limit = NODE_R as f32;
        for s in [
            Shape::Dollar,
            Shape::Tilde,
            Shape::Hash,
            Shape::Quote,
            Shape::Comma,
            Shape::Caret,
        ] {
            for stroke in s.strokes() {
                let points: Vec<(f32, f32)> = match stroke {
                    Stroke::Poly(p) => p.to_vec(),
                    Stroke::Cubic(p) => p.to_vec(),
                };
                for (x, y) in points {
                    assert!(
                        x.abs() <= limit && y.abs() <= limit,
                        "{s:?} stroke point ({x}, {y}) escapes the node"
                    );
                }
            }
        }
    }

    #[test]
    fn polylines_have_at_least_two_points() {
        for s in [Shape::Hash, Shape::Dollar, Shape::Caret] {
            for stroke in s.strokes() {
                if let Stroke::Poly(p) = stroke {
                    assert!(p.len() >= 2, "{s:?} has a polyline with {} points", p.len());
                }
            }
        }
    }

    #[test]
    fn the_diamond_corners_match_its_hit_test() {
        for (x, y) in Shape::diamond_points() {
            // Corners sit exactly on the boundary, so just inside must hit.
            let (ix, iy) = (x as f64 * 0.98, y as f64 * 0.98);
            assert!(
                Shape::Diamond.contains(ix, iy),
                "({x}, {y}) should be inside"
            );
            let (ox, oy) = (x as f64 * 1.05, y as f64 * 1.05);
            assert!(
                !Shape::Diamond.contains(ox, oy),
                "({x}, {y}) should be outside"
            );
        }
    }

    #[test]
    fn the_compose_oval_uses_elliptical_hit_geometry() {
        assert_eq!(Form::Compose.shape(), Shape::Oval);
        assert!(Shape::Oval.contains(NODE_R - 1.0, 0.0));
        assert!(Shape::Oval.contains(0.0, NODE_RY - 1.0));
        assert!(!Shape::Oval.contains(NODE_R * 0.8, NODE_RY * 0.8));
        assert!(!Shape::Oval.contains(0.0, NODE_RY + 1.0));
    }

    #[test]
    fn sigils_keep_a_round_hit_target() {
        // The glyphs are thin strokes; the clickable area must stay generous,
        // so a sigil is hit anywhere inside the full disc.
        for shape in [
            Shape::Dollar,
            Shape::Tilde,
            Shape::Hash,
            Shape::Quote,
            Shape::Comma,
            Shape::Caret,
        ] {
            assert!(shape.contains(NODE_R * 0.7, NODE_R * 0.7), "{shape:?}");
            assert!(!shape.contains(NODE_R + 1.0, 0.0), "{shape:?}");
        }
        // The box is wider than it is tall.
        assert!(Shape::Square.contains(NODE_R - 1.0, 0.0));
        assert!(!Shape::Square.contains(0.0, NODE_RY + 1.0));
    }

    #[test]
    fn a_sigil_node_is_clickable_across_its_whole_disc() {
        let mut m = Mandala::new();
        let id = m.add(Form::Concat, "", 0.0, 0.0);
        // Nowhere near the drawn stroke, but inside the target.
        assert_eq!(m.hit(NODE_R - 2.0, 0.0), Some(id));
        assert_eq!(m.hit(0.0, NODE_R - 2.0), Some(id));
        assert_eq!(m.hit(NODE_R + 2.0, 0.0), None);
    }

    #[test]
    fn sigil_shapes_carry_no_caption() {
        let mut m = Mandala::new();
        let c = m.add(Form::Concat, "", 0.0, 0.0);
        let q = m.add(Form::Quote, "", 0.0, 0.0);
        let i = m.add(Form::Invert, "", 0.0, 0.0);
        assert_eq!(m.node(c).unwrap().caption(), "");
        assert_eq!(m.node(q).unwrap().caption(), "");
        assert_eq!(m.node(i).unwrap().caption(), "");
        // A named sigil still reports its name for the renderer to place.
        let f = m.add(Form::Function(vec!["x".into()]), "twice", 0.0, 0.0);
        assert_eq!(m.node(f).unwrap().caption(), "twice");
    }

    #[test]
    fn the_palette_covers_every_placeable_form() {
        // Guards against adding an Expr variant without a way to draw it.
        // Forward and backflow are drawn as arrows, not placed, so they are the
        // only two absent here.
        let names: HashSet<&str> = Form::ALL.iter().map(|(_, f, _)| f().name()).collect();
        for expected in [
            "prompt", "symbol", "import", "quote", "unquote", "invert", "square", "concat",
            "compose", "call", "macro", "program", "input",
        ] {
            assert!(names.contains(expected), "palette is missing {expected}");
        }
        assert!(!names.contains("forward"), "forward is drawn, not placed");
        assert!(!names.contains("backflow"), "backflow is drawn, not placed");
    }

    // ── arity ──────────────────────────────────────────────────────────────

    #[test]
    fn wrong_arity_is_reported_with_the_form() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let f = m.add(Form::Forward, "", 1.0, 0.0);
        m.connect(a, f); // forward needs two children, has one
        match m.to_rebis() {
            Err(MandalaError::WrongArity { form, got, .. }) => {
                assert_eq!(form, Form::Forward);
                assert_eq!(got, 1);
            }
            other => panic!("expected an arity error, got {other:?}"),
        }
    }

    #[test]
    fn a_prompt_cannot_take_children() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Prompt, "b", 1.0, 0.0);
        m.connect(a, b);
        assert!(matches!(m.to_rebis(), Err(MandalaError::WrongArity { .. })));
    }

    #[test]
    fn nested_program_is_rejected() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Prompt, "b", 0.0, 1.0);
        let inner = m.add(Form::Program, "", 1.0, 0.0);
        m.connect(a, inner);
        m.connect(b, inner);
        let outer = m.add(Form::Quote, "", 2.0, 0.0);
        m.connect(inner, outer);
        assert_eq!(m.to_rebis(), Err(MandalaError::NestedProgram(inner)));
    }

    #[test]
    fn child_order_follows_draw_order() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "first", 0.0, 0.0);
        let b = m.add(Form::Prompt, "second", 0.0, 1.0);
        let med = m.add(Form::Prompt, "m", 0.0, 2.0);
        let sq = m.add(Form::Square, "", 1.0, 0.0);
        m.connect(med, sq);
        m.connect(b, sq);
        m.connect(a, sq);
        assert_eq!(m.to_rebis().unwrap(), "([\"m\"] \"second\" \"first\")");
    }

    // ── graph rules ────────────────────────────────────────────────────────

    #[test]
    fn empty_mandala_is_an_error() {
        assert_eq!(Mandala::new().to_rebis(), Err(MandalaError::Empty));
    }

    #[test]
    fn two_disconnected_roots_are_rejected() {
        let mut m = Mandala::new();
        m.add(Form::Prompt, "a", 0.0, 0.0);
        m.add(Form::Prompt, "b", 1.0, 0.0);
        assert!(matches!(
            m.to_rebis(),
            Err(MandalaError::ManyRoots(ids)) if ids.len() == 2
        ));
    }

    #[test]
    fn cycles_are_rejected() {
        let mut m = Mandala::new();
        let a = m.add(Form::Quote, "", 0.0, 0.0);
        let b = m.add(Form::Quote, "", 1.0, 0.0);
        m.connect(a, b);
        m.connect(b, a);
        assert_eq!(m.to_rebis(), Err(MandalaError::NoRoot));
    }

    #[test]
    fn cycle_behind_a_root_is_rejected() {
        let mut m = Mandala::new();
        let a = m.add(Form::Quote, "", 0.0, 0.0);
        let b = m.add(Form::Quote, "", 1.0, 0.0);
        let out = m.add(Form::Quote, "", 2.0, 0.0);
        m.connect(a, b);
        m.connect(b, a);
        m.connect(b, out);
        assert_eq!(m.to_rebis(), Err(MandalaError::Cycle));
    }

    #[test]
    fn shared_visual_nodes_are_rejected_instead_of_duplicated() {
        let mut mandala = Mandala::new();
        let value = mandala.add(Form::Symbol, "value", 0.0, 0.0);
        let left = mandala.add(Form::Quote, "", 100.0, -50.0);
        let right = mandala.add(Form::Quote, "", 100.0, 50.0);
        let program = mandala.add(Form::Program, "", 200.0, 0.0);
        mandala.father_of(left, value);
        mandala.father_of(right, value);
        mandala.father_of(program, left);
        mandala.father_of(program, right);

        assert_eq!(mandala.to_rebis(), Err(MandalaError::Shared(value)));
    }

    #[test]
    fn invalid_source_payload_is_an_exact_generation_error() {
        let mut mandala = Mandala::new();
        mandala.add(Form::Symbol, "not a symbol", 0.0, 0.0);
        assert!(matches!(
            mandala.to_rebis(),
            Err(MandalaError::InvalidSource(_))
        ));
    }

    #[test]
    fn removing_a_node_drops_its_arrows() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Prompt, "b", 1.0, 0.0);
        m.connect(a, b);
        m.remove(a);
        assert!(m.arrows().is_empty());
        assert_eq!(m.to_rebis().unwrap(), "\"b\"");
    }

    #[test]
    fn self_and_duplicate_arrows_are_ignored() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        let b = m.add(Form::Quote, "", 1.0, 0.0);
        m.connect(a, a);
        m.connect(a, b);
        m.connect(a, b);
        assert_eq!(m.arrows().len(), 1);
    }

    #[test]
    fn ids_are_not_reused_after_removal() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 0.0, 0.0);
        m.remove(a);
        let b = m.add(Form::Prompt, "b", 0.0, 0.0);
        assert_ne!(a, b);
    }

    #[test]
    fn rejects_invalid_source() {
        assert!(matches!(
            Mandala::from_rebis("(-> \"unclosed"),
            Err(LoadError::Parse(_))
        ));
    }

    #[test]
    fn subtree_is_a_node_and_all_its_operands() {
        // ([m] (-> a b) c): selecting the square must pull in the whole block;
        // selecting the arrow pulls only its own two operands.
        let m = Mandala::from_rebis("([\"m\"] (-> \"a\" \"b\") \"c\")").unwrap();
        let square = m.nodes().iter().find(|n| n.form == Form::Square).unwrap();
        let arrow = m.nodes().iter().find(|n| n.form == Form::Forward).unwrap();

        let whole = m.subtree(square.id);
        assert_eq!(whole.len(), m.nodes().len(), "square selects everything");

        let branch = m.subtree(arrow.id);
        assert!(branch.contains(&arrow.id));
        assert_eq!(branch.len(), 3, "arrow + its two operands");
        assert!(!branch.contains(&square.id), "does not climb to the parent");

        // The induced block round-trips to that arrow's own source.
        let block = m.induced_subgraph(branch).to_rebis().unwrap();
        assert_eq!(block, "(-> \"a\" \"b\")");
    }

    #[test]
    fn subtree_terminates_on_a_cycle() {
        let mut m = Mandala::new();
        let a = m.add(Form::Compose, "", 0.0, 0.0);
        let b = m.add(Form::Compose, "", 10.0, 0.0);
        m.connect(a, b);
        m.connect(b, a);
        assert_eq!(m.subtree(a).len(), 2, "both nodes, no infinite loop");
    }

    #[test]
    fn loaded_nodes_get_distinct_positions() {
        let m = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let mut seen = HashSet::new();
        for n in m.nodes() {
            assert!(
                seen.insert((n.x.to_bits(), n.y.to_bits())),
                "two nodes share a position"
            );
        }
        // The form sits one column left of the operands it drives, and those
        // operands share that next column, each on its own row.
        let sq = m.nodes().iter().find(|n| n.form == Form::Square).unwrap();
        let operands: Vec<&Node> = m.nodes().iter().filter(|n| n.id != sq.id).collect();
        assert!(
            operands.iter().all(|n| n.x > sq.x),
            "operands wire rightward from their form"
        );
        let columns = operands
            .iter()
            .map(|n| n.x.to_bits())
            .collect::<HashSet<_>>();
        assert_eq!(columns.len(), 1, "operands share one column");
        let rows = operands
            .iter()
            .map(|n| n.y.to_bits())
            .collect::<HashSet<_>>();
        assert_eq!(rows.len(), operands.len(), "each operand gets its own row");
    }

    #[test]
    fn relayout_restores_the_circuit_after_nodes_are_dragged() {
        let mut mandala = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let placed: Vec<(NodeId, f64, f64)> =
            mandala.nodes().iter().map(|n| (n.id, n.x, n.y)).collect();
        // Drag every node somewhere arbitrary, as a hand edit would.
        for (index, (id, _, _)) in placed.iter().enumerate() {
            mandala.move_to(*id, 17.0 * index as f64, -9.0 * index as f64);
        }
        mandala.relayout();
        // Coordinates return to exactly the layout a fresh load produces, and
        // the structure is untouched.
        for (id, x, y) in placed {
            let node = mandala.node(id).expect("node survives relayout");
            assert_eq!((node.x, node.y), (x, y));
        }
        assert_eq!(mandala.to_rebis().unwrap(), "([\"m\"] \"a\" \"b\")");
    }

    #[test]
    fn unary_source_chains_become_a_straight_run_of_stages() {
        // A chain of single-child forms is a row of components, one per column:
        // every node in its own column, all on the same row.
        let mandala = Mandala::from_rebis("(^ '(^ \"x\"))").unwrap();
        let columns = mandala
            .nodes()
            .iter()
            .map(|node| node.x.to_bits())
            .collect::<HashSet<_>>();
        assert_eq!(
            columns.len(),
            mandala.nodes().len(),
            "each nesting level is its own column"
        );
        let rows = mandala
            .nodes()
            .iter()
            .map(|node| node.y.to_bits())
            .collect::<HashSet<_>>();
        assert_eq!(rows.len(), 1, "a unary chain stays on one row");
    }

    // ── viewport ───────────────────────────────────────────────────────────

    #[test]
    fn world_and_screen_are_inverses() {
        let v = View {
            tx: -140.0,
            ty: 62.5,
            zoom: 2.5,
        };
        let (wx, wy) = v.to_world(300.0, 200.0);
        let (sx, sy) = v.to_screen(wx, wy);
        assert!((sx - 300.0).abs() < 1e-9 && (sy - 200.0).abs() < 1e-9);
    }

    #[test]
    fn panning_is_unbounded() {
        let mut v = View::new();
        for _ in 0..1_000 {
            v.pan(900.0, -700.0);
        }
        assert_eq!(v.tx, 900_000.0);
        assert_eq!(v.ty, -700_000.0);
        assert_eq!(v.zoom, 1.0);
    }

    #[test]
    fn zoom_keeps_the_point_under_the_cursor() {
        let mut v = View::new();
        let (cx, cy) = (410.0, 275.0);
        let before = v.to_world(cx, cy);
        for factor in [1.2, 1.2, 0.8, 1.5, 0.5] {
            v.zoom_at(cx, cy, factor);
            let after = v.to_world(cx, cy);
            assert!(
                (after.0 - before.0).abs() < 1e-9 && (after.1 - before.1).abs() < 1e-9,
                "cursor drifted: {before:?} -> {after:?}"
            );
        }
    }

    #[test]
    fn zoom_is_clamped_both_ways() {
        let mut v = View::new();
        for _ in 0..200 {
            v.zoom_at(0.0, 0.0, 1.5);
        }
        assert_eq!(v.zoom, View::MAX_ZOOM);
        for _ in 0..400 {
            v.zoom_at(0.0, 0.0, 0.5);
        }
        assert_eq!(v.zoom, View::MIN_ZOOM);
    }

    // ── hit testing ────────────────────────────────────────────────────────

    #[test]
    fn hit_finds_a_circle_only_inside_its_radius() {
        let mut m = Mandala::new();
        let a = m.add(Form::Prompt, "a", 100.0, 100.0);
        assert_eq!(m.hit(100.0, 100.0), Some(a));
        assert_eq!(m.hit(100.0 + NODE_R - 1.0, 100.0), Some(a));
        assert_eq!(m.hit(100.0 + NODE_R + 1.0, 100.0), None);
        assert_eq!(m.hit(100.0 + NODE_R, 100.0 + NODE_R), None);
    }

    #[test]
    fn hit_uses_the_square_box() {
        let mut m = Mandala::new();
        let s = m.add(Form::Square, "", 0.0, 0.0);
        assert_eq!(m.hit(NODE_R - 1.0, NODE_RY - 1.0), Some(s));
        assert_eq!(m.hit(0.0, NODE_RY + 1.0), None);
    }

    #[test]
    fn hit_prefers_the_shape_on_top() {
        let mut m = Mandala::new();
        m.add(Form::Prompt, "under", 0.0, 0.0);
        let over = m.add(Form::Prompt, "over", 10.0, 0.0);
        assert_eq!(m.hit(5.0, 0.0), Some(over));
    }

    #[test]
    fn hit_misses_empty_canvas() {
        let mut m = Mandala::new();
        m.add(Form::Prompt, "a", 0.0, 0.0);
        assert_eq!(m.hit(500.0, 500.0), None);
    }
}
