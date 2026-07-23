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
//! A node's children are the nodes whose arrows point at it, in the order the
//! arrows were drawn. [`Mandala::to_rebis`] folds that into source and
//! [`Mandala::from_rebis`] unfolds source back onto the canvas; every form
//! round-trips.
//!
//! The whiteboard alphabet is a *rendering* of this, not a restriction on it:
//! prompts, symbols, and combining forms use the `o-[]-o` outlines; source
//! sigils—including `^`—are their own drawn shapes; every child edge is an
//! arrow ([`Form::shape`]).
//!
//! This module is pure and std-only — no UI, no rendering, no I/O — so the
//! editor front-end is a thin shell over it.

use std::collections::HashSet;
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
    fn accepts(self, n: usize) -> bool {
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

impl Form {
    /// Forms that are *placed* on the canvas, in palette order, with a default
    /// text payload.
    ///
    /// `Forward` and `Backflow` are deliberately absent: they are created by
    /// drawing an arrow between two shapes ([`Mandala::flow`]), because the
    /// arrow and the node are the same idea. They remain full forms in every
    /// other respect — loaded, rendered and generated like the rest.
    pub const ALL: &'static [(&'static str, fn() -> Form, &'static str)] = &[
        ("o prompt", || Form::Prompt, "prompt"),
        ("◇ symbol", || Form::Symbol, "x"),
        ("[] square", || Form::Square, ""),
        ("( ) compose", || Form::Compose, ""),
        ("$ concat", || Form::Concat, ""),
        ("call", || Form::Call, "f"),
        ("~ function", || Form::Function(vec!["x".into()]), "f"),
        ("# import", || Form::Import, "std/flow"),
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
            Form::Square | Form::Compose | Form::Call | Form::Program => Shape::Square,
        }
    }

    pub fn arity(&self) -> Arity {
        match self {
            Form::Prompt | Form::Symbol | Form::Import => Arity::Exactly(0),
            Form::Quote | Form::Unquote | Form::Invert | Form::Function(_) => Arity::Exactly(1),
            Form::Forward | Form::Backflow => Arity::Exactly(2),
            Form::Square => Arity::AtLeast(1),
            Form::Program => Arity::AtLeast(2),
            Form::Concat | Form::Compose | Form::Call => Arity::Any,
        }
    }

    /// Whether the form's text payload is meaningful (and so editable).
    pub fn uses_text(&self) -> bool {
        matches!(
            self,
            Form::Prompt | Form::Symbol | Form::Import | Form::Call | Form::Function(_)
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
            Form::Function(_) => "function",
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
    /// are outlines ([`Shape::Circle`], [`Shape::Square`], [`Shape::Diamond`]).
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
            Shape::Circle | Shape::Square | Shape::Diamond | Shape::Arrow => &[],
        }
    }

    /// The four corners of the diamond, in node-local coordinates.
    pub fn diamond_points() -> [(f32, f32); 4] {
        let r = NODE_R as f32;
        [(0.0, -r), (r, 0.0), (0.0, r), (-r, 0.0)]
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
            Shape::Square => dx.abs() <= NODE_R && dy.abs() <= NODE_RY,
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Arrow {
    pub from: NodeId,
    pub to: NodeId,
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
    /// A form has the wrong number of incoming arrows.
    WrongArity {
        id: NodeId,
        form: Form,
        want: Arity,
        got: usize,
    },
    /// `Program` groups top-level forms and cannot be nested.
    NestedProgram(NodeId),
}

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

    /// Draw an arrow, making `from` the next child of `to`. Duplicates and
    /// self-arrows are ignored, so the UI can call this without guarding.
    pub fn connect(&mut self, from: NodeId, to: NodeId) {
        if from == to || !self.has(from) || !self.has(to) {
            return;
        }
        let arrow = Arrow { from, to };
        if !self.arrows.contains(&arrow) {
            self.arrows.push(arrow);
        }
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
        self.connect(from, id);
        self.connect(to, id);
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
        self.nodes
            .iter()
            .rev()
            .find_map(|n| n.shape().contains(x - n.x, y - n.y).then_some(n.id))
    }

    /// Children of `id`: the nodes whose arrows point at it, in draw order.
    pub fn children(&self, id: NodeId) -> Vec<NodeId> {
        self.arrows
            .iter()
            .filter(|a| a.to == id)
            .map(|a| a.from)
            .collect()
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
        self.render(root, true, &mut on_path)
    }

    fn render(
        &self,
        id: NodeId,
        at_root: bool,
        on_path: &mut HashSet<NodeId>,
    ) -> Result<String, MandalaError> {
        if !on_path.insert(id) {
            return Err(MandalaError::Cycle);
        }
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
            parts.push(self.render(kid, false, on_path)?);
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

/// Horizontal gap between depth columns, and vertical gap between siblings.
const COL: f64 = 210.0;
const ROW: f64 = 105.0;

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
            Expr::Program(v) => (Form::Program, String::new(), v.iter().collect()),
        };

        let id = self.add(form, text, 0.0, 0.0);
        depths.push((id, depth));
        for kid in kids {
            let child = self.build(kid, depth + 1, depths);
            self.connect(child, id);
        }
        id
    }

    /// Place nodes in columns by depth (deepest on the left, root on the
    /// right), stacking siblings within each column.
    fn layout(&mut self, depths: &[(NodeId, usize)]) {
        let max_depth = depths.iter().map(|(_, d)| *d).max().unwrap_or(0);
        let mut per_column: Vec<usize> = vec![0; max_depth + 1];
        for (id, depth) in depths {
            let row = per_column[*depth];
            per_column[*depth] += 1;
            let x = 90.0 + (max_depth - depth) as f64 * COL;
            let y = 80.0 + row as f64 * ROW;
            self.move_to(*id, x, y);
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
    fn the_rest_use_the_whiteboard_alphabet() {
        assert_eq!(Form::Prompt.shape(), Shape::Circle);
        for f in [Form::Square, Form::Compose, Form::Call, Form::Program] {
            assert_eq!(f.shape(), Shape::Square, "{}", f.name());
        }
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
        assert_eq!(distinct.len(), 7, "expected ◇ $ ~ # ' , ^ to be drawn");
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
        for s in [Shape::Circle, Shape::Square, Shape::Diamond] {
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
            "compose", "call", "function", "program",
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
    fn loaded_nodes_get_distinct_positions() {
        let m = Mandala::from_rebis("([\"m\"] \"a\" \"b\")").unwrap();
        let mut seen = HashSet::new();
        for n in m.nodes() {
            assert!(
                seen.insert((n.x.to_bits(), n.y.to_bits())),
                "two nodes share a position"
            );
        }
        let sq = m.nodes().iter().find(|n| n.form == Form::Square).unwrap();
        assert!(m
            .nodes()
            .iter()
            .filter(|n| n.id != sq.id)
            .all(|n| n.x < sq.x));
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
